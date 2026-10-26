//! Windows desktop integration (Start Menu shortcuts, Startup autostart, system DPI scaling).

use crate::launcher::{LaunchEntry, LaunchTarget};
use std::io;
use std::path::PathBuf;

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
