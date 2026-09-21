//! The secrets the person put in this computer.
//!
//! The desk delivers the whole set with `PUT /secrets`, bearer in a header,
//! each time it changes: when the computer starts, and again when the person
//! stores, replaces or removes one, or ticks one for this teammate. The set
//! lives in this process's memory and nowhere else; the computer itself
//! writes no value to disk.
//!
//! A secret has a kind, and the kind says where it goes. A **variable** joins
//! the environment of each job the agent starts through `shell` or `files
//! run`, under the name the person gave it, so a command uses `$NAME` and a
//! tool that reads that variable finds it; a preparation job is left out,
//! because what it captures is written into the workspace. A **login** — a
//! site, a username, a password, perhaps a TOTP seed — never enters a job:
//! `browser fill` types one of its fields by name into a form, and only on a
//! page whose origin is one of the login's sites, because a page a password
//! is typed into can read it, so choosing the page is the only defence. A
//! **passkey** is the teammate's own WebAuthn credential; the browser keeps
//! it in a virtual authenticator (see `crate::passkeys`) and it signs in by
//! itself, so nothing ever types it.
//!
//! Nothing answers a value back to the agent. There is no `GET`; `state
//! info` lists names, kinds and sites alone; a fill answers without the
//! value; and every text a tool returns has each secret value replaced with
//! `[redacted NAME]` on its way out — a variable's value, a login's password
//! and seed, a passkey's private key — in the spelling JSON gives it too.
//! Usernames and sites are identity, not secrets, and stay readable: a page
//! that says who is signed in must still make sense. That keeps a value out
//! of the model's context when a job prints it, and off the tape the desk
//! keeps. It is not a wall against a command written to get one out: a value
//! printed in pieces, encoded, or written to a file the desk reads on its
//! own is not caught, and the person's own view of the screen is never
//! redacted.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, RwLock, RwLockReadGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use hmac::{Hmac, Mac};
use rmcp::model::ContentBlock;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha1::Sha1;

use crate::App;
use crate::passkeys::check_rp_id;
use crate::tools::ToolResult;

/// The desk refuses a shorter value before storing it; one this short would
/// redact ordinary text out of every answer.
const MIN_VALUE_CHARS: usize = 8;
const MAX_VALUE_BYTES: usize = 64 * 1024;
const MAX_NAME_CHARS: usize = 64;
const MAX_SITES: usize = 16;
const MAX_USERNAME_BYTES: usize = 1024;
/// A TOTP seed shorter than this is refused: the RFC asks for 128 bits and
/// issuers hand out at least 80, which is sixteen base32 characters.
const MIN_SEED_CHARS: usize = 16;
const TOTP_STEP_SECONDS: u64 = 30;
/// Names a job's shell already owns; a secret cannot take one over.
const RESERVED: [&str; 12] = [
    "PATH",
    "HOME",
    "USER",
    "SHELL",
    "TERM",
    "LANG",
    "TZ",
    "DISPLAY",
    "PWD",
    "TMPDIR",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
];

/// A stored secret as the desk delivers it. The kind names where it goes.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Secret {
    /// The environment of every shell job.
    Variable(Variable),
    /// A form on one of its sites, one field at a time, by name.
    Login(Login),
    /// The browser's authenticator, where it signs in by itself.
    Passkey(Passkey),
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Variable {
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Login {
    /// Origins, `https://host[:port]`: the only pages a field is typed into.
    pub sites: Vec<String>,
    pub username: String,
    pub password: String,
    /// A base32 TOTP seed; `NAME.code` is the six digits it gives right now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub totp: Option<String>,
}

/// A WebAuthn credential as Chromium's virtual authenticator reports one,
/// bytes in base64. The desk keeps it in the vault and delivers it back.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Passkey {
    pub rp_id: String,
    pub credential_id: String,
    /// PKCS#8.
    pub private_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_handle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_display_name: Option<String>,
}

/// One entry of a delivery. A bare string is a variable, which is all a
/// desk before 0.16 sends; an object says its kind.
#[derive(Clone, Debug)]
pub enum Delivered {
    Value(String),
    Secret(Secret),
}

impl<'de> Deserialize<'de> for Delivered {
    /// By hand, so that an object with a kind this computer does not know
    /// is refused by that kind's name, not as "no variant matched".
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match Value::deserialize(deserializer)? {
            Value::String(value) => Ok(Delivered::Value(value)),
            other => serde_json::from_value::<Secret>(other)
                .map(Delivered::Secret)
                .map_err(serde::de::Error::custom),
        }
    }
}

impl From<Delivered> for Secret {
    fn from(delivered: Delivered) -> Self {
        match delivered {
            Delivered::Value(value) => Secret::Variable(Variable { value }),
            Delivered::Secret(secret) => secret,
        }
    }
}

