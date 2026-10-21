//! The computer's own WebAuthn authenticator. The browser extension hands
//! it each request a page makes (Chromium's webAuthenticationProxy, with the
//! page's origin), and it answers with a response signed here: a passkey's
//! private key is never in the browser, so no page and no DevTools client
//! can read one back.
//!
//! It is a platform authenticator that always verifies its user, as the
//! virtual one it replaces was: the person's consent is the desk's arming
//! and approval for a new passkey, and the grant itself for an existing one.

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::secrets::Passkey;

/// What a page hears when a request is refused: a DOMException's name and
/// a sentence.
#[derive(Debug, PartialEq)]
pub struct Refusal {
    pub name: &'static str,
    pub message: String,
}

fn refuse(name: &'static str, message: impl Into<String>) -> Refusal {
    Refusal {
        name,
        message: message.into(),
    }
}

/// User present, user verified, backup eligible, backed up: a synced
/// platform passkey, which is what a teammate's vault holds.
const FLAGS: u8 = 0x01 | 0x04 | 0x08 | 0x10;
const ATTESTED: u8 = 0x40;
/// ES256, the one algorithm every relying party accepts.
const ES256: i64 = -7;
/// The DER prefix of a P-256 public key's SubjectPublicKeyInfo.
const SPKI_P256: &[u8] = &[
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
    0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];

/// The page that asked: its origin, and whether it is a frame of another.
fn caller(details: &Value) -> Result<(String, bool), Refusal> {
    let page = &details["extensions"]["remoteDesktopClientOverride"];
    let origin = page["origin"].as_str().ok_or_else(|| {
        refuse(
            "NotAllowedError",
            "the request did not say which page made it",
        )
    })?;
    let same = page["sameOriginWithAncestors"].as_bool().unwrap_or(true);
    Ok((origin.to_owned(), !same))
}

/// The host of an origin, from a secure context only: HTTPS, or this
/// machine itself.
fn host_of(origin: &str) -> Result<String, Refusal> {
    let (scheme, rest) = origin
        .split_once("://")
        .ok_or_else(|| refuse("SecurityError", format!("{origin} is not an origin")))?;
    let authority = rest.split('/').next().unwrap_or_default();
    let host = if let Some(bracketed) = authority.strip_prefix('[') {
        bracketed.split(']').next().unwrap_or_default()
    } else {
        authority
            .rsplit_once(':')
            .map_or(authority, |(host, _)| host)
    }
    .to_ascii_lowercase();
    let local = host == "localhost" || host.ends_with(".localhost") || host == "127.0.0.1";
    if scheme == "https" || (scheme == "http" && local) {
        Ok(host)
    } else {
        Err(refuse(
            "SecurityError",
            format!("{origin} is not a secure context"),
        ))
    }
}

/// The relying party a request names, or the page's own host; it must be
/// that host or a domain the host is within.
fn relying_party(origin: &str, asked: Option<&str>) -> Result<String, Refusal> {
    let host = host_of(origin)?;
    let rp = asked
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| host.clone());
    if host == rp || host.ends_with(&format!(".{rp}")) {
        Ok(rp)
    } else {
        Err(refuse(
            "SecurityError",
            format!("{origin} cannot use passkeys for {rp}"),
        ))
    }
}

fn client_data(kind: &str, challenge: &str, origin: &str, cross_origin: bool) -> String {
    // Chromium's own key order, which some relying parties parse by hand.
    format!(
        r#"{{"type":{},"challenge":{},"origin":{},"crossOrigin":{cross_origin}}}"#,
        json!(kind),
        json!(challenge),
        json!(origin)
    )
}

/// The signature counter: seconds since the epoch, and always past the last
/// one this computer gave. A site keeps the highest count it has seen and
/// refuses one that does not climb, as a cloned key; Chromium's own
/// authenticator counted from zero again in every new browser, so any count
/// of a key's own would fall behind what the site already holds.
pub fn counter() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static LAST: AtomicU32 = AtomicU32::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            u32::try_from(since.as_secs()).unwrap_or(u32::MAX)
        });
    let mut last = LAST.load(Ordering::Relaxed);
    loop {
        let next = now.max(last.saturating_add(1));
        match LAST.compare_exchange(last, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(seen) => last = seen,
        }
    }
}

fn authenticator_data(rp_id: &str, flags: u8, attested: &[u8]) -> Vec<u8> {
    let mut data = Sha256::digest(rp_id.as_bytes()).to_vec();
    data.push(flags);
    data.extend_from_slice(&counter().to_be_bytes());
    data.extend_from_slice(attested);
    data
}

fn b64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn from_b64url(text: &str) -> Result<Vec<u8>, Refusal> {
    URL_SAFE_NO_PAD
        .decode(text.trim_end_matches('='))
        .map_err(|_| refuse("DataError", "a field of the request is not base64url"))
}

