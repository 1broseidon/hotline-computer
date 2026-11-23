//! `PUT /sessions/{name}` and `DELETE /sessions/{name}`: the person's shared
//! sign-ins, carried into the signed-in browser and taken out again.
//!
//! Use Cookies sends `{sites, cookies, storage}`: the sites become signed-in
//! sites, so the agent's calls for them come here, and the cookies and site
//! storage go into the signed-in browser and nowhere else; no file is written
//! in the agent's home. Removing a share (`{domains?}`, all of it when none
//! are named) clears those sites' cookies and data and closes their tabs.
//! Bearer-only, like `/secrets`: the desk's act for the person.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Signin;
use crate::App;
use crate::secrets::Origin;

#[derive(Deserialize)]
struct Share {
    sites: Vec<String>,
    #[serde(default)]
    cookies: Vec<Value>,
    /// Site storage by origin: `{"https://site": {"key": "value"}}`.
    #[serde(default)]
    storage: serde_json::Map<String, Value>,
}

#[derive(Deserialize, Default)]
struct Unshare {
    #[serde(default)]
    domains: Vec<String>,
}

fn refuse(status: StatusCode, error: impl Into<String>) -> Response {
    (status, Json(json!({"error": error.into()}))).into_response()
}

/// A shared site as the desk names it: a host, which is its https origin,
/// or an origin.
pub(super) fn site(name: &str) -> Result<Origin, String> {
    let name = name.trim().trim_start_matches('.');
    if name.contains("://") {
        Origin::site(name)
    } else {
        Origin::site(&format!("https://{name}"))
    }
}

/// The answer of a computer that keeps no signed-in browser.
fn no_signin() -> Response {
    refuse(
        StatusCode::NOT_FOUND,
        "this computer has no signed-in browser; it takes shared sign-ins as saved logins",
    )
}

pub async fn put(State(app): State<App>, Path(name): Path<String>, body: String) -> Response {
    let Some(signin) = app.signin.clone() else {
        return no_signin();
    };
    if let Err(error) = crate::tools::state::valid_name(&name) {
        return refuse(StatusCode::BAD_REQUEST, error);
    }
    let share: Share = match serde_json::from_str(&body) {
        Ok(share) => share,
        Err(error) => {
            return refuse(
                StatusCode::BAD_REQUEST,
                format!("the body is not a share: {error}"),
            );
        }
    };
    let sites = match share
        .sites
        .iter()
        .map(|name| site(name))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(sites) if !sites.is_empty() => sites,
        Ok(_) => return refuse(StatusCode::BAD_REQUEST, "a share names at least one site"),
        Err(error) => return refuse(StatusCode::BAD_REQUEST, error),
    };
    match signin
        .share(&name, sites.clone(), share.cookies, share.storage)
        .await
    {
        Ok((cookies, storage)) => Json(json!({
            "name": name,
            "sites": sites.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "cookies": cookies,
            "storage": storage,
        }))
        .into_response(),
        Err(error) => refuse(StatusCode::SERVICE_UNAVAILABLE, error),
    }
}

pub async fn delete(State(app): State<App>, Path(name): Path<String>, body: String) -> Response {
    let Some(signin) = app.signin.clone() else {
        return no_signin();
    };
    let asked: Unshare = if body.trim().is_empty() {
        Unshare::default()
    } else {
        match serde_json::from_str(&body) {
            Ok(asked) => asked,
            Err(error) => {
                return refuse(
                    StatusCode::BAD_REQUEST,
                    format!("the body is not a removal: {error}"),
                );
            }
        }
    };
    let domains = match asked
        .domains
        .iter()
        .map(|name| site(name))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(domains) => domains,
        Err(error) => return refuse(StatusCode::BAD_REQUEST, error),
    };
    match signin.unshare(&name, &domains).await {
        Ok(None) => refuse(StatusCode::NOT_FOUND, format!("no shared sign-in {name:?}")),
        Ok(Some((gone, cookies, tabs))) => {
            // What still grants a signed-out site: it must sign in again
            // (a share is shared again by the desk, a login is typed again).
            let again: Vec<String> = signin.granting(&gone);
            Json(json!({
                "name": name,
                "domains": gone.iter().map(ToString::to_string).collect::<Vec<_>>(),
                "forgotten": cookies,
                "closed": tabs,
                "signInAgain": again,
            }))
            .into_response()
        }
        Err(error) => refuse(StatusCode::SERVICE_UNAVAILABLE, error),
    }
}

