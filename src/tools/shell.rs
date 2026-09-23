use serde::Deserialize;
use serde_json::{Value, json};

use super::{CALL_BUDGET_MS, ToolResult, action_error, json_text};

/// exec's deadline, leaving the rest of the call's budget to drain output.
const EXEC_LIMIT_MS: u64 = 45_000;
use crate::{App, jobs::Start};

#[derive(Deserialize)]
struct Input {
    #[serde(default)]
    action: String,
    #[serde(flatten)]
    start: Start,
    job_id: Option<String>,
    text: Option<String>,
    #[serde(default)]
    eof: bool,
    #[serde(default)]
    cursor: u64,
    wait_ms: Option<u64>,
    max_output: Option<usize>,
    name: Option<String>,
}

pub async fn call(app: &App, arguments: Value, holder: &str) -> ToolResult {
    let mut input: Input = serde_json::from_value(arguments).map_err(|error| error.to_string())?;
    let id = input.job_id.as_deref().unwrap_or("");
    let limit = input.max_output.unwrap_or(65_536).clamp(1, 1_048_576);
    match input.action.as_str() {
        "show" => {
            let _guard = app.access.mutate(holder).await?;
            if !id.is_empty() {
                app.jobs.status(id).await?;
            }
            app.observer.select((!id.is_empty()).then_some(id)).await?;
            json_text(
                json!({"ok":true,"observer":"Alacritty","pid":app.observer.pid().await,"job_id":input.job_id}),
            )
        }
        "list" => json_text(app.jobs.list().await?),
        "status" => json_text(app.jobs.status(id).await?),
        "read" => json_text(app.jobs.read(id, input.cursor, limit).await?),
        "wait" => json_text(
            app.jobs
                .wait(id, input.wait_ms.unwrap_or(1000).min(CALL_BUDGET_MS))
                .await?,
        ),
        "cancel" => {
            // Admission is serialized with desktop input; waiting for exit is not.
            let guard = app.access.mutate(holder).await?;
            app.jobs.request_cancel(id).await?;
            drop(guard);
            json_text(app.jobs.wait(id, 5000).await?)
        }
        "write" => {
            let _guard = app.access.mutate(holder).await?;
            json_text(
                app.jobs
                    .write(id, input.text.as_deref().unwrap_or(""), input.eof)
                    .await?,
            )
        }
        "run" => {
            let name = input
                .name
                .as_deref()
                .ok_or("run needs name: one of the workspace's runs, which state manifest lists")?;
            if !input.start.command.is_empty() {
                return Err(
                    "run takes a name, not a command; args are appended to the run's own".into(),
                );
            }
            let home = &app.config.home;
            let cwd = input
                .start
                .cwd
                .as_deref()
                .map(std::path::PathBuf::from)
                .map(|p| if p.is_absolute() { p } else { home.join(p) })
                .unwrap_or_else(|| home.clone());
            let (workspace, composed) = crate::manifest::find(home, &cwd)?;
            let kind = composed.manifest.runs.get(name).map(|r| r.kind);
            let (start, url) = crate::manifest::job(&workspace, &composed, name, input.start)?;
            let before = crate::ready::windows_now(app);
            let guard = app.access.mutate(holder).await?;
            let job = app.jobs.start(start, holder).await?;
            drop(guard);
            if let Some(kind) = kind.filter(|k| *k != crate::manifest::Kind::Task) {
                crate::ready::remember(&job.id, kind, url.clone(), before);
            }
            let observer_error = if app.display.is_some() {
                app.observer.show(false).await.err()
            } else {
                None
            };
            let mut result = serde_json::to_value(job).map_err(|e| e.to_string())?;
            result["run"] = json!(name);
            result["kind"] = json!(kind);
            result["workspace"] = json!(workspace);
            if let Some(url) = url {
                result["url"] = json!(url);
            }
            if let Some(error) = observer_error {
                result["observer_error"] = json!(error);
            }
            // A desktop or web run answers when it is up, not when it was started.
            let wait = input.wait_ms.unwrap_or(45_000).min(CALL_BUDGET_MS);
            if kind.is_some_and(|k| k != crate::manifest::Kind::Task) && wait > 0 {
                let id = result["id"].as_str().unwrap_or_default().to_owned();
                let (ready, images) = crate::ready::wait(app, &id, wait).await?;
                result["ready"] = ready;
                let mut blocks = json_text(result)?;
                blocks.extend(images);
                return Ok(blocks);
            }
            json_text(result)
        }
        "ready" => {
            let (ready, images) =
                crate::ready::wait(app, id, input.wait_ms.unwrap_or(45_000).min(CALL_BUDGET_MS))
                    .await?;
            let mut blocks = json_text(ready)?;
            blocks.extend(images);
            Ok(blocks)
        }
        "" | "exec" | "start" | "launch" => {
            let synchronous = input.action.is_empty() || input.action == "exec";
            if synchronous {
                input.start.timeout = Some(
                    input
                        .start
                        .timeout
                        .unwrap_or(30)
                        .clamp(1, EXEC_LIMIT_MS / 1000),
                );
            }
            // A command the agent runs is offered the person's stored secrets.
            input.start.secrets = true;
            // A whole line in `command` means what it would at a prompt.
            if input.start.args.is_empty() && is_command_line(&input.start.command) {
                let line = std::mem::take(&mut input.start.command);
                input.start.label.get_or_insert_with(|| line.clone());
                input.start.args = vec!["-c".into(), line];
                input.start.command = "bash".into();
            }
            let guard = app.access.mutate(holder).await?;
            let job = app.jobs.start(input.start, holder).await?;
            drop(guard);
            let observer_error = if app.display.is_some() {
                app.observer.show(false).await.err()
            } else {
                None
            };
            if !synchronous {
                let mut result = serde_json::to_value(job).map_err(|e| e.to_string())?;
                if let Some(error) = observer_error {
                    result["observer_error"] = json!(error);
                }
                return json_text(result);
            }
            let mut job = app.jobs.wait(&job.id, EXEC_LIMIT_MS).await?;
            // An execution cut off at its deadline may still be draining output.
            if !job.finished() {
                job = app
                    .jobs
                    .wait(&job.id, CALL_BUDGET_MS - EXEC_LIMIT_MS)
                    .await?;
            }
            let (stdout, stderr) = app.jobs.streams(&job.id, limit).await?;
            json_text(json!({
                "stdout":stdout,"stderr":stderr,
                "exit_code":job.exit_code.unwrap_or(if job.state == "timed_out" {255} else {-1}),
                "duration_ms":job.finished_at.unwrap_or_else(crate::jobs::now).saturating_sub(job.started_at),
                "truncated":job.truncated || job.output_bytes > stdout.len() + stderr.len(),
                "job_id":job.id,"state":job.state,"signal":job.signal,"error":job.error,"observer_error":observer_error
            }))
        }
        action => Err(action_error(
            "shell",
            action,
            &[
                "exec", "start", "launch", "run", "ready", "list", "status", "read", "wait",
                "write", "cancel", "show",
            ],
        )),
    }
}

