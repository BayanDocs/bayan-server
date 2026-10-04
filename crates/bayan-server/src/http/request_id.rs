//! Request IDs.
//!
//! Every request gets a fresh ID that appears in its log line and in the `x-request-id` response header, so an operator can match a user's report to the log. IDs sent by clients are ignored and replaced: a client-chosen value would be untrusted text in our logs.

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher as _;
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

/// Generates opaque request IDs: 32 hexadecimal digits, unique within a process, that reveal nothing about how many requests the server has handled (metadata minimization, threat T9).
///
/// Each ID is a keyed hash of a counter. The key is a standard-library `RandomState`, seeded from the operating system's random source, so it differs in every process and clients never learn it. The standard hasher (`SipHash`) keeps outputs for different inputs unrelated to anyone without the key, so consecutive IDs cannot be compared to count requests. Two 64-bit hashes make collisions practically impossible.
#[derive(Debug)]
pub(super) struct Generator {
    key: RandomState,
    counter: AtomicU64,
}

impl Generator {
    pub(super) fn new() -> Self {
        Self {
            key: RandomState::new(),
            counter: AtomicU64::new(0),
        }
    }

    pub(super) fn next(&self) -> String {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        let high = self.key.hash_one((n, 0_u8));
        let low = self.key.hash_one((n, 1_u8));
        format!("{high:016x}{low:016x}")
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::Generator;

    fn shared_prefix_len(a: &str, b: &str) -> usize {
        a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count()
    }

    /// Regression test (SRV-001 review): IDs returned to clients must not reveal how many requests were handled in between. A counter shows up as a shared prefix or as consecutive IDs that differ in only a few digits; keyed hashes show neither.
    #[test]
    fn ids_are_opaque_and_unique() {
        let generator = Generator::new();
        let ids: Vec<String> = (0..1000).map(|_| generator.next()).collect();
        for id in &ids {
            assert!(
                id.len() == 32
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "unexpected ID format: {id}"
            );
        }
        assert_eq!(
            ids.iter().collect::<BTreeSet<_>>().len(),
            ids.len(),
            "IDs are unique"
        );
        let shared = ids[1..]
            .iter()
            .map(|id| shared_prefix_len(&ids[0], id))
            .min();
        assert!(shared < Some(4), "IDs share a prefix of {shared:?} digits");
        for pair in ids.windows(2) {
            let same = pair[0]
                .bytes()
                .zip(pair[1].bytes())
                .filter(|(a, b)| a == b)
                .count();
            assert!(
                same < 16,
                "consecutive IDs {} and {} look related",
                pair[0],
                pair[1]
            );
        }
        // Another process has another key, so the same counter values give different IDs.
        assert_ne!(Generator::new().next(), ids[0]);
    }
}
