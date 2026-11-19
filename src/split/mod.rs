//! The split computer (0.13). Booted as root, `boot` becomes PID 1 with only
//! `SETUID` and `SETGID`, holds the door and proxy sockets for the
//! container's life, and starts two processes under their own users:
//!
//! | uid  | name      | runs                                                      | holds |
//! |------|-----------|-----------------------------------------------------------|-------|
//! | 0    | `boot`    | the reaper and launcher (`init`)                          | nothing secret |
//! | 1001 | `hotline` | the broker: the HTTP and MCP door, secrets, holds, the passkey signer, the bridge (`broker`) | values |
//! | 1002 | `signin`  | the signed-in browser, which PID 1 starts for the broker  | only signed-in sessions |
//! | 1000 | `agent`   | the runner: jobs, files, the desktop and its viewer, the general browser, Xvfb, dbus, the keyring (`runner`) | readable variables |
//!
//! Booted as any other user, the image is today's single-user computer: the
//! legacy mode, which `GET /capabilities` names so a desk sends it nothing it
//! could not keep.
//!
//! The descriptors each process is started with:
//!
//! | fd | broker                          | runner                    |
//! |----|---------------------------------|---------------------------|
//! | 3  | the line to the runner          | the line to the broker    |
//! | 4  | the door's listener (`:8787`)   | HTTP connections handed over by the broker |
//! | 5  | the line to PID 1               | —                         |
//! | 6  | the proxy's listener (`127.0.0.1:3128`) | —                 |
//! | 7  | HTTP connections handed to the runner | —                   |

pub mod broker;
#[cfg(target_os = "linux")]
pub mod init;
pub mod migration;
pub mod runner;
pub mod wire;

use std::sync::OnceLock;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::App;

pub const AGENT_UID: u32 = 1000;
pub const BROKER_UID: u32 = 1001;
pub const SIGNIN_UID: u32 = 1002;

pub const RUN_DIR: &str = "/run/hotline-computer";
/// The broker's own state: `hotline`, 0700.
pub const BROKER_DIR: &str = "/run/hotline-computer/broker";
/// `hotline:signin`, setgid, 2750: the bridge socket inside is 0660, and
/// only a peer running as `signin` is answered there.
pub const BRIDGE_DIR: &str = "/run/hotline-computer/bridge";
pub const BRIDGE_SOCKET: &str = "/run/hotline-computer/bridge/bridge.sock";
/// What every user may read: the proxy's CA bundle. `hotline`, 0755.
pub const PUBLIC_DIR: &str = "/run/hotline-computer/public";
pub const CA_BUNDLE: &str = "/run/hotline-computer/public/ca-bundle.pem";
/// The bridge for the general browser's extension, until the signed-in
/// browser carries the extension instead: reachable by `agent`, as the
/// single-user computer's is, and answering `agent` peers alone.
pub const INTERIM_BRIDGE_SOCKET: &str = "/run/hotline-computer/public/bridge.sock";
/// The signed-in browser's profile: `signin`, 0700, in the container's own
/// layer, so a recreated computer starts it clean.
pub const SIGNIN_DIR: &str = "/var/lib/hotline-computer/signin";
pub const SIGNIN_PROFILE: &str = "/var/lib/hotline-computer/signin/profile";
pub const PROXY_ADDR: &str = "127.0.0.1:3128";
pub const AGENT_HOME: &str = "/home/agent";

pub mod fd {
    use std::os::fd::RawFd;
    /// Broker and runner: the line between them.
    pub const LINE: RawFd = 3;
    /// Broker: the door's listener. Runner: HTTP connections from the broker.
    pub const DOOR: RawFd = 4;
    pub const HANDED: RawFd = 4;
    /// Broker: the line to PID 1.
    pub const INIT: RawFd = 5;
    /// Broker: the proxy's listener.
    pub const PROXY: RawFd = 6;
    /// Broker: where it hands HTTP connections to the runner.
    pub const HANDOFF: RawFd = 7;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Booted as root: the users above.
    Split,
    /// Booted as `agent`: one user for everything, as 0.12.
    Legacy,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Split => "split",
            Mode::Legacy => "legacy",
        }
    }
}

