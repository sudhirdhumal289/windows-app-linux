//! Optional GNOME Shell helper extension (`assets/gnome-shell-extension`, installed by
//! the app packages).
//!
//! GNOME on Wayland gives apps no way to read or choose a window's workspace. The helper
//! offers both over D-Bus, limited to the calling process's own windows. GNOME Shell
//! only discovers extensions at login, so one installed during the current session
//! starts after the next login; once known, turning it on or off takes effect at once.

use gtk4::{gio, glib, prelude::*};
use std::path::{Path, PathBuf};

pub const EXTENSION_UUID: &str = "windows-app-linux@sudhirdhumal289.github.io";

const SHELL_BUS_NAME: &str = "org.gnome.Shell";
const PLACEMENT_PATH: &str = "/com/microsoft/windowsapp/linux/WindowPlacement";
const PLACEMENT_INTERFACE: &str = "com.microsoft.windowsapp.linux.WindowPlacement";
const SHELL_SCHEMA: &str = "org.gnome.shell";
const CALL_TIMEOUT_MS: i32 = 1000;

/// `ExtensionState.ACTIVE` (`ENABLED` before GNOME 46).
const STATE_ACTIVE: f64 = 1.0;
const STATE_ERROR: f64 = 3.0;
const STATE_OUT_OF_DATE: f64 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelperState {
    /// Not running GNOME Shell.
    Unavailable,
    NotInstalled,
    Off,
    /// Turned on, but installed after GNOME Shell started: it loads at the next login.
    Pending,
    /// Turned on but GNOME Shell does not load it (user extensions are switched off,
    /// or this GNOME version is not supported).
    Blocked,
    On,
}

impl HelperState {
    #[must_use]
    pub fn is_enabled(self) -> bool {
        matches!(self, Self::Pending | Self::Blocked | Self::On)
    }
}

#[must_use]
pub fn state() -> HelperState {
    let Some(info) = shell_extension_info() else {
        return HelperState::Unavailable;
    };
    let Some(settings) = shell_settings() else {
        return HelperState::Unavailable;
    };
    let shell_state = info.lookup::<f64>("state").ok().flatten();
    if shell_state == Some(STATE_ACTIVE) {
        return HelperState::On;
    }
    if shell_state.is_none() && !user_copy_exists() && !system_copy_exists() {
        return HelperState::NotInstalled;
    }
    let listed = settings
        .strv("enabled-extensions")
        .iter()
        .any(|uuid| uuid == EXTENSION_UUID);
    if !listed {
        return HelperState::Off;
    }
    let user_extensions_off = settings.boolean("disable-user-extensions") && !system_copy_exists();
    if user_extensions_off || matches!(shell_state, Some(STATE_ERROR | STATE_OUT_OF_DATE)) {
        HelperState::Blocked
    } else {
        HelperState::Pending
    }
}

/// Turns the installed helper on.
pub fn enable() -> Result<(), String> {
    if !user_copy_exists() && !system_copy_exists() {
        return Err("The GNOME Shell helper is not installed".into());
    }
    let settings = shell_settings().ok_or("GNOME Shell settings are unavailable")?;
    update_list(&settings, "disabled-extensions", false)?;
    update_list(&settings, "enabled-extensions", true)
}

pub fn disable() -> Result<(), String> {
    let settings = shell_settings().ok_or("GNOME Shell settings are unavailable")?;
    update_list(&settings, "enabled-extensions", false)
}

/// The workspace of this process's window titled `title`.
#[must_use]
pub fn workspace(title: &str) -> Option<u32> {
    let reply = call_placement("GetWorkspace", &(title,).to_variant(), "(i)")?;
    let (workspace,) = reply.get::<(i32,)>()?;
    u32::try_from(workspace).ok()
}

/// Moves this process's window titled `title` to `workspace` (the last one when fewer
/// exist) and shows that workspace.
pub fn move_to_workspace(title: &str, workspace: u32) -> bool {
    call_placement("MoveToWorkspace", &(title, workspace).to_variant(), "(b)")
        .and_then(|reply| reply.get::<(bool,)>())
        .is_some_and(|(moved,)| moved)
}

fn call_placement(method: &str, args: &glib::Variant, reply: &str) -> Option<glib::Variant> {
    call(PLACEMENT_PATH, PLACEMENT_INTERFACE, method, args, reply)
        .map_err(|e| log::debug!("GNOME Shell helper {method} failed: {e}"))
        .ok()
}

fn shell_extension_info() -> Option<glib::VariantDict> {
    let reply = call(
        "/org/gnome/Shell",
        "org.gnome.Shell.Extensions",
        "GetExtensionInfo",
        &(EXTENSION_UUID,).to_variant(),
        "(a{sv})",
    )
    .ok()?;
    Some(glib::VariantDict::new(Some(&reply.child_value(0))))
}

fn call(
    path: &str,
    interface: &str,
    method: &str,
    args: &glib::Variant,
    reply: &str,
) -> Result<glib::Variant, glib::Error> {
    let connection = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)?;
    connection.call_sync(
        Some(SHELL_BUS_NAME),
        path,
        interface,
        method,
        Some(args),
        glib::VariantTy::new(reply).ok(),
        gio::DBusCallFlags::NO_AUTO_START,
        CALL_TIMEOUT_MS,
        gio::Cancellable::NONE,
    )
}

fn shell_settings() -> Option<gio::Settings> {
    let schema = gio::SettingsSchemaSource::default()?.lookup(SHELL_SCHEMA, true)?;
    Some(gio::Settings::new_full(
        &schema,
        gio::SettingsBackend::NONE,
        None,
    ))
}

fn update_list(settings: &gio::Settings, key: &str, present: bool) -> Result<(), String> {
    let mut list: Vec<String> = settings.strv(key).iter().map(ToString::to_string).collect();
    let listed = list.iter().any(|uuid| uuid == EXTENSION_UUID);
    if listed == present {
        return Ok(());
    }
    if present {
        list.push(EXTENSION_UUID.to_string());
    } else {
        list.retain(|uuid| uuid != EXTENSION_UUID);
    }
    settings
        .set_strv(key, list)
        .map_err(|e| format!("Could not update GNOME Shell settings: {e}"))
}

fn extension_dir(data_dir: &Path) -> PathBuf {
    data_dir
        .join("gnome-shell")
        .join("extensions")
        .join(EXTENSION_UUID)
}

fn user_copy_exists() -> bool {
    extension_dir(&glib::user_data_dir())
        .join("metadata.json")
        .exists()
}

fn system_copy_exists() -> bool {
    glib::system_data_dirs()
        .iter()
        .any(|dir| extension_dir(dir).join("metadata.json").exists())
}
