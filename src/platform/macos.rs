//! macOS desktop integration (LaunchAgents autostart, Retina backing scale).

use crate::launcher::{LaunchEntry, LaunchTarget};
use std::io;
use std::path::PathBuf;

/// Desktop scale in percent as configured in the OS, querying Retina display backing scale.
#[must_use]
pub fn desktop_scale_percent() -> Option<u32> {
    #[cfg(all(target_os = "macos", feature = "appkit"))]
    {
        if let Some(screen) = objc2_app_kit::NSScreen::mainScreen() {
            let factor = screen.backingScaleFactor();
            if factor > 0.0 {
                return Some((factor * 100.0).round() as u32);
            }
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

fn launch_agents_dir() -> io::Result<PathBuf> {
    let base = directories::BaseDirs::new().ok_or(io::ErrorKind::NotFound)?;
    // ~/Library/LaunchAgents
    Ok(base.home_dir().join("Library").join("LaunchAgents"))
}

fn autostart_plist_path(target: &LaunchTarget) -> io::Result<PathBuf> {
    Ok(launch_agents_dir()?.join(format!(
        "com.microsoft.windowsapp.connection-{}.plist",
        target.file_key()
    )))
}

fn exec_path() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("windows-app"))
}

fn write_plist(path: &std::path::Path, target: &LaunchTarget) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let label = format!("com.microsoft.windowsapp.connection-{}", target.file_key());
    let exe = exec_path();
    let content = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
        <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
        <plist version=\"1.0\">\n\
        <dict>\n\
            <key>Label</key>\n\
            <string>{label}</string>\n\
            <key>ProgramArguments</key>\n\
            <array>\n\
                <string>{}</string>\n\
                <string>--connect</string>\n\
                <string>{target}</string>\n\
            </array>\n\
            <key>RunAtLoad</key>\n\
            <true/>\n\
        </dict>\n\
        </plist>\n",
        exe.display()
    );
    let tmp = path.with_extension(format!("tmp.{}", uuid::Uuid::new_v4()));
    std::fs::write(&tmp, content)?;
    let _ = std::fs::remove_file(path);
    std::fs::rename(&tmp, path)
}

pub fn install_launcher(_entry: &LaunchEntry) -> io::Result<PathBuf> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "macOS apps live in /Applications",
    ))
}

pub fn remove_launcher(_target: &LaunchTarget) -> io::Result<()> {
    Ok(())
}

#[must_use]
pub fn has_launcher(_target: &LaunchTarget) -> bool {
    false
}

pub fn set_autostart(entry: &LaunchEntry, enabled: bool) -> io::Result<()> {
    let path = autostart_plist_path(&entry.target)?;
    if enabled {
        write_plist(&path, &entry.target)
    } else {
        crate::config::storage::remove_if_exists(&path)
    }
}

#[must_use]
pub fn has_autostart(target: &LaunchTarget) -> bool {
    autostart_plist_path(target).is_ok_and(|p| p.exists())
}

pub fn remove_desktop_integration(target: &LaunchTarget) {
    if let Ok(path) = autostart_plist_path(target) {
        if let Err(e) = crate::config::storage::remove_if_exists(&path) {
            log::warn!("Could not remove {}: {e}", path.display());
        }
    }
}
