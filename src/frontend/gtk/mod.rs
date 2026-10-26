//! GTK 4 / libadwaita frontend (Linux).

mod app_window;
mod canvas;
mod clipboard;
mod connection_card;
mod connection_settings_dialog;
mod credentials_dialog;
mod fullscreen_dock;
mod gnome_shell;
mod login_dialog;
mod open_sessions;
mod placement;
mod preferences_dialog;
mod rdp_auth;
mod remote_pc_dialog;
mod search_provider;
mod session_window;
mod webkit;

use crate::api::{CloudPcService, GraphApiClient};
use crate::auth::AuthManager;
use crate::config::AppConfig;
use crate::launcher::LaunchTarget;
use adw::prelude::*;
use app_window::AppMainWindow;
use gtk4::{gdk, gio, glib};
use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

pub use crate::config::APP_ID;

/// How long to wait at exit for sessions that are still disconnecting.
const TEARDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a D-Bus activated instance (desktop search) stays up without windows.
const SERVICE_IDLE_TIMEOUT_MS: u32 = 30_000;

thread_local! {
    static MAIN_WINDOW: RefCell<Weak<AppMainWindow>> = const { RefCell::new(Weak::new()) };
}

/// Runs the application until the last window closes.
pub fn run() -> glib::ExitCode {
    // Wayland compositors map the xdg_toplevel app id to the .desktop file.
    glib::set_prgname(Some(APP_ID));
    glib::set_application_name("Windows App");

    // Stabilize PulseAudio / PipeWire capture buffer latency to prevent buffer underruns (XRUNs)
    // which cause breaking, choppy, and noisy microphone audio in FreeRDP.
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("PULSE_LATENCY_MSEC").is_none() {
            std::env::set_var("PULSE_LATENCY_MSEC", "60");
        }
        if std::env::var_os("PIPEWIRE_LATENCY").is_none() {
            std::env::set_var("PIPEWIRE_LATENCY", "1024/48000");
        }
    }

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            log::error!("Could not start the async runtime: {e}");
            return glib::ExitCode::FAILURE;
        }
    };
    // HTTP clients run on the GLib main loop but need the Tokio reactor.
    let _guard = runtime.enter();

    AppConfig::remove_legacy_files();

    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    app.add_main_option(
        "connect",
        glib::Char::from(b'c'),
        glib::OptionFlags::NONE,
        glib::OptionArg::String,
        "Connect to a Cloud PC (cloudpc:ID), Azure Virtual Desktop resource (avd:ID) or Remote PC (remote:ID)",
        Some("TARGET"),
    );
    app.set_option_context_parameter_string(Some("[FILE.rdp]"));
    if std::env::args().any(|a| a == "--gapplication-service") {
        app.set_inactivity_timeout(SERVICE_IDLE_TIMEOUT_MS);
    }
    app.connect_startup(|app| {
        let weak = app.downgrade();
        let registered = search_provider::register(app, move |target| {
            if let Some(app) = weak.upgrade() {
                open(&app, target);
            }
        });
        if let Err(e) = registered {
            log::warn!("Desktop search is unavailable: {e}");
        }
    });
    app.connect_activate(|app| open(app, None));
    // Runs in the launching process, so errors reach the caller's terminal.
    app.connect_handle_local_options(|_, options| {
        match options.lookup::<String>("connect").ok().flatten() {
            Some(text) if LaunchTarget::parse(&text).is_none() => {
                eprintln!(
                    "Invalid --connect target {text:?}; expected cloudpc:ID, avd:ID or remote:ID"
                );
                std::ops::ControlFlow::Break(glib::ExitCode::FAILURE)
            }
            _ => std::ops::ControlFlow::Continue(()),
        }
    });
    app.connect_command_line(|app, command_line| {
        let target = command_line
            .options_dict()
            .lookup::<String>("connect")
            .ok()
            .flatten()
            .and_then(|t| LaunchTarget::parse(&t));
        open(app, target);
        // Remaining arguments are .rdp files (e.g. opened from the file manager).
        let files: Vec<std::path::PathBuf> = command_line
            .arguments()
            .iter()
            .skip(1)
            .filter_map(|arg| command_line.create_file_for_arg(arg).path())
            .collect();
        if let Some(window) = MAIN_WINDOW.with_borrow(Weak::upgrade) {
            for file in files {
                window.open_rdp_file(&file);
            }
        }
        glib::ExitCode::SUCCESS
    });
    let code = app.run();

    crate::rdp::join_pending_teardowns(TEARDOWN_TIMEOUT);
    code
}

/// Shows the main window (creating it if needed) and connects to `target`, if any.
fn open(app: &adw::Application, target: Option<LaunchTarget>) {
    let window = MAIN_WINDOW.with_borrow(Weak::upgrade).unwrap_or_else(|| {
        let window = create_main_window(app);
        MAIN_WINDOW.set(Rc::downgrade(&window));
        window
    });
    window.present();
    if let Some(target) = target {
        window.connect_target(target);
    }
}

fn create_main_window(app: &adw::Application) -> Rc<AppMainWindow> {
    gtk4::Window::set_default_icon_name(APP_ID);
    if let Some(display) = gdk::Display::default() {
        let icon_theme = gtk4::IconTheme::for_display(&display);
        // Development checkouts; installed builds use the system icon theme.
        icon_theme.add_search_path("assets");
        icon_theme.add_search_path("resources/icons");
    }

    let config = AppConfig::load();
    let auth = Arc::new(AuthManager::for_profile(config.auth_profile()));
    let graph = GraphApiClient::new(Arc::clone(&auth), &config.data_boundary);
    let service = Arc::new(CloudPcService::new(graph));
    AppMainWindow::new(app, auth, service)
}

/// Opens `uri` with the user's default handler.
pub(crate) fn open_uri(uri: &str, parent: Option<&impl IsA<gtk4::Window>>) {
    let uri_owned = uri.to_string();
    gtk4::UriLauncher::new(uri).launch(parent, gio::Cancellable::NONE, move |result| {
        if let Err(e) = result {
            log::warn!("Could not open {uri_owned}: {e}");
        }
    });
}