static MODE: OnceLock<Mode> = OnceLock::new();

/// Set once, by the broker and the runner as they start.
pub fn set_mode(mode: Mode) {
    let _ = MODE.set(mode);
}

pub fn mode() -> Mode {
    MODE.get().copied().unwrap_or(Mode::Legacy)
}

/// Whether this process may split the computer: it was started as root.
pub fn booted_as_root() -> bool {
    // SAFETY: geteuid takes nothing and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

/// This start of the computer's door, which a desk compares across
/// `/capabilities` answers: a new one means everything is delivered again.
pub fn boot_id() -> &'static str {
    static BOOT: OnceLock<String> = OnceLock::new();
    BOOT.get_or_init(|| {
        use ring::rand::SecureRandom;
        let mut bytes = [0u8; 16];
        ring::rand::SystemRandom::new()
            .fill(&mut bytes)
            .expect("the system has randomness");
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    })
}

#[derive(Deserialize)]
pub struct Nonce {
    #[serde(default)]
    nonce: String,
}

/// `GET /capabilities?nonce=N`: what this computer keeps, and proof that
/// the holder of the bearer is answering, not something that took the port.
pub async fn capabilities(State(app): State<App>, Query(nonce): Query<Nonce>) -> Response {
    if nonce.nonce.is_empty() || nonce.nonce.len() > 256 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "nonce is required: 1 to 256 characters"})),
        )
            .into_response();
    }
    Json(capabilities_for(
        mode(),
        boot_id(),
        app.config.token.as_deref(),
        &nonce.nonce,
    ))
    .into_response()
}

pub fn capabilities_for(mode: Mode, boot: &str, bearer: Option<&str>, nonce: &str) -> Value {
    use hmac::{Hmac, Mac};
    let attest = bearer.map(|bearer| {
        let mut mac = Hmac::<sha2::Sha256>::new_from_slice(bearer.as_bytes())
            .expect("HMAC takes a key of any length");
        mac.update(format!("hotline-broker-v1|{boot}|{nonce}").as_bytes());
        base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
    });
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "boot": boot,
        "protected": {"v": 1, "mode": mode.name()},
        "attest": attest,
    })
}

/// The broker's hold on the runner: the line, and where HTTP connections
/// for the desktop's own pages are handed over.
#[derive(Clone)]
pub struct Runner {
    pub line: wire::Peer,
    pub handoff: std::sync::Arc<std::sync::Mutex<std::os::fd::OwnedFd>>,
    pub ready: tokio::sync::watch::Receiver<bool>,
}

impl Runner {
    pub async fn call(&self, op: &str, args: Value) -> Result<Value, String> {
        self.line.call(op, args).await
    }
}

// What the broker's half asks of the desktop's half. In the single-user
// computer each is the local call it always was.

/// Who holds the computer through the viewer now, if the person does.
pub async fn person(app: &App) -> Option<String> {
    match &app.runner {
        None => app.access.person().await,
        Some(runner) => runner
            .call("access.person", Value::Null)
            .await
            .ok()
            .and_then(|answer| answer.as_str().map(str::to_owned)),
    }
}

/// Whoever holds the computer, and until when.
pub async fn holder(app: &App) -> Option<(String, u64)> {
    match &app.runner {
        None => app.access.holder().await,
        Some(runner) => runner
            .call("access.holder", Value::Null)
            .await
            .ok()
            .and_then(|answer| serde_json::from_value(answer).ok()),
    }
}

pub async fn release(app: &App, holder: &str) -> Result<bool, String> {
    match &app.runner {
        None => app.access.release(holder).await,
        Some(runner) => runner
            .call("access.release", json!({"holder": holder}))
            .await
            .map(|answer| answer.as_bool().unwrap_or(false)),
    }
}

/// The agent as the desktop's bar shows it.
pub async fn agent(app: &App) -> crate::agent::Agent {
    match &app.runner {
        None => app.jobs.agent.snapshot(),
        Some(runner) => match runner.call("agent.snapshot", Value::Null).await {
            Ok(answer) => {
                serde_json::from_value(answer).unwrap_or_else(|_| app.jobs.agent.snapshot())
            }
            Err(_) => app.jobs.agent.snapshot(),
        },
    }
}

