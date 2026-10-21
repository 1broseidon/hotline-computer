//! The computer's own file manager and file picker: one page (`files.html`)
//! in a small Chromium app window, styled like the viewer. Browsing opens
//! from the folder handler; picking answers the desktop portal's
//! FileChooser, so any app that asks for a file gets this window.
//!
//! The page talks to `/fm/*` with a key minted at boot and kept in a file
//! only this user reads. It lists folders and opens files, which an agent
//! here can do anyway; it is never the bearer that guards the computer.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use axum::Json;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::oneshot;

use crate::App;

const PAGE: &str = include_str!("files.html");
/// The one window class each kind of window has, which the desktop places by:
/// a picker as a dialog over the app that asked, the manager as an app.
pub const PICKER_CLASS: &str = "HotlinePicker";
pub const MANAGER_CLASS: &str = "HotlineFiles";

/// What an app asked the picker for, as the page reads it.
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Request {
    pub mode: &'static str,
    pub title: String,
    pub accept_label: Option<String>,
    pub multiple: bool,
    pub directory: bool,
    pub filters: Vec<Filter>,
    pub current_filter: Option<usize>,
    pub current_name: Option<String>,
    pub current_folder: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Filter {
    pub name: String,
    pub patterns: Vec<String>,
    pub mimes: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub enum Answer {
    Chosen {
        paths: Vec<String>,
        filter: Option<usize>,
    },
    Cancelled,
}

struct Pending {
    request: Request,
    answer: oneshot::Sender<Answer>,
}

#[derive(Clone)]
pub struct Files(Arc<Inner>);

struct Inner {
    key: String,
    pending: Mutex<HashMap<String, Pending>>,
}

impl Default for Files {
    fn default() -> Self {
        Self(Arc::new(Inner {
            key: random_hex(16),
            pending: Mutex::new(HashMap::new()),
        }))
    }
}

/// Where the key waits for `hotline-computer open`, which the folder handler
/// runs in its own process.
pub fn key_file() -> PathBuf {
    std::env::temp_dir().join("hotline-computer/files.key")
}

impl Files {
    /// Leaves the key where this user, and no one else, can read it.
    pub fn publish(&self) -> Result<(), String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let path = key_file();
        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
        }
        let _ = std::fs::remove_file(&path);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        file.write_all(self.0.key.as_bytes())
            .map_err(|e| e.to_string())
    }

    fn admits(&self, presented: Option<&str>) -> bool {
        presented.is_some_and(|key| key == self.0.key)
    }

    /// Shows the picker and waits for the person or agent to answer it. A
    /// window that cannot open is a cancel, which an app already handles.
    pub async fn pick(&self, request: Request, port: u16) -> Answer {
        let id = random_hex(12);
        let (sender, receiver) = oneshot::channel();
        self.0.pending.lock().unwrap().insert(
            id.clone(),
            Pending {
                request,
                answer: sender,
            },
        );
        let fragment = format!("key={}&request={id}", self.0.key);
        if let Err(error) = launch(PICKER_CLASS, port, &fragment) {
            eprintln!("file picker: {error}");
            self.0.pending.lock().unwrap().remove(&id);
            return Answer::Cancelled;
        }
        receiver.await.unwrap_or(Answer::Cancelled)
    }

    fn answer(&self, id: &str, answer: Answer) -> bool {
        match self.0.pending.lock().unwrap().remove(id) {
            Some(pending) => pending.answer.send(answer).is_ok(),
            None => false,
        }
    }
}

