//! Windows desktop integration (Start Menu shortcuts, Startup autostart, system DPI scaling).

use crate::launcher::{LaunchEntry, LaunchTarget};
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Desktop scale in percent as configured in the OS, querying the primary monitor or system DPI.
#[must_use]
pub fn desktop_scale_percent() -> Option<u32> {
    #[cfg(all(target_os = "windows", feature = "winui"))]
    unsafe {
        let dpi = windows::Win32::UI::HiDpi::GetDpiForSystem();
        if dpi > 0 {
            let scale = (f64::from(dpi) / 96.0 * 100.0).round() as u32;
            return Some(scale);
        }
    }
    None
}

/// A USB device attached to this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbDevice {
    pub id: String,
    pub name: String,
    pub accessible: bool,
}

#[must_use]
pub fn list_usb_devices() -> Vec<UsbDevice> {
    Vec::new()
}

fn launcher_dir() -> io::Result<PathBuf> {
    let base = directories::BaseDirs::new().ok_or(io::ErrorKind::NotFound)?;
    // %APPDATA%\Microsoft\Windows\Start Menu\Programs\Windows App
    Ok(base
        .config_dir()
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join("Windows App"))
}

fn startup_dir() -> io::Result<PathBuf> {
    let base = directories::BaseDirs::new().ok_or(io::ErrorKind::NotFound)?;
    // %APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup
    Ok(base
        .config_dir()
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join("Startup"))
}

fn launcher_file(target: &LaunchTarget) -> io::Result<PathBuf> {
    Ok(launcher_dir()?.join(format!("connection-{}.cmd", target.file_key())))
}

fn autostart_file(target: &LaunchTarget) -> io::Result<PathBuf> {
    Ok(startup_dir()?.join(format!("windows-app-connection-{}.cmd", target.file_key())))
}

fn exec_path() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("WindowsApp.exe"))
}

fn write_launcher_script(
    path: &std::path::Path,
    target: &LaunchTarget,
    title: &str,
) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let exe = exec_path();
    let content = format!(
        "@echo off\r\n:: {}\r\nstart \"\" \"{}\" --connect \"{}\"\r\n",
        title,
        exe.display(),
        target
    );
    let tmp = path.with_extension(format!("tmp.{}", uuid::Uuid::new_v4()));
    std::fs::write(&tmp, content)?;
    let _ = std::fs::remove_file(path);
    std::fs::rename(&tmp, path)
}

pub fn install_launcher(entry: &LaunchEntry) -> io::Result<PathBuf> {
    let path = launcher_file(&entry.target)?;
    write_launcher_script(&path, &entry.target, &entry.name)?;
    Ok(path)
}

pub fn remove_launcher(target: &LaunchTarget) -> io::Result<()> {
    crate::config::storage::remove_if_exists(&launcher_file(target)?)
}

#[must_use]
pub fn has_launcher(target: &LaunchTarget) -> bool {
    launcher_file(target).is_ok_and(|p| p.exists())
}

pub fn set_autostart(entry: &LaunchEntry, enabled: bool) -> io::Result<()> {
    let path = autostart_file(&entry.target)?;
    if enabled {
        write_launcher_script(&path, &entry.target, &entry.name)
    } else {
        crate::config::storage::remove_if_exists(&path)
    }
}

#[must_use]
pub fn has_autostart(target: &LaunchTarget) -> bool {
    autostart_file(target).is_ok_and(|p| p.exists())
}

pub fn remove_desktop_integration(target: &LaunchTarget) {
    for path in [launcher_file(target), autostart_file(target)]
        .into_iter()
        .flatten()
    {
        if let Err(e) = crate::config::storage::remove_if_exists(&path) {
            log::warn!("Could not remove {}: {e}", path.display());
        }
    }
}

// ---------------------------------------------------------------------------
// OpenSSL Root CA certificate bundle export for Windows
// ---------------------------------------------------------------------------

