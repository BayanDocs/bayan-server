//! Behavior of the HTTP application with the default SQLite database.

mod common;

use std::collections::BTreeMap;
use std::time::Duration;

use axum::Router;
use axum::routing::get;
use bayan_server::config::{Config, DatabaseConfig, SQLITE_FILE_NAME};
use bayan_server::http::{CONTENT_SECURITY_POLICY, SECURITY_HEADERS};
use common::{Server, TempDir, config};

/// bayan-web's security headers, copied by hand from bayan-web at commit 01c59788fa755eea8665332d88b18174a90e1a22 (the head of its `main` when this copy was taken, on 2026-10-28): `productionHeaders` in `config/security-headers.ts`, whose policy `serializeCsp` builds from `cspDirectives`, and which `deploy/nginx/security-headers.conf` repeats line by line (a unit test in bayan-web keeps those two equal).
///
/// The server must send exactly these, header by header (ADR-0014 and its amendment of 2026-10-12). It never reads bayan-web's files, so this copy is how drift on either side shows up in review: when bayan-web changes its headers, change `src/http/headers.rs`, this copy and the commit named here in one pull request.
const BAYAN_WEB_HEADERS: [(&str, &str); 8] = [
    (
        "Content-Security-Policy",
        "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self' blob:; font-src 'self'; connect-src 'self'; worker-src 'self'; manifest-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-src 'none'; frame-ancestors 'none'; require-trusted-types-for 'script'; trusted-types bayan-script-url",
    ),
    ("Cross-Origin-Opener-Policy", "same-origin"),
    ("Cross-Origin-Embedder-Policy", "require-corp"),
    ("Cross-Origin-Resource-Policy", "same-origin"),
    ("X-Content-Type-Options", "nosniff"),
    ("Referrer-Policy", "no-referrer"),
    (
        "Permissions-Policy",
        "camera=(), microphone=(), geolocation=(), payment=(), usb=(), serial=(), hid=()",
    ),
    ("Integrity-Policy", "blocked-destinations=(script)"),
];

/// Header names in lower case (HTTP header names are case-insensitive) mapped to their values.
fn by_name<'a>(headers: &[(&str, &'a str)]) -> BTreeMap<String, &'a str> {
    let map: BTreeMap<String, &str> = headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), *value))
        .collect();
    assert_eq!(map.len(), headers.len(), "a header is listed twice");
    map
}

#[test]
fn security_headers_are_bayan_webs() {
    let server = by_name(&SECURITY_HEADERS);
    let web = by_name(&BAYAN_WEB_HEADERS);
    // The policy directive by directive first, so that a difference is easy to read.
    let directives = |policy: &str| policy.split("; ").map(str::to_owned).collect::<Vec<_>>();
    assert_eq!(
        directives(CONTENT_SECURITY_POLICY),
        directives(web["content-security-policy"])
    );
    assert_eq!(server["content-security-policy"], CONTENT_SECURITY_POLICY);
    // No header missing, none added, every value equal.
    assert_eq!(server, web);
}

/// Asserts that `response` carries each of bayan-web's security headers exactly once, with bayan-web's value.
fn assert_security_headers(response: &common::Response) {
    for (name, value) in by_name(&BAYAN_WEB_HEADERS) {
        let sent: Vec<&str> = response
            .headers
            .iter()
            .filter(|(header, _)| *header == name)
            .map(|(_, value)| value.as_str())
            .collect();
        assert_eq!(
            sent,
            [value],
            "header {name} on a {} response",
            response.status
        );
    }
}

#[tokio::test]
async fn starts_with_sqlite_by_default() {
    let data = TempDir::new("sqlite-default");
    let config = config(data.path(), &[]);
    assert_eq!(
        config.database,
        DatabaseConfig::Sqlite {
            path: data.path().join(SQLITE_FILE_NAME)
        }
    );

    let server = Server::start(config).await;
    let ready = common::get(server.addr, "/readyz").await;
    assert_eq!(ready.status, 200);
    assert_eq!(ready.body_text(), "ready\n");
    assert_security_headers(&ready);
    assert!(
        data.path().join(SQLITE_FILE_NAME).is_file(),
        "the SQLite database is created in the data directory"
    );
    server.stop().await;

    // A second start reuses the existing, already migrated database.
    let server = Server::start(config_for(&data)).await;
    assert_eq!(common::get(server.addr, "/readyz").await.status, 200);
    server.stop().await;
}

fn config_for(data: &TempDir) -> Config {
    config(data.path(), &[])
}

