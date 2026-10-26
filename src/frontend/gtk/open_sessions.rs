//! Session windows currently open. Session windows are not part of the application's
//! window list, so the app quits with its main window unless asked to keep them running.

use adw::prelude::*;
use gtk4::{gio, glib};
use std::cell::RefCell;

thread_local! {
    static OPEN: RefCell<Vec<(glib::WeakRef<adw::Window>, String)>> =
        const { RefCell::new(Vec::new()) };
    /// Keeps the app running without a main window until the last session closes.
    static HOLD: RefCell<Option<gio::ApplicationHoldGuard>> = const { RefCell::new(None) };
}

pub fn register(window: &adw::Window, name: &str) {
    OPEN.with_borrow_mut(|open| open.push((window.downgrade(), name.to_string())));
}

pub fn unregister(window: &adw::Window) {
    let empty = OPEN.with_borrow_mut(|open| {
        open.retain(|(weak, _)| weak.upgrade().is_some_and(|w| &w != window));
        open.is_empty()
    });
    if empty {
        let hold = HOLD.take();
        drop(hold);
    }
}

/// Names of the connections with an open session window.
pub fn names() -> Vec<String> {
    OPEN.with_borrow(|open| {
        open.iter()
            .filter(|(weak, _)| weak.upgrade().is_some())
            .map(|(_, name)| name.clone())
            .collect()
    })
}

/// Disconnects every session by closing its window.
pub fn close_all() {
    let windows: Vec<adw::Window> =
        OPEN.with_borrow(|open| open.iter().filter_map(|(weak, _)| weak.upgrade()).collect());
    for window in windows {
        window.close();
    }
}

/// Keeps the app running after its main window closes, until the last session closes.
pub fn keep_app_running() {
    if names().is_empty() || HOLD.with_borrow(Option::is_some) {
        return;
    }
    if let Some(app) = gio::Application::default() {
        HOLD.set(Some(app.hold()));
    }
}
