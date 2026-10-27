//! Session windows: one per remote session, with the session toolbar (display scale, keys,
//! full screen, disconnect), engine event handling and the open-session registry used by
//! the quit prompt.

use super::app;
use super::session_view::{Overlay, SessionView};
use super::toolbar;
use super::util::{self, ns, rect};
use crate::frontend::common::launch::Launch;
use crate::frontend::common::session::{
    certificate_text, scale_menu, scale_tooltip, screenshot_path, SessionController, SessionState,
    Wake, CERTIFICATE_ANSWERS,
};
use crate::rdp::{
    BridgeEvent, CertificateDecision, CertificatePrompt, ConnectionQuality, LocalClipboard,
    LockKeys, NetworkStats, RemoteClipboard, RgbaImage, SessionStatus,
};
use crate::session::keys::KeyCombo;
use crate::session::{PreparedSession, ScaleChoice, MIN_SCALE};
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplicationPresentationOptions, NSBackingStoreType, NSBitmapImageFileType, NSBitmapImageRep,
    NSControlStateValueOff, NSControlStateValueOn, NSEvent, NSEventModifierFlags, NSImageView,
    NSMenu, NSMenuItem, NSMenuToolbarItem, NSPasteboard, NSPasteboardTypePNG, NSPasteboardTypeTIFF,
    NSTextField, NSToolbarFlexibleSpaceItemIdentifier, NSToolbarItem, NSWindow,
    NSWindowCollectionBehavior, NSWindowDelegate, NSWindowStyleMask, NSWindowTabbingMode,
};
use objc2_core_foundation::{CFData, CFRetained};
use objc2_core_graphics::{
    kCGColorSpaceSRGB, CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage,
    CGImageAlphaInfo,
};
use objc2_foundation::{
    NSCopying, NSDictionary, NSNotification, NSObject, NSObjectProtocol, NSSize, NSString,
};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Quiet period after the last size change before the host is asked to resize.
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(300);

/// How long a notice replaces the window subtitle.
const NOTICE_DURATION: Duration = Duration::from_secs(4);

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// The open sessions. UI objects are main-thread only, so engine wake-ups (from any
    /// thread) carry only the id and look the session up here.
    static SESSIONS: RefCell<Vec<Rc<Session>>> = const { RefCell::new(Vec::new()) };
}

fn find(id: u64) -> Option<Rc<Session>> {
    SESSIONS.with_borrow(|sessions| sessions.iter().find(|s| s.id == id).cloned())
}

fn all() -> Vec<Rc<Session>> {
    SESSIONS.with_borrow(Clone::clone)
}

/// Names of the sessions that are still connected (or connecting).
pub(super) fn open_names() -> Vec<String> {
    all()
        .iter()
        .filter(|s| s.is_active())
        .map(|s| s.name.clone())
        .collect()
}

/// Disconnects every session without touching the windows (the app is terminating).
pub(super) fn end_all() {
    for session in all() {
        session.view.detach();
        session.with_controller(|controller| {
            controller.release_input();
            controller.end();
        });
    }
}

