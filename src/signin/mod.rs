//! The signed-in browser: a second, headless Chromium that only the broker
//! drives, over a DevTools pipe, for the sites the teammate is signed in to.
//!
//! The general browser stays the agent's: `eval`, DevTools on 9222, an open
//! network, and nothing signed in. A granted login's sites and the sites the
//! person shared cookies for are opened here instead. The agent reaches them
//! through site actions alone (`navigate`, `click_ref`, `fill`, `select`,
//! `check`, `hover`, `text`, `links`, `read`, `screenshot`, history, tabs,
//! `upload`), which the broker carries out with its own scripts in an isolated
//! world; no script of the agent's runs here, so `eval` is refused. Every
//! top-level navigation is judged by `lock` before it leaves.
//!
//! A sign-in is the broker's: it types the username, password and code
//! itself, and from the first secret typed until the tab loads its next
//! document nothing of that tab is shown to the agent: no text, links,
//! screenshot, form answer or tab title. The same holds while the person
//! drives the site screen (`screen`) and after they typed into it. When a
//! sign-in does not finish on its own, the agent is told to ask the person to
//! finish signing in on the site screen.
//!
//! It starts on first use and stops when idle; the sessions it holds are
//! kept in the broker's memory across a stop. Revoking a login or a share
//! (`sessions`) clears the site's cookies and storage and closes its tabs.

pub mod cdp;
pub mod lock;
pub mod screen;
pub mod sessions;
pub mod spawn;
pub mod split;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use rmcp::model::ContentBlock;
use serde_json::{Value, json};

use crate::App;
use crate::holds::{Bound, Operation, OperationKind, Pass};
use crate::secrets::{Origin, Secrets, Use};
use crate::tools::ToolResult;
use cdp::Cdp;
use lock::Hop;
use spawn::Spawner;

/// The signed-in browser's tabs are listed, and selected, from this index on,
/// after the general browser's.
pub const SIGNED_IN_TABS: usize = 100;
/// A browser nobody used for this long is stopped.
const IDLE: Duration = Duration::from_secs(10 * 60);
/// How long a withheld sign-in may sit before the agent is told to hand it
/// to the person.
const FINISH_AFTER_MS: u64 = 30_000;
/// The person's input holds the site for this long after it stops.
const PERSON_HOLDS_MS: u64 = 10_000;
/// How long a page gets to load before an action answers anyway.
const LOAD_PATIENCE: Duration = Duration::from_secs(20);
const VIEWPORT: (u32, u32) = (1280, 800);

/// Whether this computer keeps a signed-in browser, so that the general
/// browser carries no extension and no passkey. The split computer always
/// does (its broker); a single-user one only to try it out (`trial`).
pub fn enabled() -> bool {
    crate::split::mode() == crate::split::Mode::Split || trial()
}

/// `HOTLINE_COMPUTER_SIGNIN=direct` on a single-user computer: a signed-in
/// browser as the one user, which protects nothing by itself.
pub fn trial() -> bool {
    crate::split::mode() == crate::split::Mode::Legacy
        && std::env::var("HOTLINE_COMPUTER_SIGNIN").as_deref() == Ok("direct")
}

/// A file staged for the signed-in browser to upload.
pub type Staged =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<PathBuf, String>> + Send>>;

/// What the signed-in browser asks of the agent's side: in the single-user
/// computer the browser in this process; in the split computer the runner.
pub trait General: Send + Sync {
    /// Opens a plain link from a signed-in site in the general browser.
    fn open(&self, url: String);
    /// A file of the agent's, put where the signed-in browser can read it.
    fn stage(&self, path: &std::path::Path) -> Staged {
        let path = path.to_owned();
        Box::pin(async move { Ok(path) })
    }
}

/// The general browser in this process.
pub struct InProcess(pub crate::browser::BrowserManager);

impl General for InProcess {
    fn open(&self, url: String) {
        let browser = self.0.clone();
        tokio::spawn(async move {
            if let Err(error) = browser.tab_new(&url).await {
                eprintln!("signin: the general browser did not open a link: {error}");
            }
        });
    }
}

#[derive(Clone)]
pub struct Signin(Arc<Inner>);

struct Inner {
    spawner: Box<dyn Spawner>,
    general: Box<dyn General>,
    secrets: Secrets,
    running: tokio::sync::Mutex<Option<Running>>,
    tabs: Mutex<Tabs>,
    /// Shared sites by share name: what `PUT /sessions` brought.
    sessions: Mutex<BTreeMap<String, Vec<Origin>>>,
    /// The login sites as of the last look, to notice one revoked.
    logins: Mutex<Vec<Origin>>,
    /// The making of a passkey: the armed site opens here while it lasts.
    arming: Mutex<Option<crate::passkeys::Passkeys>>,
    /// The sessions held across an idle stop, as DevTools reported them.
    kept: Mutex<Vec<Value>>,
    /// Whether the agent's browser calls go here.
    focused: AtomicBool,
    last_used: AtomicU64,
    /// When the person last gave input on the site screen.
    person_at: AtomicU64,
    viewers: AtomicUsize,
    downloads: Option<PathBuf>,
}

#[derive(Clone)]
struct Running {
    cdp: Cdp,
    pid: u32,
    user_agent: String,
}

#[derive(Default)]
struct Tabs {
    list: Vec<Tab>,
    current: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct Tab {
    target: String,
    session: String,
    /// Set up and running: its documents are judged and it may be driven.
    ready: bool,
    url: String,
    title: String,
    /// The sequence number of the last secret typed here, of the person's
    /// last keystroke here, and of the last new document.
    /// The document (DevTools' loader id) a secret was typed into, and
    /// the one the person typed into: while the tab still shows it, nothing
    /// of the tab is shown to the agent. Only a look at the browser's own
    /// answer clears them, never an event that may arrive late.
    secret_doc: Option<String>,
    person_doc: Option<String>,
    secret_at: u64,
    flow: Option<Flow>,
    /// What happened to the tab that the agent hears with its next action.
    notes: Vec<String>,
}

#[derive(Clone, Debug)]
struct Flow {
    id: String,
    sites: Vec<Origin>,
}

impl Tab {
    fn withheld(&self) -> bool {
        self.secret_doc.is_some() || self.person_doc.is_some()
    }
}

impl Signin {
    pub fn new(
        spawner: Box<dyn Spawner>,
        general: Box<dyn General>,
        secrets: Secrets,
        downloads: Option<PathBuf>,
    ) -> Self {
        Self(Arc::new(Inner {
            spawner,
            general,
            secrets,
            running: tokio::sync::Mutex::new(None),
            tabs: Mutex::default(),
            sessions: Mutex::default(),
            logins: Mutex::default(),
            kept: Mutex::default(),
            arming: Mutex::default(),
            focused: AtomicBool::new(false),
            last_used: AtomicU64::new(0),
            person_at: AtomicU64::new(0),
            viewers: AtomicUsize::new(0),
            downloads,
        }))
    }

    /// The single-user computer's trial: Chromium as this user, its things
    /// under the temporary directory.
    pub fn direct(
        app_browser: crate::browser::BrowserManager,
        secrets: Secrets,
        home: &std::path::Path,
    ) -> Self {
        Self::new(
            Box::new(spawn::Direct::new(
                std::env::temp_dir().join("hotline-computer/signin"),
            )),
            Box::new(InProcess(app_browser)),
            secrets,
            Some(home.join("Downloads")),
        )
    }

    pub fn bridge(&self) -> spawn::Bridge {
        self.0.spawner.bridge()
    }

    fn touch(&self) {
        self.0
            .last_used
            .store(crate::jobs::now(), Ordering::Relaxed);
    }

