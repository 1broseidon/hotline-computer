//! A short, editable account of completed calls, not a security audit trail.
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::secrets::Secrets;

const KEEP: usize = 500;
const MAX_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Entry {
    pub ts: u64,
    pub tool: String,
    pub action: String,
    pub target: String,
    pub ok: bool,
}

fn short(value: &str) -> String {
    crate::jobs::plain(value)
        .chars()
        .filter(|c| !c.is_control())
        .take(160)
        .collect()
}

impl Entry {
    /// Only named display fields enter the log. Never fall back to a job's
    /// default label, which is its command, or serialize an argument/result.
    pub fn new(tool: &str, arguments: &Value) -> Self {
        let tool = if crate::tools::NAMES.contains(&tool) {
            tool
        } else {
            "unknown"
        };
        let supplied = arguments["action"].as_str().unwrap_or("exec");
        let allowed: &[&str] = match tool {
            "browser" => &[
                "navigate",
                "text",
                "links",
                "eval",
                "click_ref",
                "fill",
                "select",
                "check",
                "hover",
                "tabs",
                "tab_new",
                "tab_select",
                "tab_close",
                "upload",
                "dialog_accept",
                "dialog_dismiss",
                "downloads",
                "back",
                "forward",
                "reload",
            ],
            "input" => &[
                "click",
                "double_click",
                "right_click",
                "move",
                "drag",
                "scroll",
                "type",
                "key",
                "paste",
                "clipboard_read",
                "clipboard_write",
                "batch",
            ],
            "shell" => &[
                "exec", "start", "launch", "run", "ready", "list", "status", "read", "wait",
                "write", "cancel", "show",
            ],
            "windows" => &["list", "focus", "close", "maximize", "tile"],
            "files" => &["get", "put", "list", "download", "extract", "run"],
            "state" => &[
                "info",
                "guide",
                "catalog",
                "manifest",
                "prepare",
                "control",
                "release",
                "login_save",
                "login_load",
                "login_list",
                "login_delete",
                "snapshot_save",
                "snapshot_load",
                "snapshot_list",
                "snapshot_delete",
            ],
            _ => &[],
        };
        let action = if allowed.contains(&supplied) {
            supplied
        } else {
            "call"
        };
        let target = match (tool, action) {
            ("browser", "navigate" | "tab_new") | ("files", "download") => arguments["url"]
                .as_str()
                .and_then(|s| reqwest::Url::parse(s).ok())
                .filter(|u| matches!(u.scheme(), "http" | "https"))
                .and_then(|u| u.host_str().map(|host| format!("{host}{}", u.path())))
                .unwrap_or_default(),
            ("shell", "exec" | "start" | "launch" | "run") => {
                arguments["label"].as_str().unwrap_or("").to_owned()
            }
            // Key names only ("ctrl+s", "Enter"): never typed text.
            ("input", "key") => arguments["combo"]
                .as_str()
                .filter(|combo| combo.len() <= 24)
                .unwrap_or("")
                .to_owned(),
            ("files", "put") => arguments["path"]
                .as_str()
                .map(|path| path.replace("/home/agent/", "~/"))
                .unwrap_or_default(),
            _ => String::new(),
        };
        Self {
            ts: 0,
            tool: tool.into(),
            action: action.into(),
            target,
            ok: false,
        }
    }

    fn clean(&mut self, secrets: &Secrets) {
        // Redact before truncation so a long secret cannot leave a prefix.
        self.target = short(&secrets.redact(&self.target));
        self.tool = short(&secrets.redact(&self.tool));
        self.action = short(&secrets.redact(&self.action));
    }

