//! Saved direct RDP connections ("Remote PCs") reached by host name or IP address,
//! stored in `connections.json` next to the preferences. Passwords never go into this
//! file; they are kept in the OS credential store ([`crate::platform::secrets`]).

use crate::config::{storage, AppConfig};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};

/// Port used when an address doesn't name one.
pub const DEFAULT_RDP_PORT: u16 = 3389;

/// Security layer negotiated with the remote PC.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecurityProtocol {
    /// Let client and server agree (NLA, then TLS, then RDP).
    #[default]
    Auto,
    /// Network Level Authentication (CredSSP).
    Nla,
    Tls,
    /// Legacy RDP encryption.
    Rdp,
}

impl SecurityProtocol {
    pub const ALL: [Self; 4] = [Self::Auto, Self::Nla, Self::Tls, Self::Rdp];

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Automatic",
            Self::Nla => "Network Level Authentication",
            Self::Tls => "TLS",
            Self::Rdp => "RDP (legacy)",
        }
    }
}

/// What to do with the remote PC's certificate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CertificatePolicy {
    /// Ask when the certificate is unknown or changed; remember accepted ones.
    #[default]
    Ask,
    /// Accept and remember the first certificate; ask if it changes.
    TrustFirstUse,
    /// Accept any certificate (only for trusted networks).
    Ignore,
}

impl CertificatePolicy {
    pub const ALL: [Self; 3] = [Self::Ask, Self::TrustFirstUse, Self::Ignore];

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Ask => "Ask when unknown or changed",
            Self::TrustFirstUse => "Trust on first use (ask if changed)",
            Self::Ignore => "Accept any certificate",
        }
    }
}

/// Authentication package used by Network Level Authentication.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMethod {
    /// Kerberos when available, otherwise NTLM.
    #[default]
    Auto,
    /// Kerberos only (needs a ticket, e.g. from `kinit`).
    Kerberos,
    Ntlm,
}

impl AuthMethod {
    pub const ALL: [Self; 3] = [Self::Auto, Self::Kerberos, Self::Ntlm];

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Automatic",
            Self::Kerberos => "Kerberos",
            Self::Ntlm => "NTLM",
        }
    }
}

/// A Remote Desktop Gateway the connection goes through.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gateway {
    /// `host` or `host:port` (default port 443).
    pub address: String,
    /// Gateway username when it differs from the PC's (`separate_credentials`).
    #[serde(default)]
    pub username: String,
    /// Sign in to the gateway with its own username and password.
    #[serde(default)]
    pub separate_credentials: bool,
}

/// Start a single program (RemoteApp) instead of the full desktop.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteApp {
    /// Program path or published alias (`||alias`).
    pub program: String,
    #[serde(default)]
    pub arguments: String,
}

/// A remote PC the user added by address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedConnection {
    pub id: String,
    /// Optional friendly name; the address is shown when empty.
    #[serde(default)]
    pub name: String,
    /// `host`, `host:port`, `[IPv6]` or `[IPv6]:port`.
    pub address: String,
    /// Optional `user`, `DOMAIN\user` or `user@domain`.
    #[serde(default)]
    pub username: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub domain: String,
    /// Group heading the connection is listed under (empty: ungrouped).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub group: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway: Option<Gateway>,
    /// Connect to the console (administrative) session.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub admin_session: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_app: Option<RemoteApp>,
    #[serde(default)]
    pub security: SecurityProtocol,
    #[serde(default)]
    pub certificate: CertificatePolicy,
    #[serde(default)]
    pub auth: AuthMethod,
    /// The password is kept in the system keyring (never in this file).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remember_password: bool,
    /// Starred connection for quick one-click access.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_favorite: bool,
    /// Custom tags for categorization and filtering (e.g. "Production", "Development", "Lab").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Folder heading or category (e.g. "Production", "Development", "Lab").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

