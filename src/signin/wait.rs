//! The `wait` tool on a signed-in tab: text on the page, looked for the way
//! `text` reads it, and never while the tab is withheld. A wait that spans a
//! sign-in resolves only once the tab shows a new document, and never answers
//! from the page that was being signed in to.

use std::time::{Duration, Instant};

use rmcp::model::ContentBlock;
use serde_json::Value;

use super::Signin;
use crate::tools::ToolResult;

impl Signin {
    /// `None` when the agent's browser calls are not for the signed-in
    /// browser, and the wait is the desktop's.
    pub async fn wait(&self, arguments: &Value) -> Option<ToolResult> {
        if !self.focused() {
            return None;
        }
        Some(self.wait_for(arguments).await)
    }

    async fn wait_for(&self, arguments: &Value) -> ToolResult {
        let wanted = arguments["text"].as_str().unwrap_or_default();
        if wanted.is_empty() {
            return Err("text is required".to_owned());
        }
        let timeout = arguments["timeout"]
            .as_u64()
            .unwrap_or(10)
            .min(crate::tools::CALL_BUDGET_MS / 1000);
        let deadline = Instant::now() + Duration::from_secs(timeout);
        let running = self.browser().await?;
        let mut hidden;
        loop {
            let tab = self.current(&running).await?;
            hidden = self.withheld_answer(&running.cdp, &tab.target).await;
            if hidden.is_none() {
                let body = self
                    .evaluate(
                        &running.cdp,
                        &tab.session,
                        "document.body ? document.body.innerText : ''",
                    )
                    .await
                    .unwrap_or_default();
                // Checked again after the read: a sign-in that began during
                // it keeps the page to itself.
                hidden = self.withheld_answer(&running.cdp, &tab.target).await;
                if hidden.is_none() && body.as_str().is_some_and(|body| body.contains(wanted)) {
                    return Ok(vec![ContentBlock::text(format!("found {wanted:?}"))]);
                }
            }
            if Instant::now() >= deadline {
                return Err(match hidden {
                    Some(refusal) => format!("text {wanted:?} was not looked for: {refusal}"),
                    None => format!("text {wanted:?} did not appear within {timeout}s"),
                });
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}
