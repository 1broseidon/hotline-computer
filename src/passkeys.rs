//! Passkeys the teammate owns, and the one moment one is made.
//!
//! A person's own passkeys never leave their authenticator, so a teammate
//! gets its own: a WebAuthn credential the browser keeps in a virtual
//! authenticator on every tab (see `crate::browser`), loaded from the
//! granted set at start and on every change, and used when a site calls
//! `navigator.credentials.get()` inside Chromium, where the private key is
//! never typed and never answered.
//!
//! Making one is the operator's act. The desk arms this computer for one
//! site with `PUT /passkeys/registration`, for ten minutes. While armed, and
//! for that site alone, `navigator.credentials.create()` may mint a
//! credential: the person adds a passkey in the site's security settings
//! through the computer's screen, or asks the teammate to. The desk polls
//! `GET /passkeys/registration` until the minted credential is answered,
//! stores it in the vault, delivers the set with it, and `DELETE`s the
//! arming. That answer is the one time a private key leaves this process:
//! over the bearer-guarded loopback door, in the direction the cookie import
//! already trusts. A credential minted outside an arming, for another site,
//! or after the ten minutes is removed from the authenticator on the next
//! look, so a teammate cannot give itself a passkey.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::App;
use crate::secrets::{Passkey, Secret};

/// How long an arming lasts: long enough to find the site's security page
/// and add a passkey there, short enough that a forgotten one closes itself.
pub const ARMED_FOR: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Debug)]
struct Arming {
    rp_id: String,
    since: Instant,
    /// Unix milliseconds, for the desk's clock.
    expires_at: i64,
    minted: Option<Passkey>,
}

/// The arming, if any, shared between the door the desk uses and the
/// browser that watches its authenticators.
#[derive(Clone, Default)]
pub struct Passkeys(Arc<Mutex<Option<Arming>>>);

impl Passkeys {
    /// Arms this computer for `rp_id`, replacing any earlier arming, and
    /// answers when the arming ends.
    pub fn arm(&self, rp_id: &str) -> Result<i64, String> {
        check_rp_id(rp_id)?;
        let expires_at = chrono::Utc::now().timestamp_millis()
            + i64::try_from(ARMED_FOR.as_millis()).unwrap_or(i64::MAX);
        *self.lock() = Some(Arming {
            rp_id: rp_id.to_owned(),
            since: Instant::now(),
            expires_at,
            minted: None,
        });
        Ok(expires_at)
    }

    /// The site armed right now, and when the arming ends; one past its
    /// time is forgotten here.
    pub fn armed(&self) -> Option<(String, i64)> {
        self.current()
            .map(|arming| (arming.rp_id, arming.expires_at))
    }

    /// The credential minted under the arming, if one was.
    pub fn minted(&self) -> Option<Passkey> {
        self.current().and_then(|arming| arming.minted)
    }

    /// Keeps a credential the authenticator minted, if it is the one awaited:
    /// made for the armed site while none is kept yet. Answers whether it was.
    pub fn keep(&self, passkey: Passkey) -> bool {
        let mut guard = self.lock();
        let Some(arming) = guard
            .as_mut()
            .filter(|arming| arming.since.elapsed() < ARMED_FOR)
        else {
            *guard = None;
            return false;
        };
        if arming.minted.is_some() || !rp_within(&passkey.rp_id, &arming.rp_id) {
            return false;
        }
        arming.minted = Some(passkey);
        true
    }

    /// Ends the arming and answers what it minted, for the browser to drop
    /// unless the set delivered since carries it.
    pub fn disarm(&self) -> Option<Passkey> {
        self.lock().take().and_then(|arming| arming.minted)
    }

    fn current(&self) -> Option<Arming> {
        let mut guard = self.lock();
        if guard
            .as_ref()
            .is_some_and(|arming| arming.since.elapsed() >= ARMED_FOR)
        {
            *guard = None;
        }
        guard.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Arming>> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// A relying party id is a host name in lower case: `github.com`, never an
/// address with a scheme, a port or a path.
pub fn check_rp_id(rp_id: &str) -> Result<(), String> {
    let shaped = !rp_id.is_empty()
        && rp_id.len() <= 253
        && !rp_id.starts_with('.')
        && !rp_id.ends_with('.')
        && !rp_id.contains("..")
        && rp_id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-');
    if shaped {
        Ok(())
    } else {
        Err(format!(
            "{rp_id:?} is not a site for a passkey: a host name in lower case, such as github.com"
        ))
    }
}

/// Whether a credential for `rp_id` belongs to an arming for `armed`: the
/// same site, or one within the other, since a site may register under its
/// parent domain and a person may arm the page they see.
pub fn rp_within(rp_id: &str, armed: &str) -> bool {
    rp_id == armed || rp_id.ends_with(&format!(".{armed}")) || armed.ends_with(&format!(".{rp_id}"))
}

/// The body of `PUT /passkeys/registration`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Arm {
    rp_id: String,
}

fn status(passkeys: &Passkeys) -> serde_json::Value {
    match passkeys.armed() {
        None => json!({"state": "idle"}),
        Some((rp_id, expires_at)) => match passkeys.minted() {
            Some(credential) => json!({
                "state": "registered", "rpId": rp_id, "expiresAt": expires_at,
                // As a whole entry, kind and all, so the desk can deliver
                // it back with `PUT /secrets` as it is.
                "credential": Secret::Passkey(credential),
            }),
            None => json!({"state": "armed", "rpId": rp_id, "expiresAt": expires_at}),
        },
    }
}

/// `PUT /passkeys/registration {"rpId": "github.com"}`: arms this computer
/// for ten minutes and readies the browser, launching one if none is up,
/// since the person is about to use it. Answers the arming, or 400 for a
/// site that is not one, or 503 when the browser cannot be readied.
pub async fn arm(State(app): State<App>, Json(arm): Json<Arm>) -> Response {
    if let Err(error) = app.passkeys.arm(&arm.rp_id) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": error}))).into_response();
    }
    if let Err(error) = app.browser.sync_passkeys(true).await {
        app.passkeys.disarm();
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": format!("the browser is not ready for a passkey: {error}")})),
        )
            .into_response();
    }
    Json(status(&app.passkeys)).into_response()
}

