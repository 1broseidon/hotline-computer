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
    let message = serde_json::from_str::<Value>(&line);
    if let Ok(message) = &message
        && message["kind"] == "watch"
    {
        return watch(&app, &mut lines, &mut write).await;
    }
    let reply = match message {
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

/// Where the making of a passkey stands, for the extension to show: never a
/// key, only what the person needs to see.
fn arming(app: &App) -> Value {
    let Some((rp_id, expires_at)) = app.passkeys.armed() else {
        return json!({"state": "idle"});
    };
    let state = match (app.passkeys.minted(), app.passkeys.asked()) {
        (Some(_), _) => "registered",
        (None, Some((_, Some(true)))) => "approved",
        (None, Some((_, _))) => "asked",
        (None, None) => "armed",
    };
    let ask = app.passkeys.asked().map(
        |(ask, _)| json!({"origin": ask.origin, "rpName": ask.rp_name, "userName": ask.user_name}),
    );
    json!({"state": state, "rpId": rp_id, "expiresAt": expires_at, "ask": ask})
}

/// The extension's standing line: the arming as it changes, until the
/// extension hangs up.
async fn watch(
    app: &App,
    lines: &mut tokio::io::Lines<tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>>,
    write: &mut tokio::net::unix::OwnedWriteHalf,
) {
    let mut shown = Value::Null;
    loop {
        let now = json!({"passkeys": arming(app)});
        if now != shown {
            if write
                .write_all(format!("{now}\n").as_bytes())
                .await
                .is_err()
            {
                return;
            }
            shown = now;
        }
        tokio::select! {
            () = tokio::time::sleep(std::time::Duration::from_millis(300)) => {}
            _ = lines.next_line() => return,
        }
    }
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
        // The person's logins, for the page whose origin Chromium names:
        // what may be filled there, then, on a click, the values themselves.
        Some("logins") => {
            let origin = message["origin"].as_str().unwrap_or_default();
            let logins: Vec<Value> = app
                .secrets
                .logins_for(origin)
                .into_iter()
                .map(|(name, username, code)| json!({"name": name, "username": username, "code": code}))
                .collect();
            json!({"logins": logins})
        }
        Some("fill") => fill(app, message),
        _ => refused(authenticator_refusal(
            "NotSupportedError",
            "this computer does not answer that",
        )),
    }
}

/// A login's values, for a page it may be typed on and nowhere else.
fn fill(app: &App, message: &Value) -> Value {
    let origin = message["origin"].as_str().unwrap_or_default();
    let name = message["name"].as_str().unwrap_or_default();
    let allowed = app
        .secrets
        .logins_for(origin)
        .into_iter()
        .find(|(n, _, _)| n == name);
    let Some((_, _, code)) = allowed else {
        return refused(authenticator_refusal(
            "NotAllowedError",
            &format!("{name} is not a login for {origin}"),
        ));
    };
    let value = |field: &str| {
        app.secrets
            .resolve(&format!("{name}.{field}"))
            .map(|f| f.value)
    };
    match (value("username"), value("password")) {
        (Ok(username), Ok(password)) => json!({
            "username": username,
            "password": password,
            "code": if code { value("code").ok() } else { None },
        }),
        (Err(error), _) | (_, Err(error)) => {
            refused(authenticator_refusal("NotAllowedError", &error))
        }
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
/// request, so this takes one message and passes back every line the
/// service answers until it hangs up: one for a request, a stream for the
/// watch. When Chromium closes the port, stdin ends, the socket closes, and
/// the service stops.
pub fn relay() -> Result<(), String> {
    let mut input = std::io::stdin().lock();
    let mut length = [0u8; 4];
    if input.read_exact(&mut length).is_err() {
        return Ok(());
    }
    let mut message = vec![0u8; u32::from_ne_bytes(length) as usize];
    input.read_exact(&mut message).map_err(|e| e.to_string())?;
    drop(input);
    let mut output = std::io::stdout().lock();
    let mut send = |reply: &str| -> Result<(), String> {
        let reply = reply.trim_end().as_bytes();
        output
            .write_all(&(reply.len() as u32).to_ne_bytes())
            .and_then(|()| output.write_all(reply))
            .and_then(|()| output.flush())
            .map_err(|e| e.to_string())
    };
    let stream = match open(&message) {
        Ok(stream) => stream,
        Err(error) => {
            return send(
                &refused(authenticator_refusal(
                    "NotAllowedError",
                    &format!("Hotline: this computer's service did not answer ({error})"),
                ))
                .to_string(),
            );
        }
    };
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { break };
        send(&line)?;
    }
    Ok(())
}

fn open(message: &[u8]) -> Result<std::os::unix::net::UnixStream, String> {
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
    Ok(stream)
}