/// Opens a session window and starts connecting.
pub(super) fn open(
    mtm: MainThreadMarker,
    launch: Launch,
    prepared: PreparedSession,
) -> Result<(), String> {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let wake: Wake = Arc::new(move || {
        DispatchQueue::main().exec_async(move || drain(id));
    });
    let name = launch.name.clone();
    let dynamic_resolution = launch.dynamic_resolution;
    let placement = launch.target.as_ref().map(|t| format!("Session {t}"));
    let controller = SessionController::start(launch, prepared, wake)?;

    let window = new_window(mtm, &name);
    let session = Rc::new_cyclic(|weak: &Weak<Session>| Session {
        id,
        name: name.clone(),
        view: SessionView::new(mtm, weak.clone()),
        window: window.clone(),
        controller: RefCell::new(controller),
        scale_menu: util::menu(mtm, "Display Scale"),
        scale_item: RefCell::new(None),
        view_only_item: RefCell::new(None),
        quality: NSImageView::new(mtm),
        resize_generation: Cell::new(0),
        notice_generation: Cell::new(0),
        has_frame: Cell::new(false),
        frame_dirty: Cell::new(false),
        render_scheduled: Cell::new(false),
        notified: Cell::new(false),
    });
    window.setContentView(Some(&session.view));
    session.install_toolbar(dynamic_resolution);
    session.rebuild_scale_menu();

    let delegate = SessionWindowDelegate::new(mtm, id);
    window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    util::retain_with(&window, &delegate);

    match &placement {
        // Frame autosave remembers size, position and display per connection.
        Some(name) if window.setFrameUsingName(&ns(name)) => {
            window.setFrameAutosaveName(&ns(name));
        }
        Some(name) => {
            window.center();
            window.setFrameAutosaveName(&ns(name));
        }
        None => window.center(),
    }
    SESSIONS.with_borrow_mut(|sessions| sessions.push(Rc::clone(&session)));
    session.view.set_overlay(
        Overlay::Connecting {
            name: name.clone(),
            detail: "Starting session".to_string(),
        },
        || {},
    );
    window.makeKeyAndOrderFront(None);
    session.view.take_focus();
    // Events queued before the session was registered have already sent their wake-up.
    util::defer(move || drain(id));
    // Like the other frontends, sessions start full screen (the initial resolution was
    // chosen for it); a session that already ended stays windowed.
    let weak = Rc::downgrade(&session);
    util::defer(move || {
        if let Some(session) = weak.upgrade() {
            if session.is_active() && !session.is_full_screen() {
                session.window.toggleFullScreen(None);
            }
        }
    });
    Ok(())
}

fn new_window(mtm: MainThreadMarker, name: &str) -> Retained<NSWindow> {
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable;
    // SAFETY: Plain window creation; the window is owned by `Retained` and not released
    // when closed.
    let window = unsafe {
        let window = NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            rect(0.0, 0.0, 1280.0, 800.0),
            style,
            NSBackingStoreType::Buffered,
            false,
        );
        window.setReleasedWhenClosed(false);
        window
    };
    window.setTitle(&ns(name));
    window.setSubtitle(&ns("Connecting…"));
    window.setMinSize(NSSize::new(640.0, 400.0));
    window.setCollectionBehavior(NSWindowCollectionBehavior::FullScreenPrimary);
    window.setTabbingMode(NSWindowTabbingMode::Automatic);
    window.setTabbingIdentifier(&ns("WindowsAppSession"));
    window.setAcceptsMouseMovedEvents(true);
    window
}

/// Drains the engine events of session `id` on the main thread.
fn drain(id: u64) {
    let Some(session) = find(id) else { return };
    let Some(events) = session.with_controller(SessionController::drain_events) else {
        // Busy (re-entered); the queue keeps the events, so try again shortly.
        util::defer(move || drain(id));
        return;
    };
    for event in events {
        session.handle_event(event);
    }
}

pub(super) struct Session {
    id: u64,
    name: String,
    window: Retained<NSWindow>,
    view: Retained<SessionView>,
    controller: RefCell<SessionController>,
    scale_menu: Retained<NSMenu>,
    scale_item: RefCell<Option<Retained<NSMenuToolbarItem>>>,
    view_only_item: RefCell<Option<Retained<NSMenuItem>>>,
    quality: Retained<NSImageView>,
    resize_generation: Cell<u64>,
    notice_generation: Cell<u64>,
    has_frame: Cell<bool>,
    frame_dirty: Cell<bool>,
    render_scheduled: Cell<bool>,
    /// The main window has been told that the session ended.
    notified: Cell<bool>,
}

impl Session {
    /// Runs `f` with the controller, unless it is in use further up the stack.
    pub(super) fn with_controller<R>(
        &self,
        f: impl FnOnce(&mut SessionController) -> R,
    ) -> Option<R> {
        match self.controller.try_borrow_mut() {
            Ok(mut controller) => Some(f(&mut controller)),
            Err(_) => {
                log::debug!("Session controller busy; skipping a re-entrant call");
                None
            }
        }
    }