fn from_std(text: &str) -> Option<Vec<u8>> {
    STANDARD.decode(text).ok()
}

/// Signs a `navigator.credentials.get()` with a granted passkey for the site.
pub fn assert(details: &Value, keys: &[Passkey]) -> Result<Value, Refusal> {
    let (origin, cross_origin) = caller(details)?;
    let rp_id = relying_party(&origin, details["rpId"].as_str())?;
    let challenge = details["challenge"]
        .as_str()
        .ok_or_else(|| refuse("DataError", "the request has no challenge"))?;
    let allowed: Vec<Vec<u8>> = details["allowCredentials"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|c| c["id"].as_str())
                .filter_map(|id| from_b64url(id).ok())
                .collect()
        })
        .unwrap_or_default();
    let key = keys
        .iter()
        .filter(|key| key.rp_id == rp_id)
        .find(|key| {
            allowed.is_empty()
                || from_std(&key.credential_id).is_some_and(|id| allowed.contains(&id))
        })
        .ok_or_else(|| {
            refuse(
                "NotAllowedError",
                format!("no passkey for {rp_id} is granted to this computer"),
            )
        })?;
    let credential = from_std(&key.credential_id)
        .ok_or_else(|| refuse("UnknownError", "a granted passkey's id is not base64"))?;
    let pkcs8 = from_std(&key.private_key)
        .ok_or_else(|| refuse("UnknownError", "a granted passkey's key is not base64"))?;
    let client = client_data("webauthn.get", challenge, &origin, cross_origin);
    let data = authenticator_data(&rp_id, FLAGS, &[]);
    let random = SystemRandom::new();
    let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &pkcs8, &random)
        .map_err(|_| refuse("UnknownError", "a granted passkey's key is not a P-256 key"))?;
    let mut signed = data.clone();
    signed.extend_from_slice(&Sha256::digest(client.as_bytes()));
    let signature = pair
        .sign(&random, &signed)
        .map_err(|_| refuse("UnknownError", "signing failed"))?;
    let handle = key
        .user_handle
        .as_deref()
        .and_then(from_std)
        .map(|h| b64url(&h));
    Ok(json!({
        "id": b64url(&credential),
        "rawId": b64url(&credential),
        "type": "public-key",
        "authenticatorAttachment": "platform",
        "response": {
            "clientDataJSON": b64url(client.as_bytes()),
            "authenticatorData": b64url(&data),
            "signature": b64url(signature.as_ref()),
            "userHandle": handle,
        },
        "clientExtensionResults": {},
    }))
}

/// What a `navigator.credentials.create()` asks for, before anyone decides
/// whether it may.
#[derive(Debug)]
pub struct Creation {
    pub origin: String,
    pub cross_origin: bool,
    pub rp_id: String,
    pub rp_name: Option<String>,
    pub user_id: Vec<u8>,
    pub user_name: Option<String>,
    pub user_display_name: Option<String>,
    pub challenge: String,
    pub excluded: Vec<Vec<u8>>,
}

pub fn creation(details: &Value) -> Result<Creation, Refusal> {
    let (origin, cross_origin) = caller(details)?;
    let rp_id = relying_party(&origin, details["rp"]["id"].as_str())?;
    let es256 = details["pubKeyCredParams"]
        .as_array()
        .is_some_and(|params| params.iter().any(|p| p["alg"].as_i64() == Some(ES256)));
    if !es256 {
        return Err(refuse(
            "NotSupportedError",
            "this computer makes ES256 passkeys only",
        ));
    }
    let text = |value: &Value| value.as_str().filter(|s| !s.is_empty()).map(str::to_owned);
    Ok(Creation {
        rp_name: text(&details["rp"]["name"]),
        user_id: from_b64url(details["user"]["id"].as_str().unwrap_or_default())?,
        user_name: text(&details["user"]["name"]),
        user_display_name: text(&details["user"]["displayName"]),
        challenge: details["challenge"]
            .as_str()
            .ok_or_else(|| refuse("DataError", "the request has no challenge"))?
            .to_owned(),
        excluded: details["excludeCredentials"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|c| c["id"].as_str())
                    .filter_map(|id| from_b64url(id).ok())
                    .collect()
            })
            .unwrap_or_default(),
        origin,
        cross_origin,
        rp_id,
    })
}

/// A credential this computer already has that the site asked not to be
/// made twice.
pub fn excluded(creation: &Creation, keys: &[Passkey]) -> bool {
    keys.iter().any(|key| {
        key.rp_id == creation.rp_id
            && from_std(&key.credential_id).is_some_and(|id| creation.excluded.contains(&id))
    })
}