impl SavedConnection {
    #[must_use]
    pub fn folder_or_group(&self) -> &str {
        if let Some(folder) = &self.folder {
            if !folder.is_empty() {
                return folder.as_str();
            }
        }
        &self.group
    }

    /// Creates a connection with a new id after validating `address` and `username`.
    pub fn new(name: &str, address: &str, username: &str) -> Result<Self, AddressError> {
        let mut connection = Self::blank();
        connection.update(name, address, username)?;
        Ok(connection)
    }

    /// A connection with a new id and no address yet (for forms).
    #[must_use]
    pub fn blank() -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: String::new(),
            address: String::new(),
            username: String::new(),
            domain: String::new(),
            group: String::new(),
            gateway: None,
            admin_session: false,
            remote_app: None,
            security: SecurityProtocol::default(),
            certificate: CertificatePolicy::default(),
            auth: AuthMethod::default(),
            remember_password: false,
            is_favorite: false,
            tags: Vec::new(),
            folder: None,
        }
    }

    /// Replaces the basic fields (keeping the id and advanced settings) after validation.
    pub fn update(
        &mut self,
        name: &str,
        address: &str,
        username: &str,
    ) -> Result<(), AddressError> {
        parse_address(address)?;
        let username = username.trim();
        if username.chars().any(char::is_control) {
            return Err(AddressError::InvalidUsername);
        }
        self.name = name.trim().to_string();
        self.address = address.trim().to_string();
        self.username = username.to_string();
        Ok(())
    }

    /// Trims the advanced settings and checks that the engine can take them: values that
    /// end up in comma-separated engine options must not contain commas.
    pub fn validate(&mut self) -> Result<(), AddressError> {
        let plain = |s: &str| !s.chars().any(char::is_control);
        let option_safe = |s: &str| plain(s) && !s.contains(',');
        parse_address(&self.address)?;
        if !plain(&self.username) {
            return Err(AddressError::InvalidUsername);
        }
        self.domain = self.domain.trim().to_string();
        if !plain(&self.domain) || self.domain.contains(['\\', '@', '/']) {
            return Err(AddressError::InvalidDomain);
        }
        self.group = self.group.trim().to_string();
        if !plain(&self.group) {
            return Err(AddressError::InvalidGroup);
        }
        if let Some(gateway) = &mut self.gateway {
            gateway.address = gateway.address.trim().to_string();
            gateway.username = gateway.username.trim().to_string();
            if gateway.address.is_empty() {
                self.gateway = None;
            } else {
                parse_gateway_address(&gateway.address)?;
                if !option_safe(&gateway.username) {
                    return Err(AddressError::InvalidGatewayUsername);
                }
            }
        }
        if let Some(app) = &mut self.remote_app {
            app.program = app.program.trim().to_string();
            app.arguments = app.arguments.trim().to_string();
            if app.program.is_empty() {
                self.remote_app = None;
            } else if !option_safe(&app.program) || !option_safe(&app.arguments) {
                return Err(AddressError::InvalidRemoteApp);
            }
        }
        Ok(())
    }

    /// Name shown in the UI.
    #[must_use]
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() {
            &self.address
        } else {
            &self.name
        }
    }

    pub fn endpoint(&self) -> Result<Endpoint, AddressError> {
        parse_address(&self.address)
    }

    /// Whether the gateway needs its own password.
    #[must_use]
    pub fn gateway_needs_password(&self) -> bool {
        self.gateway
            .as_ref()
            .is_some_and(|g| g.separate_credentials)
    }
}

/// Default port of a Remote Desktop Gateway.
pub const DEFAULT_GATEWAY_PORT: u16 = 443;

/// Parses a gateway `host` or `host:port` (default port 443).
pub fn parse_gateway_address(input: &str) -> Result<Endpoint, AddressError> {
    let input = input.trim();
    let has_port = if let Some(rest) = input.strip_prefix('[') {
        rest.split_once(']')
            .is_some_and(|(_, after)| !after.is_empty())
    } else {
        input.matches(':').count() == 1
    };
    let mut endpoint = parse_address(input).map_err(|_| AddressError::InvalidGateway)?;
    if !has_port {
        endpoint.port = DEFAULT_GATEWAY_PORT;
    }
    Ok(endpoint)
}