    fn state(&self) -> SessionState {
        self.with_controller(|c| c.state().clone())
            .unwrap_or_else(|| SessionState::Connecting(String::new()))
    }

    fn is_active(&self) -> bool {
        !matches!(self.state(), SessionState::Ended(_))
    }

    fn is_full_screen(&self) -> bool {
        self.window
            .styleMask()
            .contains(NSWindowStyleMask::FullScreen)
    }

    // ----- Toolbar -----

    fn install_toolbar(self: &Rc<Self>, dynamic_resolution: bool) {
        let mtm = self.window.mtm();
        let weak = Rc::downgrade(self);
        let disconnect = toolbar::button_item(mtm, "disconnect", "Disconnect", "xmark.circle", {
            let weak = weak.clone();
            move || {
                if let Some(this) = weak.upgrade() {
                    this.window.close();
                }
            }
        });
        let full_screen = toolbar::button_item(
            mtm,
            "fullscreen",
            "Full Screen",
            "arrow.up.left.and.arrow.down.right",
            {
                let weak = weak.clone();
                move || {
                    if let Some(this) = weak.upgrade() {
                        this.window.toggleFullScreen(None);
                    }
                }
            },
        );
        full_screen.setToolTip(Some(&ns("Enter or exit full screen (⌃⌘F)")));
        let mic_muted = crate::platform::audio::is_mic_muted();
        let mic = toolbar::button_item(
            mtm,
            "mic",
            if mic_muted { "Unmute Mic" } else { "Mute Mic" },
            if mic_muted {
                "mic.slash.fill"
            } else {
                "mic.fill"
            },
            move || {
                let _ = crate::platform::audio::toggle_mic_muted();
            },
        );
        mic.setToolTip(Some(&ns("Toggle microphone mute/unmute")));

        let volume_menu = self.volume_menu();
        let volume =
            toolbar::menu_item(mtm, "volume", "Volume", "speaker.wave.3.fill", &volume_menu);
        volume.setToolTip(Some(&ns("Adjust session output volume")));

        let keys = toolbar::menu_item(mtm, "keys", "Keys", "keyboard", &self.keys_menu());
        keys.setToolTip(Some(&ns("Send keys and other actions")));
        let quality = toolbar::view_item(mtm, "quality", "Connection", &self.quality);
        self.quality.setFrameSize(NSSize::new(22.0, 22.0));
        self.quality
            .setImage(util::symbol("wifi", "Connection: Measuring").as_deref());
        self.quality.setToolTip(Some(&ns("Connection: Measuring")));

        // SAFETY: Reading an immutable extern string constant.
        let flexible = unsafe { NSToolbarFlexibleSpaceItemIdentifier };
        let mut order: Vec<Retained<NSString>> =
            vec![ns("disconnect"), flexible.copy(), ns("quality")];
        let mut items: Vec<Retained<NSToolbarItem>> = vec![disconnect, quality, full_screen];
        if dynamic_resolution {
            let scale = toolbar::menu_item(
                mtm,
                "scale",
                "Display Scale",
                "plus.magnifyingglass",
                &self.scale_menu,
            );
            order.push(ns("scale"));
            items.push(Retained::into_super(scale.clone()));
            *self.scale_item.borrow_mut() = Some(scale);
        }
        order.push(ns("mic"));
        order.push(ns("volume"));
        order.push(ns("keys"));
        order.push(ns("fullscreen"));
        items.push(mic);
        items.push(Retained::into_super(volume));
        items.push(Retained::into_super(keys));
        toolbar::install(&self.window, "SessionToolbar", items, order);
    }

