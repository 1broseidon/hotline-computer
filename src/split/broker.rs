//! The broker: the computer's door, as uid 1001 `hotline`. It holds the
//! bearer and every secret the desk delivers, asks the person about ask-me
//! keys, signs with passkeys and answers the browser extension. The work
//! itself is the runner's: a tool call goes over the line, and a request for
//! one of the desktop's own pages (the viewer, its socket, the Files panel,
//! the file manager) is handed to the runner as a connection of its own,
//! once the broker has checked the bearer and taken it out.

use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use super::{Runner, fd, wire};
use crate::{App, Config};

/// The proxy's pre-bound listener, for the proxy to take when it starts.
static PROXY: Mutex<Option<std::net::TcpListener>> = Mutex::new(None);
/// The line to PID 1, for starting and stopping the signed-in browser.
static INIT: Mutex<Option<std::os::unix::net::UnixStream>> = Mutex::new(None);

/// `127.0.0.1:3128`, bound by PID 1 for the container's life so that no job
/// can take it while the broker restarts. Taken once.
pub fn take_proxy_listener() -> Option<std::net::TcpListener> {
    PROXY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
}

/// Has PID 1 start the signed-in browser as uid 1002: `/usr/bin/chromium`
/// with `arguments`, DevTools on a pipe at its fds 3 and 4. Answers its
/// process ID, the end to write commands to, and the end to read from.
pub fn signin_spawn(arguments: &[String]) -> Result<(u32, OwnedFd, OwnedFd), String> {
    let mut line = INIT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let line = line.as_mut().ok_or("PID 1 is not on the line")?;
    wire::write_blocking(line, &json!({"op": "signin.spawn", "args": arguments}))?;
    // The pipe's ends come with the answer's first bytes.
    let mut length = [0u8; 4];
    let (count, mut fds) = wire::recv_fds(line.as_raw_fd(), &mut length)?;
    if count == 0 {
        return Err("PID 1 has gone".to_owned());
    }
    std::io::Read::read_exact(line, &mut length[count..]).map_err(|error| error.to_string())?;
    let length = u32::from_le_bytes(length) as usize;
    if length > wire::MAX_FRAME {
        return Err("PID 1 answered too much".to_owned());
    }
    let mut body = vec![0u8; length];
    std::io::Read::read_exact(line, &mut body).map_err(|error| error.to_string())?;
    let answer = wire::decode(&body)?;
    if let Some(error) = answer["err"].as_str() {
        return Err(error.to_owned());
    }
    if fds.len() != 2 {
        return Err("PID 1 sent no pipe".to_owned());
    }
    let read = fds.pop().expect("two");
    let write = fds.pop().expect("two");
    Ok((
        answer["ok"]["pid"].as_u64().unwrap_or_default() as u32,
        write,
        read,
    ))
}

/// Has PID 1 stop a signed-in browser it started.
pub fn signin_stop(pid: u32) -> Result<(), String> {
    let mut line = INIT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let line = line.as_mut().ok_or("PID 1 is not on the line")?;
    wire::write_blocking(line, &json!({"op": "signin.stop", "args": {"pid": pid}}))?;
    let answer = wire::read_blocking(line)?.ok_or("PID 1 has gone")?;
    match answer["err"].as_str() {
        Some(error) => Err(error.to_owned()),
        None => Ok(()),
    }
}