/// Makes the passkey: a new P-256 key, its credential for the vault, and
/// the response with `none` attestation.
pub fn mint(creation: &Creation) -> Result<(Passkey, Value), Refusal> {
    let random = SystemRandom::new();
    fn failed<E>(_: E) -> Refusal {
        refuse("UnknownError", "making the key failed")
    }
    let pkcs8 =
        EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &random).map_err(failed)?;
    let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &random)
        .map_err(failed)?;
    let point = pair.public_key().as_ref();
    let mut credential = [0u8; 16];
    random.fill(&mut credential).map_err(failed)?;
    let mut attested = vec![0u8; 16];
    attested.extend_from_slice(&(credential.len() as u16).to_be_bytes());
    attested.extend_from_slice(&credential);
    attested.extend_from_slice(&cose_key(point));
    let data = authenticator_data(&creation.rp_id, FLAGS | ATTESTED, &attested);
    let client = client_data(
        "webauthn.create",
        &creation.challenge,
        &creation.origin,
        creation.cross_origin,
    );
    let mut spki = SPKI_P256.to_vec();
    spki.extend_from_slice(point);
    let passkey = Passkey {
        rp_id: creation.rp_id.clone(),
        credential_id: STANDARD.encode(credential),
        private_key: STANDARD.encode(pkcs8.as_ref()),
        user_handle: Some(STANDARD.encode(&creation.user_id)),
        user_name: creation.user_name.clone(),
        user_display_name: creation.user_display_name.clone(),
    };
    let response = json!({
        "id": b64url(&credential),
        "rawId": b64url(&credential),
        "type": "public-key",
        "authenticatorAttachment": "platform",
        "response": {
            "clientDataJSON": b64url(client.as_bytes()),
            "attestationObject": b64url(&attestation(&data)),
            "authenticatorData": b64url(&data),
            "publicKey": b64url(&spki),
            "publicKeyAlgorithm": ES256,
            "transports": ["internal", "hybrid"],
        },
        "clientExtensionResults": {"credProps": {"rk": true}},
    });
    Ok((passkey, response))
}

// ---- the little CBOR WebAuthn needs ---------------------------------------

fn cbor_head(major: u8, length: usize, out: &mut Vec<u8>) {
    let major = major << 5;
    match length {
        0..=23 => out.push(major | length as u8),
        24..=0xff => out.extend_from_slice(&[major | 24, length as u8]),
        _ => {
            out.push(major | 25);
            out.extend_from_slice(&(length as u16).to_be_bytes());
        }
    }
}

fn cbor_text(text: &str, out: &mut Vec<u8>) {
    cbor_head(3, text.len(), out);
    out.extend_from_slice(text.as_bytes());
}

fn cbor_bytes(bytes: &[u8], out: &mut Vec<u8>) {
    cbor_head(2, bytes.len(), out);
    out.extend_from_slice(bytes);
}

/// An EC2 P-256 COSE key: kty 2, alg -7, crv 1, x, y.
fn cose_key(point: &[u8]) -> Vec<u8> {
    let mut out = vec![0xa5, 0x01, 0x02, 0x03, 0x26, 0x20, 0x01, 0x21];
    cbor_bytes(&point[1..33], &mut out);
    out.push(0x22);
    cbor_bytes(&point[33..65], &mut out);
    out
}

