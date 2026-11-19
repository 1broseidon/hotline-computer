//! The bound variables and the stand-ins made for each job.

use std::collections::{BTreeMap, HashMap};
use std::net::IpAddr;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ring::rand::{SecureRandom, SystemRandom};

use super::scrub::Pairs;
use super::{Use, policy};

/// A protected variable as delivered: its name, its value and its `use`.
#[derive(Clone, Debug)]
pub struct Binding {
    pub name: String,
    pub value: String,
    pub use_: Use,
}

const MAX_NAME_CHARS: usize = 64;
const MIN_VALUE_CHARS: usize = 8;
const MAX_VALUE_BYTES: usize = 64 * 1024;
const MAX_ORIGINS: usize = 16;
const MAX_SCHEMES: usize = 8;
const MAX_PREFIX_CHARS: usize = 32;
const MAX_STAND_IN_CHARS: usize = 256;
/// The random part of a stand-in is never shorter than this, whatever shape
/// is asked for: about 119 bits. A shorter key's stand-in is longer than it.
const MIN_RANDOM_CHARS: usize = 20;
const DEFAULT_PREFIX: &str = "hl_";
const DEFAULT_RANDOM_CHARS: usize = 40;
/// Headers that frame or route the request: a value never goes in one.
const FRAMING: [&str; 12] = [
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "keep-alive",
    "upgrade",
    "te",
    "trailer",
    "expect",
    "proxy-authorization",
    "proxy-connection",
    "accept-encoding",
];

#[derive(Default)]
pub(super) struct Table {
    generation: Option<u64>,
    bound: BTreeMap<String, Bound>,
    /// Stand-in to the job it was made for and the name it stands in for.
    issued: HashMap<String, Issued>,
}

struct Bound {
    value: String,
    origins: Vec<String>,
    /// Lowercase.
    header: String,
    schemes: Vec<String>,
    prefix: String,
    random_chars: usize,
    /// The desk's refused routes, or `None` for the built-in list.
    refuse: Option<Vec<Vec<String>>>,
}

struct Issued {
    job: String,
    name: String,
}

/// What a request header becomes, and what to write back over in the reply.
pub(super) struct Swapped {
    pub header: String,
    /// `(sent, job's spelling)`.
    pub scrub: Pairs,
    /// The variables whose values went in.
    pub names: Vec<String>,
}

impl Table {
    pub fn deliver(&mut self, generation: u64, bindings: Vec<Binding>) -> Result<(), String> {
        if let Some(current) = self.generation
            && generation < current
        {
            return Err(format!(
                "generation {generation} is older than this computer's {current}"
            ));
        }
        let mut bound = BTreeMap::new();
        for binding in bindings {
            let name = binding.name.clone();
            let checked = check(binding)?;
            if bound.insert(name.clone(), checked).is_some() {
                return Err(format!("{name} is delivered twice"));
            }
        }
        self.generation = Some(generation);
        self.bound = bound;
        let bound = &self.bound;
        self.issued
            .retain(|_, issued| bound.contains_key(&issued.name));
        Ok(())
    }

    pub fn begin(&mut self, job: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for (name, bound) in &self.bound {
            let existing = self
                .issued
                .iter()
                .find(|(_, issued)| issued.job == job && &issued.name == name)
                .map(|(stand_in, _)| stand_in.clone());
            let stand_in = match existing {
                Some(stand_in) => stand_in,
                None => loop {
                    let candidate = mint(&bound.prefix, bound.random_chars);
                    if !self.issued.contains_key(&candidate) {
                        self.issued.insert(
                            candidate.clone(),
                            Issued {
                                job: job.to_owned(),
                                name: name.clone(),
                            },
                        );
                        break candidate;
                    }
                },
            };
            out.push((name.clone(), stand_in));
        }
        out
    }

    pub fn end(&mut self, job: &str) {
        self.issued.retain(|_, issued| issued.job != job);
    }

    pub fn is_bound(&self, origin: &str) -> bool {
        self.bound
            .values()
            .any(|bound| bound.origins.iter().any(|o| o == origin))
    }

    /// The lowercase header names a value may go in for `origin`.
    pub fn headers(&self, origin: &str) -> Vec<String> {
        let mut headers: Vec<String> = self
            .bound
            .values()
            .filter(|bound| bound.origins.iter().any(|o| o == origin))
            .map(|bound| bound.header.clone())
            .collect();
        headers.sort();
        headers.dedup();
        headers
    }

