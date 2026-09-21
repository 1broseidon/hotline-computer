use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use chromiumoxide::cdp::browser_protocol::dom::SetFileInputFilesParams;
use chromiumoxide::cdp::browser_protocol::network::{CookieParam, DeleteCookiesParams};
use chromiumoxide::cdp::browser_protocol::page::{
    AddScriptToEvaluateOnNewDocumentParams, HandleJavaScriptDialogParams,
    RemoveScriptToEvaluateOnNewDocumentParams, ScriptIdentifier,
};
use chromiumoxide::cdp::browser_protocol::web_authn::{
    AddCredentialParams, AddVirtualAuthenticatorParams, AuthenticatorId, AuthenticatorProtocol,
    AuthenticatorTransport, Credential, Ctap2Version, EnableParams, GetCredentialsParams,
    RemoveCredentialParams, VirtualAuthenticatorOptions,
};
use chromiumoxide::cdp::js_protocol::runtime::RemoteObjectType;
use chromiumoxide::{Binary, Browser, BrowserConfig, Page};
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::Config;
use crate::passkeys::{Ask, Passkeys, Reported};
use crate::secrets::{Filled, Origin, Passkey, Secrets};

pub const NO_BROWSER: &str = "This computer has no browser installed.";

/// A live browser answers a version query in milliseconds; one that has not
/// in this long is gone, whatever the process table says.
const LIVENESS: Duration = Duration::from_secs(3);
/// No single browser action holds the machine longer than this.
const ACTION_DEADLINE: Duration = Duration::from_secs(60);
/// How long a fresh Chromium gets to report its first tab.
const INITIAL_TAB: Duration = Duration::from_secs(2);
/// How long a passkey delivery waits for a browser action in flight before
/// leaving the tabs to the next action's own look.
const SYNC_PATIENCE: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct BrowserManager {
    config: Arc<Config>,
    secrets: Secrets,
    passkeys: Passkeys,
    session: Arc<Mutex<Option<BrowserSession>>>,
}

struct BrowserSession {
    browser: Browser,
    current: usize,
    secrets: Secrets,
    passkeys: Passkeys,
    /// What the guard script wants before it lets a passkey be made: made
    /// for this browser at launch, and never on a page.
    token: String,
    /// The tabs that carry the teammate's passkeys, by target.
    authenticators: HashMap<String, TabAuthenticator>,
    _handler: tokio::task::JoinHandle<()>,
}

/// A tab's virtual authenticator and the guard installed in its documents.
struct TabAuthenticator {
    id: AuthenticatorId,
    guard: ScriptIdentifier,
    /// The site the installed guard is armed for.
    armed: Option<String>,
}

impl BrowserManager {
    pub fn new(config: Arc<Config>, secrets: Secrets, passkeys: Passkeys) -> Self {
        Self {
            config,
            secrets,
            passkeys,
            session: Arc::new(Mutex::new(None)),
        }
    }

