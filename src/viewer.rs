//! The viewer: the page a person opens to see and drive the desktop, and the
//! WebSocket behind it.
//!
//! The desk puts the bearer in the URL's fragment, which never leaves the
//! browser; the page hands it back as the socket's `token` query. Frames go
//! down as binary messages, input comes up as JSON. The page opens
//! view-only and sends nothing, so watching a teammate work never takes the
//! machine from it. `Take control` says otherwise, and from then on the
//! person's input holds the machine for a few seconds at a time, so the
//! teammate's mutating tools are refused while someone is driving, and
//! nobody has to remember to give the desktop back.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use std::path::PathBuf;

use axum::Json;
use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

use crate::App;
use crate::display::Display;
use crate::serve::same_secret;

const PAGE: &str = include_str!("viewer.html");
/// The holder name a person's input takes the machine under.
pub const PERSON: &str = "person";
/// Seconds the machine stays the person's after their last input.
const HOLD: u64 = 10;
static VIEWER_ID: AtomicU64 = AtomicU64::new(1);

pub async fn page() -> Html<&'static str> {
    Html(PAGE)
}

#[derive(Deserialize)]
pub struct Ticket {
    #[serde(default)]
    token: String,
}

pub async fn socket(
    State(app): State<App>,
    Query(ticket): Query<Ticket>,
    upgrade: WebSocketUpgrade,
) -> Response {
    if let Some(expected) = app.config.token.as_deref()
        && !same_secret(ticket.token.as_bytes(), expected.as_bytes())
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"unauthorized"})),
        )
            .into_response();
    }
    let Some(display) = app.display.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "this computer has no display",
        )
            .into_response();
    };
    upgrade.on_upgrade(move |ws| drive(ws, app, display))
}

/// What the page's Files panel asks with: the token, which the page holds
/// only in its fragment, and a path under the home; none means the home.
#[derive(Deserialize)]
pub struct FileRequest {
    #[serde(default)]
    token: String,
    #[serde(default)]
    path: String,
}

fn admitted(app: &App, token: &str) -> bool {
    match app.config.token.as_deref() {
        Some(expected) => same_secret(token.as_bytes(), expected.as_bytes()),
        None => true,
    }
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error":"unauthorized"})),
    )
        .into_response()
}

fn refused(error: String) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"error": error}))).into_response()
}

fn requested(app: &App, path: &str) -> PathBuf {
    if path.is_empty() {
        app.config.home.clone()
    } else {
        PathBuf::from(path)
    }
}

/// A folder under the home as the Files panel shows it: what is in it by
/// name, where it is, and where the home is so the panel knows how far up
/// it may go. Reading takes nothing from the teammate, so watching is enough.
pub async fn files(State(app): State<App>, Query(request): Query<FileRequest>) -> Response {
    if !admitted(&app, &request.token) {
        return unauthorized();
    }
    let home = match tokio::fs::canonicalize(&app.config.home).await {
        Ok(home) => home,
        Err(error) => return refused(format!("home: {error}")),
    };
    match crate::tools::files::entries(&app, &requested(&app, &request.path)).await {
        Ok((path, entries)) => {
            Json(json!({"path": path, "home": home, "entries": entries})).into_response()
        }
        Err(error) => refused(error),
    }
}

/// A file under the home, sent as an attachment so the person's own browser
/// saves it on their computer. It streams, so a build's artifact is fine.
pub async fn download(State(app): State<App>, Query(request): Query<FileRequest>) -> Response {
    if !admitted(&app, &request.token) {
        return unauthorized();
    }
    let path = match crate::tools::files::existing_path(&app, &requested(&app, &request.path)).await
    {
        Ok(path) => path,
        Err(error) => return refused(error),
    };
    let file = match tokio::fs::File::open(&path).await {
        Ok(file) => file,
        Err(error) => return refused(format!("{}: {error}", path.display())),
    };
    let length = match file.metadata().await {
        Ok(metadata) if metadata.is_dir() => return refused("path is a folder".to_owned()),
        Ok(metadata) => metadata.len(),
        Err(error) => return refused(error.to_string()),
    };
    let name = path.file_name().map_or_else(
        || "file".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    Response::builder()
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, length)
        .header(header::CONTENT_DISPOSITION, attachment(&name))
        .body(Body::from_stream(tokio_util::io::ReaderStream::new(file)))
        .unwrap_or_else(|error| refused(error.to_string()))
}