    /// Every bound value, and what it reads as when a reply carries it back
    /// without having been sent in this request.
    pub fn redactions(&self) -> Pairs {
        self.bound
            .iter()
            .map(|(name, bound)| {
                (
                    bound.value.as_bytes().to_vec(),
                    format!("[redacted {name}]").into_bytes(),
                )
            })
            .collect()
    }

    /// `value` of request header `header` (lowercase) on `origin`, with each
    /// live stand-in bound there replaced by its value, if any was.
    pub fn swap(&self, origin: &str, header: &str, value: &str) -> Option<Swapped> {
        let mut current = value.to_owned();
        let mut scrub = Vec::new();
        let mut names = Vec::new();
        for (name, bound) in &self.bound {
            if bound.header != header || !bound.origins.iter().any(|o| o == origin) {
                continue;
            }
            let live = |candidate: &[u8]| {
                std::str::from_utf8(candidate)
                    .ok()
                    .and_then(|candidate| self.issued.get(candidate))
                    .is_some_and(|issued| &issued.name == name)
            };
            if let Some((next, stand_in)) = swap_one(&current, bound, live) {
                scrub.push((bound.value.as_bytes().to_vec(), stand_in));
                names.push(name.clone());
                current = next;
            }
        }
        if scrub.is_empty() {
            return None;
        }
        scrub.push((current.as_bytes().to_vec(), value.as_bytes().to_vec()));
        Some(Swapped {
            header: current,
            scrub,
            names,
        })
    }

    /// Whether any of `names`' values may not go to `path` on `origin`.
    pub fn refused(&self, names: &[String], origin: &str, path: &str) -> bool {
        names.iter().any(|name| match self.bound.get(name) {
            Some(Bound {
                refuse: Some(routes),
                ..
            }) => policy::listed(routes, path),
            _ => policy::refused(origin, path),
        })
    }
}

/// One binding's swap in a header value: the new value and the stand-in it
/// replaced.
fn swap_one(value: &str, bound: &Bound, live: impl Fn(&[u8]) -> bool) -> Option<(String, Vec<u8>)> {
    if bound.schemes.is_empty() {
        let candidate = value.trim();
        return live(candidate.as_bytes())
            .then(|| (bound.value.clone(), candidate.as_bytes().to_vec()));
    }
    let (scheme, credential) = value.trim().split_once(' ')?;
    let credential = credential.trim_start();
    if !bound.schemes.iter().any(|s| s.eq_ignore_ascii_case(scheme)) {
        return None;
    }
    if !scheme.eq_ignore_ascii_case("basic") {
        return live(credential.as_bytes()).then(|| {
            (
                format!("{scheme} {}", bound.value),
                credential.as_bytes().to_vec(),
            )
        });
    }
    let decoded = STANDARD.decode(credential).ok()?;
    let colon = decoded.iter().position(|&b| b == b':')?;
    let (user, pass) = (&decoded[..colon], &decoded[colon + 1..]);
    let (user, pass, stand_in) = if live(user) {
        (bound.value.as_bytes(), pass, user)
    } else if live(pass) {
        (user, bound.value.as_bytes(), pass)
    } else {
        return None;
    };
    let mut joined = user.to_vec();
    joined.push(b':');
    joined.extend_from_slice(pass);
    Some((
        format!("{scheme} {}", STANDARD.encode(joined)),
        stand_in.to_vec(),
    ))
}

fn mint(prefix: &str, random_chars: usize) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let random = SystemRandom::new();
    let mut out = String::with_capacity(prefix.len() + random_chars);
    out.push_str(prefix);
    let mut byte = [0u8; 1];
    while out.len() < prefix.len() + random_chars {
        random.fill(&mut byte).expect("system randomness");
        // 248 is the largest multiple of 62 a byte holds: no bias.
        if byte[0] < 248 {
            out.push(ALPHABET[usize::from(byte[0]) % ALPHABET.len()] as char);
        }
    }
    out
}