    /// Every browser action runs against a browser that just answered, and
    /// none runs longer than `ACTION_DEADLINE`. The person can close the
    /// browser from the viewer at any moment; an action that then waits for
    /// a page event waits forever, and it holds the machine lock while it
    /// does, so every other tool queues behind it. A browser that is gone is
    /// reaped and the next action starts a fresh one.
    async fn with_session<T, F>(&self, operation: F) -> Result<T, String>
    where
        F: for<'a> FnOnce(
            &'a mut BrowserSession,
        ) -> Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'a>>,
    {
        let mut session = self.session.lock().await;
        if let Some(current) = session.as_ref()
            && !alive(&current.browser).await
        {
            reap(session.take().expect("checked above")).await;
        }
        if session.is_none() {
            *session = Some(self.launch().await?);
        }
        let current = session.as_mut().expect("created above");
        match tokio::time::timeout(ACTION_DEADLINE, operation(current)).await {
            Ok(result) => result,
            Err(_) => {
                let gone = !alive(&current.browser).await;
                if gone {
                    reap(session.take().expect("created above")).await;
                    return Err(
                        "browser: the browser exited; the next call starts a fresh one".to_owned(),
                    );
                }
                Err(format!(
                    "browser: gave up after {}s; the page may still be loading",
                    ACTION_DEADLINE.as_secs()
                ))
            }
        }
    }

    /// Have a browser on the desktop: launch one if none is running.
    pub async fn open(&self) -> Result<(), String> {
        self.with_session(|_| Box::pin(async { Ok(()) })).await
    }

    /// The desktop saw the last browser window go. The process behind it
    /// keeps running and still answers DevTools, but a browser nobody can
    /// see is no browser; the next call starts a fresh one.
    pub async fn forget(&self) {
        if let Some(session) = self.session.lock().await.take() {
            reap(session).await;
        }
    }

    /// Chromium leaves symlinks in its profile naming the host and process
    /// that hold it. A home kept across containers carries them to a new
    /// container with a new hostname, where Chromium reads them as another
    /// computer using the profile and refuses to start. This profile is only
    /// ever this computer's browser, launched from here, so a lock found at
    /// launch is always stale.
    fn clear_profile_locks(profile: &std::path::Path) {
        for name in ["SingletonLock", "SingletonSocket", "SingletonCookie"] {
            let _ = std::fs::remove_file(profile.join(name));
        }
    }

    async fn launch(&self) -> Result<BrowserSession, String> {
        let executable = find_on_path("chromium").ok_or_else(|| NO_BROWSER.to_owned())?;
        let profile = self.config.home.join(".hotline/browser");
        tokio::fs::create_dir_all(&profile)
            .await
            .map_err(|error| format!("create browser profile: {error}"))?;
        Self::clear_profile_locks(&profile);
        let browser_config = BrowserConfig::builder()
            .chrome_executable(executable)
            .with_head()
            .no_sandbox()
            .port(9222)
            .viewport(None)
            .user_data_dir(profile)
            .env("DISPLAY", self.config.display.clone())
            // chromiumoxide's own defaults are a test harness's: they include
            // `--enable-automation`, which sets `navigator.webdriver`, hangs a
            // banner over every page, and is the first thing a site checks
            // before deciding a visitor is a script. This is a person's desktop
            // browser that the agent also drives, so it starts the way a
            // desktop Chromium does, and DevTools is just a port.
            .disable_default_args()
            .args([
                // Chromium does not expose its tree to AT-SPI until forced.
                "force-renderer-accessibility",
                "disable-blink-features=AutomationControlled",
                "disable-gpu",
                "disable-software-rasterizer",
                "no-first-run",
                "no-default-browser-check",
                "no-session-restore",
                "hide-crash-restore-bubble",
                "disable-background-networking",
                "metrics-recording-only",
                "disable-breakpad",
                "disable-features=TranslateUI",
                "password-store=basic",
                "lang=en-US",
                // Without it Chromium hangs an "unsupported flag" bar over
                // the page for `--no-sandbox`, which the dropped capabilities
                // make necessary.
                "test-type",
            ])
            .build()
            .map_err(|error| format!("browser: {error}"))?;
        let (browser, mut handler) = Browser::launch(browser_config)
            .await
            .map_err(|error| format!("browser: {error}"))?;
        let handler = tokio::spawn(async move { while handler.next().await.is_some() {} });
        // Chromium opens with one tab, reported a beat after DevTools answers.
        // Creating another before it shows up leaves two tabs, and the one
        // the agent drives is not the one on screen.
        let started = std::time::Instant::now();
        while browser.pages().await.map_err(browser_error)?.is_empty() {
            if started.elapsed() > INITIAL_TAB {
                browser
                    .new_page("about:blank")
                    .await
                    .map_err(browser_error)?;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Ok(BrowserSession {
            browser,
            current: 0,
            secrets: self.secrets.clone(),
            passkeys: self.passkeys.clone(),
            token: session_token()?,
            authenticators: HashMap::new(),
            _handler: handler,
        })
    }

    /// Puts the teammate's passkeys into every tab and the arming, if any,
    /// into every guard: after a delivery, at each look while armed, and
    /// when the arming ends. With `launch`, a browser is started when none
    /// is up, since the person is about to make a passkey in it, and it
    /// opens on the armed site. Without, a browser that is busy with an
    /// action is left alone: that action's own look at its tab, and the
    /// next one's, carry the change.
    pub async fn sync_passkeys(&self, launch: bool) -> Result<(), String> {
        let mut session = if launch {
            self.session.lock().await
        } else {
            match tokio::time::timeout(SYNC_PATIENCE, self.session.lock()).await {
                Ok(session) => session,
                Err(_) => return Ok(()),
            }
        };
        if let Some(current) = session.as_ref()
            && !alive(&current.browser).await
        {
            reap(session.take().expect("checked above")).await;
        }
        let fresh = session.is_none();
        if fresh {
            if !launch {
                return Ok(());
            }
            *session = Some(self.launch().await?);
        }
        let session = session.as_mut().expect("created above");
        let synced = async {
            let pages = session.browser.pages().await.map_err(browser_error)?;
            let open: HashSet<String> = pages
                .iter()
                .map(|page| page.target_id().inner().clone())
                .collect();
            session
                .authenticators
                .retain(|target, _| open.contains(target));
            let mut held = HashSet::new();
            for page in &pages {
                held.extend(passkey_sync(session, page).await?);
            }
            // Every tab was looked at: a request none holds went with
            // its document.
            session.passkeys.reconcile(&held);
            // A courtesy on a fresh browser: the person is about to sign in
            // there. A site that does not answer is the site's business, not
            // a reason to refuse the arming.
            if fresh
                && let Some((rp_id, _)) = session.passkeys.armed()
                && let Some(page) = pages.first()
            {
                page.bring_to_front().await.map_err(browser_error)?;
                if let Err(error) = page.goto(format!("https://{rp_id}/")).await {
                    eprintln!("[passkeys] the armed site did not open: {error}");
                }
            }
            Ok(())
        };
        tokio::time::timeout(ACTION_DEADLINE, synced)
            .await
            .unwrap_or_else(|_| Err("browser: the tabs did not answer in time".to_owned()))
    }

    pub async fn navigate(&self, url: &str) -> Result<String, String> {
        if url.is_empty() {
            return Err("url is required".to_owned());
        }
        let url = url.to_owned();
        self.with_session(|session| {
            Box::pin(async move {
                let page = current_page(session).await?;
                // The page the agent drives is the page the person sees.
                page.bring_to_front().await.map_err(browser_error)?;
                page.goto(url.as_str()).await.map_err(browser_error)?;
                Ok(format!("navigated to {url}"))
            })
        })
        .await
    }

    pub async fn text(&self) -> Result<String, String> {
        self.with_session(|session| Box::pin(async move {
            let page = current_page(session).await?;
            let script = r#"(() => {
                document.querySelectorAll('[data-hotline-ref]').forEach(e => e.removeAttribute('data-hotline-ref'));
                const interesting = 'a,button,input,select,textarea,[contenteditable],[role],h1,h2,h3,h4,h5,h6';
                const lines = [];
                let next = 1;
                for (const element of document.querySelectorAll(interesting)) {
                    if (!element.getClientRects().length || getComputedStyle(element).visibility === 'hidden' || element.closest('[hidden],[inert],[aria-hidden="true"]')) continue;
                    const ref = `e${next++}`;
                    element.setAttribute('data-hotline-ref', ref);
                    const tag = element.tagName.toLowerCase();
                    const role = element.getAttribute('role') || ({a:'link',button:'button',input:element.type || 'input',select:'combobox',textarea:'textbox'}[tag] || tag);
                    const byId = (ids) => (ids || '').split(/\s+/).map(id => document.getElementById(id)).filter(Boolean).map(e => e.innerText).join(' ');
                    const labels = element.labels ? [...element.labels].map(l => l.innerText).join(' ') : '';
                    const name = element.getAttribute('aria-label') || byId(element.getAttribute('aria-labelledby')) || labels || element.innerText || (element.type === "password" ? "" : element.value) || element.placeholder || element.title || element.name || '';
                    const states = [];
                    if (element.matches(':disabled,[aria-disabled="true"]')) states.push('disabled');
                    if (element.readOnly) states.push('readonly');
                    if (element.required) states.push('required');
                    if ('checked' in element && ['checkbox','radio'].includes(element.type)) states.push(`checked=${element.checked}`);
                    if ('value' in element && element.type !== 'password') states.push(`value=${JSON.stringify(element.value)}`);
                    if (tag === 'select') states.push(`multiple=${element.multiple}`, `options=${JSON.stringify([...element.options].map(o => ({value:o.value,label:o.label,selected:o.selected,disabled:o.disabled || !!o.closest('optgroup[disabled]')})))}`);
                    if (element.validity && !element.validity.valid) states.push(`invalid=${JSON.stringify(element.validationMessage)}`);
                    lines.push(`[${ref}] [${role}] ${name.trim().replace(/\s+/g, ' ')} ${states.join(' ')}`.trim());
                }
                const body = document.body ? document.body.innerText.trim() : '';
                return [`page: ${document.title}`, body, ...lines].filter(Boolean).join('\n');
            })()"#;
            evaluate_string(&page, script).await
        }))
        .await
    }

    pub async fn links(&self) -> Result<String, String> {
        self.with_session(|session| Box::pin(async move {
            let page = current_page(session).await?;
            let value = evaluate_value(
                &page,
                "[...document.querySelectorAll('a[href]')].map(a => ({text:(a.innerText||'').trim(),href:a.href}))",
            )
            .await?;
            serde_json::to_string(&value).map_err(|error| error.to_string())
        }))
        .await
    }

    pub async fn eval(&self, javascript: &str) -> Result<String, String> {
        if javascript.is_empty() {
            return Err("js is required".to_owned());
        }
        let javascript = javascript.to_owned();
        self.with_session(|session| {
            Box::pin(async move {
                let value = evaluate_value(&current_page(session).await?, &javascript).await?;
                serde_json::to_string(&value).map_err(|error| error.to_string())
            })
        })
        .await
    }

    pub async fn click_ref(&self, reference: &str, double: bool) -> Result<String, String> {
        let selector = ref_selector(reference)?;
        let reference = reference.to_owned();
        self.with_session(|session| Box::pin(async move {
            let page = current_page(session).await?;
            let element = page.find_element(selector).await.map_err(browser_error)?;
            if double {
                element
                    .call_js_fn("function(){ this.dispatchEvent(new MouseEvent('dblclick', {bubbles:true})); }", true)
                    .await
                    .map_err(browser_error)?;
            } else {
                element.click().await.map_err(browser_error)?;
            }
            Ok(format!("clicked {reference}"))
        }))
        .await
    }

    pub async fn fill(&self, reference: &str, text: &str) -> Result<String, String> {
        self.form_action(reference, "fill", json!(text)).await
    }

    /// Types a stored secret's value into a field, and answers its name,
    /// never the value. A login's field goes only onto a page whose origin,
    /// as the browser reports it, is one of the login's sites or lies under
    /// one: the page a password is typed into can read it, so which page is
    /// the whole protection.
    pub async fn fill_secret(&self, reference: &str, filled: Filled) -> Result<String, String> {
        let selector = ref_selector(reference)?;
        let reference = reference.to_owned();
        self.with_session(|session| {
            Box::pin(async move {
                let page = current_page(session).await?;
                if let Some(sites) = &filled.sites {
                    let url = page.url().await.map_err(browser_error)?.unwrap_or_default();
                    let allowed = sites
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ");
                    let origin = Origin::of_page(&url).map_err(|_| {
                        format!(
                            "{} is typed only on {allowed}, and this page has no site",
                            filled.label
                        )
                    })?;
                    if !sites.iter().any(|site| site.allows(&origin)) {
                        return Err(format!(
                            "{} is typed only on {allowed}; this page is {origin}",
                            filled.label
                        ));
                    }
                }
                form_action_on(&page, &selector, "fill", json!(filled.value)).await?;
                Ok(format!("filled {reference} with {}", filled.label))
            })
        })
        .await
    }

    pub async fn select(&self, reference: &str, values: &[String]) -> Result<String, String> {
        self.form_action(reference, "select", json!(values)).await
    }

    pub async fn check(&self, reference: &str, checked: bool) -> Result<String, String> {
        self.form_action(reference, "check", json!(checked)).await
    }

    async fn form_action(
        &self,
        reference: &str,
        action: &str,
        requested: Value,
    ) -> Result<String, String> {
        let selector = ref_selector(reference)?;
        let action = action.to_owned();
        self.with_session(|session| {
            Box::pin(async move {
                let page = current_page(session).await?;
                let value = form_action_on(&page, &selector, &action, requested).await?;
                Ok(value.to_string())
            })
        })
        .await
    }

    pub async fn hover(&self, reference: &str) -> Result<String, String> {
        let selector = ref_selector(reference)?;
        let reference = reference.to_owned();
        self.with_session(|session| {
            Box::pin(async move {
                current_page(session)
                    .await?
                    .find_element(selector)
                    .await
                    .map_err(browser_error)?
                    .hover()
                    .await
                    .map_err(browser_error)?;
                Ok(format!("hovered {reference}"))
            })
        })
        .await
    }

    pub async fn tabs(&self) -> Result<String, String> {
        self.with_session(|session| {
            Box::pin(async move {
                let pages = session.browser.pages().await.map_err(browser_error)?;
                let mut tabs = Vec::new();
                for (index, page) in pages.iter().enumerate() {
                    tabs.push(json!({
                        "index": index,
                        "title": page.get_title().await.map_err(browser_error)?.unwrap_or_default(),
                        "url": page.url().await.map_err(browser_error)?.unwrap_or_default(),
                        "current": index == session.current,
                    }));
                }
                serde_json::to_string(&tabs).map_err(|error| error.to_string())
            })
        })
        .await
    }

    pub async fn tab_new(&self, url: &str) -> Result<String, String> {
        let url = url.to_owned();
        self.with_session(|session| {
            Box::pin(async move {
                // The tab opens blank and takes the passkeys and their guard
                // before any site's script runs in it.
                let page = session
                    .browser
                    .new_page("about:blank")
                    .await
                    .map_err(browser_error)?;
                passkey_sync(session, &page).await?;
                if !url.is_empty() {
                    page.goto(url.as_str()).await.map_err(browser_error)?;
                }
                let pages = session.browser.pages().await.map_err(browser_error)?;
                session.current = pages
                    .iter()
                    .position(|candidate| candidate.target_id() == page.target_id())
                    .unwrap_or_else(|| pages.len().saturating_sub(1));
                Ok(if url.is_empty() {
                    "opened new tab".to_owned()
                } else {
                    format!("opened new tab at {url}")
                })
            })
        })
        .await
    }

    pub async fn tab_select(&self, index: usize) -> Result<String, String> {
        self.with_session(|session| {
            Box::pin(async move {
                let pages = session.browser.pages().await.map_err(browser_error)?;
                let page = pages
                    .get(index)
                    .ok_or_else(|| format!("tab {index} not found"))?;
                page.bring_to_front().await.map_err(browser_error)?;
                session.current = index;
                Ok(format!("selected tab {index}"))
            })
        })
        .await
    }

    pub async fn tab_close(&self, index: Option<usize>) -> Result<String, String> {
        self.with_session(|session| {
            Box::pin(async move {
                let index = index.unwrap_or(session.current);
                let mut pages = session.browser.pages().await.map_err(browser_error)?;
                if index >= pages.len() {
                    return Err(format!("tab {index} not found"));
                }
                pages.remove(index).close().await.map_err(browser_error)?;
                let remaining = session.browser.pages().await.map_err(browser_error)?;
                if remaining.is_empty() {
                    session
                        .browser
                        .new_page("about:blank")
                        .await
                        .map_err(browser_error)?;
                    session.current = 0;
                } else {
                    session.current = session.current.min(remaining.len() - 1);
                }
                Ok(format!("closed tab {index}"))
            })
        })
        .await
    }

    pub async fn upload(&self, reference: &str, path: &Path) -> Result<String, String> {
        let selector = ref_selector(reference)?;
        let path = path.to_string_lossy().into_owned();
        self.with_session(|session| {
            Box::pin(async move {
                let page = current_page(session).await?;
                let element = page.find_element(selector).await.map_err(browser_error)?;
                page.execute(
                    SetFileInputFilesParams::builder()
                        .backend_node_id(element.backend_node_id)
                        .files(vec![path.clone()])
                        .build()
                        .map_err(|error| error.to_string())?,
                )
                .await
                .map_err(browser_error)?;
                Ok(format!("uploaded {path}"))
            })
        })
        .await
    }

    pub async fn dialog(&self, accept: bool, text: &str) -> Result<String, String> {
        let text = text.to_owned();
        self.with_session(|session| {
            Box::pin(async move {
                current_page(session)
                    .await?
                    .execute({
                        let builder = HandleJavaScriptDialogParams::builder().accept(accept);
                        let builder = if accept && !text.is_empty() {
                            builder.prompt_text(&text)
                        } else {
                            builder
                        };
                        builder.build().map_err(|error| error.to_string())?
                    })
                    .await
                    .map_err(browser_error)?;
                Ok(if accept {
                    "accepted dialog"
                } else {
                    "dismissed dialog"
                }
                .to_owned())
            })
        })
        .await
    }

    pub async fn history(&self, action: &str) -> Result<String, String> {
        let action = action.to_owned();
        self.with_session(|session| {
            Box::pin(async move {
                let page = current_page(session).await?;
                match action.as_str() {
                    "back" => {
                        page.evaluate("history.back()")
                            .await
                            .map_err(browser_error)?;
                    }
                    "forward" => {
                        page.evaluate("history.forward()")
                            .await
                            .map_err(browser_error)?;
                    }
                    "reload" => {
                        page.reload().await.map_err(browser_error)?;
                    }
                    _ => return Err(format!("unknown browser history action {action:?}")),
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
                Ok(action.to_owned())
            })
        })
        .await
    }

    pub async fn page_text_if_running(&self) -> Option<String> {
        let mut session = self.session.lock().await;
        let session = session.as_mut()?;
        let page = current_page(session).await.ok()?;
        evaluate_string(&page, "document.body ? document.body.innerText : ''")
            .await
            .ok()
    }

    pub async fn cookies(&self) -> Result<Value, String> {
        self.with_session(|session| {
            Box::pin(async move {
                let cookies = session.browser.get_cookies().await.map_err(browser_error)?;
                serde_json::to_value(cookies).map_err(|error| error.to_string())
            })
        })
        .await
    }

    pub async fn local_storage(&self) -> Result<Value, String> {
        self.with_session(|session| {
            Box::pin(async move {
                // A page without an origin (`about:blank`, a `data:` URL)
                // has no storage to save; the login is its cookies alone.
                evaluate_value(
                    &current_page(session).await?,
                    "(() => { try { return Object.fromEntries(Object.entries(localStorage)); } catch { return {}; } })()",
                )
                .await
            })
        })
        .await
    }

    /// Drops every cookie the browser holds for `sites` — each site and
    /// every host within it — and answers how many went. The browser is
    /// launched if it is not up, since its profile holds cookies either way.
    pub async fn forget_cookies(&self, sites: &[String]) -> Result<usize, String> {
        let sites = sites.to_vec();
        self.with_session(|session| {
            Box::pin(async move {
                let doomed: Vec<DeleteCookiesParams> = session
                    .browser
                    .get_cookies()
                    .await
                    .map_err(browser_error)?
                    .into_iter()
                    .filter(|cookie| crate::logins::within(&cookie.domain, &sites))
                    .map(|cookie| {
                        let mut params = DeleteCookiesParams::new(cookie.name);
                        params.domain = Some(cookie.domain);
                        params.path = Some(cookie.path);
                        params
                    })
                    .collect();
                let count = doomed.len();
                if count > 0 {
                    // Cookies belong to the browser context, so any page of
                    // it deletes them; the agent's tab is at hand.
                    current_page(session)
                        .await?
                        .delete_cookies(doomed)
                        .await
                        .map_err(browser_error)?;
                }
                Ok(count)
            })
        })
        .await
    }

    pub async fn restore(&self, cookies: Value, local_storage: Value) -> Result<(), String> {
        let cookies: Vec<CookieParam> =
            serde_json::from_value(cookies).map_err(|error| format!("saved cookies: {error}"))?;
        self.with_session(|session| Box::pin(async move {
            session.browser.set_cookies(cookies).await.map_err(browser_error)?;
            // A login saved with nothing in storage — every one the desk's
            // cookie import lands — leaves the page alone: the script would
            // only clear, and a page without an origin (`about:blank`, a
            // `data:` URL) refuses storage altogether.
            let entries = local_storage
                .as_object()
                .filter(|entries| !entries.is_empty());
            let Some(entries) = entries else {
                return Ok(());
            };
            let storage = serde_json::to_string(entries).map_err(|error| error.to_string())?;
            current_page(session)
                .await?
                .evaluate(format!(
                    "(() => {{ localStorage.clear(); for (const [key,value] of Object.entries({storage})) localStorage.setItem(key,value); }})()"
                ))
                .await
                .map_err(browser_error)?;
            Ok(())
        }))
        .await
    }
}

