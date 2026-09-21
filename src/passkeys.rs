//! Passkeys the teammate owns, and the one moment one is made.
//!
//! A person's own passkeys never leave their authenticator, so a teammate
//! gets its own: a WebAuthn credential the browser keeps in a virtual
//! authenticator on every tab (see `crate::browser`), loaded from the
//! granted set at start and on every change, and used when a site calls
//! `navigator.credentials.get()` inside Chromium, where the private key is
//! never typed and never answered.
//!
//! Making one is the operator's act, twice over. The desk arms this
//! computer for one site with `PUT /passkeys/registration`, for ten
//! minutes. While armed, and for that site alone, a site's call to
//! `navigator.credentials.create()` is not answered by the browser: the
//! guard parks it, and the next look — every action, every poll — reads
//! what the site asked for and records it here as the request waiting for
//! the person. `GET /passkeys/registration` answers `asked` with it; the
//! desk shows the person a card, and carries their answer back through
//! `POST /passkeys/registration/answer`. Approved, the page is told to go
//! ahead, the authenticator mints, and the next look keeps the credential
//! for the desk, which polls until it is answered, stores it in the vault,
//! delivers the set with it, and `DELETE`s the arming. Denied, the site
//! hears no and the arming is over. That answer is the one time a private
//! key leaves this process: over the bearer-guarded loopback door, in the
//! direction the cookie import already trusts. A credential minted outside
//! an arming, without an approval, for another site, or after the ten
//! minutes is removed from the authenticator on the next look, so a teammate
//! cannot give itself a passkey, and cannot ask the person for one unless
//! the person armed the site first.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::App;
use crate::secrets::{Passkey, Secret};

/// How long an arming lasts: long enough to find the site's security page
/// and add a passkey there, short enough that a forgotten one closes itself.
pub const ARMED_FOR: Duration = Duration::from_secs(10 * 60);

/// How long an approved request is remembered after the page was told to
/// go ahead and before a credential shows up: a mint takes well under a
/// second, so one not seen by then failed on the site's side, and the next
/// request the site makes is a new one for the person.
const MINT_PATIENCE: Duration = Duration::from_secs(30);

/// What a site asked for when it called `navigator.credentials.create()`
/// under an arming, read off the request by the guard and answered to the
/// desk as it is, so the card can say which site, from which origin, and
/// for which account the passkey would be.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ask {
    /// Made by the guard that parked the request; the answer names it.
    pub id: String,
    pub rp_id: String,
    pub origin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rp_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_display_name: Option<String>,
    /// Unix milliseconds, when this service first saw it.
    #[serde(default)]
    pub asked_at: i64,
}

/// The request under the arming, and where the person's answer stands.
#[derive(Clone, Debug)]
struct Asked {
    ask: Ask,
    /// `None` while the person has not answered.
    answer: Option<bool>,
    /// When the answer reached the page.
    delivered: Option<Instant>,
}

#[derive(Clone, Debug)]
struct Arming {
    rp_id: String,
    since: Instant,
    /// Unix milliseconds, for the desk's clock.
    expires_at: i64,
    asked: Option<Asked>,
    minted: Option<Passkey>,
}

