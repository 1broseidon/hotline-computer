//! Where the signed-in browser's tabs may go.
//!
//! Every top-level document request is paused (CDP `Fetch`, document stage)
//! and judged here before it leaves: a page of a signed-in site stays; a
//! plain link elsewhere opens in the general browser instead; a hop elsewhere
//! that carries a sign-in's proof — an OAuth `code`, `state` or token, or a
//! form posted off the site — goes nowhere, because the general browser is
//! the agent's own and a code there is a session for whoever holds it.

use crate::secrets::Origin;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hop {
    /// A page of a signed-in site, or the blank page: it loads here.
    Stay,
    /// A plain page elsewhere: it opens in the general browser instead.
    General,
    /// Neither: refused, with the reason the agent is told.
    Blocked(String),
}

/// Query and fragment names that carry a sign-in's proof.
const CARRIERS: [&str; 10] = [
    "code",
    "state",
    "token",
    "access_token",
    "id_token",
    "refresh_token",
    "oauth_token",
    "oauth_verifier",
    "samlresponse",
    "ticket",
];

/// Whether a page of `url` belongs to one of `sites`.
pub fn signed_in(url: &str, sites: &[Origin]) -> bool {
    Origin::of_page(url).is_ok_and(|page| sites.iter().any(|site| site.allows(&page)))
}

/// Judges one top-level navigation: `url` with its fragment, the method,
/// against the signed-in sites.
pub fn judge(url: &str, method: &str, sites: &[Origin]) -> Hop {
    let lower = url.to_ascii_lowercase();
    if lower == "about:blank" || lower.starts_with("about:blank#") {
        return Hop::Stay;
    }
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        let scheme = lower.split(':').next().unwrap_or_default();
        return Hop::Blocked(format!(
            "the signed-in browser opens web pages only, not {scheme}: addresses"
        ));
    }
    if signed_in(url, sites) {
        return Hop::Stay;
    }
    let host = crate::bridge::host_of(url);
    if !method.eq_ignore_ascii_case("GET") {
        return Hop::Blocked(format!(
            "a form sent from a signed-in site to {host} was stopped: it may carry the sign-in"
        ));
    }
    if let Some(name) = carrier(url) {
        return Hop::Blocked(format!(
            "a sign-in hand-off to {host} was stopped: it carried {name}, which is a sign-in's proof"
        ));
    }
    Hop::General
}

/// The first parameter of `url`'s query or fragment that carries a proof.
fn carrier(url: &str) -> Option<String> {
    let after = url.split_once(['?', '#'])?.1;
    after
        .split(['&', '?', '#', ';'])
        .filter_map(|pair| pair.split('=').next())
        .map(|name| name.to_ascii_lowercase())
        .find(|name| CARRIERS.contains(&name.as_str()) || name.ends_with("_token"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sites() -> Vec<Origin> {
        vec![
            Origin::site("https://github.com").unwrap(),
            Origin::site("http://localhost:8123").unwrap(),
        ]
    }

    #[test]
    fn a_signed_in_sites_pages_stay_and_plain_links_go_to_the_general_browser() {
        let sites = sites();
        assert_eq!(judge("https://github.com/login", "GET", &sites), Hop::Stay);
        assert_eq!(
            judge("https://gist.github.com/x?code=1", "GET", &sites),
            Hop::Stay
        );
        assert_eq!(judge("http://localhost:8123/a", "POST", &sites), Hop::Stay);
        assert_eq!(judge("about:blank", "GET", &sites), Hop::Stay);
        assert_eq!(
            judge("https://docs.rs/serde?query=state%20machine", "GET", &sites),
            Hop::General
        );
        assert_eq!(
            judge("https://github.com.evil.example/", "GET", &sites),
            Hop::General
        );
        assert_eq!(
            judge("http://localhost:9999/", "GET", &sites),
            Hop::General,
            "another port is another site"
        );
    }

    #[test]
    fn a_hop_carrying_a_sign_in_proof_or_a_script_address_is_blocked() {
        let sites = sites();
        for url in [
            "https://app.example/callback?code=abc&state=xyz",
            "https://app.example/cb?state=only",
            "https://app.example/cb#access_token=t&token_type=bearer",
            "https://app.example/cb?x=1&id_token=jwt",
            "https://app.example/cb?session_token=t",
            "https://app.example/saml?SAMLResponse=x",
            "https://app.example/cb?Code=ABC",
        ] {
            assert!(
                matches!(judge(url, "GET", &sites), Hop::Blocked(_)),
                "{url}"
            );
        }
        assert!(matches!(
            judge("https://app.example/cb", "POST", &sites),
            Hop::Blocked(_)
        ));
        for url in [
            "javascript:alert(1)",
            "chrome://settings",
            "devtools://devtools/bundled/inspector.html",
            "file:///etc/passwd",
            "data:text/html,hi",
        ] {
            assert!(
                matches!(judge(url, "GET", &sites), Hop::Blocked(_)),
                "{url}"
            );
        }
    }
}