impl Secret {
    /// The entry as it is kept, or the sentence refusing it. Sites and seeds
    /// come out in one spelling, so the origin rule and the redaction see the
    /// same text whichever way the person typed it.
    fn checked(self, name: &str) -> Result<Secret, String> {
        match self {
            Secret::Variable(Variable { value }) => {
                check_value(name, &value)?;
                Ok(Secret::Variable(Variable { value }))
            }
            Secret::Login(login) => {
                if login.sites.is_empty() {
                    return Err(format!(
                        "{name} names no site; a login is typed on its own sites alone"
                    ));
                }
                if login.sites.len() > MAX_SITES {
                    return Err(format!("{name} names more than {MAX_SITES} sites"));
                }
                let sites = login
                    .sites
                    .iter()
                    .map(|site| {
                        Origin::site(site)
                            .map(|origin| origin.to_string())
                            .map_err(|error| format!("{name}: {error}"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let username = login.username.trim().to_owned();
                if username.is_empty()
                    || username.len() > MAX_USERNAME_BYTES
                    || username.contains(['\n', '\r', '\0'])
                {
                    return Err(format!("{name} has no username, or one that is not a line"));
                }
                check_value(&format!("{name}.password"), &login.password)?;
                let totp = login
                    .totp
                    .as_deref()
                    .map(|seed| totp_seed(seed).map_err(|error| format!("{name}.totp: {error}")))
                    .transpose()?;
                Ok(Secret::Login(Login {
                    sites,
                    username,
                    password: login.password,
                    totp,
                }))
            }
            Secret::Passkey(passkey) => {
                check_rp_id(&passkey.rp_id).map_err(|error| format!("{name}: {error}"))?;
                let bytes = |field: &str, text: &str| {
                    base64::engine::general_purpose::STANDARD
                        .decode(text)
                        .map_err(|_| format!("{name}.{field} is not base64"))
                };
                if bytes("credentialId", &passkey.credential_id)?.is_empty() {
                    return Err(format!("{name}.credentialId is empty"));
                }
                if bytes("privateKey", &passkey.private_key)?.len() < 32 {
                    return Err(format!("{name}.privateKey is too short to be a key"));
                }
                if let Some(handle) = &passkey.user_handle {
                    bytes("userHandle", handle)?;
                }
                Ok(Secret::Passkey(passkey))
            }
        }
    }

    /// What `state info` says about it: never a value.
    fn describe(&self, name: &str) -> Value {
        match self {
            Secret::Variable(_) => json!({"name": name, "kind": "variable"}),
            Secret::Login(login) => json!({
                "name": name, "kind": "login", "sites": login.sites,
                "username": login.username, "totp": login.totp.is_some(),
            }),
            Secret::Passkey(passkey) => json!({
                "name": name, "kind": "passkey", "rpId": passkey.rp_id,
                "userName": passkey.user_name,
            }),
        }
    }
}

/// A value the browser is about to type, with the sites it may be typed on
/// when it is a login's, and the name it is answered by.
#[derive(Clone, PartialEq, Eq)]
pub struct Filled {
    /// `GITHUB.password`: what the answer names instead of the value.
    pub label: String,
    pub value: String,
    /// `None` for a variable, which has no site to check.
    pub sites: Option<Vec<Origin>>,
}

/// The set the desk last delivered, shared between the door that takes it,
/// the jobs that start with it, the browser that types and signs with it,
/// and the answers it is taken out of.
#[derive(Clone, Default)]
pub struct Secrets(Arc<RwLock<BTreeMap<String, Secret>>>);

impl Secrets {
    /// Takes the whole set, or none of it: one entry the computer would not
    /// keep refuses the delivery and keeps the last set.
    pub fn replace(&self, set: BTreeMap<String, Delivered>) -> Result<(), String> {
        let mut checked = BTreeMap::new();
        for (name, delivered) in set {
            check_name(&name)?;
            let secret = Secret::from(delivered).checked(&name)?;
            checked.insert(name, secret);
        }
        *self
            .0
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = checked;
        Ok(())
    }

    /// The names alone.
    pub fn names(&self) -> Vec<String> {
        self.read().keys().cloned().collect()
    }

    /// What `state info` lists: name, kind, and for a login its sites and
    /// username, for a passkey its site. Never a value.
    pub fn catalog(&self) -> Vec<Value> {
        self.read()
            .iter()
            .map(|(name, secret)| secret.describe(name))
            .collect()
    }

    /// What a job offered the set starts with: the variables.
    pub fn environment(&self) -> BTreeMap<String, String> {
        self.read()
            .iter()
            .filter_map(|(name, secret)| match secret {
                Secret::Variable(Variable { value }) => Some((name.clone(), value.clone())),
                Secret::Login(_) | Secret::Passkey(_) => None,
            })
            .collect()
    }

    /// The passkeys the browser's authenticators carry, by name.
    pub fn passkeys(&self) -> Vec<(String, Passkey)> {
        self.read()
            .iter()
            .filter_map(|(name, secret)| match secret {
                Secret::Passkey(passkey) => Some((name.clone(), passkey.clone())),
                Secret::Variable(_) | Secret::Login(_) => None,
            })
            .collect()
    }

    /// What `browser fill secret:` types: `NAME` for a variable's value;
    /// `NAME.username`, `NAME.password` or `NAME.code` for a login's, the
    /// last being the six TOTP digits of this moment.
    pub fn resolve(&self, reference: &str) -> Result<Filled, String> {
        self.resolve_at(reference, unix_now())
    }

    fn resolve_at(&self, reference: &str, now: u64) -> Result<Filled, String> {
        let (name, field) = match reference.split_once('.') {
            Some((name, field)) => (name, Some(field)),
            None => (reference, None),
        };
        let set = self.read();
        let Some(secret) = set.get(name) else {
            return Err(format!(
                "no secret named {name} is in this computer; state info lists them"
            ));
        };
        match (secret, field) {
            (Secret::Variable(Variable { value }), None) => Ok(Filled {
                label: name.to_owned(),
                value: value.clone(),
                sites: None,
            }),
            (Secret::Variable(_), Some(field)) => Err(format!(
                "{name} is a variable and has no {field}; fill secret:{name:?}"
            )),
            (Secret::Login(login), Some(field)) => {
                let value = match field {
                    "username" => login.username.clone(),
                    "password" => login.password.clone(),
                    "code" => match &login.totp {
                        Some(seed) => totp(seed, now)?,
                        None => {
                            return Err(format!("{name} has no TOTP seed, so no code"));
                        }
                    },
                    other => {
                        return Err(format!(
                            "{name} is a login with username, password{}, not {other}",
                            if login.totp.is_some() {
                                " and code"
                            } else {
                                ""
                            }
                        ));
                    }
                };
                let sites = login
                    .sites
                    .iter()
                    .map(|site| Origin::site(site))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Filled {
                    label: format!("{name}.{field}"),
                    value,
                    sites: Some(sites),
                })
            }
            (Secret::Login(_), None) => Err(format!(
                "{name} is a login; fill secret:\"{name}.username\", \"{name}.password\" or \"{name}.code\""
            )),
            (Secret::Passkey(_), _) => Err(format!(
                "{name} is a passkey; it signs in by itself when the site asks the browser, and nothing types it"
            )),
        }
    }

    fn read(&self) -> RwLockReadGuard<'_, BTreeMap<String, Secret>> {
        self.0
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// `text` with every secret value replaced by `[redacted NAME]`: a
    /// variable's value, a login's password and seed, a passkey's private
    /// key. Longer values go first, so one that contains another is taken
    /// whole; and a value with a quote, a backslash or a line break is also
    /// taken in the spelling JSON gives it, since most tools answer JSON.
    pub fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let set = self.read();
        if set.is_empty() {
            return Cow::Borrowed(text);
        }
        let mut needles = Vec::with_capacity(set.len() * 2);
        for (name, secret) in set.iter() {
            let values: Vec<(&str, String)> = match secret {
                Secret::Variable(Variable { value }) => vec![(value, format!("[redacted {name}]"))],
                Secret::Login(login) => {
                    let mut values = vec![(
                        login.password.as_str(),
                        format!("[redacted {name}.password]"),
                    )];
                    if let Some(seed) = &login.totp {
                        values.push((seed, format!("[redacted {name}.totp]")));
                    }
                    values
                }
                Secret::Passkey(passkey) => vec![(
                    passkey.private_key.as_str(),
                    format!("[redacted {name}.privateKey]"),
                )],
            };
            for (value, mark) in values {
                let escaped = serde_json::to_string(value)
                    .ok()
                    .and_then(|quoted| quoted.get(1..quoted.len() - 1).map(str::to_owned));
                if let Some(escaped) = escaped
                    && escaped != value
                {
                    needles.push((escaped, mark.clone()));
                }
                needles.push((value.to_owned(), mark));
            }
        }
        needles.sort_by_key(|(needle, _)| std::cmp::Reverse(needle.len()));
        let mut text = Cow::Borrowed(text);
        for (needle, mark) in &needles {
            if text.contains(needle.as_str()) {
                text = Cow::Owned(text.replace(needle.as_str(), mark));
            }
        }
        text
    }

    /// A tool's answer, or its refusal, with every value taken out of the text.
    pub fn redact_result(&self, result: ToolResult) -> ToolResult {
        match result {
            Ok(blocks) => Ok(blocks
                .into_iter()
                .map(|block| match block {
                    ContentBlock::Text(mut content) => {
                        if let Cow::Owned(redacted) = self.redact(&content.text) {
                            content.text = redacted;
                        }
                        ContentBlock::Text(content)
                    }
                    other => other,
                })
                .collect()),
            Err(error) => Err(self.redact(&error).into_owned()),
        }
    }
}

/// A name is the environment variable it becomes: capital letters, digits
/// and underscores, starting with a letter, at most 64 of them, and none the
/// computer or a shell already owns. Every kind is named the same way, so
/// one list holds them all. The desk checks the same before it stores one,
/// so a refusal here means the two disagree.
pub fn check_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let shaped = matches!(chars.next(), Some('A'..='Z'))
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    if !shaped || name.len() > MAX_NAME_CHARS {
        return Err(format!(
            "{name:?} is not a secret's name: capital letters, digits and underscores, starting with a letter, at most {MAX_NAME_CHARS}"
        ));
    }
    if name.starts_with("HOTLINE_") {
        return Err(format!(
            "{name} is the computer's own; a secret's name cannot start with HOTLINE_"
        ));
    }
    if RESERVED.contains(&name) {
        return Err(format!(
            "{name} belongs to a job's shell and cannot be a secret"
        ));
    }
    Ok(())
}

fn check_value(name: &str, value: &str) -> Result<(), String> {
    if value.chars().count() < MIN_VALUE_CHARS {
        return Err(format!(
            "{name} is shorter than {MIN_VALUE_CHARS} characters"
        ));
    }
    if value.len() > MAX_VALUE_BYTES {
        return Err(format!("{name} is longer than 64 KiB"));
    }
    if value.contains('\0') {
        return Err(format!(
            "{name} contains a NUL, which no environment variable can carry"
        ));
    }
    Ok(())
}

/// A web origin: the part of an address a page's identity is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    scheme: String,
    host: String,
    port: u16,
}