async fn alive(browser: &Browser) -> bool {
    matches!(
        tokio::time::timeout(LIVENESS, browser.version()).await,
        Ok(Ok(_))
    )
}

/// Collect the process so it is not left a zombie, then let the handler go.
async fn reap(mut session: BrowserSession) {
    let _ = session.browser.kill().await;
    let _ = session.browser.wait().await;
    session._handler.abort();
}

/// The tab the agent drives, carrying the teammate's passkeys and their
/// guard before anything is done on it.
async fn current_page(session: &mut BrowserSession) -> Result<Page, String> {
    let pages = session.browser.pages().await.map_err(browser_error)?;
    let page = if pages.is_empty() {
        session
            .browser
            .new_page("about:blank")
            .await
            .map_err(browser_error)?
    } else {
        session.current = session.current.min(pages.len() - 1);
        pages[session.current].clone()
    };
    passkey_sync(session, &page).await?;
    Ok(page)
}

/// Brings one tab's authenticator to the granted set, and its guard to the
/// arming. A tab is left untouched until there is a passkey to carry or an
/// arming to honour, and then it gets a virtual authenticator — internal,
/// resident, user-verifying, presence simulated, so a passkey signs in
/// without a prompt — and the guard in every document. Each look reads the
/// request the guard has parked, if any, records it for the person and
/// carries their answer back to the page; then it compares what the
/// authenticator holds with what it should: a granted or awaited passkey
/// missing is added, one the page minted under an approved request is kept
/// for the desk, and anything else is removed, so a passkey made outside
/// an arming, or without the person's approval, never survives the next
/// look. Answers the id of the request the tab holds, if it holds one.
async fn passkey_sync(session: &mut BrowserSession, page: &Page) -> Result<Option<String>, String> {
    let granted = session.secrets.passkeys();
    let armed = session.passkeys.armed().map(|(rp_id, _)| rp_id);
    let target = page.target_id().inner().clone();
    if granted.is_empty() && armed.is_none() && !session.authenticators.contains_key(&target) {
        return Ok(None);
    }
    if !session.authenticators.contains_key(&target) {
        page.execute(EnableParams::default())
            .await
            .map_err(browser_error)?;
        let mut options = VirtualAuthenticatorOptions::new(
            AuthenticatorProtocol::Ctap2,
            AuthenticatorTransport::Internal,
        );
        options.ctap2_version = Some(Ctap2Version::Ctap21);
        options.has_resident_key = Some(true);
        options.has_user_verification = Some(true);
        options.is_user_verified = Some(true);
        options.automatic_presence_simulation = Some(true);
        options.default_backup_eligibility = Some(true);
        options.default_backup_state = Some(true);
        let id = page
            .execute(AddVirtualAuthenticatorParams::new(options))
            .await
            .map_err(browser_error)?
            .result
            .authenticator_id;
        let guard = install_guard(page, &session.token, armed.as_deref()).await?;
        session.authenticators.insert(
            target.clone(),
            TabAuthenticator {
                id,
                guard,
                armed: armed.clone(),
            },
        );
    }
    let tab = session
        .authenticators
        .get_mut(&target)
        .expect("inserted above");
    if tab.armed != armed {
        page.execute(RemoveScriptToEvaluateOnNewDocumentParams::new(
            tab.guard.clone(),
        ))
        .await
        .map_err(browser_error)?;
        tab.guard = install_guard(page, &session.token, armed.as_deref()).await?;
        tab.armed = armed.clone();
    }
    // The document already open learns the arming too; one that has no
    // guard, such as a browser page of Chromium's own, has nothing to learn.
    let _ = page
        .evaluate(format!(
            "typeof __hotlineArm === 'function' && __hotlineArm({}, {})",
            json!(session.token),
            json!(armed)
        ))
        .await;
    let authenticator = tab.id.clone();
    let asked = look_at_request(session, page).await;
    let held = page
        .execute(GetCredentialsParams::new(authenticator.clone()))
        .await
        .map_err(browser_error)?
        .result
        .credentials;
    let mut wanted: Vec<Passkey> = granted.into_iter().map(|(_, passkey)| passkey).collect();
    wanted.extend(session.passkeys.minted());
    let held_ids: HashSet<String> = held
        .iter()
        .map(|credential| String::from(credential.credential_id.clone()))
        .collect();
    for credential in held {
        let id = String::from(credential.credential_id.clone());
        if wanted.iter().any(|passkey| passkey.credential_id == id) {
            continue;
        }
        // Not one this computer added: the page minted it. Under an arming
        // for its site, it is the one the desk is waiting for.
        let minted = passkey_from(credential, armed.as_deref());
        if session.passkeys.keep(minted.clone()) {
            wanted.push(minted);
            continue;
        }
        page.execute(RemoveCredentialParams::new(
            authenticator.clone(),
            Binary::from(id),
        ))
        .await
        .map_err(browser_error)?;
    }
    for passkey in wanted {
        if held_ids.contains(&passkey.credential_id) {
            continue;
        }
        page.execute(AddCredentialParams::new(
            authenticator.clone(),
            credential_from(&passkey),
        ))
        .await
        .map_err(|error| format!("browser: {}: {error}", passkey.rp_id))?;
    }
    Ok(asked)
}

