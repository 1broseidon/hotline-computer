//! `/sites/ws`: the signed-in browser as a screen, in the viewer's own
//! protocol, so the desk's `RemoteBrowser` and the phone's remote-browser
//! view open it as they open `/ws`.
//!
//! Frames go down as binary messages (six little-endian u16 — x, y, width,
//! height, screen width, screen height — then a PNG), from the tab's
//! screencast. Input comes up as the viewer's JSON and goes to the tab
//! through DevTools: it never touches the X display, where jobs listen.
//! While the person drives, the agent sees nothing of the site, and a tab
//! they typed into stays hidden from it until its next document.
//!
//! `?site=host` picks the tab on that site, opening the site when no tab
//! is on it: what Open Browser and Sign In show.

use axum::Json;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::atomic::Ordering;

use super::{Signin, Tab, cdp::Cdp};
use crate::App;
use crate::serve::same_secret;

#[derive(Deserialize)]
pub struct Ticket {
    #[serde(default)]
    token: String,
    #[serde(default)]
    site: String,
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
            Json(json!({"error": "unauthorized"})),
        )
            .into_response();
    }
    let Some(signin) = app.signin.clone() else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "this computer has no signed-in browser"})),
        )
            .into_response();
    };
    upgrade.on_upgrade(move |ws| drive(ws, signin, ticket.site))
}