impl Origin {
    /// A site as the person spells it: `https://host[:port]`, or `http://`
    /// on localhost alone, with nothing after the host but a `/`.
    pub fn site(site: &str) -> Result<Origin, String> {
        let (origin, rest) = Origin::parse(site.trim())?;
        if !matches!(rest, "" | "/") {
            return Err(format!(
                "a site is an origin, {origin}, with nothing after the host; not {site:?}"
            ));
        }
        if origin.scheme == "http" && !is_local(&origin.host) {
            return Err(format!(
                "{site:?} is plain http; a login goes to https, or to http on localhost alone"
            ));
        }
        Ok(origin)
    }

    /// The origin of the page the browser is on. `data:`, `about:` and the
    /// like have none, and no login is typed into them.
    pub fn of_page(url: &str) -> Result<Origin, String> {
        Origin::parse(url).map(|(origin, _)| origin)
    }

    fn parse(text: &str) -> Result<(Origin, &str), String> {
        let (scheme, rest) = text
            .split_once("://")
            .ok_or_else(|| format!("{text:?} has no scheme"))?;
        let scheme = scheme.to_ascii_lowercase();
        if scheme != "https" && scheme != "http" {
            return Err(format!("{text:?} is not an https address"));
        }
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let (authority, rest) = rest.split_at(end);
        if authority.contains('@') {
            return Err(format!("{text:?} carries a user name in the address"));
        }
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) if host.ends_with(']') || !host.contains(':') => {
                let port: u16 = port
                    .parse()
                    .ok()
                    .filter(|port| *port != 0)
                    .ok_or_else(|| format!("{text:?} has no such port"))?;
                (host, port)
            }
            _ => (authority, if scheme == "https" { 443 } else { 80 }),
        };
        let host = host.to_ascii_lowercase();
        let bracketed = host.starts_with('[') && host.ends_with(']');
        let named = !host.is_empty()
            && !host.starts_with('.')
            && !host.ends_with('.')
            && host
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-');
        if !(named || bracketed) {
            return Err(format!("{text:?} has no host"));
        }
        Ok((Origin { scheme, host, port }, rest))
    }

    /// Whether a login for this site may be typed on `page`: same scheme and
    /// port, and the page's host is the site's or lies under it.
    pub fn allows(&self, page: &Origin) -> bool {
        self.scheme == page.scheme
            && self.port == page.port
            && (self.host == page.host || page.host.ends_with(&format!(".{}", self.host)))
    }
}

