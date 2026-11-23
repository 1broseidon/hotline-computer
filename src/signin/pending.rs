//! `GET /signin/pending`: the sign-ins waiting on the person, for the desk to
//! show one quiet line that opens the site screen (`/sites/ws?site=`).
//!
//! A sign-in waits on the person when a secret was typed into a tab and, 30
//! seconds on, the browser still shows that same document: the broker could
//! not finish it (a captcha, a prompt, a code it does not have). The entry
//! goes when the tab shows another document, when the tab closes, or when
//! the login is revoked. A route of its own rather than an entry in
//! `/secrets/uses`, whose `kind` an older desk reads as passkey or login
//! only. Bearer-only, like `/secrets`.

use axum::Json;
use axum::extract::State;
use serde_json::{Value, json};

use super::{FINISH_AFTER_MS, Signin, Tab, document};
use crate::App;
use crate::secrets::Origin;

pub async fn list(State(app): State<App>) -> Json<Value> {
    let pending = match &app.signin {
        Some(signin) => signin.pending().await,
        None => Vec::new(),
    };
    Json(json!({"pending": pending}))
}

impl Signin {
    /// `{kind: "finish", name, origin, account, since}` for each sign-in
    /// waiting on the person, `since` in Unix milliseconds.
    pub async fn pending(&self) -> Vec<Value> {
        let running = self.0.running.lock().await.clone();
        let Some(running) = running.filter(|running| !running.cdp.is_closed()) else {
            return Vec::new();
        };
        let now = crate::jobs::now();
        let waiting: Vec<Tab> = self
            .lock_tabs()
            .list
            .iter()
            .filter(|tab| {
                tab.secret_doc.is_some()
                    && tab.secret_name.is_some()
                    && now.saturating_sub(tab.secret_at) > FINISH_AFTER_MS
            })
            .cloned()
            .collect();
        let catalog = self.0.secrets.catalog();
        let mut pending = Vec::new();
        for tab in waiting {
            let name = tab.secret_name.clone().unwrap_or_default();
            // A revoked login is nobody's to finish.
            let Some(login) = catalog
                .iter()
                .find(|entry| entry["name"] == name.as_str() && entry["kind"] == "login")
            else {
                continue;
            };
            // Still the document the secret went into, by the browser's word.
            if document(&running.cdp, &tab.session).await != tab.secret_doc {
                continue;
            }
            let origin = Origin::of_page(&tab.url)
                .map(|origin| origin.to_string())
                .unwrap_or_default();
            pending.push(json!({
                "kind": "finish",
                "name": name,
                "origin": origin,
                "account": login["username"],
                "since": tab.secret_at + FINISH_AFTER_MS,
            }));
        }
        pending
    }
}