    fn lock_tabs(&self) -> std::sync::MutexGuard<'_, Tabs> {
        self.0
            .tabs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    // ---- sites ---------------------------------------------------------

    /// The sites of the logins granted now.
    fn login_sites(&self) -> Vec<Origin> {
        let mut sites = Vec::new();
        for entry in self.0.secrets.catalog() {
            if entry["kind"] == "login" {
                for site in entry["sites"].as_array().into_iter().flatten() {
                    if let Some(origin) = site.as_str().and_then(|site| Origin::site(site).ok()) {
                        sites.push(origin);
                    }
                }
            }
        }
        sites
    }

    fn shared_sites(&self) -> Vec<Origin> {
        self.0
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .flatten()
            .cloned()
            .collect()
    }

    /// Every site that opens here: the logins', the shares', and the site
    /// a passkey is being made for while the person has it armed.
    pub fn sites(&self) -> Vec<Origin> {
        let mut sites = self.login_sites();
        sites.extend(self.shared_sites());
        let armed = self
            .0
            .arming
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(crate::passkeys::Passkeys::armed);
        if let Some(site) =
            armed.and_then(|(rp_id, _)| Origin::site(&format!("https://{rp_id}")).ok())
        {
            sites.push(site);
        }
        sites
    }

    /// Where the person arms the making of a passkey.
    pub fn arming_from(&self, passkeys: crate::passkeys::Passkeys) {
        *self
            .0
            .arming
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(passkeys);
    }

    /// Opens `url` in a new signed-in tab, for the person: the armed site.
    pub async fn open_site(&self, url: &str) -> Result<(), String> {
        let running = self.browser().await?;
        let tab = self.new_tab(&running).await?;
        self.navigate(&running.cdp, &tab.session, url).await
    }

    /// Whether `url` opens in the signed-in browser.
    pub fn routes(&self, url: &str) -> bool {
        lock::signed_in(url, &self.sites())
    }

    // ---- the browser's life --------------------------------------------

    /// The running browser, started if it is not.
    async fn browser(&self) -> Result<Running, String> {
        self.touch();
        let mut running = self.0.running.lock().await;
        if let Some(current) = running.as_ref() {
            if !current.cdp.is_closed() {
                return Ok(current.clone());
            }
            self.0.spawner.stop(current.pid);
            *running = None;
            self.lock_tabs().list.clear();
        }
        let started = self.launch().await?;
        *running = Some(started.clone());
        Ok(started)
    }

    fn arguments(&self) -> Vec<String> {
        let mut arguments: Vec<String> = [
            "--headless=new",
            "--remote-debugging-pipe",
            // The dropped capabilities rule out Chromium's own sandbox, as
            // for the general browser.
            "--no-sandbox",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-background-networking",
            "--disable-component-update",
            "--disable-sync",
            "--disable-breakpad",
            "--metrics-recording-only",
            // A page brought back from the back-forward cache still holds
            // what was typed into it, so every document here is new.
            "--disable-features=BackForwardCache,MediaRouter,OptimizationHints,Translate",
            "--test-third-party-cookie-phaseout",
            "--disable-blink-features=AutomationControlled",
            "--password-store=basic",
            "--lang=en-US",
        ]
        .map(str::to_owned)
        .into();
        arguments.push(format!("--window-size={},{}", VIEWPORT.0, VIEWPORT.1));
        arguments.push(format!(
            "--user-data-dir={}",
            self.0.spawner.profile().display()
        ));
        // The extension that answers passkeys is loaded here and nowhere
        // else: a page in the general browser finds no passkey at all.
        if crate::bridge::extension_answers() {
            let extension = crate::bridge::EXTENSION_DIR;
            arguments.push(format!("--load-extension={extension}"));
            arguments.push(format!("--disable-extensions-except={extension}"));
        } else {
            arguments.push("--disable-extensions".into());
        }
        arguments.push("about:blank".into());
        arguments
    }