/// Reads the request the tab's guard has parked, if any, records it as the
/// one before the person when none is, and tells the page the person's
/// answer once there is one. A page that cannot be asked — one navigating,
/// or one of Chromium's own — holds nothing. Answers the request's id.
async fn look_at_request(session: &mut BrowserSession, page: &Page) -> Option<String> {
    let reported = evaluate_value(
        page,
        &format!(
            "typeof __hotlineLook === 'function' ? __hotlineLook({}) : null",
            json!(session.token)
        ),
    )
    .await
    .ok()
    .and_then(|value| serde_json::from_value::<Option<Ask>>(value).ok())
    .flatten()?;
    let id = reported.id.clone();
    let answer = match session.passkeys.report(reported) {
        Reported::Approved => true,
        Reported::Denied => false,
        Reported::Waiting | Reported::Later => return Some(id),
    };
    let told = evaluate_value(
        page,
        &format!(
            "__hotlineAnswer({}, {}, {})",
            json!(session.token),
            json!(id),
            json!(answer)
        ),
    )
    .await;
    if told.ok().and_then(|value| value.as_bool()) == Some(true) {
        session.passkeys.delivered(&id);
    }
    Some(id)
}

/// Installs the guard for every document the tab opens from now, and the
/// one open now, armed for `armed`.
async fn install_guard(
    page: &Page,
    token: &str,
    armed: Option<&str>,
) -> Result<ScriptIdentifier, String> {
    let source = include_str!("../assets/browser-passkey-guard.js")
        .replace("__TOKEN__", &json!(token).to_string())
        .replace("__ARMED__", &json!(armed).to_string());
    let mut install = AddScriptToEvaluateOnNewDocumentParams::new(source);
    install.run_immediately = Some(true);
    Ok(page
        .execute(install)
        .await
        .map_err(browser_error)?
        .result
        .identifier)
}

