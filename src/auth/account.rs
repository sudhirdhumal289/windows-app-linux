//! Signed-in account details derived from stored token claims.

use super::jwt;
use super::token_store::TokenStore;
use crate::models::TokenResponse;

const UPN_CLAIMS: &[&str] = &["preferred_username", "upn", "email", "unique_name"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub upn: Option<String>,
    pub display_name: Option<String>,
}

impl Account {
    #[must_use]
    pub fn from_tokens(tokens: &TokenResponse) -> Self {
        let tokens_in_order = tokens
            .id_token
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(tokens.access_token.as_str()));
        let mut upn = None;
        let mut display_name = None;
        for token in tokens_in_order {
            upn = upn.or_else(|| jwt::first_claim(token, UPN_CLAIMS));
            display_name = display_name.or_else(|| jwt::claim(token, "name"));
        }
        Self { upn, display_name }
    }

    /// Account for the currently stored tokens, if signed in.
    #[must_use]
    pub fn current() -> Option<Self> {
        TokenStore::global().load().map(|t| Self::from_tokens(&t))
    }

    /// Best label for the UI: display name, then UPN.
    #[must_use]
    pub fn label(&self) -> Option<&str> {
        self.display_name.as_deref().or(self.upn.as_deref())
    }
}

/// UPN of the signed-in user, if known.
#[must_use]
pub fn current_upn() -> Option<String> {
    Account::current().and_then(|a| a.upn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn prefers_id_token_claims() {
        let tokens = TokenResponse {
            token_type: "Bearer".into(),
            scope: None,
            expires_in: 1,
            ext_expires_in: None,
            access_token: jwt::make_unsigned(&json!({"upn": "at@x.com", "name": "AT"})),
            refresh_token: None,
            id_token: Some(jwt::make_unsigned(
                &json!({"preferred_username": "id@x.com"}),
            )),
            acquired_at: None,
            client_id: None,
            authority: None,
        };
        let account = Account::from_tokens(&tokens);
        assert_eq!(account.upn.as_deref(), Some("id@x.com"));
        assert_eq!(account.display_name.as_deref(), Some("AT"));
        assert_eq!(account.label(), Some("AT"));
    }
}
