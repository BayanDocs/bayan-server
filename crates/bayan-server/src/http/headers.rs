//! Security headers sent with every response (ADR-0014 §9).
//!
//! The server will serve the web app, so these headers are its first line of defense against script injection: a strict Content Security Policy that allows only same-origin scripts plus WebAssembly compilation and requires Trusted Types, `Integrity-Policy` so every script must carry Subresource Integrity, cross-origin isolation, and no third-party origins. They are set on every response, overriding anything a handler set, so no route can forget them.

use axum::extract::Request;
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;

/// The Content Security Policy. `'wasm-unsafe-eval'` allows compiling WebAssembly (the engine) and nothing else; there is no `'unsafe-inline'` or `'unsafe-eval'`.
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self' blob:; font-src 'self'; connect-src 'self'; worker-src 'self'; manifest-src 'self'; object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'; require-trusted-types-for 'script'";

/// Every security header and its value.
pub const SECURITY_HEADERS: [(&str, &str); 10] = [
    ("content-security-policy", CONTENT_SECURITY_POLICY),
    // Browsers refuse any script without a valid Subresource Integrity hash.
    ("integrity-policy", "blocked-destinations=(script)"),
    // Cross-origin isolation (needed later for WebAssembly threads, ADR-0014 §8).
    ("cross-origin-opener-policy", "same-origin"),
    ("cross-origin-embedder-policy", "require-corp"),
    ("cross-origin-resource-policy", "same-origin"),
    ("x-content-type-options", "nosniff"),
    ("x-frame-options", "DENY"),
    ("referrer-policy", "no-referrer"),
    (
        "permissions-policy",
        "camera=(), microphone=(), geolocation=(), payment=(), usb=(), browsing-topics=()",
    ),
    ("origin-agent-cluster", "?1"),
];

/// Middleware that sets every header in [`SECURITY_HEADERS`] on the response.
pub(super) async fn add_security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    for (name, value) in SECURITY_HEADERS {
        headers.insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
    response
}