/// A stored passkey as the authenticator takes it: resident, backed up as
/// far as the site is told, and with a signature counter that stays at
/// zero, since a counter that goes backwards would lock the account after
/// the credential is put into a second tab.
fn credential_from(passkey: &Passkey) -> Credential {
    let mut credential = Credential::new(
        Binary::from(passkey.credential_id.clone()),
        true,
        Binary::from(passkey.private_key.clone()),
        0,
    );
    credential.rp_id = Some(passkey.rp_id.clone());
    credential.user_handle = passkey.user_handle.clone().map(Binary::from);
    credential.user_name = passkey.user_name.clone();
    credential.user_display_name = passkey.user_display_name.clone();
    credential.backup_eligibility = Some(true);
    credential.backup_state = Some(true);
    credential
}

/// A credential the authenticator reports, as the desk stores it.
fn passkey_from(credential: Credential, armed: Option<&str>) -> Passkey {
    Passkey {
        rp_id: credential
            .rp_id
            .or_else(|| armed.map(str::to_owned))
            .unwrap_or_default(),
        credential_id: String::from(credential.credential_id),
        private_key: String::from(credential.private_key),
        user_handle: credential.user_handle.map(String::from),
        user_name: credential.user_name,
        user_display_name: credential.user_display_name,
    }
}

