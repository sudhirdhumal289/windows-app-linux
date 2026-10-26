//! The application delegate: startup, files opened from Finder, reopening the main window
//! and the quit confirmation while sessions are open.

use super::app::{self, App};
use super::dialogs::{self, QuitChoice};
use super::session_window;
use crate::launcher::LaunchTarget;
use objc2::rc::Retained;
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSApplicationDelegate, NSApplicationTerminateReply};
use objc2_foundation::{NSArray, NSNotification, NSObject, NSObjectProtocol, NSURL};
use std::cell::RefCell;
use std::path::PathBuf;
use std::time::Duration;

/// How long to wait at exit for sessions that are still disconnecting.
const TEARDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// What the command line asked for.
#[derive(Debug, Default)]
pub(super) struct LaunchRequest {
    pub target: Option<LaunchTarget>,
    pub files: Vec<PathBuf>,
}

pub(super) struct DelegateIvars {
    /// Work requested before the app finished launching.
    pending: RefCell<LaunchRequest>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and the class has no Drop impl.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WindowsAppDelegate"]
    #[ivars = DelegateIvars]
    pub(super) struct AppDelegate;

    // SAFETY: NSObjectProtocol has no safety requirements.
    unsafe impl NSObjectProtocol for AppDelegate {}

    // SAFETY: The method signatures match the protocol.
    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _notification: &NSNotification) {
            let app = App::start(self.mtm());
            app.present();
            let request = std::mem::take(&mut *self.ivars().pending.borrow_mut());
            if let Some(target) = request.target {
                *app.pending_target.borrow_mut() = Some(target);
            }
            // Connects to the pending target once the library is known.
            app.load();
            for file in request.files {
                app.open_rdp_file(&file);
            }
        }

        #[unsafe(method(application:openURLs:))]
        fn open_urls(&self, _application: &NSApplication, urls: &NSArray<NSURL>) {
            let files: Vec<PathBuf> = urls
                .iter()
                .filter(|url| url.isFileURL())
                .filter_map(|url| url.to_file_path())
                .collect();
            match app::get() {
                Some(app) => {
                    app.present();
                    for file in files {
                        app.open_rdp_file(&file);
                    }
                }
                None => self.ivars().pending.borrow_mut().files.extend(files),
            }
        }

        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self, _sender: &NSApplication) -> NSApplicationTerminateReply {
            let names = session_window::open_names();
            if names.is_empty() {
                return NSApplicationTerminateReply::TerminateNow;
            }
            match dialogs::confirm_quit(self.mtm(), &names) {
                QuitChoice::Quit => NSApplicationTerminateReply::TerminateNow,
                QuitChoice::KeepOpen => {
                    // Only the main window goes away; the sessions keep the app running.
                    app::with(|app| {
                        if let Some(window) = app.window() {
                            window.window.orderOut(None);
                        }
                    });
                    NSApplicationTerminateReply::TerminateCancel
                }
                QuitChoice::Cancel => NSApplicationTerminateReply::TerminateCancel,
            }
        }

        #[unsafe(method(applicationWillTerminate:))]
        fn will_terminate(&self, _notification: &NSNotification) {
            session_window::end_all();
            if !crate::rdp::join_pending_teardowns(TEARDOWN_TIMEOUT) {
                log::warn!("Some sessions did not finish disconnecting");
            }
        }

        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        fn should_handle_reopen(&self, _sender: &NSApplication, has_visible: bool) -> bool {
            if !has_visible {
                app::with(|app| app.present());
            }
            true
        }

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn terminate_after_last_window(&self, _sender: &NSApplication) -> bool {
            false
        }

        #[unsafe(method(applicationSupportsSecureRestorableState:))]
        fn supports_secure_restorable_state(&self, _app: &NSApplication) -> bool {
            true
        }
    }
);

impl AppDelegate {
    pub(super) fn new(mtm: MainThreadMarker, request: LaunchRequest) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars {
            pending: RefCell::new(request),
        });
        // SAFETY: NSObject's `init` signature is correct.
        unsafe { msg_send![super(this), init] }
    }
}
