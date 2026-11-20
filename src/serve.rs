use std::sync::Arc;

use axum::extract::{Path, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use rmcp::ErrorData;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, Implementation,
    ListToolsResult, PaginatedRequestParams, RequestMetaObject, ServerCapabilities, ServerInfo,
};
use rmcp::service::{NotificationContext, Peer, RequestContext};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use serde_json::{Value, json};

use crate::{App, files_ui, holds, logins, passkeys, secrets, signin, tools, viewer};

const MAX_REQUEST_BODY: usize = 50 * 1024 * 1024 * 4 / 3 + 1024 * 1024;

#[derive(Clone)]
struct ComputerTools {
    app: App,
}

impl ComputerTools {
    /// Names the agent after the harness it connected through. A modern
    /// (2026-07-28) client says who it is in each request's `_meta`; an
    /// older one said so once, at its handshake. On the stateless path
    /// rmcp fills the handshake with its own name, which names no one.
    fn note_client(&self, meta: Option<&RequestMetaObject>, peer: &Peer<rmcp::RoleServer>) {
        let placeholder = Implementation::default();
        let client = meta
            .and_then(RequestMetaObject::client_info)
            .or_else(|| peer.peer_info().map(|info| info.client_info.clone()))
            .filter(|client| client.name != placeholder.name || client.title.is_some());
        if let Some(client) = client {
            match &self.app.runner {
                None => self
                    .app
                    .jobs
                    .agent
                    .client(&client.name, client.title.as_deref()),
                Some(runner) => runner.line.tell(
                    "agent.client",
                    json!({"name": client.name, "title": client.title}),
                ),
            }
        }
    }
}

impl ServerHandler for ComputerTools {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(
            Implementation::new("hotline-computer", env!("CARGO_PKG_VERSION")),
        ).with_instructions("Read state action=info and state action=guide on connection. The running computer supplies its release-matched skill. Inspect repository requirements, then state prepare with packages or a local flake. Use shell managed jobs for commands and builds; browser refs for web forms; capture/input for native apps.")
    }

    /// The harness shows on the desktop once it connects, before it acts.
    async fn on_initialized(&self, context: NotificationContext<rmcp::RoleServer>) {
        self.note_client(None, &context.peer);
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<rmcp::RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        // A modern client lists tools as it connects; name it then.
        self.note_client(Some(&context.meta), &context.peer);
        Ok(listing(&self.app.config.home.to_string_lossy()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let holder = match holder_of(
            context
                .extensions
                .get::<axum::http::request::Parts>()
                .and_then(|parts| parts.headers.get("X-Computer-Holder"))
                .and_then(|value| value.to_str().ok()),
        ) {
            Ok(holder) => holder,
            Err(error) => {
                return Ok(
                    CallToolResult::error(vec![rmcp::model::ContentBlock::text(error)]).into(),
                );
            }
        };
        self.note_client(Some(&context.meta), &context.peer);
        let arguments = request.arguments.map_or(Value::Null, Value::Object);
        let called = match &self.app.runner {
            None => tools::call(&self.app, &request.name, arguments, &holder).await,
            Some(runner) => {
                crate::split::call_tool(&self.app, runner, &request.name, arguments, &holder).await
            }
        };
        let result = match called {
            Ok(content) => CallToolResult::success(content),
            Err(error) => CallToolResult::error(vec![rmcp::model::ContentBlock::text(error)]),
        };
        Ok(result.into())
    }
}

/// The name a call acts under: what its `X-Computer-Holder` says, or
/// `anonymous`. Names that begin like the viewer's own are the person's and
/// no caller's to take: under one, a call would pass for the hold of the
/// person at the screen, act during it, and end it.
fn holder_of(header: Option<&str>) -> Result<String, String> {
    let holder = header
        .filter(|value| !value.is_empty())
        .unwrap_or("anonymous");
    if holder.starts_with(viewer::PERSON) {
        return Err(format!(
            "X-Computer-Holder {holder:?} is not a name a client may use: names beginning with {:?} belong to the person at the screen. Choose another.",
            viewer::PERSON
        ));
    }
    Ok(holder.to_owned())
}

/// The `tools/list` result, complete for the protocol a modern client
/// negotiates.
///
/// Since MCP 2026-07-28 a list result must carry `ttlMs` and `cacheScope`;
/// rmcp leaves both unset on a hand-built result, and a client that validates
/// the modern shape — Claude Code does — rejects the listing and the agent
/// sees no computer tools at all. Zero and private: the eight tools do not
/// change, but they reach one holder's desktop.
fn listing(home: &str) -> ListToolsResult {
    ListToolsResult::with_all_items(tools::descriptors(home))
        .with_ttl_ms(0)
        .with_cache_scope(CacheScope::Private)
}

pub async fn run(app: App) -> Result<(), String> {
    app.config.door()?;
    tokio::fs::create_dir_all(&app.config.home)
        .await
        .map_err(|error| format!("create {}: {error}", app.config.home.display()))?;
    app.jobs.initialize().await?;
    // Manifest workspaces get their services and start hooks back.
    tokio::spawn(crate::manifest::resume(app.clone()));
    // The folder handler and the portal's picker open the computer's own
    // file windows; a computer without a session bus simply has no portal.
    if let Err(error) = app.files.publish() {
        eprintln!("file windows: {error}");
    }
    tokio::spawn(crate::portal::serve(app.clone()));
    // The browser extension's passkeys and logins come through here.
    tokio::spawn(crate::bridge::serve(app.clone()));
    let jobs = app.jobs.clone();
    let observer = app.observer.clone();
    let address = app.config.addr.clone();
    let router = router(app);
    let listener = tokio::net::TcpListener::bind(&address)
        .await
        .map_err(|error| format!("bind {address}: {error}"))?;
    eprintln!("hotline-computer listening on {address}");
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            shutdown().await;
            jobs.shutdown().await;
            observer.shutdown().await;
        })
        .await
        .map_err(|error| error.to_string())
}

