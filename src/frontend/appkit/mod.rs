//! AppKit frontend (macOS), written with `objc2`.
//!
//! The main window lists Cloud PCs, Azure Virtual Desktop resources and Remote PCs in a
//! sidebar/content split with a unified toolbar; each session opens in its own window
//! that draws the remote desktop and forwards input. The toolkit-independent behaviour
//! comes from [`crate::frontend::common`].
//!
//! Threading: AppKit objects live on the main thread. Network calls run on a Tokio
//! runtime and hop back with `dispatch_async` to the main queue (see [`util::spawn`]);
//! RDP engine events wake the main queue the same way.

mod actions;
mod app;
mod cards;
mod connect;
mod delegate;
mod dialogs;
mod forms;
mod main_window;
mod menus;
mod session_view;
mod session_window;
mod signin;
mod toolbar;
mod util;

use crate::config::AppConfig;
use crate::launcher::LaunchTarget;
use delegate::{AppDelegate, LaunchRequest};
use objc2::runtime::ProtocolObject;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use std::path::PathBuf;
use std::process::ExitCode;

/// Runs the app until it quits.
#[must_use]
pub fn run() -> ExitCode {
    let request = match parse_args(std::env::args().skip(1)) {
        Ok(request) => request,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    let Some(mtm) = MainThreadMarker::new() else {
        log::error!("The app must start on the main thread");
        return ExitCode::FAILURE;
    };
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
    // HTTP clients created on the main thread need the Tokio reactor.
    let _guard = runtime.enter();
    util::set_runtime(runtime.handle().clone());

    AppConfig::remove_legacy_files();

    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let delegate = AppDelegate::new(mtm, request);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    menus::install_main_menu(mtm);
    // Returns only if the run loop is stopped; quitting exits from `terminate:` after
    // `applicationWillTerminate:` has disconnected the sessions.
    app.run();
    ExitCode::SUCCESS
}

/// `--connect TARGET` (`-c`, `--connect=TARGET`) and `.rdp` files.
fn parse_args(args: impl Iterator<Item = String>) -> Result<LaunchRequest, String> {
    let mut request = LaunchRequest::default();
    let mut args = args.peekable();
    let parse_target = |text: &str| {
        LaunchTarget::parse(text).ok_or_else(|| {
            format!("Invalid --connect target {text:?}; expected cloudpc:ID, avd:ID or remote:ID")
        })
    };
    while let Some(arg) = args.next() {
        if arg == "--connect" || arg == "-c" {
            let value = args
                .next()
                .ok_or_else(|| format!("{arg} needs a target (cloudpc:ID, avd:ID or remote:ID)"))?;
            request.target = Some(parse_target(&value)?);
        } else if let Some(value) = arg.strip_prefix("--connect=") {
            request.target = Some(parse_target(value)?);
        } else if arg.starts_with('-') {
            // Arguments added by the system or Xcode (e.g. `-psn_…`, `-NSDocument… YES`).
            if !arg.starts_with("-psn") && args.peek().is_some_and(|v| !v.starts_with('-')) {
                args.next();
            }
        } else {
            request.files.push(PathBuf::from(arg));
        }
    }
    Ok(request)
}
