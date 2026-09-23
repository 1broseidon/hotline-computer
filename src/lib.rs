pub mod a11y;
pub mod boot;
pub mod browser;
pub mod clipboard;
pub mod desktop;
pub mod display;
pub mod draft;
pub mod guide;
pub mod jobs;
pub mod lease;
pub mod limits;
pub mod logins;
pub mod manifest;
pub mod observer;
pub mod paint;
pub mod passkeys;
pub mod ready;
pub mod screen;
pub mod secrets;
pub mod serve;
pub mod tools;
pub mod viewer;
pub mod workspace;
pub mod x11;
pub mod xtest;

use std::path::PathBuf;
use std::sync::Arc;

use browser::BrowserManager;
use lease::MachineAccess;

#[derive(Clone, Debug)]
pub struct Config {
    pub addr: String,
    pub token: Option<String>,
    pub home: PathBuf,
    pub display: String,
    /// The Xvfb screen `boot` creates, as `WIDTHxHEIGHT`.
    pub screen: String,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            addr: std::env::var("HOTLINE_COMPUTER_ADDR")
                .unwrap_or_else(|_| "0.0.0.0:8787".to_owned()),
            token: std::env::var("HOTLINE_COMPUTER_TOKEN")
                .ok()
                .filter(|value| !value.is_empty()),
            home: std::env::var_os("HOTLINE_COMPUTER_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/home/agent")),
            display: std::env::var("DISPLAY").unwrap_or_else(|_| ":0".to_owned()),
            screen: std::env::var("HOTLINE_COMPUTER_SCREEN")
                .unwrap_or_else(|_| "1920x1080".to_owned()),
        }
    }
}

#[derive(Clone)]
pub struct App {
    pub config: Arc<Config>,
    pub access: MachineAccess,
    pub browser: BrowserManager,
    pub jobs: jobs::Jobs,
    /// The person's stored secrets: what a job starts with, and what every
    /// answer leaves out.
    pub secrets: secrets::Secrets,
    /// The arming under which the browser may mint a passkey.
    pub passkeys: passkeys::Passkeys,
    pub observer: observer::Observer,
    pub a11y: Arc<tokio::sync::OnceCell<atspi::zbus::Connection>>,
    /// The screen stream, the hands, and the clipboard; `None` until a display is up.
    pub display: Option<Arc<display::Display>>,
}

impl App {
    pub fn new(config: Config) -> Self {
        let config = Arc::new(config);
        let secrets = secrets::Secrets::default();
        let passkeys = passkeys::Passkeys::default();
        Self {
            access: MachineAccess::new().on_display(&config.display),
            a11y: Arc::new(tokio::sync::OnceCell::new()),
            jobs: jobs::Jobs::new(&config.home, &config.display, secrets.clone()),
            browser: BrowserManager::new(Arc::clone(&config), secrets.clone(), passkeys.clone()),
            secrets,
            passkeys,
            observer: observer::Observer::new(&config.home, &config.display),
            config,
            display: None,
        }
    }

    pub fn with_display(mut self, display: display::Display) -> Self {
        self.display = Some(Arc::new(display));
        self
    }

    /// The display, or the sentence a tool gives when there is none.
    pub fn display(&self) -> Result<Arc<display::Display>, String> {
        self.display
            .clone()
            .ok_or_else(|| "this computer has no display".to_owned())
    }
}