/// What the screen's page sends: the viewer's messages.
#[derive(Deserialize, Debug)]
#[serde(tag = "t", rename_all = "lowercase")]
pub(super) enum FromScreen {
    Ping {
        id: u64,
    },
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

/// One screen's hands: whether the person took it, where the pointer is,
/// and what is held down.
#[derive(Default)]
pub(super) struct Hands {
    pub driving: bool,
    x: f64,
    y: f64,
    button: Option<&'static str>,
    modifiers: u32,
}

async fn drive(mut ws: WebSocket, signin: Signin, site: String) {
    signin.0.viewers.fetch_add(1, Ordering::SeqCst);
    let shown = show(&mut ws, &signin, &site).await;
    signin.0.viewers.fetch_sub(1, Ordering::SeqCst);
    if let Err(error) = shown {
        let _ = ws
            .send(Message::Text(
                json!({"t": "error", "error": error}).to_string().into(),
            ))
            .await;
    }
}

async fn show(ws: &mut WebSocket, signin: &Signin, site: &str) -> Result<(), String> {
    let running = signin.browser().await?;
    let tab = signin.screen_tab(&running, site).await?;
    let cdp = running.cdp.clone();
    let mut events = cdp.events();
    cdp.on(
        &tab.session,
        "Page.startScreencast",
        json!({"format": "png", "maxWidth": super::VIEWPORT.0, "maxHeight": super::VIEWPORT.1}),
    )
    .await?;
    let mut hands = Hands::default();
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
    let mut told = String::new();
    let host = crate::bridge::host_of(&tab.url);
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                signin.touch();
                if signin.tab(&tab.target).is_none() {
                    break;
                }
                let state = json!({"t": "state", "holder": if hands.driving { "you" } else { "none" },
                    "driving": hands.driving, "site": host, "running": 0, "completed": 0, "failed": 0}).to_string();
                if state != told {
                    if ws.send(Message::Text(state.clone().into())).await.is_err() {
                        break;
                    }
                    told = state;
                }
            }
            event = events.recv() => match event {
                Ok(event) if event.method == "Page.screencastFrame" && event.session.as_deref() == Some(tab.session.as_str()) => {
                    let _ = cdp.on(&tab.session, "Page.screencastFrameAck", json!({"sessionId": event.params["sessionId"]})).await;
                    if let Some(frame) = frame(&event.params)
                        && ws.send(Message::Binary(frame.into())).await.is_err()
                    {
                        break;
                    }
                }
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(_) => break,
            },
            message = ws.recv() => match message {
                Some(Ok(Message::Text(text))) => {
                    let Ok(message) = serde_json::from_str::<FromScreen>(&text) else {
                        let _ = ws.send(Message::Text(json!({"t": "error", "error": "invalid viewer input", "driving": hands.driving}).to_string().into())).await;
                        continue;
                    };
                    if let FromScreen::Ping { id } = message {
                        if ws.send(Message::Text(json!({"t": "pong", "id": id}).to_string().into())).await.is_err() {
                            break;
                        }
                        continue;
                    }
                    match signin.input(&cdp, &tab, &mut hands, message).await {
                        Ok(true) => { let _ = ws.send(Message::Text(json!({"t": "paste", "ok": true}).to_string().into())).await; }
                        Ok(false) => {}
                        Err(error) => { let _ = ws.send(Message::Text(json!({"t": "error", "error": error, "driving": hands.driving}).to_string().into())).await; }
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
        }
    }
    let _ = cdp.on(&tab.session, "Page.stopScreencast", json!({})).await;
    Ok(())
}

/// A screencast frame in the viewer's wire form.
fn frame(params: &Value) -> Option<Vec<u8>> {
    let png = base64::engine::general_purpose::STANDARD
        .decode(params["data"].as_str()?)
        .ok()?;
    if png.len() < 24 || &png[1..4] != b"PNG" {
        return None;
    }
    let width = u32::from_be_bytes(png[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(png[20..24].try_into().ok()?);
    let (width, height) = (u16::try_from(width).ok()?, u16::try_from(height).ok()?);
    let mut bytes = Vec::with_capacity(12 + png.len());
    for value in [0, 0, width, height, width, height] {
        bytes.extend_from_slice(&u16::to_le_bytes(value));
    }
    bytes.extend_from_slice(&png);
    Some(bytes)
}

impl Signin {
    /// The tab a screen shows: the one on `site`, opened when none is, or
    /// the agent's.
    async fn screen_tab(&self, running: &super::Running, site: &str) -> Result<Tab, String> {
        if site.is_empty() {
            return self.current(running).await;
        }
        let url = format!("{}/", super::sessions::site(site)?);
        if !self.routes(&url) {
            return Err(format!("{site} is not a signed-in site on this computer"));
        }
        let host = crate::bridge::host_of(&url);
        let found = self
            .lock_tabs()
            .list
            .iter()
            .find(|tab| tab.ready && crate::bridge::host_of(&tab.url) == host)
            .cloned();
        if let Some(tab) = found {
            return Ok(tab);
        }
        let tab = self.new_tab(running).await?;
        self.navigate(&running.cdp, &tab.session, &url).await?;
        Ok(self.tab(&tab.target).unwrap_or(tab))
    }

    /// The person's input on a screen, to its tab through DevTools. Answers
    /// whether it was a paste, which the page acknowledges.
    pub(super) async fn input(
        &self,
        cdp: &Cdp,
        tab: &Tab,
        hands: &mut Hands,
        message: FromScreen,
    ) -> Result<bool, String> {
        if let FromScreen::Control { take } = message {
            hands.driving = take;
            if take {
                self.person_input(cdp, tab, false).await;
            }
            return Ok(false);
        }
        if !hands.driving {
            if matches!(message, FromScreen::Paste { .. }) {
                return Err("Take control before pasting".into());
            }
            return Ok(false);
        }
        let typed = matches!(
            message,
            FromScreen::Key { down: true, .. } | FromScreen::Paste { .. }
        );
        self.person_input(cdp, tab, typed).await;
        let session = tab.session.as_str();
        let mouse = |kind: &'static str, x: f64, y: f64, button: &'static str, extra: Value| {
            let mut params = json!({"type": kind, "x": x, "y": y, "button": button, "clickCount": 1,
                                    "modifiers": hands.modifiers});
            if let (Some(params), Some(extra)) = (params.as_object_mut(), extra.as_object()) {
                params.extend(extra.clone());
            }
            params
        };
        match message {
            FromScreen::Move { x, y } => {
                hands.x = f64::from(x);
                hands.y = f64::from(y);
                let params = mouse(
                    "mouseMoved",
                    hands.x,
                    hands.y,
                    hands.button.unwrap_or("none"),
                    json!({"clickCount": 0}),
                );
                cdp.on(session, "Input.dispatchMouseEvent", params).await?;
            }
            FromScreen::Button { b, down } => {
                let (dx, dy) = match b {
                    4 => (0, -1),
                    5 => (0, 1),
                    6 => (-1, 0),
                    7 => (1, 0),
                    _ => (0, 0),
                };
                if dx != 0 || dy != 0 {
                    if down {
                        let params = mouse(
                            "mouseWheel",
                            hands.x,
                            hands.y,
                            "none",
                            json!({"deltaX": dx * 100, "deltaY": dy * 100}),
                        );
                        cdp.on(session, "Input.dispatchMouseEvent", params).await?;
                    }
                    return Ok(false);
                }
                let button = match b {
                    2 => "middle",
                    3 => "right",
                    _ => "left",
                };
                hands.button = down.then_some(button);
                let kind = if down {
                    "mousePressed"
                } else {
                    "mouseReleased"
                };
                let params = mouse(kind, hands.x, hands.y, button, json!({}));
                cdp.on(session, "Input.dispatchMouseEvent", params).await?;
            }
            FromScreen::Wheel { dx, dy } => {
                let params = mouse(
                    "mouseWheel",
                    hands.x,
                    hands.y,
                    "none",
                    json!({"deltaX": i32::from(dx) * 100, "deltaY": i32::from(dy) * 100}),
                );
                cdp.on(session, "Input.dispatchMouseEvent", params).await?;
            }
            FromScreen::Paste { text } => {
                if text.len() > 1_048_576 {
                    return Err("Paste exceeds 1 MiB".into());
                }
                cdp.on(session, "Input.insertText", json!({"text": text}))
                    .await?;
                return Ok(true);
            }
            FromScreen::Key { key, down } => {
                let bit = match key.as_str() {
                    "Alt" | "AltGraph" => 1,
                    "Control" => 2,
                    "Meta" | "OS" | "Super" => 4,
                    "Shift" => 8,
                    _ => 0,
                };
                if down {
                    hands.modifiers |= bit;
                } else {
                    hands.modifiers &= !bit;
                }
                cdp.on(
                    session,
                    "Input.dispatchKeyEvent",
                    key_event(&key, down, hands.modifiers),
                )
                .await?;
            }
            FromScreen::Ping { .. } | FromScreen::Control { .. } => {}
        }
        Ok(false)
    }
}

/// A browser `KeyboardEvent.key` as a DevTools key event. A character
/// carries its text unless a shortcut modifier is held; a named key carries
/// the virtual key code a page's handlers look at.
fn key_event(key: &str, down: bool, modifiers: u32) -> Value {
    let code: u32 = match key {
        "Backspace" => 8,
        "Tab" => 9,
        "Enter" => 13,
        "Shift" => 16,
        "Control" => 17,
        "Alt" => 18,
        "Escape" => 27,
        " " => 32,
        "PageUp" => 33,
        "PageDown" => 34,
        "End" => 35,
        "Home" => 36,
        "ArrowLeft" => 37,
        "ArrowUp" => 38,
        "ArrowRight" => 39,
        "ArrowDown" => 40,
        "Delete" => 46,
        "Meta" => 91,
        single if single.chars().count() == 1 => {
            let c = single
                .chars()
                .next()
                .unwrap_or_default()
                .to_ascii_uppercase();
            if c.is_ascii_alphanumeric() {
                c as u32
            } else {
                0
            }
        }
        _ => 0,
    };
    let text = match key {
        "Enter" => Some("\r".to_owned()),
        single if single.chars().count() == 1 && modifiers & (2 | 4) == 0 => {
            Some(single.to_owned())
        }
        _ => None,
    };
    let kind = match (down, &text) {
        (false, _) => "keyUp",
        (true, Some(_)) => "keyDown",
        (true, None) => "rawKeyDown",
    };
    let mut event = json!({"type": kind, "key": key, "windowsVirtualKeyCode": code,
                           "nativeVirtualKeyCode": code, "modifiers": modifiers});
    if down && let Some(text) = text {
        event["text"] = json!(text.clone());
        event["unmodifiedText"] = json!(text);
    }
    event
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_become_devtools_events() {
        let a = key_event("a", true, 0);
        assert_eq!(a["type"], "keyDown");
        assert_eq!(a["text"], "a");
        assert_eq!(a["windowsVirtualKeyCode"], 65);
        let enter = key_event("Enter", true, 0);
        assert_eq!(enter["text"], "\r");
        assert_eq!(enter["windowsVirtualKeyCode"], 13);
        let shortcut = key_event("c", true, 2);
        assert_eq!(shortcut["type"], "rawKeyDown");
        assert!(shortcut.get("text").is_none());
        assert_eq!(key_event("Tab", false, 0)["type"], "keyUp");
    }

    #[test]
    fn a_frame_is_the_viewers_header_and_the_png() {
        let mut png = vec![
            0x89, b'P', b'N', b'G', 13, 10, 26, 10, 0, 0, 0, 13, b'I', b'H', b'D', b'R',
        ];
        png.extend_from_slice(&1280u32.to_be_bytes());
        png.extend_from_slice(&800u32.to_be_bytes());
        let data = base64::engine::general_purpose::STANDARD.encode(&png);
        let bytes = frame(&json!({"data": data})).unwrap();
        let header: Vec<u16> = bytes[..12]
            .chunks(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        assert_eq!(header, [0, 0, 1280, 800, 1280, 800]);
        assert_eq!(&bytes[12..], &png[..]);
    }
}
