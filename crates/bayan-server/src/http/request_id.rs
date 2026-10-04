//! Request IDs.
//!
//! Every request gets a fresh ID that appears in its log line and in the `x-request-id` response header, so an operator can match a user's report to the log. IDs sent by clients are ignored and replaced: a client-chosen value would be untrusted text in our logs.

use std::fmt::Write as _;
use std::hash::{BuildHasher as _, Hasher as _};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::extract::{Request, State};
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;

/// The request and response header carrying the request ID.
pub const REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");

/// The ID assigned to a request, stored in the request's extensions.
#[derive(Debug, Clone)]
pub struct RequestId(pub String);

/// Generates IDs of the form `<process-prefix>-<counter>`: unique within a process, and distinct across restarts.
#[derive(Debug)]
pub(super) struct Generator {
    prefix: String,
    counter: AtomicU64,
}

impl Generator {
    pub(super) fn new() -> Self {
        // The standard library seeds `RandomState` from the operating system's random source, which gives a per-process random prefix without another dependency. It only needs to be distinct, not secret.
        let random = std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish();
        let mut prefix = String::with_capacity(16);
        let _ = write!(prefix, "{random:016x}");
        Self {
            prefix,
            counter: AtomicU64::new(0),
        }
    }

    fn next(&self) -> String {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        format!("{}-{n:x}", self.prefix)
    }
}

/// Middleware that replaces any client-supplied request ID with a fresh one and echoes it in the response.
pub(super) async fn assign(
    State(generator): State<Arc<Generator>>,
    mut request: Request,
    next: Next,
) -> Response {
    let id = generator.next();
    request.headers_mut().remove(REQUEST_ID_HEADER);
    let header = HeaderValue::from_str(&id).ok();
    if let Some(value) = &header {
        request
            .headers_mut()
            .insert(REQUEST_ID_HEADER, value.clone());
    }
    request.extensions_mut().insert(RequestId(id));
    let mut response = next.run(request).await;
    if let Some(value) = header {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    response
}
