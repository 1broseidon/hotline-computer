//! When a run is ready: a desktop run when its window is up, a web run when
//! its port answers. The agent gets the window and a screenshot, or the
//! output that says why it never came, instead of a job ID to poll.
use crate::{App, manifest::Kind, x11};
use base64::Engine;
use rmcp::model::ContentBlock;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{LazyLock, Mutex};

#[derive(Clone)]
struct Pending {
    kind: Kind,
    port: Option<u16>,
    url: Option<String>,
    before: HashSet<String>,
}

static PENDING: LazyLock<Mutex<HashMap<String, Pending>>> = LazyLock::new(Default::default);

/// Windows present before a desktop run starts, so a new one can be told apart.
pub fn windows_now(app: &App) -> HashSet<String> {
    x11::windows(&app.config.display)
        .map(|w| w.into_iter().map(|w| w.id).collect())
        .unwrap_or_default()
}

pub fn remember(job: &str, kind: Kind, url: Option<String>, before: HashSet<String>) {
    let port = url
        .as_deref()
        .and_then(|u| u.rsplit(':').next())
        .and_then(|p| p.parse().ok());
    if let Ok(mut pending) = PENDING.lock() {
        pending.insert(
            job.to_owned(),
            Pending {
                kind,
                port,
                url,
                before,
            },
        );
    }
}

/// The process and everything it started, from /proc.
fn descendants(root: u32) -> HashSet<u32> {
    let mut children: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<u32>().ok())
            else {
                continue;
            };
            let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
                continue;
            };
            // The command name may hold spaces; fields resume after its closing parenthesis.
            let Some(rest) = stat.rsplit_once(')').map(|(_, r)| r) else {
                continue;
            };
            if let Some(parent) = rest.split_whitespace().nth(1).and_then(|p| p.parse().ok()) {
                children.entry(parent).or_default().push(pid);
            }
        }
    }
    let mut all = HashSet::from([root]);
    let mut queue = vec![root];
    while let Some(pid) = queue.pop() {
        for child in children.get(&pid).into_iter().flatten() {
            if all.insert(*child) {
                queue.push(*child);
            }
        }
    }
    all
}

/// A window is up before its content is: a webview loads, a toolkit lays
/// out. Wait until two looks a moment apart are the same, for at most 15 s.
async fn settle(app: &App) {
    use sha2::{Digest, Sha256};
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut previous = None;
    let mut same = 0;
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        let Ok(shot) = x11::screenshot(&app.config.display) else {
            return;
        };
        let digest = Sha256::digest(&shot.rgba);
        if previous.as_ref() == Some(&digest) {
            same += 1;
            if same >= 2 {
                return;
            }
        } else {
            same = 0;
        }
        previous = Some(digest);
    }
}

async fn tail(app: &App, job: &str) -> String {
    let Ok(record) = app.jobs.status(job).await else {
        return String::new();
    };
    let start = (record.output_bytes as u64).saturating_sub(3000);
    app.jobs
        .read(job, start, 3000)
        .await
        .map(|o| crate::jobs::plain(&o.output))
        .unwrap_or_default()
}

/// Waits up to `wait_ms` for the run behind `job` to be ready.
pub async fn wait(
    app: &App,
    job: &str,
    wait_ms: u64,
) -> Result<(Value, Vec<ContentBlock>), String> {
    let pending = PENDING
        .lock()
        .map_err(|e| e.to_string())?
        .get(job)
        .cloned()
        .ok_or_else(|| format!("job {job} is not a desktop or web run started with shell run"))?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(wait_ms);
    loop {
        let record = app.jobs.status(job).await?;
        match pending.kind {
            Kind::Desktop => {
                let windows = x11::windows(&app.config.display)?;
                let family = record.pid.map(descendants).unwrap_or_default();
                let found = windows.iter().find(|w| match w.pid {
                    Some(pid) => family.contains(&pid),
                    None => !pending.before.contains(&w.id),
                });
                if let Some(window) = found {
                    settle(app).await;
                    let picture =
                        x11::picture(&app.config.display, Some(window.bounds), Some(1568))?;
                    let value = json!({
                        "ready": true,
                        "job_id": job,
                        "window": window,
                        "screenshot": picture.describe(),
                        "next": "the screenshot shows it now; capture for its accessibility tree, input to use it",
                    });
                    let image = ContentBlock::image(
                        base64::engine::general_purpose::STANDARD.encode(&picture.png),
                        "image/png",
                    );
                    return Ok((value, vec![image]));
                }
            }
            Kind::Web => {
                if let Some(port) = pending.port
                    && tokio::net::TcpStream::connect(("localhost", port))
                        .await
                        .is_ok()
                {
                    return Ok((
                        json!({"ready": true, "job_id": job, "url": pending.url, "next": "browser navigate to the url"}),
                        vec![],
                    ));
                }
            }
            Kind::Task => {
                return Ok((json!({"ready": record.finished(), "job_id": job}), vec![]));
            }
        }
        if record.finished() {
            let what = if pending.kind == Kind::Desktop {
                "opening a window"
            } else {
                "answering on its port"
            };
            return Err(format!(
                "job {job} ended ({}, exit code {:?}) before {what}. Its last output:\n{}",
                record.state,
                record.exit_code,
                tail(app, job).await
            ));
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok((
                json!({
                    "ready": false,
                    "job_id": job,
                    "still_starting": true,
                    "output_tail": tail(app, job).await,
                    "next": "still building or starting: shell ready with this job_id waits again",
                }),
                vec![],
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}