/// Every route, behind the bearer check: the single-user computer's door.
pub(crate) fn router(app: App) -> Router {
    let service = mcp(&app);
    gated(
        app.clone(),
        desktop_routes()
            .merge(secret_routes())
            .route("/health", get(|| async { "ok" }))
            .nest_service("/mcp", service),
    )
}

/// The split computer's door, which the broker serves: the secrets and the
/// tools here, and every page of the desktop's handed to the runner.
pub(crate) fn broker_router(app: App) -> Router {
    let service = mcp(&app);
    gated(
        app.clone(),
        secret_routes()
            .route("/health", get(crate::split::broker::health))
            // A home the single-user computer used, after the first split
            // boot. Bearer-only, like `/secrets`.
            .route("/migration", get(crate::split::broker::migration))
            .route(
                "/migration/answer",
                post(crate::split::broker::migration_answer),
            )
            .nest_service("/mcp", service)
            .fallback(crate::split::broker::proxy),
    )
}

/// The runner's side of the split computer: the desktop's own pages, for
/// connections the broker hands over once it has checked the bearer. It
/// has no secrets door and no tools door of its own.
pub(crate) fn runner_router(app: App) -> Router {
    let door = Arc::new(Door::from_env());
    desktop_routes()
        .layer(axum::middleware::from_fn(
            move |request: Request, next: Next| {
                let door = Arc::clone(&door);
                async move { guard(request, next, &door).await }
            },
        ))
        .with_state(app)
}

fn mcp(app: &App) -> StreamableHttpService<ComputerTools, LocalSessionManager> {
    let tools_app = app.clone();
    StreamableHttpService::new(
        move || {
            Ok(ComputerTools {
                app: tools_app.clone(),
            })
        },
        Arc::new(LocalSessionManager::default()),
        // `guard` below checks Host and Origin for every route, this
        // one included, so rmcp's own list for `/mcp` alone stays off.
        StreamableHttpServerConfig::default()
            .disable_allowed_hosts()
            .with_max_request_body_bytes(MAX_REQUEST_BODY),
    )
}

