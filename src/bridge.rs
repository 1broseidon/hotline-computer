//! The browser extension's line to the service. Chromium starts
//! `hotline-computer` as the extension's native messaging host (the first
//! argument is the extension's origin); that process relays each message
//! over a socket in the service's private directory, and the service
//! answers. Today that is the passkey requests the extension takes from a
//! page; logins and the desk's other integrations come the same way.
//!
//! One message per connection: a page that gives up closes its port, the
//! relay goes with it, and the closed socket cancels what was waiting.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

use crate::App;
use crate::authenticator::{self, Refusal};
use crate::passkeys::{Ask, Reported, rp_within};
use crate::secrets::Passkey;

/// Where Chromium loads the extension from; its absence, or
/// `HOTLINE_COMPUTER_PASSKEYS=virtual`, keeps the older DevTools authenticator.
pub const EXTENSION_DIR: &str = "/usr/share/hotline-computer/extension";

pub fn extension_answers() -> bool {
    Path::new(EXTENSION_DIR).is_dir()
        && std::env::var("HOTLINE_COMPUTER_PASSKEYS").as_deref() != Ok("virtual")
}

pub fn socket_path() -> PathBuf {
    std::env::temp_dir().join("hotline-computer/bridge.sock")
}

/// Answers the extension for as long as the computer runs.
pub async fn serve(app: App) {
    use std::os::unix::fs::PermissionsExt;
    let path = socket_path();
    if let Some(directory) = path.parent() {
        let _ = std::fs::create_dir_all(directory);
    }
    let _ = std::fs::remove_file(&path);
    let listener = match tokio::net::UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(error) => return eprintln!("extension bridge: {}: {error}", path.display()),
    };
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(conversation(app.clone(), stream));
    }
}

async fn conversation(app: App, stream: tokio::net::UnixStream) {
    let (read, mut write) = stream.into_split();
    let mut lines = tokio::io::BufReader::new(read).lines();
    let Ok(Some(line)) = lines.next_line().await else {
        return;
    };
    let reply = match serde_json::from_str::<Value>(&line) {
        Ok(message) => tokio::select! {
            reply = answer(&app, &message) => reply,
            // The page gave up: nothing is left to answer.
            _ = lines.next_line() => return,
        },
        Err(_) => refused(authenticator_refusal(
            "DataError",
            "the message is not JSON",
        )),
    };
    let _ = write.write_all(format!("{reply}\n").as_bytes()).await;
}

fn authenticator_refusal(name: &'static str, message: &str) -> Refusal {
    Refusal {
        name,
        message: message.to_owned(),
    }
}

fn refused(refusal: Refusal) -> Value {
    json!({"error": {"name": refusal.name, "message": refusal.message}})
}

/// Every passkey this computer may sign with: the granted set, and one an
/// arming minted that the desk has not delivered back yet.
fn keys(app: &App) -> Vec<Passkey> {
    let mut keys: Vec<Passkey> = app
        .secrets
        .passkeys()
        .into_iter()
        .map(|(_, key)| key)
        .collect();
    keys.extend(app.passkeys.minted());
    keys
}

async fn answer(app: &App, message: &Value) -> Value {
    let details = &message["details"];
    match message["kind"].as_str() {
        Some("uvpaa") => json!({"isUvpaa": true}),
        Some("get") => match authenticator::assert(details, &keys(app)) {
            Ok(response) => json!({"response": response}),
            Err(refusal) => refused(refusal),
        },
        Some("create") => match create(app, details).await {
            Ok(response) => json!({"response": response}),
            Err(refusal) => refused(refusal),
        },
        _ => refused(authenticator_refusal(
            "NotSupportedError",
            "this computer does not answer that",
        )),
    }
}

