//! The content-free logging check shared by `tests/logging_text.rs` and `tests/logging_json.rs` (SRV-001 AC-3, threat T20).
//!
//! Every log line the process writes, at the most verbose level and from every library, is captured while requests full of canary values reach the server: in bodies, `Authorization` and `Cookie` headers, query strings, paths, other headers, client-supplied request IDs and an unknown method. The database driver's log events that would quote a password file line or a URL parameter's value are emitted too. Not one canary may appear in the logs.

use std::io::Write;
use std::sync::{Arc, Mutex};

use super::{Server, TempDir, config};
use bayan_server::config::{LogFormat, LogLevel};

/// A log writer that appends to a shared buffer.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("log buffer lock")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The marker every canary contains; the logs must never contain it, in any letter case.
const CANARY: &str = "canary";

/// A request with `head` (request line and headers, each ending in CRLF), a matching `Content-Length` and `body`.
fn with_body(head: &str, body: &str) -> Vec<u8> {
    format!(
        "{head}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// Installs a capturing global subscriber for `format` at the most verbose level, sends the canary requests, and asserts that no canary was logged. Call it once per test binary.
pub async fn run(format: LogFormat) {
    let capture = Capture::default();
    let writer = capture.clone();
    tracing::subscriber::set_global_default(bayan_server::logging::subscriber(
        format,
        LogLevel::Trace,
        move || writer.clone(),
    ))
    .expect("this test binary installs the only global subscriber");

    let data = TempDir::new("logging");
    let web = data.path().join("web");
    std::fs::create_dir_all(&web).expect("create web directory");
    std::fs::write(web.join("index.html"), "<!doctype html>").expect("write index.html");
    let server = Server::start(config(
        data.path(),
        &[
            ("BAYAN_WEB_DIR", web.to_str().expect("UTF-8 path")),
            ("BAYAN_MAX_REQUEST_BODY_BYTES", "2048"),
        ],
    ))
    .await;

    let requests: Vec<Vec<u8>> = vec![
        // Credentials, cookies, a query string, a client request ID and identifying headers on a real route.
        b"GET /readyz?token=Canary-Query-1 HTTP/1.1\r\nHost: test\r\nAuthorization: Bearer Canary-Auth-1\r\nCookie: session=Canary-Cookie-1\r\nX-Request-Id: Canary-RequestId-1\r\nUser-Agent: Canary-Agent-1\r\nReferer: https://example.invalid/Canary-Referer-1\r\nX-Document-Title: Canary-Title-1\r\nConnection: close\r\n\r\n".to_vec(),
        // A body on a route that rejects the method.
        with_body("POST /healthz?q=Canary-Query-2 HTTP/1.1\r\nHost: test\r\nAuthorization: Basic Q2FuYXJ5LUF1dGgtMg==\r\nContent-Type: application/json\r\n", "{\"document\":\"Canary-Body-2 secret!\"}"),
        // A document-like path served by the static file fallback.
        with_body("PUT /Canary-Path-3/Quarterly%20Canary%20Report.docx?name=Canary-Query-3 HTTP/1.1\r\nHost: test\r\nCookie: a=Canary-Cookie-3\r\n", "Canary-Body-3"),
        b"GET /Canary-Path-4.html?x=Canary-Query-4 HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n".to_vec(),
        // A body over the size limit.
        with_body("POST /healthz HTTP/1.1\r\nHost: test\r\n", &"Canary-Body-5 ".repeat(300)),
        // A client-chosen method token.
        b"CANARYMETHOD /healthz HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n".to_vec(),
        // A malformed request.
        b"GET /Canary-Path-7 HTTP/1.1\r\nHost: test\r\nCanary-Malformed-Header-7\r\n\r\n".to_vec(),
    ];
    for raw in &requests {
        let _ = super::request(server.addr, raw).await;
    }
    // What sqlx's PostgreSQL driver logs for a malformed `.pgpass` line: the line itself, password included. The subscriber must drop it.
    tracing::warn!(
        target: "sqlx_postgres::options::pgpass",
        line = "db.internal:5432:bayan:bayan:Canary-Pgpass-8",
        "Malformed line in pgpass file: invalid escape"
    );
    // What it logs for a URL query parameter it does not read: the name and value, here libpq's key passphrase. The configuration check refuses such URLs; the subscriber must drop the event anyway.
    tracing::warn!(
        target: "sqlx_postgres::options::parse",
        key = "sslpassword",
        value = "Canary-UrlParameter-9",
        "ignoring unrecognized connect parameter"
    );
    server.stop().await;

    let logs = String::from_utf8(capture.0.lock().expect("log buffer lock").clone())
        .expect("logs are UTF-8");
    let request_lines = logs
        .lines()
        .filter(|line| line.contains("bayan_server::request"))
        .count();
    assert!(
        request_lines >= requests.len() - 1,
        "expected a log line per request, got {request_lines}:\n{logs}"
    );
    assert!(
        logs.contains("/readyz") && logs.contains("OTHER"),
        "request lines name route templates and methods:\n{logs}"
    );
    if let Some(line) = logs
        .lines()
        .find(|line| line.to_ascii_lowercase().contains(CANARY))
    {
        panic!("sensitive request data reached the logs: {line}\n\nfull log:\n{logs}");
    }
}