/// The desktop: the viewer and its socket, the Files panel, the file
/// manager, and the browser's pages and brought-over logins.
fn desktop_routes() -> Router<App> {
    Router::new()
        .route("/", get(viewer::page))
        .route("/assets/fonts/{name}", get(font))
        .route("/ws", get(viewer::socket))
        .route("/files", get(viewer::files).post(viewer::upload))
        .route("/files/download", get(viewer::download))
        // The web pages the browser has open, newest first, for the desk to
        // offer the person cookies on. Bearer-only, like `/secrets`.
        .route("/pages", get(pages))
        // The person taking a brought-over login back out of the browser,
        // by domain or whole. Bearer-only, like `/secrets`.
        .route("/logins/{name}", delete(logins::forget))
        // The file manager and picker. Their page carries a key of its own,
        // which grants listing and opening files and nothing else.
        .route("/fm/ui", get(files_ui::page))
        .route("/fm/list", get(files_ui::list))
        .route(
            "/fm/request/{id}",
            get(files_ui::request).post(files_ui::reply),
        )
        .route("/fm/request/{id}/cancel", post(files_ui::cancel))
        .route("/fm/open", post(files_ui::open))
        .route("/fm/terminal", post(files_ui::terminal))
}

/// What holds values: the person's secrets, the uses waiting for them, the
/// making of a passkey, and what this computer can keep.
fn secret_routes() -> Router<App> {
    Router::new()
        // What this computer keeps, and proof the bearer's holder answers.
        .route("/capabilities", get(crate::split::capabilities))
        // The key the attest is made with, handed once per boot to the
        // first bearer holder who asks: the desk, right after it starts it.
        .route("/capabilities/key", post(crate::split::attest_key))
        // The desk's door for the person's stored secrets: the whole set
        // goes in, and no method answers one. It is not among the open
        // routes below, so the bearer rides in the header as on `/mcp`.
        .route("/secrets", put(secrets::replace))
        // Uses of an ask-me key the computer is holding for the person, and
        // the desk carrying their answer back. Bearer-only, like `/secrets`.
        .route("/secrets/uses", get(holds::list))
        .route("/secrets/uses/answer", post(holds::answer))
        // The one moment a passkey is made: the desk arms it, polls for
        // the request the site makes and then for what was minted, carries
        // the person's answer back, and ends it. Bearer-only, like
        // `/secrets`.
        .route(
            "/passkeys/registration",
            put(passkeys::arm)
                .get(passkeys::registration)
                .delete(passkeys::disarm),
        )
        .route("/passkeys/registration/answer", post(passkeys::answer))
        // Shared sign-ins into the signed-in browser, and out of it.
        // Bearer-only, like `/secrets`.
        .route(
            "/sessions/{name}",
            put(signin::sessions::put).delete(signin::sessions::delete),
        )
        // The signed-in browser as a screen, in the viewer's protocol: the
        // broker's, since only it drives that browser. Its socket checks the
        // token itself, as `/ws` does.
        .route("/sites/ws", get(signin::screen::socket))
        // Sign-ins waiting on the person. Bearer-only, like `/secrets`.
        .route("/signin/pending", get(signin::pending::list))
}

/// The bearer check, the holder's name, and the Host and Origin guard
/// around `routes`.
fn gated(app: App, routes: Router<App>) -> Router {
    let expected_token = app.config.token.clone();
    let door = Arc::new(Door::from_env());
    routes
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            record_holder,
        ))
        .layer(axum::middleware::from_fn(
            move |request: Request, next: Next| {
                let expected_token = expected_token.clone();
                async move { authenticate(request, next, expected_token.as_deref()).await }
            },
        ))
        // Outermost: a request from the wrong place is turned away before
        // anything reads its token.
        .layer(axum::middleware::from_fn(
            move |request: Request, next: Next| {
                let door = Arc::clone(&door);
                async move { guard(request, next, &door).await }
            },
        ))
        .with_state(app)
}

/// Where a request may come from, beyond an address and this computer
/// itself: the names it is reached by through a proxy or a network, in
/// `HOTLINE_COMPUTER_ALLOWED_HOSTS`, and the other sites whose pages may
/// call it, in `HOTLINE_COMPUTER_ALLOWED_ORIGINS`. Both are comma-separated.
#[derive(Default)]
pub(crate) struct Door {
    hosts: Vec<String>,
    origins: Vec<String>,
}

