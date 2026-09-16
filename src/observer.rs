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

/// The observer's window class, which the desktop places in the right third.
pub const OBSERVER_CLASS: &str = "ToadTerminal";
/// The person's shell, which the desktop keeps in the bottom third of the
/// observer's column.
pub const SHELL_CLASS: &str = "ToadShell";
/// Alacritty's configuration, kept with the image rather than in the home,
/// so a home the teammate keeps across containers never shadows it.
pub const ALACRITTY_CONFIG: &str = "/etc/toad-computer/alacritty.toml";
/// Alacritty options for the person's window alone: a visible block cursor
/// in the bar's foreground, where the observer paints its cursor away.
pub const SHELL_OPTIONS: &[&str] = &[
    "cursor.style.shape=\"Block\"",
    "colors.cursor.cursor=\"#d6d6d9\"",
    "colors.cursor.text=\"#141416\"",
];

fn window_of_class(display: &str, class: &str) -> Result<Option<String>, String> {
    let wanted = class.to_lowercase();
    Ok(crate::x11::windows(display)?
        .into_iter()
        .find(|w| w.class.to_lowercase().contains(&wanted))
        .map(|w| w.id))
}

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
        let socket = self.daemon(&mut active).await?;
        if let Some(window) = window_of_class(&self.display, OBSERVER_CLASS)? {
            if explicit {
                crate::x11::activate(&self.display, &window)?;
            }
            self.opened.store(true, Ordering::Relaxed);
            return Ok(());
        }
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        self.create_window(
            &socket,
            "Observer",
            OBSERVER_CLASS,
            &[],
            &[
                executable.as_os_str(),
                "observe".as_ref(),
                self.home.as_os_str(),
            ],
        )
        .await?;
        let window = self
            .wait_for_window(&active, OBSERVER_CLASS)
            .await
            .map_err(|error| {
                format!("{error}; the job is independent. Inspect ~/.toad/terminal.log")
            })?;
        if explicit {
            crate::x11::activate(&self.display, &window)?;
        }
        self.opened.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// A shell for the person: an interactive bash in the workspace's
    /// environment, in its own small window. What they run there is theirs,
    /// not a job: it is not retained, and the teammate sees only the screen.
    pub async fn open_shell(&self) -> Result<(), String> {
        let mut active = self.child.lock().await;
        let socket = self.daemon(&mut active).await?;
        if let Some(window) = window_of_class(&self.display, SHELL_CLASS)? {
            return crate::x11::activate(&self.display, &window);
        }
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        // The shared config hides the cursor, since nobody types in the
        // observer. The person's window gets one back.
        self.create_window(
            &socket,
            "Terminal",
            SHELL_CLASS,
            SHELL_OPTIONS,
            &[
                executable.as_os_str(),
                "shell".as_ref(),
                self.home.as_os_str(),
            ],
        )
        .await?;
        let window = self.wait_for_window(&active, SHELL_CLASS).await?;
        crate::x11::activate(&self.display, &window)
    }

    /// The Alacritty daemon every terminal window comes from, started if it
    /// is not running; returns its socket once it answers.
    async fn daemon(&self, active: &mut Option<TerminalProcess>) -> Result<PathBuf, String> {
        let directory = self.home.join(".toad");
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let socket = directory.join("alacritty.sock");
        // A daemon `toad-computer open` started answers here already; it
        // is used rather than replaced.
        if active.is_none() && std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            return Ok(socket);
        }
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
            let mut child = tokio::process::Command::from(daemon_command(&socket, &self.display)?)
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
        Ok(socket)
    }

    async fn create_window(
        &self,
        socket: &Path,
        title: &str,
        class: &str,
        options: &[&str],
        command: &[&std::ffi::OsStr],
    ) -> Result<(), String> {
        let request = tokio::process::Command::from(window_request(
            socket,
            &self.display,
            title,
            class,
            options,
            &self.home,
            command,
        ))
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
        Ok(())
    }

    async fn wait_for_window(
        &self,
        active: &Option<TerminalProcess>,
        class: &str,
    ) -> Result<String, String> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if let Some(window) = window_of_class(&self.display, class)? {
                return Ok(window);
            }
            if active
                .as_ref()
                .is_some_and(|process| process.task.is_finished())
                || tokio::time::Instant::now() >= deadline
            {
                return Err(format!("Alacritty did not open a {class} window"));
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

/// The rc file the person's shell reads instead of the system's and the
/// home's: the prompt, history kept with the computer, and a hook for the
/// operator's own `~/.bashrc`.
pub const BASHRC: &str = include_str!("../assets/bashrc");

/// `toad-computer shell <home> [folder]`: the person's interactive shell.
/// It starts in the folder asked for, else in the mounted workspace when
/// there is one, with the environment the teammate prepared there, and
/// then it is bash with Toad's rc file.
pub fn shell(home: &Path, at: Option<&Path>) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let rc = home.join(".toad/bashrc");
    std::fs::create_dir_all(home.join(".toad")).map_err(|e| e.to_string())?;
    std::fs::write(&rc, BASHRC).map_err(|e| format!("{}: {e}", rc.display()))?;
    let workspace = home.join("workspace");
    let cwd = match at {
        Some(folder) if folder.is_dir() => folder.to_path_buf(),
        _ if workspace.is_dir() => workspace,
        _ => home.to_path_buf(),
    };
    let mut command = std::process::Command::new(interactive_bash());
    command.current_dir(&cwd);
    match crate::workspace::environment(home, &cwd) {
        Ok(environment) => {
            command.envs(environment);
        }
        Err(error) => eprintln!("workspace environment not applied: {error}"),
    }
    println!("{MUTED}`exit` to close{RESET}");
    let error = command
        .arg("--noprofile")
        .arg("--rcfile")
        .arg(&rc)
        .arg("-i")
        .exec();
    Err(format!("start bash: {error}"))
}

