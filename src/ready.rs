//! When a run is ready: a desktop run when its window is up, a web run when
//! its port answers. The agent gets the window and a screenshot, or the
//! output that says why it never came, instead of a job ID to poll.
//!
//! A window being up is not the document being open: an app shows a splash,
//! a start screen or an empty document first. So a desktop run may say what
//! ready means for it, a part of the title its window has once it is what
//! the run is for ([`title`]), and is ready only when such a window is up.
//! A run that says nothing is ready at its first window, and the answer says
//! that is all it is: which window, its title, and that it proves the app
//! started, not what it shows. Either way the window answered with is the
//! one up when the answer is made, so a splash replaced while the screen
//! settles is not the window named.
use crate::{App, manifest::Kind, x11};
use base64::Engine;
use rmcp::model::ContentBlock;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{LazyLock, Mutex};

/// The longest part of a title a run may wait for.
pub const TITLE_LIMIT: usize = 200;
/// How much of a window's title is looked at, and said back. A title is
/// whatever the app sets, and an app may set a long one.
const TITLE_SEEN: usize = 1024;
/// How many of a run's windows an answer lists.
const WINDOWS_LISTED: usize = 10;

#[derive(Clone)]
struct Pending {
    kind: Kind,
    port: Option<u16>,
    url: Option<String>,
    before: HashSet<String>,
    /// For a desktop run: part of the title of the window it is for.
    title: Option<String>,
}

/// Checks a title a desktop run waits for: plain text, matched as it is
/// within a window's title, ignoring case. Never a pattern language, so what
/// a manifest or a call says costs no more to match than its length.
pub fn title(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > TITLE_LIMIT {
        return Err(format!(
            "ready_title must be 1 to {TITLE_LIMIT} characters of the title the run's window has when it is ready, such as the document's file name"
        ));
    }
    if value.chars().any(char::is_control) {
        return Err("ready_title must be plain text, without control characters".into());
    }
    Ok(value.to_owned())
}

/// The part of a title that is looked at and said back.
fn seen(title: &str) -> String {
    title.chars().take(TITLE_SEEN).collect()
}

/// Whether a window's title has `wanted` in it, ignoring case.
fn titled(title: &str, wanted: &str) -> bool {
    seen(title).to_lowercase().contains(&wanted.to_lowercase())
}

/// The windows a run has opened: its own processes' windows, and, for a
/// window that names no process, any that was not there before it started.
fn of_run<'a>(
    windows: &'a [x11::Window],
    family: &HashSet<u32>,
    before: &HashSet<String>,
) -> Vec<&'a x11::Window> {
    windows
        .iter()
        .filter(|w| match w.pid {
            Some(pid) => family.contains(&pid),
            None => !before.contains(&w.id),
        })
        .collect()
}

/// The window an answer names, of a run's windows now: one with the title
/// asked for, when a title was; otherwise the focused one, or else the one
/// opened last. Nothing about any app in particular.
fn chosen<'a>(windows: &[&'a x11::Window], title: Option<&str>) -> Option<&'a x11::Window> {
    match title {
        Some(wanted) => windows
            .iter()
            .rev()
            .find(|w| titled(&w.title, wanted))
            .copied(),
        None => windows
            .iter()
            .find(|w| w.focused)
            .or_else(|| windows.last())
            .copied(),
    }
}

/// A window as an answer says it: its title cut to what is looked at.
fn described(window: &x11::Window) -> Value {
    let mut value = serde_json::to_value(window).unwrap_or(Value::Null);
    value["title"] = json!(seen(&window.title));
    value
}

fn listed(windows: &[&x11::Window]) -> Vec<Value> {
    windows
        .iter()
        .take(WINDOWS_LISTED)
        .map(|w| json!({"id": w.id, "title": seen(&w.title)}))
        .collect()
}

static PENDING: LazyLock<Mutex<HashMap<String, Pending>>> = LazyLock::new(Default::default);

/// Windows present before a desktop run starts, so a new one can be told apart.
pub fn windows_now(app: &App) -> HashSet<String> {
    x11::windows(&app.config.display)
        .map(|w| w.into_iter().map(|w| w.id).collect())
        .unwrap_or_default()
}