/// A validated host and port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// Host name, IPv4 address or IPv6 address (without brackets).
    pub host: String,
    pub port: u16,
}

impl Endpoint {
    fn is_ipv6(&self) -> bool {
        self.host.contains(':')
    }
}

impl fmt::Display for Endpoint {
    /// `host:port`, with IPv6 addresses in brackets (the form FreeRDP's `/v:` expects).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_ipv6() {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AddressError {
    #[error("Enter a PC name or IP address")]
    Empty,
    #[error("The PC name or IP address contains characters that are not allowed")]
    InvalidHost,
    #[error("The port must be a number between 1 and 65535")]
    InvalidPort,
    #[error("The username contains characters that are not allowed")]
    InvalidUsername,
    #[error("The domain contains characters that are not allowed")]
    InvalidDomain,
    #[error("The group name contains characters that are not allowed")]
    InvalidGroup,
    #[error("The gateway address is not valid")]
    InvalidGateway,
    #[error("The gateway username must not contain commas")]
    InvalidGatewayUsername,
    #[error("The program and its arguments must not contain commas")]
    InvalidRemoteApp,
}

/// Parses `host`, `host:port`, a bare IPv6 address, `[IPv6]` or `[IPv6]:port`.
pub fn parse_address(input: &str) -> Result<Endpoint, AddressError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(AddressError::Empty);
    }
    let (host, port) = if let Some(rest) = input.strip_prefix('[') {
        let (host, after) = rest.split_once(']').ok_or(AddressError::InvalidHost)?;
        let port = match after {
            "" => None,
            _ => Some(after.strip_prefix(':').ok_or(AddressError::InvalidPort)?),
        };
        if !is_ipv6_like(host) {
            return Err(AddressError::InvalidHost);
        }
        (host, port)
    } else if input.matches(':').count() > 1 {
        if !is_ipv6_like(input) {
            return Err(AddressError::InvalidHost);
        }
        (input, None)
    } else {
        let (host, port) = match input.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (input, None),
        };
        if !is_hostname(host) {
            return Err(AddressError::InvalidHost);
        }
        (host, port)
    };
    let port = match port {
        None => DEFAULT_RDP_PORT,
        Some(port) => port
            .parse::<u16>()
            .ok()
            .filter(|p| *p != 0)
            .ok_or(AddressError::InvalidPort)?,
    };
    Ok(Endpoint {
        host: host.to_string(),
        port,
    })
}

fn is_hostname(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && !host.starts_with(['-', '.'])
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
}