/// What a look does with a request a page reports parked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reported {
    /// The request the person is being asked about, or is about to be: it
    /// stays parked.
    Waiting,
    /// The person approved it: the page is told to go ahead.
    Approved,
    /// The person denied it: the page is told no.
    Denied,
    /// Another request is before the person first, or nothing is armed for
    /// this one: it stays parked until then, or until the guard drops it.
    Later,
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
        let expires_at = now_ms() + i64::try_from(ARMED_FOR.as_millis()).unwrap_or(i64::MAX);
        *self.lock() = Some(Arming {
            rp_id: rp_id.to_owned(),
            since: Instant::now(),
            expires_at,
            asked: None,
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

    /// The request under the arming, and the person's answer to it so far.
    pub fn asked(&self) -> Option<(Ask, Option<bool>)> {
        self.current()
            .and_then(|arming| arming.asked)
            .map(|asked| (asked.ask, asked.answer))
    }

    /// The credential minted under the arming, if one was.
    pub fn minted(&self) -> Option<Passkey> {
        self.current().and_then(|arming| arming.minted)
    }

    /// A request a page reports parked. The first one under an arming is
    /// recorded as the request before the person; the same one reported
    /// again answers where the person's answer stands; another while one
    /// is before the person waits its turn, unless the earlier one was
    /// answered and told, in which case its page has moved on and this is
    /// the request now.
    pub fn report(&self, ask: Ask) -> Reported {
        let mut guard = self.lock();
        let Some(arming) = guard
            .as_mut()
            .filter(|arming| arming.since.elapsed() < ARMED_FOR)
        else {
            *guard = None;
            return Reported::Later;
        };
        if arming.minted.is_some() || !rp_within(&ask.rp_id, &arming.rp_id) {
            return Reported::Later;
        }
        match arming.asked.as_ref() {
            Some(asked) if asked.ask.id == ask.id => match asked.answer {
                None => Reported::Waiting,
                Some(true) => Reported::Approved,
                Some(false) => Reported::Denied,
            },
            Some(asked) if asked.delivered.is_none() => Reported::Later,
            _ => {
                arming.asked = Some(Asked {
                    ask: Ask {
                        asked_at: now_ms(),
                        ..ask
                    },
                    answer: None,
                    delivered: None,
                });
                Reported::Waiting
            }
        }
    }

    /// The person's answer reached the page.
    pub fn delivered(&self, id: &str) {
        if let Some(asked) = self
            .lock()
            .as_mut()
            .and_then(|arming| arming.asked.as_mut())
            .filter(|asked| asked.ask.id == id)
        {
            asked.delivered = Some(Instant::now());
        }
    }

    /// After every tab was looked at, with the ids of the requests they
    /// hold: a recorded request none holds any more went with its document
    /// — unless it was approved and told a moment ago, since a page making
    /// a passkey no longer parks the request it is making.
    pub fn reconcile(&self, seen: &HashSet<String>) {
        let mut guard = self.lock();
        let Some(arming) = guard.as_mut() else {
            return;
        };
        let gone = arming.asked.as_ref().is_some_and(|asked| {
            let making = asked.answer == Some(true)
                && asked
                    .delivered
                    .is_some_and(|at| at.elapsed() < MINT_PATIENCE);
            !(seen.contains(&asked.ask.id) || making)
        });
        if gone {
            arming.asked = None;
        }
    }

    /// The person approved the request `id`.
    pub fn approve(&self, id: &str) -> Result<(), String> {
        self.answer(id, true)
    }

    /// The person denied the request `id`. The door ends the arming with
    /// it, once the page has been told.
    pub fn deny(&self, id: &str) -> Result<(), String> {
        self.answer(id, false)
    }

    fn answer(&self, id: &str, approved: bool) -> Result<(), String> {
        let mut guard = self.lock();
        let asked = guard
            .as_mut()
            .filter(|arming| arming.since.elapsed() < ARMED_FOR)
            .and_then(|arming| arming.asked.as_mut())
            .filter(|asked| asked.ask.id == id)
            .ok_or_else(|| "no passkey request with that id is waiting for an answer".to_owned())?;
        match asked.answer {
            None => {
                asked.answer = Some(approved);
                Ok(())
            }
            Some(earlier) if earlier == approved => Ok(()),
            Some(_) => Err("that passkey request was already answered the other way".to_owned()),
        }
    }

    /// Keeps a credential the authenticator minted, if it is the one
    /// awaited: made for the armed site, under a request the person
    /// approved, while none is kept yet. Answers whether it was.
    pub fn keep(&self, passkey: Passkey) -> bool {
        let mut guard = self.lock();
        let Some(arming) = guard
            .as_mut()
            .filter(|arming| arming.since.elapsed() < ARMED_FOR)
        else {
            *guard = None;
            return false;
        };
        let approved = arming
            .asked
            .as_ref()
            .is_some_and(|asked| asked.answer == Some(true));
        if arming.minted.is_some() || !rp_within(&passkey.rp_id, &arming.rp_id) || !approved {
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

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
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

/// The body of `POST /passkeys/registration/answer`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Answer {
    id: String,
    approved: bool,
}

fn status(passkeys: &Passkeys) -> serde_json::Value {
    let Some((rp_id, expires_at)) = passkeys.armed() else {
        return json!({"state": "idle"});
    };
    let asked = passkeys.asked();
    let state = match (passkeys.minted(), &asked) {
        (Some(_), _) => "registered",
        (None, Some((_, None))) => "asked",
        (None, Some((_, Some(true)))) => "approved",
        (None, Some((_, Some(false)))) => "denied",
        (None, None) => "armed",
    };
    let mut status = json!({"state": state, "rpId": rp_id, "expiresAt": expires_at});
    if let Some((ask, _)) = asked {
        status["ask"] = json!(ask);
    }
    if let Some(credential) = passkeys.minted() {
        // As a whole entry, kind and all, so the desk can deliver it back
        // with `PUT /secrets` as it is.
        status["credential"] = json!(Secret::Passkey(credential));
    }
    status
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

/// `GET /passkeys/registration`: `idle`; `armed`; `asked` with the request
/// the site made, for the person to answer; `approved` while the page is
/// making it; or `registered` with the minted credential, which is
/// answered until the arming is deleted. The browser is looked at on every
/// call, so a request the person just raised through the screen is found,
/// and a passkey just made is kept, without any tool being used.
pub async fn registration(State(app): State<App>) -> Response {
    if app.passkeys.armed().is_some()
        && let Err(error) = app.browser.sync_passkeys(false).await
    {
        eprintln!("passkeys: the browser could not be looked at: {error}");
    }
    Json(status(&app.passkeys)).into_response()
}

/// `POST /passkeys/registration/answer {"id": …, "approved": true}`: the
/// person's answer to the request the site made. Approved, the page is
/// told to go ahead and the browser makes the passkey, which the next `GET`
/// answers as `registered`. Denied, the site hears no and the arming ends
/// with it: a new arming is what it takes to ask again, so a site, or a
/// teammate, cannot keep asking. 409 when no request with that id is
/// waiting for an answer.
pub async fn answer(State(app): State<App>, Json(answer): Json<Answer>) -> Response {
    let answered = if answer.approved {
        app.passkeys.approve(&answer.id)
    } else {
        app.passkeys.deny(&answer.id)
    };
    if let Err(error) = answered {
        return (StatusCode::CONFLICT, Json(json!({"error": error}))).into_response();
    }
    if let Err(error) = app.browser.sync_passkeys(false).await {
        eprintln!("passkeys: the browser could not be told the answer: {error}");
    }
    if !answer.approved {
        app.passkeys.disarm();
        if let Err(error) = app.browser.sync_passkeys(false).await {
            eprintln!("passkeys: the browser did not drop the arming: {error}");
        }
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

    fn ask(id: &str, rp_id: &str) -> Ask {
        Ask {
            id: id.to_owned(),
            rp_id: rp_id.to_owned(),
            origin: format!("https://{rp_id}"),
            rp_name: Some("The site".to_owned()),
            user_name: Some("george".to_owned()),
            user_display_name: Some("George".to_owned()),
            asked_at: 0,
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
    fn a_request_waits_for_the_person_and_an_approval_lets_one_credential_be_kept() {
        let passkeys = Passkeys::default();
        assert!(passkeys.armed().is_none());
        assert_eq!(
            passkeys.report(ask("r1", "github.com")),
            Reported::Later,
            "nothing is recorded unarmed"
        );
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
        assert_eq!(status(&passkeys)["state"], "armed");
        assert_eq!(
            passkeys.report(ask("r0", "gitlab.com")),
            Reported::Later,
            "another site's request is not before the person"
        );
        assert!(
            !passkeys.keep(passkey("github.com")),
            "nothing is kept before the person approved"
        );
        assert_eq!(
            passkeys.report(ask("r1", "login.github.com")),
            Reported::Waiting
        );
        assert_eq!(
            passkeys.report(ask("r2", "github.com")),
            Reported::Later,
            "one request before the person at a time"
        );
        let asked = status(&passkeys);
        assert_eq!(asked["state"], "asked", "{asked}");
        assert_eq!(asked["ask"]["id"], "r1");
        assert_eq!(asked["ask"]["rpId"], "login.github.com");
        assert_eq!(asked["ask"]["userName"], "george");
        assert!(asked["ask"]["askedAt"].as_i64().unwrap_or_default() > 0);
        assert!(asked.get("credential").is_none());
        assert!(passkeys.approve("r2").is_err(), "not the one waiting");
        assert!(
            !passkeys.keep(passkey("github.com")),
            "still nothing is kept before the approval"
        );
        passkeys.approve("r1").unwrap();
        passkeys.approve("r1").unwrap();
        assert!(passkeys.deny("r1").is_err(), "an answer is not taken back");
        assert_eq!(status(&passkeys)["state"], "approved");
        assert_eq!(
            passkeys.report(ask("r1", "login.github.com")),
            Reported::Approved
        );
        passkeys.delivered("r1");
        // The page making it no longer parks it: the approval holds while
        // the mint is under way.
        passkeys.reconcile(&HashSet::new());
        assert_eq!(status(&passkeys)["state"], "approved");
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
        assert_eq!(
            passkeys.report(ask("r3", "github.com")),
            Reported::Later,
            "a registered arming takes no further request"
        );
        let registered = status(&passkeys);
        assert_eq!(registered["state"], "registered");
        assert_eq!(registered["credential"]["rpId"], "github.com");
        assert_eq!(registered["ask"]["id"], "r1");
        assert_eq!(passkeys.disarm().unwrap().rp_id, "github.com");
        assert!(passkeys.armed().is_none());
        assert!(passkeys.minted().is_none());
        assert_eq!(status(&passkeys), json!({"state": "idle"}));
        // A new arming starts clean.
        passkeys.arm("gitlab.com").unwrap();
        assert!(passkeys.minted().is_none());
        assert!(passkeys.asked().is_none());
        assert_eq!(status(&passkeys)["state"], "armed");
    }

    #[test]
    fn a_request_that_left_with_its_page_is_forgotten_and_a_told_one_makes_way() {
        let passkeys = Passkeys::default();
        passkeys.arm("github.com").unwrap();
        assert_eq!(passkeys.report(ask("r1", "github.com")), Reported::Waiting);
        // The page navigated away: no tab holds r1 any more.
        passkeys.reconcile(&HashSet::new());
        assert_eq!(status(&passkeys)["state"], "armed");
        assert!(passkeys.approve("r1").is_err());
        // A request that is still held stays.
        assert_eq!(passkeys.report(ask("r2", "github.com")), Reported::Waiting);
        passkeys.reconcile(&HashSet::from(["r2".to_owned()]));
        assert_eq!(status(&passkeys)["ask"]["id"], "r2");
        // Approved and told, the page runs the site's request; when the
        // site asks again afterwards, that is the request now.
        passkeys.approve("r2").unwrap();
        passkeys.delivered("r2");
        assert_eq!(passkeys.report(ask("r3", "github.com")), Reported::Waiting);
        assert_eq!(status(&passkeys)["state"], "asked");
        assert_eq!(status(&passkeys)["ask"]["id"], "r3");
        // An approval told long ago with nothing minted is let go.
        passkeys.approve("r3").unwrap();
        passkeys.delivered("r3");
        passkeys
            .lock()
            .as_mut()
            .and_then(|arming| arming.asked.as_mut())
            .unwrap()
            .delivered = Some(Instant::now() - MINT_PATIENCE);
        passkeys.reconcile(&HashSet::new());
        assert_eq!(status(&passkeys)["state"], "armed");
    }

    #[test]
    fn a_denial_is_told_to_the_page_and_the_door_ends_the_arming() {
        let passkeys = Passkeys::default();
        passkeys.arm("github.com").unwrap();
        assert_eq!(passkeys.report(ask("r1", "github.com")), Reported::Waiting);
        passkeys.deny("r1").unwrap();
        assert_eq!(status(&passkeys)["state"], "denied");
        assert_eq!(passkeys.report(ask("r1", "github.com")), Reported::Denied);
        assert!(!passkeys.keep(passkey("github.com")), "denied is not kept");
        assert!(passkeys.disarm().is_none());
        assert_eq!(status(&passkeys), json!({"state": "idle"}));
    }

    #[test]
    fn an_arming_past_its_time_is_gone() {
        let passkeys = Passkeys::default();
        passkeys.arm("github.com").unwrap();
        passkeys.lock().as_mut().unwrap().since = Instant::now() - ARMED_FOR;
        assert!(passkeys.armed().is_none());
        assert_eq!(passkeys.report(ask("r1", "github.com")), Reported::Later);
        assert!(!passkeys.keep(passkey("github.com")));
        assert!(passkeys.disarm().is_none());
    }
}