/// The process PID 1 starts as `hotline`. `main` has already sealed it: the
/// bearer is out of its environment and its memory is out of reach.
pub fn run(config: Config) -> Result<(), String> {
    super::set_mode(super::Mode::Split);
    let line = wire::inherited(fd::LINE)?;
    let door = wire::inherited(fd::DOOR)?;
    let init = wire::inherited(fd::INIT)?;
    let proxy = wire::inherited(fd::PROXY)?;
    let handoff = wire::inherited(fd::HANDOFF)?;
    *PROXY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) =
        Some(std::net::TcpListener::from(proxy));
    *INIT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) =
        Some(std::os::unix::net::UnixStream::from(init));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(async move {
        // The key proxy, on the socket PID 1 holds for it, and its CA in the
        // bundle every user reads. Without it the computer still runs, and
        // says legacy at /capabilities so no protected key is sent.
        let keys = match take_proxy_listener() {
            Some(listener) => {
                match super::keys::Keys::start(listener, std::path::Path::new(super::CA_BUNDLE)) {
                    Ok(keys) => Some(keys),
                    Err(error) => {
                        eprintln!("hotline-computer: the key proxy did not start: {error}");
                        None
                    }
                }
            }
            None => None,
        };
        let (ready, readiness) = tokio::sync::watch::channel(false);
        let ready = Arc::new(ready);
        let job_keys = keys.clone();
        let handler: wire::Handler = Arc::new(move |op, args| {
            let ready = Arc::clone(&ready);
            // A job's keys are answered as the frame arrives, so a job's end
            // is taken before a later job's start on the same line.
            let keys = super::keys::answer(job_keys.as_ref(), &op, &args);
            Box::pin(async move {
                if let Some(answer) = keys {
                    return answer;
                }
                match op.as_str() {
                    "runner.ready" => {
                        let _ = ready.send(true);
                        Ok(Value::Null)
                    }
                    other => Err(format!("the broker does not know {other:?}")),
                }
            })
        });
        let (peer, done) = wire::Peer::start(line, handler)?;
        let mut app = App::new(config);
        app.keys = keys;
        app.runner = Some(Runner {
            line: peer,
            handoff: Arc::new(Mutex::new(handoff)),
            ready: readiness,
        });
        // The signed-in browser, which PID 1 starts as `signin`; its bridge
        // is the only one, and answers `signin` alone.
        app.signin = Some(crate::signin::split::for_broker(&app));
        tokio::spawn(crate::bridge::serve(app.clone()));
        let listener = std::net::TcpListener::from(door);
        listener
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        let listener =
            tokio::net::TcpListener::from_std(listener).map_err(|error| error.to_string())?;
        eprintln!(
            "hotline-computer broker listening on {}",
            listener
                .local_addr()
                .map_or_else(|_| "the door".to_owned(), |address| address.to_string())
        );
        let router = crate::serve::broker_router(app);
        tokio::select! {
            served = axum::serve(listener, router).with_graceful_shutdown(crate::serve::shutdown()) => {
                served.map_err(|error| error.to_string())
            }
            () = done => Err("the runner has gone".to_owned()),
        }
    })
}

/// `/health`: ok once the runner's desktop is up, as the single-user
/// computer's door opens only then.
pub async fn health(State(app): State<App>) -> Response {
    match &app.runner {
        Some(runner) if !*runner.ready.borrow() => {
            (StatusCode::SERVICE_UNAVAILABLE, "starting").into_response()
        }
        _ => "ok".into_response(),
    }
}

/// `GET /migration`: what the first split boot did with a home the
/// single-user computer used, and the old snapshots waiting for an answer.
pub async fn migration(State(app): State<App>) -> Response {
    match &app.runner {
        None => StatusCode::NOT_FOUND.into_response(),
        Some(runner) => answered(runner.call("migration.status", Value::Null).await),
    }
}

/// `POST /migration/answer {snapshots: "clean"|"delete"|"keep"}`.
pub async fn migration_answer(
    State(app): State<App>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    match &app.runner {
        None => StatusCode::NOT_FOUND.into_response(),
        Some(runner) => answered(
            runner
                .call(
                    "migration.answer",
                    json!({"snapshots": body["snapshots"].as_str().unwrap_or_default()}),
                )
                .await,
        ),
    }
}

fn answered(answer: Result<Value, String>) -> Response {
    match answer {
        Ok(value) => axum::Json(value).into_response(),
        Err(error) => {
            (StatusCode::BAD_REQUEST, axum::Json(json!({"error": error}))).into_response()
        }
    }
}

/// Routes whose callers present the bearer as `?token=` because a page
/// cannot set a header; the single-user computer checks it in each.
fn takes_a_query_token(path: &str) -> bool {
    matches!(path, "/ws" | "/files" | "/files/download")
}

fn without_token(query: &str) -> String {
    query
        .split('&')
        .filter(|pair| !pair.is_empty() && pair.split('=').next() != Some("token"))
        .collect::<Vec<_>>()
        .join("&")
}