fn check(binding: Binding) -> Result<Bound, String> {
    let Binding { name, value, use_ } = binding;
    let name_ok = !name.is_empty()
        && name.len() <= MAX_NAME_CHARS
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && !name.as_bytes()[0].is_ascii_digit();
    if !name_ok {
        return Err(format!("{name:?} is not a variable name"));
    }
    if value.len() < MIN_VALUE_CHARS || value.len() > MAX_VALUE_BYTES {
        return Err(format!(
            "{name}: a protected value is {MIN_VALUE_CHARS} to {MAX_VALUE_BYTES} bytes"
        ));
    }
    if !value.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(format!(
            "{name}: a protected value is printable ASCII with no spaces"
        ));
    }
    if use_.origins.is_empty() || use_.origins.len() > MAX_ORIGINS {
        return Err(format!("{name}: use names 1 to {MAX_ORIGINS} origins"));
    }
    let mut origins = Vec::new();
    for origin in &use_.origins {
        let normal = parse_origin(origin)
            .ok_or_else(|| format!("{name}: {origin:?} is not an https://host[:port] origin"))?;
        if !origins.contains(&normal) {
            origins.push(normal);
        }
    }
    let header = use_.header.to_ascii_lowercase();
    if header.is_empty() || !header.bytes().all(is_token) || FRAMING.contains(&header.as_str()) {
        return Err(format!("{name}: {:?} cannot carry a value", use_.header));
    }
    if use_.schemes.len() > MAX_SCHEMES
        || use_
            .schemes
            .iter()
            .any(|s| s.is_empty() || s.len() > 32 || !s.bytes().all(is_token))
    {
        return Err(format!(
            "{name}: a scheme is one word, at most {MAX_SCHEMES}"
        ));
    }
    // The shape is a courtesy to programs that check a token's look; it
    // never costs randomness, and never refuses a delivery.
    let (prefix, random_chars) = match &use_.stand_in {
        None => (DEFAULT_PREFIX.to_owned(), DEFAULT_RANDOM_CHARS),
        Some(shape) => {
            let prefix_ok = shape.prefix.len() <= MAX_PREFIX_CHARS
                && shape
                    .prefix
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
            let prefix = if prefix_ok {
                shape.prefix.clone()
            } else {
                DEFAULT_PREFIX.to_owned()
            };
            let random_chars = shape
                .length
                .saturating_sub(prefix.len())
                .clamp(MIN_RANDOM_CHARS, MAX_STAND_IN_CHARS - prefix.len());
            (prefix, random_chars)
        }
    };
    let refuse = match &use_.refuse {
        None => None,
        Some(routes) => Some(policy::parse(routes).map_err(|e| format!("{name}: {e}"))?),
    };
    Ok(Bound {
        refuse,
        value,
        origins,
        header,
        schemes: use_.schemes.clone(),
        prefix,
        random_chars,
    })
}

fn is_token(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

/// `https://host[:port]` in the one spelling the proxy compares, or `None`.
pub(super) fn parse_origin(text: &str) -> Option<String> {
    let rest = text
        .get(..8)
        .filter(|scheme| scheme.eq_ignore_ascii_case("https://"))
        .map(|_| &text[8..])?;
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    if rest.contains(['/', '?', '#', '@']) {
        return None;
    }
    let (host, port) = parse_authority(rest, 443)?;
    Some(origin(&host, port))
}

/// The origin a `CONNECT host:port` reaches.
pub(super) fn origin(host: &str, port: u16) -> String {
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    if port == 443 {
        format!("https://{host}")
    } else {
        format!("https://{host}:{port}")
    }
}

/// `host[:port]` as a lowercase host (no brackets, no trailing dot) and a
/// port, or `None` if either is not one.
pub(super) fn parse_authority(text: &str, default_port: u16) -> Option<(String, u16)> {
    let (host, port) = if let Some(rest) = text.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = match after {
            "" => None,
            _ => Some(after.strip_prefix(':')?),
        };
        host.parse::<std::net::Ipv6Addr>().ok()?;
        (host.to_owned(), port)
    } else {
        match text.rsplit_once(':') {
            Some((host, port)) => (host.to_owned(), Some(port)),
            None => (text.to_owned(), None),
        }
    };
    let port = match port {
        None => default_port,
        Some(port) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => {
            port.parse::<u16>().ok().filter(|&p| p != 0)?
        }
        Some(_) => return None,
    };
    let host = host.to_ascii_lowercase();
    let host = host.strip_suffix('.').unwrap_or(&host).to_owned();
    let name_ok = !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        });
    if !name_ok && host.parse::<IpAddr>().is_err() {
        return None;
    }
    Some((host, port))
}