fn is_ipv6_like(host: &str) -> bool {
    let (address, zone) = host.split_once('%').unwrap_or((host, ""));
    address.contains(':')
        && address
            .chars()
            .all(|c| c.is_ascii_hexdigit() || matches!(c, ':' | '.'))
        && zone
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct StoreFile {
    #[serde(default)]
    connections: Vec<SavedConnection>,
}

/// The list of saved connections on disk.
#[derive(Debug, Clone)]
pub struct ConnectionStore {
    path: PathBuf,
}

impl Default for ConnectionStore {
    fn default() -> Self {
        Self::at(AppConfig::config_dir().join("connections.json"))
    }
}

impl ConnectionStore {
    #[must_use]
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Saved connections; empty when the file is missing or unreadable.
    #[must_use]
    pub fn load(&self) -> Vec<SavedConnection> {
        match std::fs::read(&self.path) {
            Ok(bytes) => match serde_json::from_slice::<StoreFile>(&bytes) {
                Ok(file) => file.connections,
                Err(e) => {
                    log::warn!("Ignoring unreadable {}: {e}", self.path.display());
                    Vec::new()
                }
            },
            Err(_) => Vec::new(),
        }
    }

    fn save(&self, connections: Vec<SavedConnection>) -> std::io::Result<Vec<SavedConnection>> {
        let file = StoreFile { connections };
        let json = serde_json::to_vec_pretty(&file)?;
        storage::write_private(&self.path, &json)?;
        Ok(file.connections)
    }

    /// Adds `connection`, or replaces the saved one with the same id. Returns the new list.
    pub fn upsert(&self, connection: SavedConnection) -> std::io::Result<Vec<SavedConnection>> {
        let mut connections = self.load();
        match connections.iter_mut().find(|c| c.id == connection.id) {
            Some(existing) => *existing = connection,
            None => connections.push(connection),
        }
        self.save(connections)
    }

    /// Removes the connection with `id`. Returns the new list.
    pub fn remove(&self, id: &str) -> std::io::Result<Vec<SavedConnection>> {
        let mut connections = self.load();
        connections.retain(|c| c.id != id);
        self.save(connections)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(host: &str, port: u16) -> Endpoint {
        Endpoint {
            host: host.into(),
            port,
        }
    }

    #[test]
    fn parses_addresses() {
        assert_eq!(parse_address(" pc1 "), Ok(endpoint("pc1", 3389)));
        assert_eq!(
            parse_address("pc.example.com:3390"),
            Ok(endpoint("pc.example.com", 3390))
        );
        assert_eq!(
            parse_address("192.168.1.20"),
            Ok(endpoint("192.168.1.20", 3389))
        );
        assert_eq!(parse_address("fe80::1"), Ok(endpoint("fe80::1", 3389)));
        assert_eq!(
            parse_address("[fe80::1%eth0]:4000"),
            Ok(endpoint("fe80::1%eth0", 4000))
        );
        assert_eq!(parse_address("[::1]"), Ok(endpoint("::1", 3389)));
    }

    #[test]
    fn rejects_bad_addresses() {
        assert_eq!(parse_address("  "), Err(AddressError::Empty));
        for bad in ["pc 1", "/v:x", "pc,1", "-pc", "[::1", "[zz::1]", "a:b:c:g"] {
            assert_eq!(parse_address(bad), Err(AddressError::InvalidHost), "{bad}");
        }
        for bad in ["pc:0", "pc:70000", "pc:", "pc:x", "[::1]x", "[::1]:"] {
            assert_eq!(parse_address(bad), Err(AddressError::InvalidPort), "{bad}");
        }
    }

    #[test]
    fn formats_endpoints_for_freerdp() {
        assert_eq!(endpoint("pc", 3389).to_string(), "pc:3389");
        assert_eq!(endpoint("::1", 3390).to_string(), "[::1]:3390");
    }

    #[test]
    fn validates_and_names_connections() {
        let mut c = SavedConnection::new(" ", " host:3390 ", " CORP\\me ").unwrap();
        assert_eq!(c.display_name(), "host:3390");
        assert_eq!(c.username, "CORP\\me");
        let id = c.id.clone();
        c.update("Office", "other", "").unwrap();
        assert_eq!((c.id.as_str(), c.display_name()), (id.as_str(), "Office"));
        assert_eq!(
            c.update("x", "bad host", ""),
            Err(AddressError::InvalidHost)
        );
        assert_eq!(
            c.update("x", "h", "a\nb"),
            Err(AddressError::InvalidUsername)
        );
        assert_eq!(c.address, "other");
    }

    #[test]
    fn validates_advanced_settings() {
        let mut c = SavedConnection::new("", "pc", "me").unwrap();
        c.domain = " CORP ".into();
        c.group = " Lab ".into();
        c.gateway = Some(Gateway {
            address: " gw.example.com ".into(),
            username: "gw".into(),
            separate_credentials: true,
        });
        c.remote_app = Some(RemoteApp {
            program: "||notepad".into(),
            arguments: String::new(),
        });
        c.validate().unwrap();
        assert_eq!((c.domain.as_str(), c.group.as_str()), ("CORP", "Lab"));
        assert_eq!(c.gateway.as_ref().unwrap().address, "gw.example.com");
        assert!(c.gateway_needs_password());

        let mut bad = c.clone();
        bad.gateway.as_mut().unwrap().username = "a,b".into();
        assert_eq!(bad.validate(), Err(AddressError::InvalidGatewayUsername));
        let mut bad = c.clone();
        bad.remote_app.as_mut().unwrap().arguments = "x,y".into();
        assert_eq!(bad.validate(), Err(AddressError::InvalidRemoteApp));
        let mut bad = c.clone();
        bad.domain = "A\\B".into();
        assert_eq!(bad.validate(), Err(AddressError::InvalidDomain));
        let mut bad = c.clone();
        bad.gateway.as_mut().unwrap().address = "bad host".into();
        assert_eq!(bad.validate(), Err(AddressError::InvalidGateway));

        c.gateway.as_mut().unwrap().address = " ".into();
        c.remote_app.as_mut().unwrap().program = String::new();
        c.validate().unwrap();
        assert_eq!((c.gateway.clone(), c.remote_app.clone()), (None, None));
    }

    #[test]
    fn gateway_addresses_default_to_443() {
        assert_eq!(parse_gateway_address("gw"), Ok(endpoint("gw", 443)));
        assert_eq!(parse_gateway_address("gw:8443"), Ok(endpoint("gw", 8443)));
        assert_eq!(parse_gateway_address("[::1]"), Ok(endpoint("::1", 443)));
        assert_eq!(parse_gateway_address("[::1]:1"), Ok(endpoint("::1", 1)));
    }

    #[test]
    fn reads_connections_saved_by_older_versions() {
        let json = r#"{"connections":[{"id":"1","address":"pc"}]}"#;
        let file: StoreFile = serde_json::from_str(json).unwrap();
        let c = &file.connections[0];
        assert_eq!(c.certificate, CertificatePolicy::Ask);
        assert_eq!((c.gateway.clone(), c.admin_session), (None, false));
        let saved = serde_json::to_string(c).unwrap();
        assert!(!saved.contains("gateway") && !saved.contains("domain"));
    }

    #[test]
    fn store_round_trip() {
        let dir = std::env::temp_dir().join(format!("wal-conn-{}", uuid::Uuid::new_v4()));
        let store = ConnectionStore::at(dir.join("connections.json"));
        assert!(store.load().is_empty());

        let mut a = SavedConnection::new("A", "a", "").unwrap();
        let b = SavedConnection::new("B", "b:1", "u").unwrap();
        store.upsert(a.clone()).unwrap();
        store.upsert(b.clone()).unwrap();
        a.update("A2", "a2", "").unwrap();
        let list = store.upsert(a.clone()).unwrap();
        assert_eq!(list, vec![a.clone(), b.clone()]);
        assert_eq!(store.load(), list);

        assert_eq!(store.remove(&a.id).unwrap(), vec![b]);
        std::fs::write(store.path(), "not json").unwrap();
        assert!(store.load().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn connection_favorites_folders_tags() {
        let mut c = SavedConnection::new("Test", "10.0.0.1", "admin").unwrap();
        c.is_favorite = true;
        c.tags = vec!["Production".into(), "Lab".into()];
        c.folder = Some("Cloud".into());
        assert_eq!(c.folder_or_group(), "Cloud");

        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains(r#""is_favorite":true"#));
        assert!(json.contains(r#""tags":["Production","Lab"]"#));
        assert!(json.contains(r#""folder":"Cloud""#));

        let loaded: SavedConnection = serde_json::from_str(&json).unwrap();
        assert!(loaded.is_favorite);
        assert_eq!(loaded.tags, vec!["Production", "Lab"]);
        assert_eq!(loaded.folder.as_deref(), Some("Cloud"));
    }
}
