//! Passwords kept in the OS credential store (Secret Service on Linux, Keychain on
//! macOS, Credential Manager on Windows). Calls may block on D-Bus or an unlock prompt,
//! so run them off the UI thread.

use std::fmt;

const SERVICE: &str = "com.microsoft.windowsapp.linux";

/// Which password of a saved connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretKind {
    /// Sign-in password of the remote PC.
    Password,
    /// Password for the Remote Desktop Gateway (when it differs from the PC's).
    GatewayPassword,
}

/// Identifies one stored password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretKey {
    pub connection_id: String,
    pub kind: SecretKind,
}

impl SecretKey {
    #[must_use]
    pub fn password(connection_id: &str) -> Self {
        Self {
            connection_id: connection_id.to_string(),
            kind: SecretKind::Password,
        }
    }

    #[must_use]
    pub fn gateway(connection_id: &str) -> Self {
        Self {
            connection_id: connection_id.to_string(),
            kind: SecretKind::GatewayPassword,
        }
    }

    fn account(&self) -> String {
        match self.kind {
            SecretKind::Password => format!("remote:{}", self.connection_id),
            SecretKind::GatewayPassword => format!("remote-gateway:{}", self.connection_id),
        }
    }
}

impl fmt::Display for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.account())
    }
}

/// Credential store failure (store missing, locked or the request was dismissed).
#[derive(Debug, thiserror::Error)]
#[error("The password store is not available: {0}")]
pub struct SecretError(String);

fn entry(key: &SecretKey) -> Result<keyring::Entry, SecretError> {
    keyring::Entry::new(SERVICE, &key.account()).map_err(|e| SecretError(e.to_string()))
}

/// Stores `password` under `key`, replacing any previous value.
pub fn save_password(key: &SecretKey, password: &str) -> Result<(), SecretError> {
    entry(key)?
        .set_password(password)
        .map_err(|e| SecretError(e.to_string()))
}

/// The stored password, or `None` when nothing is stored for `key`.
pub fn load_password(key: &SecretKey) -> Result<Option<String>, SecretError> {
    match entry(key)?.get_password() {
        Ok(password) => Ok(Some(password)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(SecretError(e.to_string())),
    }
}

/// Removes the password stored under `key` (succeeds when there is none).
pub fn delete_password(key: &SecretKey) -> Result<(), SecretError> {
    match entry(key)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(SecretError(e.to_string())),
    }
}

/// Removes every password of a connection that is being deleted.
pub fn delete_connection_secrets(connection_id: &str) {
    for key in [
        SecretKey::password(connection_id),
        SecretKey::gateway(connection_id),
    ] {
        if let Err(e) = delete_password(&key) {
            log::warn!("Could not remove saved password {key}: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounts_are_distinct_per_kind() {
        assert_eq!(SecretKey::password("a").to_string(), "remote:a");
        assert_eq!(SecretKey::gateway("a").to_string(), "remote-gateway:a");
    }
}
