//! Uses of an ask-me key, held until the person answers.
//!
//! A login or a passkey the desk delivers with `"use": "ask"` is used only
//! once the person approves that use. The computer holds it: a passkey
//! sign-in waits, within the page's own WebAuthn time limit (at most five
//! minutes), and a `browser fill` answers the agent at once that it is
//! waiting, because no tool call may wait that long. The desk polls
//! `GET /secrets/uses` for what is held, shows the person one card per use,
//! and carries the answer back with `POST /secrets/uses/answer`: approve
//! lets this use through; allow lets it through and every later use of the
//! same key on the same site for ten minutes; deny refuses it. Nothing here
//! ever carries a value: a held use names its key, its site and the page.
//!
//! The answer to a fill is collected by the agent's retry: once approved,
//! the retry fills, and so do the other fields of that sign-in for a minute
//! after; once denied, the retry is told so, once. The person's own uses (the
//! computer handed to them through the viewer) are theirs and never held.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::{Value, json};

use crate::App;

/// How long "allow" lets a key through on a site.
pub const ALLOW_FOR_MS: u64 = 10 * 60 * 1000;
/// The longest a held passkey sign-in waits, whatever the page asks for.
pub const WAIT_CAP_MS: u64 = 5 * 60 * 1000;
/// How long a fill's held use waits for the person.
pub const FILL_WAITS_MS: u64 = 5 * 60 * 1000;
/// How long an approved fill waits for the agent to try again.
pub const APPROVAL_KEPT_MS: u64 = 5 * 60 * 1000;
/// After the first field filled under an approval, how long the rest of
/// that sign-in (the password, then a code) fills without asking again.
pub const SIGN_IN_MS: u64 = 60 * 1000;
const SETTLED_KEPT: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Passkey,
    Login,
}

/// A use the computer is holding: what `GET /secrets/uses` lists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Held {
    pub id: String,
    pub secret: String,
    pub kind: Kind,
    pub site: String,
    pub path: String,
    /// Unix milliseconds.
    pub asked_at: u64,
    pub expires_at: u64,
    /// Who the use is for, for the card's record.
    #[serde(skip)]
    who: String,
    /// A fill the agent comes back for: its answer waits for the retry.
    #[serde(skip)]
    retry: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Approved,
    Allowed,
    Denied,
    Expired,
}

impl Outcome {
    /// What the door answers: "allow" approves too.
    pub fn state(self) -> &'static str {
        match self {
            Outcome::Approved | Outcome::Allowed => "approved",
            Outcome::Denied => "denied",
            Outcome::Expired => "expired",
        }
    }

    /// The record line's tail for a use that went through on this answer.
    pub fn how(self) -> &'static str {
        match self {
            Outcome::Approved => "approved at the desk",
            Outcome::Allowed => "allowed 10 min",
            Outcome::Denied => "you denied it",
            Outcome::Expired => "nobody answered in time",
        }
    }
}

/// A use that is no longer held, for the waiter and for the card on the
/// page: newest last, a few of them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settled {
    pub seq: u64,
    pub id: String,
    pub secret: String,
    pub kind: Kind,
    pub site: String,
    pub outcome: Outcome,
    pub at: u64,
    /// When an allowance ends, for "allow".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Approve,
    Allow,
    Deny,
}

/// What a use may do now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pass {
    /// Through, inside an allowance.
    Within,
    /// Through, on the person's answer to this use.
    Answered(Outcome),
    /// Held: the person is being asked, under this id. `lapsed` says an
    /// earlier ask for the same fill went unanswered.
    Held { id: String, lapsed: bool },
    /// The person said no to this fill; the agent hears it once.
    Denied,
}

/// Where a key's fills on a site stand between the agent's tries.
#[derive(Clone, Debug)]
enum Ticket {
    Pending(String),
    Approved { outcome: Outcome, until: u64 },
    Signing { outcome: Outcome, until: u64 },
    Denied,
}

