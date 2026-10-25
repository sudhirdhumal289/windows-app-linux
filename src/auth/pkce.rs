//! PKCE (RFC 7636) challenge and verifier generation.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ring::digest::{digest, SHA256};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl Default for Pkce {
    fn default() -> Self {
        Self::new()
    }
}

impl Pkce {
    #[must_use]
    pub fn new() -> Self {
        let mut random_bytes = [0u8; 32];
        random_bytes[0..16].copy_from_slice(Uuid::new_v4().as_bytes());
        random_bytes[16..32].copy_from_slice(Uuid::new_v4().as_bytes());

        let verifier = URL_SAFE_NO_PAD.encode(random_bytes);
        let hash = digest(&SHA256, verifier.as_bytes());
        let challenge = URL_SAFE_NO_PAD.encode(hash.as_ref());

        Self {
            verifier,
            challenge,
        }
    }
}
