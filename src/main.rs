//! Application entrypoint for Windows App on Linux.

use gtk4::prelude::*;
use gtk4::Application;
use log::debug;
use std::sync::Arc;
use windows_app_linux::api::{CloudPcService, GraphApiClient};
use windows_app_linux::auth::AuthManager;
use windows_app_linux::config::{DEFAULT_AUTHORITY, OFFICIAL_CLIENT_ID};
use windows_app_linux::ui::AppMainWindow;

const APP_ID: &str = "com.microsoft.windowsapp.linux";

fn main() {
    let default_level = if cfg!(debug_assertions) {
        "debug"
    } else {
        "warn"
    };
    env_logger::init_from_env(env_logger::Env::default().default_filter_or(default_level));
    debug!("Starting Windows App for Linux (GTK4)...");

    // Set prgname and application name so Wayland compositors (GNOME Shell, Sway, etc.)
    // map the window xdg_toplevel app_id directly to com.microsoft.windowsapp.linux.desktop
    gtk4::glib::set_prgname(Some(APP_ID));
    gtk4::glib::set_application_name("Windows App");

    // Initialize Tokio multi-threaded runtime with timer and I/O reactors
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to initialize Tokio runtime");

    // Enter Tokio runtime context on the main thread so async libraries (reqwest, hyper)
    // executed via GLib MainContext can access the Tokio reactor
    let _guard = runtime.enter();

    let app = Application::builder().application_id(APP_ID).build();

    app.connect_activate(|app| {
        // Register default window icon
        gtk4::Window::set_default_icon_name(APP_ID);

        if let Some(display) = gtk4::gdk::Display::default() {
            let icon_theme = gtk4::IconTheme::for_display(&display);
            icon_theme.add_search_path("assets");
            icon_theme.add_search_path("resources/icons");
            if let Ok(home) = std::env::var("HOME") {
                icon_theme.add_search_path(format!("{home}/.local/share/icons"));
            }
        }

        let auth = Arc::new(AuthManager::new(DEFAULT_AUTHORITY, OFFICIAL_CLIENT_ID));
        let graph_client = GraphApiClient::new(Arc::clone(&auth), "commercial");
        let cloud_service = Arc::new(CloudPcService::new(graph_client));

        let main_window = AppMainWindow::new(app, auth, cloud_service);
        main_window.show();
    });

    app.run();
}
