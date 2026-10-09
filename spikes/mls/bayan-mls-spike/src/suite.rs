//! The MLS ciphersuites the spike uses (ADR-0016 Decision 2 and its amendment of 2026-10-04).

use openmls::prelude::Ciphersuite;

/// A ciphersuite, by the role it plays in the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Suite {
    /// `MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519` (0x0001): the classical suite ADR-0016 names. Since the amendment of 2026-10-04 it is for development and tests only until the post-quantum suite has a code point.
    Classical,
    /// `MLS_128_DHKEMX25519_CHACHA20POLY1305_SHA256_Ed25519` (0x0003): a second classical suite, used to test group re-creation between suites without provisional code points.
    ClassicalChaCha,
    /// **PROVISIONAL — test only.** `MLS_128_MLKEM768X25519_AES128GCM_SHA256_Ed25519`, the first hybrid post-quantum suite of draft-ietf-mls-pq-ciphersuites (ML-KEM-768 + X25519, "X-Wing", with AES-128-GCM, SHA-256 and Ed25519). OpenMLS 0.9 gives it the provisional code point 0x004F; IANA has not assigned one, so groups with it must never be persisted ([`Suite::is_provisional`]).
    #[cfg(feature = "provisional-pq")]
    ProvisionalHybridPq,
}

impl Suite {
    /// The suites this build supports.
    #[must_use]
    pub fn all() -> &'static [Suite] {
        &[
            Suite::Classical,
            Suite::ClassicalChaCha,
            #[cfg(feature = "provisional-pq")]
            Suite::ProvisionalHybridPq,
        ]
    }

    /// The OpenMLS ciphersuite.
    #[must_use]
    pub fn ciphersuite(self) -> Ciphersuite {
        match self {
            Suite::Classical => Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519,
            Suite::ClassicalChaCha => {
                Ciphersuite::MLS_128_DHKEMX25519_CHACHA20POLY1305_SHA256_Ed25519
            }
            #[cfg(feature = "provisional-pq")]
            Suite::ProvisionalHybridPq => {
                Ciphersuite::MLS_128_MLKEM768X25519_AES128GCM_SHA256_Ed25519
            }
        }
    }

    /// The suite of an OpenMLS ciphersuite, if the spike uses it.
    #[must_use]
    pub fn from_ciphersuite(ciphersuite: Ciphersuite) -> Option<Suite> {
        Suite::all()
            .iter()
            .copied()
            .find(|suite| suite.ciphersuite() == ciphersuite)
    }

    /// Whether the suite's code point is provisional (not assigned by IANA). Such groups exist only in memory, for measurements.
    #[must_use]
    pub fn is_provisional(self) -> bool {
        match self {
            Suite::Classical | Suite::ClassicalChaCha => false,
            #[cfg(feature = "provisional-pq")]
            Suite::ProvisionalHybridPq => true,
        }
    }

    /// A short label for reports; provisional suites say so.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Suite::Classical => "0x0001 X25519/AES-128-GCM/Ed25519",
            Suite::ClassicalChaCha => "0x0003 X25519/ChaCha20-Poly1305/Ed25519",
            #[cfg(feature = "provisional-pq")]
            Suite::ProvisionalHybridPq => {
                "0x004F ML-KEM-768+X25519/AES-128-GCM/Ed25519 (PROVISIONAL, test only)"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;

    #[wasm_bindgen_test(unsupported = test)]
    fn code_points() {
        assert_eq!(u16::from(Suite::Classical.ciphersuite()), 0x0001);
        assert_eq!(u16::from(Suite::ClassicalChaCha.ciphersuite()), 0x0003);
        assert!(!Suite::Classical.is_provisional());
        for suite in Suite::all() {
            assert_eq!(Suite::from_ciphersuite(suite.ciphersuite()), Some(*suite));
        }
    }

    #[cfg(feature = "provisional-pq")]
    #[wasm_bindgen_test(unsupported = test)]
    fn the_hybrid_suite_is_provisional() {
        assert_eq!(u16::from(Suite::ProvisionalHybridPq.ciphersuite()), 0x004F);
        assert!(Suite::ProvisionalHybridPq.is_provisional());
        assert!(Suite::ProvisionalHybridPq.label().contains("PROVISIONAL"));
    }
}
