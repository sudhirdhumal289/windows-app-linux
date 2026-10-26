//! Windows App: the native frontend selected at build time (`gtk`, `winui` or `appkit`).

#![cfg_attr(
    all(windows, feature = "winui", not(debug_assertions)),
    windows_subsystem = "windows"
)]

use std::process::ExitCode;

fn main() -> ExitCode {
    let default_level = if cfg!(debug_assertions) {
        "debug"
    } else {
        "warn"
    };
    env_logger::init_from_env(env_logger::Env::default().default_filter_or(default_level));
    run()
}

#[cfg(feature = "gtk")]
fn run() -> ExitCode {
    windows_app_linux::frontend::gtk::run().into()
}

#[cfg(all(windows, feature = "winui", not(feature = "gtk")))]
fn run() -> ExitCode {
    windows_app_linux::frontend::winui::run()
}

#[cfg(all(target_os = "macos", feature = "appkit", not(feature = "gtk")))]
fn run() -> ExitCode {
    windows_app_linux::frontend::appkit::run()
}

#[cfg(not(any(
    feature = "gtk",
    all(windows, feature = "winui"),
    all(target_os = "macos", feature = "appkit")
)))]
fn run() -> ExitCode {
    eprintln!(
        "This build has no user interface. Rebuild with one of the features \
         `gtk` (Linux), `winui` (Windows) or `appkit` (macOS)."
    );
    ExitCode::FAILURE
}
