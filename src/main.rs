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
    windows_app::frontend::gtk::run().into()
}

#[cfg(all(windows, feature = "winui", not(feature = "gtk")))]
fn run() -> ExitCode {
    windows_app::frontend::winui::run()
}

#[cfg(all(target_os = "macos", feature = "appkit", not(feature = "gtk")))]
fn run() -> ExitCode {
    windows_app::frontend::appkit::run()
}

#[cfg(not(any(
    feature = "gtk",
    all(windows, feature = "winui"),
    all(target_os = "macos", feature = "appkit")
)))]
fn run() -> ExitCode {
    eprintln!("No frontend enabled; build with --features gtk, winui or appkit");
    ExitCode::FAILURE
}
