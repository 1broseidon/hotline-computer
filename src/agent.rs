//! Display metadata shared by the desktop and viewer. It grants no authority.
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use crate::secrets::Secrets;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Agent {
    pub name: String,
    pub task: String,
    pub last_action_ms: u64,
    /// The harness the agent connected through, when it is one with a mark
    /// to draw beside the name; empty for any other client.
    #[serde(default)]
    pub harness: String,
}

impl Default for Agent {
    fn default() -> Self {
        Self {
            name: "Agent".into(),
            task: String::new(),
            last_action_ms: 0,
            harness: String::new(),
        }
    }
}

/// Harnesses known by what their MCP client calls itself, and the name to
/// show. Most have a mark in `assets/harness`.
const HARNESSES: [(&str, &str); 11] = [
    ("opencode", "OpenCode"),
    ("claude", "Claude"),
    ("codex", "Codex"),
    ("cursor", "Cursor"),
    ("copilot", "Copilot"),
    ("windsurf", "Windsurf"),
    ("cline", "Cline"),
    ("roocode", "Roo Code"),
    ("kilocode", "Kilo Code"),
    ("zed", "Zed"),
    ("gemini", "Gemini"),
];

/// A whole word of what the client said, or what its words run together
/// begin with ("Roo Code"), so a short key such as `zed` does not match
/// inside another name.
fn says(said: &str, key: &str) -> bool {
    let said = said.to_ascii_lowercase();
    let words: Vec<&str> = said
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    words.contains(&key) || words.concat().starts_with(key)
}