impl Signin {
    /// The logins and shares that still name one of `sites`.
    pub fn granting(&self, sites: &[Origin]) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for site in sites {
            let url = format!("{site}/");
            for name in self.names_for(&url).split(", ").filter(|n| !n.is_empty()) {
                if !names.iter().any(|known| known == name) {
                    names.push(name.to_owned());
                }
            }
        }
        names
    }

    /// Takes a share in: its sites open here from now on, and its cookies
    /// and storage are loaded. Cookies for any other site are left out.
    /// Answers how many cookies and storage entries went in.
    pub async fn share(
        &self,
        name: &str,
        sites: Vec<Origin>,
        cookies: Vec<Value>,
        storage: serde_json::Map<String, Value>,
    ) -> Result<(usize, usize), String> {
        self.0
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(name.to_owned(), sites.clone());
        let running = self.browser().await?;
        let cookies: Vec<Value> = cookies
            .iter()
            .filter(|cookie| super::cookie_within(cookie, &sites))
            .map(super::cookie_param)
            .collect();
        if !cookies.is_empty() {
            running
                .cdp
                .call("Storage.setCookies", json!({"cookies": cookies}))
                .await?;
        }
        let mut stored = 0;
        let entries: Vec<(String, String, String)> = storage
            .iter()
            .filter(|(origin, _)| super::lock::signed_in(origin, &sites))
            .flat_map(|(origin, items)| {
                items
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter_map(|(key, value)| {
                        Some((origin.clone(), key.clone(), value.as_str()?.to_owned()))
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        if !entries.is_empty() {
            let tab = self.current(&running).await?;
            let _ = running
                .cdp
                .on(&tab.session, "DOMStorage.enable", json!({}))
                .await;
            for (origin, key, value) in entries {
                let set = running
                    .cdp
                    .on(
                        &tab.session,
                        "DOMStorage.setDOMStorageItem",
                        json!({"storageId": {"securityOrigin": origin, "isLocalStorage": true},
                               "key": key, "value": value}),
                    )
                    .await;
                if set.is_ok() {
                    stored += 1;
                }
            }
        }
        Ok((cookies.len(), stored))
    }

    /// Takes a share out, whole or for `domains`: those sites are signed out,
    /// even where a login or another share still covers them. `None` when
    /// there is no such share. Answers the sites that went, and how many cookies
    /// and tabs went with them.
    pub async fn unshare(
        &self,
        name: &str,
        domains: &[Origin],
    ) -> Result<Option<(Vec<Origin>, usize, usize)>, String> {
        let gone: Vec<Origin> = {
            let mut sessions = self
                .0
                .sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(sites) = sessions.get_mut(name) else {
                return Ok(None);
            };
            let gone: Vec<Origin> = if domains.is_empty() {
                std::mem::take(sites)
            } else {
                let gone = sites
                    .iter()
                    .filter(|site| domains.contains(site))
                    .cloned()
                    .collect();
                sites.retain(|site| !domains.contains(site));
                gone
            };
            if sites.is_empty() {
                sessions.remove(name);
            }
            gone
        };
        // Signed out whatever else still names them: the revoked share's
        // session must not stay usable. A login or another share on the
        // same site signs in again (`again`, for the desk to share anew).
        let forgotten = self.forget(&gone).await;
        self.settle_revocations().await;
        let (cookies, tabs) = forgotten?;
        Ok(Some((gone, cookies, tabs)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shared_site_is_a_host_or_an_origin() {
        assert_eq!(
            site("github.com").unwrap().to_string(),
            "https://github.com"
        );
        assert_eq!(
            site(".github.com").unwrap().to_string(),
            "https://github.com"
        );
        assert_eq!(
            site("http://localhost:8123").unwrap().to_string(),
            "http://localhost:8123"
        );
        assert!(site("http://example.com").is_err());
    }
}
