//! Host-OS integration that is independent of the UI toolkit.
//!
//! Each supported OS provides the same small API; frontends call these functions
//! instead of probing the system themselves.

pub mod hooks;
pub mod secrets;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;
#[cfg(target_os = "linux")]
mod xdg;
#[cfg(target_os = "linux")]
pub use xdg::*;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use self::windows::*;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use self::macos::*;

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
mod other {
    /// Desktop scale in percent as configured in the OS, if it can be determined.
    #[must_use]
    pub fn desktop_scale_percent() -> Option<u32> {
        None
    }

    /// A USB device attached to this computer.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct UsbDevice {
        pub id: String,
        pub name: String,
        pub accessible: bool,
    }

    /// Attached USB devices that can be redirected.
    #[must_use]
    pub fn list_usb_devices() -> Vec<UsbDevice> {
        Vec::new()
    }

    use crate::launcher::{LaunchEntry, LaunchTarget};
    use std::io;

    fn unsupported() -> io::Error {
        io::Error::new(io::ErrorKind::Unsupported, "not supported on this OS yet")
    }

    /// Adds an app launcher entry that connects to `entry`.
    pub fn install_launcher(_entry: &LaunchEntry) -> io::Result<std::path::PathBuf> {
        Err(unsupported())
    }

    pub fn remove_launcher(_target: &LaunchTarget) -> io::Result<()> {
        Err(unsupported())
    }

    #[must_use]
    pub fn has_launcher(_target: &LaunchTarget) -> bool {
        false
    }

    /// Connects to `entry` when the user logs in (or stops doing so).
    pub fn set_autostart(_entry: &LaunchEntry, _enabled: bool) -> io::Result<()> {
        Err(unsupported())
    }

    #[must_use]
    pub fn has_autostart(_target: &LaunchTarget) -> bool {
        false
    }

    /// Removes launcher and login entries of a connection that no longer exists.
    pub fn remove_desktop_integration(_target: &LaunchTarget) {}
}
#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
pub use other::*;

/// Private folder (created owner-only) for files copied out of sessions. They can be
/// large, so they go to the cache rather than the memory-backed runtime directory.
pub fn clipboard_files_dir() -> std::io::Result<std::path::PathBuf> {
    let dir = directories::BaseDirs::new()
        .map(|d| d.cache_dir().to_path_buf())
        .unwrap_or_else(std::env::temp_dir)
        .join("windows-app-linux")
        .join("clipboard");
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

/// Directory for short-lived private runtime files (e.g. session `.rdp` files).
#[must_use]
pub fn runtime_dir() -> std::path::PathBuf {
    directories::BaseDirs::new()
        .and_then(|d| d.runtime_dir().map(std::path::Path::to_path_buf))
        .unwrap_or_else(std::env::temp_dir)
        .join("windows-app-linux")
}
