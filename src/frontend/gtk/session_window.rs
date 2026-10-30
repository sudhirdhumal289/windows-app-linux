//! Window hosting one embedded FreeRDP session.

use super::canvas::RdpCanvas;
use super::clipboard::{self, RemoteFilesProvider};
use super::fullscreen_dock::FullscreenDock;
use super::open_sessions;
use super::placement;
use super::rdp_auth::RdpAuthDialog;
use crate::launcher::LaunchTarget;
use crate::placement::{PlacementStore, WindowPlacement};
use crate::rdp::keymap::{rdp_scancode_from_xkb, PressedKeys};
use crate::rdp::{
    BridgeEvent, BridgeOptions, CertificateDecision, CertificatePrompt, ConnectionQuality,
    LockKeys, MonitorRect, MouseButton, NativeRdpBridge, NetworkStats, RemoteClipboard,
    SessionStatus, WheelAccumulator,
};
use crate::session::keys::{self, KeyCombo, KeyInput};
use crate::session::{PreparedSession, ScaleChoice};
use adw::prelude::*;
use gtk4::gdk::{self, Key, ModifierType};
use gtk4::{
    gio, glib, Box, Button, EventControllerKey, EventControllerLegacy, EventControllerMotion,
    EventControllerScroll, EventControllerScrollFlags, GestureClick, MenuButton, Orientation,
    Overlay, Separator, Spinner,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// Resize requests are sent once the window size has been stable for this long.
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(300);
const PLACEMENT_SETTLE: Duration = Duration::from_secs(1);
/// Touchpad scrolling reports pixels; this many count as one wheel notch.
const SURFACE_PX_PER_NOTCH: f64 = 50.0;
/// Most monitors a remote layout can contain.
const MAX_MONITORS: usize = 16;

/// How a session window behaves.
#[derive(Debug, Clone, Default)]
pub struct SessionWindowOptions {
    /// Account pre-filled when the session host asks for a Microsoft sign-in.
    pub login_hint: Option<String>,
    /// Span all local monitors while fullscreen.
    pub multi_monitor: bool,
    /// The remote desktop follows the window size (otherwise it has a fixed size, which
    /// is scaled to the window).
    pub dynamic_resolution: bool,
    /// Reopen on the monitor and workspace this connection's window was last on.
    pub remember_placement: Option<LaunchTarget>,
}

/// A fullscreen window showing the session's part on another monitor.
struct SecondaryScreen {
    monitor: gdk::Monitor,
    window: gtk4::Window,
    canvas: RdpCanvas,
}

pub struct SessionWindow {
    window: adw::Window,
    name: String,
    /// Identifies this session's clipboard offers.
    id: String,
    /// Where files copied in the session are downloaded to (removed on close).
    clipboard_dir: RefCell<Option<std::path::PathBuf>>,
    title: adw::WindowTitle,
    toasts: adw::ToastOverlay,
    canvas: RdpCanvas,
    loading: Box,
    loading_label: gtk4::Label,
    bridge: RefCell<Option<Rc<NativeRdpBridge>>>,
    /// Owns the private `.rdp` file for the lifetime of the session.
    session: RefCell<Option<PreparedSession>>,
    /// Remote desktop scale for this session only; never persisted.
    scale: Cell<ScaleChoice>,
    scale_action: gio::SimpleAction,
    /// Menu section holding the custom scale entry.
    custom_scale_section: gio::Menu,
    /// Header and dock buttons opening the scale menu.
    scale_buttons: Vec<MenuButton>,
    /// Header and dock buttons opening the session actions menu.
    session_buttons: Vec<MenuButton>,
    dynamic_resolution: bool,
    /// Input is not sent to the session.
    view_only: Cell<bool>,
    /// System shortcuts (Super, Alt+Tab, ...) go to the session while it is focused.
    capture_shortcuts: Cell<bool>,
    pointer: Cell<(u16, u16)>,
    held_buttons: RefCell<Vec<MouseButton>>,
    pressed_keys: RefCell<PressedKeys>,
    wheel: RefCell<WheelAccumulator>,
    resize_timer: RefCell<Option<glib::SourceId>>,
    has_frame: Cell<bool>,
    frame_dirty: Cell<bool>,
    multi_monitor: bool,
    /// Windows on the other monitors while the session spans them.
    screens: RefCell<Vec<SecondaryScreen>>,
    /// Latest remote frame, shared by all canvases.
    texture: RefCell<Option<gdk::Texture>>,
    /// Connection quality indicators (header and dock).
    hud: Box,
    hud_label: gtk4::Label,
    hud_visible: Cell<bool>,
    diagnostics: RefCell<crate::session::diagnostics::SessionDiagnostics>,
    quality_icons: Vec<gtk4::Image>,
    /// Connection whose window placement is remembered, and the placement last saved.
    placement_key: Option<LaunchTarget>,
    placement: RefCell<WindowPlacement>,
    /// The saved placement has been applied, so the current one can be recorded.
    placement_restored: Cell<bool>,
    on_close: RefCell<Option<std::boxed::Box<dyn FnOnce()>>>,
}

impl SessionWindow {
    /// Opens a window titled `name` and connects with `session`. `on_close` runs once when
    /// the window closes (or the session could not be created).
    pub fn open(
        name: &str,
        options: SessionWindowOptions,
        session: PreparedSession,
        on_close: impl FnOnce() + 'static,
    ) {
        // Independent toplevel (not transient) so the compositor offers resize handles.
        let window = adw::Window::builder()
            .title(format!("{name} - Windows App"))
            .default_width(1280)
            .default_height(800)
            .build();

        let header = adw::HeaderBar::new();
        let disconnect_button = Button::builder()
            .label("Disconnect")
            .icon_name("system-log-out-symbolic")
            .tooltip_text("Disconnect")
            .build();
        disconnect_button.add_css_class("destructive-action");
        header.pack_start(&disconnect_button);
        let fullscreen_button = Button::builder()
            .icon_name("view-fullscreen-symbolic")
            .tooltip_text("Toggle Fullscreen (Ctrl+Alt+Enter)")
            .build();
        header.pack_end(&fullscreen_button);

        let (scale_menu, custom_scale_section) = scale_menu();
        let scale_button = MenuButton::builder()
            .icon_name("zoom-in-symbolic")
            .tooltip_text("Display Scale")
            .menu_model(&scale_menu)
            .visible(options.dynamic_resolution)
            .build();
        let session_menu = session_menu();
        let session_button = MenuButton::builder()
            .icon_name("input-keyboard-symbolic")
            .tooltip_text("Keys and Session Actions")
            .menu_model(&session_menu)
            .build();
        header.pack_end(&session_button);
        header.pack_end(&scale_button);
        let header_quality = quality_icon();
        header.pack_end(&header_quality);

        let title = adw::WindowTitle::new(name, "Connecting…");
        header.set_title_widget(Some(&title));

        let canvas = RdpCanvas::default();
        let overlay = Overlay::new();
        overlay.set_child(Some(&canvas));

        let (loading, loading_label) = loading_card(name);
        overlay.add_overlay(&loading);
        let (hud, hud_label) = hud_card();
        overlay.add_overlay(&hud);

        let dock = FullscreenDock::new(
            &window,
            &overlay,
            name,
            scale_menu.upcast_ref(),
            session_menu.upcast_ref(),
        );
        dock.scale_button.set_visible(options.dynamic_resolution);

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&overlay));
        let content = adw::ToolbarView::new();
        content.add_top_bar(&header);
        content.set_content(Some(&toasts));
        window.set_content(Some(&content));

        let this = Rc::new(Self {
            window: window.clone(),
            name: name.to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            clipboard_dir: RefCell::new(None),
            title,
            toasts,
            canvas,
            loading,
            loading_label,
            bridge: RefCell::new(None),
            session: RefCell::new(None),
            scale: Cell::new(ScaleChoice::Auto),
            scale_action: gio::SimpleAction::new_stateful(
                "scale",
                Some(glib::VariantTy::STRING),
                &ScaleChoice::Auto.id().to_variant(),
            ),
            custom_scale_section,
            scale_buttons: vec![scale_button, dock.scale_button.clone()],
            session_buttons: vec![session_button, dock.session_button.clone()],
            dynamic_resolution: options.dynamic_resolution,
            view_only: Cell::new(false),
            capture_shortcuts: Cell::new(true),
            pointer: Cell::new((0, 0)),
            held_buttons: RefCell::new(Vec::new()),
            pressed_keys: RefCell::new(PressedKeys::default()),
            wheel: RefCell::new(WheelAccumulator::default()),
            resize_timer: RefCell::new(None),
            has_frame: Cell::new(false),
            frame_dirty: Cell::new(false),
            multi_monitor: options.multi_monitor,
            screens: RefCell::new(Vec::new()),
            texture: RefCell::new(None),
            hud,
            hud_label,
            hud_visible: Cell::new(false),
            diagnostics: RefCell::new(crate::session::diagnostics::SessionDiagnostics::new()),
            quality_icons: vec![header_quality, dock.quality_icon.clone()],
            placement: RefCell::new(
                options
                    .remember_placement
                    .as_ref()
                    .map(|target| PlacementStore::default().get(target))
                    .unwrap_or_default(),
            ),
            placement_key: options.remember_placement,
            placement_restored: Cell::new(false),
            on_close: RefCell::new(Some(std::boxed::Box::new(on_close))),
        });

        open_sessions::register(&this.window, name);
        this.install_window_chrome(&header, &fullscreen_button, &disconnect_button, &dock);
        this.install_scale_actions();
        this.install_session_actions();
        this.attach_pointer_input(&this.canvas);
        this.window.add_controller(this.key_controller());
        this.install_focus_handling(this.window.upcast_ref());
        this.install_clipboard_sync();
        this.install_monitor_tracking();
        this.install_frame_renderer();

        this.start(session, options.login_hint);

        window.present();
        this.track_placement();
        // Enter fullscreen only after the window has been mapped normally: mapping it
        // directly as fullscreen leaves Mutter without a placed restore geometry, so
        // leaving fullscreen would put the title bar behind GNOME's top bar.
        let ticks = Cell::new(0u8);
        let weak = Rc::downgrade(&this);
        window.add_tick_callback(move |_, _| {
            ticks.set(ticks.get() + 1);
            if ticks.get() < 2 {
                return glib::ControlFlow::Continue;
            }
            if let Some(this) = weak.upgrade() {
                this.restore_placement();
            }
            glib::ControlFlow::Break
        });
    }

    /// Moves the newly shown window to its saved workspace and enters fullscreen on its
    /// saved monitor (or the current one).
    fn restore_placement(self: &Rc<Self>) {
        let saved = self.placement.borrow().clone();
        if let Some(workspace) = saved.workspace {
            placement::move_to_workspace(self.window.upcast_ref(), workspace);
        }
        // A session that already ended (e.g. the host was not found) stays windowed.
        if self.bridge().is_some() && !self.window.is_fullscreen() {
            let monitor = saved
                .monitor
                .and_then(|id| placement::find_monitor(&WidgetExt::display(&self.window), &id));
            match monitor {
                Some(monitor) => self.window.fullscreen_on_monitor(&monitor),
                None => self.window.fullscreen(),
            }
        }
        // Start tracking once the window manager has applied the moves, so their
        // intermediate state is not recorded. Staying on the same monitor emits no
        // `enter-monitor`, hence the explicit save.
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(PLACEMENT_SETTLE, move || {
            if let Some(this) = weak.upgrade() {
                this.placement_restored.set(true);
                this.save_placement();
            }
        });
    }

    /// Records the placement whenever the window moves to another monitor.
    fn track_placement(self: &Rc<Self>) {
        if self.placement_key.is_none() {
            return;
        }
        let Some(surface) = self.window.surface() else {
            return;
        };
        let weak = Rc::downgrade(self);
        surface.connect_enter_monitor(move |_, _| {
            if let Some(this) = weak.upgrade() {
                this.save_placement();
            }
        });
    }

    /// Saves where the window is now. What the platform cannot report keeps its saved
    /// value (e.g. the workspace on Wayland).
    fn save_placement(&self) {
        let Some(target) = &self.placement_key else {
            return;
        };
        if !self.placement_restored.get() {
            return;
        }
        let Some(current) = placement::current(self.window.upcast_ref()) else {
            return;
        };
        let merged = {
            let saved = self.placement.borrow();
            WindowPlacement {
                monitor: current.monitor.or_else(|| saved.monitor.clone()),
                workspace: current.workspace.or(saved.workspace),
            }
        };
        if *self.placement.borrow() == merged {
            return;
        }
        log::debug!("Session window placement for {target}: {merged:?}");
        if let Err(e) = PlacementStore::default().set(target, merged.clone()) {
            log::warn!("Could not save the session window placement: {e}");
        }
        self.placement.replace(merged);
    }

    fn bridge(&self) -> Option<Rc<NativeRdpBridge>> {
        self.bridge.borrow().clone()
    }

    fn start(self: &Rc<Self>, session: PreparedSession, login_hint: Option<String>) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<BridgeEvent>();
        let sink = Arc::new(move |event| {
            let _ = tx.send(event);
        });
        let options = BridgeOptions { login_hint };
        let bridge = NativeRdpBridge::new(&session.args, options, sink);
        // The engine has copied the arguments (which may include a password).
        let session = PreparedSession {
            args: Vec::new(),
            ..session
        };
        let bridge = match bridge {
            Ok(bridge) => Rc::new(bridge),
            Err(e) => {
                log::error!("Could not create the RDP session: {e}");
                self.show_ended(&format!("Could not start the session: {e}"));
                return;
            }
        };
        *self.session.borrow_mut() = Some(session);
        *self.bridge.borrow_mut() = Some(Rc::clone(&bridge));

        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            // Ends when the bridge (and with it the sender) is gone.
            while let Some(event) = rx.recv().await {
                let Some(this) = weak.upgrade() else { break };
                this.handle_event(event);
            }
        });

        if let Err(e) = bridge.start() {
            log::error!("{e}");
            self.show_ended(&e.to_string());
        }
    }

    fn handle_event(self: &Rc<Self>, event: BridgeEvent) {
        match event {
            BridgeEvent::FrameUpdated => {
                self.diagnostics
                    .borrow_mut()
                    .record_frame(std::time::Instant::now());
                if self.hud_visible.get() {
                    self.update_hud();
                }
                self.frame_dirty.set(true);
                if !self.has_frame.replace(true) {
                    let texture = self
                        .bridge()
                        .and_then(|bridge| bridge.with_frame(RdpCanvas::texture_for));
                    if let Some(texture) = texture {
                        self.canvas.set_texture(&texture);
                        for screen in self.screens.borrow().iter() {
                            screen.canvas.set_texture(&texture);
                        }
                        *self.texture.borrow_mut() = Some(texture);
                    }
                    self.loading.set_visible(false);
                    self.canvas.grab_focus();
                } else {
                    self.canvas.queue_draw();
                }
            }
            BridgeEvent::Status(status) => self.handle_status(status),
            BridgeEvent::AuthRequired { url } => {
                self.title.set_subtitle("Signing in…");
                let weak = Rc::downgrade(self);
                RdpAuthDialog::run(&self.window, &url, move |redirect| {
                    let Some(this) = weak.upgrade() else { return };
                    if redirect.is_none() {
                        this.title.set_subtitle("Sign-in cancelled");
                    }
                    if let Some(bridge) = this.bridge() {
                        bridge.provide_auth_result(redirect.as_deref());
                    }
                });
            }
            BridgeEvent::RemoteClipboard(content) => self.set_remote_clipboard(content),
            BridgeEvent::CertificatePrompt(prompt) => self.ask_certificate(&prompt),
            BridgeEvent::Network(stats) => self.show_network(stats),
            BridgeEvent::Cursor(kind) => {
                let name = match kind {
                    crate::rdp::CursorKind::Hidden => "none",
                    crate::rdp::CursorKind::IBeam => "text",
                    crate::rdp::CursorKind::Hand => "pointer",
                    crate::rdp::CursorKind::Wait => "wait",
                    crate::rdp::CursorKind::Cross => "crosshair",
                    crate::rdp::CursorKind::ResizeNs => "ns-resize",
                    crate::rdp::CursorKind::ResizeWe => "ew-resize",
                    crate::rdp::CursorKind::ResizeNwse => "nwse-resize",
                    crate::rdp::CursorKind::ResizeNesw => "nesw-resize",
                    crate::rdp::CursorKind::Move => "move",
                    crate::rdp::CursorKind::NotAllowed => "not-allowed",
                    crate::rdp::CursorKind::Default => "default",
                };
                self.canvas.set_cursor_from_name(Some(name));
            }
            BridgeEvent::CursorBitmap(bitmap) => {
                let expected = (bitmap.width as usize)
                    .checked_mul(bitmap.height as usize)
                    .and_then(|n| n.checked_mul(4));
                if bitmap.width > 0 && bitmap.height > 0 && Some(bitmap.rgba.len()) == expected {
                    let bytes = gtk4::glib::Bytes::from(&bitmap.rgba);
                    let texture = gtk4::gdk::MemoryTexture::new(
                        bitmap.width as i32,
                        bitmap.height as i32,
                        gtk4::gdk::MemoryFormat::R8g8b8a8,
                        &bytes,
                        (bitmap.width * 4) as usize,
                    );
                    let cursor = gtk4::gdk::Cursor::from_texture(
                        &texture,
                        bitmap.hot_x as i32,
                        bitmap.hot_y as i32,
                        None,
                    );
                    self.canvas.set_cursor(Some(&cursor));
                }
            }
            BridgeEvent::CursorPosition { x, y } => {
                log::trace!("Guest OS mouse position: {x}, {y}");
            }
        }
    }

    /// Puts content copied in the session on the local clipboard.
    fn set_remote_clipboard(&self, content: RemoteClipboard) {
        let local = self.window.clipboard();
        match content {
            RemoteClipboard::Text(text) => {
                local.set_text(&text);
                local.display().primary_clipboard().set_text(&text);
            }
            RemoteClipboard::Image(image) => match clipboard::texture_from_image(&image) {
                Some(texture) => local.set_texture(&texture),
                None => log::warn!("Ignoring an invalid clipboard image from the session"),
            },
            RemoteClipboard::Files(files) => {
                let Some(bridge) = self.bridge() else { return };
                log::debug!("The session copied {} clipboard file(s)", files.len());
                let session_dir = match crate::platform::clipboard_files_dir() {
                    Ok(dir) => self
                        .clipboard_dir
                        .borrow_mut()
                        .get_or_insert_with(|| dir.join(&self.id))
                        .clone(),
                    Err(e) => {
                        log::warn!("Cannot offer the session's files: {e}");
                        return;
                    }
                };
                // Each copy gets its own folder so earlier pastes stay intact.
                let dest = session_dir.join(uuid::Uuid::new_v4().to_string());
                let provider = RemoteFilesProvider::new(&self.id, bridge.remote_files(), dest);
                if let Err(e) = local.set_content(Some(&provider)) {
                    log::warn!("Could not offer the session's files: {e}");
                }
            }
        }
    }

    /// Asks whether to trust a certificate the session host presented.
    fn ask_certificate(self: &Rc<Self>, prompt: &CertificatePrompt) {
        self.title.set_subtitle("Verifying the certificate…");
        let host = if prompt.port == 3389 || prompt.port == 0 {
            prompt.host.clone()
        } else {
            format!("{}:{}", prompt.host, prompt.port)
        };
        let mut details = vec![
            ("Name", prompt.common_name.as_str()),
            ("Subject", prompt.subject.as_str()),
            ("Issuer", prompt.issuer.as_str()),
            ("Fingerprint", prompt.fingerprint.as_str()),
        ];
        if let Some(previous) = &prompt.previous_fingerprint {
            details.push(("Previous fingerprint", previous.as_str()));
        }
        let details: String = details
            .iter()
            .filter(|(_, value)| !value.is_empty())
            .map(|(label, value)| format!("\n<b>{label}:</b> {}", glib::markup_escape_text(value)))
            .collect();
        let (heading, intro) = if prompt.changed {
            (
                "Certificate Changed",
                format!(
                    "{} presented a different certificate than before. This can mean someone \
                     is intercepting the connection. Only continue if you know why it changed.",
                    glib::markup_escape_text(&host)
                ),
            )
        } else {
            (
                "Unknown Certificate",
                format!(
                    "The identity of {} cannot be verified. Check that the fingerprint \
                     matches the one your administrator gave you.",
                    glib::markup_escape_text(&host)
                ),
            )
        };
        let dialog = adw::AlertDialog::builder()
            .heading(heading)
            .body(format!("{intro}\n{details}"))
            .body_use_markup(true)
            .default_response("reject")
            .close_response("reject")
            .build();
        dialog.add_responses(&[
            ("reject", "_Cancel"),
            ("once", "Connect _Once"),
            ("always", "_Always Trust"),
        ]);
        let appearance = if prompt.changed {
            adw::ResponseAppearance::Destructive
        } else {
            adw::ResponseAppearance::Suggested
        };
        dialog.set_response_appearance("always", appearance);
        let weak = Rc::downgrade(self);
        dialog.choose(
            Some(&self.window),
            gio::Cancellable::NONE,
            move |response| {
                let Some(this) = weak.upgrade() else { return };
                let decision = match response.as_str() {
                    "once" => CertificateDecision::AcceptOnce,
                    "always" => CertificateDecision::AcceptAlways,
                    _ => CertificateDecision::Reject,
                };
                if let Some(bridge) = this.bridge() {
                    bridge.answer_certificate(decision);
                }
            },
        );
    }

    fn show_network(&self, stats: NetworkStats) {
        self.diagnostics.borrow_mut().update_network(stats);
        if self.hud_visible.get() {
            self.update_hud();
        }
        let icon = match stats.quality() {
            ConnectionQuality::Excellent => "network-cellular-signal-excellent-symbolic",
            ConnectionQuality::Good => "network-cellular-signal-good-symbolic",
            ConnectionQuality::Fair => "network-cellular-signal-ok-symbolic",
            ConnectionQuality::Poor => "network-cellular-signal-weak-symbolic",
            ConnectionQuality::Unknown => "network-cellular-signal-none-symbolic",
        };
        let tooltip = format!("Connection: {}", stats.summary());
        for image in &self.quality_icons {
            image.set_icon_name(Some(icon));
            image.set_tooltip_text(Some(&tooltip));
            image.update_property(&[gtk4::accessible::Property::Label(&tooltip)]);
            image.set_visible(true);
        }
    }

    fn handle_status(self: &Rc<Self>, status: SessionStatus) {
        match status {
            SessionStatus::Progress(message) => {
                self.title.set_subtitle(&message);
                self.loading_label.set_text(&message);
            }
            SessionStatus::Connected => {
                self.title.set_subtitle("Connected");
                for image in &self.quality_icons {
                    image.set_visible(true);
                }
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
    fn show_ended(&self, message: &str) {
        let message = if message.is_empty() {
            "Disconnected"
        } else {
            message
        };
        self.title.set_subtitle(message);
        self.close_screens();
        for image in &self.quality_icons {
            image.set_visible(false);
        }
        if self.window.is_fullscreen() {
            self.window.unfullscreen();
        }
        let page = adw::StatusPage::builder()
            .icon_name("network-offline-symbolic")
            .title("Session Ended")
            .description(glib::markup_escape_text(message).as_str())
            .build();
        let close = Button::builder()
            .label("Close")
            .halign(gtk4::Align::Center)
            .build();
        close.add_css_class("pill");
        let window = self.window.downgrade();
        close.connect_clicked(move |_| {
            if let Some(window) = window.upgrade() {
                window.close();
            }
        });
        page.set_child(Some(&close));
        if let Some(content) = self.window.content().and_downcast::<adw::ToolbarView>() {
            content.set_content(Some(&page));
        }
        // Disconnect in the background; no further events are needed.
        self.bridge.borrow_mut().take();
    }

    fn install_window_chrome(
        self: &Rc<Self>,
        header: &adw::HeaderBar,
        fullscreen_button: &Button,
        disconnect_button: &Button,
        dock: &Rc<FullscreenDock>,
    ) {
        let window = &self.window;

        // Double-click on the header or title toggles maximize.
        for widget in [header.upcast_ref::<gtk4::Widget>(), self.title.upcast_ref()] {
            let gesture = GestureClick::new();
            let win = window.downgrade();
            gesture.connect_released(move |_, n_press, _, _| {
                if let (2, Some(win)) = (n_press, win.upgrade()) {
                    if win.is_maximized() {
                        win.unmaximize();
                    } else {
                        win.maximize();
                    }
                }
            });
            widget.add_controller(gesture);
        }

        // Fullscreen chrome follows the real window state, so it stays correct when the
        // compositor leaves fullscreen on its own.
        let header_weak = header.downgrade();
        let button_weak = fullscreen_button.downgrade();
        let dock_fs = Rc::downgrade(dock);
        let weak = Rc::downgrade(self);
        window.connect_fullscreened_notify(move |win| {
            let fullscreen = win.is_fullscreen();
            if let Some(this) = weak.upgrade() {
                // Spanning monitors starts and ends with fullscreen.
                if this.multi_monitor {
                    this.schedule_resize();
                }
                this.save_placement();
            }
            if let Some(header) = header_weak.upgrade() {
                header.set_visible(!fullscreen);
            }
            if let Some(button) = button_weak.upgrade() {
                button.set_icon_name(if fullscreen {
                    "view-restore-symbolic"
                } else {
                    "view-fullscreen-symbolic"
                });
            }
            if let Some(dock) = dock_fs.upgrade() {
                dock.set_fullscreen(fullscreen);
            }
        });

        for button in [fullscreen_button, &dock.restore_button] {
            let win = window.downgrade();
            button.connect_clicked(move |_| {
                if let Some(win) = win.upgrade() {
                    toggle_fullscreen(&win);
                }
            });
        }
        let win = window.downgrade();
        dock.minimize_button.connect_clicked(move |_| {
            if let Some(win) = win.upgrade() {
                win.minimize();
            }
        });
        for button in [disconnect_button, &dock.disconnect_button] {
            let win = window.downgrade();
            button.connect_clicked(move |_| {
                if let Some(win) = win.upgrade() {
                    win.close();
                }
            });
        }

        // Every close path goes through `close()`, so the session (which owns the window)
        // stays alive until the close request and is released there.
        let keep_alive = RefCell::new(Some(Rc::clone(self)));
        let dock = Rc::clone(dock);
        window.connect_close_request(move |_| {
            let _ = &dock;
            if let Some(this) = keep_alive.take() {
                this.shutdown();
            }
            glib::Propagation::Proceed
        });

        let weak = Rc::downgrade(self);
        self.canvas.connect_resized(move |_, _| {
            if let Some(this) = weak.upgrade() {
                this.schedule_resize();
            }
        });
    }

    /// Disconnects and notifies the owner. Safe to call more than once.
    fn shutdown(&self) {
        self.save_placement();
        if let Some(id) = self.resize_timer.take() {
            id.remove();
        }
        self.release_input();
        self.close_screens();
        self.bridge.borrow_mut().take();
        self.session.borrow_mut().take();
        // Pasting copies the files elsewhere, so the downloads are no longer needed; files
        // still on the clipboard can no longer be fetched anyway.
        let local = self.window.clipboard();
        if clipboard::is_own_files(&local, &self.id) {
            local.set_content(None::<&gtk4::gdk::ContentProvider>).ok();
        }
        if let Some(dir) = self.clipboard_dir.take() {
            if let Err(e) = std::fs::remove_dir_all(&dir) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    log::warn!("Could not remove {}: {e}", dir.display());
                }
            }
        }
        if let Some(on_close) = self.on_close.borrow_mut().take() {
            on_close();
        }
        open_sessions::unregister(&self.window);
    }

    fn schedule_resize(self: &Rc<Self>) {
        if let Some(id) = self.resize_timer.take() {
            id.remove();
        }
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(RESIZE_DEBOUNCE, move || {
            if let Some(this) = weak.upgrade() {
                this.resize_timer.take();
                this.send_resize();
            }
        });
        *self.resize_timer.borrow_mut() = Some(id);
    }

    fn send_resize(self: &Rc<Self>) {
        if !self.dynamic_resolution {
            return;
        }
        let Some(bridge) = self.bridge() else { return };
        let device_scale = self.display_scale();
        let scale_percent = self.scale.get().resolve(device_scale);

        if let Some((monitors, rects)) = self.monitor_layout(device_scale) {
            log::debug!(
                "Requesting a {}-monitor layout at {scale_percent}%",
                rects.len()
            );
            bridge.request_layout(&rects, scale_percent);
            self.show_on_monitors(&monitors, &rects);
            return;
        }
        self.close_screens();

        let (width, height) = (self.canvas.width(), self.canvas.height());
        if width <= 0 || height <= 0 {
            return;
        }
        let to_device = |v: i32| (f64::from(v) * device_scale).round() as u32;
        let (w, h) = (to_device(width), to_device(height));
        log::debug!("Requesting remote resize to {w}x{h} at {scale_percent}%");
        bridge.request_resize(w, h, scale_percent);
    }

    /// The local monitors as a remote layout when the session should span them: multi-
    /// monitor enabled, fullscreen, and more than one monitor. The monitor showing this
    /// window becomes the remote primary. All monitors use this window's scale.
    fn monitor_layout(&self, device_scale: f64) -> Option<(Vec<gdk::Monitor>, Vec<MonitorRect>)> {
        if !self.multi_monitor || !self.window.is_fullscreen() {
            return None;
        }
        let display = WidgetExt::display(&self.window);
        let monitors: Vec<gdk::Monitor> = display
            .monitors()
            .iter::<gdk::Monitor>()
            .filter_map(Result::ok)
            .take(MAX_MONITORS)
            .collect();
        if monitors.len() < 2 {
            return None;
        }
        let current = self
            .window
            .surface()
            .and_then(|surface| display.monitor_at_surface(&surface))?;
        let to_device = |v: i32| (f64::from(v) * device_scale).round() as i32;
        let rects = monitors
            .iter()
            .map(|monitor| {
                let geometry = monitor.geometry();
                MonitorRect {
                    x: to_device(geometry.x()),
                    y: to_device(geometry.y()),
                    width: to_device(geometry.width()).max(0) as u32,
                    height: to_device(geometry.height()).max(0) as u32,
                    primary: *monitor == current,
                }
            })
            .collect::<Vec<_>>();
        rects.iter().any(|r| r.primary).then_some((monitors, rects))
    }

    /// Shows each monitor's part of the remote desktop: this window on the primary and a
    /// borderless fullscreen window on every other monitor.
    fn show_on_monitors(self: &Rc<Self>, monitors: &[gdk::Monitor], rects: &[MonitorRect]) {
        let (viewports, _) = MonitorRect::viewports(rects);
        let mut secondary = Vec::new();
        for ((monitor, rect), viewport) in monitors.iter().zip(rects).zip(viewports) {
            if rect.primary {
                self.canvas.set_viewport(Some(viewport));
            } else {
                secondary.push((monitor.clone(), viewport));
            }
        }

        let same_monitors = {
            let screens = self.screens.borrow();
            screens.len() == secondary.len()
                && screens
                    .iter()
                    .zip(&secondary)
                    .all(|(screen, (monitor, _))| screen.monitor == *monitor)
        };
        if !same_monitors {
            self.close_screens();
            let screens = secondary
                .iter()
                .map(|(monitor, _)| self.open_screen(monitor))
                .collect();
            *self.screens.borrow_mut() = screens;
        }
        for (screen, (_, viewport)) in self.screens.borrow().iter().zip(&secondary) {
            screen.canvas.set_viewport(Some(*viewport));
        }
    }

    fn open_screen(self: &Rc<Self>, monitor: &gdk::Monitor) -> SecondaryScreen {
        let canvas = RdpCanvas::secondary();
        let window = gtk4::Window::builder()
            .title(self.window.title().unwrap_or_default())
            .decorated(false)
            .child(&canvas)
            .build();
        if let Some(texture) = self.texture.borrow().as_ref() {
            canvas.set_texture(texture);
        }
        self.attach_pointer_input(&canvas);
        window.add_controller(self.key_controller());
        self.install_focus_handling(&window);
        // Closed from outside (we destroy our own): stop spanning monitors.
        let weak = Rc::downgrade(self);
        window.connect_close_request(move |_| {
            if let Some(this) = weak.upgrade() {
                this.window.unfullscreen();
            }
            glib::Propagation::Proceed
        });
        window.fullscreen_on_monitor(monitor);
        window.present();
        SecondaryScreen {
            monitor: monitor.clone(),
            window,
            canvas,
        }
    }

    /// Returns to a single-monitor view.
    fn close_screens(&self) {
        self.canvas.set_viewport(None);
        for screen in self.screens.take() {
            screen.window.destroy();
        }
    }

    /// Re-applies the layout when monitors are added, removed or rearranged.
    /// Synchronizes remote frame presentation with the display refresh rate (VSync),
    /// providing triple buffering so intermediate strips/tiles are never drawn progressively.
    fn install_frame_renderer(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.canvas.add_tick_callback(move |_, _| {
            let Some(this) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if this.frame_dirty.replace(false) {
                let texture = this
                    .bridge()
                    .and_then(|bridge| bridge.with_frame(RdpCanvas::texture_for));
                if let Some(texture) = texture {
                    this.canvas.set_texture(&texture);
                    for screen in this.screens.borrow().iter() {
                        screen.canvas.set_texture(&texture);
                    }
                    *this.texture.borrow_mut() = Some(texture);
                }
            }
            glib::ControlFlow::Continue
        });
    }

    fn install_monitor_tracking(self: &Rc<Self>) {
        if !self.multi_monitor {
            return;
        }
        let weak = Rc::downgrade(self);
        WidgetExt::display(&self.window)
            .monitors()
            .connect_items_changed(move |_, _, _, _| {
                if let Some(this) = weak.upgrade() {
                    this.schedule_resize();
                }
            });
    }

    /// Registers `win.scale` (radio items, state = [`ScaleChoice::id`]) and
    /// `win.scale-custom` for the scale menus.
    fn install_scale_actions(self: &Rc<Self>) {
        let group = gio::SimpleActionGroup::new();

        let weak = Rc::downgrade(self);
        self.scale_action.connect_change_state(move |_, value| {
            let choice = value
                .and_then(glib::Variant::str)
                .and_then(ScaleChoice::from_id);
            if let (Some(this), Some(choice)) = (weak.upgrade(), choice) {
                this.set_scale(choice);
            }
        });
        group.add_action(&self.scale_action);

        let custom = gio::SimpleAction::new("scale-custom", None);
        let weak = Rc::downgrade(self);
        custom.connect_activate(move |_, _| {
            if let Some(this) = weak.upgrade() {
                this.ask_custom_scale();
            }
        });
        group.add_action(&custom);

        self.window.insert_action_group("win", Some(&group));
        self.update_scale_ui();
    }

    /// Applies `choice` to the running session right away.
    fn set_scale(self: &Rc<Self>, choice: ScaleChoice) {
        self.scale.set(choice);
        self.update_scale_ui();
        self.send_resize();
    }

    fn display_scale(&self) -> f64 {
        self.window
            .surface()
            .map_or_else(|| f64::from(self.window.scale_factor()), |s| s.scale())
    }

    fn update_scale_ui(&self) {
        let choice = self.scale.get();
        self.scale_action.set_state(&choice.id().to_variant());

        self.custom_scale_section.remove_all();
        if choice.is_custom() {
            let percent = choice.resolve(1.0);
            self.custom_scale_section
                .append_item(&scale_item(&format!("{percent}% (Custom)"), choice));
        }
        self.custom_scale_section
            .append(Some("Custom…"), Some("win.scale-custom"));

        let tooltip = match choice {
            ScaleChoice::Auto => "Display Scale: Match Display".to_string(),
            ScaleChoice::Percent(percent) => format!("Display Scale: {percent}%"),
        };
        for button in &self.scale_buttons {
            button.set_tooltip_text(Some(&tooltip));
        }
    }

    fn ask_custom_scale(self: &Rc<Self>) {
        let current = self.scale.get().resolve(self.display_scale());
        let spin = gtk4::SpinButton::with_range(
            f64::from(crate::session::MIN_SCALE),
            f64::from(ScaleChoice::MAX_PERCENT),
            5.0,
        );
        spin.set_value(f64::from(current.min(ScaleChoice::MAX_PERCENT)));
        spin.set_halign(gtk4::Align::Center);
        spin.set_activates_default(true);
        let dialog = adw::AlertDialog::builder()
            .heading("Custom Display Scale")
            .body(format!(
                "Scale of the remote desktop in percent, from {}% to {}%. Applies to this \
                 session only.",
                crate::session::MIN_SCALE,
                ScaleChoice::MAX_PERCENT
            ))
            .extra_child(&spin)
            .default_response("apply")
            .close_response("cancel")
            .build();
        dialog.add_responses(&[("cancel", "Cancel"), ("apply", "Apply")]);
        dialog.set_response_appearance("apply", adw::ResponseAppearance::Suggested);
        let weak = Rc::downgrade(self);
        dialog.connect_response(Some("apply"), move |_, _| {
            spin.update();
            if let Some(this) = weak.upgrade() {
                this.set_scale(ScaleChoice::percent(spin.value_as_int().max(0) as u32));
            }
        });
        dialog.present(Some(&self.window));
    }

    /// Whether keyboard input belongs to the window's own UI (an open menu or dialog)
    /// rather than the remote session.
    fn local_ui_has_keyboard(&self) -> bool {
        self.window.visible_dialog().is_some()
            || self
                .scale_buttons
                .iter()
                .chain(&self.session_buttons)
                .any(|b| b.is_active())
    }

    /// Mouse input on `canvas` (the main one or a secondary monitor's).
    fn attach_pointer_input(self: &Rc<Self>, canvas: &RdpCanvas) {
        let motion = EventControllerMotion::new();
        let weak = Rc::downgrade(self);
        let canvas_weak = canvas.downgrade();
        motion.connect_motion(move |_, x, y| {
            let (Some(this), Some(canvas)) = (weak.upgrade(), canvas_weak.upgrade()) else {
                return;
            };
            if this.view_only.get() {
                return;
            }
            if let Some(bridge) = this.bridge() {
                let (rx, ry) = canvas.to_remote(x, y, bridge.frame_size());
                this.pointer.set((rx, ry));
                bridge.send_pointer_move(rx, ry);
            }
        });
        canvas.add_controller(motion);

        // Raw button events: unlike click gestures they report every button independently,
        // so chords and drags with several buttons reach the session unchanged.
        let buttons = EventControllerLegacy::new();
        let weak = Rc::downgrade(self);
        let canvas_weak = canvas.downgrade();
        buttons.connect_event(move |_, event| {
            let Some(this) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let down = match event.event_type() {
                gdk::EventType::ButtonPress => true,
                gdk::EventType::ButtonRelease => false,
                _ => return glib::Propagation::Proceed,
            };
            let Some(button) = event
                .downcast_ref::<gdk::ButtonEvent>()
                .and_then(|e| MouseButton::from_button_number(e.button()))
            else {
                return glib::Propagation::Proceed;
            };
            if let (true, Some(canvas)) = (down, canvas_weak.upgrade()) {
                canvas.grab_focus();
            }
            if down && this.view_only.get() {
                return glib::Propagation::Stop;
            }
            this.send_button(button, down);
            glib::Propagation::Stop
        });
        canvas.add_controller(buttons);

        let scroll = EventControllerScroll::new(EventControllerScrollFlags::BOTH_AXES);
        let weak = Rc::downgrade(self);
        scroll.connect_scroll(move |controller, dx, dy| {
            let Some(this) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if this.view_only.get() {
                return glib::Propagation::Stop;
            }
            let (dx, dy) = if controller.unit() == gdk::ScrollUnit::Surface {
                (dx / SURFACE_PX_PER_NOTCH, dy / SURFACE_PX_PER_NOTCH)
            } else {
                (dx, dy)
            };
            let (horizontal, vertical) = this.wheel.borrow_mut().push(dx, dy);
            if let Some(bridge) = this.bridge() {
                let (x, y) = this.pointer.get();
                if vertical != 0 {
                    bridge.send_wheel(vertical, false, x, y);
                }
                if horizontal != 0 {
                    bridge.send_wheel(horizontal, true, x, y);
                }
            }
            glib::Propagation::Stop
        });
        canvas.add_controller(scroll);
    }

    /// Keyboard input for a session window. Capture phase on the window: every key goes to
    /// the session, including Tab, F10 and Alt mnemonics that GTK would otherwise use for
    /// its own navigation.
    fn key_controller(self: &Rc<Self>) -> EventControllerKey {
        let keys = EventControllerKey::new();
        keys.set_propagation_phase(gtk4::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, keyval, keycode, state| {
            let Some(this) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if this.local_ui_has_keyboard() {
                return glib::Propagation::Proceed;
            }
            let ctrl_shift = ModifierType::CONTROL_MASK | ModifierType::SHIFT_MASK;
            if state.contains(ctrl_shift) && matches!(keyval, Key::d | Key::D) {
                this.toggle_hud();
                return glib::Propagation::Stop;
            }
            let ctrl_alt = ModifierType::CONTROL_MASK | ModifierType::ALT_MASK;
            if state.contains(ctrl_alt) && matches!(keyval, Key::Return | Key::KP_Enter) {
                toggle_fullscreen(&this.window);
                return glib::Propagation::Stop;
            }
            this.send_key(keycode, true)
        });
        let weak = Rc::downgrade(self);
        keys.connect_key_released(move |_, _, keycode, _| {
            if let Some(this) = weak.upgrade() {
                // Releases still go out so keys pressed before a menu opened don't stick.
                this.send_key(keycode, false);
            }
        });
        keys
    }

    fn send_button(&self, button: MouseButton, down: bool) {
        {
            let mut held = self.held_buttons.borrow_mut();
            if down {
                if !held.contains(&button) {
                    held.push(button);
                }
            } else if let Some(i) = held.iter().position(|b| *b == button) {
                held.remove(i);
            } else {
                // Release without a press we sent (e.g. press landed on another widget).
                return;
            }
        }
        if let Some(bridge) = self.bridge() {
            let (x, y) = self.pointer.get();
            bridge.send_button(button, down, x, y);
        }
    }

    fn send_key(&self, keycode: u32, down: bool) -> glib::Propagation {
        let Some(scancode) = rdp_scancode_from_xkb(keycode) else {
            log::debug!("No RDP scancode for key code {keycode}");
            return glib::Propagation::Proceed;
        };
        if down && self.view_only.get() {
            return glib::Propagation::Stop;
        }
        let repeat = self.pressed_keys.borrow_mut().update(scancode, down);
        if let Some(bridge) = self.bridge() {
            bridge.send_key(scancode, down, repeat);
        }
        glib::Propagation::Stop
    }

    /// Releases everything held in the session (keys, buttons, partial wheel steps).
    fn release_input(&self) {
        let keys = self.pressed_keys.borrow_mut().take_all();
        let buttons = std::mem::take(&mut *self.held_buttons.borrow_mut());
        self.wheel.borrow_mut().reset();
        let Some(bridge) = self.bridge() else { return };
        for scancode in keys {
            bridge.send_key(scancode, false, false);
        }
        let (x, y) = self.pointer.get();
        for button in buttons {
            bridge.send_button(button, false, x, y);
        }
    }

    /// Focus handling for a window showing the session (main or secondary monitor).
    fn install_focus_handling(self: &Rc<Self>, window: &gtk4::Window) {
        let weak = Rc::downgrade(self);
        window.connect_realize(move |win| {
            let capture = weak
                .upgrade()
                .is_some_and(|this| this.capture_shortcuts.get());
            set_shortcuts_inhibited(win, capture);
        });

        let weak = Rc::downgrade(self);
        window.connect_is_active_notify(move |win| {
            let Some(this) = weak.upgrade() else { return };
            let active = win.is_active();
            set_shortcuts_inhibited(win, active && this.capture_shortcuts.get());
            if active {
                if let Some(bridge) = this.bridge() {
                    bridge.send_focus_in(this.lock_keys());
                }
                this.sync_local_clipboard();
                let weak_retry = Rc::downgrade(&this);
                glib::timeout_add_local_once(std::time::Duration::from_millis(150), move || {
                    if let Some(this) = weak_retry.upgrade() {
                        this.sync_local_clipboard();
                    }
                });
            } else {
                // Otherwise e.g. Alt stays held remotely after Alt+Tab.
                this.release_input();
            }
        });
    }

    /// Registers the `session.*` actions behind the session menus.
    fn install_session_actions(self: &Rc<Self>) {
        let group = gio::SimpleActionGroup::new();

        let send_keys = gio::SimpleAction::new("send-keys", Some(glib::VariantTy::STRING));
        let weak = Rc::downgrade(self);
        send_keys.connect_activate(move |_, value| {
            let combo = value
                .and_then(glib::Variant::str)
                .and_then(KeyCombo::from_id);
            if let (Some(this), Some(combo)) = (weak.upgrade(), combo) {
                this.send_inputs(&combo.events());
            }
        });
        group.add_action(&send_keys);

        let capture = gio::SimpleAction::new_stateful(
            "capture-shortcuts",
            None,
            &self.capture_shortcuts.get().to_variant(),
        );
        let weak = Rc::downgrade(self);
        capture.connect_change_state(move |action, value| {
            let (Some(this), Some(enabled)) = (weak.upgrade(), value.and_then(|v| v.get::<bool>()))
            else {
                return;
            };
            action.set_state(&enabled.to_variant());
            this.capture_shortcuts.set(enabled);
            let mut windows = vec![this.window.clone().upcast::<gtk4::Window>()];
            windows.extend(this.screens.borrow().iter().map(|s| s.window.clone()));
            for window in windows {
                set_shortcuts_inhibited(&window, enabled && window.is_active());
            }
        });
        group.add_action(&capture);

        let toggle_hud = gio::SimpleAction::new("toggle-hud", None);
        let weak = Rc::downgrade(self);
        toggle_hud.connect_activate(move |_, _| {
            if let Some(this) = weak.upgrade() {
                this.toggle_hud();
            }
        });
        group.add_action(&toggle_hud);

        let view_only = gio::SimpleAction::new_stateful("view-only", None, &false.to_variant());
        let weak = Rc::downgrade(self);
        view_only.connect_change_state(move |action, value| {
            let (Some(this), Some(enabled)) = (weak.upgrade(), value.and_then(|v| v.get::<bool>()))
            else {
                return;
            };
            action.set_state(&enabled.to_variant());
            this.view_only.set(enabled);
            if enabled {
                this.release_input();
            }
            this.title
                .set_subtitle(if enabled { "View Only" } else { "Connected" });
        });
        group.add_action(&view_only);

        let screenshot = gio::SimpleAction::new("screenshot", None);
        let weak = Rc::downgrade(self);
        screenshot.connect_activate(move |_, _| {
            if let Some(this) = weak.upgrade() {
                this.save_screenshot();
            }
        });
        group.add_action(&screenshot);

        let type_clipboard = gio::SimpleAction::new("type-clipboard", None);
        let weak = Rc::downgrade(self);
        type_clipboard.connect_activate(move |_, _| {
            if let Some(this) = weak.upgrade() {
                this.type_clipboard();
            }
        });
        group.add_action(&type_clipboard);

        self.window.insert_action_group("session", Some(&group));
    }

    /// Sends key events as they are, after releasing whatever is held locally so
    /// modifiers don't combine with them.
    fn send_inputs(&self, inputs: &[KeyInput]) {
        if self.view_only.get() {
            self.toast("Input is off while the session is view only.");
            return;
        }
        self.release_input();
        let Some(bridge) = self.bridge() else { return };
        for input in inputs {
            match *input {
                KeyInput::Scancode { code, down } => bridge.send_key(code, down, false),
                KeyInput::Unicode { unit, down } => bridge.send_unicode(unit, down),
            }
        }
    }

    /// Types the local clipboard text into the session, for places where pasting does
    /// not work (sign-in screens, consoles without clipboard support).
    fn type_clipboard(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let clipboard = self.window.clipboard();
        clipboard.read_text_async(gio::Cancellable::NONE, move |result| {
            let Some(this) = weak.upgrade() else { return };
            match result {
                Ok(Some(text)) if !text.is_empty() => this.do_type_clipboard(&text),
                _ => {
                    let weak2 = Rc::downgrade(&this);
                    WidgetExt::display(&this.window)
                        .primary_clipboard()
                        .read_text_async(gio::Cancellable::NONE, move |res| {
                            let Some(this) = weak2.upgrade() else { return };
                            match res {
                                Ok(Some(text)) if !text.is_empty() => this.do_type_clipboard(&text),
                                _ => this.toast("The clipboard holds no text."),
                            }
                        });
                }
            }
        });
    }

    fn do_type_clipboard(&self, text: &str) {
        match keys::type_text(text) {
            Some(inputs) => self.send_inputs(&inputs),
            None => self.toast(&format!(
                "The clipboard text is too long to type (over {} characters).",
                keys::MAX_TYPED_CHARS
            )),
        }
    }

    /// Saves the current remote screen as a PNG in the Pictures folder.
    fn save_screenshot(&self) {
        let Some(texture) = self.texture.borrow().clone() else {
            self.toast("Nothing to capture yet.");
            return;
        };
        let pictures =
            glib::user_special_dir(glib::UserDirectory::Pictures).unwrap_or_else(glib::home_dir);
        let folder = Some(pictures.join("Screenshots"))
            .filter(|dir| dir.is_dir())
            .unwrap_or(pictures);
        let stamp = chrono::Local::now().format("%Y-%m-%d %H-%M-%S");
        let name: String = self
            .name
            .chars()
            .map(|c| if c == '/' || c.is_control() { '_' } else { c })
            .collect();
        let path = folder.join(format!("{} {stamp}.png", name.trim_start_matches('.')));
        match texture.save_to_png(&path) {
            Ok(()) => self.toast(&format!("Screenshot saved to {}", path.display())),
            Err(e) => {
                log::warn!("Could not save the screenshot to {}: {e}", path.display());
                self.toast(&format!("Could not save the screenshot: {e}"));
            }
        }
    }

    fn toast(&self, message: &str) {
        self.toasts.add_toast(adw::Toast::new(message));
    }

    fn install_clipboard_sync(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.window.clipboard().connect_changed(move |_| {
            if let Some(this) = weak.upgrade() {
                this.sync_local_clipboard();
            }
        });
    }

    fn toggle_hud(&self) {
        let visible = !self.hud_visible.get();
        self.hud_visible.set(visible);
        self.hud.set_visible(visible);
        if visible {
            self.update_hud();
        }
    }

    fn update_hud(&self) {
        let (w, h) = self.bridge().map_or((0, 0), |b| b.frame_size());
        let scale = match self.scale.get() {
            ScaleChoice::Auto => 100,
            ScaleChoice::Percent(p) => p,
        };
        let mut diag = self.diagnostics.borrow_mut();
        diag.set_display(w, h, scale);
        let markup = diag.format_pango_hud();
        self.hud_label.set_markup(&markup);
    }

    fn lock_keys(&self) -> LockKeys {
        let keyboard = WidgetExt::display(&self.window)
            .default_seat()
            .and_then(|s| s.keyboard());
        keyboard.map_or_else(LockKeys::default, |k| LockKeys {
            scroll_lock: k.is_scroll_locked(),
            num_lock: k.is_num_locked(),
            caps_lock: k.is_caps_locked(),
        })
    }

    /// Offers the local clipboard (files, image or text) to the session. The bridge skips
    /// unchanged content, including what the session itself just copied.
    fn sync_local_clipboard(&self) {
        let Some(bridge) = self.bridge() else { return };
        let local = self.window.clipboard();
        // Files this session offered itself would be downloaded and sent straight back.
        if clipboard::is_own_files(&local, &self.id) {
            return;
        }
        let bridge = Rc::downgrade(&bridge);
        glib::MainContext::default().spawn_local(async move {
            let content = clipboard::read_local(&local).await;
            if let (Some(content), Some(bridge)) = (content, bridge.upgrade()) {
                bridge.set_local_clipboard(content);
            }
        });
    }
}