pub fn remember(
    job: &str,
    kind: Kind,
    url: Option<String>,
    before: HashSet<String>,
    title: Option<String>,
) {
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
                title,
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
/// out. Wait until two looks a moment apart are the same, for at most 15 s,
/// and never past `until` by more than the two looks it takes.
async fn settle(app: &App, until: tokio::time::Instant) {
    use sha2::{Digest, Sha256};
    let looks = tokio::time::Instant::now() + std::time::Duration::from_millis(2100);
    let deadline =
        (tokio::time::Instant::now() + std::time::Duration::from_secs(15)).min(until.max(looks));
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

/// Waits up to `wait_ms` for the run behind `job` to be ready. `title`, when
/// given, is what ready means for a desktop run from now on, in place of
/// what its manifest said.
pub async fn wait(
    app: &App,
    job: &str,
    wait_ms: u64,
    title: Option<String>,
) -> Result<(Value, Vec<ContentBlock>), String> {
    let pending = {
        let mut known = PENDING.lock().map_err(|e| e.to_string())?;
        let pending = known.get_mut(job).ok_or_else(|| {
            format!("job {job} is not a desktop or web run started with shell run")
        })?;
        if let Some(title) = title {
            if pending.kind != Kind::Desktop {
                return Err("ready_title is for a desktop run, whose window it names".into());
            }
            pending.title = Some(title);
        }
        pending.clone()
    };
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(wait_ms);
    let mut last_seen: Vec<Value> = Vec::new();
    loop {
        let record = app.jobs.status(job).await?;
        match pending.kind {
            Kind::Desktop => {
                let windows = x11::windows(&app.config.display)?;
                let family = record.pid.map(descendants).unwrap_or_default();
                let mine = of_run(&windows, &family, &pending.before);
                last_seen = listed(&mine);
                if chosen(&mine, pending.title.as_deref()).is_some() {
                    settle(app, deadline).await;
                    // The window there now, which may not be the one that
                    // was first: a splash goes, a title changes.
                    let windows = x11::windows(&app.config.display)?;
                    let family = record.pid.map(descendants).unwrap_or_default();
                    let mine = of_run(&windows, &family, &pending.before);
                    last_seen = listed(&mine);
                    if let Some(window) = chosen(&mine, pending.title.as_deref()) {
                        return answer(app, job, window, &mine, pending.title.as_deref());
                    }
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
            let mut value = json!({
                "ready": false,
                "job_id": job,
                "still_starting": true,
                "output_tail": tail(app, job).await,
                "next": "still building or starting: shell ready with this job_id waits again",
            });
            if let Some(wanted) = &pending.title {
                value["waiting_for"] = json!(format!(
                    "a window of this run whose title contains {wanted:?}"
                ));
                value["windows"] = json!(last_seen);
                if !last_seen.is_empty() {
                    value["next"] = json!(
                        "the run has windows, listed with their titles, but none has that title yet: shell ready with this job_id waits again; if a dialog or start screen is in the way, capture shows it"
                    );
                }
            }
            return Ok((value, vec![]));
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

/// A desktop run's answer once its window is up: the window, a screenshot,
/// and what that window being up does and does not show.
fn answer(
    app: &App,
    job: &str,
    window: &x11::Window,
    mine: &[&x11::Window],
    title: Option<&str>,
) -> Result<(Value, Vec<ContentBlock>), String> {
    let picture = x11::picture(&app.config.display, Some(window.bounds), Some(1568))?;
    let mut value = json!({
        "ready": true,
        "job_id": job,
        "window": described(window),
        "screenshot": picture.describe(),
    });
    match title {
        Some(wanted) => {
            value["ready_by"] = json!("title");
            value["ready_title"] = json!(wanted);
            value["next"] = json!(
                "its window has the title asked for, and the screenshot shows it now; capture for its accessibility tree, input to use it"
            );
        }
        None => {
            value["ready_by"] = json!("first window");
            value["caution"] = json!(format!(
                "Ready means only that this run has a window up, titled {:?}. That shows the app started, not that it opened what you meant: a start screen, a splash or an empty document is a window too. Look at the title and the screenshot before going on, and give the run ready_title (part of the title its window has once it is right, such as the file's name) so ready waits for that window.",
                seen(&window.title)
            ));
            value["next"] = json!(
                "check the title and the screenshot; capture for its accessibility tree, input to use it"
            );
        }
    }
    if mine.len() > 1 {
        value["windows"] = json!(listed(mine));
    }
    let image = ContentBlock::image(
        base64::engine::general_purpose::STANDARD.encode(&picture.png),
        "image/png",
    );
    Ok((value, vec![image]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(id: &str, title: &str, pid: Option<u32>, focused: bool) -> x11::Window {
        x11::Window {
            id: id.into(),
            title: title.into(),
            class: String::new(),
            bounds: [0, 0, 100, 100],
            focused,
            pid,
            maximized: false,
            minimum_size: [1, 1],
        }
    }

    #[test]
    fn a_title_is_plain_text_of_bounded_length() {
        assert_eq!(title("  drawing.svg ").unwrap(), "drawing.svg");
        assert_eq!(title("(a|b)*.blend").unwrap(), "(a|b)*.blend");
        assert!(title("").is_err());
        assert!(title("   ").is_err());
        assert!(title(&"x".repeat(TITLE_LIMIT + 1)).is_err());
        assert!(title(&"é".repeat(TITLE_LIMIT)).is_ok());
        assert!(title(r"line\nbreak").is_ok());
        assert!(title("line\nbreak").is_err());
        assert!(title("tab\there").is_err());
    }

    #[test]
    fn a_title_matches_as_text_within_the_title_ignoring_case() {
        assert!(titled("drawing.svg - Inkscape", "DRAWING.SVG"));
        assert!(titled("Blender [/home/agent/scene.blend]", "scene.blend"));
        assert!(!titled("Blender [Unsaved]", "scene.blend"));
        // Never a pattern: these are only characters.
        assert!(!titled("scene-blend", "scene.blend"));
        assert!(!titled("anything", ".*"));
        assert!(titled("a .* b", ".*"));
        // Only the start of a very long title is looked at.
        let long = format!("{}scene.blend", "x".repeat(TITLE_SEEN));
        assert!(!titled(&long, "scene.blend"));
        assert_eq!(seen(&long).chars().count(), TITLE_SEEN);
    }

    #[test]
    fn a_run_answers_with_its_window_now_and_waits_for_the_title_asked_for() {
        let family = HashSet::from([10, 11]);
        let before = HashSet::from(["0x1".to_owned()]);
        let windows = vec![
            window("0x1", "Old terminal", None, false),
            window("0x2", "Someone else's app", Some(99), true),
            window("0x3", "Inkscape", Some(10), false),
            window("0x4", "Untitled", None, false),
        ];
        let mine = of_run(&windows, &family, &before);
        let ids: Vec<&str> = mine.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(
            ids,
            ["0x3", "0x4"],
            "its own, and new ones that name no process"
        );

        // Unfocused, the one opened last is named.
        assert_eq!(chosen(&mine, None).unwrap().id, "0x4");
        // The splash gone and the main window focused: that is the one.
        let windows = vec![
            window("0x5", "drawing.svg - Inkscape", Some(11), true),
            window("0x6", "Tip of the day", Some(11), false),
        ];
        let mine = of_run(&windows, &family, &before);
        assert_eq!(chosen(&mine, None).unwrap().id, "0x5");
        // A title asked for is waited on, whichever is focused.
        assert_eq!(chosen(&mine, Some("tip of")).unwrap().id, "0x6");
        assert!(chosen(&mine, Some("scene.blend")).is_none());
        assert!(chosen(&[], None).is_none());
    }

    #[test]
    fn an_answer_lists_a_bounded_number_of_windows_with_bounded_titles() {
        let many: Vec<x11::Window> = (0..30)
            .map(|n| window(&format!("0x{n}"), &"t".repeat(5000), Some(1), false))
            .collect();
        let refs: Vec<&x11::Window> = many.iter().collect();
        let list = listed(&refs);
        assert_eq!(list.len(), WINDOWS_LISTED);
        assert_eq!(list[0]["title"].as_str().unwrap().len(), TITLE_SEEN);
        assert_eq!(
            described(&many[0])["title"].as_str().unwrap().len(),
            TITLE_SEEN
        );
    }
}
