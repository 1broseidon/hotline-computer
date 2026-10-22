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
use crate::holds::{self, Kind, Outcome, Pass};
use crate::passkeys::{Ask, Reported, rp_within};
use crate::secrets::{Passkey, Use as KeyUse};

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
        // Held uses and new keys ride along, names and sites only, for the
        // card on the page to say what waits for the person and what arrived.
        let now = json!({
            "passkeys": arming(app),
            "wheel": wheel(app).await,
            "uses": recent_uses(),
            "held": app.holds.held(crate::jobs::now()),
            "settled": app.holds.settled(),
            "arrivals": app.secrets.arrivals(),
        });
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
        Some("get") => {
            let site = site_of_request(details);
            let granted = keys(app);
            let how = match authenticator::chosen(details, &granted) {
                Ok(key) => match passkey_gate(app, details, key, &site).await {
                    Gate::Through(how) => how,
                    // Refused by the person, or by nobody answering: the
                    // record already says which.
                    Gate::Refused(refusal) => return refused(refusal),
                },
                Err(_) => None,
            };
            // Signed with the set as it is now: a key revoked while the
            // person was deciding no longer signs.
            match authenticator::assert(details, &keys(app)) {
                Ok(response) => {
                    record(app, &site, "signed in with the passkey", true, how).await;
                    json!({"response": response})
                }
                Err(refusal) => {
                    // A page that only offers passkeys in its sign-in field asks
                    // quietly on load; that is not a refusal worth a note.
                    if details["mediation"].as_str() != Some("conditional") {
                        record(app, &site, "was refused a passkey", false, None).await;
                    }
                    refused(refusal)
                }
            }
        }
        Some("create") => {
            let site = site_of_request(details);
            match create(app, details).await {
                Ok(response) => {
                    record(app, &site, "made a passkey", true, None).await;
                    json!({"response": response})
                }
                Err(refusal) => refused(refusal),
            }
        }
        // The passkeys this computer may sign with on a page's site: the
        // account each is for, never a key.
        Some("passkeys") => {
            let host = host_of(message["origin"].as_str().unwrap_or_default());
            let passkeys: Vec<Value> = keys(app)
                .into_iter()
                .filter(|key| !host.is_empty() && serves(&host, &key.rp_id))
                .map(|key| json!({"rpId": key.rp_id, "userName": key.user_name}))
                .collect();
            json!({"passkeys": passkeys})
        }
        // The person at the wheel hands the computer back to the agent. Only
        // a person's hold is released: an agent's is never touched from here.
        // The person says they are done: their hold ends, and the popup
        // stops saying it is their turn.
        Some("hand_back") => {
            let handed = PERSON_SINCE.lock().unwrap().take().is_some();
            match app.access.holder().await {
                Some((holder, _)) if holder.starts_with(crate::viewer::PERSON) => {
                    match app.access.release(&holder).await {
                        Ok(_) => json!({"handedBack": true}),
                        Err(error) => refused(authenticator_refusal("NotAllowedError", &error)),
                    }
                }
                _ => json!({"handedBack": handed}),
            }
        }
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
        Some("fill") => {
            let site = host_of(message["origin"].as_str().unwrap_or_default());
            let how = match fill_gate(app, message, &site).await {
                Gate::Through(how) => how,
                Gate::Refused(refusal) => return refused(refusal),
            };
            let reply = fill(app, message);
            if reply.get("error").is_some() {
                record(app, &site, "was refused a login", false, None).await;
            } else {
                record(app, &site, "filled a login", true, how).await;
            }
            reply
        }
        _ => refused(authenticator_refusal(
            "NotSupportedError",
            "this computer does not answer that",
        )),
    }
}