#[repr(C)]
#[allow(non_snake_case)]
struct CERT_CONTEXT {
    dwCertEncodingType: u32,
    pbCertEncoded: *const u8,
    cbCertEncoded: u32,
    pCertInfo: *mut std::ffi::c_void,
    hCertStore: *mut std::ffi::c_void,
}

#[link(name = "crypt32")]
extern "system" {
    fn CertOpenSystemStoreW(hprov: usize, szsubsystemprotocol: *const u16)
        -> *mut std::ffi::c_void;
    fn CertEnumCertificatesInStore(
        hcertstore: *mut std::ffi::c_void,
        pprevcertcontext: *const CERT_CONTEXT,
    ) -> *const CERT_CONTEXT;
    fn CertCloseStore(hcertstore: *mut std::ffi::c_void, dwflags: u32) -> i32;
}

fn der_to_pem(der: &[u8]) -> String {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;

    let b64 = STANDARD.encode(der);
    let mut pem = String::with_capacity(b64.len() + 64);
    pem.push_str("-----BEGIN CERTIFICATE-----\r\n");
    for chunk in b64.as_bytes().chunks(64) {
        if let Ok(s) = std::str::from_utf8(chunk) {
            pem.push_str(s);
            pem.push_str("\r\n");
        }
    }
    pem.push_str("-----END CERTIFICATE-----\r\n");
    pem
}

fn export_store_certificates(store_name: &[u16]) -> Vec<Vec<u8>> {
    let mut certs = Vec::new();
    unsafe {
        let store = CertOpenSystemStoreW(0, store_name.as_ptr());
        if store.is_null() {
            return certs;
        }
        let mut ctx = CertEnumCertificatesInStore(store, std::ptr::null());
        while !ctx.is_null() {
            let cert_ref = &*ctx;
            if cert_ref.cbCertEncoded > 0 && !cert_ref.pbCertEncoded.is_null() {
                let der = std::slice::from_raw_parts(
                    cert_ref.pbCertEncoded,
                    cert_ref.cbCertEncoded as usize,
                );
                certs.push(der.to_vec());
            }
            ctx = CertEnumCertificatesInStore(store, ctx);
        }
        CertCloseStore(store, 0);
    }
    certs
}

/// Generates a PEM bundle containing all Windows system root and intermediate CA certificates.
pub fn generate_windows_ca_bundle() -> String {
    const ROOT: &[u16] = &[b'R' as u16, b'O' as u16, b'O' as u16, b'T' as u16, 0];
    const CA: &[u16] = &[b'C' as u16, b'A' as u16, 0];

    let mut seen = HashSet::new();
    let mut pem_bundle = String::new();

    for store in [ROOT, CA] {
        for der in export_store_certificates(store) {
            if seen.insert(der.clone()) {
                pem_bundle.push_str(&der_to_pem(&der));
            }
        }
    }

    pem_bundle
}

/// Known standard CA bundle paths on Windows (e.g. Git for Windows, OpenSSL defaults).
const KNOWN_CA_BUNDLE_PATHS: &[&str] = &[
    r"C:\Program Files\Git\mingw64\etc\ssl\certs\ca-bundle.crt",
    r"C:\Program Files\Git\etc\pki\ca-trust\extracted\openssl\ca-bundle.trust.crt",
    r"C:\Program Files\Git\usr\ssl\certs\ca-bundle.crt",
    r"C:\Program Files\Common Files\SSL\cert.pem",
];

/// Sets an environment variable in both the Windows OS environment block (via `std::env::set_var`)
/// and the C runtime (CRT) environment table (via `_wputenv_s`) so that C libraries like OpenSSL
/// and FreeRDP can see it via `getenv()`.
pub fn set_crt_var(key: &str, value: &Path) {
    set_crt_var_str(key, &value.display().to_string());
}

pub fn set_crt_var_str(key: &str, value: &str) {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    extern "C" {
        fn _wputenv_s(name: *const u16, value: *const u16) -> i32;
    }

    let wide_key: Vec<u16> = OsStr::new(key).encode_wide().chain(Some(0)).collect();
    let wide_val: Vec<u16> = OsStr::new(value).encode_wide().chain(Some(0)).collect();

    unsafe {
        _wputenv_s(wide_key.as_ptr(), wide_val.as_ptr());
    }
    std::env::set_var(key, value);
}