/// Closes the picker that answered `id`. Chromium will not let an app window
/// close itself, so the page titles itself `Closing <id>` first and the
/// window is found by that, which stays exact with two pickers open.
fn close_picker(display: String, id: String) {
    tokio::spawn(async move {
        let title = format!("Closing {id}");
        for _ in 0..30 {
            if let Ok(windows) = crate::x11::windows(&display)
                && let Some(window) = windows.iter().find(|w| {
                    w.title == title && w.class.to_ascii_lowercase().contains("hotlinepicker")
                })
            {
                let _ = crate::x11::close(&display, &window.id);
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    });
}

impl Files {}

/// `hotline-computer open <folder>`: the file manager, in that folder. It
/// runs in its own process, so it reads the key the service left.
pub fn open_folder(target: &Path) -> Result<(), String> {
    let folder =
        std::fs::canonicalize(target).map_err(|error| format!("{}: {error}", target.display()))?;
    if !folder.is_dir() {
        return Err(format!("{} is not a folder", folder.display()));
    }
    let key = std::fs::read_to_string(key_file())
        .map_err(|_| "the computer's service is not running".to_owned())?;
    let port = port(&std::env::var("HOTLINE_COMPUTER_ADDR").unwrap_or_default());
    let fragment = format!(
        "key={}&mode=browse&path={}",
        key.trim(),
        encode(&folder.to_string_lossy(), false)
    );
    launch(MANAGER_CLASS, port, &fragment)
}

/// The port the service listens on, from its address.
pub fn port(address: &str) -> u16 {
    address
        .rsplit(':')
        .next()
        .and_then(|port| port.parse().ok())
        .unwrap_or(8787)
}

/// One Chromium per kind of window, each with its own profile: a second
/// launch hands its window to the running one, so it opens at once, and the
/// agent's browser and its tabs are never involved.
fn launch(class: &str, port: u16, fragment: &str) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let profile =
        std::env::temp_dir().join(format!("hotline-computer/{}", class.to_ascii_lowercase()));
    let size = if class == PICKER_CLASS {
        "920,620"
    } else {
        "1100,720"
    };
    std::process::Command::new("chromium")
        .args([
            format!("--user-data-dir={}", profile.display()),
            format!("--class={class}"),
            format!("--app=http://127.0.0.1:{port}/fm/ui#{fragment}"),
            format!("--window-size={size}"),
            "--force-renderer-accessibility".into(),
            "--no-first-run".into(),
            "--no-default-browser-check".into(),
            "--disable-gpu".into(),
            "--disable-background-networking".into(),
            "--disable-features=TranslateUI".into(),
            "--password-store=basic".into(),
            "--no-sandbox".into(),
            "--test-type".into(),
        ])
        .env_remove("HOTLINE_COMPUTER_TOKEN")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("start the {class} window: {error}"))
}

fn random_hex(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut buffer))
        .expect("the kernel's random source");
    buffer.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Percent-encodes a path for a URL fragment or a `file://` URI.
pub fn encode(text: &str, keep_slashes: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        let keep = byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~')
            || (keep_slashes && byte == b'/');
        if keep {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

// ---- the page's routes ---------------------------------------------------

fn key_of(headers: &HeaderMap) -> Option<&str> {
    headers.get("x-files-key").and_then(|v| v.to_str().ok())
}

fn refused(status: StatusCode, error: impl Into<String>) -> Response {
    (status, Json(json!({"error": error.into()}))).into_response()
}

pub async fn page() -> Html<&'static str> {
    Html(PAGE)
}

#[derive(Deserialize)]
pub struct Listing {
    path: String,
}

pub async fn list(
    State(app): State<App>,
    headers: HeaderMap,
    Query(query): Query<Listing>,
) -> Response {
    if !app.files.admits(key_of(&headers)) {
        return refused(
            StatusCode::UNAUTHORIZED,
            "this window's key is not this computer's",
        );
    }
    match listing(&app.config.home, Path::new(&query.path)) {
        Ok(body) => Json(body).into_response(),
        Err(error) => refused(StatusCode::UNPROCESSABLE_ENTITY, error),
    }
}

fn listing(home: &Path, path: &Path) -> Result<serde_json::Value, String> {
    let folder = std::fs::canonicalize(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => format!("{} does not exist", path.display()),
        std::io::ErrorKind::PermissionDenied => format!("{} cannot be opened", path.display()),
        _ => format!("{}: {error}", path.display()),
    })?;
    if !folder.is_dir() {
        return Err(format!("{} is not a folder", folder.display()));
    }
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&folder).map_err(|e| format!("{}: {e}", folder.display()))? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        let link = entry.file_type().is_ok_and(|kind| kind.is_symlink());
        // A link reads as what it points to; a broken one as a file.
        let metadata = std::fs::metadata(entry.path()).or_else(|_| entry.metadata());
        let (dir, size, modified) = metadata.map_or((false, 0, 0), |m| {
            let modified = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            (m.is_dir(), m.len(), modified)
        });
        entries.push(json!({
            "name": name, "path": entry.path(), "dir": dir, "size": if dir { 0 } else { size },
            "modified": modified, "hidden": name.starts_with('.'), "link": link,
        }));
    }
    Ok(json!({
        "path": folder,
        "parent": folder.parent(),
        "entries": entries,
        "places": places(home),
    }))
}