/// Delivers the Super key and other system shortcuts to the session while focused.
fn set_shortcuts_inhibited(window: &gtk4::Window, inhibit: bool) {
    let Some(toplevel) = window.surface().and_downcast::<gdk::Toplevel>() else {
        return;
    };
    if inhibit {
        toplevel.inhibit_system_shortcuts(None::<&gdk::Event>);
    } else {
        toplevel.restore_system_shortcuts();
    }
}

/// Connection quality indicator; hidden until the session is connected.
pub(super) fn quality_icon() -> gtk4::Image {
    let image = gtk4::Image::from_icon_name("network-cellular-signal-none-symbolic");
    image.set_tooltip_text(Some("Connection: Measuring"));
    image.set_visible(false);
    image
}

fn toggle_fullscreen(window: &adw::Window) {
    if window.is_fullscreen() {
        window.unfullscreen();
    } else {
        window.fullscreen();
    }
}

/// The scale menu shared by the header and the fullscreen dock, plus its custom section.
fn scale_menu() -> (gio::Menu, gio::Menu) {
    let presets = gio::Menu::new();
    presets.append_item(&scale_item("Match Display", ScaleChoice::Auto));
    for percent in ScaleChoice::PRESETS {
        presets.append_item(&scale_item(
            &format!("{percent}%"),
            ScaleChoice::Percent(percent),
        ));
    }
    let custom = gio::Menu::new();
    let menu = gio::Menu::new();
    menu.append_section(Some("Display Scale"), &presets);
    menu.append_section(None, &custom);
    (menu, custom)
}

