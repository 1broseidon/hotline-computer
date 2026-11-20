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
//! because what it captures is written into the workspace. A variable the
//! desk binds to a service (a v2 delivery's `use`, see `Delivery`) is a
//! **protected key**: only the split computer takes one, its value goes to
//! the key proxy alone, and each job gets a stand-in under its name
//! (`crate::split::keys`). A **login** — a
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
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard};
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
    /// Whether each use waits for the person; absent is automatic.
    #[serde(default, rename = "use", skip_serializing_if = "Use::is_auto")]
    pub use_: Use,
    /// Granted knowing a program on this computer may read it: what a v1
    /// delivery means by every login, and what a v2 one must say before a
    /// login on a loopback site is typed there (see `Origin::is_loopback`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub readable: bool,
}

/// How a login or a passkey may be used: by itself, or only once the person
/// approves each use at the desk (see `crate::holds`). A variable has no
/// use: it is in every job's environment from the start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Use {
    #[default]
    Auto,
    Ask,
}

impl Use {
    pub fn is_auto(&self) -> bool {
        *self == Use::Auto
    }
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
    /// Whether each sign-in waits for the person; absent is automatic.
    #[serde(default, rename = "use", skip_serializing_if = "Use::is_auto")]
    pub use_: Use,
    /// As a login's: a passkey for a loopback site signs there only if so.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub readable: bool,
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
                    use_: login.use_,
                    readable: login.readable,
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
            Secret::Login(login) => {
                let mut described = json!({
                    "name": name, "kind": "login", "sites": login.sites,
                    "username": login.username, "totp": login.totp.is_some(),
                });
                if login.use_ == Use::Ask {
                    described["use"] = json!("ask");
                }
                // A loopback site is one any job can serve: what is typed
                // there is readable, and is typed only if granted so.
                if login
                    .sites
                    .iter()
                    .any(|site| Origin::site(site).is_ok_and(|site| site.is_loopback()))
                {
                    described["loopback"] = json!(true);
                    described["readable"] = json!(true);
                    if !login.readable {
                        described["typed"] =
                            json!("not on its loopback site: not granted readable");
                    }
                }
                described
            }
            Secret::Passkey(passkey) => {
                let mut described = json!({
                    "name": name, "kind": "passkey", "rpId": passkey.rp_id,
                    "userName": passkey.user_name,
                });
                if passkey.use_ == Use::Ask {
                    described["use"] = json!("ask");
                }
                if is_loopback_host(&passkey.rp_id) {
                    described["loopback"] = json!(true);
                    described["readable"] = json!(true);
                    if !passkey.readable {
                        described["signs"] =
                            json!("not on its loopback site: not granted readable");
                    }
                }
                described
            }
        }
    }

    /// The use a login or a passkey asks for; a variable has none.
    pub fn use_(&self) -> Option<Use> {
        match self {
            Secret::Variable(_) => None,
            Secret::Login(login) => Some(login.use_),
            Secret::Passkey(passkey) => Some(passkey.use_),
        }
    }

    /// The hosts a login or a passkey is for, as a page's card matches them.
    fn hosts(&self) -> Vec<String> {
        match self {
            Secret::Variable(_) => Vec::new(),
            Secret::Login(login) => login
                .sites
                .iter()
                .filter_map(|site| Origin::site(site).ok())
                .map(|origin| origin.host)
                .collect(),
            Secret::Passkey(passkey) => vec![passkey.rp_id.clone()],
        }
    }
}

/// A login or a passkey that came with a delivery and was not in the one
/// before: what the card on a page of its site says arrived. Names and
/// hosts, never a value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Arrival {
    pub id: u64,
    pub name: String,
    pub kind: &'static str,
    pub sites: Vec<String>,
    /// Unix milliseconds.
    pub at: u64,
}

const ARRIVALS_KEPT: usize = 20;

/// A value the browser is about to type, with the sites it may be typed on
/// when it is a login's, and the name it is answered by.
#[derive(Clone, PartialEq, Eq)]
pub struct Filled {
    /// `GITHUB.password`: what the answer names instead of the value.
    pub label: String,
    pub value: String,
    /// `None` for a variable, which has no site to check.
    pub sites: Option<Vec<Origin>>,
    /// A login granted knowing a program here may read it (`Login::readable`).
    pub readable: bool,
}

/// The set the desk last delivered, shared between the door that takes it,
/// the jobs that start with it, the browser that types and signs with it,
/// and the answers it is taken out of.
#[derive(Clone, Default)]
pub struct Secrets {
    set: Arc<RwLock<BTreeMap<String, Secret>>>,
    arrivals: Arc<Mutex<(u64, VecDeque<Arrival>)>>,
    /// In the split computer's runner, which holds the variables alone: the
    /// whole set as `state info` lists it, from the broker.
    listing: Arc<RwLock<Option<Vec<Value>>>>,
    /// In the split computer's broker: the protected keys, whose values the
    /// key proxy uses. Listed and redacted here, never in `environment`, so
    /// never handed to the runner or a job.
    protected: Arc<RwLock<BTreeMap<String, Protected>>>,
    /// The matchers `redact` uses, made once per set: (the set's version,
    /// each value's every spelling and its mark, longest value first).
    spellings: Arc<Mutex<Option<(u64, Spellings)>>>,
    version: Arc<std::sync::atomic::AtomicU64>,
}

/// Each value's every spelling, matched, and the mark it is replaced by.
type Spellings = Arc<Vec<(regex::Regex, String)>>;

/// A protected key as the broker keeps it beside the proxy.
#[derive(Clone)]
pub struct Protected {
    pub value: String,
    pub origins: Vec<String>,
}

