//! Windows App: the native frontend selected at build time (`gtk`, `winui` or `appkit`).

#![cfg_attr(
    all(windows, feature = "winui", not(debug_assertions)),
    windows_subsystem = "windows"
)]

use std::process::ExitCode;

#[cfg(windows)]
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "An unknown panic occurred".to_string()
        };
        let location = info
            .location()
            .map(|l| format!(" at {}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let msg = format!(
            "Windows App encountered a fatal error:

{payload}{location}"
        );
        log::error!("{msg}");
        eprintln!("{msg}");
        if let Ok(mut dir) = std::env::current_exe() {
            dir.pop();
            let _ = std::fs::write(dir.join("windows-app-crash.log"), &msg);
        }
        let temp_log = std::env::temp_dir().join("windows-app-crash.log");
        let _ = std::fs::write(temp_log, &msg);
        #[cfg(all(feature = "winui", not(feature = "gtk")))]
        windows_app::frontend::winui::show_fatal_error(&msg);
    }));
}

fn main() -> ExitCode {
    #[cfg(windows)]
    install_panic_hook();
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