/// Keys, clipboard typing, screenshot and input toggles (`session.*` actions).
fn session_menu() -> gio::Menu {
    let keys = gio::Menu::new();
    for combo in KeyCombo::ALL {
        let item = gio::MenuItem::new(Some(combo.label()), None);
        item.set_action_and_target_value(Some("session.send-keys"), Some(&combo.id().to_variant()));
        keys.append_item(&item);
    }
    let tools = gio::Menu::new();
    tools.append(Some("Type Clipboard Text"), Some("session.type-clipboard"));
    tools.append(Some("Save Screenshot"), Some("session.screenshot"));
    let toggles = gio::Menu::new();
    toggles.append(
        Some("Performance HUD (Ctrl+Shift+D)"),
        Some("session.toggle-hud"),
    );
    toggles.append(
        Some("Capture System Shortcuts"),
        Some("session.capture-shortcuts"),
    );
    toggles.append(Some("View Only"), Some("session.view-only"));
    let menu = gio::Menu::new();
    menu.append_section(Some("Send Keys"), &keys);
    menu.append_section(None, &tools);
    menu.append_section(None, &toggles);
    menu
}

fn scale_item(label: &str, choice: ScaleChoice) -> gio::MenuItem {
    let item = gio::MenuItem::new(Some(label), None);
    item.set_action_and_target_value(Some("win.scale"), Some(&choice.id().to_variant()));
    item
}

