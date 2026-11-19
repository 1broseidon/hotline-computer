//! The first split boot of a home the single-user computer used. Run by the
//! runner, as the agent, since only the agent reads its home.
//!
//! - The saved logins in `~/.hotline/logins` (cookie files the desk's Use
//!   Cookies left there in plain text) are dropped.
//! - The general browser's cookies for those sites, and for every login's
//!   sites once the desk has delivered them, are cleared.
//! - Snapshots made before the split hold the old browser profile, saved
//!   logins and job output; they are quarantined: `snapshot_load` refuses
//!   them until the person answers `POST /migration/answer` with `clean`
//!   (those parts taken out), `delete`, or `keep` (left as they are, still
//!   quarantined). Nothing is erased without that answer.
//!
//! The record is `~/.hotline/split.json`. It is the agent's file, as the old
//! snapshots are: quarantine keeps them from being restored by the computer's
//! own tools, and does not hide them from a job, which could always unpack a
//! tarball in its home. That is why the answer the desk asks for is `clean`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::App;

const RECORD: &str = ".hotline/split.json";
/// Set when this start was the home's first split boot: only then are the
/// logins' sites cleared from the general browser as they are delivered.
static FIRST_BOOT: AtomicBool = AtomicBool::new(false);
/// What a snapshot made before the split carries that a split computer
/// keeps out of the agent's reach, or that held values in plain text.
const OLD_PARTS: [&str; 3] = [".hotline/browser", ".hotline/logins", ".hotline/jobs"];

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub v: u32,
    /// Unix milliseconds of the first split boot.
    pub at: u64,
    /// Snapshots made before the split, not restored until cleaned.
    pub quarantined: Vec<String>,
    /// Saved logins dropped from `~/.hotline/logins`.
    pub logins_dropped: usize,
    /// Sites whose cookies the general browser no longer has.
    pub sites_cleared: Vec<String>,
    /// Sites still to clear, once the browser can be reached.
    #[serde(default)]
    pub sites_pending: Vec<String>,
    /// What the person answered about the old snapshots, if anything.
    #[serde(default)]
    pub answer: Option<String>,
}

fn record_path(home: &Path) -> PathBuf {
    home.join(RECORD)
}

pub async fn read(home: &Path) -> Option<Record> {
    let bytes = tokio::fs::read(record_path(home)).await.ok()?;
    serde_json::from_slice(&bytes).ok()
}

async fn write(home: &Path, record: &Record) -> Result<(), String> {
    let path = record_path(home);
    if let Some(directory) = path.parent() {
        tokio::fs::create_dir_all(directory)
            .await
            .map_err(|error| error.to_string())?;
    }
    let staged = path.with_extension("json.tmp");
    tokio::fs::write(
        &staged,
        serde_json::to_vec_pretty(record).unwrap_or_default(),
    )
    .await
    .map_err(|error| error.to_string())?;
    tokio::fs::rename(&staged, &path)
        .await
        .map_err(|error| error.to_string())
}

/// Every cookie domain a saved login file names, as sites.
fn domains(value: &Value, into: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(domain)) = map.get("domain") {
                let site = crate::logins::site_of(domain);
                if !site.is_empty() {
                    into.insert(site);
                }
            }
            map.values().for_each(|value| domains(value, into));
        }
        Value::Array(values) => values.iter().for_each(|value| domains(value, into)),
        _ => {}
    }
}