/// The folders a person reaches for: home, the room's workspace, the
/// teammate's own scratch, downloads, the folders granted to it, the root.
fn places(home: &Path) -> Vec<serde_json::Value> {
    let mut places = vec![json!({"name": "Home", "path": home})];
    for (name, folder) in [
        ("Workspace", "workspace"),
        ("Source", "src"),
        ("Downloads", "Downloads"),
    ] {
        if home.join(folder).is_dir() {
            places.push(json!({"name": name, "path": home.join(folder)}));
        }
    }
    if let Ok(granted) = std::fs::read_dir(home.join("folders")) {
        let mut granted: Vec<_> = granted
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        granted.sort();
        for name in granted {
            places.push(json!({"name": name, "path": home.join("folders").join(&name)}));
        }
    }
    places.push(json!({"name": "Computer", "path": "/"}));
    places
}

pub async fn request(
    State(app): State<App>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
) -> Response {
    if !app.files.admits(key_of(&headers)) {
        return refused(
            StatusCode::UNAUTHORIZED,
            "this window's key is not this computer's",
        );
    }
    match app.files.0.pending.lock().unwrap().get(&id) {
        Some(pending) => Json(&pending.request).into_response(),
        None => refused(StatusCode::NOT_FOUND, "this request was already answered"),
    }
}

#[derive(Deserialize)]
pub struct Reply {
    #[serde(default)]
    paths: Vec<String>,
    filter: Option<usize>,
    #[serde(default)]
    cancel: bool,
}

pub async fn reply(
    State(app): State<App>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    Json(reply): Json<Reply>,
) -> Response {
    if !app.files.admits(key_of(&headers)) {
        return refused(
            StatusCode::UNAUTHORIZED,
            "this window's key is not this computer's",
        );
    }
    let answer = if reply.cancel {
        Answer::Cancelled
    } else if reply.paths.is_empty() || reply.paths.iter().any(|p| !p.starts_with('/')) {
        return refused(StatusCode::UNPROCESSABLE_ENTITY, "choose at least one file");
    } else {
        Answer::Chosen {
            paths: reply.paths,
            filter: reply.filter,
        }
    };
    if app.files.answer(&id, answer) {
        close_picker(app.config.display.clone(), id);
        StatusCode::NO_CONTENT.into_response()
    } else {
        refused(StatusCode::NOT_FOUND, "this request was already answered")
    }
}

#[derive(Deserialize)]
pub struct Beacon {
    key: Option<String>,
}

/// A window closed without an answer cancels: the page's last word is a
/// beacon, which cannot carry a header, so the key rides in the query.
pub async fn cancel(
    State(app): State<App>,
    UrlPath(id): UrlPath<String>,
    Query(beacon): Query<Beacon>,
) -> Response {
    if !app.files.admits(beacon.key.as_deref()) {
        return refused(
            StatusCode::UNAUTHORIZED,
            "this window's key is not this computer's",
        );
    }
    if app.files.answer(&id, Answer::Cancelled) {
        close_picker(app.config.display.clone(), id);
    }
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
pub struct Target {
    path: String,
}

/// What a page or a picture is, Chromium shows in a tab of the computer's
/// browser; anything else goes to whatever app claims it, and when nothing
/// does, the page says so.
pub async fn open(
    State(app): State<App>,
    headers: HeaderMap,
    Json(target): Json<Target>,
) -> Response {
    if !app.files.admits(key_of(&headers)) {
        return refused(
            StatusCode::UNAUTHORIZED,
            "this window's key is not this computer's",
        );
    }
    let path = match std::fs::canonicalize(&target.path) {
        Ok(path) if path.is_file() => path,
        Ok(path) => {
            return refused(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("{} is not a file", path.display()),
            );
        }
        Err(error) => {
            return refused(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("{}: {error}", target.path),
            );
        }
    };
    if shown_by_browser(&path) {
        let url = format!("file://{}", encode(&path.to_string_lossy(), true));
        return match app.browser.tab_new(&url).await {
            Ok(_) => StatusCode::NO_CONTENT.into_response(),
            Err(error) => refused(StatusCode::BAD_GATEWAY, error),
        };
    }
    let opened = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        tokio::process::Command::new("xdg-open")
            .arg(&path)
            .env_remove("HOTLINE_COMPUTER_TOKEN")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status(),
    )
    .await;
    // Still running after a moment means an app took it.
    let taken = match opened {
        Err(_) => true,
        Ok(Ok(status)) => status.success(),
        Ok(Err(_)) => false,
    };
    match taken {
        true => StatusCode::NO_CONTENT.into_response(),
        false => {
            let kind = path
                .extension()
                .map(|e| format!(".{} files", e.to_string_lossy()))
                .unwrap_or_else(|| "this file".to_owned());
            refused(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("No app on this computer opens {kind}"),
            )
        }
    }
}