impl Door {
    fn from_env() -> Self {
        let list = |name: &str| -> Vec<String> {
            std::env::var(name)
                .unwrap_or_default()
                .split(',')
                .map(|entry| entry.trim().trim_end_matches('/').to_ascii_lowercase())
                .filter(|entry| !entry.is_empty())
                .collect()
        };
        Self {
            hosts: list("HOTLINE_COMPUTER_ALLOWED_HOSTS"),
            origins: list("HOTLINE_COMPUTER_ALLOWED_ORIGINS"),
        }
    }

    /// A browser sends the name it looked up as `Host`. A page that rebinds
    /// its own name to this computer's address still sends its own name, so
    /// a name is only taken when it was listed. An address needs no listing:
    /// nothing can rebind one.
    fn admits_host(&self, authority: &str) -> bool {
        let authority = authority.to_ascii_lowercase();
        let host = host_of(&authority);
        let address = host
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
            .map_or_else(
                || host.parse::<std::net::Ipv4Addr>().is_ok(),
                |inner| inner.parse::<std::net::Ipv6Addr>().is_ok(),
            );
        address
            || host == "localhost"
            || self
                .hosts
                .iter()
                .any(|allowed| allowed == host || *allowed == authority)
    }

    /// A browser names the page a request came from as `Origin` whenever
    /// that is another site, and on every socket. The computer's own pages
    /// come from the address they call; anything else was listed or is
    /// turned away. No `Origin` is no browser, or a page of this computer.
    fn admits_origin(&self, origin: &str, authority: &str) -> bool {
        let origin = origin.trim().trim_end_matches('/').to_ascii_lowercase();
        if self.origins.contains(&origin) {
            return true;
        }
        origin
            .split_once("://")
            .is_some_and(|(_, from)| from == authority.to_ascii_lowercase())
    }
}

/// The host of `host[:port]`, an IPv6 address kept in its brackets. What
/// does not have that shape comes back whole, and is then no host anyone
/// listed.
fn host_of(authority: &str) -> &str {
    let port = |port: &str| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit());
    if authority.starts_with('[') {
        return match authority.find(']') {
            Some(end) => match &authority[end + 1..] {
                "" => authority,
                rest if rest.strip_prefix(':').is_some_and(port) => &authority[..=end],
                _ => authority,
            },
            None => authority,
        };
    }
    match authority.rsplit_once(':') {
        Some((host, after)) if port(after) => host,
        _ => authority,
    }
}

/// Turns away a request addressed to a name this computer does not go by,
/// or sent by a page of another site. The bearer already keeps a stranger
/// out; this keeps a page in the person's own browser from reaching the
/// computer with whatever that browser can do. `/health` says only "ok".
async fn guard(request: Request, next: Next, door: &Door) -> Response {
    if request.uri().path() == "/health" {
        return next.run(request).await;
    }
    let refuse =
        |error: &str| (StatusCode::FORBIDDEN, Json(json!({ "error": error }))).into_response();
    let authority = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| {
            request
                .uri()
                .authority()
                .map(|authority| authority.as_str())
        })
        .unwrap_or("")
        .to_owned();
    if !door.admits_host(&authority) {
        return refuse(
            "host not allowed: reach this computer by address or localhost, or list the name in HOTLINE_COMPUTER_ALLOWED_HOSTS",
        );
    }
    if let Some(origin) = request.headers().get(header::ORIGIN) {
        let admitted = origin
            .to_str()
            .is_ok_and(|origin| door.admits_origin(origin, &authority));
        if !admitted {
            return refuse(
                "origin not allowed: list the calling site in HOTLINE_COMPUTER_ALLOWED_ORIGINS",
            );
        }
    }
    next.run(request).await
}