/// `toad-computer open <folder>`: a fresh terminal for the person in that
/// folder. The browser's "Show in folder" and `xdg-open` on a folder land
/// here through the image's desktop entry, since the computer has no file
/// manager and a shell in the folder is what a person wants from one. The
/// terminal comes from the same Alacritty daemon as the observer, started
/// here if nothing has started it yet.
pub fn open(home: &Path, target: &Path) -> Result<(), String> {
    let folder =
        std::fs::canonicalize(target).map_err(|error| format!("{}: {error}", target.display()))?;
    if !folder.is_dir() {
        return Err(format!("{} is not a folder", folder.display()));
    }
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".to_owned());
    let directory = home.join(".toad");
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let socket = directory.join("alacritty.sock");
    if std::os::unix::net::UnixStream::connect(&socket).is_err() {
        let _ = std::fs::remove_file(&socket);
        daemon_command(&socket, &display)?
            .spawn()
            .map_err(|e| format!("start Alacritty daemon: {e}"))?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::os::unix::net::UnixStream::connect(&socket).is_err() {
            if std::time::Instant::now() >= deadline {
                return Err(
                    "Alacritty daemon did not become ready; inspect ~/.toad/terminal.log".into(),
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    let title = match folder.strip_prefix(home) {
        Ok(inside) if inside.as_os_str().is_empty() => "~".to_owned(),
        Ok(inside) => format!("~/{}", inside.display()),
        Err(_) => folder.display().to_string(),
    };
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let response = window_request(
        &socket,
        &display,
        &title,
        SHELL_CLASS,
        SHELL_OPTIONS,
        &folder,
        &[
            executable.as_os_str(),
            "shell".as_ref(),
            home.as_os_str(),
            folder.as_os_str(),
        ],
    )
    .output()
    .map_err(|e| format!("Alacritty IPC: {e}"))?;
    if !response.status.success() {
        return Err(format!(
            "Alacritty IPC failed: {}",
            String::from_utf8_lossy(&response.stderr)
        ));
    }
    Ok(())
}

/// The Alacritty daemon every terminal window comes from, as a command:
/// its socket, the shared config, and its log under `~/.toad`.
fn daemon_command(socket: &Path, display: &str) -> Result<std::process::Command, String> {
    let log =
        std::fs::File::create(socket.with_file_name("terminal.log")).map_err(|e| e.to_string())?;
    let mut daemon = std::process::Command::new("alacritty");
    daemon.arg("--daemon").arg("--socket").arg(socket);
    if Path::new(ALACRITTY_CONFIG).is_file() {
        daemon.args(["--config-file", ALACRITTY_CONFIG]);
    }
    daemon
        .env("DISPLAY", display)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log));
    Ok(daemon)
}

/// A window from the daemon, as a command: its title, class and options,
/// the folder it starts in, and what runs inside it.
fn window_request(
    socket: &Path,
    display: &str,
    title: &str,
    class: &str,
    options: &[&str],
    cwd: &Path,
    command: &[&std::ffi::OsStr],
) -> std::process::Command {
    let mut request = std::process::Command::new("alacritty");
    request.args(["msg", "--socket"]).arg(socket).args([
        "create-window",
        "--title",
        title,
        "--class",
        class,
    ]);
    for option in options {
        request.args(["-o", option]);
    }
    request
        .arg("--working-directory")
        .arg(cwd)
        .arg("-e")
        .args(command)
        .env("DISPLAY", display)
        .stdin(Stdio::null());
    request
}

/// The bash a person types into: the system's, found on this process's own
/// PATH. A prepared workspace puts Nixpkgs' bash first on its PATH, and that
/// one is built without readline: no history, no completion, and a prompt
/// whose `\[` and `\]` print as text. The workspace environment still applies
/// inside; only the shell binary comes from outside it.
fn interactive_bash() -> std::path::PathBuf {
    let system = std::path::PathBuf::from("/bin/bash");
    std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path)
                .filter(|dir| !dir.starts_with("/nix/store"))
                .map(|dir| dir.join("bash"))
                .find(|candidate| candidate.is_file())
                .unwrap_or_else(|| system.clone())
        })
        .unwrap_or(system)
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
            stdout
                .write_all(banner(selection.as_deref()).as_bytes())
                .map_err(|e| e.to_string())?;
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
                    "{MUTED}This job is no longer retained. Choose another from the bar's jobs list.{RESET}\r"
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
                let _ = stdout.write_all(header(&record).as_bytes());
                let offset = length.saturating_sub(16384);
                if offset > 0 {
                    let _ = writeln!(
                        stdout,
                        "{MUTED}Earlier output is retained; the tools read it with shell read.{RESET}\r"
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
                stdout
                    .write_all(footer(&record).as_bytes())
                    .map_err(|e| e.to_string())?;
            }
            entry.1 = record.state;
        }
        stdout.flush().map_err(|e| e.to_string())?;
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
const RESET: &str = "\x1b[0m";
const INK: &str = "\x1b[1;38;2;232;232;234m";
const INK_2: &str = "\x1b[38;2;170;170;174m";
const MUTED: &str = "\x1b[38;2;125;125;130m";
const OK: &str = "\x1b[38;2;134;214;156m";
const WARN: &str = "\x1b[38;2;240;194;122m";

