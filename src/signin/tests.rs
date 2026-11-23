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
    let found = std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join("chromium").is_file()));
    // The checks image has one: there a skip would hide a broken guarantee.
    assert!(
        found || std::env::var_os("HOTLINE_TESTS_NEED_CHROMIUM").is_none(),
        "HOTLINE_TESTS_NEED_CHROMIUM is set and no chromium is on PATH"
    );
    found
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

/// Every request any rig site was sent, whole, by the site's port: what
/// reached a server.
static HEARD: Mutex<Vec<(u16, String)>> = Mutex::new(Vec::new());

/// Whether the site on `port` was ever sent the password, anywhere in a
/// request.
fn heard_password(port: u16) -> bool {
    HEARD
        .lock()
        .unwrap()
        .iter()
        .any(|(at, request)| *at == port && request.contains(PASSWORD))
}

/// A sign-in site: `/login` (username and password), `/totp` (the code),
/// `/home`, with links out; `/stuck`, whose form never leaves; `/type`, a
/// field with the focus; `/whoami`, its cookies; `/script`, fields a script
/// builds with no form; `/noform-get`, a form with no method and no script
/// (sent by GET); `/grows`, a form that adds its Next button as the name
/// is typed.
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
                HEARD.lock().unwrap().push((port, text.clone()));
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
                            ("200 OK", String::new(), r#"<title>Code</title><form action="/home" method="post">
                                <label>Code <input name="otp" autocomplete="one-time-code"></label><button>Verify</button></form>"#.to_owned())
                        } else {
                            ("200 OK", String::new(), "<title>Wrong</title>wrong password".to_owned())
                        }
                    }
                    "/home" => ("200 OK", String::new(), format!(
                        r#"<title>Home</title><p>Welcome back</p><a href="/oauth">Connect app</a>
                        <a href="http://127.0.0.1:{off_site}/plain">Docs</a>"#)),
                    "/oauth" => ("302 Found", format!("http://127.0.0.1:{off_site}/cb?code=sekrit-code&state=s1"), String::new()),
                    "/stuck" => ("200 OK", String::new(), r#"<title>Stuck</title><form action="/stuck" method="post" onsubmit="return false">
                        <label>Password <input name="p" type="password"></label><button>Sign in</button></form>"#.to_owned()),
                    "/type" => ("200 OK", String::new(), r#"<title>Type</title><input id="t" autofocus>"#.to_owned()),
                    "/script" => ("200 OK", String::new(), r#"<title>Script</title><div id="app"></div><script>
                        app.innerHTML = '<label>Username <input id="u"></label><label>Password <input id="p" type="password"></label><button id="go">Sign in</button>';
                        go.onclick = () => fetch('/totp', {method: 'POST', body: 'p=' + encodeURIComponent(p.value)});</script>"#.to_owned()),
                    "/noform-get" => ("200 OK", String::new(), r#"<meta charset="utf-8"><title>No method</title><form action="/home">
                        <label>Username <input name="u"></label><label>Password <input name="p" type="password"></label><button>Sign in</button></form>"#.to_owned()),
                    "/grows" => ("200 OK", String::new(), r#"<title>Grows</title><form action="/totp" method="post">
                        <label>Username <input name="u" oninput="if (!this.form.querySelector('button')) this.form.insertAdjacentHTML('beforeend', '<button>Next</button>')"></label></form>"#.to_owned()),
                    "/pk" => ("200 OK", String::new(), r#"<title>PK</title><p id="r">pending</p><script>
                        navigator.credentials.get({publicKey:{challenge:new Uint8Array(32),rpId:'localhost',userVerification:'preferred',timeout:20000}})
                        .then(c => document.getElementById('r').textContent = 'signed ' + c.type)
                        .catch(e => document.getElementById('r').textContent = 'refused ' + e.name + ' ' + e.message);</script>"#.to_owned()),
                    "/mk" => ("200 OK", String::new(), r#"<title>MK</title><p id="r">pending</p><script>
                        navigator.credentials.create({publicKey:{rp:{id:'localhost',name:'Test'},user:{id:new Uint8Array([1,2,3]),name:'george',displayName:'George'},
                        challenge:new Uint8Array(32),pubKeyCredParams:[{type:'public-key',alg:-7}],timeout:20000}})
                        .then(c => document.getElementById('r').textContent = 'made ' + c.type)
                        .catch(e => document.getElementById('r').textContent = 'refused ' + e.name + ' ' + e.message);</script>"#.to_owned()),
                    "/leaky" => ("200 OK", String::new(), format!(r#"<title>Leaky</title>
                        <form action="http://127.0.0.1:{off_site}/form" method="get"><label>Password <input name="p" type="password"></label><button>Sign in</button></form>"#)),
                    "/latin" => ("200 OK", String::new(), r#"<meta charset="windows-1252"><title>Latin</title><form action="/home">
                        <label>Password <input name="p" type="password"></label><button>Sign in</button></form>"#.to_owned()),
                    "/latin-form" => ("200 OK", String::new(), r#"<meta charset="utf-8"><title>Latin form</title><form action="/home" accept-charset="ISO-8859-1">
                        <label>Password <input name="p" type="password"></label><button>Sign in</button></form>"#.to_owned()),
                    "/image" => ("200 OK", String::new(), format!(r#"<title>Image</title><form action="/totp" method="post">
                        <label>Password <input name="p" type="password"></label>
                        <input type="image" alt="Go" formaction="http://127.0.0.1:{off_site}/form" formtarget="f"></form><iframe name="f"></iframe>"#)),
                    "/image-later" => ("200 OK", String::new(), format!(r#"<title>Image later</title><form id="f" action="/totp" method="post">
                        <label>Password <input id="p" name="p" type="password"></label></form><iframe name="t"></iframe><script>
                        p.addEventListener('input', () => setTimeout(() => {{
                            f.insertAdjacentHTML('beforeend', '<input type="image" id="go" alt="Go" formaction="http://127.0.0.1:{off_site}/form" formtarget="t">');
                            go.click();
                        }}, 300), {{once: true}});</script>"#)),
                    "/turncoat" => ("200 OK", String::new(), format!(r#"<title>Turncoat</title>
                        <form action="/totp" method="post" onsubmit="this.action='http://127.0.0.1:{off_site}/form'; this.method='get'">
                        <label>Password <input name="p" type="password"></label><button>Sign in</button></form>"#)),
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
    /// The site `/leaky` and the others send elsewhere: no login's.
    off_site: u16,
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
        // The test site is on loopback: granted readable, as a person would
        // have to for a site the computer itself serves.
        if login.get("readable").is_none() {
            login["readable"] = json!(true);
        }
        set.insert(name.clone(), serde_json::from_value(login).unwrap());
    }
    app.secrets.replace(set).unwrap();
    // As `PUT /secrets` does after each delivery: what is granted now is
    // what a later delivery's revocations are told apart from.
    signin.notice_revocations().await;
    Rig {
        app,
        signin,
        general,
        port,
        off_site,
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

/// A share revoked while the browser is stopped is cleared from the profile
/// then, not at the next start: a broker that starts again over the same
/// profile (a restart) knows nothing of the revocation, and a login still
/// names the site.
#[tokio::test]
async fn a_share_revoked_while_stopped_is_gone_after_a_restart() {
    if !have_chromium() {
        return eprintln!("skipped: no chromium");
    }
    let _serial = SERIAL.lock().await;
    let rig = rig(login()).await;
    let site = Origin::site(&rig.url("")).unwrap();
    let cookie = json!({"name": "sid", "value": "shared-session-r1", "domain": "localhost", "path": "/",
                        "expires": crate::jobs::now() / 1000 + 86_400});
    rig.signin
        .share("SHARED", vec![site], vec![cookie], serde_json::Map::new())
        .await
        .unwrap();
    rig.browser(json!({"action": "navigate", "url": rig.url("/whoami")}))
        .await
        .unwrap();
    let before = rig.browser(json!({"action": "text"})).await.unwrap();
    assert!(before.contains("sid=shared-session-r1"), "{before}");
    // Idled out: the cookie is written to the profile as Chromium closes.
    rig.signin.stop().await;
    assert!(!rig.signin.is_running());
    rig.signin.unshare("SHARED", &[]).await.unwrap().unwrap();
    rig.signin.stop().await;
    // The restart: another broker's signed-in browser on the same profile.
    let again = Signin::new(
        Box::new(spawn::Direct::new(rig._root.path().join("signin"))),
        Box::new(Recorder::default()),
        rig.app.secrets.clone(),
        None,
    );
    again.notice_revocations().await;
    let running = again.browser().await.unwrap();
    let left = running
        .cdp
        .call("Storage.getCookies", json!({}))
        .await
        .unwrap();
    assert!(
        !left.to_string().contains("shared-session-r1"),
        "the revoked share's cookie came back: {left}"
    );
    again.stop().await;
}

/// The share's cookie on disk and in a running browser: a rig whose
/// browser stopped once in order (Chromium writes its cookies as it closes)
/// and started again from that profile.
async fn shared_and_written(cookie_value: &str) -> Rig {
    let rig = rig(login()).await;
    let site = Origin::site(&rig.url("")).unwrap();
    let cookie = json!({"name": "sid", "value": cookie_value, "domain": "localhost", "path": "/",
                        "expires": crate::jobs::now() / 1000 + 86_400});
    rig.signin
        .share("SHARED", vec![site], vec![cookie], serde_json::Map::new())
        .await
        .unwrap();
    rig.browser(json!({"action": "navigate", "url": rig.url("/whoami")}))
        .await
        .unwrap();
    rig.signin.stop().await;
    rig.browser(json!({"action": "navigate", "url": rig.url("/whoami")}))
        .await
        .unwrap();
    let shown = rig.browser(json!({"action": "text"})).await.unwrap();
    assert!(shown.contains(cookie_value), "{shown}");
    rig
}

/// The cookies another broker's browser starts with on the same profile
/// (a restart: nothing of this broker's memory), as text.
async fn after_a_restart(rig: &Rig) -> String {
    let again = Signin::new(
        Box::new(spawn::Direct::new(rig._root.path().join("signin"))),
        Box::new(Recorder::default()),
        rig.app.secrets.clone(),
        None,
    );
    again.notice_revocations().await;
    let running = again.browser().await.unwrap();
    let left = running
        .cdp
        .call("Storage.getCookies", json!({}))
        .await
        .unwrap();
    again.stop().await;
    left.to_string()
}

/// An idle stop that lands inside `forget`, after it found the browser
/// running and before its first DevTools call, saves every cookie (the
/// revoked one too) and closes: the call fails, and the revocation must
/// still hold, now and after a restart.
#[tokio::test]
async fn a_stop_inside_a_revocation_does_not_keep_the_revoked_session() {
    if !have_chromium() {
        return eprintln!("skipped: no chromium");
    }
    let _serial = SERIAL.lock().await;
    let rig = shared_and_written("shared-session-r2").await;
    rig.signin
        .0
        .stop_inside_forget
        .store(true, Ordering::SeqCst);
    let answer = rig.signin.unshare("SHARED", &[]).await;
    assert!(
        answer.is_err(),
        "the stop broke the DevTools calls: {answer:?}"
    );
    let kept = serde_json::to_string(&*rig.signin.0.kept.lock().unwrap()).unwrap();
    assert!(!kept.contains("shared-session-r2"), "{kept}");
    // Settled: started again, the profile cleared.
    assert!(rig.signin.is_running());
    rig.browser(json!({"action": "navigate", "url": rig.url("/whoami")}))
        .await
        .unwrap();
    let shown = rig.browser(json!({"action": "text"})).await.unwrap();
    assert!(!shown.contains("shared-session-r2"), "{shown}");
    rig.signin.stop().await;
    let left = after_a_restart(&rig).await;
    assert!(!left.contains("shared-session-r2"), "{left}");
}

/// Chromium writes a cookie's deletion lazily: a computer killed right
/// after a revocation (no orderly close) keeps the cookie in the profile.
/// The broker's list on disk takes it out at the next start, and goes once
/// the browser has closed in order.
#[tokio::test]
async fn a_revocation_outlives_a_kill_before_chromium_writes_it() {
    if !have_chromium() {
        return eprintln!("skipped: no chromium");
    }
    let _serial = SERIAL.lock().await;
    let rig = shared_and_written("shared-session-r3").await;
    rig.signin.unshare("SHARED", &[]).await.unwrap().unwrap();
    let list = rig._root.path().join("signin/revoked.json");
    assert!(list.exists(), "the revocation is on disk");
    // Killed: no Browser.close, and this broker's memory gone with it.
    let pid = rig.signin.browser().await.unwrap().pid;
    rig.signin.0.spawner.stop(pid);
    let left = after_a_restart(&rig).await;
    assert!(!left.contains("shared-session-r3"), "{left}");
    assert!(!list.exists(), "an orderly close takes the list away");
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
    rig.signin.notice_revocations().await;
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
    assert_eq!(rig.signin.flow_on(&rig.url("")), Some(flow.clone()));
    assert!(rig.signin.flow_on("https://evil.example").is_none());
    assert!(rig.signin.flow_on("http://127.0.0.1:1").is_none());
    // A second tab on the same origin: a request naming the origin could be
    // either tab's, so it is refused rather than given the first's flow.
    rig.browser(json!({"action": "tab_new", "url": rig.url("/home")}))
        .await
        .unwrap();
    assert!(rig.signin.flow_on(&rig.url("")).is_none());
    rig.browser(json!({"action": "tab_close"})).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    // And a new document of the one tab starts a flow of its own.
    rig.browser(json!({"action": "navigate", "url": rig.url("/login")}))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let later = rig.signin.flow_on(&rig.url("")).unwrap();
    assert_ne!(later, flow, "an approval on one page is not another page's");
    rig.signin.stop().await;
}

/// A passkey comes with its RP id alone: that host opens here as WebAuthn
/// scopes it, unless a login names a site on it, and goes with the grant.
#[test]
fn a_granted_passkeys_site_opens_here_unless_a_login_names_its_host() {
    let root = tempfile::tempdir().unwrap();
    let secrets = crate::secrets::Secrets::default();
    let signin = Signin::new(
        Box::new(spawn::Direct::new(root.path().join("signin"))),
        Box::new(Recorder::default()),
        secrets.clone(),
        None,
    );
    let passkey = json!({"kind": "passkey", "rpId": "localhost", "credentialId": "AQID",
        "privateKey": "MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgB9aEvdD1k7n55Wu+sDtf5cUbUumdw/XyKmgddnpO1eihRANCAASw/VbAXHGTIuqCMysLtvnlfwqu+ZFr7EQiwzWmLuynjO5Z/X+o4fuRzcMX916P1LxoWt4Ywc5KEVBuTzUzjBVI"});
    let deliver = |set: Value| {
        secrets
            .replace(serde_json::from_value(set).unwrap())
            .unwrap()
    };
    assert!(!signin.routes("http://localhost:8600/passkey"));
    deliver(json!({"PK": passkey}));
    assert!(signin.routes("http://localhost:8600/passkey"));
    assert!(signin.routes("https://localhost/"));
    assert!(!signin.routes("http://127.0.0.1:8600/"));
    let mut login = login()["SITE"].clone();
    login["sites"] = json!(["http://localhost:8000"]);
    deliver(json!({"PK": passkey, "SITE": login}));
    assert!(signin.routes("http://localhost:8000/login"));
    assert!(
        !signin.routes("http://localhost:8001/"),
        "a login names where its host signs in"
    );
    deliver(json!({}));
    assert!(!signin.routes("http://localhost:8600/passkey"));
}

/// A revocation is a login or a passkey taken away, or a site it no longer
/// names: not a grant that only changes how a host's site is spelled. In
/// the rc.3 ACP run, granting a login on a host whose passkey was granted
/// read as the passkey's site revoked, and signed the host out (and, with
/// it, cleared every cookie in the browser to set the others back).
#[tokio::test]
async fn a_login_granted_on_a_passkeys_host_revokes_nothing() {
    let root = tempfile::tempdir().unwrap();
    let secrets = crate::secrets::Secrets::default();
    let signin = Signin::new(
        Box::new(spawn::Direct::new(root.path().join("signin"))),
        Box::new(Recorder::default()),
        secrets.clone(),
        None,
    );
    let passkey = json!({"kind": "passkey", "rpId": "example.com", "credentialId": "AQID",
        "privateKey": "MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgB9aEvdD1k7n55Wu+sDtf5cUbUumdw/XyKmgddnpO1eihRANCAASw/VbAXHGTIuqCMysLtvnlfwqu+ZFr7EQiwzWmLuynjO5Z/X+o4fuRzcMX916P1LxoWt4Ywc5KEVBuTzUzjBVI"});
    let mut login = login()["SITE"].clone();
    login["sites"] = json!(["https://example.com"]);
    // The browser is not running: what a delivery revokes waits for its
    // next start, and is read here.
    let revoked = |signin: &Signin| -> Vec<String> {
        let mut sites: Vec<String> = std::mem::take(&mut *signin.0.revoked.lock().unwrap())
            .iter()
            .map(ToString::to_string)
            .collect();
        sites.sort();
        sites
    };
    let deliver = |set: Value| async {
        secrets
            .replace(serde_json::from_value(set).unwrap())
            .unwrap();
        signin.notice_revocations().await;
    };
    deliver(json!({"PK": passkey})).await;
    assert!(revoked(&signin).is_empty());
    deliver(json!({"PK": passkey, "SITE": login})).await;
    assert!(
        revoked(&signin).is_empty(),
        "a login granted on the passkey's host revoked nothing"
    );
    deliver(json!({"PK": passkey, "SITE": login})).await;
    assert!(revoked(&signin).is_empty(), "the same set again");
    deliver(json!({"PK": passkey})).await;
    assert_eq!(
        revoked(&signin),
        ["https://example.com"],
        "the login's site"
    );
    deliver(json!({"PK": passkey, "SITE": login})).await;
    deliver(json!({"SITE": login})).await;
    assert_eq!(
        revoked(&signin),
        ["https://example.com"],
        "the passkey's own site, though the login still names the host"
    );
    let mut moved = login.clone();
    moved["sites"] = json!(["https://login.example.com"]);
    deliver(json!({"SITE": moved})).await;
    assert_eq!(
        revoked(&signin),
        ["https://example.com"],
        "a site the login no longer names"
    );
}

/// An arming for a site on the computer itself opens that host on the
/// port its page is on, as the passkey it makes will once granted, and no
/// other host; a site on the web is opened as before, on https alone.
#[test]
fn an_armed_loopback_site_opens_here_on_its_own_port() {
    let root = tempfile::tempdir().unwrap();
    let secrets = crate::secrets::Secrets::default();
    let signin = Signin::new(
        Box::new(spawn::Direct::new(root.path().join("signin"))),
        Box::new(Recorder::default()),
        secrets.clone(),
        None,
    );
    let passkeys = crate::passkeys::Passkeys::default();
    signin.arming_from(passkeys.clone());
    let page = "http://localhost:8600/settings/passkeys";
    assert!(!signin.routes(page));
    passkeys.arm("localhost").unwrap();
    assert!(signin.routes(page));
    assert!(signin.routes("https://localhost:8443/"));
    assert!(!signin.routes("http://127.0.0.1:8600/"));
    assert!(!signin.routes("http://example.com/"));
    // A login that names where the host signs in is where the arming is.
    let mut login = login()["SITE"].clone();
    login["sites"] = json!(["http://localhost:8000"]);
    secrets
        .replace(serde_json::from_value(json!({"SITE": login})).unwrap())
        .unwrap();
    assert!(signin.routes("http://localhost:8000/settings/passkeys"));
    assert!(!signin.routes(page));
    secrets.replace(BTreeMap::new()).unwrap();
    passkeys.disarm();
    assert!(!signin.routes(page));
    passkeys.arm("github.com").unwrap();
    assert!(signin.routes("https://github.com/settings/security"));
    assert!(!signin.routes("https://github.com:8443/"));
    assert!(!signin.routes(page));
}

/// What the rc.3 ACP run hit: a site with a login and a passkey on one
/// host, signed in there, then armed for a passkey. The arming opened the
/// site in a second tab, and a passkey signs only on an origin one tab is
/// on, so every sign-in with the new passkey was refused until the login
/// was revoked and the site's tabs closed. The arming now opens no second
/// tab, and the passkey signs; a second tab there still refuses it.
#[tokio::test]
async fn a_passkey_signs_on_a_host_whose_login_signed_in_after_an_arming() {
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
    let mut key = serde_json::to_value(&passkey).unwrap();
    key["kind"] = json!("passkey");
    key["readable"] = json!(true);
    let rig = rig(login()).await;
    let mut site = login()["SITE"].clone();
    site["sites"] = json!([rig.url("")]);
    site["readable"] = json!(true);
    let mut set: BTreeMap<String, crate::secrets::Delivered> = BTreeMap::new();
    set.insert("SITE".into(), serde_json::from_value(site).unwrap());
    set.insert("PK".into(), serde_json::from_value(key).unwrap());
    rig.app.secrets.replace(set).unwrap();
    let bridge = tokio::spawn(crate::bridge::serve(rig.app.clone()));
    // Signed in by the login: the agent's tab is on the site.
    rig.browser(json!({"action": "navigate", "url": rig.url("/login")}))
        .await
        .unwrap();
    let user = rig.find("Username").await;
    let password = rig.find("Password").await;
    let button = rig.find("[button] Sign in").await;
    rig.browser(json!({"action": "fill", "ref": user, "secret": "SITE.username"}))
        .await
        .unwrap();
    rig.browser(json!({"action": "fill", "ref": password, "secret": "SITE.password"}))
        .await
        .unwrap();
    rig.browser(json!({"action": "click_ref", "ref": button}))
        .await
        .unwrap();
    rig.settle("Code").await;
    let on_site = |signin: &Signin| {
        signin
            .lock_tabs()
            .list
            .iter()
            .filter(|tab| tab.url.starts_with(&rig.url("")))
            .count()
    };
    assert_eq!(on_site(&rig.signin), 1);
    // The arming shows the person the site: on the tab already there.
    let running = rig.signin.browser().await.unwrap();
    rig.signin.open_once(&running, &rig.url("/")).await.unwrap();
    assert_eq!(on_site(&rig.signin), 1, "the arming opened a second tab");
    rig.browser(json!({"action": "navigate", "url": rig.url("/pk")}))
        .await
        .unwrap();
    let signed = rig.settle("signed public-key").await;
    assert!(!signed.contains("refused"), "{signed}");
    // Two tabs on the origin: which one asks cannot be told, so neither
    // gets a passkey. What the old arming made.
    let second = rig.signin.new_tab(&running).await.unwrap();
    rig.signin
        .navigate(&running.cdp, &second.session, &rig.url("/plain"))
        .await
        .unwrap();
    assert_eq!(on_site(&rig.signin), 2);
    rig.browser(json!({"action": "navigate", "url": rig.url("/pk")}))
        .await
        .unwrap();
    let refused = rig.settle("refused").await;
    assert!(refused.contains("NotAllowedError"), "{refused}");
    bridge.abort();
    rig.signin.stop().await;
}

/// What point 1 of the rc.3 QA could not do: arm a site the computer itself
/// serves, on a port of its own. The arming readies the signed-in browser
/// and loads nothing (no `https://localhost/`, where nothing listens); the
/// site's own page opens there, and its `create()` waits for the person
/// with the page's real origin on the card.
#[tokio::test]
async fn a_loopback_site_is_armed_and_its_page_asks_the_person() {
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
    let rig = rig(json!({})).await;
    rig.signin.arming_from(rig.app.passkeys.clone());
    let bridge = tokio::spawn(crate::bridge::serve(rig.app.clone()));
    rig.app.passkeys.arm("localhost").unwrap();
    crate::split::sync_passkeys(&rig.app, true)
        .await
        .expect("a loopback site is armed");
    let loaded: Vec<String> = rig
        .signin
        .lock_tabs()
        .list
        .iter()
        .map(|tab| tab.url.clone())
        .filter(|url| url.starts_with("http"))
        .collect();
    assert!(loaded.is_empty(), "the arming loaded {loaded:?}");
    rig.browser(json!({"action": "navigate", "url": rig.url("/mk")}))
        .await
        .unwrap();
    let mut asked = None;
    for _ in 0..200 {
        asked = rig.app.passkeys.asked();
        if asked.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let (ask, answer) = asked.expect("the page's create() reached the person");
    assert_eq!(ask.rp_id, "localhost");
    assert_eq!(ask.origin, rig.url(""));
    assert_eq!(answer, None, "nobody answered for the person");
    assert!(rig.app.passkeys.minted().is_none());
    rig.app.passkeys.approve(&ask.id).unwrap();
    let made = rig.settle("made public-key").await;
    assert!(!made.contains("refused"), "{made}");
    assert_eq!(
        rig.app.passkeys.minted().map(|key| key.rp_id).as_deref(),
        Some("localhost")
    );
    assert!(
        rig.general.0.lock().unwrap().is_empty(),
        "nothing went to the general browser"
    );
    bridge.abort();
    rig.signin.stop().await;
}

/// With nothing granted but a passkey, its site opens here, not in the
/// general browser, and the passkey signs in on that tab.
#[tokio::test]
async fn a_passkey_alone_opens_its_site_here_and_signs_in() {
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
    let mut key = serde_json::to_value(&passkey).unwrap();
    key["kind"] = json!("passkey");
    // The test site is on loopback: granted readable, as a person would
    // have to for a site the computer itself serves.
    key["readable"] = json!(true);
    let rig = rig(json!({})).await;
    let mut set: BTreeMap<String, crate::secrets::Delivered> = BTreeMap::new();
    set.insert("PK".into(), serde_json::from_value(key).unwrap());
    rig.app.secrets.replace(set).unwrap();
    let bridge = tokio::spawn(crate::bridge::serve(rig.app.clone()));
    rig.browser(json!({"action": "navigate", "url": rig.url("/pk")}))
        .await
        .unwrap();
    let signed = rig.settle("signed public-key").await;
    assert!(!signed.contains("refused"), "{signed}");
    assert!(rig.signin.flow_on(&rig.url("")).is_some());
    assert!(
        rig.general.0.lock().unwrap().is_empty(),
        "nothing went to the general browser"
    );
    bridge.abort();
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
    // The test site is on loopback: granted readable, as a person would
    // have to for a site the computer itself serves.
    key["readable"] = json!(true);
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
    // The fill rule refuses the field at once: its form sends by GET, and
    // elsewhere. Nothing is typed.
    let refused = rig
        .browser(json!({"action": "fill", "ref": password, "secret": "SITE.password"}))
        .await
        .unwrap_err();
    assert!(refused.contains("sends by GET"), "{refused}");
    // Sent empty, it is a plain link elsewhere: it opens in the general
    // browser, carrying nothing.
    rig.browser(json!({"action": "click_ref", "ref": button}))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let opened = rig.general.0.lock().unwrap().clone();
    assert!(
        opened.iter().all(|url| url.ends_with("/form?p=")),
        "only the empty form went to the general browser: {opened:?}"
    );
    // Typed into its own site's form, which the page then sends elsewhere
    // by GET: the load is stopped, and no server or browser gets it.
    rig.general.0.lock().unwrap().clear();
    rig.browser(json!({"action": "navigate", "url": rig.url("/turncoat")}))
        .await
        .unwrap();
    let password = rig.find("Password").await;
    let button = rig.find("[button] Sign in").await;
    rig.browser(json!({"action": "fill", "ref": password, "secret": "SITE.password"}))
        .await
        .unwrap();
    rig.browser(json!({"action": "click_ref", "ref": button}))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    // Neither sent on to its own /totp (the code page) nor anywhere else:
    // the tab still shows the sign-in page.
    let running = rig.signin.browser().await.unwrap();
    let session = rig.signin.lock_tabs().list[0].session.clone();
    let history = running
        .cdp
        .on(&session, "Page.getNavigationHistory", json!({}))
        .await
        .unwrap();
    let index = history["currentIndex"].as_u64().unwrap() as usize;
    assert!(
        history["entries"][index]["url"]
            .as_str()
            .unwrap()
            .ends_with("/turncoat"),
        "the sign-in page stays: {history}"
    );
    assert!(!history.to_string().contains(PASSWORD), "{history}");
    assert!(
        rig.general.0.lock().unwrap().is_empty(),
        "nothing went to the general browser"
    );
    assert!(
        !heard_password(rig.off_site),
        "the other site heard the password"
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

/// A sign-in page a script builds, with no form (Google's): the password
/// goes in, as into a POST form, and the page is withheld until it moves on.
#[tokio::test]
async fn a_password_field_a_script_built_with_no_form_is_typed() {
    if !have_chromium() {
        return eprintln!("skipped: no chromium");
    }
    let _serial = SERIAL.lock().await;
    let rig = rig(login()).await;
    rig.browser(json!({"action": "navigate", "url": rig.url("/script")}))
        .await
        .unwrap();
    let password = rig.find("Password").await;
    let filled = rig
        .browser(json!({"action": "fill", "ref": password, "secret": "SITE.password"}))
        .await
        .unwrap();
    assert!(filled.contains("hidden from you"), "{filled}");
    let hidden = rig.browser(json!({"action": "text"})).await.unwrap();
    assert!(hidden.contains("middle of signing in"), "{hidden}");
    rig.signin.stop().await;
}

/// A form with no method sends by GET. A script-built one is sent by its
/// script, so the password is typed; and if the browser sends it itself
/// (here: no script at all), the load carrying it is stopped before it
/// leaves: no server hears it, no address or history shows it, and the
/// sign-in page stays.
#[tokio::test]
async fn a_password_in_a_get_form_is_typed_but_never_sent_in_an_address() {
    if !have_chromium() {
        return eprintln!("skipped: no chromium");
    }
    let _serial = SERIAL.lock().await;
    let rig = rig(login()).await;
    rig.browser(json!({"action": "navigate", "url": rig.url("/noform-get")}))
        .await
        .unwrap();
    let password = rig.find("Password").await;
    let button = rig.find("[button] Sign in").await;
    let filled = rig
        .browser(json!({"action": "fill", "ref": password, "secret": "SITE.password"}))
        .await
        .unwrap();
    assert!(filled.contains("hidden from you"), "{filled}");
    rig.browser(json!({"action": "click_ref", "ref": button}))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    // The other tests post it to the site's own /totp; no request line, a
    // GET's address, may carry it.
    let heard = HEARD.lock().unwrap().clone();
    assert!(
        heard.iter().all(|(_, request)| !request
            .lines()
            .next()
            .unwrap_or_default()
            .contains(PASSWORD)),
        "a server heard the password in an address: {heard:?}"
    );
    let running = rig.signin.browser().await.unwrap();
    let session = rig.signin.lock_tabs().list[0].session.clone();
    let history = running
        .cdp
        .on(&session, "Page.getNavigationHistory", json!({}))
        .await
        .unwrap();
    assert!(
        !history.to_string().contains(PASSWORD),
        "an address carries the password: {history}"
    );
    let index = history["currentIndex"].as_u64().unwrap() as usize;
    assert!(
        history["entries"][index]["url"]
            .as_str()
            .unwrap()
            .ends_with("/noform-get"),
        "the sign-in page stays: {history}"
    );
    let tabs = rig.browser(json!({"action": "tabs"})).await.unwrap();
    assert!(!tabs.contains(PASSWORD), "{tabs}");
    assert!(rig.general.0.lock().unwrap().is_empty());
    rig.signin.stop().await;
}

/// A page that adds its Next button as the name is typed (X's) changes
/// nothing about where the form sends: the username stays typed.
#[tokio::test]
async fn a_button_the_page_adds_while_typing_keeps_the_fill() {
    if !have_chromium() {
        return eprintln!("skipped: no chromium");
    }
    let _serial = SERIAL.lock().await;
    let rig = rig(login()).await;
    rig.browser(json!({"action": "navigate", "url": rig.url("/grows")}))
        .await
        .unwrap();
    let username = rig.find("Username").await;
    let filled = rig
        .browser(json!({"action": "fill", "ref": username, "secret": "SITE.username"}))
        .await
        .unwrap();
    assert_eq!(filled, format!("filled {username} with SITE.username"));
    let shown = rig.browser(json!({"action": "text"})).await.unwrap();
    assert!(
        shown.contains("value=\"george\"") && shown.contains("Next"),
        "{shown}"
    );
    rig.signin.stop().await;
}

/// The stop knows a value as UTF-8 spells it in an address. A page in
/// another charset, or a form sending one, would spell it otherwise: its
/// GET form is refused as before.
#[tokio::test]
async fn a_get_form_on_a_page_of_another_charset_is_refused() {
    if !have_chromium() {
        return eprintln!("skipped: no chromium");
    }
    let _serial = SERIAL.lock().await;
    let rig = rig(login()).await;
    for page in ["/latin", "/latin-form"] {
        rig.browser(json!({"action": "navigate", "url": rig.url(page)}))
            .await
            .unwrap();
        let password = rig.find("Password").await;
        let refused = rig
            .browser(json!({"action": "fill", "ref": password, "secret": "SITE.password"}))
            .await
            .unwrap_err();
        assert!(refused.contains("sends by GET"), "{page}: {refused}");
    }
    rig.signin.stop().await;
}

/// An image button is a submit button form.elements never lists: one that
/// sends the form elsewhere refuses the fill, and one a page adds after the
/// password is typed, posting into a frame, is stopped before it leaves.
#[tokio::test]
async fn an_image_button_never_posts_a_password_elsewhere() {
    if !have_chromium() {
        return eprintln!("skipped: no chromium");
    }
    let _serial = SERIAL.lock().await;
    let rig = rig(login()).await;
    rig.browser(json!({"action": "navigate", "url": rig.url("/image")}))
        .await
        .unwrap();
    let password = rig.find("Password").await;
    let refused = rig
        .browser(json!({"action": "fill", "ref": password, "secret": "SITE.password"}))
        .await
        .unwrap_err();
    assert!(refused.contains("sends elsewhere"), "{refused}");
    rig.browser(json!({"action": "navigate", "url": rig.url("/image-later")}))
        .await
        .unwrap();
    let password = rig.find("Password").await;
    rig.browser(json!({"action": "fill", "ref": password, "secret": "SITE.password"}))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    assert!(
        !heard_password(rig.off_site),
        "the frame posted the password to the other site"
    );
    rig.signin.stop().await;
}