/// On the runner's start: nothing when the home has been through it, else
/// the migration above. A home never used before goes through it at once,
/// with nothing to do.
pub async fn on_boot(app: &App) -> Result<Record, String> {
    let home = app.config.home.clone();
    if let Some(record) = read(&home).await {
        return Ok(record);
    }
    let logins = home.join(".hotline/logins");
    let mut sites = BTreeSet::new();
    let mut dropped = 0;
    if let Ok(mut entries) = tokio::fs::read_dir(&logins).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if let Ok(bytes) = tokio::fs::read(entry.path()).await
                && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
            {
                domains(&value, &mut sites);
            }
            dropped += 1;
        }
    }
    match tokio::fs::remove_dir_all(&logins).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("drop {}: {error}", logins.display())),
    }
    let mut quarantined = Vec::new();
    if let Ok(mut entries) = tokio::fs::read_dir(home.join(".hotline/snapshots")).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if let Some(name) = entry.file_name().to_string_lossy().strip_suffix(".tar.gz") {
                quarantined.push(name.to_owned());
            }
        }
    }
    quarantined.sort();
    let mut record = Record {
        v: 1,
        at: crate::jobs::now(),
        quarantined,
        logins_dropped: dropped,
        sites_cleared: Vec::new(),
        sites_pending: sites.into_iter().collect(),
        answer: None,
    };
    write(&home, &record).await?;
    FIRST_BOOT.store(true, Ordering::Relaxed);
    clear_pending(app, &mut record).await;
    if dropped > 0 || !record.quarantined.is_empty() {
        eprintln!(
            "hotline-computer: first split boot: {} saved logins dropped, {} snapshots quarantined",
            dropped,
            record.quarantined.len()
        );
    }
    Ok(record)
}

/// After a delivery: the logins' sites join what is cleared from the general
/// browser, once, on the first split boot's record.
pub async fn on_delivery(app: &App, catalog: &[Value]) {
    if !FIRST_BOOT.load(Ordering::Relaxed) {
        return;
    }
    let Some(mut record) = read(&app.config.home).await else {
        return;
    };
    let mut fresh = false;
    for entry in catalog {
        if entry["kind"] != "login" {
            continue;
        }
        for site in entry["sites"].as_array().into_iter().flatten() {
            let Some(host) = site
                .as_str()
                .map(crate::bridge::host_of)
                .filter(|host| !host.is_empty())
            else {
                continue;
            };
            if !record.sites_cleared.contains(&host) && !record.sites_pending.contains(&host) {
                record.sites_pending.push(host);
                fresh = true;
            }
        }
    }
    if fresh {
        clear_pending(app, &mut record).await;
    }
}

/// Clears the pending sites from the general browser: through DevTools when
/// it runs, else in its cookie file before it next starts.
async fn clear_pending(app: &App, record: &mut Record) {
    if record.sites_pending.is_empty() {
        return;
    }
    let sites = std::mem::take(&mut record.sites_pending);
    let running = app
        .browser
        .open_pages()
        .await
        .is_ok_and(|pages| !pages.is_empty());
    let cleared = if running {
        app.browser.forget_cookies(&sites).await.map(|_| ())
    } else {
        forget_offline(&app.config.home, &sites).await
    };
    match cleared {
        Ok(()) => record.sites_cleared.extend(sites),
        Err(error) => {
            eprintln!("hotline-computer: clearing old sites from the browser: {error}");
            record.sites_pending = sites;
        }
    }
    let _ = write(&app.config.home, record).await;
}

const FORGET: &str = r"
import sqlite3, sys
db = sqlite3.connect(sys.argv[1])
for site in sys.argv[2:]:
    db.execute('DELETE FROM cookies WHERE host_key = ? OR host_key = ? OR host_key LIKE ?', (site, '.' + site, '%.' + site))
db.commit()
";