/// Whether `command` is a shell line rather than one program's name.
fn is_command_line(command: &str) -> bool {
    command
        .chars()
        .any(|c| c.is_whitespace() || "|&;<>()$`*?~\"'".contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_goes_to_the_shell_and_a_name_does_not() {
        assert!(is_command_line("ls -la"));
        assert!(is_command_line("cat a|wc"));
        assert!(is_command_line("echo $HOME"));
        assert!(!is_command_line("ls"));
        assert!(!is_command_line("/usr/bin/env"));
        assert!(!is_command_line("cargo"));
    }
    #[tokio::test]
    async fn a_waiting_command_does_not_lock_out_desktop_takeover() {
        let home = tempfile::tempdir().unwrap();
        let app = App::new(crate::Config {
            addr: String::new(),
            token: None,
            home: home.path().to_owned(),
            display: ":0".into(),
            screen: "800x600".into(),
        });
        let running = app.clone();
        let task = tokio::spawn(async move {
            call(
                &running,
                json!({"command":"sh","args":["-c","printf started; sleep 1"]}),
                "agent",
            )
            .await
        });
        while app.jobs.list().await.unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
        let guard = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            app.access.mutate("person"),
        )
        .await
        .expect("job wait held the desktop lock")
        .unwrap();
        drop(guard);
        app.access.seize("person", 10).await;
        assert!(
            call(&app, json!({"action":"start","command":"true"}), "agent")
                .await
                .is_err()
        );
        assert!(call(&app, json!({"action":"list"}), "agent").await.is_ok());
        assert!(task.await.unwrap().is_ok());
    }
}