/// The top of the window: its name, and a note only when it is narrowed
/// to one job, since then the rest are missing on purpose.
fn banner(selection: Option<&str>) -> String {
    let showing = match selection {
        Some(_) => format!("{MUTED}Showing one job, chosen from the bar's jobs list.{RESET}\r\n"),
        None => String::new(),
    };
    format!("\x1b[2J\x1b[H\x1b[?25l{INK}Observer{RESET}\r\n{showing}")
}

/// A job's opening: its name, then the facts the tools need, then the command.
fn header(record: &Record) -> String {
    let command = std::iter::once(quoted(&record.command))
        .chain(record.args.iter().map(|s| quoted(s)))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "\r\n{INK}▎ {}{RESET}\r\n{MUTED}  job {} · started {} · {}{RESET}\r\n{INK_2}  $ {}{RESET}\r\n",
        record.label,
        record.id,
        clock(record.started_at),
        record.cwd,
        command
    )
}

/// A job's close: how it ended, named again so a long output still reads.
fn footer(record: &Record) -> String {
    let ok = record.exit_code == Some(0);
    let elapsed = record
        .finished_at
        .unwrap_or_else(crate::jobs::now)
        .saturating_sub(record.started_at) as f64
        / 1000.0;
    let mut facts = vec![record.label.clone()];
    if !ok || record.state != "exited" {
        facts.push(record.state.replace('_', " "));
    }
    if let Some(code) = record.exit_code {
        facts.push(format!("exit {code}"));
    }
    if let Some(signal) = record.signal {
        facts.push(format!("signal {signal}"));
    }
    facts.push(format!("{elapsed:.1} s"));
    if record.truncated {
        facts.push("output capped".to_owned());
    }
    format!(
        "\r\n{}{} {}{RESET}\r\n",
        if ok { OK } else { WARN },
        if ok { "✓" } else { "✗" },
        facts.join(" · ")
    )
}