async fn forget_offline(home: &Path, sites: &[String]) -> Result<(), String> {
    let cookies = home.join(".hotline/browser/Default/Cookies");
    if !cookies.exists() {
        return Ok(());
    }
    let output = tokio::process::Command::new("python3")
        .arg("-I")
        .arg("-c")
        .arg(FORGET)
        .arg(&cookies)
        .args(sites)
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|error| format!("python3: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

/// Why a snapshot is not restored, when it is quarantined.
pub async fn refusal(home: &Path, name: &str) -> Option<String> {
    let record = read(home).await?;
    record.quarantined.iter().any(|quarantined| quarantined == name).then(|| {
        format!(
            "snapshot {name:?} was made before this computer kept signed-in sites apart: it holds the old browser profile, saved logins and job output. It is not restored until the person answers at the desk whether to clean those out of it or delete it."
        )
    })
}

/// A snapshot saved or deleted under a quarantined name is no longer the
/// old one.
pub async fn release(home: &Path, name: &str) {
    if let Some(mut record) = read(home).await
        && record
            .quarantined
            .iter()
            .any(|quarantined| quarantined == name)
    {
        record.quarantined.retain(|quarantined| quarantined != name);
        let _ = write(home, &record).await;
    }
}

/// `GET /migration`, from the runner's side.
pub async fn status(app: &App) -> Value {
    let Some(record) = read(&app.config.home).await else {
        return json!({"state": "none"});
    };
    let mut snapshots = Vec::new();
    for name in &record.quarantined {
        let size = tokio::fs::metadata(snapshot(&app.config.home, name))
            .await
            .map_or(0, |metadata| metadata.len());
        snapshots.push(json!({"name": name, "bytes": size}));
    }
    json!({
        "state": if record.quarantined.is_empty() { "done" } else { "pending" },
        "at": record.at,
        "snapshots": snapshots,
        "loginsDropped": record.logins_dropped,
        "sitesCleared": record.sites_cleared,
        "answer": record.answer,
    })
}

fn snapshot(home: &Path, name: &str) -> PathBuf {
    home.join(".hotline/snapshots")
        .join(format!("{name}.tar.gz"))
}

/// `POST /migration/answer {snapshots}`: the person's answer about the old
/// snapshots.
pub async fn answer(app: &App, snapshots: &str) -> Result<Value, String> {
    let home = app.config.home.clone();
    let mut record = read(&home)
        .await
        .ok_or("this computer has nothing to migrate")?;
    match snapshots {
        "keep" => {}
        "delete" => {
            for name in &record.quarantined {
                match tokio::fs::remove_file(snapshot(&home, name)).await {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(format!("delete snapshot {name}: {error}")),
                }
            }
            record.quarantined.clear();
        }
        "clean" => {
            for name in record.quarantined.clone() {
                let path = snapshot(&home, &name);
                if path.exists() {
                    clean(&path).await?;
                }
                record
                    .quarantined
                    .retain(|quarantined| *quarantined != name);
                write(&home, &record).await?;
            }
        }
        other => {
            return Err(format!("snapshots is clean, delete or keep, not {other:?}"));
        }
    }
    record.answer = Some(snapshots.to_owned());
    write(&home, &record).await?;
    Ok(status(app).await)
}

/// The snapshot rewritten without its old parts: unpacked beside itself,
/// those parts removed, packed again, and moved over the original.
async fn clean(path: &Path) -> Result<(), String> {
    let directory = path.parent().ok_or("a snapshot has a folder")?;
    let work = tempfile::Builder::new()
        .prefix(".cleaning-")
        .tempdir_in(directory)
        .map_err(|error| error.to_string())?;
    let tree = work.path().join("home");
    tokio::fs::create_dir(&tree)
        .await
        .map_err(|error| error.to_string())?;
    run_tar(&[
        "xzf".as_ref(),
        path.as_os_str(),
        "-C".as_ref(),
        tree.as_os_str(),
    ])
    .await?;
    for part in OLD_PARTS {
        match tokio::fs::remove_dir_all(tree.join(part)).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("clean {part}: {error}")),
        }
    }
    let cleaned = work.path().join("cleaned.tar.gz");
    run_tar(&[
        "czf".as_ref(),
        cleaned.as_os_str(),
        "-C".as_ref(),
        tree.as_os_str(),
        ".".as_ref(),
    ])
    .await?;
    tokio::fs::rename(&cleaned, path)
        .await
        .map_err(|error| error.to_string())
}