    fn sentence(&self) -> String {
        let verb = match (self.tool.as_str(), self.action.as_str()) {
            ("browser", "navigate" | "tab_new") => "Opened",
            ("browser", "fill") if self.target.contains(" from sealed secret ") => "Filled",
            ("browser", "fill") | ("input", "type" | "paste") => "Typed into",
            (_, "click_ref" | "click" | "double_click" | "right_click") => "Clicked",
            (_, "scroll") => "Scrolled",
            ("shell", "exec" | "start" | "launch" | "run") => "Started",
            ("windows", "focus") => "Focused",
            ("windows", "close") => "Closed",
            ("windows", "tile") => "Tiled the windows",
            ("windows", "maximize") => "Maximized",
            ("input", "key") => "Pressed",
            ("input", "drag") => "Dragged",
            ("input", "clipboard_write") => "Copied to the clipboard",
            ("browser", "eval") => "Ran a script on the page",
            ("browser", "back") => "Went back",
            ("browser", "forward") => "Went forward",
            ("browser", "reload") => "Reloaded the page",
            ("browser", "hover") => "Pointed at",
            ("browser", "check") => "Checked",
            ("browser", "select") => "Chose an option in",
            ("browser", "upload") => "Attached a file",
            ("browser", "tab_close") => "Closed a tab",
            ("browser", "tab_select") => "Switched tabs",
            ("files", "put") => "Saved",
            ("files", "download") => "Downloaded",
            ("files", "extract") => "Unpacked an archive",
            ("state", "prepare") => "Prepared the workspace",
            ("capture", _) => "Captured the desktop",
            ("browser", "text") => "Read the page",
            _ => {
                return format!(
                    "{} {} {}",
                    short(&self.tool),
                    short(&self.action),
                    short(&self.target)
                )
                .trim()
                .to_owned();
            }
        };
        format!("{verb} {}", short(&self.target)).trim().to_owned()
    }
}

#[derive(Clone)]
pub struct Log {
    path: PathBuf,
    entries: Arc<Mutex<Option<VecDeque<Entry>>>>,
    secrets: Secrets,
}

impl Log {
    pub fn new(home: &Path, secrets: Secrets) -> Self {
        Self {
            path: home.join(".hotline/activity.jsonl"),
            entries: Arc::new(Mutex::new(None)),
            secrets,
        }
    }

    pub fn complete(&self, mut entry: Entry, ok: bool) -> std::io::Result<()> {
        entry.ts = crate::jobs::now();
        entry.ok = ok;
        let mut state = self.entries.lock().unwrap();
        let entries = state.get_or_insert_with(|| read(&self.path));
        entries.push_back(entry);
        while entries.len() > KEEP {
            entries.pop_front();
        }
        let directory = self.path.parent().expect("activity directory");
        std::fs::create_dir_all(directory)?;
        // Atomic replacement keeps a reader from seeing half a line, and
        // never follows an agent-created symlink at the destination.
        let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
        for entry in entries.iter_mut() {
            entry.clean(&self.secrets);
            serde_json::to_writer(&mut temporary, entry)?;
            temporary.write_all(b"\n")?;
        }
        temporary.persist(&self.path).map_err(|e| e.error)?;
        Ok(())
    }
}

fn read(path: &Path) -> VecDeque<Entry> {
    let mut entries = VecDeque::new();
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    else {
        return entries;
    };
    if !file
        .metadata()
        .is_ok_and(|m| m.is_file() && m.len() <= MAX_BYTES)
    {
        return entries;
    }
    let mut bytes = String::new();
    if file.take(MAX_BYTES).read_to_string(&mut bytes).is_err() {
        return entries;
    }
    for line in bytes.lines() {
        if let Ok(entry) = serde_json::from_str::<Entry>(line) {
            entries.push_back(entry);
            if entries.len() > KEEP {
                entries.pop_front();
            }
        }
    }
    entries
}

fn frame(entries: &VecDeque<Entry>, rows: usize, columns: usize, working: bool) -> String {
    let mut output = "\x1b[2J\x1b[H\x1b[?25l\x1b[97mActivity\x1b[0m\r\n".to_owned();
    for (index, entry) in entries
        .iter()
        .enumerate()
        .skip(entries.len().saturating_sub(rows.saturating_sub(3)))
    {
        let time = chrono::DateTime::from_timestamp_millis(entry.ts as i64)
            .map(|t| {
                t.with_timezone(&chrono::Local)
                    .format("%H:%M:%S")
                    .to_string()
            })
            .unwrap_or_default();
        let (color, marker) = if !entry.ok {
            (31, "✗")
        } else if working && index + 1 == entries.len() {
            (34, "●")
        } else {
            (32, "✓")
        };
        let sentence: String = entry
            .sentence()
            .chars()
            .take(columns.saturating_sub(12))
            .collect();
        output.push_str(&format!(
            "\x1b[90m{time} \x1b[{color}m{marker} \x1b[97m{sentence}\x1b[0m\r\n"
        ));
    }
    output.push_str("\x1b[90mDisplay only, not an audit log.\x1b[0m\r\n");
    output
}