fn clock(epoch_ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(epoch_ms as i64)
        .map(|at| {
            at.with_timezone(&chrono::Local)
                .format("%H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| "—".to_owned())
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

#[cfg(test)]
mod tests {
    #[test]
    fn the_rc_file_sets_the_prompt_and_leaves_room_for_the_operators_own() {
        let rc = super::BASHRC;
        assert!(rc.contains("PS1='\\[\\e[38;2;127;196;240m\\]\\w\\[\\e[0m\\] $ '"));
        assert!(
            !rc.contains("export PS1"),
            "an exported prompt reaches sh as text"
        );
        assert!(rc.contains("HISTFILE=\"$HOME/.toad/shell_history\""));
        assert!(rc.contains("export SHELL=/bin/bash"));
        assert!(
            rc.trim_end()
                .ends_with("[ -r \"$HOME/.bashrc\" ] && . \"$HOME/.bashrc\"")
        );
        // Nothing here prints: the terminal shows a prompt, not prose.
        assert!(
            !rc.lines()
                .any(|line| line.trim_start().starts_with("echo "))
        );
    }

    #[test]
    fn the_persons_bash_is_never_the_store_one() {
        let bash = super::interactive_bash();
        assert!(!bash.starts_with("/nix/store"), "{}", bash.display());
        assert!(bash.is_absolute());
    }

    use super::*;

    fn record(label: &str, state: &str, exit_code: Option<i32>) -> Record {
        Record {
            id: "0800001a0a5ee4cb4-f789fa4d".into(),
            pid: None,
            label: label.into(),
            command: "bash".into(),
            args: vec!["-c".into(), "cargo test".into()],
            cwd: "/home/agent/src/app".into(),
            holder: "teammate".into(),
            state: state.into(),
            exit_code,
            signal: None,
            started_at: 1_789_489_000_000,
            finished_at: Some(1_789_489_002_500),
            error: None,
            output_bytes: 0,
            truncated: false,
            pty: false,
            request_id: None,
            fingerprint: String::new(),
            artifact_staging: None,
        }
    }

    #[test]
    fn a_job_is_named_first_and_identified_underneath() {
        let text = header(&record("Run the unit tests", "running", None));
        let lines: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
        assert!(lines[0].contains("▎ Run the unit tests"), "{text}");
        assert!(
            lines[1].contains("job 0800001a0a5ee4cb4-f789fa4d · started "),
            "{text}"
        );
        assert!(lines[1].contains("· /home/agent/src/app"), "{text}");
        assert!(lines[2].contains("$ bash -c 'cargo test'"), "{text}");
    }

    #[test]
    fn the_ending_names_the_job_again_and_says_how_it_went() {
        let ok = footer(&record("Run the unit tests", "exited", Some(0)));
        assert!(ok.contains("✓ Run the unit tests · exit 0 · 2.5 s"), "{ok}");
        assert!(ok.starts_with(&format!("\r\n{OK}")), "{ok}");
        let failed = footer(&record("Run the unit tests", "failed", Some(101)));
        assert!(
            failed.contains("✗ Run the unit tests · failed · exit 101 · 2.5 s"),
            "{failed}"
        );
        let cancelled = footer(&record("Serve the app", "cancelled", None));
        assert!(
            cancelled.contains("✗ Serve the app · cancelled · 2.5 s"),
            "{cancelled}"
        );
    }

    #[test]
    fn the_banner_is_the_name_and_nothing_more_unless_narrowed() {
        let all = banner(None);
        assert!(
            all.starts_with("\x1b[2J\x1b[H\x1b[?25l"),
            "clears and hides the cursor"
        );
        assert_eq!(all.lines().filter(|l| !l.is_empty()).count(), 1, "{all:?}");
        assert!(banner(Some("some-id")).contains("Showing one job"));
    }
}
