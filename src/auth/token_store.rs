//! Persistent OAuth token cache (`tokens.json`, owner-only) with an in-memory copy.
//!
//! Every signed-in account is also kept in an owner-only vault (`accounts/<key>.json`,
//! keyed by a hash of the UPN), so users can switch between work accounts without
//! signing in again. `tokens.json` always holds the active account.

use super::account::Account;
use crate::config::{storage, AppConfig};
use crate::models::TokenResponse;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// File-backed token store. All access goes through [`TokenStore::global`] in the app;
/// tests construct isolated instances.
pub struct TokenStore {
    path: PathBuf,
    /// `None` until first read from disk.
    cache: Mutex<Option<Option<TokenResponse>>>,
}

impl TokenStore {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            cache: Mutex::new(None),
        }
    }

    /// Store backed by `<config dir>/tokens.json`.
    pub fn global() -> &'static TokenStore {
        static STORE: OnceLock<TokenStore> = OnceLock::new();
        STORE.get_or_init(|| TokenStore::new(AppConfig::config_dir().join("tokens.json")))
    }

    #[must_use]
    pub fn load(&self) -> Option<TokenResponse> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache
            .get_or_insert_with(|| {
                std::fs::read_to_string(&self.path)
                    .ok()
                    .and_then(|s| serde_json::from_str(&s).ok())
            })
            .clone()
    }

    pub fn save(&self, tokens: &TokenResponse) -> std::io::Result<()> {
        let json = serde_json::to_vec_pretty(tokens)?;
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        storage::write_private(&self.path, &json)?;
        *cache = Some(Some(tokens.clone()));
        drop(cache);
        self.remember(tokens, &json)
    }

    /// Signs the active account out: forgets its tokens here and in the vault. Other
    /// accounts stay available.
    pub fn clear(&self) -> std::io::Result<()> {
        let active = self.load();
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        *cache = Some(None);
        storage::remove_if_exists(&self.path)?;
        drop(cache);
        if let Some(upn) = active.and_then(|t| Account::from_tokens(&t).upn) {
            storage::remove_if_exists(&self.vault_file(&upn))?;
        }
        Ok(())
    }

    /// Accounts that can be switched to (including the active one), sorted by UPN.
    #[must_use]
    pub fn accounts(&self) -> Vec<Account> {
        // Tokens saved before the vault existed are added on first use.
        if let Some(active) = self.load() {
            if let Some(upn) = Account::from_tokens(&active).upn {
                if !self.vault_file(&upn).exists() {
                    if let Ok(json) = serde_json::to_vec_pretty(&active) {
                        let _ = self.remember(&active, &json);
                    }
                }
            }
        }
        let mut accounts: Vec<Account> = std::fs::read_dir(self.vault_dir())
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| read_tokens(&e.path()))
            .map(|t| Account::from_tokens(&t))
            .filter(|a| a.upn.is_some())
            .collect();
        accounts.sort_by(|a, b| a.upn.cmp(&b.upn));
        accounts.dedup_by(|a, b| a.upn == b.upn);
        accounts
    }

    /// Makes the vaulted account `upn` the active one. Returns `false` if it is unknown.
    pub fn switch_to(&self, upn: &str) -> std::io::Result<bool> {
        let Some(tokens) = read_tokens(&self.vault_file(upn)) else {
            return Ok(false);
        };
        self.save(&tokens)?;
        Ok(true)
    }

    /// Removes a vaulted account; signs out if it is the active one.
    pub fn forget(&self, upn: &str) -> std::io::Result<()> {
        let active_upn = self.load().and_then(|t| Account::from_tokens(&t).upn);
        if active_upn.is_some_and(|a| a.eq_ignore_ascii_case(upn)) {
            return self.clear();
        }
        storage::remove_if_exists(&self.vault_file(upn))
    }

    fn vault_dir(&self) -> PathBuf {
        self.path
            .parent()
            .map_or_else(|| PathBuf::from("accounts"), |p| p.join("accounts"))
    }

    fn vault_file(&self, upn: &str) -> PathBuf {
        let digest =
            ring::digest::digest(&ring::digest::SHA256, upn.trim().to_lowercase().as_bytes());
        let key: String = digest.as_ref()[..8]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        self.vault_dir().join(format!("{key}.json"))
    }

    /// Mirrors `tokens` (already serialized as `json`) into the vault.
    fn remember(&self, tokens: &TokenResponse, json: &[u8]) -> std::io::Result<()> {
        match Account::from_tokens(tokens).upn {
            Some(upn) => storage::write_private(&self.vault_file(&upn), json),
            None => Ok(()),
        }
    }
}

fn read_tokens(path: &Path) -> Option<TokenResponse> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> TokenResponse {
        TokenResponse {
            token_type: "Bearer".into(),
            scope: None,
            expires_in: 3600,
            ext_expires_in: None,
            access_token: "at".into(),
            refresh_token: Some("rt".into()),
            id_token: None,
            acquired_at: Some(1),
            client_id: Some("cid".into()),
            authority: None,
        }
    }

    #[test]
    fn save_load_clear() {
        let dir = std::env::temp_dir().join(format!("wal-tokens-{}", uuid::Uuid::new_v4()));
        let store = TokenStore::new(dir.join("tokens.json"));
        assert!(store.load().is_none());
        store.save(&sample()).unwrap();
        assert_eq!(store.load().unwrap().refresh_token.as_deref(), Some("rt"));

        // A fresh instance reads what the first one wrote.
        let other = TokenStore::new(dir.join("tokens.json"));
        assert_eq!(other.load().unwrap().client_id.as_deref(), Some("cid"));

        store.clear().unwrap();
        assert!(store.load().is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn for_user(upn: &str) -> TokenResponse {
        TokenResponse {
            access_token: crate::auth::jwt::make_unsigned(&serde_json::json!({ "upn": upn })),
            ..sample()
        }
    }

    #[test]
    fn keeps_and_switches_between_accounts() {
        let dir = std::env::temp_dir().join(format!("wal-accounts-{}", uuid::Uuid::new_v4()));
        let store = TokenStore::new(dir.join("tokens.json"));
        store.save(&for_user("a@contoso.com")).unwrap();
        store.save(&for_user("b@contoso.com")).unwrap();
        let upns = |s: &TokenStore| -> Vec<String> {
            s.accounts().into_iter().filter_map(|a| a.upn).collect()
        };
        assert_eq!(upns(&store), ["a@contoso.com", "b@contoso.com"]);

        assert!(store.switch_to("A@contoso.com").unwrap());
        let active = Account::from_tokens(&store.load().unwrap()).upn;
        assert_eq!(active.as_deref(), Some("a@contoso.com"));
        assert!(!store.switch_to("nobody@contoso.com").unwrap());

        // Signing out forgets only the active account.
        store.clear().unwrap();
        assert!(store.load().is_none());
        assert_eq!(upns(&store), ["b@contoso.com"]);
        store.forget("b@contoso.com").unwrap();
        assert!(upns(&store).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
