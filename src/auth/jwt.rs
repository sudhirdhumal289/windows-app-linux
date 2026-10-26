//! Minimal, unverified JWT claim reading (display purposes only — never for trust decisions).

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

/// Decodes the payload segment of a JWT into JSON.
#[must_use]
pub fn payload(jwt: &str) -> Option<serde_json::Value> {
    let segment = jwt.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD
        .decode(segment.trim_end_matches('=').as_bytes())
        .ok()?;
    serde_json::from_slice(&decoded).ok()
}

/// Returns a string claim from the token payload.
#[must_use]
pub fn claim(jwt: &str, key: &str) -> Option<String> {
    payload(jwt)?
        .get(key)?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}

/// Returns the first present claim of `keys`, in order.
#[must_use]
pub fn first_claim(jwt: &str, keys: &[&str]) -> Option<String> {
    let payload = payload(jwt)?;
    keys.iter().find_map(|key| {
        payload
            .get(*key)?
            .as_str()
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
    })
}

#[cfg(test)]
pub(crate) fn make_unsigned(claims: &serde_json::Value) -> String {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
    let body = URL_SAFE_NO_PAD.encode(claims.to_string().as_bytes());
    format!("{header}.{body}.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_claims() {
        let jwt = make_unsigned(&json!({"upn": "a@b.com", "name": "Ä B", "empty": ""}));
        assert_eq!(claim(&jwt, "upn").as_deref(), Some("a@b.com"));
        assert_eq!(claim(&jwt, "name").as_deref(), Some("Ä B"));
        assert_eq!(claim(&jwt, "empty"), None);
        assert_eq!(
            first_claim(&jwt, &["preferred_username", "empty", "upn"]).as_deref(),
            Some("a@b.com")
        );
        assert_eq!(claim("garbage", "upn"), None);
    }

    #[test]
    fn tolerates_padding() {
        let jwt = make_unsigned(&json!({"upn": "x"}));
        let mut parts: Vec<String> = jwt.split('.').map(String::from).collect();
        while !parts[1].len().is_multiple_of(4) {
            parts[1].push('=');
        }
        assert_eq!(claim(&parts.join("."), "upn").as_deref(), Some("x"));
    }
}