/// A set that passed every check, waiting to be taken.
pub struct Checked(BTreeMap<String, Secret>);

impl Secrets {
    /// Takes the whole set, or none of it: one entry the computer would not
    /// keep refuses the delivery and keeps the last set.
    pub fn replace(&self, set: BTreeMap<String, Delivered>) -> Result<(), String> {
        let checked = Secrets::check(set)?;
        self.commit(checked, BTreeMap::new());
        Ok(())
    }

    /// Every entry checked, nothing taken yet.
    pub fn check(set: BTreeMap<String, Delivered>) -> Result<Checked, String> {
        let mut checked = BTreeMap::new();
        for (name, delivered) in set {
            check_name(&name)?;
            let secret = Secret::from(delivered).checked(&name)?;
            checked.insert(name, secret);
        }
        Ok(Checked(checked))
    }

    /// Takes a checked set, and the protected keys delivered with it.
    pub fn commit(&self, checked: Checked, protected: BTreeMap<String, Protected>) {
        let Checked(checked) = checked;
        self.version
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        *self
            .protected
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = protected;
        let mut set = self
            .set
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let new: Vec<(String, &'static str, Vec<String>)> = checked
            .iter()
            .filter(|(name, _)| !set.contains_key(*name))
            .filter_map(|(name, secret)| {
                let kind = match secret {
                    Secret::Variable(_) => return None,
                    Secret::Login(_) => "login",
                    Secret::Passkey(_) => "passkey",
                };
                Some((name.clone(), kind, secret.hosts()))
            })
            .collect();
        *set = checked;
        drop(set);
        if !new.is_empty() {
            let at = crate::jobs::now();
            let mut arrivals = self
                .arrivals
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            for (name, kind, sites) in new {
                arrivals.0 += 1;
                let id = arrivals.0;
                arrivals.1.push_back(Arrival {
                    id,
                    name,
                    kind,
                    sites,
                    at,
                });
                while arrivals.1.len() > ARRIVALS_KEPT {
                    arrivals.1.pop_front();
                }
            }
        }
    }

    fn protected(&self) -> RwLockReadGuard<'_, BTreeMap<String, Protected>> {
        self.protected
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The logins and passkeys that arrived lately, oldest first.
    pub fn arrivals(&self) -> Vec<Arrival> {
        self.arrivals
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .1
            .iter()
            .cloned()
            .collect()
    }

    /// How a login or passkey named `name` may be used, read from the set as
    /// it stands now, so a delivery that changes it applies from the next
    /// use. `None` when there is no such login or passkey.
    pub fn use_of(&self, name: &str) -> Option<Use> {
        self.read().get(name).and_then(Secret::use_)
    }

    /// Each entry's version: a digest of what it is now, protected keys'
    /// included. Two sets differ for a name when it was changed or revoked.
    pub fn versions(&self) -> BTreeMap<String, Vec<u8>> {
        use sha2::Digest;
        let mut versions: BTreeMap<String, Vec<u8>> = self
            .read()
            .iter()
            .map(|(name, secret)| {
                let text = serde_json::to_vec(secret).unwrap_or_default();
                (name.clone(), sha2::Sha256::digest(text).to_vec())
            })
            .collect();
        for (name, key) in self.protected().iter() {
            let text = format!("{}\0{}", key.value, key.origins.join(","));
            versions.insert(name.clone(), sha2::Sha256::digest(text).to_vec());
        }
        versions
    }

    /// The names of what this computer cannot keep from its jobs, though it
    /// holds it as protected: a login or passkey for a loopback site (a page
    /// a job may serve), and a protected key for an "Other" service. What
    /// `/capabilities` reports so the desk can label them readable.
    pub fn readable_equivalent(&self) -> Vec<String> {
        self.catalog()
            .into_iter()
            .filter(|entry| entry["readable"] == true)
            .filter_map(|entry| entry["name"].as_str().map(str::to_owned))
            .collect()
    }

    /// The names alone, protected keys' included.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.read().keys().cloned().collect();
        names.extend(self.protected().keys().cloned());
        names.sort();
        names
    }

    /// What `state info` lists: name, kind, and for a login its sites and
    /// username, for a passkey its site. Never a value.
    pub fn catalog(&self) -> Vec<Value> {
        if let Some(listing) = self
            .listing
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        {
            return listing;
        }
        let mut listed: Vec<(String, Value)> = self
            .read()
            .iter()
            .map(|(name, secret)| (name.clone(), secret.describe(name)))
            .collect();
        listed.extend(self.protected().iter().map(|(name, key)| {
            let mut described =
                json!({"name": name, "kind": "variable", "protected": true, "origins": key.origins});
            // An "Other" service: replies the proxy cannot know may carry the
            // key, or a new one, back in a shape no scrubbing finds.
            if is_other(&key.origins) {
                described["other"] = json!(true);
                described["readable"] = json!(true);
            }
            (name.clone(), described)
        }));
        listed.sort_by(|a, b| a.0.cmp(&b.0));
        listed.into_iter().map(|(_, described)| described).collect()
    }