/// Ensures a valid CA certificate bundle exists and returns its path.
///
/// On Windows, OpenSSL does not query the Windows CryptoAPI certificate store by default.
/// This exports root CAs from Windows system stores into `%LOCALAPPDATA%\windows-app\ca-bundle.crt`
/// and configures both the OS environment and the C runtime (CRT) environment table.
#[must_use]
pub fn ensure_ca_bundle() -> Option<PathBuf> {
    // If the environment already specifies a certificate file or directory, respect it.
    if let Some(existing) = std::env::var_os("SSL_CERT_FILE") {
        let p = PathBuf::from(existing);
        if p.is_file() {
            set_crt_var("SSL_CERT_FILE", &p);
            return Some(p);
        }
    }
    if std::env::var_os("SSL_CERT_DIR").is_some() {
        return None;
    }

    let cache_dir = directories::BaseDirs::new()
        .map(|d| d.cache_dir().to_path_buf())
        .unwrap_or_else(std::env::temp_dir)
        .join("windows-app");
    let target = cache_dir.join("ca-bundle.crt");

    // Helper to configure CRT and return target.
    let apply_target = |p: PathBuf| {
        set_crt_var("SSL_CERT_FILE", &p);
        Some(p)
    };

    // Reuse existing generated bundle if modified within the last 7 days and non-empty.
    if target.is_file() {
        if let Ok(meta) = target.metadata() {
            if meta.len() > 0 {
                if let Ok(mod_time) = meta.modified() {
                    if SystemTime::now()
                        .duration_since(mod_time)
                        .unwrap_or(Duration::ZERO)
                        < Duration::from_secs(7 * 24 * 3600)
                    {
                        return apply_target(target);
                    }
                }
            }
        }
    }

    // Try exporting from Windows CryptoAPI certificate store.
    let bundle = generate_windows_ca_bundle();
    if !bundle.is_empty() {
        if let Err(e) = std::fs::create_dir_all(&cache_dir) {
            log::warn!(
                "Could not create cache dir for CA bundle {}: {e}",
                cache_dir.display()
            );
        } else if let Err(e) = std::fs::write(&target, bundle.as_bytes()) {
            log::warn!("Could not write CA bundle {}: {e}", target.display());
        } else {
            log::info!("Exported Windows CA bundle to {}", target.display());
            return apply_target(target);
        }
    }

    // Fall back to any discovered system CA bundle.
    for path_str in KNOWN_CA_BUNDLE_PATHS {
        let p = Path::new(path_str);
        if p.is_file() {
            log::info!("Using fallback CA bundle at {}", p.display());
            return apply_target(p.to_path_buf());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_windows_ca_bundle_export() {
        let bundle = generate_windows_ca_bundle();
        assert!(!bundle.is_empty(), "Expected non-empty Windows CA bundle");
        assert!(bundle.contains("-----BEGIN CERTIFICATE-----"));
        assert!(bundle.contains("-----END CERTIFICATE-----"));
    }

    #[test]
    fn test_ensure_ca_bundle() {
        let ca_file = ensure_ca_bundle();
        assert!(
            ca_file.is_some(),
            "Expected ensure_ca_bundle to return a valid path"
        );
        let path = ca_file.unwrap();
        assert!(
            path.is_file(),
            "CA bundle file should exist: {}",
            path.display()
        );

        // Verify that the C runtime getenv() can see SSL_CERT_FILE.
        extern "C" {
            fn getenv(name: *const std::ffi::c_char) -> *const std::ffi::c_char;
        }
        let env_val = unsafe { getenv(b"SSL_CERT_FILE\0".as_ptr().cast()) };
        assert!(
            !env_val.is_null(),
            "C runtime getenv('SSL_CERT_FILE') must not be null"
        );
    }
}