/// Sixteen random bytes, in hex: what the guard script wants before it
/// changes the armed site on a document that is already open.
fn session_token() -> Result<String, String> {
    let mut bytes = [0_u8; 16];
    std::io::Read::read_exact(
        &mut std::fs::File::open("/dev/urandom").map_err(|error| format!("browser: {error}"))?,
        &mut bytes,
    )
    .map_err(|error| format!("browser: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Runs the form script on one element, and answers what it reports when
/// it reports success.
async fn form_action_on(
    page: &Page,
    selector: &str,
    action: &str,
    requested: Value,
) -> Result<Value, String> {
    let script = format!(
        "({})(document.querySelector({}),{}, {})",
        include_str!("../assets/browser-form.js"),
        json!(selector),
        json!(action),
        requested
    );
    let value = evaluate_value(page, &script).await?;
    if value.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(value.to_string());
    }
    Ok(value)
}

async fn evaluate_value(page: &Page, script: &str) -> Result<Value, String> {
    let result = page.evaluate(script).await.map_err(browser_error)?;
    // Valid statements such as focus() have no JavaScript return value.
    if result.object().r#type == RemoteObjectType::Undefined {
        return Ok(Value::Null);
    }
    result.into_value().map_err(browser_error)
}

async fn evaluate_string(page: &Page, script: &str) -> Result<String, String> {
    let value = evaluate_value(page, script).await?;
    Ok(value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_owned))
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

fn browser_error(error: impl std::fmt::Display) -> String {
    format!("browser: {error}")
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")?
        .to_string_lossy()
        .split(':')
        .find_map(|directory| {
            let candidate = Path::new(directory).join(name);
            candidate.is_file().then_some(candidate)
        })
}

#[cfg(test)]
mod tests {
    #[test]
    fn stale_profile_locks_are_cleared_before_launch() {
        let profile = std::env::temp_dir().join(format!("hotline-profile-{}", std::process::id()));
        std::fs::create_dir_all(&profile).unwrap();
        for name in ["SingletonLock", "SingletonSocket", "SingletonCookie"] {
            std::os::unix::fs::symlink("old-host-62", profile.join(name)).unwrap();
        }
        std::fs::write(profile.join("Local State"), "kept").unwrap();
        super::BrowserManager::clear_profile_locks(&profile);
        assert!(profile.join("SingletonLock").symlink_metadata().is_err());
        assert!(profile.join("SingletonSocket").symlink_metadata().is_err());
        assert!(profile.join("SingletonCookie").symlink_metadata().is_err());
        assert_eq!(
            std::fs::read_to_string(profile.join("Local State")).unwrap(),
            "kept"
        );
        std::fs::remove_dir_all(profile).unwrap();
    }
}
