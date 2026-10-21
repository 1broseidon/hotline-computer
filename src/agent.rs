//! Display metadata shared by the desktop and viewer. It grants no authority.
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::secrets::Secrets;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Agent {
    pub name: String,
    pub task: String,
    pub last_action_ms: u64,
}

impl Default for Agent {
    fn default() -> Self {
        Self {
            name: "Agent".into(),
            task: String::new(),
            last_action_ms: 0,
        }
    }
}

pub struct Identity {
    fixed_name: Option<String>,
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
            self.update(|state| state.name = holder.to_owned());
        }
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
}