    /// What `catalog` answers from now on, in place of this set's own.
    pub fn list_as(&self, listing: Vec<Value>) {
        *self
            .listing
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(listing);
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
    /// The logins that may be typed on `page`, an origin: what the browser
    /// extension offers there, by name, with the username and whether a
    /// code comes with it. Never a password.
    pub fn logins_for(&self, page: &str) -> Vec<(String, String, bool)> {
        let Ok(page) = Origin::site(page) else {
            return Vec::new();
        };
        self.read()
            .iter()
            .filter_map(|(name, secret)| match secret {
                Secret::Login(login)
                    if login
                        .sites
                        .iter()
                        .filter_map(|site| Origin::site(site).ok())
                        .any(|site| site.is_exactly(&page)) =>
                {
                    Some((name.clone(), login.username.clone(), login.totp.is_some()))
                }
                _ => None,
            })
            .collect()
    }

    pub fn resolve(&self, reference: &str) -> Result<Filled, String> {
        self.resolve_at(reference, unix_now())
    }

    fn resolve_at(&self, reference: &str, now: u64) -> Result<Filled, String> {
        let (name, field) = match reference.split_once('.') {
            Some((name, field)) => (name, Some(field)),
            None => (reference, None),
        };
        if self.protected().contains_key(name) {
            return Err(format!(
                "{name} is a protected key: every shell job has a stand-in for it as ${name}, which the computer swaps for the key only on the way to its service, and the browser never types one"
            ));
        }
        let set = self.read();
        let Some(secret) = set.get(name) else {
            return Err(format!(
                "no secret named {name} is in this computer; state info lists them"
            ));
        };
        match (secret, field) {
            // A variable has no site, so no page it belongs on: typed into
            // one, that page's script could read it and send it anywhere.
            // It is in every shell job's environment instead.
            (Secret::Variable(_), None) => Err(format!(
                "{name} is a variable: it is in the environment of every shell job as ${name}, and the browser never types one"
            )),
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
                    readable: login.readable,
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
        self.set
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// `text` with every secret value replaced by `[redacted NAME]`: a
    /// variable's value, a login's password and seed, a passkey's private
    /// key. Longer values go first, so one that contains another is taken
    /// whole; and a value with a quote, a backslash or a line break is also
    /// taken in the spelling JSON gives it, since most tools answer JSON.
    pub fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
        self.redact_at(text, unix_now())
    }

    fn redact_at<'a>(&self, text: &'a str, now: u64) -> Cow<'a, str> {
        let codes: Vec<(String, String)> = self
            .read()
            .iter()
            .filter_map(|(name, secret)| match secret {
                Secret::Login(Login {
                    totp: Some(seed), ..
                }) => Some((seed.clone(), format!("[redacted {name}.code]"))),
                _ => None,
            })
            .collect();
        let spellings = self.spellings();
        if spellings.is_empty() {
            return Cow::Borrowed(text);
        }
        let mut text = Cow::Borrowed(text);
        for (spelled, mark) in spellings.iter() {
            if spelled.is_match(&text) {
                text = Cow::Owned(
                    spelled
                        .replace_all(&text, regex::NoExpand(mark.as_str()))
                        .into_owned(),
                );
            }
        }
        // A login's six digits are a password for a minute: the ones a page
        // or a job could be showing now, from the step before last to the
        // next, are taken out wherever they stand as a number of their own.
        for (seed, mark) in codes {
            for step in [-2_i64, -1, 0, 1] {
                let at = now.saturating_add_signed(step * TOTP_STEP_SECONDS as i64);
                if let Ok(code) = totp(&seed, at)
                    && let Cow::Owned(redacted) = without_number(&text, &code, &mark)
                {
                    text = Cow::Owned(redacted);
                }
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

impl Secrets {
    /// Each value's matcher and mark, made again only when the set changed.
    fn spellings(&self) -> Spellings {
        let version = self.version.load(std::sync::atomic::Ordering::SeqCst);
        let mut cached = self
            .spellings
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((made, spellings)) = cached.as_ref()
            && *made == version
        {
            return Arc::clone(spellings);
        }
        let mut values: Vec<(String, String)> = Vec::new();
        for (name, key) in self.protected().iter() {
            values.push((key.value.clone(), format!("[redacted {name}]")));
        }
        for (name, secret) in self.read().iter() {
            match secret {
                Secret::Variable(Variable { value }) => {
                    values.push((value.clone(), format!("[redacted {name}]")));
                }
                Secret::Login(login) => {
                    values.push((
                        login.password.clone(),
                        format!("[redacted {name}.password]"),
                    ));
                    if let Some(seed) = &login.totp {
                        values.push((seed.clone(), format!("[redacted {name}.totp]")));
                    }
                }
                Secret::Passkey(passkey) => values.push((
                    passkey.private_key.clone(),
                    format!("[redacted {name}.privateKey]"),
                )),
            }
        }
        values.sort_by_key(|(value, _)| std::cmp::Reverse(value.len()));
        let spellings: Spellings = Arc::new(
            values
                .into_iter()
                .filter_map(|(value, mark)| spelling(&value).map(|spelled| (spelled, mark)))
                .collect(),
        );
        *cached = Some((version, Arc::clone(&spellings)));
        spellings
    }
}

/// Longer values are matched in their plain, JSON and base64 spellings
/// alone: a pattern of every character's every spelling grows past what a
/// matcher should hold, and such values are keys, not typed words.
const SPELLED_CHARS: usize = 512;

/// One matcher for `value` as text carries it: plain; percent-encoded
/// (either case) or form-encoded (`+` for a space); with HTML entities,
/// named or numeric; with JSON's escapes; any mix of these character by
/// character; and base64 (standard or URL-safe) at each of the three
/// alignments a value can have inside a longer base64 text.
fn spelling(value: &str) -> Option<regex::Regex> {
    if value.is_empty() {
        return None;
    }
    let mut alternatives = Vec::new();
    if value.chars().count() <= SPELLED_CHARS {
        let mut each = String::new();
        for c in value.chars() {
            each.push_str("(?:");
            each.push_str(&char_spellings(c).join("|"));
            each.push(')');
        }
        alternatives.push(each);
    } else {
        alternatives.push(regex::escape(value));
        if let Ok(quoted) = serde_json::to_string(value)
            && let Some(escaped) = quoted.get(1..quoted.len() - 1)
        {
            alternatives.push(regex::escape(escaped));
        }
    }
    for core in base64_cores(value.as_bytes()) {
        alternatives.push(regex::escape(&core));
    }
    regex::RegexBuilder::new(&alternatives.join("|"))
        .size_limit(64 << 20)
        .build()
        .ok()
}

/// The spellings of one character, longest first: the matcher takes the
/// first that fits, so an escape is never read as its first character.
fn char_spellings(c: char) -> Vec<String> {
    let mut spellings = Vec::new();
    let mut bytes = [0; 4];
    let percent: String = c
        .encode_utf8(&mut bytes)
        .bytes()
        .map(|byte| format!("%(?i:{byte:02x})"))
        .collect();
    spellings.push(percent);
    let code = u32::from(c);
    spellings.push(format!("&#0*{code};?"));
    spellings.push(format!("&#(?i:x)0*(?i:{code:x});?"));
    let named = match c {
        '&' => Some("amp"),
        '<' => Some("lt"),
        '>' => Some("gt"),
        '"' => Some("quot"),
        '\'' => Some("apos"),
        ' ' => Some("nbsp"),
        _ => None,
    };
    if let Some(named) = named {
        spellings.push(format!("&(?i:{named});"));
    }
    match c {
        ' ' => spellings.push(r"\+".to_owned()),
        '"' => spellings.push(r#"\\""#.to_owned()),
        '\\' => spellings.push(r"\\\\".to_owned()),
        '/' => spellings.push(r"\\/".to_owned()),
        '\n' => spellings.push(r"\\n".to_owned()),
        '\r' => spellings.push(r"\\r".to_owned()),
        '\t' => spellings.push(r"\\t".to_owned()),
        _ => {}
    }
    if code <= 0xffff {
        spellings.push(format!(r"\\(?i:u{code:04x})"));
    }
    spellings.push(regex::escape(c.encode_utf8(&mut [0; 4])));
    spellings
}

/// The base64 characters that depend on `value` alone, at each alignment
/// it may start at inside a longer text, in the standard and URL-safe
/// alphabets.
fn base64_cores(value: &[u8]) -> Vec<String> {
    use base64::engine::general_purpose::STANDARD_NO_PAD;
    let mut cores = Vec::new();
    for offset in 0..3usize {
        let mut padded = vec![0u8; offset];
        padded.extend_from_slice(value);
        let encoded = STANDARD_NO_PAD.encode(&padded);
        let start = (8 * offset).div_ceil(6);
        let end = (8 * (offset + value.len())) / 6;
        if end <= start || end - start < 8 {
            continue;
        }
        let core = encoded[start..end.min(encoded.len())].to_owned();
        let safe = core.replace('+', "-").replace('/', "_");
        if safe != core {
            cores.push(safe);
        }
        cores.push(core);
    }
    cores
}

/// Whether `text` carries `value` in any spelling `redact` takes out.
pub fn spelled(text: &str, value: &str) -> bool {
    spelling(value).is_some_and(|spelled| spelled.is_match(text))
}

/// `text` with each standalone `number` (no digit right before or after it)
/// replaced by `mark`: a code inside a longer number is that number's, not
/// the code.
fn without_number<'a>(text: &'a str, number: &str, mark: &str) -> Cow<'a, str> {
    if !text.contains(number) {
        return Cow::Borrowed(text);
    }
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut changed = false;
    for (at, _) in text.match_indices(number) {
        if at < last {
            continue;
        }
        let end = at + number.len();
        let before = at > 0 && bytes[at - 1].is_ascii_digit();
        let after = end < bytes.len() && bytes[end].is_ascii_digit();
        if before || after {
            continue;
        }
        out.push_str(&text[last..at]);
        out.push_str(mark);
        last = end;
        changed = true;
    }
    if !changed {
        return Cow::Borrowed(text);
    }
    out.push_str(&text[last..]);
    Cow::Owned(out)
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
    /// A passkey's site, which WebAuthn scopes by host alone: any port.
    any_port: bool,
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
        Ok((
            Origin {
                scheme,
                host,
                port,
                any_port: false,
            },
            rest,
        ))
    }

    /// The sites a passkey for `rp_id` signs in on, as WebAuthn scopes them:
    /// the host and those under it, over https on any port, and over http
    /// too on localhost. Empty for an RP id that is not a host.
    pub fn relying_party(rp_id: &str) -> Vec<Origin> {
        let Ok(https) = Origin::site(&format!("https://{rp_id}")) else {
            return Vec::new();
        };
        let mut sites = vec![Origin {
            any_port: true,
            ..https.clone()
        }];
        if is_local(&https.host) {
            sites.push(Origin {
                scheme: "http".into(),
                ..sites[0].clone()
            });
        }
        sites
    }

    /// The site's host.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Whether the site is this computer itself (`is_loopback_host`).
    pub fn is_loopback(&self) -> bool {
        is_loopback_host(&self.host)
    }

    /// Whether `page` is exactly this origin: scheme, host and port. A
    /// login's field is typed only there: a subdomain may be delegated to
    /// someone else, so it gets the password only if the grant names it.
    pub fn is_exactly(&self, page: &Origin) -> bool {
        self.scheme == page.scheme
            && self.host == page.host
            && (self.any_port || self.port == page.port)
    }

    /// Whether a page of `page` belongs to this site: same scheme and port,
    /// and the page's host is the site's or lies under it. What opens in
    /// the signed-in browser; a login is typed only on `is_exactly`.
    pub fn allows(&self, page: &Origin) -> bool {
        self.scheme == page.scheme
            && (self.any_port || self.port == page.port)
            && (self.host == page.host || page.host.ends_with(&format!(".{}", self.host)))
    }
}

fn is_local(host: &str) -> bool {
    is_loopback_host(host)
}

/// The services Hotline names, by the origins a key for each is used on
/// (the desk's `services.json`). A key bound anywhere else is "Other".
const SERVICES: [&[&str]; 3] = [
    &[
        "https://api.github.com",
        "https://github.com",
        "https://uploads.github.com",
    ],
    &["https://api.openai.com"],
    &["https://api.anthropic.com"],
];

/// Whether a protected key's origins are not all one named service's: the
/// proxy can neither know its replies nor which routes mint a new key, so
/// it is reported readable-equivalent (still used through the proxy).
pub fn is_other(origins: &[String]) -> bool {
    origins.is_empty()
        || !SERVICES.iter().any(|service| {
            origins
                .iter()
                .all(|origin| service.contains(&origin.as_str()))
        })
}

/// Whether `host` names this computer itself, however it is spelled:
/// `localhost` and any name under it, 127.0.0.0/8 in any of the IPv4
/// spellings a URL parser accepts (`127.1`, `0x7f.1`, `2130706433`),
/// `0.0.0.0`, `::1`, `::` and an IPv4-mapped loopback. A page there may be
/// one a job serves, so nothing typed or signed there is protected.
pub fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    if let Some(inner) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        return inner.parse::<std::net::Ipv6Addr>().is_ok_and(|ip| {
            ip.is_loopback()
                || ip.is_unspecified()
                || ip
                    .to_ipv4_mapped()
                    .is_some_and(|v4| v4.is_loopback() || v4.is_unspecified())
        });
    }
    ipv4_of(&host).is_some_and(|ip| ip >> 24 == 127 || ip == 0)
}