/// A site asking to make a passkey: only for the site the person armed, and
/// only once the person approves, through the desk's own request and answer.
async fn create(app: &App, details: &Value) -> Result<Value, Refusal> {
    let creation = authenticator::creation(details)?;
    if authenticator::excluded(&creation, &keys(app)) {
        return Err(authenticator_refusal(
            "InvalidStateError",
            &format!("this computer already has a passkey for {}", creation.rp_id),
        ));
    }
    let not_armed = || {
        authenticator_refusal(
            "NotAllowedError",
            &format!(
                "Hotline: making a passkey for {} is not armed. The person arms it in the teammate's pane.",
                creation.rp_id
            ),
        )
    };
    match app.passkeys.armed() {
        Some((armed, _)) if rp_within(&creation.rp_id, &armed) => {}
        _ => return Err(not_armed()),
    }
    // Ended without the person's answer (the page gave up, or the wait
    // was dropped), the request stops holding the arming's one place.
    struct Withdraw<'a>(&'a crate::passkeys::Passkeys, String);
    impl Drop for Withdraw<'_> {
        fn drop(&mut self) {
            self.0.withdraw(&self.1);
        }
    }
    let ask = Ask {
        id: request_id(),
        rp_id: creation.rp_id.clone(),
        origin: creation.origin.clone(),
        rp_name: creation.rp_name.clone(),
        user_name: creation.user_name.clone(),
        user_display_name: creation.user_display_name.clone(),
        asked_at: 0,
    };
    let _withdraw = Withdraw(&app.passkeys, ask.id.clone());
    loop {
        match app.passkeys.report(ask.clone()) {
            Reported::Waiting => tokio::time::sleep(std::time::Duration::from_millis(250)).await,
            Reported::Approved => {
                let (passkey, response) = authenticator::mint(&creation)?;
                app.passkeys.delivered(&ask.id);
                return if app.passkeys.keep(passkey) {
                    Ok(response)
                } else {
                    Err(authenticator_refusal(
                        "NotAllowedError",
                        "Hotline: the arming ended before the passkey was made.",
                    ))
                };
            }
            // A denial ends the arming at once, so it may arrive as Later.
            Reported::Denied => return Err(denied()),
            Reported::Later if app.passkeys.decided(&ask.id) == Some(false) => {
                return Err(denied());
            }
            Reported::Later => {
                return Err(authenticator_refusal(
                    "NotAllowedError",
                    "Hotline: another passkey request is waiting for the person, or the arming ended.",
                ));
            }
        }
    }
}

fn denied() -> Refusal {
    authenticator_refusal(
        "NotAllowedError",
        "Hotline: the person did not approve making this passkey.",
    )
}

fn request_id() -> String {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 9];
    let _ = ring::rand::SystemRandom::new().fill(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The native messaging host: Chromium's framing on stdio (a 32-bit
/// length in native byte order, then JSON). The extension opens a port per
/// request, so this answers one message; when Chromium closes the port,
/// stdin ends, the socket closes, and the service stops waiting.
pub fn relay() -> Result<(), String> {
    let mut input = std::io::stdin().lock();
    let mut length = [0u8; 4];
    if input.read_exact(&mut length).is_err() {
        return Ok(());
    }
    let mut message = vec![0u8; u32::from_ne_bytes(length) as usize];
    input.read_exact(&mut message).map_err(|e| e.to_string())?;
    drop(input);
    let reply = converse(&message).unwrap_or_else(|error| {
        refused(authenticator_refusal(
            "NotAllowedError",
            &format!("Hotline: this computer's service did not answer ({error})"),
        ))
        .to_string()
    });
    let reply = reply.trim_end().as_bytes();
    let mut output = std::io::stdout().lock();
    output
        .write_all(&(reply.len() as u32).to_ne_bytes())
        .and_then(|()| output.write_all(reply))
        .and_then(|()| output.flush())
        .map_err(|e| e.to_string())
}

fn converse(message: &[u8]) -> Result<String, String> {
    let mut stream =
        std::os::unix::net::UnixStream::connect(socket_path()).map_err(|e| e.to_string())?;
    stream.write_all(message).map_err(|e| e.to_string())?;
    stream.write_all(b"\n").map_err(|e| e.to_string())?;
    // The port's end is stdin's end: close the socket with it.
    let closer = stream.try_clone().map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
        let _ = closer.shutdown(std::net::Shutdown::Both);
    });
    let mut reply = String::new();
    BufReader::new(stream)
        .read_line(&mut reply)
        .map_err(|e| e.to_string())?;
    if reply.is_empty() {
        return Err("no reply".into());
    }
    Ok(reply)
}
