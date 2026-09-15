//! The terminal only observes retained jobs; closing it never owns their lifetime.
use crate::jobs::Record;
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct Observer {
    home: PathBuf,
    display: String,
    opened: Arc<AtomicBool>,
    child: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}
impl Observer {
    pub fn new(home: &Path, display: &str) -> Self {
        Self {
            home: home.into(),
            display: display.into(),
            opened: Arc::new(AtomicBool::new(false)),
            child: Arc::new(Mutex::new(None)),
        }
    }
    pub async fn show(&self, explicit: bool) -> Result<(), String> {
        if !explicit && self.opened.swap(true, Ordering::Relaxed) {
            return Ok(());
        }
        self.opened.store(true, Ordering::Relaxed);
        let mut active = self.child.lock().await;
        if let Some(handle) = active.as_ref()
            && !handle.is_finished()
        {
            if !explicit {
                return Ok(());
            }
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
            loop {
                if let Some(window) = crate::x11::windows(&self.display)?
                    .iter()
                    .find(|w| w.class.to_lowercase().contains("toadterminal"))
                {
                    crate::x11::activate(&self.display, &window.id)?;
                    return Ok(());
                }
                if handle.is_finished() {
                    break;
                }
                if tokio::time::Instant::now() >= deadline {
                    handle.abort();
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }
        if let Some(handle) = active.take() {
            let _ = handle.await;
        }
        std::fs::create_dir_all(self.home.join(".toad")).map_err(|e| e.to_string())?;
        let log = std::fs::File::create(self.home.join(".toad/terminal.log"))
            .map_err(|e| e.to_string())?;
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut child = tokio::process::Command::new("alacritty")
            .args(["--title", "Toad Terminal", "--class", "ToadTerminal", "-e"])
            .arg(executable)
            .arg("observe")
            .arg(&self.home)
            .env("DISPLAY", &self.display)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(log))
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                self.opened.store(false, Ordering::Relaxed);
                format!("open Alacritty observer: {e}")
            })?;
        *active = Some(tokio::spawn(async move {
            let _ = child.wait().await;
        }));
        if explicit {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
            loop {
                if crate::x11::windows(&self.display)?
                    .iter()
                    .any(|w| w.class.to_lowercase().contains("toadterminal"))
                {
                    break;
                }
                if active.as_ref().is_some_and(|handle| handle.is_finished())
                    || tokio::time::Instant::now() >= deadline
                {
                    return Err(
                        "Alacritty did not open a window; inspect ~/.toad/terminal.log".into(),
                    );
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }
        Ok(())
    }
    pub async fn shutdown(&self) {
        if let Some(task) = self.child.lock().await.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

pub fn run(home: &Path) -> Result<(), String> {
    let root = home.join(".toad/jobs");
    let mut seen: BTreeMap<String, (u64, String)> = BTreeMap::new();
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout,"\x1b[1;32mToad Terminal\x1b[0m\r\nCommands run through tools. This window shows their output.\r\nClose or reopen it without stopping any job.\r\n").map_err(|e|e.to_string())?;
    loop {
        let mut paths: Vec<_> = std::fs::read_dir(&root)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect();
        paths.sort();
        seen.retain(|id, _| {
            paths
                .iter()
                .any(|path| path.file_name().and_then(|name| name.to_str()) == Some(id.as_str()))
        });
        for path in paths {
            let Ok(bytes) = std::fs::read(path.join("record.json")) else {
                continue;
            };
            let Ok(record) = serde_json::from_slice::<Record>(&bytes) else {
                continue;
            };
            let output = path.join("output.log");
            let length = std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0);
            let entry = seen.entry(record.id.clone()).or_insert_with(|| {
                let _ = writeln!(
                    stdout,
                    "\r\n\x1b[1;36m[{}] {}\x1b[0m\r\n{}\r\n$ {} {}\r",
                    record.id,
                    record.label,
                    record.cwd,
                    quoted(&record.command),
                    record
                        .args
                        .iter()
                        .map(|s| quoted(s))
                        .collect::<Vec<_>>()
                        .join(" ")
                );
                let offset = length.saturating_sub(16384);
                if offset > 0 {
                    let _ = writeln!(
                        stdout,
                        "[Earlier output is retained; read it with shell read.]\r"
                    );
                }
                (offset, String::new())
            });
            if length > entry.0 {
                let Ok(mut file) = std::fs::File::open(output) else {
                    continue;
                };
                file.seek(SeekFrom::Start(entry.0))
                    .map_err(|e| e.to_string())?;
                let mut bytes = Vec::new();
                file.take(65536)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                stdout.write_all(&bytes).map_err(|e| e.to_string())?;
                entry.0 += bytes.len() as u64;
            }
            if entry.1 != record.state && record.finished() {
                writeln!(
                    stdout,
                    "\r\n\x1b[{}m[{}: {}, exit {}, signal {}{}]\x1b[0m\r",
                    if record.exit_code == Some(0) { 32 } else { 33 },
                    record.id,
                    record.state,
                    record
                        .exit_code
                        .map_or_else(|| "—".into(), |code| code.to_string()),
                    record
                        .signal
                        .map_or_else(|| "—".into(), |signal| signal.to_string()),
                    if record.truncated {
                        ", output capped"
                    } else {
                        ""
                    }
                )
                .map_err(|e| e.to_string())?;
            }
            entry.1 = record.state;
        }
        stdout.flush().map_err(|e| e.to_string())?;
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
fn quoted(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/_-.".contains(&b))
    {
        value.into()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}