/// `{"fmt": "none", "attStmt": {}, "authData": ...}`.
fn attestation(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0xa3];
    cbor_text("fmt", &mut out);
    cbor_text("none", &mut out);
    cbor_text("attStmt", &mut out);
    out.push(0xa0);
    cbor_text("authData", &mut out);
    cbor_bytes(data, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{ECDSA_P256_SHA256_ASN1, UnparsedPublicKey};

    fn asked(origin: &str, extra: Value) -> Value {
        let mut details = json!({
            "challenge": "Y2hhbGxlbmdl",
            "extensions": {"remoteDesktopClientOverride": {"origin": origin, "sameOriginWithAncestors": true}},
        });
        details
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        details
    }

    fn made(origin: &str, rp: &str) -> (Passkey, Value) {
        let details = asked(
            origin,
            json!({
                "rp": {"id": rp, "name": "Example"},
                "user": {"id": "dXNlci0x", "name": "teammate", "displayName": "The teammate"},
                "pubKeyCredParams": [{"type": "public-key", "alg": -7}],
            }),
        );
        mint(&creation(&details).unwrap()).unwrap()
    }

    #[test]
    fn a_minted_passkey_signs_for_its_site_and_the_signature_verifies() {
        let (key, created) = made("https://login.example.com", "example.com");
        assert_eq!(key.rp_id, "example.com");
        let data = URL_SAFE_NO_PAD
            .decode(created["response"]["authenticatorData"].as_str().unwrap())
            .unwrap();
        assert_eq!(&data[..32], Sha256::digest(b"example.com").as_slice());
        assert_eq!(data[32], FLAGS | ATTESTED);
        let client: Value = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(created["response"]["clientDataJSON"].as_str().unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(client["type"], "webauthn.create");
        assert_eq!(client["origin"], "https://login.example.com");

        let got = assert(
            &asked("https://example.com", json!({"rpId": "example.com"})),
            std::slice::from_ref(&key),
        )
        .unwrap();
        assert_eq!(got["id"], created["id"]);
        assert_eq!(got["response"]["userHandle"], "dXNlci0x");
        let data = URL_SAFE_NO_PAD
            .decode(got["response"]["authenticatorData"].as_str().unwrap())
            .unwrap();
        let client = URL_SAFE_NO_PAD
            .decode(got["response"]["clientDataJSON"].as_str().unwrap())
            .unwrap();
        let signature = URL_SAFE_NO_PAD
            .decode(got["response"]["signature"].as_str().unwrap())
            .unwrap();
        let mut signed = data.clone();
        signed.extend_from_slice(&Sha256::digest(&client));
        let spki = URL_SAFE_NO_PAD
            .decode(created["response"]["publicKey"].as_str().unwrap())
            .unwrap();
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, &spki[SPKI_P256.len()..])
            .verify(&signed, &signature)
            .expect("the site can verify what this computer signed");
        assert_eq!(data[32], FLAGS);
    }

    #[test]
    fn every_signature_counts_past_what_a_site_has_seen() {
        let (key, _) = made("https://example.com", "example.com");
        let count = |got: &Value| {
            let data = URL_SAFE_NO_PAD
                .decode(got["response"]["authenticatorData"].as_str().unwrap())
                .unwrap();
            u32::from_be_bytes(data[33..37].try_into().unwrap())
        };
        let first = assert(
            &asked("https://example.com", json!({})),
            std::slice::from_ref(&key),
        )
        .unwrap();
        let second = assert(
            &asked("https://example.com", json!({})),
            std::slice::from_ref(&key),
        )
        .unwrap();
        // Past any count Chromium's authenticator reached, and climbing even
        // within one second.
        assert!(count(&first) > 1_700_000_000, "{}", count(&first));
        assert!(count(&second) > count(&first));
    }

    #[test]
    fn a_passkey_is_never_offered_to_another_site_or_an_insecure_page() {
        let (key, _) = made("https://example.com", "example.com");
        let other = assert(
            &asked("https://evil.test", json!({"rpId": "example.com"})),
            std::slice::from_ref(&key),
        );
        assert_eq!(other.unwrap_err().name, "SecurityError");
        let plain = assert(
            &asked("http://example.com", json!({})),
            std::slice::from_ref(&key),
        );
        assert_eq!(plain.unwrap_err().name, "SecurityError");
        let none = assert(
            &asked("https://other.example.org", json!({})),
            std::slice::from_ref(&key),
        );
        assert_eq!(none.unwrap_err().name, "NotAllowedError");
        let listed = assert(
            &asked(
                "https://example.com",
                json!({"allowCredentials": [{"type": "public-key", "id": "AAAA"}]}),
            ),
            &[key],
        );
        assert_eq!(listed.unwrap_err().name, "NotAllowedError");
        assert!(relying_party("http://localhost:8123", Some("localhost")).is_ok());
    }

    #[test]
    fn a_site_that_excludes_a_known_credential_or_wants_another_algorithm_is_told() {
        let (key, created) = made("https://example.com", "example.com");
        let again = creation(&asked(
            "https://example.com",
            json!({
                "rp": {"name": "Example"}, "user": {"id": "dXNlci0x"},
                "pubKeyCredParams": [{"type": "public-key", "alg": -7}],
                "excludeCredentials": [{"type": "public-key", "id": created["id"]}],
            }),
        ))
        .unwrap();
        assert!(excluded(&again, &[key]));
        let rsa = creation(&asked(
            "https://example.com",
            json!({
                "rp": {}, "user": {"id": "dXNlci0x"}, "pubKeyCredParams": [{"type": "public-key", "alg": -257}],
            }),
        ));
        assert_eq!(rsa.unwrap_err().name, "NotSupportedError");
    }

    #[test]
    fn the_cose_key_and_attestation_are_the_bytes_webauthn_expects() {
        let point: Vec<u8> = std::iter::once(4).chain(1..=64).collect();
        let key = cose_key(&point);
        assert_eq!(&key[..8], &[0xa5, 0x01, 0x02, 0x03, 0x26, 0x20, 0x01, 0x21]);
        assert_eq!(&key[8..10], &[0x58, 0x20]);
        assert_eq!(key.len(), 8 + 2 + 32 + 1 + 2 + 32);
        let object = attestation(&[7; 30]);
        assert_eq!(&object[..5], &[0xa3, 0x63, b'f', b'm', b't']);
        assert!(object.ends_with(&[&[0x58, 30][..], &[7; 30]].concat()));
    }
}