async fn run_tar(arguments: &[&std::ffi::OsStr]) -> Result<(), String> {
    let output = tokio::process::Command::new("tar")
        .args(arguments)
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|error| format!("tar: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "tar: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;

    fn app(home: &Path) -> App {
        App::new(Config {
            addr: String::new(),
            token: None,
            home: home.to_owned(),
            display: ":97".to_owned(),
            screen: "1920x1080".to_owned(),
        })
    }

    async fn state(app: &App, action: &str, name: &str) -> Result<String, String> {
        crate::tools::call(
            app,
            "state",
            json!({"action": action, "name": name}),
            "test",
        )
        .await
        .map(|blocks| {
            blocks[0]
                .as_text()
                .map(|t| t.text.clone())
                .unwrap_or_default()
        })
    }

    /// A home the single-user computer left: a saved login in plain text,
    /// and a snapshot holding it and the browser profile.
    async fn old_home() -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        let logins = home.path().join(".hotline/logins");
        std::fs::create_dir_all(&logins).unwrap();
        std::fs::write(
            logins.join("github-Default.json"),
            r#"{"cookies":[{"name":"user_session","value":"hl-canary-session","domain":".github.com"}]}"#,
        )
        .unwrap();
        let profile = home.path().join(".hotline/browser/Default");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(profile.join("Cookies"), "hl-canary-cookie-db").unwrap();
        std::fs::create_dir_all(home.path().join("src")).unwrap();
        std::fs::write(home.path().join("src/notes.txt"), "kept").unwrap();
        let app = app(home.path());
        state(&app, "snapshot_save", "before").await.unwrap();
        std::fs::remove_dir_all(home.path().join("src")).unwrap();
        home
    }

    // snapshot_load extracts with GNU tar's --overwrite, as the image does.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn an_old_snapshot_is_not_restored_until_cleaned() {
        let home = old_home().await;
        let app = app(home.path());
        let record = on_boot(&app).await.unwrap();
        assert_eq!(record.quarantined, vec!["before".to_owned()]);
        assert_eq!(record.logins_dropped, 1);
        assert!(
            !home.path().join(".hotline/logins").exists(),
            "N1b: no saved login is left"
        );
        // The Cookies file here is no database: clearing it is left pending.
        assert_eq!(on_boot(&app).await.unwrap().quarantined, record.quarantined);
        let refused = state(&app, "snapshot_load", "before").await.unwrap_err();
        assert!(refused.contains("not restored until"), "{refused}");
        assert!(!home.path().join("src").exists());
        assert_eq!(status(&app).await["state"], "pending");
        // Keeping it keeps it quarantined.
        answer(&app, "keep").await.unwrap();
        assert!(state(&app, "snapshot_load", "before").await.is_err());
        let cleaned = answer(&app, "clean").await.unwrap();
        assert_eq!(cleaned["state"], "done", "{cleaned}");
        let profile = home.path().join(".hotline/browser");
        std::fs::remove_dir_all(&profile).unwrap();
        state(&app, "snapshot_load", "before").await.unwrap();
        assert_eq!(
            std::fs::read_to_string(home.path().join("src/notes.txt")).unwrap(),
            "kept",
            "the person's files come back"
        );
        assert!(
            !home.path().join(".hotline/logins").exists() && !profile.exists(),
            "N2: the old logins and browser profile do not"
        );
    }

    #[tokio::test]
    async fn deleting_answers_for_the_old_snapshots_and_a_new_home_has_nothing_to_do() {
        let home = old_home().await;
        let app = app(home.path());
        on_boot(&app).await.unwrap();
        let deleted = answer(&app, "delete").await.unwrap();
        assert_eq!(deleted["state"], "done");
        assert!(
            !home
                .path()
                .join(".hotline/snapshots/before.tar.gz")
                .exists()
        );
        assert!(answer(&app, "shred").await.is_err());

        let fresh = tempfile::tempdir().unwrap();
        let app = super::tests::app(fresh.path());
        let record = on_boot(&app).await.unwrap();
        assert!(record.quarantined.is_empty() && record.logins_dropped == 0);
        assert_eq!(status(&app).await["state"], "done");
    }
}
