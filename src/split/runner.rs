//! The runner: the computer the agent works in, as uid 1000 `agent`. It is
//! the single-user computer without its door: Xvfb, the session bus, the
//! keyring, the desktop, the jobs, the files, the viewer and the general
//! browser, all as before. What it is never given is a value it does not
//! hand on to a job anyway, or the bearer: the broker answers the desk, and
//! asks this side over the line (fd 3) and by handing it HTTP connections
//! for the desktop's own pages (fd 4).

use std::os::fd::OwnedFd;
use std::path::Path;

use base64::Engine;
use serde_json::{Value, json};

use super::{fd, wire};
use crate::secrets::Delivered;
use crate::{App, Config};

/// The process PID 1 starts as `agent`.
pub fn run(mut config: Config) -> Result<(), String> {
    super::set_mode(super::Mode::Split);
    for descriptor in [fd::LINE, fd::HANDED] {
        wire::cloexec(descriptor)?;
    }
    // The runner never holds the bearer; the broker checked it.
    config.token = None;
    // Only the agent reads its home; the broker and the signed-in browser
    // have no business there, and nothing of theirs is kept in it.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&config.home, std::fs::Permissions::from_mode(0o700));
    }
    crate::boot::run_runner(config)
}

/// What the runner serves once its desktop is up, in place of the door.
pub async fn serve(app: App) -> Result<(), String> {
    app.jobs.initialize().await?;
    tokio::spawn(crate::manifest::resume(app.clone()));
    if let Err(error) = app.files.publish() {
        eprintln!("file windows: {error}");
    }
    tokio::spawn(crate::portal::serve(app.clone()));
    // A home the single-user computer used is migrated before the broker
    // is told the desktop is up.
    if let Err(error) = super::migration::on_boot(&app).await {
        eprintln!("hotline-computer: migration: {error}");
    }
    // SAFETY of ownership: these were inherited for this alone; `run` marked
    // them close-on-exec before anything was started.
    let line = wire::inherited(fd::LINE)?;
    let handed = wire::inherited(fd::HANDED)?;
    let (peer, done) = wire::Peer::start(line, handler(app.clone()))?;
    let router = crate::serve::runner_router(app.clone());
    let accepting = accept(handed, router);
    peer.tell("runner.ready", Value::Null);
    eprintln!("hotline-computer: runner ready");
    let jobs = app.jobs.clone();
    let observer = app.observer.clone();
    let result = tokio::select! {
        () = done => Err("the broker has gone".to_owned()),
        result = accepting => result,
        () = crate::serve::shutdown() => Ok(()),
    };
    jobs.shutdown().await;
    observer.shutdown().await;
    result
}

/// Connections the broker hands over, each served like the single-user
/// computer's own, minus the bearer check the broker already made.
async fn accept(handed: OwnedFd, router: axum::Router) -> Result<(), String> {
    let (sender, mut connections) = tokio::sync::mpsc::unbounded_channel::<OwnedFd>();
    std::thread::Builder::new()
        .name("handed".to_owned())
        .spawn(move || {
            let mut byte = [0u8; 1];
            loop {
                match wire::recv_fds(wire::raw(&handed), &mut byte) {
                    Ok((0, fds)) if fds.is_empty() => return,
                    Ok((_, fds)) => {
                        for fd in fds {
                            if sender.send(fd).is_err() {
                                return;
                            }
                        }
                    }
                    Err(error) => {
                        eprintln!("hotline-computer: handed connections: {error}");
                        return;
                    }
                }
            }
        })
        .map_err(|error| error.to_string())?;
    while let Some(connection) = connections.recv().await {
        let router = router.clone();
        tokio::spawn(async move {
            let stream = std::os::unix::net::UnixStream::from(connection);
            if stream.set_nonblocking(true).is_err() {
                return;
            }
            let Ok(stream) = tokio::net::UnixStream::from_std(stream) else {
                return;
            };
            let service = hyper_util::service::TowerToHyperService::new(router);
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                .with_upgrades()
                .await;
        });
    }
    Err("the broker stopped handing over connections".to_owned())
}

fn handler(app: App) -> wire::Handler {
    std::sync::Arc::new(move |op, args| {
        let app = app.clone();
        Box::pin(async move { answer(&app, &op, args).await })
    })
}

fn text_of<'a>(args: &'a Value, key: &str) -> &'a str {
    args[key].as_str().unwrap_or_default()
}