fn shown_by_browser(path: &Path) -> bool {
    const SHOWN: &[&str] = &[
        "txt", "md", "markdown", "log", "csv", "tsv", "json", "jsonl", "xml", "yaml", "yml",
        "toml", "ini", "conf", "cfg", "env", "sh", "bash", "zsh", "py", "rs", "go", "js", "mjs",
        "cjs", "ts", "tsx", "jsx", "css", "scss", "html", "htm", "svg", "png", "jpg", "jpeg",
        "gif", "webp", "avif", "bmp", "ico", "pdf", "mp4", "webm", "ogg", "mp3", "wav", "c", "h",
        "cpp", "hpp", "java", "kt", "swift", "rb", "php", "sql", "lock", "nix",
    ];
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        name.as_str(),
        "readme" | "license" | "makefile" | "dockerfile"
    ) || path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| SHOWN.contains(&e.to_ascii_lowercase().as_str()))
}

/// The person's terminal in a folder.
pub async fn terminal(
    State(app): State<App>,
    headers: HeaderMap,
    Json(target): Json<Target>,
) -> Response {
    if !app.files.admits(key_of(&headers)) {
        return refused(
            StatusCode::UNAUTHORIZED,
            "this window's key is not this computer's",
        );
    }
    match crate::observer::open(&app.config.home, Path::new(&target.path)) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => refused(StatusCode::UNPROCESSABLE_ENTITY, error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listing_names_folders_files_and_the_places_around_them() {
        let root = std::env::temp_dir().join(format!("files-ui-{}", random_hex(4)));
        let home = root.join("agent");
        std::fs::create_dir_all(home.join("workspace/app")).unwrap();
        std::fs::create_dir_all(home.join("folders/notes")).unwrap();
        std::fs::write(home.join("workspace/readme.md"), "hi").unwrap();
        std::fs::write(home.join("workspace/.env"), "x").unwrap();
        let body = listing(&home, &home.join("workspace")).unwrap();
        let mut names: Vec<(String, bool, bool)> = body["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e["name"].as_str().unwrap().to_owned(),
                    e["dir"].as_bool().unwrap(),
                    e["hidden"].as_bool().unwrap(),
                )
            })
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                (".env".into(), false, true),
                ("app".into(), true, false),
                ("readme.md".into(), false, false)
            ]
        );
        let places: Vec<&str> = body["places"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert_eq!(places, ["Home", "Workspace", "notes", "Computer"]);
        assert!(
            listing(&home, &home.join("workspace/readme.md"))
                .unwrap_err()
                .contains("is not a folder")
        );
        assert!(
            listing(&home, &home.join("missing"))
                .unwrap_err()
                .contains("does not exist")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn an_answer_reaches_the_waiting_request_once() {
        let files = Files::default();
        let (sender, receiver) = oneshot::channel();
        files.0.pending.lock().unwrap().insert(
            "r1".into(),
            Pending {
                request: Request::default(),
                answer: sender,
            },
        );
        assert!(files.answer(
            "r1",
            Answer::Chosen {
                paths: vec!["/a".into()],
                filter: None
            }
        ));
        assert!(
            !files.answer("r1", Answer::Cancelled),
            "a second answer finds nothing"
        );
        assert_eq!(
            receiver.await.unwrap(),
            Answer::Chosen {
                paths: vec!["/a".into()],
                filter: None
            }
        );
        assert!(files.admits(Some(&files.0.key.clone())));
        assert!(!files.admits(Some("nope")) && !files.admits(None));
    }

    #[test]
    fn paths_encode_for_uris_and_addresses_name_their_port() {
        assert_eq!(
            encode("/home/agent/My notes/ü.md", true),
            "/home/agent/My%20notes/%C3%BC.md"
        );
        assert_eq!(encode("/a b", false), "%2Fa%20b");
        assert_eq!(port("0.0.0.0:8787"), 8787);
        assert_eq!(port("[::]:9000"), 9000);
        assert_eq!(port(""), 8787);
        assert!(
            shown_by_browser(Path::new("/x/README")) && shown_by_browser(Path::new("/x/a.PNG"))
        );
        assert!(!shown_by_browser(Path::new("/x/app.AppImage")));
    }
}