    async fn launch(&self) -> Result<Running, String> {
        let spawned = self.0.spawner.spawn(&self.arguments())?;
        let cdp = Cdp::over(spawned.write, spawned.read);
        let version = match cdp.call("Browser.getVersion", json!({})).await {
            Ok(version) => version,
            Err(error) => {
                self.0.spawner.stop(spawned.pid);
                return Err(format!("the signed-in browser did not start: {error}"));
            }
        };
        // Sites turn a headless browser away by its name alone.
        let user_agent = version["userAgent"]
            .as_str()
            .unwrap_or_default()
            .replace("HeadlessChrome", "Chrome");
        let running = Running {
            cdp: cdp.clone(),
            pid: spawned.pid,
            user_agent,
        };
        self.lock_tabs().list.clear();
        tokio::spawn(self.clone().events(running.clone()));
        cdp.call("Target.setDiscoverTargets", json!({"discover": true}))
            .await?;
        // Every new tab waits for the broker before its first request, so
        // its navigations are judged from the start.
        cdp.call(
            "Target.setAutoAttach",
            json!({"autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true}),
        )
        .await?;
        if let Some(downloads) = &self.0.downloads {
            let _ = cdp
                .call(
                    "Browser.setDownloadBehavior",
                    json!({"behavior": "allow", "downloadPath": downloads, "eventsEnabled": true}),
                )
                .await;
        }
        // What a stop kept, and what the profile kept, less any site no
        // longer granted or shared.
        let sites = self.sites();
        let mut cookies: Vec<Value> = std::mem::take(
            &mut *self
                .0
                .kept
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        if let Ok(stored) = cdp.call("Storage.getCookies", json!({})).await {
            cookies.extend(stored["cookies"].as_array().cloned().unwrap_or_default());
        }
        cdp.call("Storage.clearCookies", json!({})).await?;
        let kept: Vec<Value> = cookies
            .iter()
            .filter(|cookie| cookie_within(cookie, &sites))
            .map(cookie_param)
            .collect();
        if !kept.is_empty() {
            cdp.call("Storage.setCookies", json!({"cookies": kept}))
                .await?;
        }
        // The tab Chromium opened with, attached by hand if auto-attach
        // left it.
        let started = std::time::Instant::now();
        loop {
            let targets = cdp.call("Target.getTargets", json!({})).await?;
            for target in targets["targetInfos"].as_array().into_iter().flatten() {
                let id = target["targetId"].as_str().unwrap_or_default();
                let known = self.lock_tabs().list.iter().any(|tab| tab.target == id);
                if target["type"] == "page" && !known && target["attached"] != true {
                    let _ = cdp
                        .call(
                            "Target.attachToTarget",
                            json!({"targetId": id, "flatten": true}),
                        )
                        .await;
                }
            }
            if self.lock_tabs().list.iter().any(|tab| tab.ready)
                || started.elapsed() > Duration::from_secs(5)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        tokio::spawn(self.clone().watch(running.clone()));
        Ok(running)
    }

    /// Stops the browser, keeping its sessions in the broker's memory.
    pub async fn stop(&self) {
        let mut running = self.0.running.lock().await;
        if let Some(current) = running.take() {
            if let Ok(stored) = current.cdp.call("Storage.getCookies", json!({})).await {
                *self
                    .0
                    .kept
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    stored["cookies"].as_array().cloned().unwrap_or_default();
            }
            let _ = current.cdp.call("Browser.close", json!({})).await;
            self.0.spawner.stop(current.pid);
        }
        let mut tabs = self.lock_tabs();
        tabs.list.clear();
        tabs.current = None;
    }

    pub fn is_running(&self) -> bool {
        self.0
            .running
            .try_lock()
            .map(|running| running.as_ref().is_some_and(|r| !r.cdp.is_closed()))
            .unwrap_or(true)
    }

    /// While the browser runs: stops it when idle, and signs a revoked
    /// login's sites out.
    async fn watch(self, running: Running) {
        *self
            .0
            .logins
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = self.login_sites();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if running.cdp.is_closed() {
                return;
            }
            let now = self.login_sites();
            let before = std::mem::replace(
                &mut *self
                    .0
                    .logins
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                now.clone(),
            );
            let still = self.sites();
            let gone: Vec<Origin> = before
                .into_iter()
                .filter(|site| !still.contains(site))
                .collect();
            if !gone.is_empty() {
                let _ = self.forget(&gone).await;
            }
            let idle = crate::jobs::now().saturating_sub(self.0.last_used.load(Ordering::Relaxed))
                > IDLE.as_millis() as u64;
            if idle && self.0.viewers.load(Ordering::Relaxed) == 0 {
                self.stop().await;
                return;
            }
        }
    }

    // ---- events ----------------------------------------------------------

    async fn events(self, running: Running) {
        let mut events = running.cdp.events();
        loop {
            let event = match events.recv().await {
                Ok(event) => event,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            };
            self.event(&running, event).await;
        }
    }

    async fn event(&self, running: &Running, event: cdp::Event) {
        let params = &event.params;
        match event.method.as_str() {
            "Target.attachedToTarget" => {
                let session = params["sessionId"].as_str().unwrap_or_default().to_owned();
                let info = &params["targetInfo"];
                let target = info["targetId"].as_str().unwrap_or_default().to_owned();
                if info["type"] != "page" {
                    let _ = running
                        .cdp
                        .on(&session, "Runtime.runIfWaitingForDebugger", json!({}))
                        .await;
                    return;
                }
                let attached_twice = {
                    let mut tabs = self.lock_tabs();
                    let twice = tabs.list.iter().any(|tab| tab.target == target);
                    if !twice {
                        tabs.list.push(Tab {
                            target: target.clone(),
                            session: session.clone(),
                            url: info["url"].as_str().unwrap_or_default().to_owned(),
                            title: info["title"].as_str().unwrap_or_default().to_owned(),
                            ..Tab::default()
                        });
                    }
                    twice
                };
                if attached_twice {
                    let _ = running
                        .cdp
                        .call("Target.detachFromTarget", json!({"sessionId": session}))
                        .await;
                    return;
                }
                let this = self.clone();
                let running = running.clone();
                tokio::spawn(async move { this.set_up(&running, &target, &session).await });
            }
            "Target.targetInfoChanged" => {
                let info = &params["targetInfo"];
                let target = info["targetId"].as_str().unwrap_or_default();
                if let Some(tab) = self
                    .lock_tabs()
                    .list
                    .iter_mut()
                    .find(|t| t.target == target)
                {
                    tab.title = info["title"].as_str().unwrap_or_default().to_owned();
                }
            }
            "Target.targetDestroyed" | "Target.detachedFromTarget" => {
                let target = params["targetId"].as_str().unwrap_or_default().to_owned();
                let session = params["sessionId"].as_str().unwrap_or_default();
                let mut tabs = self.lock_tabs();
                tabs.list.retain(|tab| {
                    tab.target != target && (session.is_empty() || tab.session != session)
                });
                if tabs
                    .current
                    .as_ref()
                    .is_some_and(|current| !tabs.list.iter().any(|tab| &tab.target == current))
                {
                    tabs.current = tabs.list.last().map(|tab| tab.target.clone());
                }
            }
            "Fetch.requestPaused" => {
                if let Some(session) = event.session.as_deref() {
                    self.judge(running, session, params).await;
                }
            }
            "Page.frameNavigated" => {
                let frame = &params["frame"];
                if frame.get("parentId").is_some() {
                    return;
                }
                let url = format!(
                    "{}{}",
                    frame["url"].as_str().unwrap_or_default(),
                    frame["urlFragment"].as_str().unwrap_or_default()
                );
                if let Some(tab) = self
                    .lock_tabs()
                    .list
                    .iter_mut()
                    .find(|tab| Some(tab.session.as_str()) == event.session.as_deref())
                {
                    tab.url = url.clone();
                    if tab
                        .flow
                        .as_ref()
                        .is_some_and(|flow| !lock::signed_in(&url, &flow.sites))
                    {
                        tab.flow = None;
                    }
                }
            }
            _ => {}
        }
    }

    /// A new tab: judged from its first request, told to look like a
    /// desktop browser, then let go.
    async fn set_up(&self, running: &Running, target: &str, session: &str) {
        let cdp = &running.cdp;
        let _ = cdp.on(session, "Page.enable", json!({})).await;
        let _ = cdp
            .on(
                session,
                "Fetch.enable",
                json!({"patterns": [{"urlPattern": "*", "resourceType": "Document", "requestStage": "Request"}]}),
            )
            .await;
        let _ = cdp
            .on(
                session,
                "Emulation.setUserAgentOverride",
                json!({"userAgent": running.user_agent, "acceptLanguage": "en-US,en"}),
            )
            .await;
        let _ = cdp
            .on(
                session,
                "Network.setCookieControls",
                json!({"enableThirdPartyCookieRestriction": true}),
            )
            .await;
        let _ = cdp
            .on(session, "Runtime.runIfWaitingForDebugger", json!({}))
            .await;
        let mut tabs = self.lock_tabs();
        if let Some(tab) = tabs.list.iter_mut().find(|tab| tab.target == target) {
            tab.ready = true;
        }
        if tabs.current.is_none() {
            tabs.current = Some(target.to_owned());
        }
    }

    async fn judge(&self, running: &Running, session: &str, params: &Value) {
        let request = params["requestId"].as_str().unwrap_or_default();
        let main = {
            let tabs = self.lock_tabs();
            tabs.list
                .iter()
                .find(|tab| tab.session == session)
                .is_some_and(|tab| params["frameId"].as_str() == Some(tab.target.as_str()))
        };
        let url = format!(
            "{}{}",
            params["request"]["url"].as_str().unwrap_or_default(),
            params["request"]["urlFragment"]
                .as_str()
                .unwrap_or_default()
        );
        let hop = if main {
            lock::judge(
                &url,
                params["request"]["method"].as_str().unwrap_or("GET"),
                &self.sites(),
            )
        } else {
            // Frames inside a signed-in page load as the page asks.
            Hop::Stay
        };
        // A plain link is only plain if it carries nothing typed here: not a
        // secret's value (a sign-in form sent off the site by GET), and
        // nothing from a tab that is in the middle of signing in.
        let signing_in = self
            .lock_tabs()
            .list
            .iter()
            .any(|tab| tab.session == session && tab.withheld());
        let hop = match hop {
            Hop::General if signing_in || self.0.secrets.redact(&url) != url => {
                Hop::Blocked(format!(
                    "a page in the middle of signing in tried to send you to {}; it was stopped",
                    crate::bridge::host_of(&url)
                ))
            }
            hop => hop,
        };
        let cdp = &running.cdp;
        let note = match hop {
            Hop::Stay => {
                let _ = cdp
                    .on(
                        session,
                        "Fetch.continueRequest",
                        json!({"requestId": request}),
                    )
                    .await;
                return;
            }
            Hop::General => {
                self.0.general.open(url.clone());
                format!(
                    "{} is not a signed-in site, so it opened in the general browser instead",
                    crate::bridge::host_of(&url)
                )
            }
            Hop::Blocked(why) => why,
        };
        let _ = cdp
            .on(
                session,
                "Fetch.failRequest",
                json!({"requestId": request, "errorReason": "BlockedByClient"}),
            )
            .await;
        if let Some(tab) = self
            .lock_tabs()
            .list
            .iter_mut()
            .find(|tab| tab.session == session)
        {
            tab.notes.push(note);
        }
    }

    // ---- routing -------------------------------------------------------

    /// The agent's `browser` call, when it is for the signed-in browser:
    /// `None` leaves it to the general one.
    pub async fn route(&self, app: &App, arguments: &Value, holder: &str) -> Option<ToolResult> {
        let action = arguments["action"].as_str()?;
        let url = arguments["url"].as_str().unwrap_or_default();
        let secret = arguments["secret"]
            .as_str()
            .filter(|secret| !secret.is_empty());
        let index = arguments["index"].as_u64().map(|index| index as usize);
        let here = match action {
            "navigate" | "tab_new" => {
                let here = !url.is_empty() && self.routes(url);
                self.0.focused.store(here, Ordering::SeqCst);
                here
            }
            // A login's field is only ever typed here, whatever was in front.
            "fill" if secret.is_some_and(|secret| self.is_login(secret)) => true,
            "tab_select" => index.is_some_and(|index| index >= SIGNED_IN_TABS),
            "tab_close" => index.map_or_else(
                || self.0.focused.load(Ordering::SeqCst),
                |index| index >= SIGNED_IN_TABS,
            ),
            "tabs" | "downloads" => false,
            _ => self.0.focused.load(Ordering::SeqCst),
        };
        if action == "tab_select" && !here {
            self.0.focused.store(false, Ordering::SeqCst);
        }
        if !here {
            return None;
        }
        self.0.focused.store(true, Ordering::SeqCst);
        Some(self.act(app, action, arguments, holder).await)
    }

    fn is_login(&self, reference: &str) -> bool {
        let name = reference.split('.').next().unwrap_or(reference);
        self.0
            .secrets
            .catalog()
            .iter()
            .any(|entry| entry["name"] == name && entry["kind"] == "login")
    }

    /// The signed-in tabs, for `tabs` to list after the general browser's.
    /// A withheld tab shows its site alone.
    pub fn tab_list(&self) -> Vec<Value> {
        let focused = self.0.focused.load(Ordering::SeqCst);
        let person = self.person_holds();
        let tabs = self.lock_tabs();
        tabs.list
            .iter()
            .filter(|tab| tab.ready)
            .enumerate()
            .map(|(index, tab)| {
                let current = focused && tabs.current.as_ref() == Some(&tab.target);
                let site = crate::bridge::host_of(&tab.url);
                if tab.withheld() || person {
                    json!({"index": SIGNED_IN_TABS + index, "signedIn": true, "site": site,
                           "withheld": true, "current": current})
                } else {
                    json!({"index": SIGNED_IN_TABS + index, "signedIn": true, "title": tab.title,
                           "url": tab.url, "current": current})
                }
            })
            .collect()
    }

    /// The address of the agent's signed-in tab when it is on `site`, else
    /// of any signed-in tab on it: what a held use names as its page.
    pub fn url_on(&self, site: &str) -> Option<String> {
        let tabs = self.lock_tabs();
        let on = |tab: &&Tab| crate::bridge::host_of(&tab.url) == site;
        tabs.list
            .iter()
            .filter(on)
            .find(|tab| tabs.current.as_ref() == Some(&tab.target))
            .or_else(|| tabs.list.iter().find(on))
            .map(|tab| tab.url.clone())
    }

    pub fn focused(&self) -> bool {
        self.0.focused.load(Ordering::SeqCst)
    }

    // ---- the person ------------------------------------------------------

    fn person_holds(&self) -> bool {
        crate::jobs::now().saturating_sub(self.0.person_at.load(Ordering::Relaxed))
            < PERSON_HOLDS_MS
    }

    /// The person's input on the site screen, to `target`: the agent sees
    /// nothing of the site while it lasts, and nothing of that tab, if they
    /// typed, until it loads a new document.
    async fn person_input(&self, cdp: &Cdp, tab: &Tab, typed: bool) {
        self.0
            .person_at
            .store(crate::jobs::now(), Ordering::Relaxed);
        self.touch();
        if typed {
            let doc = document(cdp, &tab.session).await.unwrap_or_default();
            if let Some(entry) = self
                .lock_tabs()
                .list
                .iter_mut()
                .find(|t| t.target == tab.target)
            {
                entry.person_doc = Some(doc);
            }
        }
    }

    /// The extension is about to fill a login into a page of `origin`:
    /// those tabs are withheld as if the broker had typed it.
    pub async fn withhold_origin(&self, origin: &str) {
        let running = self.0.running.lock().await.clone();
        let Some(running) = running else { return };
        let tabs: Vec<Tab> = self
            .lock_tabs()
            .list
            .iter()
            .filter(|tab| Origin::of_page(&tab.url).is_ok_and(|page| page.to_string() == origin))
            .cloned()
            .collect();
        for tab in tabs {
            let doc = document(&running.cdp, &tab.session)
                .await
                .unwrap_or_default();
            if let Some(entry) = self
                .lock_tabs()
                .list
                .iter_mut()
                .find(|t| t.target == tab.target)
            {
                entry.secret_doc = Some(doc);
                entry.secret_at = crate::jobs::now();
            }
        }
    }

    /// The sign-in flow of the signed-in tab on exactly `origin`, begun if
    /// none is: what a passkey request or a fill from the extension is bound
    /// to. `None` when no signed-in tab is on that origin, and then the
    /// request is refused.
    pub fn flow_on(&self, origin: &str) -> Option<String> {
        let sites = self.sites();
        let mut tabs = self.lock_tabs();
        let current = tabs.current.clone();
        let mut on_origin: Vec<&mut Tab> = tabs
            .list
            .iter_mut()
            .filter(|tab| {
                Origin::of_page(&tab.url).is_ok_and(|page| page.to_string() == origin)
                    && lock::signed_in(&tab.url, &sites)
            })
            .collect();
        on_origin.sort_by_key(|tab| Some(&tab.target) != current.as_ref());
        let tab = on_origin.into_iter().next()?;
        let url = tab.url.clone();
        let flow = tab.flow.get_or_insert_with(|| Flow {
            id: flow_id(),
            sites: sites
                .iter()
                .filter(|site| lock::signed_in(&url, std::slice::from_ref(site)))
                .cloned()
                .collect(),
        });
        Some(flow.id.clone())
    }

    // ---- actions ---------------------------------------------------------

    /// The tab the agent drives, made if there is none.
    async fn current(&self, running: &Running) -> Result<Tab, String> {
        {
            let tabs = self.lock_tabs();
            let current = tabs
                .current
                .as_ref()
                .and_then(|current| {
                    tabs.list
                        .iter()
                        .find(|tab| &tab.target == current && tab.ready)
                })
                .or_else(|| tabs.list.iter().rfind(|tab| tab.ready));
            if let Some(tab) = current {
                return Ok(tab.clone());
            }
        }
        self.new_tab(running).await
    }

    async fn new_tab(&self, running: &Running) -> Result<Tab, String> {
        let made = running
            .cdp
            .call("Target.createTarget", json!({"url": "about:blank"}))
            .await?;
        let target = made["targetId"].as_str().unwrap_or_default().to_owned();
        let started = std::time::Instant::now();
        loop {
            {
                let mut tabs = self.lock_tabs();
                if let Some(tab) = tabs
                    .list
                    .iter()
                    .find(|tab| tab.target == target && tab.ready)
                {
                    let tab = tab.clone();
                    tabs.current = Some(target.clone());
                    return Ok(tab);
                }
            }
            if started.elapsed() > Duration::from_secs(10) {
                return Err("the signed-in browser did not open a tab".into());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn tab(&self, target: &str) -> Option<Tab> {
        self.lock_tabs()
            .list
            .iter()
            .find(|tab| tab.target == target)
            .cloned()
    }

    /// What happened to the tab since the agent last heard, drained.
    fn notes(&self, target: &str) -> String {
        let notes = self
            .lock_tabs()
            .list
            .iter_mut()
            .find(|tab| tab.target == target)
            .map(|tab| std::mem::take(&mut tab.notes))
            .unwrap_or_default();
        notes.into_iter().map(|note| format!("\n{note}")).collect()
    }

    /// The refusal for any look at a tab the agent may not see now: asked
    /// after the look, so a sign-in that began during it still hides it.
    async fn withheld_answer(&self, cdp: &Cdp, target: &str) -> Option<String> {
        let tab = self.tab(target)?;
        let site = crate::bridge::host_of(&tab.url);
        if self.person_holds() {
            return Some(format!(
                "The person is using {site} on the site screen; nothing of it is shown until they stop. Try again shortly."
            ));
        }
        if !tab.withheld() {
            return None;
        }
        // The browser's own word on which document the tab shows. One it
        // cannot give keeps the tab hidden.
        let now = document(cdp, &tab.session).await;
        let still = |doc: &Option<String>| doc.is_some() && (now.is_none() || *doc == now);
        let (secret, person) = (still(&tab.secret_doc), still(&tab.person_doc));
        if !secret && !person {
            // A new document holds nothing anyone typed. Cleared only if
            // nothing was typed since this look began.
            if let Some(entry) = self
                .lock_tabs()
                .list
                .iter_mut()
                .find(|t| t.target == target)
            {
                if entry.secret_doc == tab.secret_doc {
                    entry.secret_doc = None;
                }
                if entry.person_doc == tab.person_doc {
                    entry.person_doc = None;
                }
            }
            return None;
        }
        if secret && crate::jobs::now().saturating_sub(tab.secret_at) > FINISH_AFTER_MS {
            return Some(format!(
                "Signing in to {site} did not finish here, and its page stays hidden from you while a password or code may show. Ask the person to Finish signing in: Open Browser on this computer shows them the site. Once it loads a new page, or you navigate this tab elsewhere, you can see it again."
            ));
        }
        Some(format!(
            "{site} is in the middle of signing in, so its page is hidden from you until it loads a new one. Submit the form (click_ref the sign-in button), then look again."
        ))
    }

    /// An observation's answer, unless the tab may not be seen now.
    async fn shown(&self, cdp: &Cdp, target: &str, answer: String) -> ToolResult {
        Ok(vec![ContentBlock::text(
            self.withheld_answer(cdp, target).await.unwrap_or(answer),
        )])
    }

    async fn act(&self, app: &App, action: &str, arguments: &Value, holder: &str) -> ToolResult {
        let _ = holder;
        let mutating = !matches!(action, "text" | "links" | "read" | "screenshot" | "tabs");
        if mutating && self.person_holds() {
            return Err(
                "The person is using the signed-in site on its screen; try again in a few seconds."
                    .into(),
            );
        }
        let running = self.browser().await?;
        let tab = self.current(&running).await?;
        let cdp = &running.cdp;
        let session = tab.session.as_str();
        let reference = arguments["ref"].as_str().unwrap_or_default();
        let site = crate::bridge::host_of(&tab.url);
        let answer = match action {
            "eval" => {
                let names = self.names_for(&tab.url);
                return Err(format!(
                    "{site} is signed in{}; scripts do not run there. Use text, links or read.",
                    if names.is_empty() {
                        String::new()
                    } else {
                        format!(" with {names}")
                    }
                ));
            }
            "navigate" => {
                let url = arguments["url"].as_str().unwrap_or_default();
                self.navigate(cdp, session, url).await?;
                format!("navigated to {url} (signed-in browser)")
            }
            "tab_new" => {
                let new = self.new_tab(&running).await?;
                let url = arguments["url"].as_str().unwrap_or_default();
                if !url.is_empty() {
                    self.navigate(cdp, &new.session, url).await?;
                }
                let notes = self.notes(&new.target);
                return Ok(vec![ContentBlock::text(format!(
                    "opened a signed-in tab at {url}{notes}"
                ))]);
            }
            "tab_select" => {
                let index = arguments["index"].as_u64().unwrap_or_default() as usize;
                let mut tabs = self.lock_tabs();
                let target = tabs
                    .list
                    .iter()
                    .filter(|tab| tab.ready)
                    .nth(index.saturating_sub(SIGNED_IN_TABS))
                    .map(|tab| tab.target.clone())
                    .ok_or_else(|| format!("tab {index} not found"))?;
                tabs.current = Some(target);
                return Ok(vec![ContentBlock::text(format!("selected tab {index}"))]);
            }
            "tab_close" => {
                let target = match arguments["index"].as_u64() {
                    Some(index) => self
                        .lock_tabs()
                        .list
                        .iter()
                        .filter(|tab| tab.ready)
                        .nth((index as usize).saturating_sub(SIGNED_IN_TABS))
                        .map(|tab| tab.target.clone())
                        .ok_or_else(|| format!("tab {index} not found"))?,
                    None => tab.target.clone(),
                };
                cdp.call("Target.closeTarget", json!({"targetId": target}))
                    .await?;
                if !self
                    .lock_tabs()
                    .list
                    .iter()
                    .any(|tab| tab.ready && tab.target != target)
                {
                    self.0.focused.store(false, Ordering::SeqCst);
                }
                return Ok(vec![ContentBlock::text(
                    "closed the signed-in tab".to_owned(),
                )]);
            }
            "text" => {
                let text = self.evaluate(cdp, session, TEXT_SCRIPT).await?;
                let text = text.as_str().unwrap_or_default().to_owned() + &self.notes(&tab.target);
                return self.shown(cdp, &tab.target, text).await;
            }
            "links" => {
                let links = self.evaluate(cdp, session, LINKS_SCRIPT).await?;
                return self.shown(cdp, &tab.target, links.to_string()).await;
            }
            "read" => {
                let selector = arguments["selector"].as_str().unwrap_or_default();
                if selector.is_empty() {
                    return Err("read takes a CSS selector".into());
                }
                let read = self
                    .evaluate(
                        cdp,
                        session,
                        &format!("({READ_SCRIPT})({})", json!(selector)),
                    )
                    .await?;
                return self.shown(cdp, &tab.target, read.to_string()).await;
            }
            "screenshot" => {
                if let Some(refusal) = self.withheld_answer(cdp, &tab.target).await {
                    return Ok(vec![ContentBlock::text(refusal)]);
                }
                let shot = cdp
                    .on(session, "Page.captureScreenshot", json!({"format": "png"}))
                    .await?;
                // Taken, then checked again: a sign-in that began meanwhile
                // keeps its picture.
                if let Some(refusal) = self.withheld_answer(cdp, &tab.target).await {
                    return Ok(vec![ContentBlock::text(refusal)]);
                }
                return Ok(vec![
                    ContentBlock::image(shot["data"].as_str().unwrap_or_default(), "image/png"),
                    ContentBlock::text(format!("the signed-in tab on {site}")),
                ]);
            }
            "click_ref" => {
                let double = matches!(arguments["button"].as_str(), Some("dbl" | "double"));
                self.click(cdp, session, reference, double).await?;
                format!("clicked {reference}")
            }
            "hover" => {
                let (x, y) = self.point(cdp, session, reference).await?;
                mouse(cdp, session, "mouseMoved", x, y, "none", 0).await?;
                format!("hovered {reference}")
            }
            "fill" => {
                if let Some(secret) = arguments["secret"].as_str().filter(|s| !s.is_empty()) {
                    if arguments["text"]
                        .as_str()
                        .is_some_and(|text| !text.is_empty())
                    {
                        return Err("fill takes text or secret, not both; omit whichever one you are not using".into());
                    }
                    self.fill_secret(app, &running, &tab, reference, secret)
                        .await?
                } else {
                    let text = arguments["text"]
                        .as_str()
                        .ok_or("fill requires text or secret; use text:\"\" to clear a field")?;
                    let done = self
                        .form(cdp, session, reference, "fill", json!(text))
                        .await?;
                    if self.withheld_answer(cdp, &tab.target).await.is_some() {
                        format!("filled {reference}")
                    } else {
                        done.to_string()
                    }
                }
            }
            "select" => {
                let values: Vec<String> = match arguments["values"].as_array() {
                    Some(values) => values
                        .iter()
                        .filter_map(|value| value.as_str().map(str::to_owned))
                        .collect(),
                    None => vec![arguments["value"].as_str().unwrap_or_default().to_owned()],
                };
                self.form(cdp, session, reference, "select", json!(values))
                    .await?;
                format!("selected {reference}")
            }
            "check" => {
                let checked = arguments["uncheck"] != true;
                self.form(cdp, session, reference, "check", json!(checked))
                    .await?;
                format!(
                    "{} {reference}",
                    if checked { "checked" } else { "unchecked" }
                )
            }
            "back" | "forward" => {
                let history = cdp
                    .on(session, "Page.getNavigationHistory", json!({}))
                    .await?;
                let at = history["currentIndex"].as_i64().unwrap_or_default()
                    + if action == "back" { -1 } else { 1 };
                let entry = history["entries"]
                    .as_array()
                    .and_then(|entries| entries.get(usize::try_from(at).ok()?))
                    .ok_or_else(|| format!("there is nothing to go {action} to"))?;
                let mut loaded = Loaded(cdp.events());
                cdp.on(
                    session,
                    "Page.navigateToHistoryEntry",
                    json!({"entryId": entry["id"]}),
                )
                .await?;
                loaded.wait(session).await;
                action.to_owned()
            }
            "reload" => {
                let mut loaded = Loaded(cdp.events());
                cdp.on(session, "Page.reload", json!({})).await?;
                loaded.wait(session).await;
                "reload".to_owned()
            }
            "upload" => {
                let path = arguments["path"].as_str().unwrap_or_default();
                if path.is_empty() {
                    return Err("path is required".into());
                }
                let staged = self.0.general.stage(std::path::Path::new(path)).await?;
                let node = self.node(cdp, session, reference).await?;
                cdp.on(
                    session,
                    "DOM.setFileInputFiles",
                    json!({"nodeId": node, "files": [staged]}),
                )
                .await?;
                format!("uploaded {path}")
            }
            "dialog_accept" | "dialog_dismiss" => {
                let accept = action == "dialog_accept";
                let mut params = json!({"accept": accept});
                if let Some(text) = arguments["text"].as_str().filter(|_| accept) {
                    params["promptText"] = json!(text);
                }
                cdp.on(session, "Page.handleJavaScriptDialog", params)
                    .await?;
                if accept {
                    "accepted dialog"
                } else {
                    "dismissed dialog"
                }
                .to_owned()
            }
            other => {
                return Err(crate::tools::action_error(
                    "browser",
                    other,
                    &[
                        "navigate",
                        "text",
                        "links",
                        "read",
                        "screenshot",
                        "click_ref",
                        "fill",
                        "select",
                        "check",
                        "hover",
                        "tabs",
                        "tab_new",
                        "tab_select",
                        "tab_close",
                        "upload",
                        "dialog_accept",
                        "dialog_dismiss",
                        "downloads",
                        "back",
                        "forward",
                        "reload",
                    ],
                ));
            }
        };
        let notes = self.notes(&tab.target);
        Ok(vec![ContentBlock::text(format!("{answer}{notes}"))])
    }

    /// The login names and shares a page's site is signed in with.
    fn names_for(&self, url: &str) -> String {
        let mut names: Vec<String> = Vec::new();
        for entry in self.0.secrets.catalog() {
            let sites: Vec<Origin> = entry["sites"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|site| site.as_str().and_then(|site| Origin::site(site).ok()))
                .collect();
            if entry["kind"] == "login" && lock::signed_in(url, &sites) {
                names.push(entry["name"].as_str().unwrap_or_default().to_owned());
            }
        }
        for (name, sites) in self
            .0
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
        {
            if lock::signed_in(url, sites) {
                names.push(name.clone());
            }
        }
        names.join(", ")
    }

    /// Loads `url` in a tab and waits for it, a little.
    async fn navigate(&self, cdp: &Cdp, session: &str, url: &str) -> Result<(), String> {
        let mut loaded = Loaded(cdp.events());
        let answer = cdp
            .on(session, "Page.navigate", json!({"url": url}))
            .await?;
        match answer["errorText"].as_str() {
            // Stopped by the lock: the note says why.
            Some("net::ERR_BLOCKED_BY_CLIENT") => Ok(()),
            Some(error) if !error.is_empty() => Err(format!("browser: {error}")),
            _ => {
                loaded.wait(session).await;
                // Where it landed, as the browser says, not as an event that
                // may still be on its way.
                if let Ok(history) = cdp
                    .on(session, "Page.getNavigationHistory", json!({}))
                    .await
                    && let Some(url) = history["entries"]
                        .as_array()
                        .and_then(|entries| entries.get(history["currentIndex"].as_u64()? as usize))
                        .and_then(|entry| entry["url"].as_str())
                    && let Some(tab) = self
                        .lock_tabs()
                        .list
                        .iter_mut()
                        .find(|tab| tab.session == session)
                {
                    tab.url = url.to_owned();
                }
                Ok(())
            }
        }
    }

    /// Runs one of the broker's own scripts in a world of its own on the
    /// tab's page: the page's scripts cannot see or change it.
    async fn evaluate(&self, cdp: &Cdp, session: &str, script: &str) -> Result<Value, String> {
        // A page between documents has no world to run in for a moment;
        // the next document's comes shortly.
        for _ in 0..30 {
            match self.evaluate_once(cdp, session, script).await {
                Err(error) if between_documents(&error) => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                answer => return answer,
            }
        }
        self.evaluate_once(cdp, session, script).await
    }

    async fn evaluate_once(&self, cdp: &Cdp, session: &str, script: &str) -> Result<Value, String> {
        let tree = cdp.on(session, "Page.getFrameTree", json!({})).await?;
        let frame = tree["frameTree"]["frame"]["id"]
            .as_str()
            .unwrap_or_default();
        let world = cdp
            .on(
                session,
                "Page.createIsolatedWorld",
                json!({"frameId": frame, "worldName": "hotline"}),
            )
            .await?;
        let answer = cdp
            .on(
                session,
                "Runtime.evaluate",
                json!({"expression": script, "contextId": world["executionContextId"],
                       "returnByValue": true, "awaitPromise": true}),
            )
            .await?;
        if let Some(thrown) = answer.get("exceptionDetails") {
            return Err(format!(
                "browser: {}",
                thrown["exception"]["description"]
                    .as_str()
                    .or_else(|| thrown["text"].as_str())
                    .unwrap_or("the page refused")
            ));
        }
        Ok(answer["result"]["value"].clone())
    }

    /// Where a ref's element is on the page, scrolled into view.
    async fn point(&self, cdp: &Cdp, session: &str, reference: &str) -> Result<(f64, f64), String> {
        let selector = ref_selector(reference)?;
        let point = self
            .evaluate(
                cdp,
                session,
                &format!(
                    "(() => {{ const e = document.querySelector({}); if (!e) return null; e.scrollIntoView({{block:'center',inline:'center'}}); const r = e.getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2]; }})()",
                    json!(selector)
                ),
            )
            .await?;
        match point.as_array().map(Vec::as_slice) {
            Some([x, y]) => Ok((
                x.as_f64().unwrap_or_default(),
                y.as_f64().unwrap_or_default(),
            )),
            _ => Err(format!(
                "no element {reference}; take a fresh browser text snapshot"
            )),
        }
    }

    async fn click(
        &self,
        cdp: &Cdp,
        session: &str,
        reference: &str,
        double: bool,
    ) -> Result<(), String> {
        let (x, y) = self.point(cdp, session, reference).await?;
        mouse(cdp, session, "mouseMoved", x, y, "none", 0).await?;
        for count in 1..=if double { 2 } else { 1 } {
            mouse(cdp, session, "mousePressed", x, y, "left", count).await?;
            mouse(cdp, session, "mouseReleased", x, y, "left", count).await?;
        }
        Ok(())
    }

    async fn form(
        &self,
        cdp: &Cdp,
        session: &str,
        reference: &str,
        action: &str,
        requested: Value,
    ) -> Result<Value, String> {
        let selector = ref_selector(reference)?;
        let script = format!(
            "({})(document.querySelector({}), {}, {})",
            include_str!("../../assets/browser-form.js"),
            json!(selector),
            json!(action),
            requested
        );
        let value = self.evaluate(cdp, session, &script).await?;
        if value["ok"] != true {
            let mut refusal = value;
            if let Some(object) = refusal.as_object_mut() {
                object.remove("value");
            }
            return Err(refusal.to_string());
        }
        Ok(value)
    }

    async fn node(&self, cdp: &Cdp, session: &str, reference: &str) -> Result<i64, String> {
        let selector = ref_selector(reference)?;
        let document = cdp
            .on(session, "DOM.getDocument", json!({"depth": 0}))
            .await?;
        let found = cdp
            .on(
                session,
                "DOM.querySelector",
                json!({"nodeId": document["root"]["nodeId"], "selector": selector}),
            )
            .await?;
        found["nodeId"]
            .as_i64()
            .filter(|node| *node != 0)
            .ok_or_else(|| format!("no element {reference}; take a fresh browser text snapshot"))
    }

    /// The sign-in step: the broker looks the value up, checks the page, asks
    /// the person when the login says so, and types it; the tab is withheld
    /// from the first secret typed until its next document.
    async fn fill_secret(
        &self,
        app: &App,
        running: &Running,
        tab: &Tab,
        reference: &str,
        secret: &str,
    ) -> Result<String, String> {
        let cdp = &running.cdp;
        let filled = app.secrets.resolve(secret)?;
        let label = app.secrets.redact(&filled.label).into_owned();
        let Some(sites) = filled.sites.clone() else {
            return Err(format!(
                "{label} is a variable; in the signed-in browser only a login's fields are typed"
            ));
        };
        let (name, field) = secret.split_once('.').unwrap_or((secret, ""));
        // The page as the browser has it now, not as last heard.
        let history = cdp
            .on(&tab.session, "Page.getNavigationHistory", json!({}))
            .await?;
        let url = history["entries"]
            .as_array()
            .and_then(|entries| entries.get(history["currentIndex"].as_u64()? as usize))
            .and_then(|entry| entry["url"].as_str())
            .unwrap_or_default()
            .to_owned();
        let site = crate::bridge::host_of(&url);
        let allowed = sites
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let page = Origin::of_page(&url);
        if !page
            .as_ref()
            .is_ok_and(|page| sites.iter().any(|s| s.allows(page)))
        {
            crate::bridge::record(app, &site, "was refused a login", false, None).await;
            return Err(match page {
                Ok(page) => format!(
                    "{label} is typed only on {allowed}; the signed-in tab is on {page}. Navigate to the site's sign-in page first: it opens in the signed-in browser."
                ),
                Err(_) => format!(
                    "{label} is typed only on {allowed}. Navigate to the site's sign-in page first: it opens in the signed-in browser."
                ),
            });
        }
        let origin = page.map(|page| page.to_string()).unwrap_or_default();
        let flow = {
            let mut tabs = self.lock_tabs();
            let entry = tabs.list.iter_mut().find(|t| t.target == tab.target);
            match entry {
                Some(entry) => entry
                    .flow
                    .get_or_insert_with(|| Flow {
                        id: flow_id(),
                        sites: sites.clone(),
                    })
                    .id
                    .clone(),
                None => return Err("the signed-in tab closed".into()),
            }
        };
        let mut how = None;
        // A username is who, not a secret: only what signs in waits.
        if field != "username" && app.secrets.use_of(name) == Some(Use::Ask) {
            let account = app
                .secrets
                .resolve(&format!("{name}.username"))
                .map(|username| username.value)
                .unwrap_or_default();
            let bound = Bound {
                operation: Operation {
                    kind: OperationKind::Signin,
                    origin: origin.clone(),
                    account,
                    rp_id: None,
                },
                attested: true,
                flow,
            };
            let who = crate::bridge::who(app).await;
            let path = crate::bridge::path_on(&url, &site);
            match app
                .holds
                .fill_bound(name, &site, &path, &who, crate::jobs::now(), Some(bound))
            {
                Pass::Within => how = Some("within the 10 min"),
                Pass::Answered(outcome) => how = Some(outcome.how()),
                Pass::Held { id, lapsed } => {
                    return Err(format!(
                        "{}Waiting for the person to approve {name} on {site} (use {id}). Nothing was typed. Try this fill again after they answer at the desk.",
                        if lapsed {
                            "The earlier ask went unanswered and lapsed. "
                        } else {
                            ""
                        }
                    ));
                }
                Pass::Denied => {
                    return Err(format!(
                        "The person denied this use of {name} on {site}. Nothing was typed; do not try another way, and ask them in the chat if it is still needed."
                    ));
                }
            }
        }
        let selector = ref_selector(reference)?;
        let secret_field = field != "username";
        // Withheld before the first character goes in, bound to the
        // document it goes into.
        if secret_field {
            let doc = document(cdp, &tab.session)
                .await
                .ok_or("the signed-in tab is changing pages; nothing was typed. Try again.")?;
            if let Some(entry) = self
                .lock_tabs()
                .list
                .iter_mut()
                .find(|t| t.target == tab.target)
            {
                entry.secret_doc = Some(doc);
                entry.secret_at = crate::jobs::now();
            }
        }
        let focused = self
            .evaluate(
                cdp,
                &tab.session,
                &format!(
                    "(() => {{ const e = document.querySelector({}); if (!e || !['input','textarea'].includes(e.tagName.toLowerCase())) return false; e.focus(); e.select && e.select(); if ({}) e.setAttribute('data-hotline-secret', ''); return document.activeElement === e; }})()",
                    json!(selector),
                    secret_field
                ),
            )
            .await?;
        if focused != true {
            return Err(format!(
                "{reference} is not a field {label} can be typed into; take a fresh browser text snapshot"
            ));
        }
        cdp.on(
            &tab.session,
            "Input.insertText",
            json!({"text": filled.value}),
        )
        .await?;
        // Whether the field kept something, never what.
        let kept = self
            .evaluate(
                cdp,
                &tab.session,
                &format!("(() => {{ const e = document.querySelector({}); return !!e && e.value.length > 0; }})()", json!(selector)),
            )
            .await
            .unwrap_or(Value::Bool(false));
        if field != "username" {
            crate::bridge::record(app, &site, "filled a login", true, how).await;
        }
        if kept != true {
            return Err(format!(
                "{reference} did not keep {label}; the page may want the person to finish signing in"
            ));
        }
        Ok(if secret_field {
            format!(
                "filled {reference} with {label}. This tab is hidden from you until it loads its next page: submit the form now."
            )
        } else {
            format!("filled {reference} with {label}")
        })
    }

    // ---- revocation ------------------------------------------------------

    /// Signs `sites` out: their cookies and site data go, and their tabs
    /// close. Answers how many cookies and tabs went.
    pub async fn forget(&self, sites: &[Origin]) -> Result<(usize, usize), String> {
        if sites.is_empty() {
            return Ok((0, 0));
        }
        {
            let mut kept = self
                .0
                .kept
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            kept.retain(|cookie| !cookie_within(cookie, sites));
        }
        let running = {
            let running = self.0.running.lock().await;
            running.clone()
        };
        let Some(running) = running.filter(|running| !running.cdp.is_closed()) else {
            return Ok((0, 0));
        };
        let cdp = &running.cdp;
        let stored = cdp.call("Storage.getCookies", json!({})).await?;
        let cookies = stored["cookies"].as_array().cloned().unwrap_or_default();
        let (gone, kept): (Vec<Value>, Vec<Value>) = cookies
            .into_iter()
            .partition(|cookie| cookie_within(cookie, sites));
        if !gone.is_empty() {
            cdp.call("Storage.clearCookies", json!({})).await?;
            let kept: Vec<Value> = kept.iter().map(cookie_param).collect();
            if !kept.is_empty() {
                cdp.call("Storage.setCookies", json!({"cookies": kept}))
                    .await?;
            }
        }
        for site in sites {
            let _ = cdp
                .call(
                    "Storage.clearDataForOrigin",
                    json!({"origin": site.to_string(), "storageTypes": "all"}),
                )
                .await;
        }
        let doomed: Vec<String> = self
            .lock_tabs()
            .list
            .iter()
            .filter(|tab| lock::signed_in(&tab.url, sites))
            .map(|tab| tab.target.clone())
            .collect();
        for target in &doomed {
            let _ = cdp
                .call("Target.closeTarget", json!({"targetId": target}))
                .await;
        }
        Ok((gone.len(), doomed.len()))
    }
}

/// The document a tab shows now: its main frame's loader id.
async fn document(cdp: &Cdp, session: &str) -> Option<String> {
    let tree = cdp.on(session, "Page.getFrameTree", json!({})).await.ok()?;
    tree["frameTree"]["frame"]["loaderId"]
        .as_str()
        .map(str::to_owned)
}

/// Waits for a tab's next load, a little.
struct Loaded(tokio::sync::broadcast::Receiver<cdp::Event>);

impl Loaded {
    async fn wait(&mut self, session: &str) {
        let _ = tokio::time::timeout(LOAD_PATIENCE, async {
            loop {
                match self.0.recv().await {
                    Ok(event)
                        if event.method == "Page.loadEventFired"
                            && event.session.as_deref() == Some(session) =>
                    {
                        return;
                    }
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(_) => return,
                }
            }
        })
        .await;
    }
}

async fn mouse(
    cdp: &Cdp,
    session: &str,
    kind: &str,
    x: f64,
    y: f64,
    button: &str,
    count: u32,
) -> Result<(), String> {
    cdp.on(
        session,
        "Input.dispatchMouseEvent",
        json!({"type": kind, "x": x, "y": y, "button": button, "clickCount": count}),
    )
    .await
    .map(|_| ())
}

/// Whether a DevTools refusal only means the page was changing documents.
fn between_documents(error: &str) -> bool {
    [
        "Cannot find context",
        "Execution context was destroyed",
        "No frame",
        "Cannot find default execution context",
    ]
    .iter()
    .any(|sign| error.contains(sign))
}

fn ref_selector(reference: &str) -> Result<String, String> {
    if reference.is_empty() {
        return Err("ref is required".to_owned());
    }
    if !reference
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return Err("ref is invalid".to_owned());
    }
    Ok(format!("[data-hotline-ref=\"{reference}\"]"))
}

fn flow_id() -> String {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 9];
    let _ = ring::rand::SystemRandom::new().fill(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Whether a cookie DevTools reports belongs to one of `sites`.
fn cookie_within(cookie: &Value, sites: &[Origin]) -> bool {
    let hosts: Vec<String> = sites
        .iter()
        .map(|site| crate::bridge::host_of(&site.to_string()))
        .collect();
    crate::logins::within(cookie["domain"].as_str().unwrap_or_default(), &hosts)
}

/// A reported cookie as one to set again.
fn cookie_param(cookie: &Value) -> Value {
    let mut param = json!({});
    for field in [
        "name",
        "value",
        "domain",
        "path",
        "secure",
        "httpOnly",
        "sameSite",
        "priority",
        "sourceScheme",
        "sourcePort",
        "partitionKey",
    ] {
        if let Some(value) = cookie.get(field)
            && !value.is_null()
        {
            param[field] = value.clone();
        }
    }
    if cookie["session"] != true
        && let Some(expires) = cookie["expires"].as_f64().filter(|at| *at > 0.0)
    {
        param["expires"] = json!(expires);
    }
    param
}

/// The page snapshot `text` gives, with refs. A field the broker typed a
/// secret into, or one for a one-time code, never shows its value.
const TEXT_SCRIPT: &str = r#"(() => {
    document.querySelectorAll('[data-hotline-ref]').forEach(e => e.removeAttribute('data-hotline-ref'));
    const interesting = 'a,button,input,select,textarea,[contenteditable],[role],h1,h2,h3,h4,h5,h6';
    const lines = [];
    let next = 1;
    const hidden = e => e.type === 'password' || e.hasAttribute('data-hotline-secret') || (e.autocomplete || '').includes('one-time-code');
    for (const element of document.querySelectorAll(interesting)) {
        if (!element.getClientRects().length || getComputedStyle(element).visibility === 'hidden' || element.closest('[hidden],[inert],[aria-hidden="true"]')) continue;
        const ref = `e${next++}`;
        element.setAttribute('data-hotline-ref', ref);
        const tag = element.tagName.toLowerCase();
        const role = element.getAttribute('role') || ({a:'link',button:'button',input:element.type || 'input',select:'combobox',textarea:'textbox'}[tag] || tag);
        const byId = (ids) => (ids || '').split(/\s+/).map(id => document.getElementById(id)).filter(Boolean).map(e => e.innerText).join(' ');
        const labels = element.labels ? [...element.labels].map(l => l.innerText).join(' ') : '';
        const name = element.getAttribute('aria-label') || byId(element.getAttribute('aria-labelledby')) || labels || element.innerText || (hidden(element) ? "" : element.value) || element.placeholder || element.title || element.name || '';
        const states = [];
        if (element.matches(':disabled,[aria-disabled="true"]')) states.push('disabled');
        if (element.readOnly) states.push('readonly');
        if (element.required) states.push('required');
        if ('checked' in element && ['checkbox','radio'].includes(element.type)) states.push(`checked=${element.checked}`);
        if ('value' in element && !hidden(element)) states.push(`value=${JSON.stringify(element.value)}`);
        if (tag === 'select') states.push(`multiple=${element.multiple}`, `options=${JSON.stringify([...element.options].map(o => ({value:o.value,label:o.label,selected:o.selected,disabled:o.disabled || !!o.closest('optgroup[disabled]')})))}`);
        lines.push(`[${ref}] [${role}] ${name.trim().replace(/\s+/g, ' ')} ${states.join(' ')}`.trim());
    }
    const body = document.body ? document.body.innerText.trim() : '';
    return [`page: ${document.title}`, `signed-in browser: ${location.host}`, body, ...lines].filter(Boolean).join('\n');
})()"#;

const LINKS_SCRIPT: &str = "[...document.querySelectorAll('a[href]')].map(a => ({text:(a.innerText||'').trim(),href:a.href}))";

/// `read {selector}`: the text of what matches, never a field's value.
const READ_SCRIPT: &str = "(selector) => [...document.querySelectorAll(selector)].slice(0, 50).map(e => (e.innerText || e.textContent || '').trim())";

#[cfg(test)]
mod tests;
