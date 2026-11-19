//! The signed-in browser against a real headless Chromium and a small local
//! site. Skipped where no `chromium` is on `PATH`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use rmcp::model::ContentBlock;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::screen::{FromScreen, Hands};
use super::*;

/// One browser at a time: each test starts its own.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const PASSWORD: &str = "hunter2-canary-w3";
const SEED: &str = "JBSWY3DPEHPK3PXP";

fn have_chromium() -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join("chromium").is_file()))
}

/// The links the general browser was asked to open.
#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<String>>>);

impl General for Recorder {
    fn open(&self, url: String) {
        self.0.lock().unwrap().push(url);
    }

    fn deliver(&self, name: String, bytes: Vec<u8>) -> Delivered {
        let line = format!("download {name} {}", String::from_utf8_lossy(&bytes));
        self.0.lock().unwrap().push(line);
        Box::pin(async move { Ok(format!("~/Downloads/{name}")) })
    }
}

/// A sign-in site: `/login` (username and password), `/totp` (the code),
/// `/home`, with links out; `/stuck`, whose form never leaves; `/type`, a
/// field with the focus; `/whoami`, its cookies.
async fn site(off_site: u16) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let Ok(count) = stream.read(&mut chunk).await else {
                        return;
                    };
                    if count == 0 {
                        return;
                    }
                    request.extend_from_slice(&chunk[..count]);
                    let text = String::from_utf8_lossy(&request).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length = text
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                            })
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let text = String::from_utf8_lossy(&request).to_string();
                let path = text.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let cookie = text
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("Cookie: ")
                            .or_else(|| line.strip_prefix("cookie: "))
                    })
                    .unwrap_or("")
                    .to_owned();
                let (status, location, body) = match path.split('?').next().unwrap_or("/") {
                    "/login" => ("200 OK", String::new(), r#"<title>Sign in</title><h1>Sign in to the test site</h1>
                        <form action="/totp" method="post"><label>Username <input name="u"></label>
                        <label>Password <input name="p" type="password"></label><button>Sign in</button></form>"#.to_owned()),
                    "/totp" => {
                        let body = text.split("\r\n\r\n").nth(1).unwrap_or("");
                        if body.contains(&format!("p={PASSWORD}")) {
                            ("200 OK", String::new(), r#"<title>Code</title><form action="/home" method="get">
                                <label>Code <input name="otp" autocomplete="one-time-code"></label><button>Verify</button></form>"#.to_owned())
                        } else {
                            ("200 OK", String::new(), "<title>Wrong</title>wrong password".to_owned())
                        }
                    }
                    "/home" => ("200 OK", String::new(), format!(
                        r#"<title>Home</title><p>Welcome back</p><a href="/oauth">Connect app</a>
                        <a href="http://127.0.0.1:{off_site}/plain">Docs</a>"#)),
                    "/oauth" => ("302 Found", format!("http://127.0.0.1:{off_site}/cb?code=sekrit-code&state=s1"), String::new()),
                    "/stuck" => ("200 OK", String::new(), r#"<title>Stuck</title><form onsubmit="return false">
                        <label>Password <input name="p" type="password"></label><button>Sign in</button></form>"#.to_owned()),
                    "/type" => ("200 OK", String::new(), r#"<title>Type</title><input id="t" autofocus>"#.to_owned()),
                    "/pk" => ("200 OK", String::new(), r#"<title>PK</title><p id="r">pending</p><script>
                        navigator.credentials.get({publicKey:{challenge:new Uint8Array(32),rpId:'localhost',userVerification:'preferred',timeout:20000}})
                        .then(c => document.getElementById('r').textContent = 'signed ' + c.type)
                        .catch(e => document.getElementById('r').textContent = 'refused ' + e.name + ' ' + e.message);</script>"#.to_owned()),
                    "/leaky" => ("200 OK", String::new(), format!(r#"<title>Leaky</title>
                        <form action="http://127.0.0.1:{off_site}/form" method="get"><label>Password <input name="p" type="password"></label><button>Sign in</button></form>"#)),
                    "/files" => ("200 OK", String::new(), r#"<title>Files</title><a href="/report.csv">Report</a>"#.to_owned()),
                    "/report.csv" => ("200 OK\r\nContent-Disposition: attachment; filename=\"report.csv\"", String::new(), "a,b\n1,2\n".to_owned()),
                    "/whoami" => ("200 OK", String::new(), format!("<title>Who</title><p>cookies: [{cookie}]</p>")),
                    _ => ("200 OK", String::new(), "<title>Plain</title>plain page".to_owned()),
                };
                let location = if location.is_empty() {
                    String::new()
                } else {
                    format!("Location: {location}\r\n")
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\n{location}Content-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    port
}

struct Rig {
    app: App,
    signin: Signin,
    general: Recorder,
    port: u16,
    _root: tempfile::TempDir,
}

async fn rig(logins: Value) -> Rig {
    let root = tempfile::tempdir().unwrap();
    let off_site = site(0).await;
    let port = site(off_site).await;
    let mut app = App::new(crate::Config {
        addr: "127.0.0.1:0".into(),
        token: Some("test-only".into()),
        home: root.path().join("home"),
        display: String::new(),
        screen: "1920x1080".into(),
    });
    let general = Recorder::default();
    let signin = Signin::new(
        Box::new(spawn::Direct::new(root.path().join("signin"))),
        Box::new(general.clone()),
        app.secrets.clone(),
        Some(Downloads::Staged({
            let staged = root.path().join("downloads");
            std::fs::create_dir_all(&staged).unwrap();
            staged
        })),
    );
    app.signin = Some(signin.clone());
    let mut set: BTreeMap<String, crate::secrets::Delivered> = BTreeMap::new();
    for (name, login) in logins.as_object().unwrap() {
        let mut login = login.clone();
        login["sites"] = json!([format!("http://localhost:{port}")]);
        set.insert(name.clone(), serde_json::from_value(login).unwrap());
    }
    app.secrets.replace(set).unwrap();
    Rig {
        app,
        signin,
        general,
        port,
        _root: root,
    }
}

fn login() -> Value {
    json!({"SITE": {"kind": "login", "sites": [], "username": "george", "password": PASSWORD, "totp": SEED}})
}

impl Rig {
    async fn browser(&self, arguments: Value) -> Result<String, String> {
        let blocks = crate::tools::call(&self.app, "browser", arguments, "Rex").await?;
        let text: Vec<String> = blocks
            .iter()
            .map(|block| match block.as_text() {
                Some(text) => text.text.clone(),
                None => "[image]".to_owned(),
            })
            .collect();
        let text = text.join("\n");
        assert!(
            !text.contains(PASSWORD),
            "a password reached the agent: {text}"
        );
        Ok(text)
    }

    async fn browser_wait(&self, arguments: Value) -> Result<String, String> {
        let blocks = crate::tools::call(&self.app, "wait", arguments, "Rex").await?;
        Ok(blocks
            .iter()
            .filter_map(|block| block.as_text().map(|text| text.text.clone()))
            .collect())
    }

    async fn blocks(&self, arguments: Value) -> Vec<ContentBlock> {
        crate::tools::call(&self.app, "browser", arguments, "Rex")
            .await
            .unwrap()
    }

    fn url(&self, path: &str) -> String {
        format!("http://localhost:{}{path}", self.port)
    }

    /// The ref `text` gives the element named `label`.
    async fn find(&self, label: &str) -> String {
        let text = self.browser(json!({"action": "text"})).await.unwrap();
        text.lines()
            .find(|line| line.contains(label) && line.starts_with("[e"))
            .and_then(|line| line.strip_prefix('[')?.split(']').next())
            .unwrap_or_else(|| panic!("no {label} in {text}"))
            .to_owned()
    }

    /// Waits up to 20 s, a page's own WebAuthn patience here, for `until`.
    async fn settle(&self, until: &str) -> String {
        let mut text = String::new();
        for _ in 0..200 {
            text = self.browser(json!({"action": "text"})).await.unwrap();
            if text.contains(until) {
                return text;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        panic!("the page never showed {until}: {text}");
    }
}

#[tokio::test]
async fn eval_is_refused_on_a_signed_in_site() {
    if !have_chromium() {
        return eprintln!("skipped: no chromium");
    }
    let _serial = SERIAL.lock().await;
    let rig = rig(login()).await;
    let went = rig
        .browser(json!({"action": "navigate", "url": rig.url("/login")}))
        .await
        .unwrap();
    assert!(went.contains("signed-in browser"), "{went}");
    let refused = rig
        .browser(json!({"action": "eval", "js": "btoa(document.querySelector('[type=password]').value)"}))
        .await
        .unwrap_err();
    assert!(refused.contains("scripts do not run there"), "{refused}");
    assert!(refused.contains("SITE"), "{refused}");
    let text = rig.browser(json!({"action": "text"})).await.unwrap();
    assert!(text.contains("Sign in to the test site"), "{text}");
    let read = rig
        .browser(json!({"action": "read", "selector": "h1"}))
        .await
        .unwrap();
    assert!(read.contains("Sign in to the test site"), "{read}");
    // The general browser never started, so its DevTools port lists nothing.
    assert!(rig.app.browser.open_pages().await.unwrap().is_empty());
    rig.signin.stop().await;
}

#[tokio::test]
async fn no_observation_during_sign_in_including_screenshots() {
    if !have_chromium() {
        return eprintln!("skipped: no chromium");
    }
    let _serial = SERIAL.lock().await;
    let rig = rig(login()).await;
    rig.browser(json!({"action": "navigate", "url": rig.url("/login")}))
        .await
        .unwrap();
    let username = rig.find("Username").await;
    let password = rig.find("Password").await;
    let button = rig.find("[button] Sign in").await;
    rig.browser(json!({"action": "fill", "ref": username, "secret": "SITE.username"}))
        .await
        .unwrap();
    // The username is who, not a secret: the page is still shown.
    let shown = rig.browser(json!({"action": "text"})).await.unwrap();
    assert!(shown.contains("george"), "{shown}");
    let filled = rig
        .browser(json!({"action": "fill", "ref": password, "secret": "SITE.password"}))
        .await
        .unwrap();
    assert!(filled.contains("hidden from you"), "{filled}");
    // A new-document event that arrives late says nothing: only the
    // browser's own answer about the tab ends the withholding.
    let running = rig.signin.browser().await.unwrap();
    let session = rig.signin.lock_tabs().list[0].session.clone();
    rig.signin
        .event(
            &running,
            cdp::Event {
                method: "Page.frameNavigated".into(),
                params: json!({"frame": {"id": "late", "url": rig.url("/elsewhere")}}),
                session: Some(session),
            },
        )
        .await;
    for look in [
        json!({"action": "text"}),
        json!({"action": "links"}),
        json!({"action": "read", "selector": "form"}),
    ] {
        let answer = rig.browser(look.clone()).await.unwrap();
        assert!(answer.contains("middle of signing in"), "{look}: {answer}");
        assert!(!answer.contains("Username"), "{look}: {answer}");
    }
    let shot = rig.blocks(json!({"action": "screenshot"})).await;
    assert!(
        shot.iter().all(|block| block.as_image().is_none()),
        "no picture while signing in"
    );
    let tabs = rig.browser(json!({"action": "tabs"})).await.unwrap();
    let tabs: Vec<Value> = serde_json::from_str(&tabs).unwrap();
    let signed_in = tabs.iter().find(|tab| tab["signedIn"] == true).unwrap();
    assert_eq!(signed_in["withheld"], true);
    assert!(signed_in.get("url").is_none() && signed_in.get("title").is_none());
    // A plain fill on a withheld page answers without what the field holds.
    let typed = rig
        .browser(json!({"action": "fill", "ref": username, "text": "george"}))
        .await
        .unwrap();
    assert_eq!(typed, format!("filled {username}"));
    // Submitting loads a new document: the code page is shown again.
    rig.browser(json!({"action": "click_ref", "ref": button}))
        .await
        .unwrap();
    rig.settle("Verify").await;
    let code = rig.find("Code").await;
    let verify = rig.find("[button] Verify").await;
    rig.browser(json!({"action": "fill", "ref": code, "secret": "SITE.code"}))
        .await
        .unwrap();
    let hidden = rig.browser(json!({"action": "text"})).await.unwrap();
    assert!(hidden.contains("middle of signing in"), "{hidden}");
    assert!(!hidden.contains("otp"), "{hidden}");
    rig.browser(json!({"action": "click_ref", "ref": verify}))
        .await
        .unwrap();
    let home = rig.settle("Welcome back").await;
    assert!(!home.contains("otp="), "the code is in the address: {home}");
    let shot = rig.blocks(json!({"action": "screenshot"})).await;
    assert!(shot.iter().any(|block| block.as_image().is_some()));
    rig.signin.stop().await;
}

#[tokio::test]
async fn a_sign_in_that_cannot_finish_is_handed_to_the_person() {
    if !have_chromium() {
        return eprintln!("skipped: no chromium");
    }
    let _serial = SERIAL.lock().await;
    let rig = rig(login()).await;
    rig.browser(json!({"action": "navigate", "url": rig.url("/stuck")}))
        .await
        .unwrap();
    let password = rig.find("Password").await;
    rig.browser(json!({"action": "fill", "ref": password, "secret": "SITE.password"}))
        .await
        .unwrap();
    // As if half a minute went by with no new page.
    for tab in rig.signin.lock_tabs().list.iter_mut() {
        tab.secret_at = tab.secret_at.saturating_sub(FINISH_AFTER_MS + 1);
    }
    let answer = rig.browser(json!({"action": "text"})).await.unwrap();
    assert!(answer.contains("Finish signing in"), "{answer}");
    // Leaving for another page of the site shows it again.
    rig.browser(json!({"action": "navigate", "url": rig.url("/home")}))
        .await
        .unwrap();
    let home = rig.browser(json!({"action": "text"})).await.unwrap();
    assert!(home.contains("Welcome back"), "{home}");
    rig.signin.stop().await;
}

#[tokio::test]
async fn an_oauth_callback_never_reaches_the_general_browser() {
    if !have_chromium() {
        return eprintln!("skipped: no chromium");
    }
    let _serial = SERIAL.lock().await;
    let rig = rig(login()).await;
    rig.browser(json!({"action": "navigate", "url": rig.url("/home")}))
        .await
        .unwrap();
    let connect = rig.find("Connect app").await;
    let answer = rig
        .browser(json!({"action": "click_ref", "ref": connect}))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let later = rig.browser(json!({"action": "text"})).await.unwrap();
    let told = format!("{answer}\n{later}");
    assert!(told.contains("was stopped"), "{told}");
    assert!(!told.contains("sekrit-code"), "{told}");
    assert!(
        rig.general
            .0
            .lock()
            .unwrap()
            .iter()
            .all(|url| !url.contains("code=")),
        "the callback reached the general browser"
    );
    // A plain link out opens there instead.
    rig.browser(json!({"action": "navigate", "url": rig.url("/home")}))
        .await
        .unwrap();
    let docs = rig.find("Docs").await;
    rig.browser(json!({"action": "click_ref", "ref": docs}))
        .await
        .unwrap();
    for _ in 0..30 {
        if !rig.general.0.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let opened = rig.general.0.lock().unwrap().clone();
    assert!(
        opened.iter().any(|url| url.ends_with("/plain")),
        "{opened:?}"
    );
    rig.signin.stop().await;
}

#[tokio::test]
async fn removing_a_share_signs_the_site_out() {
    if !have_chromium() {
        return eprintln!("skipped: no chromium");
    }
    let _serial = SERIAL.lock().await;
    let rig = rig(json!({})).await;
    let site = Origin::site(&rig.url("")).unwrap();
    let cookies = vec![
        json!({"name": "sid", "value": "shared-session-w3", "domain": "localhost", "path": "/"}),
        json!({"name": "other", "value": "x", "domain": "elsewhere.example", "path": "/"}),
    ];
    let (taken, _) = rig
        .signin
        .share(
            "SHARED",
            vec![site.clone()],
            cookies,
            serde_json::Map::new(),
        )
        .await
        .unwrap();
    assert_eq!(taken, 1, "a cookie for another site is left out");
    assert!(rig.signin.routes(&rig.url("/whoami")));
    rig.browser(json!({"action": "navigate", "url": rig.url("/whoami")}))
        .await
        .unwrap();
    let before = rig.browser(json!({"action": "text"})).await.unwrap();
    assert!(before.contains("sid=shared-session-w3"), "{before}");
    let (gone, forgotten, closed) = rig.signin.unshare("SHARED", &[]).await.unwrap().unwrap();
    assert_eq!(gone, vec![site]);
    assert_eq!(forgotten, 1);
    assert_eq!(closed, 1);
    assert!(!rig.signin.routes(&rig.url("/whoami")));
    let running = rig.signin.browser().await.unwrap();
    let left = running
        .cdp
        .call("Storage.getCookies", json!({}))
        .await
        .unwrap();
    assert_eq!(left["cookies"], json!([]), "the session is gone");
    assert!(rig.signin.unshare("SHARED", &[]).await.unwrap().is_none());
    rig.signin.stop().await;
}

#[tokio::test]
async fn revoking_a_login_signs_its_site_out() {
    if !have_chromium() {
        return eprintln!("skipped: no chromium");
    }
    let _serial = SERIAL.lock().await;
    let rig = rig(login()).await;
    rig.browser(json!({"action": "navigate", "url": rig.url("/home")}))
        .await
        .unwrap();
    let running = rig.signin.browser().await.unwrap();
    running
        .cdp
        .call(
            "Storage.setCookies",
            json!({"cookies": [{"name": "sid", "value": "v", "domain": "localhost", "path": "/"}]}),
        )
        .await
        .unwrap();
    rig.app.secrets.replace(BTreeMap::new()).unwrap();
    for _ in 0..40 {
        if rig.signin.tab_list().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(rig.signin.tab_list().is_empty(), "the site's tab closed");
    let left = running
        .cdp
        .call("Storage.getCookies", json!({}))
        .await
        .unwrap();
    assert_eq!(left["cookies"], json!([]));
    rig.signin.stop().await;
}

#[tokio::test]
async fn allow_covers_this_tabs_sign_in_only() {
    if !have_chromium() {
        return eprintln!("skipped: no chromium");
    }
    let _serial = SERIAL.lock().await;
    let mut asking = login();
    asking["SITE"]["use"] = json!("ask");
    let rig = rig(asking).await;
    rig.browser(json!({"action": "navigate", "url": rig.url("/login")}))
        .await
        .unwrap();
    let password = rig.find("Password").await;
    let fill = json!({"action": "fill", "ref": password, "secret": "SITE.password"});
    let waiting = rig.browser(fill.clone()).await.unwrap_err();
    assert!(waiting.contains("Waiting for the person"), "{waiting}");
    let held = rig.app.holds.held(crate::jobs::now());
    let wire = serde_json::to_value(&held[0]).unwrap();
    assert_eq!(wire["operation"]["kind"], "signin");
    assert_eq!(wire["operation"]["origin"], rig.url(""));
    assert_eq!(wire["operation"]["account"], "george");
    assert_eq!(wire["attested"], true);
    let flow = wire["flow"].as_str().unwrap().to_owned();
    rig.app
        .holds
        .answer(&held[0].id, crate::holds::Answer::Allow, crate::jobs::now())
        .unwrap();
    rig.browser(fill).await.unwrap();
    // Another tab is another sign-in.
    rig.browser(json!({"action": "tab_new", "url": rig.url("/login")}))
        .await
        .unwrap();
    let other = rig.find("Password").await;
    let again = rig
        .browser(json!({"action": "fill", "ref": other, "secret": "SITE.password"}))
        .await
        .unwrap_err();
    assert!(again.contains("Waiting for the person"), "{again}");
    let held = rig.app.holds.held(crate::jobs::now());
    let wire = serde_json::to_value(held.last().unwrap()).unwrap();
    assert_ne!(wire["flow"], json!(flow));
    rig.signin.stop().await;
}

#[tokio::test]
async fn a_passkey_signs_only_for_a_signed_in_tabs_own_origin() {
    if !have_chromium() {
        return eprintln!("skipped: no chromium");
    }
    let _serial = SERIAL.lock().await;
    let rig = rig(login()).await;
    assert!(
        rig.signin.flow_on(&rig.url("")).is_none(),
        "no tab is there yet"
    );
    rig.browser(json!({"action": "navigate", "url": rig.url("/home")}))
        .await
        .unwrap();
    let flow = rig.signin.flow_on(&rig.url("")).unwrap();
    assert_eq!(rig.signin.flow_on(&rig.url("")), Some(flow));
    assert!(rig.signin.flow_on("https://evil.example").is_none());
    assert!(rig.signin.flow_on("http://127.0.0.1:1").is_none());
    rig.signin.stop().await;
}

#[tokio::test]
async fn site_input_never_reaches_display_0() {
    if !have_chromium() {
        return eprintln!("skipped: no chromium");
    }
    let _serial = SERIAL.lock().await;
    let rig = rig(login()).await;
    // No X display at all: whatever the person types can only go to the
    // signed-in tab through DevTools.
    assert!(rig.app.display.is_none());
    rig.browser(json!({"action": "navigate", "url": rig.url("/type")}))
        .await
        .unwrap();
    let running = rig.signin.browser().await.unwrap();
    let tab = rig.signin.current(&running).await.unwrap();
    let mut hands = Hands::default();
    let ignored = rig
        .signin
        .input(
            &running.cdp,
            &tab,
            &mut hands,
            FromScreen::Key {
                key: "x".into(),
                down: true,
            },
        )
        .await
        .unwrap();
    assert!(!ignored, "a watching screen moves nothing");
    rig.signin
        .input(
            &running.cdp,
            &tab,
            &mut hands,
            FromScreen::Control { take: true },
        )
        .await
        .unwrap();
    // The person clicks into the field, then types.
    let (x, y) = rig
        .signin
        .evaluate(
            &running.cdp,
            &tab.session,
            "(() => { const r = document.getElementById('t').getBoundingClientRect(); return [r.left + 5, r.top + r.height / 2]; })()",
        )
        .await
        .map(|point| (point[0].as_f64().unwrap() as i16, point[1].as_f64().unwrap() as i16))
        .unwrap();
    for message in [
        FromScreen::Move { x, y },
        FromScreen::Button { b: 1, down: true },
        FromScreen::Button { b: 1, down: false },
    ] {
        rig.signin
            .input(&running.cdp, &tab, &mut hands, message)
            .await
            .unwrap();
    }
    for key in ["h", "i"] {
        for down in [true, false] {
            rig.signin
                .input(
                    &running.cdp,
                    &tab,
                    &mut hands,
                    FromScreen::Key {
                        key: key.into(),
                        down,
                    },
                )
                .await
                .unwrap();
        }
    }
    let typed = rig
        .signin
        .evaluate(
            &running.cdp,
            &tab.session,
            "document.getElementById('t').value",
        )
        .await
        .unwrap();
    assert_eq!(typed, "hi");
    // While the person drives, and after they typed, the agent sees nothing.
    let hidden = rig.browser(json!({"action": "text"})).await.unwrap();
    assert!(hidden.contains("The person is using"), "{hidden}");
    rig.signin.0.person_at.store(0, Ordering::Relaxed);
    let still = rig.browser(json!({"action": "text"})).await.unwrap();
    assert!(still.contains("middle of signing in"), "{still}");
    rig.signin.stop().await;
}

/// The whole passkey path: the page asks, the extension in the signed-in
/// browser forwards it, the native host relays it to the bridge, and the
/// broker signs. Needs the image's extension and native host manifest.
#[tokio::test]
async fn a_passkey_signs_in_on_its_site_through_the_signed_in_browser() {
    let manifest =
        std::path::Path::new("/etc/chromium/native-messaging-hosts/dev.hotline.computer.json");
    let host = std::fs::read_to_string(manifest)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|manifest| manifest["path"].as_str().map(std::path::PathBuf::from));
    if !have_chromium()
        || !crate::bridge::extension_answers()
        || !host.is_some_and(|host| host.exists())
    {
        return eprintln!("skipped: no chromium, extension or native host");
    }
    let _serial = SERIAL.lock().await;
    let details = json!({"rp": {"id": "localhost", "name": "Test"}, "user": {"id": "dXNlcg", "name": "george"},
        "challenge": "AAAA", "pubKeyCredParams": [{"type": "public-key", "alg": -7}],
        "extensions": {"remoteDesktopClientOverride": {"origin": "http://localhost:1", "sameOriginWithAncestors": true}}});
    let (passkey, _) =
        crate::authenticator::mint(&crate::authenticator::creation(&details).unwrap()).unwrap();
    let mut logins = login();
    let mut key = serde_json::to_value(&passkey).unwrap();
    key["kind"] = json!("passkey");
    let rig = rig(logins.take()).await;
    let mut set: BTreeMap<String, crate::secrets::Delivered> = BTreeMap::new();
    set.insert("PK".into(), serde_json::from_value(key).unwrap());
    let mut site = login();
    site["SITE"]["sites"] = json!([rig.url("")]);
    set.insert(
        "SITE".into(),
        serde_json::from_value(site["SITE"].clone()).unwrap(),
    );
    rig.app.secrets.replace(set).unwrap();
    let bridge = tokio::spawn(crate::bridge::serve(rig.app.clone()));
    rig.browser(json!({"action": "navigate", "url": rig.url("/pk")}))
        .await
        .unwrap();
    let signed = rig.settle("signed public-key").await;
    assert!(!signed.contains("refused"), "{signed}");
    bridge.abort();
    rig.signin.stop().await;
}

/// Open Browser: the desk opens `/sites/ws` as it opens `/ws`, and gets the
/// signed-in site's frames in the viewer's own wire form.
#[tokio::test]
async fn the_site_screen_streams_frames_in_the_viewer_protocol() {
    use futures_util::StreamExt;
    if !have_chromium() {
        return eprintln!("skipped: no chromium");
    }
    let _serial = SERIAL.lock().await;
    let rig = rig(login()).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = crate::serve::router(rig.app.clone());
    let serving = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    let refused =
        tokio_tungstenite::connect_async(format!("ws://{address}/sites/ws?token=wrong")).await;
    assert!(refused.is_err(), "the token is checked");
    let site = rig.url("");
    let (mut socket, _) = tokio_tungstenite::connect_async(format!(
        "ws://{address}/sites/ws?token=test-only&site={}",
        site.replace(':', "%3A").replace('/', "%2F")
    ))
    .await
    .unwrap();
    let frame = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        while let Some(message) = socket.next().await {
            if let Ok(tokio_tungstenite::tungstenite::Message::Binary(bytes)) = message {
                return bytes.to_vec();
            }
        }
        panic!("the screen closed");
    })
    .await
    .unwrap();
    let header: Vec<u16> = frame[..12]
        .chunks(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    assert_eq!(header[0..2], [0, 0]);
    assert!(
        header[2] > 0 && header[2] == header[4] && header[3] == header[5],
        "{header:?}"
    );
    assert_eq!(&frame[13..16], b"PNG");
    let tabs = rig.signin.tab_list();
    assert!(
        tabs.iter().any(|tab| tab["url"]
            .as_str()
            .is_some_and(|url| url.starts_with(&site))),
        "{tabs:?}"
    );
    socket.close(None).await.unwrap();
    serving.abort();
    rig.signin.stop().await;
}

#[tokio::test]
async fn shares_go_to_the_signed_in_browser_only_where_there_is_one() {
    let home = tempfile::tempdir().unwrap();
    let app = App::new(crate::Config {
        addr: "127.0.0.1:0".into(),
        token: Some("test-only".into()),
        home: home.path().into(),
        display: String::new(),
        screen: "1920x1080".into(),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let serving = tokio::spawn(async move {
        let _ = axum::serve(listener, crate::serve::router(app)).await;
    });
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let share = json!({"sites": ["github.com"], "cookies": [], "storage": {}});
    let naked = http
        .put(format!("{base}/sessions/GH"))
        .json(&share)
        .send()
        .await
        .unwrap();
    assert_eq!(naked.status(), 401);
    let legacy = http
        .put(format!("{base}/sessions/GH"))
        .bearer_auth("test-only")
        .json(&share)
        .send()
        .await
        .unwrap();
    assert_eq!(
        legacy.status(),
        404,
        "a single-user computer keeps saved logins"
    );
    serving.abort();
}

/// A sign-in form a site sends elsewhere by GET would put the password in
/// the address: it never goes to the general browser, or anywhere.
#[tokio::test]
async fn a_password_sent_off_the_site_never_leaves() {
    if !have_chromium() {
        return eprintln!("skipped: no chromium");
    }
    let _serial = SERIAL.lock().await;
    let rig = rig(login()).await;
    rig.browser(json!({"action": "navigate", "url": rig.url("/leaky")}))
        .await
        .unwrap();
    let password = rig.find("Password").await;
    let button = rig.find("[button] Sign in").await;
    rig.browser(json!({"action": "fill", "ref": password, "secret": "SITE.password"}))
        .await
        .unwrap();
    let clicked = rig
        .browser(json!({"action": "click_ref", "ref": button}))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let later = rig.browser(json!({"action": "text"})).await.unwrap();
    assert!(
        format!("{clicked}{later}").contains("was stopped"),
        "{clicked} {later}"
    );
    assert!(
        rig.general.0.lock().unwrap().is_empty(),
        "nothing went to the general browser"
    );
    rig.signin.stop().await;
}

/// Finish signing in reaches the desk: a sign-in the broker could not finish
/// is listed for the person until the tab moves on or the login goes.
#[tokio::test]
async fn a_sign_in_waiting_on_the_person_is_listed_for_the_desk() {
    if !have_chromium() {
        return eprintln!("skipped: no chromium");
    }
    let _serial = SERIAL.lock().await;
    let rig = rig(login()).await;
    rig.browser(json!({"action": "navigate", "url": rig.url("/stuck")}))
        .await
        .unwrap();
    let password = rig.find("Password").await;
    rig.browser(json!({"action": "fill", "ref": password, "secret": "SITE.password"}))
        .await
        .unwrap();
    assert!(
        rig.signin.pending().await.is_empty(),
        "not yet: the broker may still finish it"
    );
    let age = |rig: &Rig| {
        for tab in rig.signin.lock_tabs().list.iter_mut() {
            tab.secret_at = tab.secret_at.saturating_sub(FINISH_AFTER_MS + 1);
        }
    };
    age(&rig);
    let pending = rig.signin.pending().await;
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(pending[0]["kind"], "finish");
    assert_eq!(pending[0]["name"], "SITE");
    assert_eq!(pending[0]["origin"], rig.url(""));
    assert_eq!(pending[0]["account"], "george");
    assert!(pending[0]["since"].as_u64().unwrap() > 0);
    // The agent is still told, as before.
    let told = rig.browser(json!({"action": "text"})).await.unwrap();
    assert!(told.contains("Finish signing in"), "{told}");
    // Through the door, bearer-only.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = crate::serve::router(rig.app.clone());
    let serving = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let naked = http
        .get(format!("{base}/signin/pending"))
        .send()
        .await
        .unwrap();
    assert_eq!(naked.status(), 401);
    let listed: Value = http
        .get(format!("{base}/signin/pending"))
        .bearer_auth("test-only")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed["pending"][0]["name"], "SITE");
    serving.abort();
    // A new document ends it.
    rig.browser(json!({"action": "navigate", "url": rig.url("/home")}))
        .await
        .unwrap();
    assert!(rig.signin.pending().await.is_empty());
    // So does revoking the login.
    rig.browser(json!({"action": "navigate", "url": rig.url("/stuck")}))
        .await
        .unwrap();
    let password = rig.find("Password").await;
    rig.browser(json!({"action": "fill", "ref": password, "secret": "SITE.password"}))
        .await
        .unwrap();
    age(&rig);
    assert_eq!(rig.signin.pending().await.len(), 1);
    rig.app.secrets.replace(BTreeMap::new()).unwrap();
    assert!(rig.signin.pending().await.is_empty());
    rig.signin.stop().await;
}

/// A download in the signed-in browser lands in the staging folder and is
/// handed to the agent's Downloads; the staged file goes.
#[tokio::test]
async fn a_download_is_handed_to_the_agents_downloads() {
    if !have_chromium() {
        return eprintln!("skipped: no chromium");
    }
    let _serial = SERIAL.lock().await;
    let rig = rig(login()).await;
    rig.browser(json!({"action": "navigate", "url": rig.url("/files")}))
        .await
        .unwrap();
    let report = rig.find("Report").await;
    rig.browser(json!({"action": "click_ref", "ref": report}))
        .await
        .unwrap();
    let mut handed = Vec::new();
    for _ in 0..50 {
        handed = rig.general.0.lock().unwrap().clone();
        if !handed.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(handed, vec!["download report.csv a,b\n1,2\n".to_owned()]);
    let told = rig.browser(json!({"action": "text"})).await.unwrap();
    assert!(told.contains("downloaded report.csv"), "{told}");
    let staged: Vec<_> = std::fs::read_dir(rig._root.path().join("downloads"))
        .unwrap()
        .collect();
    assert!(staged.is_empty(), "the staged file is removed");
    rig.signin.stop().await;
}

/// A wait spanning a sign-in answers only from the next document, never
/// from the page the secret was typed into.
#[tokio::test]
async fn a_wait_during_sign_in_resolves_only_after_the_next_document() {
    if !have_chromium() {
        return eprintln!("skipped: no chromium");
    }
    let _serial = SERIAL.lock().await;
    let rig = rig(login()).await;
    rig.browser(json!({"action": "navigate", "url": rig.url("/login")}))
        .await
        .unwrap();
    let found = rig
        .browser_wait(json!({"text": "Sign in to the test site", "timeout": 2}))
        .await
        .unwrap();
    assert!(found.contains("found"), "{found}");
    let password = rig.find("Password").await;
    let button = rig.find("[button] Sign in").await;
    rig.browser(json!({"action": "fill", "ref": password, "secret": "SITE.password"}))
        .await
        .unwrap();
    let refused = rig
        .browser_wait(json!({"text": "Sign in to the test site", "timeout": 2}))
        .await
        .unwrap_err();
    assert!(refused.contains("was not looked for"), "{refused}");
    let (waited, _) = tokio::join!(
        rig.browser_wait(json!({"text": "Verify", "timeout": 15})),
        async {
            tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
            rig.browser(json!({"action": "click_ref", "ref": button}))
                .await
        }
    );
    assert!(waited.unwrap().contains("found"));
    rig.signin.stop().await;
}
