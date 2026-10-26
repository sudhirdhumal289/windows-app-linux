//! Maps GDK monitors and workspaces to the platform-neutral [`WindowPlacement`].
//!
//! Monitors work on every backend. Workspaces work on X11, and on GNOME Wayland when
//! the GNOME Shell helper is on: Wayland itself gives clients no way to read or choose
//! a window's workspace.

use super::gnome_shell;
use crate::placement::{MonitorId, WindowPlacement};
use gtk4::{gdk, glib, prelude::*};

#[must_use]
pub fn monitor_id(monitor: &gdk::Monitor) -> MonitorId {
    let text = |value: Option<glib::GString>| value.map(String::from).filter(|v| !v.is_empty());
    MonitorId {
        connector: text(monitor.connector()),
        manufacturer: text(monitor.manufacturer()),
        model: text(monitor.model()),
    }
}

/// The connected monitor that best matches `id`.
#[must_use]
pub fn find_monitor(display: &gdk::Display, id: &MonitorId) -> Option<gdk::Monitor> {
    id.best_match(
        display
            .monitors()
            .iter::<gdk::Monitor>()
            .flatten()
            .map(|monitor| {
                let id = monitor_id(&monitor);
                (monitor, id)
            }),
    )
}

/// Where `window` is shown now; `None` before it is realized.
#[must_use]
pub fn current(window: &gtk4::Window) -> Option<WindowPlacement> {
    let surface = window.surface()?;
    let workspace = if x11::is_x11(&surface) {
        x11::workspace(&surface)
    } else {
        window
            .title()
            .and_then(|title| gnome_shell::workspace(&title))
    };
    Some(WindowPlacement {
        monitor: surface
            .display()
            .monitor_at_surface(&surface)
            .map(|monitor| monitor_id(&monitor)),
        workspace,
    })
}

/// Moves a shown window to `workspace` (the last one when fewer exist now). Returns
/// whether the platform supports it.
pub fn move_to_workspace(window: &gtk4::Window, workspace: u32) -> bool {
    match window.surface() {
        Some(surface) if x11::is_x11(&surface) => x11::move_to_workspace(&surface, workspace),
        Some(_) => window
            .title()
            .is_some_and(|title| gnome_shell::move_to_workspace(&title, workspace)),
        None => false,
    }
}

#[cfg(target_os = "linux")]
#[allow(deprecated)] // GDK's X11 API is deprecated since GTK 4.18 but has no replacement.
mod x11 {
    use gdk4_x11::{X11Display, X11Surface};
    use gtk4::{gdk, prelude::*};

    /// `_NET_WM_DESKTOP` of windows shown on every workspace.
    const ALL_WORKSPACES: u32 = u32::MAX;

    pub fn is_x11(surface: &gdk::Surface) -> bool {
        surface.is::<X11Surface>()
    }

    pub fn workspace(surface: &gdk::Surface) -> Option<u32> {
        let desktop = surface.downcast_ref::<X11Surface>()?.desktop();
        (desktop != ALL_WORKSPACES).then_some(desktop)
    }

    pub fn move_to_workspace(surface: &gdk::Surface, workspace: u32) -> bool {
        let (Some(x11_surface), Some(display)) = (
            surface.downcast_ref::<X11Surface>(),
            surface.display().downcast_ref::<X11Display>().cloned(),
        ) else {
            return false;
        };
        let count = display.screen().number_of_desktops();
        if count > 0 {
            x11_surface.move_to_desktop(workspace.min(count - 1));
        }
        true
    }
}

#[cfg(not(target_os = "linux"))]
mod x11 {
    use gtk4::gdk;

    pub fn is_x11(_surface: &gdk::Surface) -> bool {
        false
    }

    pub fn workspace(_surface: &gdk::Surface) -> Option<u32> {
        None
    }

    pub fn move_to_workspace(_surface: &gdk::Surface, _workspace: u32) -> bool {
        false
    }
}
