//! Import and export of Remote PCs as `.rdp` files (the format of Microsoft's Remote
//! Desktop clients and Remmina): `key:type:value` lines, UTF-8 or UTF-16 encoded.

use crate::connections::{
    AddressError, CertificatePolicy, Gateway, RemoteApp, SavedConnection, SecurityProtocol,
};
use crate::overrides::{ConnectionOverrides, Resolution};
use std::collections::HashMap;
use std::fmt::Write as _;

/// A connection read from an `.rdp` file, with the settings it carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedRdp {
    pub connection: SavedConnection,
    pub overrides: ConnectionOverrides,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImportError {
    #[error("The file does not name a computer (no \"full address\")")]
    MissingAddress,
    #[error("The file is not a Remote Desktop connection file")]
    NotText,
    #[error(transparent)]
    Invalid(#[from] AddressError),
}

/// Decodes UTF-16 (with byte-order mark, as written by Windows) or UTF-8 text.
pub fn decode(bytes: &[u8]) -> Result<String, ImportError> {
    let utf16 = |bytes: &[u8], le: bool| {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&pair| {
                if le {
                    u16::from_le_bytes(pair)
                } else {
                    u16::from_be_bytes(pair)
                }
            })
            .collect();
        String::from_utf16(&units).map_err(|_| ImportError::NotText)
    };
    match bytes {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, true),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, false),
        [0xEF, 0xBB, 0xBF, rest @ ..] => {
            String::from_utf8(rest.to_vec()).map_err(|_| ImportError::NotText)
        }
        _ => String::from_utf8(bytes.to_vec()).map_err(|_| ImportError::NotText),
    }
}

fn properties(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.trim().splitn(3, ':');
            let (key, _ty, value) = (parts.next()?, parts.next()?, parts.next()?);
            Some((key.trim().to_ascii_lowercase(), value.trim().to_string()))
        })
        .collect()
}

/// Reads a connection from `.rdp` text. `name` is used as its display name (usually the
/// file name). Unknown keys are ignored; the connection gets a new id.
pub fn import(text: &str, name: &str) -> Result<ImportedRdp, ImportError> {
    let props = properties(text);
    let get = |key: &str| props.get(key).map(String::as_str).filter(|v| !v.is_empty());
    let flag = |key: &str| get(key).and_then(|v| v.parse::<i64>().ok());
    let bool_flag = |key: &str| flag(key).map(|v| v != 0);

    let mut address = get("full address")
        .ok_or(ImportError::MissingAddress)?
        .to_string();
    if let Some(port) = flag("server port").filter(|p| *p != 3389) {
        let has_port = address.starts_with('[') && address.contains("]:")
            || !address.starts_with('[') && address.matches(':').count() == 1;
        if !has_port {
            address = if address.matches(':').count() > 1 && !address.starts_with('[') {
                format!("[{address}]:{port}")
            } else {
                format!("{address}:{port}")
            };
        }
    }
    let mut connection = SavedConnection::new(name, &address, get("username").unwrap_or_default())?;
    connection.domain = get("domain").unwrap_or_default().to_string();

    let gateway_used = flag("gatewayusagemethod").is_none_or(|m| m == 1 || m == 2);
    if let Some(host) = get("gatewayhostname").filter(|_| gateway_used) {
        connection.gateway = Some(Gateway {
            address: host.to_string(),
            username: String::new(),
            separate_credentials: flag("promptcredentialonce") == Some(0),
        });
    }
    connection.admin_session = bool_flag("administrative session").unwrap_or(false)
        || bool_flag("connect to console").unwrap_or(false);
    if bool_flag("remoteapplicationmode") == Some(true) {
        if let Some(program) = get("remoteapplicationprogram") {
            connection.remote_app = Some(RemoteApp {
                program: program.to_string(),
                arguments: get("remoteapplicationcmdline")
                    .unwrap_or_default()
                    .to_string(),
            });
        }
    }
    if flag("authentication level") == Some(0) {
        connection.certificate = CertificatePolicy::Ignore;
    }
    if bool_flag("enablecredsspsupport") == Some(false) {
        connection.security = SecurityProtocol::Tls;
    }
    connection.validate()?;

    let mut overrides = ConnectionOverrides::default();
    if bool_flag("dynamic resolution") == Some(false) {
        if let (Some(width), Some(height)) = (flag("desktopwidth"), flag("desktopheight")) {
            let size = |v: i64| u32::try_from(v).unwrap_or(0);
            overrides.resolution = Some(Resolution::new(size(width), size(height)).clamped());
        }
    }
    overrides.clipboard = bool_flag("redirectclipboard");
    overrides.audio = flag("audiomode").map(|m| m == 0);
    overrides.microphone = bool_flag("audiocapturemode");
    overrides.printers = bool_flag("redirectprinters");
    overrides.smartcard = bool_flag("redirectsmartcards");
    overrides.multi_monitor = bool_flag("use multimon");
    overrides.drives = props.get("drivestoredirect").map(|v| !v.is_empty());
    overrides.camera = props.get("camerastoredirect").map(|v| !v.is_empty());
    Ok(ImportedRdp {
        connection,
        overrides,
    })
}

