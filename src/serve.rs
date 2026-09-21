use std::sync::Arc;

use axum::extract::Request;
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use rmcp::ErrorData;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use serde_json::{Value, json};

use crate::{App, logins, passkeys, secrets, tools, viewer};

const MAX_REQUEST_BODY: usize = 50 * 1024 * 1024 * 4 / 3 + 1024 * 1024;

#[derive(Clone)]
struct ComputerTools {
    app: App,
}

impl ServerHandler for ComputerTools {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(
            Implementation::new("hotline-computer", env!("CARGO_PKG_VERSION")),
        ).with_instructions("Read state action=info and state action=guide on connection. The running computer supplies its release-matched skill. Inspect repository requirements, then state prepare with packages or a local flake. Use shell managed jobs for commands and builds; browser refs for web forms; capture/input for native apps.")
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<rmcp::RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
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
        .nest_service("/mcp", service)
        .layer(axum::middleware::from_fn(
            move |request: Request, next: Next| {
                let expected_token = expected_token.clone();
                async move { authenticate(request, next, expected_token.as_deref()).await }
            },
        ))
        .with_state(app)
}

/// `/health` is open; the viewer page is open and its socket checks the
/// token itself, because a browser cannot send a bearer header on either.
async fn authenticate(request: Request, next: Next, token: Option<&str>) -> Response {
    // The page holds the token in its fragment, so what it opens itself
    // presents the token as a query, and those routes check it themselves.
    if matches!(
        request.uri().path(),
        "/health" | "/" | "/ws" | "/files" | "/files/download"
    ) || token.is_none()
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

    #[test]
    fn secret_comparison_needs_equal_bytes() {
        assert!(same_secret(b"Bearer token", b"Bearer token"));
        assert!(!same_secret(b"Bearer token", b"Bearer other"));
        assert!(!same_secret(b"", b"Bearer token"));
    }
}
