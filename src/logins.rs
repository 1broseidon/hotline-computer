//! Taking a saved login back out of the browser: the operator's door.
//!
//! The desk's cookie import writes a saved login into `.hotline/logins` and
//! loads it, the way `state login_load` would. Loading puts cookies into
//! the browser; nothing before this took them out again, so a person who
//! brought a site over had no way to undo it short of removing the computer.
//! `DELETE /logins/{name}` is that way: every cookie in the browser for the
//! named domains — or for every domain the saved login carries, when none
//! are named — is deleted, and the saved document loses them too, or is
//! removed when nothing of it is left. Bearer-only, like `/secrets`: it is
//! the desk's act on the person's behalf, not a tool the agent has.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::App;

/// The body of `DELETE /logins/{name}`, optional: which of the login's
/// domains to forget. None named means the whole login.
#[derive(Deserialize, Default)]
pub struct Forget {
    #[serde(default)]
    domains: Vec<String>,
}

/// A cookie domain as a site name: lower case, without the leading dot a
/// host-wide cookie carries.
pub fn site_of(domain: &str) -> String {
    domain.trim().trim_start_matches('.').to_ascii_lowercase()
}

/// Whether a cookie set for `domain` belongs to one of `sites`: the site
/// itself, or a host within it.
pub fn within(domain: &str, sites: &[String]) -> bool {
    let domain = site_of(domain);
    sites
        .iter()
        .any(|site| domain == *site || domain.ends_with(&format!(".{site}")))
}

fn refuse(status: StatusCode, error: String) -> Response {
    (status, Json(json!({"error": error}))).into_response()
}

/// `DELETE /logins/{name}` with an optional `{"domains": [...]}` body.
/// Answers what was forgotten: the domains, how many cookies the browser
/// dropped, and how many the saved login still carries.
pub async fn forget(State(app): State<App>, Path(name): Path<String>, body: String) -> Response {
    // The same rule `state login_save` applies, so the door names exactly
    // what the tool can have saved.
    if let Err(error) = crate::tools::state::valid_name(&name) {
        return refuse(StatusCode::BAD_REQUEST, error);
    }
    let asked: Forget = if body.trim().is_empty() {
        Forget::default()
    } else {
        match serde_json::from_str(&body) {
            Ok(asked) => asked,
            Err(error) => {
                return refuse(
                    StatusCode::BAD_REQUEST,
                    format!("the body is not a forget: {error}"),
                );
            }
        }
    };
    let path = app
        .config
        .home
        .join(".hotline/logins")
        .join(format!("{name}.json"));
    let saved: Option<Value> = match tokio::fs::read(&path).await {
        Ok(data) => match serde_json::from_slice(&data) {
            Ok(saved) => Some(saved),
            Err(error) => {
                return refuse(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("the saved login {name:?} cannot be read: {error}"),
                );
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return refuse(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("the saved login {name:?} cannot be read: {error}"),
            );
        }
    };
    let whole = asked.domains.is_empty();
    let mut sites: Vec<String> = if whole {
        saved
            .as_ref()
            .and_then(|saved| saved["cookies"].as_array())
            .map(|cookies| {
                cookies
                    .iter()
                    .filter_map(|cookie| cookie["domain"].as_str())
                    .map(site_of)
                    .collect()
            })
            .unwrap_or_default()
    } else {
        asked.domains.iter().map(|domain| site_of(domain)).collect()
    };
    sites.sort();
    sites.dedup();
    sites.retain(|site| !site.is_empty());
    if saved.is_none() && whole {
        return refuse(StatusCode::NOT_FOUND, format!("no saved login {name:?}"));
    }
    let forgotten = match app.browser.forget_cookies(&sites).await {
        Ok(forgotten) => forgotten,
        Err(error) => {
            return refuse(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("the browser did not drop the cookies: {error}"),
            );
        }
    };
    // The saved login loses what the browser lost, so a later load does not
    // bring it back; a login with nothing left is removed.
    let mut kept = 0;
    if let Some(mut saved) = saved {
        let remaining: Vec<Value> = if whole {
            Vec::new()
        } else {
            saved["cookies"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|cookie| !within(cookie["domain"].as_str().unwrap_or(""), &sites))
                .collect()
        };
        kept = remaining.len();
        let written = if remaining.is_empty() {
            tokio::fs::remove_file(&path).await
        } else {
            saved["cookies"] = Value::Array(remaining);
            match serde_json::to_vec(&saved) {
                Ok(data) => tokio::fs::write(&path, data).await,
                Err(error) => Err(std::io::Error::other(error)),
            }
        };
        if let Err(error) = written {
            return refuse(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("the saved login {name:?} could not be updated: {error}"),
            );
        }
    }
    Json(json!({"name": name, "domains": sites, "forgotten": forgotten, "kept": kept}))
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cookie_belongs_to_its_site_or_a_host_within_it() {
        let sites = vec!["github.com".to_owned(), "localhost".to_owned()];
        assert!(within(".github.com", &sites));
        assert!(within("github.com", &sites));
        assert!(within("api.github.com", &sites));
        assert!(within("LOCALHOST", &sites));
        assert!(!within("evilgithub.com", &sites));
        assert!(!within("github.com.evil.example", &sites));
        assert_eq!(site_of(" .Example.COM "), "example.com");
    }
}