/// Writes `connection` and its settings as `.rdp` text (CRLF line endings). Passwords
/// are never written.
#[must_use]
pub fn export(connection: &SavedConnection, overrides: &ConnectionOverrides) -> String {
    let mut out = String::new();
    let mut line = |key: &str, ty: char, value: &dyn std::fmt::Display| {
        let _ = write!(out, "{key}:{ty}:{value}\r\n");
    };
    line("full address", 's', &connection.address);
    if !connection.username.is_empty() {
        line("username", 's', &connection.username);
    }
    if !connection.domain.is_empty() {
        line("domain", 's', &connection.domain);
    }
    if let Some(gateway) = &connection.gateway {
        line("gatewayhostname", 's', &gateway.address);
        line("gatewayusagemethod", 'i', &1);
        line("gatewayprofileusagemethod", 'i', &1);
        line("gatewaycredentialssource", 'i', &0);
        line(
            "promptcredentialonce",
            'i',
            &u8::from(!gateway.separate_credentials),
        );
    }
    if connection.admin_session {
        line("administrative session", 'i', &1);
    }
    if let Some(app) = &connection.remote_app {
        line("remoteapplicationmode", 'i', &1);
        line("remoteapplicationprogram", 's', &app.program);
        if !app.arguments.is_empty() {
            line("remoteapplicationcmdline", 's', &app.arguments);
        }
    }
    line(
        "authentication level",
        'i',
        &match connection.certificate {
            CertificatePolicy::Ignore => 0,
            CertificatePolicy::Ask | CertificatePolicy::TrustFirstUse => 2,
        },
    );
    if matches!(
        connection.security,
        SecurityProtocol::Tls | SecurityProtocol::Rdp
    ) {
        line("enablecredsspsupport", 'i', &0);
    }
    match overrides.resolution {
        Some(size) => {
            line("dynamic resolution", 'i', &0);
            line("screen mode id", 'i', &1);
            line("desktopwidth", 'i', &size.width);
            line("desktopheight", 'i', &size.height);
        }
        None => line("dynamic resolution", 'i', &1),
    }
    let mut flag = |key: &str, value: Option<bool>| {
        if let Some(value) = value {
            line(key, 'i', &u8::from(value));
        }
    };
    flag("redirectclipboard", overrides.clipboard);
    flag("audiocapturemode", overrides.microphone);
    flag("redirectprinters", overrides.printers);
    flag("redirectsmartcards", overrides.smartcard);
    flag("use multimon", overrides.multi_monitor);
    if let Some(audio) = overrides.audio {
        line("audiomode", 'i', &if audio { 0 } else { 2 });
    }
    if let Some(drives) = overrides.drives {
        line("drivestoredirect", 's', &if drives { "*" } else { "" });
    }
    if let Some(camera) = overrides.camera {
        line("camerastoredirect", 's', &if camera { "*" } else { "" });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MSTSC: &str = "screen mode id:i:2\r\nuse multimon:i:1\r\ndesktopwidth:i:1920\r\n\
        desktopheight:i:1080\r\ndynamic resolution:i:0\r\nfull address:s:pc.corp.local\r\n\
        server port:i:3390\r\naudiomode:i:2\r\nredirectclipboard:i:1\r\n\
        drivestoredirect:s:*\r\nusername:s:CORP\\me\r\ngatewayhostname:s:gw.corp.com\r\n\
        gatewayusagemethod:i:1\r\npromptcredentialonce:i:0\r\nauthentication level:i:0\r\n\
        administrative session:i:1\r\nremoteapplicationmode:i:1\r\n\
        remoteapplicationprogram:s:||calc\r\n";

    #[test]
    fn imports_mstsc_files() {
        let imported = import(MSTSC, "Office").unwrap();
        let c = &imported.connection;
        assert_eq!(c.name, "Office");
        assert_eq!(c.address, "pc.corp.local:3390");
        assert_eq!(c.username, "CORP\\me");
        let gateway = c.gateway.as_ref().unwrap();
        assert_eq!(gateway.address, "gw.corp.com");
        assert!(gateway.separate_credentials);
        assert!(c.admin_session);
        assert_eq!(c.remote_app.as_ref().unwrap().program, "||calc");
        assert_eq!(c.certificate, CertificatePolicy::Ignore);
        let o = &imported.overrides;
        assert_eq!(o.resolution, Some(Resolution::new(1920, 1080)));
        assert_eq!(o.audio, Some(false));
        assert_eq!(o.clipboard, Some(true));
        assert_eq!(o.drives, Some(true));
        assert_eq!(o.multi_monitor, Some(true));
        assert_eq!(o.printers, None);
    }

    #[test]
    fn export_round_trips() {
        let imported = import(MSTSC, "Office").unwrap();
        let text = export(&imported.connection, &imported.overrides);
        assert!(text.ends_with("\r\n") && !text.contains("password"));
        let again = import(&text, "Office").unwrap();
        assert_eq!(
            ImportedRdp {
                connection: SavedConnection {
                    id: imported.connection.id.clone(),
                    ..again.connection
                },
                overrides: again.overrides,
            },
            imported
        );
    }

    #[test]
    fn decodes_utf16_and_rejects_bad_files() {
        let mut bytes = vec![0xFF, 0xFE];
        for unit in "full address:s:pc\r\n".encode_utf16() {
            bytes.extend(unit.to_le_bytes());
        }
        let text = decode(&bytes).unwrap();
        assert_eq!(import(&text, "").unwrap().connection.address, "pc");
        assert_eq!(decode(&[0xC3, 0x28]), Err(ImportError::NotText));
        assert_eq!(
            import("username:s:x", "n").unwrap_err(),
            ImportError::MissingAddress
        );
        assert_eq!(
            import("full address:s:bad host", "n").unwrap_err(),
            ImportError::Invalid(AddressError::InvalidHost)
        );
    }

    #[test]
    fn ipv6_with_server_port() {
        let text = "full address:s:fe80::1\nserver port:i:4000\n";
        assert_eq!(
            import(text, "").unwrap().connection.address,
            "[fe80::1]:4000"
        );
    }
}