/// Whether a use goes ahead, and the record line's tail when it went
/// through on the person's answer or an allowance.
enum Gate {
    Through(Option<&'static str>),
    Refused(Refusal),
}

/// A sign-in with `key`: through at once when the key is automatic, when
/// the person has the computer, or inside an allowance; otherwise held
/// until the person answers or the page's own time runs out.
async fn passkey_gate(app: &App, details: &Value, key: &Passkey, site: &str) -> Gate {
    let named = app
        .secrets
        .passkeys()
        .into_iter()
        .find(|(_, granted)| granted.credential_id == key.credential_id);
    let Some((name, granted)) = named else {
        // One the arming just minted, not yet delivered back: automatic.
        return Gate::Through(None);
    };
    if granted.use_ != KeyUse::Ask || person_has_it(app).await {
        return Gate::Through(None);
    }
    // A quiet request on load is never answered behind a site's back, and
    // never asks the person: only the site's own button does.
    if details["mediation"].as_str() == Some("conditional") {
        return Gate::Refused(authenticator_refusal(
            "NotAllowedError",
            "Hotline: this passkey asks the person before each use; use the site's passkey button",
        ));
    }
    let wait_ms = details["timeout"]
        .as_f64()
        .filter(|ms| *ms > 0.0)
        .map_or(holds::WAIT_CAP_MS, |ms| ms as u64);
    held_until_answered(app, &name, Kind::Passkey, site, wait_ms).await
}

/// A fill the card on the page asked for: an ask-me login waits for the
/// person here, since nobody's tool call is waiting on it.
async fn fill_gate(app: &App, message: &Value, site: &str) -> Gate {
    let origin = message["origin"].as_str().unwrap_or_default();
    let name = message["name"].as_str().unwrap_or_default();
    let for_here = app
        .secrets
        .logins_for(origin)
        .iter()
        .any(|(login, _, _)| login == name);
    if !for_here || app.secrets.use_of(name) != Some(KeyUse::Ask) || person_has_it(app).await {
        return Gate::Through(None);
    }
    held_until_answered(app, name, Kind::Login, site, holds::FILL_WAITS_MS).await
}

async fn held_until_answered(app: &App, name: &str, kind: Kind, site: &str, wait_ms: u64) -> Gate {
    let path = page_path(app, site).await;
    let who = who(app).await;
    let id = match app
        .holds
        .ask(name, kind, site, &path, &who, wait_ms, crate::jobs::now())
    {
        Pass::Within => return Gate::Through(Some("within the 10 min")),
        Pass::Held { id, .. } => id,
        Pass::Answered(outcome) => return Gate::Through(Some(outcome.how())),
        Pass::Denied => return Gate::Refused(declined(name, site)),
    };
    // Dropped without an answer (the page gave up and closed its port), the
    // use stops being held, and the desk's card settles as expired.
    struct Withdraw<'a>(&'a holds::Holds, String);
    impl Drop for Withdraw<'_> {
        fn drop(&mut self) {
            self.0.withdraw(&self.1, crate::jobs::now());
        }
    }
    let _withdraw = Withdraw(&app.holds, id.clone());
    loop {
        match app.holds.status(&id, crate::jobs::now()) {
            None => tokio::time::sleep(std::time::Duration::from_millis(200)).await,
            Some(outcome @ (Outcome::Approved | Outcome::Allowed)) => {
                return Gate::Through(Some(outcome.how()));
            }
            Some(Outcome::Denied) => return Gate::Refused(declined(name, site)),
            Some(Outcome::Expired) => {
                return Gate::Refused(authenticator_refusal(
                    "NotAllowedError",
                    &format!("Hotline: nobody approved this use of {name} on {site} in time."),
                ));
            }
        }
    }
}

fn declined(name: &str, site: &str) -> Refusal {
    authenticator_refusal(
        "NotAllowedError",
        &format!("Hotline: the person denied this use of {name} on {site}."),
    )
}

/// Whether the person has the computer (through the viewer) right now: their
/// own uses are never held.
pub(crate) async fn person_has_it(app: &App) -> bool {
    matches!(app.access.holder().await, Some((holder, _)) if holder.starts_with(crate::viewer::PERSON))
}