fn query_token(query: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == "token").then(|| percent_decode(value))
    })
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                match u8::from_str_radix(&text[index + 1..index + 3], 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        index += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Every other route: the runner's, over a connection handed to it. The
/// bearer never goes with it, in a header or in the query.
pub async fn proxy(State(app): State<App>, mut request: Request) -> Response {
    let Some(runner) = app.runner.clone() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let path = request.uri().path().to_owned();
    let query = request.uri().query().unwrap_or("").to_owned();
    if takes_a_query_token(&path)
        && let Some(expected) = app.config.token.as_deref()
    {
        let presented = request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(str::to_owned)
            .or_else(|| query_token(&query))
            .unwrap_or_default();
        if !crate::serve::same_secret(presented.as_bytes(), expected.as_bytes()) {
            return (
                StatusCode::UNAUTHORIZED,
                axum::Json(json!({"error": "unauthorized"})),
            )
                .into_response();
        }
    }
    request.headers_mut().remove(header::AUTHORIZATION);
    let query = without_token(&query);
    let uri = if query.is_empty() {
        path.clone()
    } else {
        format!("{path}?{query}")
    };
    match uri.parse() {
        Ok(uri) => *request.uri_mut() = uri,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    }
    match forward(&runner, request).await {
        Ok(response) => response,
        Err(error) => {
            (StatusCode::BAD_GATEWAY, axum::Json(json!({"error": error}))).into_response()
        }
    }
}

async fn forward(runner: &Runner, mut request: Request) -> Result<Response, String> {
    let mut ready = runner.ready.clone();
    tokio::time::timeout(Duration::from_secs(60), ready.wait_for(|up| *up))
        .await
        .map_err(|_| "the desktop is not up yet".to_owned())?
        .map_err(|_| "the desktop has gone".to_owned())?;
    let (mine, theirs) = wire::pair()?;
    let handoff = Arc::clone(&runner.handoff);
    tokio::task::spawn_blocking(move || {
        let handoff = handoff
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        wire::send_fds(wire::raw(&handoff), b"c", &[wire::raw(&theirs)])
    })
    .await
    .map_err(|error| error.to_string())??;
    let stream = std::os::unix::net::UnixStream::from(mine);
    stream
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let stream = tokio::net::UnixStream::from_std(stream).map_err(|error| error.to_string())?;
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream))
            .await
            .map_err(|error| error.to_string())?;
    tokio::spawn(async move {
        let _ = connection.with_upgrades().await;
    });
    let upgrade = request
        .headers()
        .get(header::UPGRADE)
        .is_some()
        .then(|| hyper::upgrade::on(&mut request));
    let mut response = sender
        .send_request(request)
        .await
        .map_err(|error| error.to_string())?;
    if response.status() == StatusCode::SWITCHING_PROTOCOLS
        && let Some(upgrade) = upgrade
    {
        let theirs = hyper::upgrade::on(&mut response);
        tokio::spawn(async move {
            let (Ok(desk), Ok(runner)) = tokio::join!(upgrade, theirs) else {
                return;
            };
            let mut desk = hyper_util::rt::TokioIo::new(desk);
            let mut runner = hyper_util::rt::TokioIo::new(runner);
            let _ = tokio::io::copy_bidirectional(&mut desk, &mut runner).await;
        });
        let (parts, _) = response.into_parts();
        return Ok(Response::from_parts(parts, Body::empty()));
    }
    Ok(response.map(Body::new))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bearer_never_rides_on_to_the_runner() {
        assert_eq!(
            without_token("token=abc&path=/home/agent"),
            "path=/home/agent"
        );
        assert_eq!(without_token("path=x&token=abc"), "path=x");
        assert_eq!(without_token("token=abc"), "");
        assert_eq!(query_token("path=x&token=a%2Bb"), Some("a+b".to_owned()));
        assert_eq!(query_token("path=x"), None);
    }
}

/// The broker against a stand-in runner, with the agent's home watched by
/// inotify: every route and every tool goes through the broker, and the
/// home sees no open, read, write or change from it.
#[cfg(all(test, target_os = "linux"))]
mod home_tests {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::path::{Path, PathBuf};

    use super::*;

    struct Watch {
        fd: OwnedFd,
    }