#[derive(Default)]
struct Inner {
    /// Oldest first.
    held: Vec<Held>,
    settled: VecDeque<Settled>,
    seq: u64,
    /// (secret, site) to when its allowance ends.
    allowed: HashMap<(String, String), u64>,
    tickets: HashMap<(String, String), Ticket>,
}

/// The uses held now, the answers given lately and the allowances standing.
/// Kept in memory: a restart forgets them, and a held use with it.
#[derive(Clone, Default)]
pub struct Holds(Arc<Mutex<Inner>>);

impl Holds {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A use that waits for its answer where it is: a passkey sign-in, or a
    /// fill the card on the page asked for. Through at once inside an
    /// allowance; otherwise held for `wait_ms` (capped) under a new id.
    #[allow(clippy::too_many_arguments)]
    pub fn ask(
        &self,
        secret: &str,
        kind: Kind,
        site: &str,
        path: &str,
        who: &str,
        wait_ms: u64,
        now: u64,
    ) -> Pass {
        let mut inner = self.lock();
        prune(&mut inner, now);
        if allowed(&inner, secret, site, now) {
            return Pass::Within;
        }
        let id = hold(
            &mut inner,
            secret,
            kind,
            site,
            path,
            who,
            now + wait_ms.min(WAIT_CAP_MS),
            false,
            now,
        );
        Pass::Held { id, lapsed: false }
    }

    /// A `browser fill` of an ask-me login, which never waits: through when
    /// allowed or approved, else the id it is held under (the one already
    /// held, if the agent tries again before the person answers).
    pub fn fill(&self, secret: &str, site: &str, path: &str, who: &str, now: u64) -> Pass {
        let mut inner = self.lock();
        prune(&mut inner, now);
        if allowed(&inner, secret, site, now) {
            return Pass::Within;
        }
        let key = (secret.to_owned(), site.to_owned());
        let mut lapsed = false;
        match inner.tickets.get(&key).cloned() {
            Some(Ticket::Signing { outcome, until }) if until > now => {
                return Pass::Answered(outcome);
            }
            Some(Ticket::Approved { outcome, until }) if until > now => {
                inner.tickets.insert(
                    key,
                    Ticket::Signing {
                        outcome,
                        until: now + SIGN_IN_MS,
                    },
                );
                return Pass::Answered(outcome);
            }
            Some(Ticket::Denied) => {
                inner.tickets.remove(&key);
                return Pass::Denied;
            }
            Some(Ticket::Pending(id)) if inner.held.iter().any(|held| held.id == id) => {
                return Pass::Held { id, lapsed: false };
            }
            Some(Ticket::Pending(_)) => lapsed = true,
            _ => {}
        }
        let id = hold(
            &mut inner,
            secret,
            Kind::Login,
            site,
            path,
            who,
            now + FILL_WAITS_MS,
            true,
            now,
        );
        inner.tickets.insert(key, Ticket::Pending(id.clone()));
        Pass::Held { id, lapsed }
    }

    /// `None` while the use is held; then how it ended. An id nobody holds
    /// or remembers ended unanswered.
    pub fn status(&self, id: &str, now: u64) -> Option<Outcome> {
        let mut inner = self.lock();
        prune(&mut inner, now);
        if inner.held.iter().any(|held| held.id == id) {
            return None;
        }
        Some(
            inner
                .settled
                .iter()
                .rev()
                .find(|settled| settled.id == id)
                .map_or(Outcome::Expired, |settled| settled.outcome),
        )
    }

    /// The page stopped waiting: the use is over, unanswered.
    pub fn withdraw(&self, id: &str, now: u64) {
        let mut inner = self.lock();
        if let Some(at) = inner.held.iter().position(|held| held.id == id) {
            let held = inner.held.remove(at);
            settle(&mut inner, held, Outcome::Expired, now);
        }
    }

