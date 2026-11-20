//! Protected keys end to end, in one process: the broker's door takes a
//! delivery, the runner (its real op handler, over a real line) starts real
//! jobs, the jobs reach a fake upstream through the real proxy with real
//! curl. What a job, its log and the runner's frames hold is checked for the
//! value. Separation by uid is the image's to show (`tests/protected`).

use std::path::Path;
use std::sync::{Arc, Mutex};

use rustls::RootCertStore;
use serde_json::{Value, json};

use super::*;
use crate::jobs::Start;
use crate::keyproxy::tests::{API, REAL, Upstream, upstream, wait_for, which};
use crate::secrets::Delivery;
use crate::split::{Runner, wire};
use crate::{App, Config};

const READABLE: &str = "hl-readable-w22-0001";

fn config(home: &Path, token: Option<&str>) -> Config {
    Config {
        addr: "127.0.0.1:0".into(),
        token: token.map(str::to_owned),
        home: home.to_owned(),
        display: ":97".into(),
        screen: "1920x1080".into(),
    }
}

struct Rig {
    broker: App,
    runner: App,
    door: String,
    up: Upstream,
    /// Every op the broker sent the runner, with its arguments.
    frames: Arc<Mutex<Vec<String>>>,
    home: tempfile::TempDir,
    _broker_home: tempfile::TempDir,
    _bundle: tempfile::TempDir,
}

async fn rig() -> Rig {
    let up = upstream().await;
    let mut roots = RootCertStore::empty();
    roots.add(up.ca_der.clone()).unwrap();
    let proxy = KeyProxy::for_test(roots, up.addr);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(proxy.clone().serve(listener));
    let bundle_dir = tempfile::tempdir().unwrap();
    let bundle = bundle_dir.path().join("ca-bundle.pem");
    // The fake upstream's CA stands for the system roots, as a host the
    // proxy only tunnels to is checked by the job against those.
    write_bundle(&bundle, &up.ca_pem, proxy.ca_pem()).unwrap();
    let keys = Keys::with(proxy, address, &bundle);

    let home = tempfile::tempdir().unwrap();
    let runner = App::new(config(home.path(), None));
    let frames = Arc::new(Mutex::new(Vec::new()));
    let (broker_end, runner_end) = wire::pair().unwrap();
    let runner_side: wire::Handler = {
        let app = runner.clone();
        let frames = Arc::clone(&frames);
        Arc::new(move |op, args| {
            frames.lock().unwrap().push(format!("{op} {args}"));
            let app = app.clone();
            Box::pin(async move { crate::split::runner::answer(&app, &op, args).await })
        })
    };
    let (runner_line, runner_done) = wire::Peer::start(runner_end, runner_side).unwrap();
    tokio::spawn(runner_done);
    runner.jobs.use_keys(runner_line);
    let broker_side: wire::Handler = {
        let keys = keys.clone();
        Arc::new(move |op, args| {
            let answered =
                answer(Some(&keys), &op, &args).unwrap_or_else(|| Err(format!("not {op}")));
            Box::pin(async move { answered })
        })
    };
    let (broker_line, broker_done) = wire::Peer::start(broker_end, broker_side).unwrap();
    tokio::spawn(broker_done);
    let (handoff, _handed) = wire::pair().unwrap();
    let (_, ready) = tokio::sync::watch::channel(true);
    let broker_home = tempfile::tempdir().unwrap();
    let mut broker = App::new(config(broker_home.path(), Some("test-only")));
    broker.keys = Some(keys);
    broker.runner = Some(Runner {
        line: broker_line,
        handoff: Arc::new(Mutex::new(handoff)),
        ready,
    });
    let door = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/secrets", door.local_addr().unwrap());
    let router = crate::serve::broker_router(broker.clone());
    tokio::spawn(async move { axum::serve(door, router).await.unwrap() });
    Rig {
        broker,
        runner,
        door: base,
        up,
        frames,
        home,
        _broker_home: broker_home,
        _bundle: bundle_dir,
    }
}