thread_local! {
    static HUD_CSS_INSTALLED: Cell<bool> = const { Cell::new(false) };
}

fn install_hud_css() {
    if HUD_CSS_INSTALLED.replace(true) {
        return;
    }
    let Some(display) = gtk4::gdk::Display::default() else {
        return;
    };
    let provider = gtk4::CssProvider::new();
    provider.load_from_string(
        r#"
        .performance-hud {
            background-color: rgba(15, 23, 42, 0.94);
            color: #f8fafc;
            border: 1px solid rgba(148, 163, 184, 0.35);
            border-radius: 10px;
            padding: 12px 16px;
            box-shadow: 0 8px 24px rgba(0, 0, 0, 0.85);
        }
        .hud-title {
            font-weight: bold;
            font-size: 13px;
            color: #f1f5f9;
            letter-spacing: 0.3px;
        }
        .hud-hint {
            font-size: 11px;
            color: #94a3b8;
            font-family: monospace;
        }
        .hud-sep {
            background-color: rgba(148, 163, 184, 0.25);
            min-height: 1px;
            margin-top: 4px;
            margin-bottom: 6px;
        }
        .hud-body {
            font-family: monospace;
            font-size: 12px;
            line-height: 1.45;
            color: #f8fafc;
        }
        "#,
    );
    gtk4::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn hud_card() -> (Box, gtk4::Label) {
    install_hud_css();

    let card = Box::new(Orientation::Vertical, 4);
    card.set_halign(gtk4::Align::End);
    card.set_valign(gtk4::Align::Start);
    card.set_margin_top(20);
    card.set_margin_end(20);
    card.add_css_class("performance-hud");
    card.set_can_target(false);
    card.set_visible(false);

    let header_box = Box::new(Orientation::Horizontal, 12);
    let title = gtk4::Label::new(Some("Performance HUD"));
    title.set_halign(gtk4::Align::Start);
    title.add_css_class("hud-title");
    let hint = gtk4::Label::new(Some("Ctrl+Shift+D"));
    hint.set_halign(gtk4::Align::End);
    hint.set_hexpand(true);
    hint.add_css_class("hud-hint");
    header_box.append(&title);
    header_box.append(&hint);
    card.append(&header_box);

    let sep = Separator::new(Orientation::Horizontal);
    sep.add_css_class("hud-sep");
    card.append(&sep);

    let label = gtk4::Label::new(None);
    label.set_halign(gtk4::Align::Start);
    label.set_use_markup(true);
    label.add_css_class("hud-body");
    card.append(&label);

    (card, label)
}

fn loading_card(name: &str) -> (Box, gtk4::Label) {
    let card = Box::new(Orientation::Vertical, 12);
    card.set_halign(gtk4::Align::Center);
    card.set_valign(gtk4::Align::Center);
    card.add_css_class("card");
    for set_margin in [
        Box::set_margin_start,
        Box::set_margin_end,
        Box::set_margin_top,
        Box::set_margin_bottom,
    ] {
        set_margin(&card, 24);
    }
    let spinner = Spinner::new();
    spinner.set_size_request(40, 40);
    spinner.start();
    card.append(&spinner);
    let title = gtk4::Label::new(Some(&format!("Connecting to {name}…")));
    title.add_css_class("title-4");
    title.set_wrap(true);
    title.set_justify(gtk4::Justification::Center);
    card.append(&title);
    let detail = gtk4::Label::new(Some("Starting session"));
    detail.add_css_class("dim-label");
    detail.set_wrap(true);
    card.append(&detail);
    (card, detail)
}