    /// The person's answer to a held use; refused when it is not held
    /// (answered already, expired, or never asked).
    pub fn answer(&self, id: &str, answer: Answer, now: u64) -> Result<Outcome, String> {
        let mut inner = self.lock();
        prune(&mut inner, now);
        let Some(at) = inner.held.iter().position(|held| held.id == id) else {
            return Err(format!(
                "no use {id} is held: it was answered, it expired, or it was never asked"
            ));
        };
        let held = inner.held.remove(at);
        let outcome = match answer {
            Answer::Approve => Outcome::Approved,
            Answer::Allow => Outcome::Allowed,
            Answer::Deny => Outcome::Denied,
        };
        if outcome == Outcome::Allowed {
            inner
                .allowed
                .insert((held.secret.clone(), held.site.clone()), now + ALLOW_FOR_MS);
        }
        if held.retry {
            let ticket = match outcome {
                Outcome::Denied => Ticket::Denied,
                outcome => Ticket::Approved {
                    outcome,
                    until: now + APPROVAL_KEPT_MS,
                },
            };
            inner
                .tickets
                .insert((held.secret.clone(), held.site.clone()), ticket);
        }
        settle(&mut inner, held, outcome, now);
        Ok(outcome)
    }

    /// What is held now, oldest first.
    pub fn held(&self, now: u64) -> Vec<Held> {
        let mut inner = self.lock();
        prune(&mut inner, now);
        inner.held.clone()
    }

    /// The last answers and expiries, newest last.
    pub fn settled(&self) -> Vec<Settled> {
        self.lock().settled.iter().cloned().collect()
    }
}

fn allowed(inner: &Inner, secret: &str, site: &str, now: u64) -> bool {
    inner
        .allowed
        .get(&(secret.to_owned(), site.to_owned()))
        .is_some_and(|until| *until > now)
}

#[allow(clippy::too_many_arguments)]
fn hold(
    inner: &mut Inner,
    secret: &str,
    kind: Kind,
    site: &str,
    path: &str,
    who: &str,
    expires_at: u64,
    retry: bool,
    now: u64,
) -> String {
    let id = use_id();
    inner.held.push(Held {
        id: id.clone(),
        secret: secret.to_owned(),
        kind,
        site: site.to_owned(),
        path: if path.is_empty() { "/" } else { path }.to_owned(),
        asked_at: now,
        expires_at,
        who: who.to_owned(),
        retry,
    });
    id
}

/// Ends what ran out: held uses past their time, allowances and approvals
/// that lapsed.
fn prune(inner: &mut Inner, now: u64) {
    let (over, kept): (Vec<Held>, Vec<Held>) = std::mem::take(&mut inner.held)
        .into_iter()
        .partition(|held| held.expires_at <= now);
    inner.held = kept;
    for held in over {
        settle(inner, held, Outcome::Expired, now);
    }
    inner.allowed.retain(|_, until| *until > now);
    inner.tickets.retain(|_, ticket| match ticket {
        Ticket::Approved { until, .. } | Ticket::Signing { until, .. } => *until > now,
        Ticket::Pending(_) | Ticket::Denied => true,
    });
}

/// Remembers how a use ended, and writes a refusal into the card's record:
/// a use that went through is written where it is used.
fn settle(inner: &mut Inner, held: Held, outcome: Outcome, now: u64) {
    if matches!(outcome, Outcome::Denied | Outcome::Expired) {
        let what = match held.kind {
            Kind::Passkey => "was refused a passkey",
            Kind::Login => "was refused a login",
        };
        crate::bridge::log_use(&held.site, what, false, Some(outcome.how()), &held.who);
    }
    inner.seq += 1;
    let until = (outcome == Outcome::Allowed).then_some(now + ALLOW_FOR_MS);
    let seq = inner.seq;
    inner.settled.push_back(Settled {
        seq,
        id: held.id,
        secret: held.secret,
        kind: held.kind,
        site: held.site,
        outcome,
        at: now,
        until,
    });
    while inner.settled.len() > SETTLED_KEPT {
        inner.settled.pop_front();
    }
}