/// `GET /pages`: `{"pages": [{"origin", "url", "title", "openedAt"}]}`,
/// each top-level tab of the browser on an http or https page, newest
/// first. No browser running is no page open.
async fn pages(State(app): State<App>) -> Response {
    match app.browser.open_pages().await {
        Ok(pages) => Json(json!({ "pages": pages })).into_response(),
        Err(error) => (StatusCode::BAD_GATEWAY, Json(json!({ "error": error }))).into_response(),
    }
}

/// This runs after the existing bearer gate. Names describe the agent;
/// they never select a lease or grant access to a tool.
async fn record_holder(State(app): State<App>, request: Request, next: Next) -> Response {
    if (request.uri().path() == "/mcp" || request.uri().path().starts_with("/mcp/"))
        && let Some(holder) = request
            .headers()
            .get("X-Computer-Holder")
            .and_then(|v| v.to_str().ok())
    {
        match &app.runner {
            None => app.jobs.agent.holder(holder),
            Some(runner) => runner.line.tell("agent.holder", json!({"holder": holder})),
        }
    }
    next.run(request).await
}

/// `/health` is open; the viewer page is open and its socket checks the
/// token itself, because a browser cannot send a bearer header on either.
async fn authenticate(request: Request, next: Next, token: Option<&str>) -> Response {
    // The page holds the token in its fragment, so what it opens itself
    // presents the token as a query, and those routes check it themselves.
    if matches!(
        request.uri().path(),
        "/health" | "/" | "/ws" | "/sites/ws" | "/files" | "/files/download"
    ) || request.uri().path().starts_with("/assets/fonts/")
        || request.uri().path().starts_with("/fm/")
        || token.is_none()
    {
        return next.run(request).await;
    }
    let expected = format!("Bearer {}", token.expect("checked above"));
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if same_secret(presented.as_bytes(), expected.as_bytes()) {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"unauthorized"})),
        )
            .into_response()
    }
}

/// The open font route contains embedded public assets, never filesystem paths.
async fn font(Path(name): Path<String>) -> Response {
    let (bytes, mime): (&[u8], &str) = match name.as_str() {
        "Geist-Regular.ttf" => (
            include_bytes!("../assets/fonts/Geist-Regular.ttf"),
            "font/ttf",
        ),
        "Geist-Medium.ttf" => (
            include_bytes!("../assets/fonts/Geist-Medium.ttf"),
            "font/ttf",
        ),
        "Geist-SemiBold.ttf" => (
            include_bytes!("../assets/fonts/Geist-SemiBold.ttf"),
            "font/ttf",
        ),
        "GeistMono-Regular.ttf" => (
            include_bytes!("../assets/fonts/GeistMono-Regular.ttf"),
            "font/ttf",
        ),
        "GeistMono-Medium.ttf" => (
            include_bytes!("../assets/fonts/GeistMono-Medium.ttf"),
            "font/ttf",
        ),
        "Geist-Regular.woff2" => (
            include_bytes!("../assets/fonts/Geist-Regular.woff2"),
            "font/woff2",
        ),
        "Geist-Medium.woff2" => (
            include_bytes!("../assets/fonts/Geist-Medium.woff2"),
            "font/woff2",
        ),
        "Geist-SemiBold.woff2" => (
            include_bytes!("../assets/fonts/Geist-SemiBold.woff2"),
            "font/woff2",
        ),
        "GeistMono-Regular.woff2" => (
            include_bytes!("../assets/fonts/GeistMono-Regular.woff2"),
            "font/woff2",
        ),
        "GeistMono-Medium.woff2" => (
            include_bytes!("../assets/fonts/GeistMono-Medium.woff2"),
            "font/woff2",
        ),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    (
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, "public, max-age=31536000"),
        ],
        bytes,
    )
        .into_response()
}