pub async fn current_url_if_idle(app: &App) -> Option<String> {
    match &app.runner {
        None => app.browser.current_url_if_idle().await,
        Some(runner) => runner
            .call("browser.current_url", json!({"idle": true}))
            .await
            .ok()
            .and_then(|answer| answer.as_str().map(str::to_owned)),
    }
}

pub async fn current_url(app: &App) -> Result<String, String> {
    match &app.runner {
        None => app.browser.current_url().await,
        Some(runner) => runner
            .call("browser.current_url", json!({"idle": false}))
            .await
            .map(|answer| answer.as_str().unwrap_or_default().to_owned()),
    }
}

pub async fn sync_passkeys(app: &App, launch: bool) -> Result<(), String> {
    match &app.runner {
        None => app.browser.sync_passkeys(launch).await,
        Some(runner) => runner
            .call("browser.sync_passkeys", json!({"launch": launch}))
            .await
            .map(|_| ()),
    }
}

/// After a delivery: the bar's count of sealed secrets, and the passkeys
/// the browser knows of. Split, the runner is given what its jobs may read
/// and nothing else: each variable, which is readable until a key may be
/// bound to a service, and the names, kinds and sites `state info` lists.
pub async fn delivered(app: &App) {
    match &app.runner {
        None => {
            let _ =
                crate::x11::publish_sealed(&app.config.display, app.secrets.names().len() as u32);
        }
        Some(runner) => {
            let pushed = runner
                .call(
                    "secrets.set",
                    json!({
                        "variables": app.secrets.environment(),
                        "catalog": app.secrets.catalog(),
                        "count": app.secrets.names().len(),
                    }),
                )
                .await;
            if let Err(error) = pushed {
                eprintln!("secrets: the desktop did not take the readable set: {error}");
            }
        }
    }
    if let Err(error) = sync_passkeys(app, false).await {
        eprintln!("secrets: the browser did not take the passkeys: {error}");
    }
}

/// A tool call through the split computer's door: the runner does the work
/// and the broker takes every value out of the answer. A login's field is
/// looked up, checked and held here, and the runner is handed the value
/// for that one fill.
pub async fn call_tool(
    app: &App,
    runner: &Runner,
    name: &str,
    arguments: Value,
    holder: &str,
) -> crate::tools::ToolResult {
    let result = if name == "browser" && crate::tools::browser::fills_a_secret(&arguments) {
        crate::tools::browser::fill_secret_through(app, runner, arguments, holder).await
    } else {
        call_runner(runner, name, arguments, holder, None).await
    };
    app.secrets.redact_result(result)
}

pub async fn call_runner(
    runner: &Runner,
    name: &str,
    arguments: Value,
    holder: &str,
    filled: Option<&crate::secrets::Filled>,
) -> crate::tools::ToolResult {
    let filled = filled.map(|filled| {
        json!({
            "label": filled.label,
            "value": filled.value,
            "sites": filled.sites.as_ref().map(|sites| sites.iter().map(ToString::to_string).collect::<Vec<_>>()),
        })
    });
    let answer = runner
        .call(
            "tool.call",
            json!({"name": name, "arguments": arguments, "holder": holder, "filled": filled}),
        )
        .await?;
    serde_json::from_value(answer["content"].clone()).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_attest_the_bearer_holder_and_name_the_mode() {
        let answer = capabilities_for(Mode::Split, "b00t", Some("bearer"), "n1");
        assert_eq!(answer["protected"], json!({"v": 1, "mode": "split"}));
        assert_eq!(answer["boot"], "b00t");
        assert_eq!(answer["version"], env!("CARGO_PKG_VERSION"));
        use hmac::{Hmac, Mac};
        let mut mac = Hmac::<sha2::Sha256>::new_from_slice(b"bearer").unwrap();
        mac.update(b"hotline-broker-v1|b00t|n1");
        let expected =
            base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
        assert_eq!(answer["attest"], expected);
        let legacy = capabilities_for(Mode::Legacy, "b00t", Some("bearer"), "n2");
        assert_eq!(legacy["protected"]["mode"], "legacy");
        assert_ne!(legacy["attest"], answer["attest"], "the nonce is in it");
    }
}