fn is_local(host: &str) -> bool {
    host == "localhost" || host.ends_with(".localhost") || host == "127.0.0.1" || host == "[::1]"
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}://{}", self.scheme, self.host)?;
        let default = if self.scheme == "https" { 443 } else { 80 };
        if self.port != default {
            write!(f, ":{}", self.port)?;
        }
        Ok(())
    }
}

/// A TOTP seed in one spelling: base32 in capitals, without the spaces,
/// dashes and padding issuers print it with.
fn totp_seed(seed: &str) -> Result<String, String> {
    let cleaned: String = seed
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-' && *c != '=')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if cleaned.len() < MIN_SEED_CHARS {
        return Err(format!(
            "a TOTP seed has at least {MIN_SEED_CHARS} base32 characters"
        ));
    }
    data_encoding::BASE32_NOPAD
        .decode(cleaned.as_bytes())
        .map_err(|_| {
            "a TOTP seed is base32: the letters A to Z and the digits 2 to 7".to_owned()
        })?;
    Ok(cleaned)
}

/// The six digits RFC 6238 gives for `seed` at `now` seconds: SHA-1,
/// thirty-second steps, which is what every issuer's QR means unless it
/// says otherwise.
fn totp(seed: &str, now: u64) -> Result<String, String> {
    let key = data_encoding::BASE32_NOPAD
        .decode(seed.as_bytes())
        .map_err(|_| "the TOTP seed is not base32".to_owned())?;
    let mut mac = Hmac::<Sha1>::new_from_slice(&key).map_err(|error| error.to_string())?;
    mac.update(&(now / TOTP_STEP_SECONDS).to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = usize::from(digest[19] & 0x0f);
    let binary = (u32::from(digest[offset] & 0x7f) << 24)
        | (u32::from(digest[offset + 1]) << 16)
        | (u32::from(digest[offset + 2]) << 8)
        | u32::from(digest[offset + 3]);
    Ok(format!("{:06}", binary % 1_000_000))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

/// `PUT /secrets`: the whole set, as a JSON object of name to entry. It
/// answers 204 with nothing, 400 naming the entry it refused, and never a
/// value. A running browser learns of the passkeys at once, so a revoked
/// one is gone from its authenticators before the answer.
pub async fn replace(
    State(app): State<App>,
    Json(set): Json<BTreeMap<String, Delivered>>,
) -> Response {
    match app.secrets.replace(set) {
        Ok(()) => {
            if let Err(error) = app.browser.sync_passkeys(false).await {
                eprintln!("secrets: the browser did not take the passkeys: {error}");
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => (StatusCode::BAD_REQUEST, Json(json!({"error": error}))).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;
    use std::path::PathBuf;

    fn set(entries: &[(&str, &str)]) -> BTreeMap<String, Delivered> {
        entries
            .iter()
            .map(|(name, value)| ((*name).to_owned(), Delivered::Value((*value).to_owned())))
            .collect()
    }

    fn typed(entries: Value) -> BTreeMap<String, Delivered> {
        serde_json::from_value(entries).unwrap()
    }

    fn app(home: PathBuf, token: Option<&str>) -> App {
        App::new(Config {
            addr: "127.0.0.1:0".to_owned(),
            token: token.map(str::to_owned),
            home,
            display: ":0".to_owned(),
            screen: "1920x1080".to_owned(),
        })
    }

    fn text(blocks: &[ContentBlock]) -> String {
        blocks
            .iter()
            .filter_map(|block| block.as_text().map(|content| content.text.as_str()))
            .collect()
    }

    /// RFC 6238's test seed, in base32.
    const RFC_SEED: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
    const KEY_BASE64: &str = "MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgeE7T2PDJPCRfPvTUFvVcQI7KFSDmnKbrRfEjRGbtV9WhRANCAATfeasaWHkMKJ4oCcdDzVX9c2xUUkC7Uiuqu8tS0LXtRJ8pCk+gNSvvqWaB3WgFNn4rvQ8wS1bH+dOjfgZoq2gz";

    #[test]
    fn a_name_is_an_environment_variable_and_the_computers_own_are_refused() {
        for name in ["GITHUB_TOKEN", "A", "NPM_TOKEN_2", &"A".repeat(64)] {
            check_name(name).unwrap_or_else(|error| panic!("{name}: {error}"));
        }
        for name in [
            "github_token",
            "1TOKEN",
            "GITHUB-TOKEN",
            "",
            "HOTLINE_COMPUTER_TOKEN",
            "HOTLINE_ANYTHING",
            "PATH",
            "LD_PRELOAD",
            &"A".repeat(65),
        ] {
            assert!(check_name(name).is_err(), "{name:?} is taken");
        }
    }

    #[test]
    fn the_set_is_taken_whole_or_the_last_one_kept() {
        let secrets = Secrets::default();
        secrets
            .replace(set(&[("GITHUB_TOKEN", "ghp_notarealtoken0001")]))
            .unwrap();
        let refused = secrets
            .replace(set(&[
                ("NPM_TOKEN", "npm_notarealtoken0002"),
                ("short", "npm_notarealtoken0003"),
            ]))
            .unwrap_err();
        assert!(refused.contains("\"short\""), "{refused}");
        assert_eq!(
            secrets.names(),
            ["GITHUB_TOKEN"],
            "a refused set changes nothing"
        );
        let refused = secrets
            .replace(set(&[("NPM_TOKEN", "1234567")]))
            .unwrap_err();
        assert!(refused.contains("8 characters"), "{refused}");
        let refused = secrets
            .replace(set(&[("NPM_TOKEN", "with\0nul-inside")]))
            .unwrap_err();
        assert!(refused.contains("NUL"), "{refused}");
        assert_eq!(
            secrets.environment(),
            BTreeMap::from([(
                "GITHUB_TOKEN".to_owned(),
                "ghp_notarealtoken0001".to_owned()
            )])
        );
        secrets.replace(BTreeMap::new()).unwrap();
        assert!(secrets.names().is_empty());
        assert!(secrets.environment().is_empty());
    }

    #[test]
    fn a_login_and_a_passkey_are_kept_by_kind_and_a_job_never_sees_them() {
        let secrets = Secrets::default();
        secrets
            .replace(typed(json!({
                "GITHUB_TOKEN": "ghp_notarealtoken0001",
                "GITHUB": {"kind": "login", "sites": ["https://GitHub.com/", "http://localhost:8123"],
                           "username": " george ", "password": "correct horse battery", "totp": "gezd gnbv-gy3t qojq gezd gnbv gy3t qojq"},
                "GITHUB_PASSKEY": {"kind": "passkey", "rpId": "github.com", "credentialId": "AQID",
                                   "privateKey": KEY_BASE64, "userName": "george"},
            })))
            .unwrap();
        assert_eq!(
            secrets.environment(),
            BTreeMap::from([(
                "GITHUB_TOKEN".to_owned(),
                "ghp_notarealtoken0001".to_owned()
            )]),
            "only a variable enters a job"
        );
        let catalog = secrets.catalog();
        assert_eq!(
            catalog,
            vec![
                json!({"name": "GITHUB", "kind": "login", "sites": ["https://github.com", "http://localhost:8123"], "username": "george", "totp": true}),
                json!({"name": "GITHUB_PASSKEY", "kind": "passkey", "rpId": "github.com", "userName": "george"}),
                json!({"name": "GITHUB_TOKEN", "kind": "variable"}),
            ]
        );
        assert!(!serde_json::to_string(&catalog).unwrap().contains("battery"));
        assert_eq!(secrets.passkeys().len(), 1);
        assert_eq!(secrets.passkeys()[0].0, "GITHUB_PASSKEY");
        for (name, refused, expect) in [
            (
                "no site",
                json!({"kind": "login", "sites": [], "username": "a", "password": "correct horse"}),
                "names no site",
            ),
            (
                "a site with a path",
                json!({"kind": "login", "sites": ["https://github.com/login"], "username": "a", "password": "correct horse"}),
                "nothing after the host",
            ),
            (
                "plain http",
                json!({"kind": "login", "sites": ["http://github.com"], "username": "a", "password": "correct horse"}),
                "plain http",
            ),
            (
                "no username",
                json!({"kind": "login", "sites": ["https://github.com"], "username": "  ", "password": "correct horse"}),
                "no username",
            ),
            (
                "a short password",
                json!({"kind": "login", "sites": ["https://github.com"], "username": "a", "password": "hunter2"}),
                "8 characters",
            ),
            (
                "a short seed",
                json!({"kind": "login", "sites": ["https://github.com"], "username": "a", "password": "correct horse", "totp": "GEZDGNBV"}),
                "16 base32",
            ),
            (
                "a seed that is not base32",
                json!({"kind": "login", "sites": ["https://github.com"], "username": "a", "password": "correct horse", "totp": "GEZDGNBVGY3TQOJQ1890"}),
                "base32",
            ),
            (
                "a passkey on no site",
                json!({"kind": "passkey", "rpId": "", "credentialId": "AQID", "privateKey": KEY_BASE64}),
                "site",
            ),
            (
                "a passkey with no key",
                json!({"kind": "passkey", "rpId": "github.com", "credentialId": "AQID", "privateKey": "AQID"}),
                "too short",
            ),
            (
                "a kind nobody knows",
                json!({"kind": "cookie", "value": "chocolate chip"}),
                "cookie",
            ),
        ] {
            let mut entries = BTreeMap::new();
            entries.insert("LOGIN".to_owned(), refused);
            let error = match serde_json::from_value::<BTreeMap<String, Delivered>>(Value::Object(
                entries.into_iter().collect(),
            )) {
                Ok(delivery) => secrets.replace(delivery).unwrap_err(),
                Err(error) => error.to_string(),
            };
            assert!(error.contains(expect), "{name}: {error}");
        }
        assert_eq!(
            secrets.names().len(),
            3,
            "a refused set leaves the last one"
        );
    }

    #[test]
    fn a_fill_resolves_a_field_by_name_and_a_code_from_the_seed() {
        let secrets = Secrets::default();
        secrets
            .replace(typed(json!({
                "GITHUB_TOKEN": "ghp_notarealtoken0001",
                "GITHUB": {"kind": "login", "sites": ["https://github.com"], "username": "george",
                           "password": "correct horse battery", "totp": RFC_SEED},
                "PLAIN": {"kind": "login", "sites": ["https://example.com"], "username": "george", "password": "correct horse battery"},
                "GITHUB_PASSKEY": {"kind": "passkey", "rpId": "github.com", "credentialId": "AQID", "privateKey": KEY_BASE64},
            })))
            .unwrap();
        let token = secrets.resolve("GITHUB_TOKEN").ok().unwrap();
        assert_eq!(
            (token.label.as_str(), token.value.as_str()),
            ("GITHUB_TOKEN", "ghp_notarealtoken0001")
        );
        assert!(token.sites.is_none(), "a variable has no site to check");
        let user = secrets.resolve("GITHUB.username").ok().unwrap();
        assert_eq!(
            (user.label.as_str(), user.value.as_str()),
            ("GITHUB.username", "george")
        );
        assert_eq!(
            user.sites.unwrap(),
            vec![Origin::site("https://github.com").unwrap()]
        );
        assert_eq!(
            secrets.resolve("GITHUB.password").ok().unwrap().value,
            "correct horse battery"
        );
        // RFC 6238, appendix B: the SHA-1 row for T = 59 ends in these digits.
        assert_eq!(
            secrets.resolve_at("GITHUB.code", 59).ok().unwrap().value,
            "287082"
        );
        assert_eq!(
            secrets
                .resolve_at("GITHUB.code", 1111111109)
                .ok()
                .unwrap()
                .value,
            "081804"
        );
        for (reference, expect) in [
            ("GITHUB", "username"),
            ("GITHUB.token", "not token"),
            ("GITHUB_TOKEN.value", "is a variable"),
            ("PLAIN.code", "no TOTP seed"),
            ("GITHUB_PASSKEY", "signs in by itself"),
            ("GITHUB_PASSKEY.password", "signs in by itself"),
            ("NOBODY.password", "no secret named NOBODY"),
        ] {
            let error = secrets.resolve(reference).err().unwrap_or_default();
            assert!(error.contains(expect), "{reference}: {error}");
            assert!(!error.contains("battery"), "{reference}: {error}");
        }
    }

    #[test]
    fn a_site_allows_its_own_pages_and_no_other() {
        let site = Origin::site("https://github.com").unwrap();
        for page in [
            "https://github.com/login",
            "https://GITHUB.com:443/settings?x=1#y",
            "https://login.github.com/sso",
        ] {
            assert!(site.allows(&Origin::of_page(page).unwrap()), "{page}");
        }
        for page in [
            "https://github.com.evil.example/login",
            "https://evilgithub.com/",
            "https://github.com:8443/",
            "http://github.com/",
            "https://gitlab.com/users/sign_in",
        ] {
            assert!(!site.allows(&Origin::of_page(page).unwrap()), "{page}");
        }
        for page in [
            "data:text/html,<form>",
            "about:blank",
            "chrome://settings",
            "",
        ] {
            assert!(Origin::of_page(page).is_err(), "{page}");
        }
        let local = Origin::site("http://localhost:8123/").unwrap();
        assert_eq!(local.to_string(), "http://localhost:8123");
        assert!(local.allows(&Origin::of_page("http://localhost:8123/index.html").unwrap()));
        assert!(!local.allows(&Origin::of_page("http://localhost:8124/").unwrap()));
        assert_eq!(
            Origin::site("https://Example.com:443").unwrap().to_string(),
            "https://example.com"
        );
        assert_eq!(
            Origin::site("https://[::1]:8443").unwrap().to_string(),
            "https://[::1]:8443"
        );
        for site in [
            "github.com",
            "ftp://github.com",
            "https://user@github.com",
            "https://",
            "https://github.com:0",
            "https://.github.com",
        ] {
            assert!(Origin::site(site).is_err(), "{site}");
        }
    }

    #[test]
    fn every_value_is_taken_out_of_a_text_whole_and_in_its_json_spelling() {
        let secrets = Secrets::default();
        assert!(matches!(secrets.redact("nothing stored"), Cow::Borrowed(_)));
        secrets
            .replace(typed(json!({
                "GITHUB_TOKEN": "ghp_notarealtoken0001",
                "LONGER": "ghp_notarealtoken0001-and-more",
                "PEM": "line one\nline \"two\"\\",
                "GITHUB": {"kind": "login", "sites": ["https://github.com"], "username": "george@example.com",
                           "password": "correct horse battery", "totp": RFC_SEED},
                "GITHUB_PASSKEY": {"kind": "passkey", "rpId": "github.com", "credentialId": "AQID", "privateKey": KEY_BASE64},
            })))
            .unwrap();
        assert_eq!(
            secrets.redact("token=ghp_notarealtoken0001;"),
            "token=[redacted GITHUB_TOKEN];"
        );
        // The longer value that contains the shorter one is taken whole.
        assert_eq!(
            secrets.redact("ghp_notarealtoken0001-and-more"),
            "[redacted LONGER]"
        );
        assert_eq!(secrets.redact("line one\nline \"two\"\\"), "[redacted PEM]");
        // Inside the JSON most tools answer, the same value reads escaped.
        let answer =
            serde_json::to_string(&json!({"output": "line one\nline \"two\"\\", "code": 0}))
                .unwrap();
        assert_eq!(
            secrets.redact(&answer),
            r#"{"code":0,"output":"[redacted PEM]"}"#
        );
        // A login's password and seed go; its username and site stay readable.
        assert_eq!(
            secrets.redact(&format!("signed in as george@example.com on github.com with correct horse battery and {RFC_SEED}")),
            "signed in as george@example.com on github.com with [redacted GITHUB.password] and [redacted GITHUB.totp]"
        );
        assert_eq!(
            secrets.redact(&format!("key {KEY_BASE64} end")),
            "key [redacted GITHUB_PASSKEY.privateKey] end"
        );
        // A piece of a value is the documented limit.
        assert!(matches!(secrets.redact("ghp_notareal"), Cow::Borrowed(_)));
        assert_eq!(
            secrets
                .redact_result(Err("refused: ghp_notarealtoken0001".into()))
                .unwrap_err(),
            "refused: [redacted GITHUB_TOKEN]"
        );
        let blocks = secrets
            .redact_result(Ok(vec![
                ContentBlock::text("ghp_notarealtoken0001"),
                ContentBlock::text("nothing here"),
            ]))
            .unwrap();
        assert_eq!(text(&blocks), "[redacted GITHUB_TOKEN]nothing here");
    }

    #[tokio::test]
    async fn a_job_finds_a_secret_by_name_and_the_tools_answer_the_name_alone() {
        let home = tempfile::tempdir().unwrap();
        let app = app(home.path().to_owned(), None);
        app.secrets
            .replace(typed(json!({
                "CONTRACT_SECRET": "contract-secret-value-0001",
                "SITE": {"kind": "login", "sites": ["https://example.com"], "username": "george", "password": "correct horse battery"},
            })))
            .unwrap();
        let printed = crate::tools::call(
            &app,
            "shell",
            json!({"command": "sh", "args": ["-c", "printf '%s|%s' \"$CONTRACT_SECRET\" \"${SITE-absent}\""]}),
            "tester",
        )
        .await
        .unwrap();
        let printed = text(&printed);
        assert!(
            printed.contains("[redacted CONTRACT_SECRET]|absent"),
            "{printed}"
        );
        assert!(!printed.contains("contract-secret-value-0001"), "{printed}");
        let info = crate::tools::call(&app, "state", json!({"action": "info"}), "tester")
            .await
            .unwrap();
        let info = text(&info);
        let report: serde_json::Value = serde_json::from_str(&info).unwrap();
        assert_eq!(
            report["secrets"],
            json!([
                {"name": "CONTRACT_SECRET", "kind": "variable"},
                {"name": "SITE", "kind": "login", "sites": ["https://example.com"], "username": "george", "totp": false},
            ])
        );
        assert!(!info.contains("contract-secret-value-0001"), "{info}");
        assert!(!info.contains("battery"), "{info}");
    }

    #[tokio::test]
    async fn the_door_takes_the_set_from_the_bearer_alone_and_answers_no_value() {
        let home = tempfile::tempdir().unwrap();
        let app = app(home.path().to_owned(), Some("the-token"));
        let secrets = app.secrets.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let router = crate::serve::router(app);
        let serving = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        let http = reqwest::Client::new();
        let door = format!("http://127.0.0.1:{port}/secrets");
        let delivery = json!({"GITHUB_TOKEN": "ghp_notarealtoken0001"});
        let naked = http.put(&door).json(&delivery).send().await.unwrap();
        assert_eq!(naked.status(), 401, "the door wants the bearer");
        assert!(secrets.names().is_empty());
        let taken = http
            .put(&door)
            .bearer_auth("the-token")
            .json(&delivery)
            .send()
            .await
            .unwrap();
        assert_eq!(taken.status(), 204);
        assert_eq!(secrets.names(), ["GITHUB_TOKEN"]);
        // A desk that knows kinds sends objects; the flat form still works.
        let taken = http
            .put(&door)
            .bearer_auth("the-token")
            .json(&json!({
                "GITHUB_TOKEN": {"kind": "variable", "value": "ghp_notarealtoken0001"},
                "GITHUB": {"kind": "login", "sites": ["https://github.com"], "username": "george", "password": "correct horse battery"},
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(taken.status(), 204);
        assert_eq!(secrets.names(), ["GITHUB", "GITHUB_TOKEN"]);
        let asked = http
            .get(&door)
            .bearer_auth("the-token")
            .send()
            .await
            .unwrap();
        assert_eq!(asked.status(), 405, "nothing answers a value");
        let refused = http
            .put(&door)
            .bearer_auth("the-token")
            .json(&json!({"github-token": "ghp_notarealtoken0002"}))
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), 400);
        assert!(refused.text().await.unwrap().contains("github-token"));
        let refused = http
            .put(&door)
            .bearer_auth("the-token")
            .json(&json!({"GITHUB": {"kind": "login", "sites": ["https://github.com"], "username": "george", "password": "pw12345"}}))
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), 400);
        let refusal = refused.text().await.unwrap();
        assert!(refusal.contains("GITHUB.password"), "{refusal}");
        assert!(
            !refusal.contains("pw12345"),
            "a refusal names the entry, not the value: {refusal}"
        );
        assert_eq!(
            secrets.names(),
            ["GITHUB", "GITHUB_TOKEN"],
            "a refused set leaves the last one"
        );
        let cleared = http
            .put(&door)
            .bearer_auth("the-token")
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(cleared.status(), 204);
        assert!(secrets.names().is_empty());
        serving.abort();
    }
}