/// A host as the WHATWG URL parser reads an IPv4 address: one to four
/// parts, each decimal, `0x` hex or `0` octal, the last filling the rest.
fn ipv4_of(host: &str) -> Option<u32> {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.is_empty() || parts.len() > 4 {
        return None;
    }
    let mut numbers = Vec::with_capacity(parts.len());
    for part in &parts {
        let (digits, radix) = if let Some(hex) = part.strip_prefix("0x") {
            (hex, 16)
        } else if part.len() > 1 && part.starts_with('0') {
            (&part[1..], 8)
        } else {
            (*part, 10)
        };
        let number = if digits.is_empty() {
            0
        } else {
            u64::from_str_radix(digits, radix).ok()?
        };
        numbers.push(number);
    }
    let last = numbers.pop()?;
    let mut ip: u64 = 0;
    for (index, number) in numbers.iter().enumerate() {
        if *number > 255 {
            return None;
        }
        ip |= number << (8 * (3 - index));
    }
    let room = 8 * (4 - numbers.len()) as u32;
    if last >= 1u64 << room {
        return None;
    }
    u32::try_from(ip | last).ok()
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}://{}", self.scheme, self.host)?;
        let default = if self.scheme == "https" { 443 } else { 80 };
        if !self.any_port && self.port != default {
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

/// The media type of a v2 delivery.
pub const V2: &str = "application/vnd.hotline.secrets.v2+json";

/// A delivery as the door read it: the entries every computer keeps as
/// before, and the protected keys only the split computer's proxy takes.
pub struct Delivery {
    /// v2 only; a v1 body has none.
    pub generation: Option<u64>,
    pub kept: BTreeMap<String, Delivered>,
    pub protected: Vec<crate::keyproxy::Binding>,
}

impl Delivery {
    /// A v1 body is an object of name to entry, as a desk before 0.50 sends
    /// it. A v2 body (`Content-Type` `V2`) is `{v: 2, generation, secrets}`,
    /// where a variable is `readable: true` (today's variable) or carries a
    /// `use` (a protected key); logins and passkeys are as in v1.
    pub fn read(content_type: Option<&str>, body: Value) -> Result<Delivery, String> {
        let essence = content_type
            .and_then(|value| value.split(';').next())
            .map(str::trim)
            .unwrap_or_default();
        if essence.eq_ignore_ascii_case(V2) {
            return Delivery::v2(body);
        }
        let Value::Object(entries) = body else {
            return Err("a delivery is an object of name to secret".to_owned());
        };
        for (name, entry) in &entries {
            if entry["kind"] == "variable" && entry.get("use").is_some() {
                return Err(format!(
                    "{name} is bound to a service, which only a v2 delivery ({V2}) can say"
                ));
            }
        }
        let mut kept: BTreeMap<String, Delivered> =
            serde_json::from_value(Value::Object(entries)).map_err(|e| e.to_string())?;
        // A desk that sends v1 holds nothing back as protected: it labels
        // this computer unprotected, and every login and passkey it sends
        // is as readable as 0.12 kept them.
        for delivered in kept.values_mut() {
            match delivered {
                Delivered::Secret(Secret::Login(login)) => login.readable = true,
                Delivered::Secret(Secret::Passkey(passkey)) => passkey.readable = true,
                _ => {}
            }
        }
        Ok(Delivery {
            generation: None,
            kept,
            protected: Vec::new(),
        })
    }

    fn v2(body: Value) -> Result<Delivery, String> {
        if body["v"] != 2 {
            return Err(format!("a {V2} body says v: 2"));
        }
        let generation = body["generation"]
            .as_u64()
            .ok_or("a v2 delivery carries its generation, a whole number")?;
        let Some(entries) = body["secrets"].as_object() else {
            return Err("a v2 delivery carries secrets, an object of name to secret".to_owned());
        };
        let mut kept = BTreeMap::new();
        let mut protected = Vec::new();
        for (name, entry) in entries {
            if entry["kind"] != "variable" {
                let delivered: Delivered =
                    serde_json::from_value(entry.clone()).map_err(|e| format!("{name}: {e}"))?;
                kept.insert(name.clone(), delivered);
                continue;
            }
            let Some(value) = entry["value"].as_str() else {
                return Err(format!("{name} is a variable with no value"));
            };
            let readable = entry["readable"].as_bool() == Some(true);
            match (entry.get("use"), readable) {
                (None, true) => {
                    kept.insert(name.clone(), Delivered::Value(value.to_owned()));
                }
                (Some(use_), false) => {
                    check_name(name)?;
                    if crate::split::keys::JOB_VARIABLES.contains(&name.as_str()) {
                        return Err(format!(
                            "{name} is set by the computer for a job that uses a protected key, so a protected key cannot take that name"
                        ));
                    }
                    let use_ = serde_json::from_value(use_.clone())
                        .map_err(|e| format!("{name}.use: {e}"))?;
                    protected.push(crate::keyproxy::Binding {
                        name: name.clone(),
                        value: value.to_owned(),
                        use_,
                    });
                }
                (Some(_), true) => {
                    return Err(format!(
                        "{name} is both readable and bound to a service; it is one or the other"
                    ));
                }
                (None, false) => {
                    return Err(format!(
                        "{name} is neither readable nor bound to a service (use)"
                    ));
                }
            }
        }
        Ok(Delivery {
            generation: Some(generation),
            kept,
            protected,
        })
    }

    /// Takes the delivery whole or not at all. A protected key goes to the
    /// key proxy alone; a computer without one (legacy mode) refuses it.
    pub fn apply(self, app: &App) -> Result<(), String> {
        let checked = Secrets::check(self.kept)?;
        let listed: BTreeMap<String, Protected> = self
            .protected
            .iter()
            .map(|binding| {
                (
                    binding.name.clone(),
                    Protected {
                        value: binding.value.clone(),
                        origins: binding.use_.origins.clone(),
                    },
                )
            })
            .collect();
        match &app.keys {
            Some(keys) => keys.deliver(self.generation, self.protected)?,
            None => {
                if let Some(binding) = self.protected.first() {
                    return Err(format!(
                        "{} is bound to a service, and this computer keeps no protected key: it runs as one user (legacy mode), so every program in it could read one. Send it as a readable variable, or update the computer.",
                        binding.name
                    ));
                }
            }
        }
        app.secrets.commit(checked, listed);
        Ok(())
    }
}

/// `PUT /secrets`: the whole set, as a v1 or v2 body (`Delivery`). It
/// answers 204 with nothing, 400 naming the entry it refused, and never a
/// value. A running browser learns of the passkeys at once, so a revoked
/// one is gone from its authenticators before the answer.
pub async fn replace(
    State(app): State<App>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok());
    let before = app.secrets.versions();
    match Delivery::read(content_type, body).and_then(|delivery| delivery.apply(&app)) {
        Ok(()) => {
            // An approval or allowance is for the key as it was: one that
            // changed or went takes its answers with it.
            let after = app.secrets.versions();
            let changed: Vec<String> = before
                .iter()
                .filter(|(name, version)| after.get(*name) != Some(*version))
                .map(|(name, _)| name.clone())
                .collect();
            app.holds.forget(&changed);
            crate::split::delivered(&app).await;
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
    fn the_use_rides_with_the_set_and_a_change_applies_from_the_next_use() {
        let secrets = Secrets::default();
        let delivered = |use_: Option<&str>| {
            let mut login = json!({"kind": "login", "sites": ["https://github.com"],
                                   "username": "george", "password": "correct horse battery"});
            let mut passkey = json!({"kind": "passkey", "rpId": "github.com", "credentialId": "AQID",
                                     "privateKey": KEY_BASE64, "userName": "george"});
            if let Some(use_) = use_ {
                login["use"] = json!(use_);
                passkey["use"] = json!(use_);
            }
            typed(
                json!({"GH_LOGIN": login, "GH_PASSKEY": passkey, "NPM_TOKEN": "npm_notarealtoken0002"}),
            )
        };
        secrets.replace(delivered(None)).unwrap();
        assert_eq!(
            secrets.use_of("GH_LOGIN"),
            Some(Use::Auto),
            "absent is auto"
        );
        assert_eq!(secrets.use_of("GH_PASSKEY"), Some(Use::Auto));
        assert_eq!(secrets.use_of("NPM_TOKEN"), None, "a variable has no use");
        assert!(
            !serde_json::to_string(&secrets.catalog())
                .unwrap()
                .contains("\"use\""),
            "an automatic key says nothing of it"
        );
        secrets.replace(delivered(Some("ask"))).unwrap();
        assert_eq!(secrets.use_of("GH_LOGIN"), Some(Use::Ask));
        assert_eq!(secrets.use_of("GH_PASSKEY"), Some(Use::Ask));
        assert_eq!(secrets.passkeys()[0].1.use_, Use::Ask);
        let catalog = secrets.catalog();
        assert_eq!(catalog[0]["use"], "ask", "{catalog:?}");
        assert_eq!(catalog[1]["use"], "ask", "{catalog:?}");
        secrets.replace(delivered(Some("auto"))).unwrap();
        assert_eq!(secrets.use_of("GH_LOGIN"), Some(Use::Auto));
        assert_eq!(secrets.use_of("GH_PASSKEY"), Some(Use::Auto));
        let refused = serde_json::from_value::<BTreeMap<String, Delivered>>(json!({
            "GH_LOGIN": {"kind": "login", "sites": ["https://github.com"], "username": "george",
                         "password": "correct horse battery", "use": "sometimes"}
        }));
        assert!(refused.is_err(), "a use is auto or ask");
    }

    #[test]
    fn a_login_or_passkey_new_to_the_set_is_an_arrival_and_a_variable_never() {
        let secrets = Secrets::default();
        secrets
            .replace(typed(json!({"NPM_TOKEN": "npm_notarealtoken0002"})))
            .unwrap();
        assert!(secrets.arrivals().is_empty());
        let set = json!({
            "NPM_TOKEN": "npm_notarealtoken0002",
            "GITHUB_LOGIN": {"kind": "login", "sites": ["https://github.com", "http://localhost:8123"],
                             "username": "george", "password": "correct horse battery"},
        });
        secrets.replace(typed(set.clone())).unwrap();
        let arrived = secrets.arrivals();
        assert_eq!(arrived.len(), 1);
        assert_eq!(arrived[0].name, "GITHUB_LOGIN");
        assert_eq!(arrived[0].kind, "login");
        assert_eq!(arrived[0].sites, ["github.com", "localhost"]);
        secrets.replace(typed(set)).unwrap();
        assert_eq!(
            secrets.arrivals().len(),
            1,
            "delivered again, it is not new"
        );
        secrets
            .replace(typed(json!({
                "GH_PASSKEY": {"kind": "passkey", "rpId": "github.com", "credentialId": "AQID",
                               "privateKey": KEY_BASE64},
            })))
            .unwrap();
        let arrived = secrets.arrivals();
        assert_eq!(arrived.len(), 2);
        assert_eq!(arrived[1].name, "GH_PASSKEY");
        assert_eq!(arrived[1].sites, ["github.com"]);
        assert!(arrived[1].id > arrived[0].id);
        let wire = serde_json::to_string(&arrived).unwrap();
        assert!(
            !wire.contains("battery") && !wire.contains(KEY_BASE64),
            "{wire}"
        );
    }

    #[test]
    fn the_extension_is_offered_a_login_only_where_it_may_be_typed() {
        let secrets = Secrets::default();
        secrets
            .replace(typed(json!({
                "GITHUB": {"kind": "login", "sites": ["https://github.com"], "username": "george",
                           "password": "correct horse battery", "totp": "gezdgnbvgy3tqojqgezdgnbvgy3tqojq"},
                "LOCAL": {"kind": "login", "sites": ["http://localhost:8123"], "username": "teammate", "password": "pw-for-the-test"},
                "GITHUB_TOKEN": "ghp_notarealtoken0001",
            })))
            .unwrap();
        let named = |page: &str| -> Vec<String> {
            secrets
                .logins_for(page)
                .into_iter()
                .map(|(name, _, _)| name)
                .collect()
        };
        assert_eq!(
            secrets.logins_for("https://github.com"),
            [("GITHUB".to_owned(), "george".to_owned(), true)]
        );
        assert_eq!(named("https://gist.github.com"), ["GITHUB"]);
        assert!(named("https://github.com.evil.test").is_empty());
        assert!(named("http://github.com").is_empty());
        assert!(named("https://github.com:8443").is_empty());
        assert_eq!(named("http://localhost:8123"), ["LOCAL"]);
        assert!(named("not an origin").is_empty());
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
                // A loopback site is labelled readable, and not typed on
                // unless the login was granted so.
                json!({"name": "GITHUB", "kind": "login", "sites": ["https://github.com", "http://localhost:8123"], "username": "george", "totp": true,
                       "loopback": true, "readable": true, "typed": "not on its loopback site: not granted readable"}),
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
            ("GITHUB_TOKEN", "the browser never types one"),
            ("PLAIN.code", "no TOTP seed"),
            ("GITHUB_PASSKEY", "signs in by itself"),
            ("GITHUB_PASSKEY.password", "signs in by itself"),
            ("NOBODY.password", "no secret named NOBODY"),
        ] {
            let error = secrets.resolve(reference).err().unwrap_or_default();
            assert!(error.contains(expect), "{reference}: {error}");
            assert!(!error.contains("battery"), "{reference}: {error}");
            assert!(!error.contains("notarealtoken"), "{reference}: {error}");
        }
    }

    #[test]
    fn the_codes_a_login_could_show_now_are_redacted() {
        let secrets = Secrets::default();
        secrets
            .replace(typed(json!({
                "GITHUB": {"kind": "login", "sites": ["https://github.com"], "username": "george",
                           "password": "correct horse battery", "totp": RFC_SEED},
            })))
            .unwrap();
        // RFC 6238, appendix B: 287082 at T = 59, the step before is 755224.
        let shown = "code 287082, before it 755224, in a longer number 12870823";
        assert_eq!(
            secrets.redact_at(shown, 59),
            "code [redacted GITHUB.code], before it [redacted GITHUB.code], in a longer number 12870823"
        );
        assert_eq!(secrets.redact_at("287082", 59), "[redacted GITHUB.code]");
        // Long after, those digits are just a number.
        assert_eq!(secrets.redact_at(shown, 59 + 3600), shown);
    }

    #[test]
    fn a_passkeys_site_is_its_rp_id_on_any_port() {
        let pages = |rp_id: &str, page: &str| {
            let page = Origin::of_page(page).unwrap();
            Origin::relying_party(rp_id)
                .iter()
                .any(|site| site.allows(&page))
        };
        for page in [
            "https://github.com/login",
            "https://github.com:8443/",
            "https://login.github.com/sso",
        ] {
            assert!(pages("github.com", page), "{page}");
        }
        for page in [
            "http://github.com/",
            "https://github.com.evil.example/",
            "https://evilgithub.com/",
        ] {
            assert!(!pages("github.com", page), "{page}");
        }
        assert!(pages("localhost", "http://localhost:8600/passkey"));
        assert!(pages("localhost", "https://localhost/"));
        assert!(!pages("localhost", "http://127.0.0.1:8600/"));
        assert_eq!(
            Origin::relying_party("github.com")[0].to_string(),
            "https://github.com"
        );
        assert!(Origin::relying_party("not a host").is_empty());
    }

    #[test]
    fn loopback_in_any_spelling_and_other_keys_are_readable() {
        for host in [
            "localhost",
            "app.localhost",
            "127.0.0.1",
            "127.1",
            "0x7f.1",
            "2130706433",
            "0177.0.0.1",
            "127.255.1.2",
            "0.0.0.0",
            "[::1]",
            "[::ffff:127.0.0.1]",
            "[::]",
        ] {
            assert!(is_loopback_host(host), "{host}");
        }
        for host in [
            "example.com",
            "128.0.0.1",
            "10.0.0.1",
            "localhost.example",
            "[2001:db8::1]",
            "1.2.3.4.5",
        ] {
            assert!(!is_loopback_host(host), "{host}");
        }
        assert!(
            Origin::site("http://0x7f000001:8000")
                .unwrap()
                .is_loopback()
        );
        assert!(!is_other(&[
            "https://api.github.com".to_owned(),
            "https://github.com".to_owned()
        ]));
        assert!(!is_other(&["https://api.anthropic.com".to_owned()]));
        assert!(is_other(&[
            "https://api.github.com".to_owned(),
            "https://api.openai.com".to_owned()
        ]));
        assert!(is_other(&["https://example.com".to_owned()]));
        let secrets = Secrets::default();
        secrets
            .replace(typed(json!({
                "LOOP": {"kind": "login", "sites": ["http://127.1:9000"], "username": "u", "password": "p4ssw0rd-long"},
                "PK": {"kind": "passkey", "rpId": "localhost", "credentialId": "AQID",
                       "privateKey": "MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgB9aEvdD1k7n55Wu+sDtf5cUbUumdw/XyKmgddnpO1eihRANCAASw/VbAXHGTIuqCMysLtvnlfwqu+ZFr7EQiwzWmLuynjO5Z/X+o4fuRzcMX916P1LxoWt4Ywc5KEVBuTzUzjBVI"},
                "GH": {"kind": "login", "sites": ["https://github.com"], "username": "u", "password": "p4ssw0rd-long"},
            })))
            .unwrap();
        assert_eq!(
            secrets.readable_equivalent(),
            vec!["LOOP".to_owned(), "PK".to_owned()]
        );
    }

    #[test]
    fn a_login_is_typed_on_exactly_its_origins() {
        let site = Origin::site("https://github.com").unwrap();
        for (page, exact) in [
            ("https://github.com/login", true),
            ("https://GITHUB.com:443/settings", true),
            ("https://login.github.com/sso", false),
            ("https://github.com:8443/", false),
            ("http://github.com/", false),
        ] {
            assert_eq!(
                site.is_exactly(&Origin::of_page(page).unwrap()),
                exact,
                "{page}"
            );
        }
        let secrets = Secrets::default();
        secrets
            .replace(typed(json!({
                "GH": {"kind": "login", "sites": ["https://github.com"], "username": "u", "password": "p4ssw0rd-long"},
            })))
            .unwrap();
        assert_eq!(secrets.logins_for("https://github.com").len(), 1);
        assert!(
            secrets.logins_for("https://gist.github.com").is_empty(),
            "a subdomain is not offered it"
        );
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
    fn a_value_is_taken_out_in_every_spelling_a_url_a_form_or_html_gives_it() {
        let secrets = Secrets::default();
        let password = "hl canary/enc+5&k=9%<\"x";
        secrets
            .replace(typed(json!({
                "LOCAL": {"kind": "login", "sites": ["http://localhost:8000"], "username": "u", "password": password},
            })))
            .unwrap();
        let mark = "[redacted LOCAL.password]";
        let pct = |upper: bool| -> String {
            password
                .bytes()
                .map(|b| {
                    if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                        (b as char).to_string()
                    } else if upper {
                        format!("%{b:02X}")
                    } else {
                        format!("%{b:02x}")
                    }
                })
                .collect()
        };
        let form = pct(true).replace("%20", "+");
        let entities = password
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('"', "&quot;");
        let numeric: String = password
            .chars()
            .map(|c| format!("&#{};", c as u32))
            .collect();
        let hex: String = password
            .chars()
            .map(|c| format!("&#x{:X};", c as u32))
            .collect();
        let b64 = base64::engine::general_purpose::STANDARD.encode(password);
        let url_safe = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(password);
        let inside = base64::engine::general_purpose::STANDARD.encode(format!("user:{password}:x"));
        for spelled in [
            pct(true),
            pct(false),
            form,
            entities,
            numeric,
            hex,
            b64,
            url_safe,
        ] {
            let text = format!("http://localhost:8000/f1-done?q={spelled}&next=1");
            let out = secrets.redact(&text);
            assert!(out.contains(mark), "{spelled} -> {out}");
            assert!(!spelled_in(&out, password), "{out}");
        }
        assert!(
            secrets.redact(&inside).contains(mark),
            "inside a longer base64 text too"
        );
        assert!(spelled("a%20b", "a b") && !spelled("a-b", "a b"));
        assert_eq!(
            secrets.redact("q=hl+canary"),
            "q=hl+canary",
            "a part of it is not it"
        );
    }

    /// The probes' own check: any decoding of `text` holds `value`.
    fn spelled_in(text: &str, value: &str) -> bool {
        text.contains(value) || spelled(text, value)
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