pub(crate) fn same_secret(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

/// Ctrl-C at a terminal, or the SIGTERM a container stop forwards.
pub(crate) async fn shutdown() {
    let mut terminate =
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(signal) => signal,
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A client on protocol 2026-07-28 requires the cache hints on a list
    /// result and refuses the whole listing without them; this is what hid
    /// the computer from a Claude Code teammate.
    #[test]
    fn the_listing_carries_the_cache_hints_a_modern_client_requires() {
        let wire = serde_json::to_value(listing("/home/agent")).unwrap();
        assert_eq!(wire["ttlMs"], json!(0), "{wire}");
        assert_eq!(wire["cacheScope"], json!("private"), "{wire}");
        assert_eq!(wire["tools"].as_array().unwrap().len(), 8, "{wire}");
    }

    #[tokio::test]
    async fn public_fonts_are_allowlisted_and_never_resolve_paths() {
        for name in [
            "missing.ttf",
            "..",
            "../Geist-Regular.ttf",
            "%2e%2e",
            "OFL.txt",
        ] {
            assert_eq!(
                font(Path(name.into())).await.status(),
                StatusCode::NOT_FOUND
            );
        }
        let router = Router::new()
            .route("/assets/fonts/{name}", get(font))
            .route("/private", get(|| async { "private" }))
            .layer(axum::middleware::from_fn(
                |request: Request, next: Next| async {
                    authenticate(request, next, Some("test-only")).await
                },
            ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let asset = http
            .get(format!("{origin}/assets/fonts/Geist-Regular.woff2"))
            .send()
            .await
            .unwrap();
        assert_eq!(asset.status(), StatusCode::OK);
        assert_eq!(asset.headers()[header::CONTENT_TYPE], "font/woff2");
        assert_eq!(
            asset.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000"
        );
        assert_eq!(
            asset.bytes().await.unwrap().as_ref(),
            include_bytes!("../assets/fonts/Geist-Regular.woff2")
        );
        for path in ["missing.ttf", "%2e%2e%2fGeist-Regular.ttf"] {
            assert_eq!(
                http.get(format!("{origin}/assets/fonts/{path}"))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::NOT_FOUND
            );
        }
        assert_eq!(
            http.get(format!("{origin}/private"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        server.abort();
    }

    #[tokio::test]
    async fn only_an_authenticated_mcp_request_updates_the_displayed_name() {
        let home = tempfile::tempdir().unwrap();
        let app = App::new(crate::Config {
            addr: "127.0.0.1:0".into(),
            token: Some("test-only".into()),
            home: home.path().into(),
            display: String::new(),
            screen: "1920x1080".into(),
        });
        let identity = app.jobs.agent.clone();
        let before = identity.snapshot().name;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, router(app)).await.unwrap();
        });
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let denied = http
            .post(format!("{origin}/mcp"))
            .header("X-Computer-Holder", "Untrusted")
            .send()
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(identity.snapshot().name, before);
        let _ = http
            .post(format!("{origin}/mcp"))
            .bearer_auth("test-only")
            .header("X-Computer-Holder", "Tad")
            .send()
            .await
            .unwrap();
        if std::env::var("HOTLINE_COMPUTER_AGENT")
            .unwrap_or_default()
            .trim()
            .is_empty()
        {
            assert_eq!(identity.snapshot().name, "Tad");
        }
        server.abort();
    }

    #[test]
    fn a_client_cannot_name_itself_the_person() {
        assert_eq!(holder_of(None).unwrap(), "anonymous");
        assert_eq!(holder_of(Some("")).unwrap(), "anonymous");
        assert_eq!(holder_of(Some("Tad")).unwrap(), "Tad");
        assert_eq!(holder_of(Some("Personal")).unwrap(), "Personal");
        for taken in ["person", "person-1", "person-7 ", "personal-agent"] {
            assert!(holder_of(Some(taken)).is_err(), "{taken}");
        }
    }

    #[test]
    fn a_request_is_taken_by_address_or_listed_name_only() {
        let door = Door::default();
        for host in [
            "127.0.0.1",
            "127.0.0.1:8787",
            "192.168.65.89:8787",
            "localhost",
            "LOCALHOST:49152",
            "[::1]",
            "[::1]:8787",
        ] {
            assert!(door.admits_host(host), "{host}");
        }
        for host in [
            "",
            "evil.example",
            "evil.example:8787",
            "127.0.0.1.evil.example",
            "localhost.evil.example:8787",
            "[::1].evil.example",
            "::1",
            "computer.internal",
        ] {
            assert!(!door.admits_host(host), "{host}");
        }
        let listed = Door {
            hosts: vec!["computer.internal".into(), "desk.example:8443".into()],
            origins: vec!["http://localhost:6274".into()],
        };
        assert!(listed.admits_host("Computer.Internal:8787"));
        assert!(listed.admits_host("desk.example:8443"));
        assert!(!listed.admits_host("desk.example:9000"));
        assert!(!listed.admits_host("evil.example"));

        // The computer's own pages, whatever the scheme a proxy gave them.
        assert!(door.admits_origin("http://127.0.0.1:8787", "127.0.0.1:8787"));
        assert!(door.admits_origin("https://LocalHost:8787", "localhost:8787"));
        assert!(door.admits_origin("http://[::1]:8787", "[::1]:8787"));
        for origin in [
            "null",
            "http://evil.example",
            "http://127.0.0.1:9999",
            "http://127.0.0.1",
            "127.0.0.1:8787",
            "chrome-extension://abcdef",
        ] {
            assert!(!door.admits_origin(origin, "127.0.0.1:8787"), "{origin}");
        }
        assert!(listed.admits_origin("http://localhost:6274/", "127.0.0.1:8787"));
        assert!(!listed.admits_origin("http://localhost:6275", "127.0.0.1:8787"));
    }

    #[tokio::test]
    async fn another_sites_page_and_a_rebound_name_are_turned_away_before_the_token() {
        let home = tempfile::tempdir().unwrap();
        let app = App::new(crate::Config {
            addr: "127.0.0.1:0".into(),
            token: Some("test-only".into()),
            home: home.path().into(),
            display: String::new(),
            screen: "1920x1080".into(),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let origin = format!("http://{address}");
        let server = tokio::spawn(async move {
            axum::serve(listener, router(app)).await.unwrap();
        });
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let ask = |host: Option<&str>, from: Option<&str>, path: &str| {
            let mut request = http.get(format!("{origin}{path}")).bearer_auth("test-only");
            if let Some(host) = host {
                request = request.header(header::HOST, host);
            }
            if let Some(from) = from {
                request = request.header(header::ORIGIN, from);
            }
            async move { request.send().await.unwrap().status() }
        };
        // The desk and every MCP client: an address, no Origin.
        assert_eq!(ask(None, None, "/secrets/uses").await, StatusCode::OK);
        // The computer's own page calling home.
        assert_eq!(
            ask(None, Some(&origin), "/secrets/uses").await,
            StatusCode::OK
        );
        // A page of another site, even holding the right token.
        assert_eq!(
            ask(None, Some("https://evil.example"), "/secrets/uses").await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            ask(None, Some("https://evil.example"), "/").await,
            StatusCode::FORBIDDEN
        );
        // A name rebound to this address.
        let rebound = format!("evil.example:{}", address.port());
        assert_eq!(
            ask(Some(&rebound), None, "/secrets/uses").await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            ask(Some(&rebound), Some(&format!("http://{rebound}")), "/").await,
            StatusCode::FORBIDDEN
        );
        // A probe needs neither.
        assert_eq!(
            ask(Some(&rebound), Some("https://evil.example"), "/health").await,
            StatusCode::OK
        );
        server.abort();
    }

    #[test]
    fn a_computer_with_no_token_does_not_start_by_accident() {
        let config = |token: Option<&str>| crate::Config {
            addr: "0.0.0.0:8787".into(),
            token: token.map(str::to_owned),
            home: "/home/agent".into(),
            display: String::new(),
            screen: "1920x1080".into(),
        };
        assert!(config(Some("a-token")).door().is_ok());
        if std::env::var("HOTLINE_COMPUTER_ALLOW_NO_TOKEN").is_err() {
            let refusal = config(None).door().unwrap_err();
            assert!(refusal.contains("HOTLINE_COMPUTER_TOKEN"), "{refusal}");
        }
    }

    #[test]
    fn secret_comparison_needs_equal_bytes() {
        assert!(same_secret(b"Bearer token", b"Bearer token"));
        assert!(!same_secret(b"Bearer token", b"Bearer other"));
        assert!(!same_secret(b"", b"Bearer token"));
    }
}
