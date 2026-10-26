//! WinUI 3 frontend (Windows), built with `windows-reactor`.
//!
//! The main window, its dialogs, the settings page and the sign-in windows are Reactor
//! components ([`app`], [`dialogs`], [`forms`], [`settings`], [`signin`]); session windows
//! are plain Win32 windows rendered with GDI ([`session_window`]) because they need raw
//! keyboard scancodes and wheel input. Async core calls run on a shared Tokio runtime
//! from thread-pool work items ([`block_on`]).

mod app;
mod dialogs;
mod forms;
mod session_window;
mod settings;
mod signin;
mod views;
mod win32;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::OnceLock;
use std::time::Duration;

use windows_reactor::App;

use crate::config::AppConfig;
use crate::launcher::LaunchTarget;

/// How long to wait at exit for sessions that are still disconnecting.
const TEARDOWN_TIMEOUT: Duration = Duration::from_secs(5);

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

/// Runs `future` to completion on the shared runtime. Call it off the UI thread (from
/// `spawn_background` work); the UI thread must not block on the network.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    if let Some(runtime) = RUNTIME.get() {
        return runtime.block_on(future);
    }
    log::error!("The async runtime is not running; using a temporary one");
    match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime.block_on(future),
        // Without any runtime there is no way to run the future at all.
        Err(e) => panic!("could not start an async runtime: {e}"),
    }
}

/// Command line: `--connect TARGET` (or `-c`) and `.rdp` files.
fn parse_args(args: impl IntoIterator<Item = String>) -> Result<app::Startup, String> {
    let mut startup = app::Startup::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let value = match arg.as_str() {
            "--connect" | "-c" | "/connect" => match args.next() {
                Some(value) => value,
                None => return Err("--connect needs a target".to_string()),
            },
            _ => match arg.strip_prefix("--connect=") {
                Some(value) => value.to_string(),
                None => {
                    startup.files.push(PathBuf::from(arg));
                    continue;
                }
            },
        };
        match LaunchTarget::parse(&value) {
            Some(target) => startup.target = Some(target),
            None => {
                return Err(format!(
                    "Invalid --connect target {value:?}; expected cloudpc:ID, avd:ID or remote:ID"
                ))
            }
        }
    }
    Ok(startup)
}

/// Points the OpenSSL copy bundled next to the executable to its `ossl-modules` directory
/// (for the legacy provider needed by FreeRDP for NTLM).
fn configure_bundled_openssl() {
    if std::env::var_os("OPENSSL_MODULES").is_none() {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let modules = dir.join("ossl-modules");
                if modules.is_dir() {
                    std::env::set_var("OPENSSL_MODULES", modules);
                }
            }
        }
    }
}

/// Runs the app until its main window closes.
#[must_use]
pub fn run() -> ExitCode {
    // Before any threads are spawned: environment is not thread-safe to mutate.
    configure_bundled_openssl();

    let startup = match parse_args(std::env::args().skip(1)) {
        Ok(startup) => startup,
        Err(message) => {
            eprintln!("{message}");
            win32::message(None, "Windows App", &message, win32::Icon::Error);
            return ExitCode::FAILURE;
        }
    };

    // Sign-in web views keep their cookies next to the configuration.
    std::env::set_var(
        "WEBVIEW2_USER_DATA_FOLDER",
        AppConfig::config_dir().join("WebView2"),
    );

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            log::error!("Could not start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let runtime = RUNTIME.get_or_init(|| runtime);
    // Core code on the UI thread (sessions, HTTP clients) needs the Tokio context.
    let _guard = runtime.enter();

    AppConfig::remove_legacy_files();

    let result = App::run_component::<app::MainApp>(startup);
    session_window::end_all();
    crate::rdp::join_pending_teardowns(TEARDOWN_TIMEOUT);
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            log::error!("The user interface failed: {e}");
            win32::message(
                None,
                "Windows App",
                &format!("Windows App could not start its user interface: {e}"),
                win32::Icon::Error,
            );
            ExitCode::FAILURE
        }
    }
}