/// Names the harness from the client's handshake: a known one by its own
/// name and mark, any other by the title it gave, else its name tidied.
pub fn harness(name: &str, title: Option<&str>) -> (&'static str, String) {
    let known = HARNESSES.iter().find(|(key, _)| {
        [Some(name), title]
            .into_iter()
            .flatten()
            .any(|said| says(said, key))
    });
    if let Some((key, label)) = known {
        return (key, (*label).into());
    }
    let label = match title.map(str::trim).filter(|t| !t.is_empty()) {
        Some(title) => title.to_owned(),
        None => {
            let mut name = name.trim();
            for suffix in ["-mcp-client", "_mcp_client", "-client", "_client", "-mcp"] {
                if let Some(rest) = name.strip_suffix(suffix) {
                    name = rest;
                    break;
                }
            }
            let words = name.replace(['-', '_'], " ");
            let mut chars = words.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
    };
    (
        "",
        if label.is_empty() {
            "Agent".into()
        } else {
            label
        },
    )
}

pub struct Identity {
    fixed_name: Option<String>,
    /// A holder header named the agent; the harness no longer does.
    named: AtomicBool,
    state: Mutex<Agent>,
    display: String,
    secrets: Secrets,
}

impl Identity {
    pub fn new(display: &str, secrets: Secrets) -> Self {
        Self::with_name(
            display,
            secrets,
            std::env::var("HOTLINE_COMPUTER_AGENT")
                .ok()
                .filter(|n| !n.trim().is_empty()),
        )
    }

    fn with_name(display: &str, secrets: Secrets, fixed_name: Option<String>) -> Self {
        let state = Agent {
            name: fixed_name.clone().unwrap_or_else(|| "Agent".into()),
            ..Agent::default()
        };
        Self {
            fixed_name,
            named: AtomicBool::new(false),
            state: Mutex::new(state),
            display: display.into(),
            secrets,
        }
    }

    fn clean(&self, state: &Agent) -> Agent {
        let label = |s: &str| {
            self.secrets
                .redact(&crate::jobs::plain(s))
                .chars()
                .filter(|c| !c.is_control())
                .take(128)
                .collect()
        };
        Agent {
            name: label(&state.name),
            task: label(&state.task),
            last_action_ms: state.last_action_ms,
            harness: state.harness.clone(),
        }
    }

    pub fn snapshot(&self) -> Agent {
        self.clean(&self.state.lock().unwrap())
    }

    fn update(&self, change: impl FnOnce(&mut Agent)) {
        let mut state = self.state.lock().unwrap();
        change(&mut state);
        // Publication stays under this lock so an older update cannot win a race.
        let _ = crate::x11::publish_agent(&self.display, &self.clean(&state));
    }

    pub fn holder(&self, holder: &str) {
        if self.fixed_name.is_none() && !holder.trim().is_empty() && holder != "anonymous" {
            self.named.store(true, Ordering::Relaxed);
            self.update(|state| state.name = holder.to_owned());
        }
    }

    /// What the MCP client called itself when it connected. Its mark always
    /// shows; its name shows unless the operator or a holder header chose one.
    pub fn client(&self, name: &str, title: Option<&str>) {
        let (harness, label) = harness(name, title);
        let rename = self.fixed_name.is_none() && !self.named.load(Ordering::Relaxed);
        {
            // Every tool call reports the client; publish only a change.
            let state = self.state.lock().unwrap();
            if state.harness == harness && (!rename || state.name == label) {
                return;
            }
            // Hotline's desk connects as `hotline` for its own errands (the
            // guide, logins, files) beside the agent it starts; those never
            // take the name from an agent that has one.
            if name.trim().eq_ignore_ascii_case("hotline")
                && (!state.harness.is_empty() || state.name != Agent::default().name)
            {
                return;
            }
        }
        self.update(|state| {
            state.harness = harness.to_owned();
            if rename {
                state.name = label;
            }
        });
    }

    pub fn task(&self, task: &str) {
        self.update(|state| state.task = task.to_owned());
    }
    pub fn acted(&self) {
        self.update(|state| state.last_action_ms = crate::jobs::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_tracks_the_last_named_holder_but_honors_an_override() {
        let identity = Identity::with_name("", Secrets::default(), None);
        assert_eq!(identity.snapshot().name, "Agent");
        identity.holder("Tad");
        identity.holder("anonymous");
        assert_eq!(identity.snapshot().name, "Tad");
        identity.holder("George");
        identity.task("running checks");
        identity.acted();
        let state = identity.snapshot();
        assert_eq!(
            (state.name.as_str(), state.task.as_str()),
            ("George", "running checks")
        );
        assert!(state.last_action_ms > 0);
        let fixed = Identity::with_name("", Secrets::default(), Some("Tad".into()));
        fixed.holder("George");
        assert_eq!(fixed.snapshot().name, "Tad");
    }

    #[test]
    fn a_known_harness_lends_its_name_and_mark_and_others_their_own_words() {
        assert_eq!(harness("claude-code", None), ("claude", "Claude".into()));
        assert_eq!(
            harness("codex-mcp-client", Some("Codex")),
            ("codex", "Codex".into())
        );
        assert_eq!(harness("cursor-vscode", None), ("cursor", "Cursor".into()));
        assert_eq!(harness("opencode", None), ("opencode", "OpenCode".into()));
        assert_eq!(
            harness("github-copilot-cli", None),
            ("copilot", "Copilot".into())
        );
        assert_eq!(harness("Roo Code", None), ("roocode", "Roo Code".into()));
        assert_eq!(harness("zed", Some("Zed")), ("zed", "Zed".into()));
        // A short key is a whole word, never a fragment of another name.
        assert_eq!(
            harness("authorized-client", None),
            ("", "Authorized".into())
        );
        assert_eq!(
            harness("gemini-cli-mcp-client", None),
            ("gemini", "Gemini".into())
        );
        assert_eq!(harness("warp-agent", None), ("", "Warp agent".into()));
        assert_eq!(harness("  ", None), ("", "Agent".into()));
    }

    #[test]
    fn the_harness_names_the_agent_until_a_holder_header_does() {
        let identity = Identity::with_name("", Secrets::default(), None);
        identity.client("claude-code", None);
        let state = identity.snapshot();
        assert_eq!(
            (state.name.as_str(), state.harness.as_str()),
            ("Claude", "claude")
        );
        identity.holder("Tad");
        identity.client("claude-code", None);
        let state = identity.snapshot();
        assert_eq!(
            (state.name.as_str(), state.harness.as_str()),
            ("Tad", "claude")
        );
        let desk = Identity::with_name("", Secrets::default(), None);
        desk.client("claude-code", Some("Claude Code"));
        desk.client("hotline", None);
        assert_eq!(
            desk.snapshot().name,
            "Claude",
            "a desk errand renamed the agent"
        );
        let alone = Identity::with_name("", Secrets::default(), None);
        alone.client("Hotline", None);
        assert_eq!(alone.snapshot().name, "Hotline");
        let fixed = Identity::with_name("", Secrets::default(), Some("Ops".into()));
        fixed.client("codex-mcp-client", Some("Codex"));
        assert_eq!(fixed.snapshot().name, "Ops");
        assert_eq!(fixed.snapshot().harness, "codex");
    }
}
