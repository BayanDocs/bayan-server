//! Security headers sent with every response (ADR-0014 §9 and its amendment of 2026-10-12).
//!
//! The server serves the web app (ADR-0015 §2), so these headers are the web app's first line of defense against script injection (threat model F16.3): a strict Content Security Policy that allows only same-origin scripts plus WebAssembly compilation and lets exactly one Trusted Types policy exist, `Integrity-Policy` so every script must carry Subresource Integrity, cross-origin isolation, and no third-party origins. They are set on every response, overriding anything a handler set, so no route can forget them.
//!
//! They are bayan-web's own set, header by header: `productionHeaders` in its `config/security-headers.ts`, which its `deploy/nginx/security-headers.conf` repeats. The server's own responses (health, readiness, version and errors) get the same set, because none of them needs a different one. The values are copied, not read from bayan-web, so `tests/http.rs` pins them against a named bayan-web commit; when bayan-web changes its headers, change this file and that copy together. (Making the web app's build the single source is later work: WEB-103 with SRV-105.)

use axum::extract::Request;
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;

/// The Content Security Policy, as bayan-web writes it.
///
/// Everything comes from the server's own origin; `img-src` also allows `blob:`, for images the engine renders. Scripts never run inline or from `eval`: `'wasm-unsafe-eval'` allows compiling WebAssembly (the engine) and nothing else. No plugins, no `<base>` element, no form submissions, no frames inside the page (`frame-src`) and no framing of the page by other pages (`frame-ancestors`).
///
/// Trusted Types: script sinks such as `innerHTML` and script URLs accept only typed values, and exactly one policy may create them, `bayan-script-url`, which turns the address of the engine's worker script into the value `new Worker()` requires and refuses every other address (ADR-0014, amendment of 2026-10-12, design detail 1). Without `'allow-duplicates'` no second policy of that name can be created, so nothing else can obtain one; any further policy needs an amendment of ADR-0014.
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self' blob:; font-src 'self'; connect-src 'self'; worker-src 'self'; manifest-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-src 'none'; frame-ancestors 'none'; require-trusted-types-for 'script'; trusted-types bayan-script-url";

/// Every security header and its value, in bayan-web's order.
///
/// `X-Frame-Options` and `Origin-Agent-Cluster` are deliberately absent, as in bayan-web: with the rest of the set, every browser that can run the app ignores them. The HTML standard has browsers ignore `X-Frame-Options` whenever the Content Security Policy has `frame-ancestors`, and it puts a cross-origin isolated page in an agent cluster of its own origin whatever `Origin-Agent-Cluster` says. A header added or changed here alone would make the server differ from bayan-web; propose it for bayan-web's set instead, unless the server's own API responses need it to differ, which must then be no weaker and explained here.
pub const SECURITY_HEADERS: [(&str, &str); 8] = [
    ("content-security-policy", CONTENT_SECURITY_POLICY),
    // Cross-origin isolation: required for SharedArrayBuffer (WebAssembly threads, ADR-0014 §8), and keeps other sites' windows and resources out of the app's process.
    ("cross-origin-opener-policy", "same-origin"),
    ("cross-origin-embedder-policy", "require-corp"),
    ("cross-origin-resource-policy", "same-origin"),
    ("x-content-type-options", "nosniff"),
    ("referrer-policy", "no-referrer"),
    // No camera, microphone, location, payments, or USB, serial or HID devices.
    (
        "permissions-policy",
        "camera=(), microphone=(), geolocation=(), payment=(), usb=(), serial=(), hid=()",
    ),
    // Browsers refuse any script without a valid Subresource Integrity hash.
    ("integrity-policy", "blocked-destinations=(script)"),
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