#[tokio::test]
async fn health_and_version_endpoints() {
    let data = TempDir::new("endpoints");
    let server = Server::start(config(data.path(), &[])).await;

    let health = common::get(server.addr, "/healthz").await;
    assert_eq!(health.status, 200);
    assert_eq!(health.body_text(), "ok\n");
    assert_eq!(health.header("cache-control"), Some("no-store"));
    assert_security_headers(&health);

    let head = common::request(
        server.addr,
        b"HEAD /healthz HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert_eq!(head.status, 200);
    assert!(head.body.is_empty());
    assert_security_headers(&head);

    let version = common::get(server.addr, "/version").await;
    assert_eq!(version.status, 200);
    assert_eq!(version.header("content-type"), Some("application/json"));
    assert_security_headers(&version);
    let expected = format!(
        "{{\"name\":\"bayan-server\",\"version\":\"{}\",\"commit\":\"{}\"}}",
        env!("CARGO_PKG_VERSION"),
        bayan_server::version::BUILD_COMMIT
    );
    assert_eq!(version.body_text(), expected);

    let missing = common::get(server.addr, "/no-such-route").await;
    assert_eq!(missing.status, 404);
    assert_security_headers(&missing);

    let wrong_method = common::request(
        server.addr,
        b"DELETE /healthz HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert_eq!(wrong_method.status, 405);
    assert_security_headers(&wrong_method);

    server.stop().await;
}

#[tokio::test]
async fn request_ids_are_fresh_and_ignore_client_values() {
    let data = TempDir::new("request-id");
    let server = Server::start(config(data.path(), &[])).await;

    let first = common::request(
        server.addr,
        b"GET /healthz HTTP/1.1\r\nHost: test\r\nX-Request-Id: chosen-by-client\r\nConnection: close\r\n\r\n",
    )
    .await;
    let second = common::get(server.addr, "/healthz").await;
    let first_id = first
        .header("x-request-id")
        .expect("request ID on the first response");
    let second_id = second
        .header("x-request-id")
        .expect("request ID on the second response");
    assert_ne!(
        first_id, "chosen-by-client",
        "client-supplied request IDs are replaced"
    );
    assert_ne!(first_id, second_id);

    server.stop().await;
}

#[tokio::test]
async fn request_bodies_over_the_limit_are_rejected() {
    let data = TempDir::new("body-limit");
    let server = Server::start(config(
        data.path(),
        &[("BAYAN_MAX_REQUEST_BODY_BYTES", "1024")],
    ))
    .await;

    let body = "x".repeat(4096);
    let raw = format!(
        "POST /healthz HTTP/1.1\r\nHost: test\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let response = common::request(server.addr, raw.as_bytes()).await;
    assert_eq!(response.status, 413);
    assert_security_headers(&response);

    server.stop().await;
}

#[tokio::test]
async fn slow_requests_time_out() {
    let data = TempDir::new("timeout");
    let config = config(data.path(), &[("BAYAN_REQUEST_TIMEOUT_SECS", "1")]);
    let routes = Router::new().route(
        "/slow",
        get(|| async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            "too late"
        }),
    );
    let (addr, stop, server) = common::serve_routes(routes, &config).await;

    let started = std::time::Instant::now();
    let response = common::get(addr, "/slow").await;
    assert_eq!(response.status, 408);
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_security_headers(&response);
    let _ = stop.send(());
    server.await.expect("the server task does not panic");
}

#[tokio::test]
async fn serves_static_files_with_security_headers_and_no_traversal() {
    let data = TempDir::new("static");
    let web = data.path().join("web");
    std::fs::create_dir_all(web.join("assets")).expect("create web directory");
    std::fs::write(
        web.join("index.html"),
        "<!doctype html><title>BayanDocs</title>",
    )
    .expect("write index.html");
    std::fs::write(web.join("assets").join("app.js"), "export {};").expect("write app.js");
    std::fs::write(data.path().join("outside.txt"), "must not be served")
        .expect("write outside file");
    let config = config(
        data.path(),
        &[("BAYAN_WEB_DIR", web.to_str().expect("UTF-8 path"))],
    );
    let server = Server::start(config).await;

    let index = common::get(server.addr, "/").await;
    assert_eq!(index.status, 200);
    assert!(index.body_text().contains("<title>BayanDocs</title>"));
    assert_security_headers(&index);

    let script = common::get(server.addr, "/assets/app.js").await;
    assert_eq!(script.status, 200);
    assert!(
        script
            .header("content-type")
            .is_some_and(|value| value.contains("javascript")),
        "{:?}",
        script.headers
    );
    assert_security_headers(&script);

    // Routes still take precedence over files.
    assert_eq!(
        common::get(server.addr, "/healthz").await.body_text(),
        "ok\n"
    );

    for path in [
        "/../outside.txt",
        "/assets/../../outside.txt",
        "/%2e%2e/outside.txt",
        "/assets/%2e%2e%2f%2e%2e%2foutside.txt",
    ] {
        let response = common::get(server.addr, path).await;
        assert_ne!(response.status, 200, "{path} must not be served");
        assert!(
            !response.body_text().contains("must not be served"),
            "{path} leaked a file outside the web directory"
        );
    }

    server.stop().await;
}

#[tokio::test]
async fn shuts_down_gracefully() {
    let data = TempDir::new("shutdown");
    let server = Server::start(config(data.path(), &[("BAYAN_SHUTDOWN_GRACE_SECS", "5")])).await;
    let addr = server.addr;
    assert_eq!(common::get(addr, "/healthz").await.status, 200);
    // `stop` sends the shutdown signal and asserts that `run` returns Ok within 10 seconds.
    server.stop().await;
    assert!(
        tokio::net::TcpStream::connect(addr).await.is_err(),
        "the listener is closed after shutdown"
    );
}