/// `attachment; filename=…` spelled for every browser: the name in ASCII,
/// quoted, and beside it the UTF-8 name percent-encoded.
fn attachment(name: &str) -> String {
    let ascii: String = name
        .chars()
        .map(|c| {
            if c == ' ' || (c.is_ascii_graphic() && c != '"' && c != '\\') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut encoded = String::new();
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}")
}

async fn drive(mut ws: WebSocket, app: App, display: Arc<Display>) {
    let holder = format!("{PERSON}-{}", VIEWER_ID.fetch_add(1, Ordering::Relaxed));
    let mut frames = display.screen.subscribe();
    // The pointer is not in the frames, so the hands say where they are.
    let mut gestures = display.gestures.subscribe();
    // A socket arrives watching, and says so when the person takes the
    // screen. One viewer driving leaves another one still only looking.
    let mut driving = false;
    // The control bar says whose screen this is and what the machine is
    // doing; it hears once a second, and only when something changed.
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
    let mut told = String::new();
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let state = state_message(&app, &holder, driving).await;
                if state != told {
                    if ws.send(Message::Text(state.clone().into())).await.is_err() {
                        break;
                    }
                    told = state;
                }
            },
            frame = frames.recv() => match frame {
                Ok(frame) => {
                    if ws.send(Message::Binary(frame.encode().into())).await.is_err() {
                        break;
                    }
                }
                Err(RecvError::Lagged(_)) => display.screen.request_full(),
                Err(RecvError::Closed) => break,
            },
            gesture = gestures.recv() => match gesture {
                Ok(gesture) => {
                    if ws.send(Message::Text(pointer_message(&gesture).into())).await.is_err() {
                        break;
                    }
                }
                // A viewer that fell behind the hands just picks up where they are now.
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            },
            message = ws.recv() => match message {
                Some(Ok(Message::Text(text))) => {
                    match handle(&text, &display, &app, &holder, &mut driving).await {
                        Ok(true) => { let _ = ws.send(Message::Text(json!({"t":"paste","ok":true}).to_string().into())).await; }
                        Ok(false) => {},
                        Err(error) => { let _ = ws.send(Message::Text(json!({"t":"error","error":error,"driving":driving}).to_string().into())).await; }
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
        }
    }
    if driving {
        let_go(&display);
    }
    let _ = app.access.release(&holder).await;
}

/// A viewer that stops driving, by choice or by vanishing, may have sent a
/// key down whose up never came. The hands let go of whatever they hold.
fn let_go(display: &Display) {
    if let Ok(mut hands) = display.hands.lock() {
        let _ = hands.release_all();
    }
}

/// Where the hands are, for the page to draw an arrow: a move carries only
/// the point; a button carries which one and whether it went down.
fn pointer_message(gesture: &crate::xtest::Gesture) -> String {
    let mut message = serde_json::to_value(gesture).unwrap_or_default();
    message["t"] = json!("pointer");
    message.to_string()
}

/// What the page shows in its control bar: who holds the machine, from this
/// socket's point of view, and the job counts the desktop's bar shows.
async fn state_message(app: &App, holder: &str, driving: bool) -> String {
    let who = match app.access.holder().await {
        Some((current, _)) if current == holder => "you",
        Some((current, _)) if current.starts_with(PERSON) => "person",
        Some(_) => "agent",
        None => "none",
    };
    let jobs = app.jobs.list().await.unwrap_or_default();
    let running = jobs.iter().filter(|job| job.state == "running").count();
    let completed = jobs
        .iter()
        .filter(|job| job.state == "exited" && job.exit_code == Some(0))
        .count();
    json!({"t":"state","holder":who,"driving":driving,"running":running,"completed":completed,"failed":jobs.len() - running - completed})
        .to_string()
}

#[derive(Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
enum FromViewer {
    /// The person taking the screen, or handing it back. Taking it holds the
    /// machine at once rather than at the first twitch of the mouse, and
    /// handing it back frees the teammate now rather than in ten seconds.
    Control {
        take: bool,
    },
    Move {
        x: i16,
        y: i16,
    },
    Button {
        b: u8,
        down: bool,
    },
    /// One notch: `dy` −1 is up and 1 is down; `dx` −1 is left and 1 is right.
    Wheel {
        #[serde(default)]
        dx: i8,
        #[serde(default)]
        dy: i8,
    },
    Paste {
        text: String,
    },
    Key {
        key: String,
        down: bool,
    },
}

async fn handle(
    text: &str,
    display: &Display,
    app: &App,
    holder: &str,
    driving: &mut bool,
) -> Result<bool, String> {
    let message: FromViewer =
        serde_json::from_str(text).map_err(|_| "invalid viewer input".to_owned())?;
    if !reaches_the_hands(&message, app, holder, driving).await {
        if matches!(message, FromViewer::Paste { .. }) {
            return Err("Take control before pasting".into());
        }
        if matches!(message, FromViewer::Control { take: false }) {
            let_go(display);
        }
        return Ok(false);
    }
    let _guard = app.access.mutate(holder).await?;
    if let FromViewer::Paste { text } = message {
        if text.len() > 1_048_576 {
            return Err("Paste exceeds 1 MiB".into());
        }
        if text.contains('\0') {
            return Err("Clipboard text contains a NUL byte".into());
        }
        if crate::x11::windows(&app.config.display)?
            .iter()
            .any(|window| {
                window.focused && window.class.to_ascii_lowercase().contains("toadterminal")
            })
        {
            return Err(
                "The terminal observer is read-only. Paste into the application you are testing."
                    .into(),
            );
        }
        let display = app.display()?;
        tokio::task::spawn_blocking(move || {
            display.clipboard.write(&text)?;
            let mut hands = display
                .hands
                .lock()
                .map_err(|_| "the hands are poisoned".to_owned())?;
            for key in ["Control", "Meta", "Shift", "Alt"] {
                hands.key(key, false)?;
            }
            hands.combo("ctrl+v")
        })
        .await
        .map_err(|_| "paste task failed".to_owned())??;
        return Ok(true);
    }
    let mut hands = display
        .hands
        .lock()
        .map_err(|_| "the hands are poisoned".to_owned())?;
    match message {
        FromViewer::Move { x, y } => hands.move_to(x, y),
        FromViewer::Button { b, down } => hands.button(b, down),
        FromViewer::Wheel { dx, dy } => {
            for button in [(dy < 0, 4), (dy > 0, 5), (dx < 0, 6), (dx > 0, 7)]
                .into_iter()
                .filter_map(|(turned, button)| turned.then_some(button))
            {
                hands.button(button, true)?;
                hands.button(button, false)?;
            }
            Ok(())
        }
        FromViewer::Key { key, down } => hands.key(&key, down),
        // Answered by `reaches_the_hands`, before the hands were taken.
        FromViewer::Control { .. } | FromViewer::Paste { .. } => Ok(()),
    }?;
    Ok(false)
}

/// Whether this message moves the desktop, and who holds the machine once it
/// has been read. `Take control` and `Give it back` are the person's word on
/// that and touch nothing themselves; everything else is only the hands, and
/// only for a socket that has taken the screen — a viewer that is watching is
/// watching whatever it sends.
async fn reaches_the_hands(
    message: &FromViewer,
    app: &App,
    holder: &str,
    driving: &mut bool,
) -> bool {
    if let FromViewer::Control { take } = message {
        *driving = *take;
        if *take {
            app.access.seize(holder, HOLD).await;
        } else {
            let _ = app.access.release(holder).await;
        }
        return false;
    }
    if !*driving {
        return false;
    }
    if !app.access.renew(holder, HOLD).await {
        *driving = false;
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;

    fn app() -> App {
        app_at(std::env::temp_dir(), None)
    }

    fn app_at(home: PathBuf, token: Option<&str>) -> App {
        App::new(Config {
            addr: "127.0.0.1:0".to_owned(),
            token: token.map(str::to_owned),
            home,
            display: ":0".to_owned(),
            screen: "1920x1080".to_owned(),
        })
    }

    async fn body_of(response: Response) -> Vec<u8> {
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("a whole body")
            .to_vec()
    }

    #[tokio::test]
    async fn the_files_panel_sees_the_home_and_nothing_above_it() {
        let home = std::env::temp_dir().join(format!("toad-viewer-files-{}", std::process::id()));
        std::fs::create_dir_all(home.join("notes")).unwrap();
        std::fs::write(home.join("notes/café.txt"), "kept").unwrap();
        let app = app_at(home.clone(), None);
        let request = |path: &str| {
            Query(FileRequest {
                token: String::new(),
                path: path.to_owned(),
            })
        };

        let listed = files(State(app.clone()), request("")).await;
        assert_eq!(listed.status(), StatusCode::OK);
        let listing: serde_json::Value = serde_json::from_slice(&body_of(listed).await).unwrap();
        assert_eq!(listing["path"], listing["home"], "no path means the home");
        assert_eq!(listing["entries"][0]["name"], "notes");
        assert_eq!(listing["entries"][0]["is_dir"], true);

        let saved = download(
            State(app.clone()),
            request(&home.join("notes/café.txt").display().to_string()),
        )
        .await;
        assert_eq!(saved.status(), StatusCode::OK);
        assert_eq!(
            saved.headers()[header::CONTENT_DISPOSITION],
            "attachment; filename=\"caf_.txt\"; filename*=UTF-8''caf%C3%A9.txt"
        );
        assert_eq!(body_of(saved).await, b"kept");

        let above = files(State(app.clone()), request("/")).await;
        assert_eq!(above.status(), StatusCode::BAD_REQUEST);
        let folder = download(
            State(app),
            request(&home.join("notes").display().to_string()),
        )
        .await;
        assert_eq!(folder.status(), StatusCode::BAD_REQUEST);
        std::fs::remove_dir_all(home).unwrap();
    }

    #[tokio::test]
    async fn the_files_panel_wants_the_token() {
        let app = app_at(std::env::temp_dir(), Some("secret"));
        let wrong = Query(FileRequest {
            token: "guess".to_owned(),
            path: String::new(),
        });
        assert_eq!(
            files(State(app.clone()), wrong).await.status(),
            StatusCode::UNAUTHORIZED
        );
        let wrong = Query(FileRequest {
            token: "guess".to_owned(),
            path: String::new(),
        });
        assert_eq!(
            download(State(app), wrong).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }

    fn sent(text: &str) -> FromViewer {
        serde_json::from_str(text).expect("a message the viewer sends")
    }

    #[tokio::test]
    async fn the_screen_is_the_teammate_s_until_a_person_takes_it() {
        let app = app();
        let mut driving = false;

        // The page opens view-only, so nothing it sends moves the desktop and
        // the teammate keeps a machine someone is only looking at.
        let move_to = sent(r#"{"t":"move","x":10,"y":10}"#);
        assert!(!reaches_the_hands(&move_to, &app, PERSON, &mut driving).await);
        assert!(app.access.mutate("teammate").await.is_ok());

        // Taking it holds the machine at once, not at the first twitch.
        let take = sent(r#"{"t":"control","take":true}"#);
        assert!(!reaches_the_hands(&take, &app, PERSON, &mut driving).await);
        let refused = app.access.mutate("teammate").await.unwrap_err();
        assert!(refused.contains(PERSON), "{refused}");
        assert!(reaches_the_hands(&move_to, &app, PERSON, &mut driving).await);

        // Handing it back frees the teammate now, not ten seconds from now.
        let give_back = sent(r#"{"t":"control","take":false}"#);
        assert!(!reaches_the_hands(&give_back, &app, PERSON, &mut driving).await);
        assert!(app.access.mutate("teammate").await.is_ok());
        assert!(!reaches_the_hands(&move_to, &app, PERSON, &mut driving).await);
        let paste = sent(r#"{"t":"paste","text":"private clipboard"}"#);
        assert!(!reaches_the_hands(&paste, &app, PERSON, &mut driving).await);
        reaches_the_hands(&take, &app, "person-a", &mut driving).await;
        let mut other = true;
        app.access.seize("person-b", HOLD).await;
        assert!(!reaches_the_hands(&paste, &app, "person-a", &mut driving).await);
        assert!(reaches_the_hands(&paste, &app, "person-b", &mut other).await);
        assert!(app.access.release("person-a").await.is_err());
    }

    #[test]
    fn the_page_is_told_where_the_hands_are() {
        use crate::xtest::Gesture;
        let moved: serde_json::Value = serde_json::from_str(&pointer_message(&Gesture {
            x: 640,
            y: 360,
            button: None,
            down: false,
        }))
        .unwrap();
        assert_eq!(moved, json!({"t":"pointer","x":640,"y":360}));
        let pressed: serde_json::Value = serde_json::from_str(&pointer_message(&Gesture {
            x: 640,
            y: 360,
            button: Some(1),
            down: true,
        }))
        .unwrap();
        assert_eq!(
            pressed,
            json!({"t":"pointer","x":640,"y":360,"button":1,"down":true})
        );
    }

    #[tokio::test]
    async fn the_control_bar_is_told_whose_screen_it_is() {
        let app = app();
        let state: serde_json::Value =
            serde_json::from_str(&state_message(&app, "person-1", false).await).unwrap();
        assert_eq!(state["holder"], "none");
        assert_eq!(state["running"], 0);
        app.access.seize("person-1", HOLD).await;
        let state: serde_json::Value =
            serde_json::from_str(&state_message(&app, "person-1", true).await).unwrap();
        assert_eq!(state["holder"], "you");
        assert_eq!(state["driving"], true);
        let state: serde_json::Value =
            serde_json::from_str(&state_message(&app, "person-2", false).await).unwrap();
        assert_eq!(state["holder"], "person");
        app.access.release("person-1").await.unwrap();
        app.access.control("teammate", Some(60)).await.unwrap();
        let state: serde_json::Value =
            serde_json::from_str(&state_message(&app, "person-2", false).await).unwrap();
        assert_eq!(state["holder"], "agent");
    }
}
