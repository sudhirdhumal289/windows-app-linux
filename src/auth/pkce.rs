//! PKCE (RFC 7636) verifier and S256 challenge generation.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ring::digest::{digest, SHA256};
use ring::rand::{SecureRandom, SystemRandom};

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
        SystemRandom::new()
            .fill(&mut random_bytes)
            .expect("system RNG unavailable");
        let verifier = URL_SAFE_NO_PAD.encode(random_bytes);
        let challenge = URL_SAFE_NO_PAD.encode(digest(&SHA256, verifier.as_bytes()).as_ref());
        Self {
            verifier,
            challenge,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifier_is_rfc7636_compliant_and_unique() {
        let a = Pkce::new();
        let b = Pkce::new();
        assert_ne!(a.verifier, b.verifier);
        assert_eq!(a.verifier.len(), 43);
        assert!(a
            .verifier
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        let expected = URL_SAFE_NO_PAD.encode(digest(&SHA256, a.verifier.as_bytes()).as_ref());
        assert_eq!(a.challenge, expected);
    }
}