pub fn run(home: &Path) -> Result<(), String> {
    // Like Observe, this window takes no typed input.
    unsafe {
        let mut term: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(0, &mut term) == 0 {
            term.c_lflag &= !(libc::ECHO | libc::ECHONL);
            libc::tcsetattr(0, libc::TCSANOW, &term);
        }
    }
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into());
    let mut previous = String::new();
    let mut output = std::io::stdout().lock();
    loop {
        let mut size: libc::winsize = unsafe { std::mem::zeroed() };
        unsafe {
            libc::ioctl(1, libc::TIOCGWINSZ, &mut size);
        }
        let entries = read(&home.join(".hotline/activity.jsonl"));
        let working = crate::x11::read_agent(&display).is_some_and(|agent| {
            !agent.task.is_empty()
                || crate::jobs::now().saturating_sub(agent.last_action_ms) < 10_000
        });
        let next = frame(
            &entries,
            usize::from(size.ws_row.max(4)),
            usize::from(size.ws_col.max(16)),
            working,
        );
        if next != previous {
            output
                .write_all(next.as_bytes())
                .and_then(|_| output.flush())
                .map_err(|e| e.to_string())?;
            previous = next;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn activity_omits_payloads_and_url_credentials_queries_fragments() {
        let args = json!({"action":"navigate", "url":"https://user:password@example.org/login?token=sensitive#secret", "text":"typed", "js":"javascript", "command":"rm -rf", "content":"file bytes"});
        let entry = Entry::new("browser", &args);
        assert_eq!(entry.target, "example.org/login");
        for tool in crate::tools::NAMES {
            let entry = Entry::new(
                tool,
                &json!({"action":"unknown-payload", "text":"typed", "command":"command line", "js":"javascript", "content":"bytes"}),
            );
            assert_eq!(entry.action, "call");
            assert!(entry.target.is_empty());
        }
        assert!(
            Entry::new("shell", &json!({"command":"never log this"}))
                .target
                .is_empty()
        );
    }

    #[test]
    fn activity_redacts_before_truncation_and_never_follows_a_log_symlink() {
        let directory = tempfile::tempdir().unwrap();
        let secrets = Secrets::default();
        let value = "private-value".repeat(20);
        secrets
            .replace(serde_json::from_value(json!({"TOKEN": value})).unwrap())
            .unwrap();
        let log = Log::new(directory.path(), secrets);
        std::fs::create_dir_all(log.path.parent().unwrap()).unwrap();
        let other = directory.path().join("unchanged");
        std::fs::write(&other, "leave this alone").unwrap();
        std::os::unix::fs::symlink(&other, &log.path).unwrap();
        log.complete(Entry::new("shell", &json!({"label":value})), false)
            .unwrap();
        assert_eq!(std::fs::read_to_string(other).unwrap(), "leave this alone");
        let bytes = std::fs::read_to_string(&log.path).unwrap();
        assert!(!bytes.contains("private-value"));
        assert!(!read(&log.path).front().unwrap().ok);
        assert!(frame(&read(&log.path), 10, 80, true).contains("\x1b[31m✗"));
    }

    #[test]
    fn activity_keeps_newest_500_and_removes_terminal_control_codes() {
        let directory = tempfile::tempdir().unwrap();
        let log = Log::new(directory.path(), Secrets::default());
        for n in 0..503 {
            log.complete(
                Entry::new("shell", &json!({"label":format!("\x1b[31mjob {n}\n" )})),
                true,
            )
            .unwrap();
        }
        let entries = read(&log.path);
        assert_eq!(entries.len(), 500);
        assert_eq!(entries.front().unwrap().target, "job 3");
        assert_eq!(entries.back().unwrap().target, "job 502");
        assert!(frame(&entries, 10, 80, true).contains("\x1b[34m●"));
        assert!(frame(&entries, 10, 80, false).contains("\x1b[32m✓"));
        assert!(frame(&entries, 10, 80, false).contains("Display only, not an audit log."));
    }
}