/// The path of the page the agent drives, when it is on `site`: what the
/// desk's card shows under the site. `/` when it cannot tell.
async fn page_path(app: &App, site: &str) -> String {
    // The tool call that made the page ask may still hold the browser for a
    // moment; a few short looks, never a wait on it.
    for _ in 0..6 {
        let url = tokio::time::timeout(
            std::time::Duration::from_millis(300),
            app.browser.current_url_if_idle(),
        )
        .await
        .ok()
        .flatten();
        if let Some(url) = url {
            return path_on(&url, site);
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    "/".to_owned()
}

pub(crate) fn path_on(url: &str, site: &str) -> String {
    let host = host_of(url);
    if host.is_empty() || !serves(&host, site) {
        return "/".to_owned();
    }
    let rest = url.split_once("://").map_or("", |(_, rest)| rest);
    let path = rest
        .find('/')
        .map_or("/", |at| &rest[at..])
        .split(['?', '#'])
        .next()
        .unwrap_or("/");
    if path.is_empty() { "/" } else { path }.to_owned()
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

// ---- the wheel and the record --------------------------------------------
// What the extension shows besides a request: who has the computer, and a
// short record of what its keys did. Neither carries a key or a value.

/// Who holds the computer now: the person (through the viewer), the agent,
/// or nobody; and the agent's name and task as the desktop bar shows them.
/// When the person last took the computer, while it is still theirs. The
/// viewer's hold lapses ten seconds after their last input, but the turn is
/// theirs until they hand it back or the agent acts again.
static PERSON_SINCE: std::sync::Mutex<Option<u64>> = std::sync::Mutex::new(None);

async fn wheel(app: &App) -> Value {
    let agent = app.jobs.agent.snapshot();
    let holder = app.access.holder().await;
    let driving = turn(
        &PERSON_SINCE,
        holder.as_ref().map(|(h, _)| h.as_str()),
        agent.last_action_ms,
        crate::jobs::now(),
    );
    json!({"driving": driving, "agent": agent.name, "task": agent.task})
}

/// Who has the computer: a person holding it now, or one who took it and
/// has not handed it back while the agent stayed still; else whoever holds.
fn turn(
    since: &std::sync::Mutex<Option<u64>>,
    holder: Option<&str>,
    agent_acted_ms: u64,
    now: u64,
) -> &'static str {
    let mut since = since.lock().unwrap();
    match holder {
        Some(holder) if holder.starts_with(crate::viewer::PERSON) => {
            since.get_or_insert(now);
            "person"
        }
        Some(_) => {
            *since = None;
            "agent"
        }
        None if since.is_some_and(|at| agent_acted_ms <= at) => "person",
        None => {
            *since = None;
            "none"
        }
    }
}

#[derive(Clone, serde::Serialize)]
struct Use {
    id: u64,
    ts: u64,
    site: String,
    what: &'static str,
    ok: bool,
    /// The agent's name when it held the computer, else "you".
    who: String,
    /// For an ask-me key: approved at the desk, allowed 10 min, within the
    /// 10 min, you denied it, nobody answered in time.
    #[serde(skip_serializing_if = "Option::is_none")]
    how: Option<&'static str>,
}

/// The last uses, newest last: enough for a site's record in the popup and
/// for a note when one happens. Kept in memory; a restart starts it empty.
static USES: std::sync::Mutex<std::collections::VecDeque<Use>> =
    std::sync::Mutex::new(std::collections::VecDeque::new());
static NEXT_USE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
const USES_KEPT: usize = 40;

/// Who a use is for: "you" while the person has the computer, else the agent.
pub(crate) async fn who(app: &App) -> String {
    if person_has_it(app).await {
        "you".to_owned()
    } else {
        app.jobs.agent.snapshot().name
    }
}

pub(crate) async fn record(
    app: &App,
    site: &str,
    what: &'static str,
    ok: bool,
    how: Option<&'static str>,
) {
    let who = who(app).await;
    log_use(site, what, ok, how, &who);
}

/// How long a use repeating the last one (the password, then the code of
/// one sign-in) is the same line in the record.
const SAME_USE_MS: u64 = 10_000;

/// Writes one line of the record, which the card shows and notes come from.
pub(crate) fn log_use(
    site: &str,
    what: &'static str,
    ok: bool,
    how: Option<&'static str>,
    who: &str,
) {
    if site.is_empty() {
        return;
    }
    let ts = crate::jobs::now();
    let mut uses = USES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if uses.back().is_some_and(|last| {
        last.site == site
            && last.what == what
            && last.ok == ok
            && last.how == how
            && last.who == who
            && ts.saturating_sub(last.ts) < SAME_USE_MS
    }) {
        return;
    }
    let entry = Use {
        id: NEXT_USE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        ts,
        site: site.to_owned(),
        what,
        ok,
        who: who.to_owned(),
        how,
    };
    uses.push_back(entry);
    while uses.len() > USES_KEPT {
        uses.pop_front();
    }
}

fn recent_uses() -> Value {
    json!(
        USES.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .cloned()
            .collect::<Vec<_>>()
    )
}

/// The host of a web origin, or empty for anything that is not one.
pub(crate) fn host_of(origin: &str) -> String {
    let rest = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))
        .unwrap_or_default();
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    host.rsplit_once(':')
        .filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit()))
        .map_or(host, |(name, _)| name)
        .to_ascii_lowercase()
}