    impl Watch {
        fn on(directories: &[PathBuf]) -> Watch {
            // SAFETY: inotify calls on a descriptor made and owned here.
            unsafe {
                let fd = libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC);
                assert!(fd >= 0);
                for directory in directories {
                    let path = std::ffi::CString::new(directory.to_str().unwrap()).unwrap();
                    let mask = libc::IN_OPEN
                        | libc::IN_ACCESS
                        | libc::IN_MODIFY
                        | libc::IN_CREATE
                        | libc::IN_DELETE
                        | libc::IN_ATTRIB
                        | libc::IN_MOVED_TO;
                    assert!(libc::inotify_add_watch(fd, path.as_ptr(), mask) >= 0);
                }
                Watch {
                    fd: OwnedFd::from_raw_fd(fd),
                }
            }
        }

        fn events(&self) -> usize {
            let mut buffer = [0u8; 65536];
            let mut count = 0;
            loop {
                // SAFETY: a read into a buffer of its own size.
                let read = unsafe {
                    libc::read(
                        self.fd.as_raw_fd(),
                        buffer.as_mut_ptr().cast(),
                        buffer.len(),
                    )
                };
                if read <= 0 {
                    return count;
                }
                count += read as usize;
            }
        }
    }

    /// The runner's side: answers each op with a plain reply and records it;
    /// serves every handed connection with one page, noting its headers.
    type Noted = Arc<Mutex<Vec<String>>>;

    fn stand_in_runner() -> (Runner, Noted, Noted, tokio::sync::watch::Sender<bool>) {
        let ops = Arc::new(Mutex::new(Vec::new()));
        let pages = Arc::new(Mutex::new(Vec::new()));
        let (broker_line, runner_line) = wire::pair().unwrap();
        let (handoff, handed) = wire::pair().unwrap();
        let noted = Arc::clone(&ops);
        let runner_side: wire::Handler = Arc::new(move |op, args| {
            noted.lock().unwrap().push(op.clone());
            Box::pin(async move {
                Ok(match op.as_str() {
                    "tool.call" => {
                        json!({"content": [{"type": "text", "text": format!("ran {} {}", args["name"].as_str().unwrap_or(""), args["filled"]["label"].as_str().unwrap_or(""))}]})
                    }
                    "browser.current_url" => json!("https://example.test/login"),
                    "agent.snapshot" => {
                        serde_json::to_value(crate::agent::Agent::default()).unwrap()
                    }
                    _ => Value::Null,
                })
            })
        });
        let (_runner_peer, runner_done) = wire::Peer::start(runner_line, runner_side).unwrap();
        tokio::spawn(async move {
            let _keep = _runner_peer;
            runner_done.await;
        });
        let quiet: wire::Handler = Arc::new(|_, _| Box::pin(async { Ok(Value::Null) }));
        let (line, done) = wire::Peer::start(broker_line, quiet).unwrap();
        tokio::spawn(done);
        let seen = Arc::clone(&pages);
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let mut byte = [0u8; 1];
            while let Ok((_, fds)) = wire::recv_fds(wire::raw(&handed), &mut byte) {
                if fds.is_empty() {
                    return;
                }
                for fd in fds {
                    let seen = Arc::clone(&seen);
                    let stream = std::os::unix::net::UnixStream::from(fd);
                    stream.set_nonblocking(true).unwrap();
                    runtime.block_on(async move {
                        let stream = tokio::net::UnixStream::from_std(stream).unwrap();
                        let service = hyper::service::service_fn(
                            move |request: hyper::Request<hyper::body::Incoming>| {
                                seen.lock().unwrap().push(format!(
                                    "{} auth={}",
                                    request.uri(),
                                    request.headers().contains_key(header::AUTHORIZATION)
                                ));
                                async move {
                                    Ok::<_, std::convert::Infallible>(hyper::Response::new(
                                        Body::from("from the runner"),
                                    ))
                                }
                            },
                        );
                        let _ = hyper::server::conn::http1::Builder::new()
                            .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                            .await;
                    });
                }
            }
        });
        let (ready, readiness) = tokio::sync::watch::channel(true);
        (
            Runner {
                line,
                handoff: Arc::new(Mutex::new(handoff)),
                ready: readiness,
            },
            ops,
            pages,
            ready,
        )
    }

    fn directories(home: &Path) -> Vec<PathBuf> {
        let mut out = vec![home.to_owned()];
        for directory in [".hotline", ".hotline/jobs", ".hotline/logins", "src"] {
            let path = home.join(directory);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("kept"), "x").unwrap();
            out.push(path);
        }
        out
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_broker_never_opens_a_path_in_the_agents_home() {
        let home = tempfile::tempdir().unwrap();
        let watched = directories(home.path());
        let (runner, ops, pages, _ready) = stand_in_runner();
        let mut app = App::new(Config {
            addr: "127.0.0.1:0".into(),
            token: Some("test-only".into()),
            home: home.path().into(),
            display: ":97".into(),
            screen: "1920x1080".into(),
        });
        app.runner = Some(runner.clone());
        let watch = Watch::on(&watched);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let router = crate::serve::broker_router(app.clone());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let bearer = |request: reqwest::RequestBuilder| request.bearer_auth("test-only");
        let status = |response: reqwest::Response| response.status().as_u16();

        assert_eq!(
            status(http.get(format!("{base}/health")).send().await.unwrap()),
            200
        );
        assert_eq!(
            status(
                bearer(http.get(format!("{base}/capabilities?nonce=n")))
                    .send()
                    .await
                    .unwrap()
            ),
            200
        );
        let delivered = bearer(http.put(format!("{base}/secrets")))
            .json(&json!({
                "TOKEN": "hl-test-variable-1",
                "SITE": {"kind": "login", "sites": ["https://example.test"], "username": "u", "password": "hl-test-password-1"},
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(status(delivered), 204);
        for path in ["/secrets/uses", "/passkeys/registration"] {
            assert!(
                status(
                    bearer(http.get(format!("{base}{path}")))
                        .send()
                        .await
                        .unwrap()
                ) < 500,
                "{path}"
            );
        }
        // The desktop's own pages are the runner's, with the bearer taken out.
        for path in [
            "/",
            "/files?token=test-only&path=/home/agent",
            "/files/download?token=test-only&path=/home/agent/src/kept",
            "/pages",
            "/fm/list",
        ] {
            let response = bearer(http.get(format!("{base}{path}")))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status().as_u16(), 200, "{path}");
            assert_eq!(response.text().await.unwrap(), "from the runner", "{path}");
        }
        assert_eq!(
            status(
                http.get(format!("{base}/files?path=/home/agent"))
                    .send()
                    .await
                    .unwrap()
            ),
            401,
            "the broker checks the query token itself"
        );
        let pages = pages.lock().unwrap().clone();
        assert!(!pages.is_empty());
        for page in &pages {
            assert!(
                page.ends_with("auth=false") && !page.contains("test-only"),
                "the bearer never reaches the runner: {page}"
            );
        }
        // Every tool is the runner's, but no secret's value is ever handed
        // to it: a login's field is the signed-in browser's to type.
        for (tool, arguments) in [
            ("capture", json!({})),
            ("input", json!({"action": "click", "x": 1, "y": 1})),
            ("browser", json!({"action": "text"})),
            ("shell", json!({"action": "exec", "command": "true"})),
            (
                "files",
                json!({"action": "get", "path": "/home/agent/src/kept"}),
            ),
            ("windows", json!({"action": "list"})),
            ("wait", json!({"text": "x"})),
            ("state", json!({"action": "info"})),
        ] {
            let answer = crate::split::call_tool(&app, &runner, tool, arguments, "test")
                .await
                .unwrap();
            let text = answer[0].as_text().unwrap().text.clone();
            assert!(text.starts_with(&format!("ran {tool}")), "{tool}: {text}");
        }
        let filled = crate::split::call_tool(
            &app,
            &runner,
            "browser",
            json!({"action": "fill", "ref": "e1", "secret": "TOKEN"}),
            "test",
        )
        .await
        .unwrap_err();
        assert!(filled.contains("never types one"), "{filled}");
        let login = crate::split::call_tool(
            &app,
            &runner,
            "browser",
            json!({"action": "fill", "ref": "e1", "secret": "SITE.password"}),
            "test",
        )
        .await
        .unwrap_err();
        assert!(login.contains("signed-in browser"), "{login}");
        let ops = ops.lock().unwrap().clone();
        assert_eq!(
            ops.iter().filter(|op| *op == "tool.call").count(),
            8,
            "neither fill reached the runner: {ops:?}"
        );
        assert!(ops.contains(&"secrets.set".to_owned()), "{ops:?}");
        server.abort();
        // Give a stray write a moment to show.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(watch.events(), 0, "the broker touched the agent's home");
    }
}
