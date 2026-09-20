//! The secrets the person put in this computer.
//!
//! The desk delivers the whole set with `PUT /secrets`, bearer in a header,
//! each time it changes: when the computer starts, and again when the person
//! stores, replaces or removes one. The set lives in this process's memory
//! and nowhere else; the computer itself writes no value to disk. Every entry
//! joins the environment of each job the agent starts through `shell` or
//! `files run`, under the name the person gave it, so a command uses `$NAME`
//! and a tool that reads that variable finds it. A preparation job is left
//! out, because what it captures is written into the workspace.
//!
//! Nothing answers a value back to the agent. There is no `GET`; `state
//! info` lists the names alone; and every text a tool returns has each value
//! replaced with `[redacted NAME]` on its way out, in the spelling JSON gives
//! it too. That keeps a value out of the model's context when a job prints
//! it, and off the tape the desk keeps. It is not a wall against a command
//! written to get one out: a value printed in pieces, encoded, or written to
//! a file the desk reads on its own is not caught, and the person's own view
//! of the screen is never redacted.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock, RwLockReadGuard};

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use rmcp::model::ContentBlock;
use serde_json::json;

use crate::App;
use crate::tools::ToolResult;

/// The desk refuses a shorter value before storing it; one this short would
/// redact ordinary text out of every answer.
const MIN_VALUE_CHARS: usize = 8;
const MAX_VALUE_BYTES: usize = 64 * 1024;
const MAX_NAME_CHARS: usize = 64;
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

/// The set the desk last delivered, shared between the door that takes it,
/// the jobs that start with it, and the answers it is taken out of.
#[derive(Clone, Default)]
pub struct Secrets(Arc<RwLock<BTreeMap<String, String>>>);

impl Secrets {
    /// Takes the whole set, or none of it: one entry the computer would not
    /// put in a job's environment refuses the delivery and keeps the last set.
    pub fn replace(&self, set: BTreeMap<String, String>) -> Result<(), String> {
        for (name, value) in &set {
            check_name(name)?;
            check_value(name, value)?;
        }
        *self
            .0
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = set;
        Ok(())
    }

    /// The names, which is all any answer carries.
    pub fn names(&self) -> Vec<String> {
        self.read().keys().cloned().collect()
    }

    /// What a job offered the set starts with.
    pub fn environment(&self) -> BTreeMap<String, String> {
        self.read().clone()
    }

    fn read(&self) -> RwLockReadGuard<'_, BTreeMap<String, String>> {
        self.0
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// `text` with every value replaced by `[redacted NAME]`. Longer values
    /// go first, so one that contains another is taken whole; and a value
    /// with a quote, a backslash or a line break is also taken in the
    /// spelling JSON gives it, since most tools answer JSON.
    pub fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let set = self.read();
        if set.is_empty() {
            return Cow::Borrowed(text);
        }
        let mut needles = Vec::with_capacity(set.len() * 2);
        for (name, value) in set.iter() {
            let mark = format!("[redacted {name}]");
            let escaped = serde_json::to_string(value)
                .ok()
                .and_then(|quoted| quoted.get(1..quoted.len() - 1).map(str::to_owned));
            if let Some(escaped) = escaped
                && escaped != *value
            {
                needles.push((escaped, mark.clone()));
            }
            needles.push((value.clone(), mark));
        }
        needles.sort_by_key(|(needle, _)| std::cmp::Reverse(needle.len()));
        let mut text = Cow::Borrowed(text);
        for (needle, mark) in &needles {
            if text.contains(needle.as_str()) {
                text = Cow::Owned(text.replace(needle.as_str(), mark));
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

/// A name is the environment variable it becomes: capital letters, digits
/// and underscores, starting with a letter, at most 64 of them, and none the
/// computer or a shell already owns. The desk checks the same before it
/// stores one, so a refusal here means the two disagree.
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

/// `PUT /secrets`: the whole set, as a JSON object of name to value. It
/// answers 204 with nothing, 400 naming the entry it refused, and never a
/// value.
pub async fn replace(
    State(app): State<App>,
    Json(set): Json<BTreeMap<String, String>>,
) -> Response {
    match app.secrets.replace(set) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => (StatusCode::BAD_REQUEST, Json(json!({"error": error}))).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;
    use std::path::PathBuf;

    fn set(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
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
            set(&[("GITHUB_TOKEN", "ghp_notarealtoken0001")])
        );
        secrets.replace(BTreeMap::new()).unwrap();
        assert!(secrets.names().is_empty());
        assert!(secrets.environment().is_empty());
    }

    #[test]
    fn every_value_is_taken_out_of_a_text_whole_and_in_its_json_spelling() {
        let secrets = Secrets::default();
        assert!(matches!(secrets.redact("nothing stored"), Cow::Borrowed(_)));
        secrets
            .replace(set(&[
                ("GITHUB_TOKEN", "ghp_notarealtoken0001"),
                ("LONGER", "ghp_notarealtoken0001-and-more"),
                ("PEM", "line one\nline \"two\"\\"),
            ]))
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
            .replace(set(&[("CONTRACT_SECRET", "contract-secret-value-0001")]))
            .unwrap();
        let printed = crate::tools::call(
            &app,
            "shell",
            json!({"command": "sh", "args": ["-c", "printf '%s' \"$CONTRACT_SECRET\""]}),
            "tester",
        )
        .await
        .unwrap();
        let printed = text(&printed);
        assert!(printed.contains("[redacted CONTRACT_SECRET]"), "{printed}");
        assert!(!printed.contains("contract-secret-value-0001"), "{printed}");
        let info = crate::tools::call(&app, "state", json!({"action": "info"}), "tester")
            .await
            .unwrap();
        let info = text(&info);
        let report: serde_json::Value = serde_json::from_str(&info).unwrap();
        assert_eq!(report["secrets"], json!(["CONTRACT_SECRET"]));
        assert!(!info.contains("contract-secret-value-0001"), "{info}");
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
        assert_eq!(
            secrets.names(),
            ["GITHUB_TOKEN"],
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