fn protected_key() -> Value {
    json!({"kind": "variable", "value": REAL, "use": {
        "origins": [format!("https://{API}")],
        "header": "Authorization",
        "schemes": ["Bearer", "token", "Basic"],
        "standIn": {"prefix": "ghp_", "length": 40},
    }})
}

fn readable() -> Value {
    json!({"kind": "variable", "value": READABLE, "readable": true})
}

async fn put(rig: &Rig, content_type: &str, body: Value) -> u16 {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .put(&rig.door)
        .bearer_auth("test-only")
        .header("content-type", content_type)
        .body(body.to_string())
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

async fn v2(rig: &Rig, generation: u64, secrets: Value) -> u16 {
    put(
        rig,
        crate::secrets::V2,
        json!({"v": 2, "generation": generation, "secrets": secrets}),
    )
    .await
}

/// A shell job offered the secrets, as `shell exec` starts one; its output
/// once it ends.
async fn job(rig: &Rig, script: &str, env: &[(&str, &str)]) -> String {
    let start = Start {
        command: "sh".into(),
        args: vec!["-c".into(), script.into()],
        env: env
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        secrets: true,
        ..Start::default()
    };
    let record = rig.runner.jobs.start(start, "test").await.unwrap();
    let record = rig.runner.jobs.wait(&record.id, 60_000).await.unwrap();
    assert!(record.finished(), "{record:?}");
    rig.runner
        .jobs
        .read(&record.id, 0, 1 << 20)
        .await
        .unwrap()
        .output
}

fn files_holding(root: &Path, needle: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_owned()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if std::fs::read(&path)
                .is_ok_and(|bytes| bytes.windows(needle.len()).any(|w| w == needle.as_bytes()))
            {
                found.push(path.display().to_string());
            }
        }
    }
    found
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revoking_stops_a_running_curl_loop() {
    if which("curl").is_none() {
        eprintln!("skipped: no curl on this machine");
        return;
    }
    let rig = rig().await;
    assert_eq!(
        v2(
            &rig,
            1,
            json!({"GH_TOKEN": protected_key(), "PLAIN": readable()})
        )
        .await,
        204
    );
    let stop = rig.home.path().join("stop");
    let script = format!(
        "while [ ! -e '{}' ]; do curl -sS -o /dev/null https://{API}/loop -H \"Authorization: Bearer $GH_TOKEN\"; sleep 0.05; done",
        stop.display()
    );
    let start = Start {
        command: "sh".into(),
        args: vec!["-c".into(), script],
        secrets: true,
        ..Start::default()
    };
    let looping = rig.runner.jobs.start(start, "test").await.unwrap();
    assert!(
        wait_for(|| rig.up.seen().len() >= 3).await,
        "the loop reached the service"
    );
    // The desk revokes: the next generation no longer has the key.
    assert_eq!(v2(&rig, 2, json!({"PLAIN": readable()})).await, 204);
    let at = rig.up.seen().len();
    let more = wait_for(|| rig.up.seen().len() >= at + 4).await;
    std::fs::write(&stop, "").unwrap();
    rig.runner.jobs.wait(&looping.id, 10_000).await.unwrap();
    assert!(more, "the loop kept running after the revoke");
    let seen = rig.up.seen();
    assert!(
        seen[..3].iter().all(|s| s.auth == format!("Bearer {REAL}")),
        "{seen:?}"
    );
    // One request may have been on its way when the delivery landed.
    for s in &seen[at + 1..] {
        assert!(
            s.auth.starts_with("Bearer ghp_") && !s.auth.contains(REAL),
            "{seen:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stand_in_from_a_finished_job_is_dead() {
    if which("curl").is_none() {
        eprintln!("skipped: no curl on this machine");
        return;
    }
    let rig = rig().await;
    assert_eq!(v2(&rig, 1, json!({"GH_TOKEN": protected_key()})).await, 204);
    let first = job(&rig, "printf 'STANDIN=%s\\n' \"$GH_TOKEN\"", &[]).await;
    let old = first
        .lines()
        .find_map(|line| line.strip_prefix("STANDIN="))
        .expect("the job printed its stand-in")
        .trim()
        .to_owned();
    assert!(old.starts_with("ghp_") && old.len() == 40, "{old}");
    // The runner has told the broker the job ended before it reads as
    // finished, and the broker takes that before the next job's start.
    job(
        &rig,
        &format!(
            "curl -sS -o /dev/null https://{API}/dead -H \"Authorization: Bearer $OLD\"; \
             curl -sS -o /dev/null https://{API}/alive -H \"Authorization: Bearer $GH_TOKEN\""
        ),
        &[("OLD", &old)],
    )
    .await;
    let seen = rig.up.seen();
    let at = |path: &str| seen.iter().find(|s| s.path == path).unwrap().auth.clone();
    assert_eq!(at("/dead"), format!("Bearer {old}"), "{seen:?}");
    assert_eq!(at("/alive"), format!("Bearer {REAL}"), "{seen:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_protected_value_reaches_no_job_env_log_or_runner_frame() {
    let rig = rig().await;
    assert_eq!(
        v2(
            &rig,
            1,
            json!({"GH_TOKEN": protected_key(), "PLAIN": readable()})
        )
        .await,
        204
    );
    let curl = if which("curl").is_some() {
        format!("curl -sS https://{API}/echo -H \"Authorization: Bearer $GH_TOKEN\"; echo")
    } else {
        String::new()
    };
    let out = job(
        &rig,
        &format!(
            "env; echo \"token=$GH_TOKEN\"; {curl} \
             cat /proc/self/environ 2>/dev/null | tr '\\0' '\\n'; \
             printf %s \"$GH_TOKEN\" > leak-plain.txt"
        ),
        &[],
    )
    .await;
    assert!(!out.contains(REAL), "{out}");
    assert!(out.contains("token=ghp_"), "{out}");
    assert!(out.contains(&format!("PLAIN={READABLE}")), "{out}");
    for variable in [
        "HTTPS_PROXY=http://job:",
        "https_proxy=http://job:",
        "NODE_USE_ENV_PROXY=1",
        "NO_PROXY=localhost",
        "SSL_CERT_FILE=",
        "CURL_CA_BUNDLE=",
        "GIT_SSL_CAINFO=",
        "REQUESTS_CA_BUNDLE=",
        "NODE_EXTRA_CA_CERTS=",
    ] {
        assert!(out.contains(variable), "{variable} in {out}");
    }
    if !curl.is_empty() {
        assert!(
            out.contains("\"you_sent\":\"Bearer ghp_"),
            "the reflected header is the stand-in: {out}"
        );
        assert_eq!(rig.up.seen()[0].auth, format!("Bearer {REAL}"));
    }
    // The job's logs and files, and everything else in the agent's home.
    assert!(files_holding(rig.home.path(), REAL).is_empty());
    // The runner was told the readable set and the names, never the value.
    let frames = rig.frames.lock().unwrap().clone();
    assert!(
        frames.iter().any(|f| f.starts_with("secrets.set ")),
        "{frames:?}"
    );
    for frame in &frames {
        assert!(!frame.contains(REAL), "{frame}");
    }
    assert!(!rig.runner.secrets.environment().contains_key("GH_TOKEN"));
    assert_eq!(
        rig.runner
            .secrets
            .environment()
            .get("PLAIN")
            .map(String::as_str),
        Some(READABLE)
    );
    let listed = rig.runner.secrets.catalog();
    assert!(
        listed
            .iter()
            .any(|e| e["name"] == "GH_TOKEN" && e["protected"] == true),
        "{listed:?}"
    );
    // The broker keeps it out of every answer as well.
    assert_eq!(
        rig.broker.secrets.redact(&format!("x {REAL} y")),
        "x [redacted GH_TOKEN] y"
    );
    let refused = match rig.broker.secrets.resolve("GH_TOKEN") {
        Err(error) => error,
        Ok(_) => panic!("a protected key resolved for the browser"),
    };
    assert!(refused.contains("protected key"), "{refused}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_readable_variable_behaves_as_today() {
    let rig = rig().await;
    for (content_type, body) in [
        ("application/json", json!({"PLAIN": READABLE})),
        (
            crate::secrets::V2,
            json!({"v": 2, "generation": 1, "secrets": {"PLAIN": readable()}}),
        ),
    ] {
        assert_eq!(put(&rig, content_type, body).await, 204, "{content_type}");
        let out = job(&rig, "env", &[]).await;
        assert!(out.contains(&format!("PLAIN={READABLE}")), "{out}");
        assert!(
            !out.contains("HTTPS_PROXY=http://127.0.0.1:"),
            "no key, no proxy: {out}"
        );
    }
}

fn plain_app(keys: bool) -> (App, tempfile::TempDir) {
    let home = tempfile::tempdir().unwrap();
    let mut app = App::new(config(home.path(), Some("t")));
    if keys {
        let bundle = home.path().join("ca-bundle.pem");
        app.keys = Some(Keys::with(
            KeyProxy::new().unwrap(),
            "127.0.0.1:3128".parse().unwrap(),
            &bundle,
        ));
    }
    (app, home)
}

fn read(content_type: &str, body: Value) -> Result<Delivery, String> {
    Delivery::read(Some(content_type), body)
}

#[test]
fn a_v2_delivery_reads_every_kind_and_the_use_shapes() {
    let delivery = read(
        "application/vnd.hotline.secrets.v2+json; charset=utf-8",
        json!({"v": 2, "generation": 7, "secrets": {
            "GH_TOKEN": protected_key(),
            "ANTHROPIC_API_KEY": {"kind": "variable", "value": "sk-ant-canary-w22-000000", "use": {
                "origins": ["https://api.anthropic.com"], "header": "x-api-key", "schemes": [],
                "refuse": ["/v1/organizations/*/api_keys"],
            }},
            "PLAIN": readable(),
            "SITE": {"kind": "login", "sites": ["https://example.test"], "username": "u", "password": "hl-test-password-1"},
        }}),
    )
    .unwrap();
    assert_eq!(delivery.generation, Some(7));
    assert_eq!(
        delivery.kept.keys().collect::<Vec<_>>(),
        ["PLAIN", "SITE"],
        "readable variables and logins are kept as today"
    );
    let anthropic = delivery
        .protected
        .iter()
        .find(|b| b.name == "ANTHROPIC_API_KEY")
        .unwrap();
    assert!(anthropic.use_.schemes.is_empty());
    assert_eq!(anthropic.use_.refuse.as_ref().unwrap().len(), 1);
    assert_eq!(delivery.protected.len(), 2);
    let (app, _home) = plain_app(true);
    delivery.apply(&app).unwrap();
    assert_eq!(
        app.secrets.names(),
        ["ANTHROPIC_API_KEY", "GH_TOKEN", "PLAIN", "SITE"]
    );
    assert_eq!(
        app.secrets.environment().keys().collect::<Vec<_>>(),
        ["PLAIN"],
        "a protected key is never in a job's environment"
    );
    let env = app.keys.as_ref().unwrap().begin("job-1");
    assert!(env["GH_TOKEN"].starts_with("ghp_") && env["GH_TOKEN"] != REAL);
    // The job's own proxy credential rides in the address.
    let proxy = &env["HTTPS_PROXY"];
    assert!(
        proxy.starts_with("http://job:") && proxy.ends_with("@127.0.0.1:3128"),
        "{proxy}"
    );
    let other = app.keys.as_ref().unwrap().begin("job-2");
    assert_ne!(other["HTTPS_PROXY"], *proxy, "each job its own");
}

#[test]
fn a_delivery_that_does_not_say_what_a_variable_is_is_refused() {
    let v2 = |secrets: Value| {
        read(
            crate::secrets::V2,
            json!({"v": 2, "generation": 1, "secrets": secrets}),
        )
    };
    let mut both = protected_key();
    both["readable"] = json!(true);
    assert!(v2(json!({"GH_TOKEN": both})).is_err());
    assert!(
        v2(json!({"PLAIN": {"kind": "variable", "value": READABLE}})).is_err(),
        "neither readable nor use"
    );
    let mut named = protected_key();
    named["value"] = json!(REAL);
    assert!(v2(json!({"SSL_CERT_FILE": named})).is_err());
    assert!(
        read(
            crate::secrets::V2,
            json!({"v": 1, "generation": 1, "secrets": {}})
        )
        .is_err()
    );
    assert!(read(crate::secrets::V2, json!({"v": 2, "secrets": {}})).is_err());
    assert!(
        read("application/json", json!({"GH_TOKEN": protected_key()})).is_err(),
        "a v1 body never binds a key: it would be read as a readable one"
    );
    let error = match v2(json!({"GH_TOKEN": protected_key()}))
        .unwrap()
        .apply(&plain_app(false).0)
    {
        Err(error) => error,
        Ok(()) => panic!("a legacy computer took a protected key"),
    };
    assert!(error.contains("legacy") && !error.contains(REAL), "{error}");
}

#[test]
fn legacy_takes_readable_variables_in_v2_and_v1_as_today() {
    let (app, _home) = plain_app(false);
    read(
        crate::secrets::V2,
        json!({"v": 2, "generation": 1, "secrets": {"PLAIN": readable()}}),
    )
    .unwrap()
    .apply(&app)
    .unwrap();
    assert_eq!(app.secrets.environment()["PLAIN"], READABLE);
    read("application/json", json!({"OTHER": READABLE}))
        .unwrap()
        .apply(&app)
        .unwrap();
    assert_eq!(app.secrets.names(), ["OTHER"]);
}

#[test]
fn an_older_generation_changes_nothing_and_v1_unbinds_every_key() {
    let (app, _home) = plain_app(true);
    let at = |generation: u64, secrets: Value| {
        read(
            crate::secrets::V2,
            json!({"v": 2, "generation": generation, "secrets": secrets}),
        )
        .unwrap()
        .apply(&app)
    };
    at(5, json!({"GH_TOKEN": protected_key(), "PLAIN": readable()})).unwrap();
    let keys = app.keys.clone().unwrap();
    let stand_in = keys.begin("job-1")["GH_TOKEN"].clone();
    assert!(at(4, json!({})).is_err());
    assert_eq!(
        app.secrets.names(),
        ["GH_TOKEN", "PLAIN"],
        "the last set stays"
    );
    assert_eq!(keys.begin("job-1")["GH_TOKEN"], stand_in);
    // An old desk's v1 body is the whole set: it holds no key, so none stays.
    read("application/json", json!({"PLAIN": READABLE}))
        .unwrap()
        .apply(&app)
        .unwrap();
    assert_eq!(app.secrets.names(), ["PLAIN"]);
    assert!(
        keys.begin("job-2").is_empty(),
        "no key, no stand-in, no proxy"
    );
}

#[test]
fn the_public_bundle_is_the_system_roots_and_this_boots_ca() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ca-bundle.pem");
    let proxy = KeyProxy::new().unwrap();
    write_bundle(
        &path,
        "-----BEGIN CERTIFICATE-----\nroots\n-----END CERTIFICATE-----\n",
        proxy.ca_pem(),
    )
    .unwrap();
    write_bundle(
        &path,
        "-----BEGIN CERTIFICATE-----\nroots\n-----END CERTIFICATE-----\n",
        proxy.ca_pem(),
    )
    .unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("-----BEGIN CERTIFICATE-----\nroots"));
    assert!(text.ends_with(proxy.ca_pem()));
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o644
    );
}
