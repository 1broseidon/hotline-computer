use serde::Deserialize;
use serde_json::{Value, json};

use super::{ToolResult, action_error, json_text};
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
        "wait" => json_text(app.jobs.wait(id, input.wait_ms.unwrap_or(1000)).await?),
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
        "" | "exec" | "start" | "launch" => {
            let synchronous = input.action.is_empty() || input.action == "exec";
            if synchronous {
                input.start.timeout = Some(input.start.timeout.unwrap_or(30).clamp(1, 60));
            }
            // A command the agent runs is offered the person's stored secrets.
            input.start.secrets = true;
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
            let mut job = app.jobs.wait(&job.id, 60_000).await?;
            // A 60-second execution may still be draining output at the deadline.
            if !job.finished() {
                job = app.jobs.wait(&job.id, 5000).await?;
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
                "exec", "start", "launch", "list", "status", "read", "wait", "write", "cancel",
                "show",
            ],
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