    fn volume_menu(self: &Rc<Self>) -> Retained<NSMenu> {
        let mtm = self.window.mtm();
        let menu = util::menu(mtm, "Volume");
        menu.addItem(&util::caption_item(mtm, "Session Volume"));
        let levels = [
            (100, "100%"),
            (75, "75%"),
            (50, "50%"),
            (25, "25%"),
            (0, "Mute (0%)"),
        ];
        for (vol, label) in levels {
            menu.addItem(&util::menu_item(mtm, label, "", move || {
                crate::platform::audio::set_volume(vol);
            }));
        }
        menu
    }

    fn keys_menu(self: &Rc<Self>) -> Retained<NSMenu> {
        let mtm = self.window.mtm();
        let menu = util::menu(mtm, "Keys");
        menu.addItem(&util::caption_item(mtm, "Send Keys"));
        for combo in KeyCombo::ALL {
            let weak = Rc::downgrade(self);
            menu.addItem(&util::menu_item(mtm, combo.label(), "", move || {
                if let Some(this) = weak.upgrade() {
                    this.send_combo(combo);
                }
            }));
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let weak = Rc::downgrade(self);
        menu.addItem(&util::menu_item(
            mtm,
            "Type Clipboard Text",
            "",
            move || {
                if let Some(this) = weak.upgrade() {
                    this.type_clipboard();
                }
            },
        ));
        let weak = Rc::downgrade(self);
        menu.addItem(&util::menu_item(mtm, "Save Screenshot", "", move || {
            if let Some(this) = weak.upgrade() {
                this.save_screenshot();
            }
        }));
        let weak = Rc::downgrade(self);
        let view_only = util::menu_item(mtm, "View Only", "", move || {
            if let Some(this) = weak.upgrade() {
                this.toggle_view_only();
            }
        });
        menu.addItem(&view_only);
        *self.view_only_item.borrow_mut() = Some(view_only);
        menu
    }

    fn rebuild_scale_menu(self: &Rc<Self>) {
        let mtm = self.window.mtm();
        let Some(current) = self.with_controller(|c| c.scale()) else {
            return;
        };
        let menu = &self.scale_menu;
        menu.removeAllItems();
        menu.addItem(&util::caption_item(mtm, "Display Scale"));
        for entry in scale_menu(current) {
            let weak = Rc::downgrade(self);
            let choice = entry.choice;
            let item = util::menu_item(mtm, &entry.label, "", move || {
                if let Some(this) = weak.upgrade() {
                    this.set_scale(choice);
                }
            });
            item.setState(if entry.checked {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
            menu.addItem(&item);
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let weak = Rc::downgrade(self);
        menu.addItem(&util::menu_item(mtm, "Custom…", "", move || {
            if let Some(this) = weak.upgrade() {
                this.ask_custom_scale();
            }
        }));
        if let Some(item) = &*self.scale_item.borrow() {
            item.setToolTip(Some(&ns(&scale_tooltip(current))));
        }
    }

    fn set_scale(self: &Rc<Self>, choice: ScaleChoice) {
        self.with_controller(|c| c.set_scale(choice));
        self.rebuild_scale_menu();
        self.send_resize();
    }

    fn ask_custom_scale(self: &Rc<Self>) {
        let mtm = self.window.mtm();
        let current = self
            .with_controller(|c| c.scale().resolve(self.window.backingScaleFactor()))
            .unwrap_or(MIN_SCALE)
            .min(ScaleChoice::MAX_PERCENT);
        let alert = util::alert(
            mtm,
            "Custom Display Scale",
            &format!(
                "Scale of the remote desktop in percent, from {MIN_SCALE}% to {}%. Applies to \
                 this session only.",
                ScaleChoice::MAX_PERCENT
            ),
            &["Apply", "Cancel"],
        );
        let field = NSTextField::textFieldWithString(&ns(&current.to_string()), mtm);
        field.setFrame(rect(0.0, 0.0, 120.0, 24.0));
        field.setPlaceholderString(Some(&ns("Percent")));
        alert.setAccessoryView(Some(&field));
        alert.window().setInitialFirstResponder(Some(&field));
        let weak = Rc::downgrade(self);
        let input = field.clone();
        util::present(&alert, Some(&self.window), move |index| {
            let Some(this) = weak.upgrade() else { return };
            if index != 0 {
                return;
            }
            let text = input.stringValue().to_string();
            let text = text.trim().trim_end_matches('%').trim();
            match text.parse::<u32>() {
                Ok(percent) => this.set_scale(ScaleChoice::percent(percent)),
                Err(_) => util::inform(
                    this.window.mtm(),
                    Some(&this.window),
                    "Invalid Scale",
                    &format!(
                        "Enter a whole number from {MIN_SCALE} to {}.",
                        ScaleChoice::MAX_PERCENT
                    ),
                ),
            }
        });
    }

    // ----- Session actions -----

    /// Sends keys after releasing what is held locally (so modifiers don't combine).
    fn send_combo(self: &Rc<Self>, combo: KeyCombo) {
        if self.view_only() {
            self.notice("Input is off while the session is view only.");
            return;
        }
        self.with_controller(|c| {
            c.release_input();
            c.send_combo(combo);
        });
    }

    /// Types the clipboard text where pasting does not work (sign-in screens, consoles).
    fn type_clipboard(self: &Rc<Self>) {
        if self.view_only() {
            self.notice("Input is off while the session is view only.");
            return;
        }
        let Some(text) = util::pasteboard_text().filter(|t| !t.is_empty()) else {
            self.notice("The clipboard holds no text.");
            return;
        };
        let result = self.with_controller(|c| {
            c.release_input();
            c.type_text(&text)
        });
        if let Some(Err(message)) = result {
            self.notice(&message);
        }
    }

    fn view_only(&self) -> bool {
        self.with_controller(|c| c.view_only()).unwrap_or(false)
    }

    fn toggle_view_only(self: &Rc<Self>) {
        let enabled = !self.view_only();
        self.with_controller(|c| c.set_view_only(enabled));
        if let Some(item) = &*self.view_only_item.borrow() {
            item.setState(if enabled {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        }
        self.update_subtitle();
    }

    /// Saves the remote screen as a PNG in the Pictures folder.
    fn save_screenshot(self: &Rc<Self>) {
        let Some(image) = self.view.image() else {
            self.notice("Nothing to capture yet.");
            return;
        };
        let path = screenshot_path(&self.name);
        match png_data(&image).ok_or_else(|| "The image could not be encoded.".to_string()) {
            Ok(data) => match std::fs::write(&path, data) {
                Ok(()) => self.notice(&format!("Screenshot saved to {}", path.display())),
                Err(e) => {
                    log::warn!("Could not save the screenshot to {}: {e}", path.display());
                    self.notice(&format!("Could not save the screenshot: {e}"));
                }
            },
            Err(e) => self.notice(&format!("Could not save the screenshot: {e}")),
        }
    }

    // ----- Window chrome -----

    fn update_subtitle(&self) {
        let state = self.state();
        let text = if state == SessionState::Connected && self.view_only() {
            "View Only"
        } else {
            state.title_subtitle()
        };
        self.window.setSubtitle(&ns(text));
    }

    /// Shows `message` in place of the subtitle for a while.
    fn notice(self: &Rc<Self>, message: &str) {
        let generation = self.notice_generation.get().wrapping_add(1);
        self.notice_generation.set(generation);
        self.window.setSubtitle(&ns(message));
        let weak = Rc::downgrade(self);
        util::run_later(NOTICE_DURATION, move || {
            if let Some(this) = weak.upgrade() {
                if this.notice_generation.get() == generation {
                    this.update_subtitle();
                }
            }
        });
    }

    fn show_network(&self, stats: &NetworkStats) {
        let symbol = match stats.quality() {
            ConnectionQuality::Excellent | ConnectionQuality::Good => "wifi",
            ConnectionQuality::Fair => "wifi.exclamationmark",
            ConnectionQuality::Poor => "exclamationmark.triangle",
            ConnectionQuality::Unknown => "wifi.slash",
        };
        let tooltip = format!("Connection: {}", stats.summary());
        self.quality
            .setImage(util::symbol(symbol, &tooltip).as_deref());
        self.quality.setToolTip(Some(&ns(&tooltip)));
    }

    // ----- Size and focus -----

    /// Asks the host to match the view once resizing has settled.
    pub(super) fn schedule_resize(self: &Rc<Self>) {
        let generation = self.resize_generation.get().wrapping_add(1);
        self.resize_generation.set(generation);
        let weak = Rc::downgrade(self);
        util::run_later(RESIZE_DEBOUNCE, move || {
            if let Some(this) = weak.upgrade() {
                if this.resize_generation.get() == generation && !this.view.inLiveResize() {
                    this.send_resize();
                }
            }
        });
    }

    fn send_resize(&self) {
        let size = self.view.bounds().size;
        let scale = self.window.backingScaleFactor();
        if size.width < 1.0 || size.height < 1.0 {
            return;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let device = (
            (size.width * scale).round() as u32,
            (size.height * scale).round() as u32,
        );
        self.with_controller(|c| c.resize(device, scale));
    }

    /// The session got the keyboard: synchronise lock keys and offer the clipboard.
    pub(super) fn focus_gained(&self) {
        if !self.window.isKeyWindow() || !self.view.is_first_responder() {
            return;
        }
        let caps_lock = NSEvent::modifierFlags_class().contains(NSEventModifierFlags::CapsLock);
        self.with_controller(|c| {
            c.focus_in(LockKeys {
                scroll_lock: false,
                num_lock: false,
                caps_lock,
            });
        });
        self.sync_local_clipboard();
    }

    /// The session lost the keyboard: release everything held remotely.
    pub(super) fn focus_lost(&self) {
        self.with_controller(SessionController::release_input);
    }

    /// Offers the local clipboard text (the engine skips unchanged content).
    fn sync_local_clipboard(&self) {
        if let Some(text) = util::pasteboard_text().filter(|t| !t.is_empty()) {
            self.with_controller(|c| c.set_local_clipboard(LocalClipboard::Text(text)));
        }
    }

    /// Synchronizes remote frame presentation with display refresh (triple-buffered),
    /// coalescing intermediate slice/tile updates so only complete frames are presented.
    fn schedule_frame_render(self: &Rc<Self>) {
        if self.render_scheduled.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        util::run_later(Duration::from_millis(8), move || {
            let Some(this) = weak.upgrade() else { return };
            this.render_scheduled.set(false);
            if this.frame_dirty.replace(false) {
                let view = &this.view;
                this.with_controller(|c| {
                    if let Some(bridge) = c.bridge() {
                        bridge.with_frame(|frame| view.show_frame(frame));
                    }
                });
            }
        });
    }

    // ----- Engine events -----

    fn handle_event(self: &Rc<Self>, event: BridgeEvent) {
        match event {
            BridgeEvent::FrameUpdated => {
                self.frame_dirty.set(true);
                if !self.has_frame.replace(true) {
                    let view = &self.view;
                    self.with_controller(|c| {
                        if let Some(bridge) = c.bridge() {
                            bridge.with_frame(|frame| view.show_frame(frame));
                        }
                    });
                    self.view.set_overlay(Overlay::None, || {});
                    self.view.take_focus();
                } else {
                    self.schedule_frame_render();
                }
            }
            BridgeEvent::Status(status) => self.handle_status(status),
            BridgeEvent::AuthRequired { url } => self.sign_in(&url),
            BridgeEvent::RemoteClipboard(content) => set_remote_clipboard(content),
            BridgeEvent::CertificatePrompt(prompt) => self.ask_certificate(&prompt),
            BridgeEvent::Network(stats) => self.show_network(&stats),
        }
    }

    fn handle_status(self: &Rc<Self>, status: SessionStatus) {
        match status {
            SessionStatus::Progress(message) => {
                self.window.setSubtitle(&ns(&message));
                if !self.has_frame.get() {
                    self.view.set_overlay(
                        Overlay::Connecting {
                            name: self.name.clone(),
                            detail: message,
                        },
                        || {},
                    );
                }
            }
            SessionStatus::Connected => {
                self.update_subtitle();
                // The initial size was a guess; match the window now.
                self.schedule_resize();
                self.sync_local_clipboard();
            }
            SessionStatus::Failed(message) | SessionStatus::Ended(message) => {
                self.show_ended(&message);
            }
        }
    }

    /// Shows why the session is over; the window stays open until the user closes it.
    fn show_ended(self: &Rc<Self>, message: &str) {
        self.update_subtitle();
        if message.is_empty() {
            self.window.setSubtitle(&ns("Disconnected"));
        }
        self.quality
            .setImage(util::symbol("wifi.slash", "Connection: Disconnected").as_deref());
        self.quality
            .setToolTip(Some(&ns("Connection: Disconnected")));
        if self.is_full_screen() {
            self.window.toggleFullScreen(None);
        }
        let weak = Rc::downgrade(self);
        self.view.set_overlay(
            Overlay::Ended {
                message: message.to_string(),
            },
            move || {
                if let Some(this) = weak.upgrade() {
                    this.window.close();
                }
            },
        );
        self.notify_ended();
    }

    fn notify_ended(&self) {
        if !self.notified.replace(true) {
            app::with(|app| app.session_ended(&self.name));
        }
    }

    fn sign_in(self: &Rc<Self>, url: &str) {
        self.window.setSubtitle(&ns("Signing in…"));
        let weak = Rc::downgrade(self);
        super::signin::session_host(
            self.window.mtm(),
            Some(self.window.clone()),
            url,
            move |redirect| {
                let Some(this) = weak.upgrade() else { return };
                if redirect.is_none() {
                    this.window.setSubtitle(&ns("Sign-in cancelled"));
                }
                this.with_controller(|c| c.provide_auth_result(redirect.as_deref()));
            },
        );
    }

    /// Asks whether to trust a certificate the session host presented.
    fn ask_certificate(self: &Rc<Self>, prompt: &CertificatePrompt) {
        self.window.setSubtitle(&ns("Verifying the certificate…"));
        let text = certificate_text(prompt);
        let labels: Vec<&str> = CERTIFICATE_ANSWERS
            .iter()
            .map(|(_, label)| *label)
            .collect();
        let alert = util::alert(self.window.mtm(), text.heading, &text.body(), &labels);
        if text.changed {
            util::destructive(&alert, CERTIFICATE_ANSWERS.len() - 1);
        }
        let weak = Rc::downgrade(self);
        util::present(&alert, Some(&self.window), move |index| {
            let decision = CERTIFICATE_ANSWERS
                .get(index)
                .map_or(CertificateDecision::Reject, |(decision, _)| *decision);
            if let Some(this) = weak.upgrade() {
                this.with_controller(|c| c.answer_certificate(decision));
                this.update_subtitle();
            }
        });
    }

    /// The window is closing: disconnect and forget the session.
    fn closed(&self) {
        self.view.detach();
        self.with_controller(|c| {
            c.release_input();
            c.end();
        });
        self.notify_ended();
        let id = self.id;
        // Dropped after AppKit has finished closing the window.
        util::defer(move || SESSIONS.with_borrow_mut(|sessions| sessions.retain(|s| s.id != id)));
    }
}

/// Puts content copied in the session on the general pasteboard.
fn set_remote_clipboard(content: RemoteClipboard) {
    match content {
        RemoteClipboard::Text(text) => util::set_pasteboard_text(&text),
        RemoteClipboard::Image(image) => {
            if !set_pasteboard_image(&image) {
                log::warn!("Ignoring an invalid clipboard image from the session");
            }
        }
        RemoteClipboard::Files(files) => {
            log::info!(
                "The session copied {} file(s); file transfer through the clipboard is not \
                 supported on macOS",
                files.len()
            );
        }
    }
}

fn set_pasteboard_image(image: &RgbaImage) -> bool {
    let Some(cg_image) = image_from_rgba(image) else {
        return false;
    };
    let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg_image);
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();
    // SAFETY: Reading immutable extern string constants; the property dictionary is empty.
    let (png, tiff, png_type, tiff_type) = unsafe {
        (
            rep.representationUsingType_properties(
                NSBitmapImageFileType::PNG,
                &NSDictionary::new(),
            ),
            rep.TIFFRepresentation(),
            NSPasteboardTypePNG,
            NSPasteboardTypeTIFF,
        )
    };
    let png_set = png.is_some_and(|data| pasteboard.setData_forType(Some(&data), png_type));
    let tiff_set = tiff.is_some_and(|data| pasteboard.setData_forType(Some(&data), tiff_type));
    png_set || tiff_set
}

fn image_from_rgba(image: &RgbaImage) -> Option<CFRetained<CGImage>> {
    let width = usize::try_from(image.width).ok()?;
    let height = usize::try_from(image.height).ok()?;
    let stride = width.checked_mul(4)?;
    let len = stride.checked_mul(height)?;
    if width == 0 || height == 0 || image.pixels.len() < len {
        return None;
    }
    let data = CFData::from_bytes(&image.pixels[..len]);
    let provider = CGDataProvider::with_cf_data(Some(&data))?;
    // SAFETY: Reading an immutable extern string constant.
    let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))?;
    // SAFETY: The geometry matches the data provider's bytes and `decode` may be null.
    unsafe {
        CGImage::new(
            width,
            height,
            8,
            32,
            stride,
            Some(&space),
            CGBitmapInfo(CGImageAlphaInfo::Last.0),
            Some(&provider),
            std::ptr::null(),
            false,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }
}

fn png_data(image: &CGImage) -> Option<Vec<u8>> {
    let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), image);
    // SAFETY: An empty property dictionary is valid for every file type.
    let data = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }?;
    Some(data.to_vec())
}

// ----- Window delegate -----

pub(super) struct DelegateIvars {
    id: u64,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and the class has no Drop impl.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WindowsAppSessionWindowDelegate"]
    #[ivars = DelegateIvars]
    struct SessionWindowDelegate;

    // SAFETY: NSObjectProtocol has no safety requirements.
    unsafe impl NSObjectProtocol for SessionWindowDelegate {}

    // SAFETY: The method signatures match the protocol.
    unsafe impl NSWindowDelegate for SessionWindowDelegate {
        #[unsafe(method(windowDidBecomeKey:))]
        fn window_did_become_key(&self, _notification: &NSNotification) {
            if let Some(session) = self.session() {
                session.focus_gained();
            }
        }

        #[unsafe(method(windowDidResignKey:))]
        fn window_did_resign_key(&self, _notification: &NSNotification) {
            if let Some(session) = self.session() {
                session.focus_lost();
            }
        }

        #[unsafe(method(windowDidEndLiveResize:))]
        fn window_did_end_live_resize(&self, _notification: &NSNotification) {
            if let Some(session) = self.session() {
                session.schedule_resize();
            }
        }

        #[unsafe(method(windowDidChangeBackingProperties:))]
        fn window_did_change_backing_properties(&self, _notification: &NSNotification) {
            if let Some(session) = self.session() {
                session.schedule_resize();
            }
        }

        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            if let Some(session) = self.session() {
                session.closed();
            }
        }

        #[unsafe(method(window:willUseFullScreenPresentationOptions:))]
        fn full_screen_options(
            &self,
            _window: &NSWindow,
            proposed: NSApplicationPresentationOptions,
        ) -> NSApplicationPresentationOptions {
            // The toolbar slides in with the menu bar instead of covering the desktop.
            proposed | NSApplicationPresentationOptions::AutoHideToolbar
        }
    }
);

impl SessionWindowDelegate {
    fn new(mtm: MainThreadMarker, id: u64) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars { id });
        // SAFETY: NSObject's `init` signature is correct.
        unsafe { msg_send![super(this), init] }
    }

    fn session(&self) -> Option<Rc<Session>> {
        find(self.ivars().id)
    }
}