/// `GET /passkeys/registration`: `idle`, `armed`, or `registered` with the
/// minted credential, which is answered until the arming is deleted. The
/// browser is looked at on every call, so a passkey the person just added
/// through the screen is found without any tool being used.
pub async fn registration(State(app): State<App>) -> Response {
    if app.passkeys.armed().is_some()
        && let Err(error) = app.browser.sync_passkeys(false).await
    {
        eprintln!("passkeys: the browser could not be looked at: {error}");
    }
    Json(status(&app.passkeys)).into_response()
}

/// `DELETE /passkeys/registration`: ends the arming. What it minted stays
/// in the authenticators only if the set delivered since carries it.
pub async fn disarm(State(app): State<App>) -> Response {
    app.passkeys.disarm();
    if let Err(error) = app.browser.sync_passkeys(false).await {
        eprintln!("passkeys: the browser did not drop the arming: {error}");
    }
    StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passkey(rp_id: &str) -> Passkey {
        Passkey {
            rp_id: rp_id.to_owned(),
            credential_id: "AQID".to_owned(),
            private_key: "MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgeE7T2PDJPCRfPvTUFvVcQI7KFSDmnKbrRfEjRGbtV9WhRANCAATfeasaWHkMKJ4oCcdDzVX9c2xUUkC7Uiuqu8tS0LXtRJ8pCk+gNSvvqWaB3WgFNn4rvQ8wS1bH+dOjfgZoq2gz".to_owned(),
            user_handle: None,
            user_name: Some("george".to_owned()),
            user_display_name: None,
        }
    }

    #[test]
    fn a_site_for_a_passkey_is_a_host_name() {
        for rp in ["github.com", "localhost", "login.example.co.uk", "x-y.z"] {
            check_rp_id(rp).unwrap_or_else(|error| panic!("{rp}: {error}"));
        }
        for rp in [
            "",
            "GitHub.com",
            "https://github.com",
            "github.com/",
            "github.com:443",
            ".github.com",
            "a..b",
            "git hub.com",
        ] {
            assert!(check_rp_id(rp).is_err(), "{rp:?}");
        }
        assert!(rp_within("github.com", "github.com"));
        assert!(rp_within("github.com", "login.github.com"));
        assert!(rp_within("login.github.com", "github.com"));
        assert!(!rp_within("github.com", "gitlab.com"));
        assert!(!rp_within("evilgithub.com", "github.com"));
    }

    #[test]
    fn an_arming_keeps_one_credential_for_its_site_and_ends_on_delete() {
        let passkeys = Passkeys::default();
        assert!(passkeys.armed().is_none());
        assert!(
            !passkeys.keep(passkey("github.com")),
            "nothing is kept unarmed"
        );
        assert!(passkeys.arm("GitHub.com").is_err());
        let expires_at = passkeys.arm("github.com").unwrap();
        assert_eq!(
            passkeys.armed(),
            Some(("github.com".to_owned(), expires_at))
        );
        assert!(
            !passkeys.keep(passkey("gitlab.com")),
            "another site's is not kept"
        );
        assert!(passkeys.minted().is_none());
        assert!(passkeys.keep(passkey("github.com")));
        assert!(!passkeys.keep(passkey("github.com")), "one per arming");
        assert_eq!(
            passkeys.minted().unwrap().user_name.as_deref(),
            Some("george")
        );
        let registered = status(&passkeys);
        assert_eq!(registered["state"], "registered");
        assert_eq!(registered["credential"]["rpId"], "github.com");
        assert_eq!(passkeys.disarm().unwrap().rp_id, "github.com");
        assert!(passkeys.armed().is_none());
        assert!(passkeys.minted().is_none());
        assert_eq!(status(&passkeys), json!({"state": "idle"}));
        // A new arming starts clean.
        passkeys.arm("gitlab.com").unwrap();
        assert!(passkeys.minted().is_none());
        assert_eq!(status(&passkeys)["state"], "armed");
    }

    #[test]
    fn an_arming_past_its_time_is_gone() {
        let passkeys = Passkeys::default();
        passkeys.arm("github.com").unwrap();
        passkeys.lock().as_mut().unwrap().since = Instant::now() - ARMED_FOR;
        assert!(passkeys.armed().is_none());
        assert!(!passkeys.keep(passkey("github.com")));
        assert!(passkeys.disarm().is_none());
    }
}
