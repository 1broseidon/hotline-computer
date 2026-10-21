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

use crate::{App, files_ui, logins, passkeys, secrets, tools, viewer};

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
            self.app
                .jobs
                .agent
                .client(&client.name, client.title.as_deref());
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
        let holder = context
            .extensions
            .get::<axum::http::request::Parts>()
            .and_then(|parts| parts.headers.get("X-Computer-Holder"))
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
            .unwrap_or("anonymous")
            .to_owned();
        self.note_client(Some(&context.meta), &context.peer);
        let arguments = request.arguments.map_or(Value::Null, Value::Object);
        let result = match tools::call(&self.app, &request.name, arguments, &holder).await {
            Ok(content) => CallToolResult::success(content),
            Err(error) => CallToolResult::error(vec![rmcp::model::ContentBlock::text(error)]),
        };
        Ok(result.into())
    }
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

/// Every route, behind the bearer check.
pub(crate) fn router(app: App) -> Router {
    let expected_token = app.config.token.clone();
    let tools_app = app.clone();
    let service: StreamableHttpService<ComputerTools, LocalSessionManager> =
        StreamableHttpService::new(
            move || {
                Ok(ComputerTools {
                    app: tools_app.clone(),
                })
            },
            Arc::new(LocalSessionManager::default()),
            StreamableHttpServerConfig::default()
                .disable_allowed_hosts()
                .with_max_request_body_bytes(MAX_REQUEST_BODY),
        );
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/", get(viewer::page))
        .route("/assets/fonts/{name}", get(font))
        .route("/ws", get(viewer::socket))
        .route("/files", get(viewer::files).post(viewer::upload))
        .route("/files/download", get(viewer::download))
        // The desk's door for the person's stored secrets: the whole set
        // goes in, and no method answers one. It is not among the open
        // routes below, so the bearer rides in the header as on `/mcp`.
        .route("/secrets", put(secrets::replace))
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
        .nest_service("/mcp", service)
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
        .with_state(app)
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
        app.jobs.agent.holder(holder);
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
        "/health" | "/" | "/ws" | "/files" | "/files/download"
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
async fn shutdown() {
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
    fn secret_comparison_needs_equal_bytes() {
        assert!(same_secret(b"Bearer token", b"Bearer token"));
        assert!(!same_secret(b"Bearer token", b"Bearer other"));
        assert!(!same_secret(b"", b"Bearer token"));
    }
}