fn use_id() -> String {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 8];
    let _ = ring::rand::SystemRandom::new().fill(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `GET /secrets/uses`: the uses held now, oldest first.
pub async fn list(State(app): State<App>) -> Json<Value> {
    Json(json!({"uses": app.holds.held(crate::jobs::now())}))
}

/// `POST /secrets/uses/answer`: `{"id", "answer": "approve" | "allow" |
/// "deny"}`. 200 with the use's state, 409 when the id is not held, 400 for
/// anything else.
pub async fn answer(State(app): State<App>, body: Bytes) -> Response {
    let bad = |why: &str| (StatusCode::BAD_REQUEST, Json(json!({"error": why}))).into_response();
    let Ok(body) = serde_json::from_slice::<Value>(&body) else {
        return bad("the body is not JSON");
    };
    let Some(id) = body["id"].as_str().filter(|id| !id.is_empty()) else {
        return bad("id is required");
    };
    let answer = match body["answer"].as_str() {
        Some("approve") => Answer::Approve,
        Some("allow") => Answer::Allow,
        Some("deny") => Answer::Deny,
        _ => return bad("answer is approve, allow or deny"),
    };
    match app.holds.answer(id, answer, crate::jobs::now()) {
        Ok(outcome) => Json(json!({"state": outcome.state()})).into_response(),
        Err(error) => (StatusCode::CONFLICT, Json(json!({"error": error}))).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: u64 = 1_800_000_000_000;

    fn held_id(pass: Pass) -> String {
        match pass {
            Pass::Held { id, .. } => id,
            other => panic!("not held: {other:?}"),
        }
    }

    #[test]
    fn a_held_use_is_listed_until_answered_and_answered_once() {
        let holds = Holds::default();
        let first = held_id(holds.ask(
            "GH_PASSKEY",
            Kind::Passkey,
            "github.com",
            "/login",
            "Rex",
            60_000,
            T,
        ));
        let second = held_id(holds.ask(
            "GH_LOGIN",
            Kind::Login,
            "github.com",
            "",
            "Rex",
            60_000,
            T + 1,
        ));
        let listed = holds.held(T + 2);
        assert_eq!(
            listed.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
            [first.as_str(), second.as_str()],
            "oldest first"
        );
        assert_eq!(listed[0].path, "/login");
        assert_eq!(listed[1].path, "/", "a use with no path is the site's root");
        assert_eq!(listed[0].expires_at, T + 60_000);
        let wire = serde_json::to_value(&listed[0]).unwrap();
        assert_eq!(
            wire,
            json!({"id": first, "secret": "GH_PASSKEY", "kind": "passkey", "site": "github.com",
                   "path": "/login", "askedAt": T, "expiresAt": T + 60_000})
        );
        assert_eq!(holds.status(&first, T + 3), None, "waiting");
        assert_eq!(
            holds.answer(&first, Answer::Approve, T + 4),
            Ok(Outcome::Approved)
        );
        assert_eq!(holds.status(&first, T + 5), Some(Outcome::Approved));
        assert!(
            holds.answer(&first, Answer::Deny, T + 6).is_err(),
            "answered once"
        );
        assert!(holds.answer("unknown", Answer::Approve, T + 6).is_err());
        assert_eq!(
            holds.answer(&second, Answer::Deny, T + 7),
            Ok(Outcome::Denied)
        );
        assert_eq!(holds.status(&second, T + 8), Some(Outcome::Denied));
        assert!(holds.held(T + 9).is_empty());
        let settled = holds.settled();
        assert_eq!(settled.len(), 2);
        assert!(settled[0].seq < settled[1].seq);
    }

    #[test]
    fn a_held_use_expires_at_its_time_and_a_page_that_gives_up_ends_it() {
        let holds = Holds::default();
        let id = held_id(holds.ask("K", Kind::Passkey, "a.test", "/", "Rex", 10_000, T));
        assert_eq!(holds.status(&id, T + 9_999), None);
        assert_eq!(holds.status(&id, T + 10_000), Some(Outcome::Expired));
        assert!(holds.held(T + 10_000).is_empty());
        assert!(
            holds.answer(&id, Answer::Approve, T + 10_001).is_err(),
            "an expired use cannot be answered"
        );
        // A page's own limit beyond five minutes is capped.
        let long = held_id(holds.ask("K", Kind::Passkey, "a.test", "/", "Rex", u64::MAX, T));
        assert_eq!(holds.held(T)[0].expires_at, T + WAIT_CAP_MS);
        holds.withdraw(&long, T + 1);
        assert_eq!(holds.status(&long, T + 2), Some(Outcome::Expired));
        assert!(holds.answer(&long, Answer::Approve, T + 3).is_err());
    }

    #[test]
    fn allow_lets_that_key_through_on_that_site_for_ten_minutes() {
        let holds = Holds::default();
        let id = held_id(holds.ask(
            "GH_PASSKEY",
            Kind::Passkey,
            "github.com",
            "/",
            "Rex",
            60_000,
            T,
        ));
        assert_eq!(
            holds.answer(&id, Answer::Allow, T + 1),
            Ok(Outcome::Allowed)
        );
        assert_eq!(holds.settled()[0].until, Some(T + 1 + ALLOW_FOR_MS));
        let again = |secret: &str, site: &str, at: u64| {
            holds.ask(secret, Kind::Passkey, site, "/", "Rex", 60_000, at)
        };
        assert_eq!(again("GH_PASSKEY", "github.com", T + 60_000), Pass::Within);
        assert_eq!(
            holds.fill("GH_PASSKEY", "github.com", "/", "Rex", T + 60_000),
            Pass::Within
        );
        assert!(holds.held(T + 60_000).is_empty(), "nothing held within it");
        assert!(matches!(
            again("GH_PASSKEY", "gist.github.com", T + 60_000),
            Pass::Held { .. }
        ));
        assert!(matches!(
            again("GH_LOGIN", "github.com", T + 60_000),
            Pass::Held { .. }
        ));
        assert!(matches!(
            again("GH_PASSKEY", "github.com", T + 1 + ALLOW_FOR_MS),
            Pass::Held { .. }
        ));
    }

    #[test]
    fn a_fill_answers_waiting_at_once_and_its_retry_collects_the_answer() {
        let holds = Holds::default();
        let id = held_id(holds.fill("PROOF", "localhost", "/login", "Rex", T));
        assert_eq!(
            holds.fill("PROOF", "localhost", "/login", "Rex", T + 1_000),
            Pass::Held {
                id: id.clone(),
                lapsed: false
            },
            "a retry before the answer is the same ask"
        );
        assert_eq!(holds.held(T + 1_000).len(), 1);
        assert_eq!(
            holds.answer(&id, Answer::Approve, T + 2_000),
            Ok(Outcome::Approved)
        );
        // The retry fills, and so do the sign-in's other fields for a minute.
        assert_eq!(
            holds.fill("PROOF", "localhost", "/login", "Rex", T + 3_000),
            Pass::Answered(Outcome::Approved)
        );
        assert_eq!(
            holds.fill(
                "PROOF",
                "localhost",
                "/login",
                "Rex",
                T + 3_000 + SIGN_IN_MS - 1
            ),
            Pass::Answered(Outcome::Approved)
        );
        let next = held_id(holds.fill(
            "PROOF",
            "localhost",
            "/login",
            "Rex",
            T + 3_000 + SIGN_IN_MS,
        ));
        assert_ne!(next, id, "a later sign-in is asked again");
        assert_eq!(
            holds.answer(&next, Answer::Deny, T + 100_000),
            Ok(Outcome::Denied)
        );
        assert_eq!(
            holds.fill("PROOF", "localhost", "/login", "Rex", T + 101_000),
            Pass::Denied
        );
        let asked_again = held_id(holds.fill("PROOF", "localhost", "/login", "Rex", T + 102_000));
        // Unanswered, it lapses; the next try says so and asks again.
        let lapsed = holds.fill(
            "PROOF",
            "localhost",
            "/login",
            "Rex",
            T + 102_000 + FILL_WAITS_MS,
        );
        match lapsed {
            Pass::Held { id, lapsed: true } => assert_ne!(id, asked_again),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_approval_nobody_collects_lapses() {
        let holds = Holds::default();
        let id = held_id(holds.fill("PROOF", "localhost", "/", "Rex", T));
        holds.answer(&id, Answer::Approve, T + 1).unwrap();
        assert!(matches!(
            holds.fill("PROOF", "localhost", "/", "Rex", T + 1 + APPROVAL_KEPT_MS),
            Pass::Held { .. }
        ));
    }

    #[tokio::test]
    async fn the_door_lists_held_uses_and_takes_one_answer_each() {
        let home = tempfile::tempdir().unwrap();
        let app = App::new(crate::Config {
            addr: "127.0.0.1:0".to_owned(),
            token: Some("the-token".to_owned()),
            home: home.path().to_owned(),
            display: ":0".to_owned(),
            screen: "1920x1080".to_owned(),
        });
        let holds = app.holds.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let router = crate::serve::router(app);
        let serving = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let uses = format!("{base}/secrets/uses");
        let answer = format!("{base}/secrets/uses/answer");
        assert_eq!(http.get(&uses).send().await.unwrap().status(), 401);
        let empty: Value = http
            .get(&uses)
            .bearer_auth("the-token")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(empty, json!({"uses": []}));
        let now = crate::jobs::now();
        let id = held_id(holds.ask(
            "GH_PASSKEY",
            Kind::Passkey,
            "github.com",
            "/login",
            "Rex",
            60_000,
            now,
        ));
        let listed: Value = http
            .get(&uses)
            .bearer_auth("the-token")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(listed["uses"][0]["id"], json!(id));
        assert_eq!(listed["uses"][0]["kind"], "passkey");
        assert_eq!(listed["uses"][0]["site"], "github.com");
        for bad in [
            "not json".to_owned(),
            json!({"answer": "approve"}).to_string(),
            json!({"id": id, "answer": "maybe"}).to_string(),
            json!({"id": "", "answer": "approve"}).to_string(),
        ] {
            let refused = http
                .post(&answer)
                .bearer_auth("the-token")
                .body(bad.clone())
                .send()
                .await
                .unwrap();
            assert_eq!(refused.status(), 400, "{bad}");
        }
        let naked = http
            .post(&answer)
            .json(&json!({"id": id, "answer": "approve"}))
            .send()
            .await
            .unwrap();
        assert_eq!(naked.status(), 401);
        let allowed = http
            .post(&answer)
            .bearer_auth("the-token")
            .json(&json!({"id": id, "answer": "allow"}))
            .send()
            .await
            .unwrap();
        assert_eq!(allowed.status(), 200);
        assert_eq!(
            allowed.json::<Value>().await.unwrap(),
            json!({"state": "approved"})
        );
        let stale = http
            .post(&answer)
            .bearer_auth("the-token")
            .json(&json!({"id": id, "answer": "deny"}))
            .send()
            .await
            .unwrap();
        assert_eq!(stale.status(), 409);
        let id = held_id(holds.ask(
            "GH_LOGIN",
            Kind::Login,
            "github.com",
            "/login",
            "Rex",
            60_000,
            crate::jobs::now(),
        ));
        let denied: Value = http
            .post(&answer)
            .bearer_auth("the-token")
            .json(&json!({"id": id, "answer": "deny"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(denied, json!({"state": "denied"}));
        serving.abort();
    }
}
