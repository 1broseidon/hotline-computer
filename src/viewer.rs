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

use axum::Json;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
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

async fn drive(mut ws: WebSocket, app: App, display: Arc<Display>) {
    let holder = format!("{PERSON}-{}", VIEWER_ID.fetch_add(1, Ordering::Relaxed));
    let mut frames = display.screen.subscribe();
    // A socket arrives watching, and says so when the person takes the
    // screen. One viewer driving leaves another one still only looking.
    let mut driving = false;
    loop {
        tokio::select! {
            frame = frames.recv() => match frame {
                Ok(frame) => {
                    if ws.send(Message::Binary(frame.encode().into())).await.is_err() {
                        break;
                    }
                }
                Err(RecvError::Lagged(_)) => display.screen.request_full(),
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
    let _ = app.access.release(&holder).await;
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
        App::new(Config {
            addr: "127.0.0.1:0".to_owned(),
            token: None,
            home: std::env::temp_dir(),
            display: ":0".to_owned(),
            screen: "1920x1080".to_owned(),
        })
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
}