/// The site a WebAuthn request is for: its relying party, else its page.
fn site_of_request(details: &Value) -> String {
    details["rpId"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| details["rp"]["id"].as_str().map(str::to_owned))
        .unwrap_or_else(|| {
            host_of(
                details["extensions"]["remoteDesktopClientOverride"]["origin"]
                    .as_str()
                    .unwrap_or_default(),
            )
        })
}

/// Whether a page on `host` may use a passkey made for `rp_id`.
fn serves(host: &str, rp_id: &str) -> bool {
    host == rp_id || host.ends_with(&format!(".{rp_id}"))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_turn_stays_with_the_person_until_handed_back_or_the_agent_acts() {
        let since = std::sync::Mutex::new(None);
        assert_eq!(turn(&since, None, 0, 1_000), "none");
        assert_eq!(turn(&since, Some("person-1"), 500, 2_000), "person");
        // The viewer's hold lapsed; the agent has not moved since.
        assert_eq!(turn(&since, None, 500, 20_000), "person");
        // Renewing does not move when the turn began.
        assert_eq!(turn(&since, Some("person-1"), 500, 21_000), "person");
        assert_eq!(turn(&since, None, 1_500, 30_000), "person");
        // The agent acting after the person took it ends the turn.
        assert_eq!(turn(&since, None, 2_500, 31_000), "none");
        assert_eq!(turn(&since, None, 2_500, 32_000), "none");
        assert_eq!(turn(&since, Some("person-2"), 2_500, 40_000), "person");
        assert_eq!(turn(&since, Some("agent"), 2_500, 41_000), "agent");
        assert_eq!(turn(&since, None, 2_500, 42_000), "none");
    }

    #[test]
    fn host_of_reads_web_origins_only() {
        assert_eq!(host_of("https://GitHub.com"), "github.com");
        assert_eq!(
            host_of("https://login.example.com:8443/path?q"),
            "login.example.com"
        );
        assert_eq!(host_of("http://localhost:3000"), "localhost");
        assert_eq!(host_of("chrome-extension://abc"), "");
        assert_eq!(host_of(""), "");
    }

    #[test]
    fn a_request_names_its_relying_party_before_its_page() {
        let get = json!({"rpId": "github.com", "extensions": {"remoteDesktopClientOverride": {"origin": "https://evil.example"}}});
        assert_eq!(site_of_request(&get), "github.com");
        let create = json!({"rp": {"id": "example.com"}});
        assert_eq!(site_of_request(&create), "example.com");
        let bare =
            json!({"extensions": {"remoteDesktopClientOverride": {"origin": "https://gitbub.co"}}});
        assert_eq!(site_of_request(&bare), "gitbub.co");
    }

    #[test]
    fn a_held_use_names_the_page_path_only_on_its_own_site() {
        assert_eq!(
            path_on("https://github.com/login?x=1#y", "github.com"),
            "/login"
        );
        assert_eq!(path_on("https://github.com", "github.com"), "/");
        assert_eq!(
            path_on("http://localhost:8123/passkey.html", "localhost"),
            "/passkey.html"
        );
        assert_eq!(path_on("https://evil.example/login", "github.com"), "/");
        assert_eq!(path_on("about:blank", "github.com"), "/");
    }

    #[test]
    fn a_repeated_use_is_one_line_in_the_record() {
        let site = "record-test.example";
        let count = || {
            USES.lock()
                .unwrap()
                .iter()
                .filter(|u| u.site == site)
                .count()
        };
        log_use(
            site,
            "filled a login",
            true,
            Some("approved at the desk"),
            "Rex",
        );
        log_use(
            site,
            "filled a login",
            true,
            Some("approved at the desk"),
            "Rex",
        );
        assert_eq!(count(), 1, "the password, then the code, of one sign-in");
        log_use(
            site,
            "was refused a login",
            false,
            Some("you denied it"),
            "Rex",
        );
        assert_eq!(count(), 2);
        let line = USES
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|u| u.site == site)
            .cloned()
            .unwrap();
        assert_eq!(serde_json::to_value(&line).unwrap()["how"], "you denied it");
    }

    #[test]
    fn a_passkey_serves_its_site_and_subdomains_only() {
        assert!(serves("github.com", "github.com"));
        assert!(serves("auth.github.com", "github.com"));
        assert!(!serves("notgithub.com", "github.com"));
        assert!(!serves("github.com.evil.example", "github.com"));
        assert!(!serves("github.com", "auth.github.com"));
    }
}
