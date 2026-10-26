//! Microsoft PKeyAuth (MS-PKAP) challenge parsing and response construction.
//!
//! Entra ID redirects to `urn:http-auth:PKeyAuth?SubmitUrl=…&Nonce=…&Context=…` to ask
//! for proof of a registered device. Without a device key we answer with the context
//! only, which Entra treats as "unregistered device" and continues the flow.

use super::pkap::DeviceIdentity;
use url::Url;

pub const CHALLENGE_PREFIX: &str = "urn:http-auth:PKeyAuth";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Challenge {
    pub submit_url: String,
    pub nonce: String,
    pub context: String,
}

impl Challenge {
    /// Parses a PKeyAuth challenge URI; returns `None` for any other URI.
    #[must_use]
    pub fn parse(uri: &str) -> Option<Self> {
        if !uri.starts_with(CHALLENGE_PREFIX) {
            return None;
        }
        let parsed = Url::parse(uri).ok()?;
        let mut submit_url = None;
        let mut nonce = String::new();
        let mut context = String::new();
        for (key, value) in parsed.query_pairs() {
            match key.as_ref() {
                "SubmitUrl" => submit_url = Some(value.into_owned()),
                "Nonce" => nonce = value.into_owned(),
                "Context" => context = value.into_owned(),
                _ => {}
            }
        }
        let submit_url = submit_url.filter(|u| u.starts_with("https://"))?;
        Some(Self {
            submit_url,
            nonce,
            context,
        })
    }

    /// `Authorization` header value answering this challenge. Signs with the device key
    /// when one is configured, otherwise responds with the context only.
    #[must_use]
    pub fn authorization(&self, identity: Option<&DeviceIdentity>) -> String {
        if let Some(identity) = identity {
            match identity.sign_challenge(&self.submit_url, &self.nonce, &self.context) {
                Ok(header) => return header,
                Err(e) => log::warn!("PKeyAuth signing failed, answering unsigned: {e}"),
            }
        }
        format!(r#"PKeyAuth Context="{}", Version="1.0""#, self.context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_challenge() {
        let uri = "urn:http-auth:PKeyAuth?SubmitUrl=https%3A%2F%2Flogin.microsoftonline.com%2Fcommon%2FDeviceAuthPKeyAuth&nonce=x&Nonce=abc&Context=ctx%3D1&Version=1.0";
        let c = Challenge::parse(uri).unwrap();
        assert_eq!(
            c.submit_url,
            "https://login.microsoftonline.com/common/DeviceAuthPKeyAuth"
        );
        assert_eq!(c.nonce, "abc");
        assert_eq!(c.context, "ctx=1");
        assert_eq!(
            c.authorization(None),
            r#"PKeyAuth Context="ctx=1", Version="1.0""#
        );
    }

    #[test]
    fn rejects_other_or_unsafe_uris() {
        assert!(Challenge::parse("https://login.microsoftonline.com/?code=1").is_none());
        assert!(Challenge::parse("urn:http-auth:PKeyAuth?Nonce=1").is_none());
        assert!(Challenge::parse("urn:http-auth:PKeyAuth?SubmitUrl=http%3A%2F%2Fevil").is_none());
    }
}
