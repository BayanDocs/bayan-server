//! The HTTP application: routes, middleware and static files.
//!
//! Every request passes through, from the outside in: request-ID assignment, content-free request logging, the security headers of ADR-0014, a request timeout, and a request body size limit. [`serve`] runs the connections themselves, with a header-read timeout and graceful shutdown.

mod headers;
mod request_id;
mod serve;

use std::sync::Arc;

use axum::Router;
use axum::extract::{MatchedPath, Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::services::ServeDir;
use tower_http::timeout::TimeoutLayer;

pub use headers::{CONTENT_SECURITY_POLICY, SECURITY_HEADERS};
pub use request_id::{REQUEST_ID_HEADER, RequestId};
pub use serve::serve;

use crate::config::Config;
use crate::db::Database;
use crate::version;

/// Shared state of the HTTP handlers.
#[derive(Debug, Clone)]
pub struct AppState {
    /// The metadata database.
    pub database: Database,
}

/// Builds the complete application for `config`.
pub fn app(config: &Config, state: AppState) -> Router {
    let routes = Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/version", get(version_info))
        .with_state(state);
    let routes = match &config.web_dir {
        Some(dir) => {
            routes.fallback_service(ServeDir::new(dir).append_index_html_on_directories(true))
        }
        None => routes,
    };
    with_middleware(routes, config)
}

/// Wraps `routes` in the middleware every response goes through. Public so tests can wrap their own routes.
pub fn with_middleware(routes: Router, config: &Config) -> Router {
    let counter = Arc::new(request_id::Generator::new());
    // Layers added later wrap the earlier ones, so the request passes through them in reverse order.
    routes
        .layer(RequestBodyLimitLayer::new(config.max_request_body_bytes))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            config.request_timeout,
        ))
        .layer(middleware::from_fn(headers::add_security_headers))
        .layer(middleware::from_fn(log_request))
        .layer(middleware::from_fn_with_state(counter, request_id::assign))
}

/// Logs one line per request with only content-free facts: request ID, method, matched route template, status and duration.
async fn log_request(request: Request, next: Next) -> Response {
    let started = tokio::time::Instant::now();
    let method = loggable_method(request.method());
    // The route template, never the path the client sent: paths and query strings can carry identifiers or content.
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or("-", |path| known_route(path.as_str()));
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .map(|id| id.0.clone())
        .unwrap_or_default();
    let response = next.run(request).await;
    let status = response.status().as_u16();
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    tracing::info!(target: "bayan_server::request", request_id, method, route, status, duration_ms, "request");
    response
}

/// Standard methods are logged by name; anything else (extension methods are arbitrary client-chosen tokens) as `OTHER`.
fn loggable_method(method: &Method) -> &'static str {
    const STANDARD: [(Method, &str); 9] = [
        (Method::GET, "GET"),
        (Method::HEAD, "HEAD"),
        (Method::POST, "POST"),
        (Method::PUT, "PUT"),
        (Method::DELETE, "DELETE"),
        (Method::PATCH, "PATCH"),
        (Method::OPTIONS, "OPTIONS"),
        (Method::CONNECT, "CONNECT"),
        (Method::TRACE, "TRACE"),
    ];
    STANDARD
        .iter()
        .find(|(standard, _)| standard == method)
        .map_or("OTHER", |(_, name)| name)
}

/// Maps a matched route template to a static string, so logs can only ever contain templates defined in this file.
fn known_route(template: &str) -> &'static str {
    match template {
        "/healthz" => "/healthz",
        "/readyz" => "/readyz",
        "/version" => "/version",
        _ => "other",
    }
}

/// Liveness: the process is running and serving HTTP. Does not touch the database.
async fn healthz() -> Response {
    no_store((StatusCode::OK, "ok\n"))
}

/// Readiness: the server can handle requests, which needs a reachable, migrated database.
async fn readyz(State(state): State<AppState>) -> Response {
    if state.database.is_ready().await {
        no_store((StatusCode::OK, "ready\n"))
    } else {
        no_store((StatusCode::SERVICE_UNAVAILABLE, "not ready\n"))
    }
}

/// Name, version and source revision of the running server.
async fn version_info() -> Response {
    match serde_json::to_vec(&version::VersionInfo::current()) {
        Ok(body) => no_store((
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            )],
            body,
        )),
        Err(_) => no_store(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

fn no_store(response: impl IntoResponse) -> Response {
    let mut response = response.into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