/// The broker's requests. None of them carries a value a job could not
/// already read: a login's fields are typed by the broker's signed-in
/// browser, and a `tool.call` that brings one is refused.
async fn answer(app: &App, op: &str, args: Value) -> Result<Value, String> {
    match op {
        "tool.call" => {
            if !matches!(args.get("filled"), None | Some(Value::Null)) {
                return Err("the runner never takes a secret's value".to_owned());
            }
            let content = crate::tools::call(
                app,
                text_of(&args, "name"),
                args["arguments"].clone(),
                text_of(&args, "holder"),
            )
            .await?;
            Ok(json!({"content": content}))
        }
        "secrets.set" => {
            let variables: std::collections::BTreeMap<String, String> =
                serde_json::from_value(args["variables"].clone())
                    .map_err(|error| error.to_string())?;
            app.secrets.replace(
                variables
                    .into_iter()
                    .map(|(name, value)| (name, Delivered::Value(value)))
                    .collect(),
            )?;
            let catalog: Vec<Value> =
                serde_json::from_value(args["catalog"].clone()).map_err(|e| e.to_string())?;
            super::migration::on_delivery(app, &catalog).await;
            app.secrets.list_as(catalog);
            let count = args["count"].as_u64().unwrap_or_default() as u32;
            let _ = crate::x11::publish_sealed(&app.config.display, count);
            Ok(Value::Null)
        }
        "access.person" => Ok(json!(app.access.person().await)),
        "access.holder" => Ok(json!(app.access.holder().await)),
        "access.release" => app
            .access
            .release(text_of(&args, "holder"))
            .await
            .map(|released| json!(released)),
        "agent.snapshot" => {
            serde_json::to_value(app.jobs.agent.snapshot()).map_err(|error| error.to_string())
        }
        "agent.client" => {
            app.jobs
                .agent
                .client(text_of(&args, "name"), args["title"].as_str());
            Ok(Value::Null)
        }
        "agent.holder" => {
            app.jobs.agent.holder(text_of(&args, "holder"));
            Ok(Value::Null)
        }
        "browser.current_url" => {
            if args["idle"].as_bool().unwrap_or(false) {
                Ok(json!(app.browser.current_url_if_idle().await))
            } else {
                app.browser.current_url().await.map(|url| json!(url))
            }
        }
        "browser.sync_passkeys" => app
            .browser
            .sync_passkeys(args["launch"].as_bool().unwrap_or(false))
            .await
            .map(|()| Value::Null),
        // The general browser on the desktop, for what the broker's own
        // browser will not open itself.
        "general.launch" => app.browser.open().await.map(|()| Value::Null),
        "links.open" => {
            let url = text_of(&args, "url");
            if !crate::links::is_link(url) {
                return Err(format!("{url:?} is not a web address"));
            }
            app.browser.tab_new(url).await.map(|answer| json!(answer))
        }
        // A file of the agent's for a page to take, and a page's file for
        // the agent: read and written here, as the agent, under the home and
        // outside the computer's own records.
        "upload.read" => {
            let path =
                crate::tools::files::existing_path(app, Path::new(text_of(&args, "path"))).await?;
            crate::tools::files::refuse_private(app, &path).await?;
            let bytes = read_capped(&path).await?;
            Ok(json!({
                "path": path,
                "name": path.file_name().map(|name| name.to_string_lossy().into_owned()),
                "content": base64::engine::general_purpose::STANDARD.encode(bytes),
            }))
        }
        "download.write" => {
            let path =
                crate::tools::files::writable_path(app, Path::new(text_of(&args, "path"))).await?;
            crate::tools::files::refuse_private(app, &path).await?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(text_of(&args, "content"))
                .map_err(|error| format!("content is not base64: {error}"))?;
            tokio::fs::write(&path, bytes)
                .await
                .map_err(|error| error.to_string())?;
            Ok(json!({"path": path}))
        }
        "migration.status" => Ok(super::migration::status(app).await),
        "migration.answer" => super::migration::answer(app, text_of(&args, "snapshots")).await,
        other => Err(format!("the runner does not know {other:?}")),
    }
}

async fn read_capped(path: &Path) -> Result<Vec<u8>, String> {
    const MAX: u64 = 50 << 20;
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|error| error.to_string())?;
    if metadata.is_dir() || metadata.len() > MAX {
        return Err(format!(
            "{} is a folder or larger than 50 MB",
            path.display()
        ));
    }
    tokio::fs::read(path)
        .await
        .map_err(|error| error.to_string())
}
