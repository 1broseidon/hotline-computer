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
    child: Arc<Mutex<Option<TerminalProcess>>>,
}
struct TerminalProcess {
    pid: u32,
    task: tokio::task::JoinHandle<()>,
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
    pub async fn select(&self, job_id: Option<&str>) -> Result<(), String> {
        {
            let _guard = self.child.lock().await;
            let directory = self.home.join(".toad");
            std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
            let temporary = directory.join("observer-view.tmp");
            std::fs::write(
                &temporary,
                serde_json::to_vec(&job_id).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            std::fs::rename(temporary, directory.join("observer-view.json"))
                .map_err(|e| e.to_string())?;
        }
        self.show(true).await
    }

    pub async fn show(&self, explicit: bool) -> Result<(), String> {
        if !explicit && self.opened.load(Ordering::Relaxed) {
            return Ok(());
        }
        self.opened.store(false, Ordering::Relaxed);
        let mut active = self.child.lock().await;
        let directory = self.home.join(".toad");
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let socket = directory.join("alacritty.sock");
        if active
            .as_ref()
            .is_none_or(|process| process.task.is_finished())
        {
            if let Some(process) = active.take() {
                let _ = process.task.await;
            }
            match std::fs::remove_file(&socket) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("remove stale observer socket: {error}")),
            }
            let log =
                std::fs::File::create(directory.join("terminal.log")).map_err(|e| e.to_string())?;
            let mut child = tokio::process::Command::new("alacritty")
                .arg("--daemon")
                .arg("--socket")
                .arg(&socket)
                .env("DISPLAY", &self.display)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::from(log))
                .kill_on_drop(true)
                .spawn()
                .map_err(|e| {
                    self.opened.store(false, Ordering::Relaxed);
                    format!("start Alacritty observer daemon: {e}")
                })?;
            *active = Some(TerminalProcess {
                pid: child.id().expect("spawned terminal daemon"),
                task: tokio::spawn(async move {
                    let _ = child.wait().await;
                }),
            });
        }
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::os::unix::net::UnixStream::connect(&socket).is_err() {
            if active
                .as_ref()
                .is_some_and(|process| process.task.is_finished())
                || tokio::time::Instant::now() >= deadline
            {
                self.opened.store(false, Ordering::Relaxed);
                return Err(
                    "Alacritty daemon did not become ready; inspect ~/.toad/terminal.log".into(),
                );
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        if let Some(window) = crate::x11::windows(&self.display)?
            .iter()
            .find(|w| w.class.to_lowercase().contains("toadterminal"))
        {
            if explicit {
                crate::x11::activate(&self.display, &window.id)?;
            }
            self.opened.store(true, Ordering::Relaxed);
            return Ok(());
        }
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        let request = tokio::process::Command::new("alacritty")
            .args(["msg", "--socket"])
            .arg(&socket)
            .args([
                "create-window",
                "--title",
                "Toad Terminal",
                "--class",
                "ToadTerminal",
                "--working-directory",
            ])
            .arg(&self.home)
            .arg("-e")
            .arg(executable)
            .arg("observe")
            .arg(&self.home)
            .env("DISPLAY", &self.display)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output();
        let response = tokio::time::timeout(std::time::Duration::from_secs(2), request)
            .await
            .map_err(|_| "Alacritty observer IPC timed out".to_owned())?
            .map_err(|e| format!("Alacritty observer IPC: {e}"))?;
        if !response.status.success() {
            return Err(format!(
                "Alacritty observer IPC failed: {}",
                String::from_utf8_lossy(&response.stderr)
            ));
        }
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if let Some(window) = crate::x11::windows(&self.display)?
                .iter()
                .find(|w| w.class.to_lowercase().contains("toadterminal"))
            {
                if explicit {
                    crate::x11::activate(&self.display, &window.id)?;
                }
                self.opened.store(true, Ordering::Relaxed);
                return Ok(());
            }
            if active
                .as_ref()
                .is_some_and(|process| process.task.is_finished())
                || tokio::time::Instant::now() >= deadline
            {
                return Err("Alacritty did not open an observer window; the job is independent. Inspect ~/.toad/terminal.log".into());
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
    pub async fn pid(&self) -> Option<u32> {
        self.child
            .lock()
            .await
            .as_ref()
            .filter(|process| !process.task.is_finished())
            .map(|process| process.pid)
    }
    pub async fn shutdown(&self) {
        if let Some(process) = self.child.lock().await.take() {
            process.task.abort();
            let _ = process.task.await;
        }
    }
}

pub fn run(home: &Path) -> Result<(), String> {
    // Input belongs to shell write. A person typing in the observer must not
    // create echoed text that could be mistaken for an executed command.
    unsafe {
        let mut settings: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(0, &mut settings) == 0 {
            settings.c_lflag &= !(libc::ECHO | libc::ECHONL);
            libc::tcsetattr(0, libc::TCSANOW, &settings);
        }
    }
    let root = home.join(".toad/jobs");
    let mut seen: BTreeMap<String, (u64, String)> = BTreeMap::new();
    let mut stdout = std::io::stdout().lock();
    let mut selection: Option<String> = None;
    let mut initialized = false;
    let mut missing_reported = false;
    loop {
        let selected = std::fs::read(home.join(".toad/observer-view.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Option<String>>(&bytes).ok())
            .flatten();
        if !initialized || selected != selection {
            selection = selected;
            initialized = true;
            missing_reported = false;
            seen.clear();
            writeln!(stdout, "\x1b[2J\x1b[H\x1b[1;32mToad Terminal\x1b[0m\r\nCommands run through tools. This window shows their output.\r\nClose or reopen it without stopping any job.\r\nView: {}\r\n", selection.as_deref().unwrap_or("all retained jobs")).map_err(|e| e.to_string())?;
        }
        let mut paths: Vec<_> = std::fs::read_dir(&root)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect();
        paths.sort();
        if let Some(selected) = &selection {
            paths.retain(|path| {
                path.file_name().and_then(|name| name.to_str()) == Some(selected.as_str())
            });
            if paths.is_empty() && !missing_reported {
                writeln!(
                    stdout,
                    "This job is no longer retained. Choose another job from the desktop bar.\r"
                )
                .map_err(|e| e.to_string())?;
                missing_reported = true;
            }
        }
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
                    "\r\n\x1b[{}m[{}: {}, elapsed {:.1}s, exit {}, signal {}{}]\x1b[0m\r",
                    if record.exit_code == Some(0) { 32 } else { 33 },
                    record.id,
                    record.state,
                    record
                        .finished_at
                        .unwrap_or_else(crate::jobs::now)
                        .saturating_sub(record.started_at) as f64
                        / 1000.0,
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
