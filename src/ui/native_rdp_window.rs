//! Native In-App Remote Desktop Session Window powered by FreeRDP 3 C-FFI.

use crate::models::CloudPc;
use crate::rdp::bridge::*;
use crate::ui::rdp_auth_dialog::RdpAuthDialog;
use gtk4::cairo;
use gtk4::gdk::prelude::*;
use gtk4::gdk::{Key, ModifierType};
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Box, Button, DrawingArea, EventControllerKey, EventControllerMotion, EventControllerScroll,
    EventControllerScrollFlags, GestureClick, HeaderBar, Label, Orientation, Overlay, Revealer,
    RevealerTransitionType, Separator, Spinner, Window,
};
use log::{debug, info, warn};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct NativeRdpWindow;

fn map_coords(x: f64, y: f64, area_w: i32, area_h: i32, b: &NativeRdpBridge) -> (u16, u16) {
    let aw = area_w as f64;
    let ah = area_h as f64;
    if let Ok(fb) = b.frame_buffer.lock() {
        let fw = fb.width as f64;
        let fh = fb.height as f64;
        if fw > 0.0 && fh > 0.0 && aw > 0.0 && ah > 0.0 {
            let rx = (x * (fw / aw)).clamp(0.0, fw - 1.0) as u16;
            let ry = (y * (fh / ah)).clamp(0.0, fh - 1.0) as u16;
            return (rx, ry);
        }
    }
    (x.max(0.0) as u16, y.max(0.0) as u16)
}

impl NativeRdpWindow {
    /// Opens a native GTK 4 in-app session window rendering FreeRDP directly via C-FFI.
    pub fn open(
        _parent: Option<&impl IsA<Window>>,
        pc: &CloudPc,
        cli_args: Vec<String>,
        _desktop_scale: u32,
        on_close: impl Fn() + 'static,
    ) -> Rc<Self> {
        // Register custom CSS for fullscreen dock and hover trigger strip
        if let Some(display) = gtk4::gdk::Display::default() {
            let provider = gtk4::CssProvider::new();
            provider.load_from_string(
                ".fullscreen-dock { background-color: rgba(30, 30, 30, 0.90); border: 1px solid rgba(255, 255, 255, 0.20); border-radius: 12px; padding: 4px 12px; box-shadow: 0 4px 14px rgba(0,0,0,0.5); }\n\
                 .fullscreen-strip { background-color: rgba(255, 255, 255, 0.50); border-radius: 2px; }",
            );
            gtk4::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }

        // Independent top-level window (not transient) so GNOME Mutter enables resize handles
        let window = Window::builder()
            .title(&format!("{} - Windows App", pc.display_name))
            .default_width(1280)
            .default_height(800)
            .resizable(true)
            .build();

        // Header bar with explicit minimize, maximize, close buttons
        let header = HeaderBar::new();
        header.set_show_title_buttons(true);
        header.set_decoration_layout(Some(":minimize,maximize,close"));

        let win_hdr_dbl = window.clone();
        let hdr_dbl = GestureClick::new();
        hdr_dbl.connect_released(move |_gesture, n_press, _x, _y| {
            if n_press == 2 {
                if win_hdr_dbl.is_maximized() {
                    win_hdr_dbl.unmaximize();
                } else {
                    win_hdr_dbl.maximize();
                }
            }
        });
        header.add_controller(hdr_dbl);

        let disconnect_btn = Button::builder()
            .label("Disconnect")
            .icon_name("system-log-out-symbolic")
            .tooltip_text("Disconnect remote desktop session")
            .build();
        disconnect_btn.add_css_class("destructive-action");
        header.pack_start(&disconnect_btn);

        let title_box = Box::new(Orientation::Vertical, 0);
        title_box.set_valign(gtk4::Align::Center);

        let title_lbl = Label::new(Some(&pc.display_name));
        title_lbl.add_css_class("title-4");
        title_box.append(&title_lbl);

        let status_lbl = Label::new(Some("Connecting via Azure ARM Gateway..."));
        status_lbl.add_css_class("dim-label");
        title_box.append(&status_lbl);

        let win_tb_dbl = window.clone();
        let title_dbl = GestureClick::new();
        title_dbl.connect_released(move |_gesture, n_press, _x, _y| {
            if n_press == 2 {
                if win_tb_dbl.is_maximized() {
                    win_tb_dbl.unmaximize();
                } else {
                    win_tb_dbl.maximize();
                }
            }
        });
        title_box.add_controller(title_dbl);

        header.set_title_widget(Some(&title_box));

        let fullscreen_btn = Button::builder()
            .icon_name("view-fullscreen-symbolic")
            .tooltip_text("Toggle Fullscreen (Ctrl+Alt+Enter)")
            .build();
        fullscreen_btn.add_css_class("flat");
        header.pack_end(&fullscreen_btn);

        window.set_titlebar(Some(&header));

        // Drawing Area for RDP Framebuffer (fills exact inner window area)
        let drawing_area = DrawingArea::new();
        drawing_area.set_can_focus(true);
        drawing_area.set_focusable(true);
        drawing_area.set_focus_on_click(true);
        drawing_area.set_hexpand(true);
        drawing_area.set_vexpand(true);
        drawing_area.set_halign(gtk4::Align::Fill);
        drawing_area.set_valign(gtk4::Align::Fill);

        // Overlay with loading spinner and status feedback
        let overlay = Overlay::new();
        overlay.set_hexpand(true);
        overlay.set_vexpand(true);
        overlay.set_halign(gtk4::Align::Fill);
        overlay.set_valign(gtk4::Align::Fill);
        overlay.set_child(Some(&drawing_area));

        // Fullscreen floating top dock & hover trigger strip
        let hover_strip = Box::new(Orientation::Horizontal, 0);
        hover_strip.set_valign(gtk4::Align::Start);
        hover_strip.set_halign(gtk4::Align::Center);
        hover_strip.set_size_request(100, 6);
        hover_strip.add_css_class("fullscreen-strip");
        hover_strip.set_visible(false);

        let top_dock_revealer = Revealer::new();
        top_dock_revealer.set_transition_type(RevealerTransitionType::SlideDown);
        top_dock_revealer.set_transition_duration(200);
        top_dock_revealer.set_valign(gtk4::Align::Start);
        top_dock_revealer.set_halign(gtk4::Align::Center);
        top_dock_revealer.set_reveal_child(false);

        let dock_box = Box::new(Orientation::Horizontal, 8);
        dock_box.add_css_class("fullscreen-dock");
        dock_box.set_margin_top(4);

        let dock_title = Label::new(Some(&pc.display_name));
        dock_title.add_css_class("bold");
        dock_box.append(&dock_title);

        let dock_sep = Separator::new(Orientation::Vertical);
        dock_box.append(&dock_sep);

        let dock_restore_btn = Button::builder()
            .icon_name("view-restore-symbolic")
            .tooltip_text("Exit Fullscreen (Escape or Ctrl+Alt+Enter)")
            .build();
        dock_restore_btn.add_css_class("flat");
        dock_box.append(&dock_restore_btn);

        let dock_min_btn = Button::builder()
            .icon_name("window-minimize-symbolic")
            .tooltip_text("Minimize")
            .build();
        dock_min_btn.add_css_class("flat");
        let win_min = window.clone();
        dock_min_btn.connect_clicked(move |_| {
            win_min.minimize();
        });
        dock_box.append(&dock_min_btn);

        let dock_disconnect_btn = Button::builder()
            .icon_name("system-log-out-symbolic")
            .tooltip_text("Disconnect session")
            .build();
        dock_disconnect_btn.add_css_class("destructive-action");
        dock_box.append(&dock_disconnect_btn);

        top_dock_revealer.set_child(Some(&dock_box));

        let top_dock_container = Box::new(Orientation::Vertical, 0);
        top_dock_container.set_valign(gtk4::Align::Start);
        top_dock_container.set_halign(gtk4::Align::Center);
        top_dock_container.append(&hover_strip);
        top_dock_container.append(&top_dock_revealer);

        // Motion controller on the dock/strip container:
        // Reveals the dock ONLY when mouse enters/moves over this container (the white strip),
        // and hides the dock when the mouse leaves.
        let dock_motion_ctrl = EventControllerMotion::new();
        let revealer_de = top_dock_revealer.clone();
        let strip_de = hover_strip.clone();
        let win_de = window.clone();
        dock_motion_ctrl.connect_enter(move |_ctrl, _x, _y| {
            if win_de.is_fullscreen() {
                revealer_de.set_reveal_child(true);
                strip_de.set_opacity(0.0);
            }
        });

        let revealer_dm = top_dock_revealer.clone();
        let strip_dm = hover_strip.clone();
        let win_dm = window.clone();
        dock_motion_ctrl.connect_motion(move |_ctrl, _x, _y| {
            if win_dm.is_fullscreen() {
                revealer_dm.set_reveal_child(true);
                strip_dm.set_opacity(0.0);
            }
        });

        let revealer_dl = top_dock_revealer.clone();
        let strip_dl = hover_strip.clone();
        let win_dl = window.clone();
        dock_motion_ctrl.connect_leave(move |_ctrl| {
            if win_dl.is_fullscreen() {
                revealer_dl.set_reveal_child(false);
                strip_dl.set_opacity(1.0);
            }
        });
        top_dock_container.add_controller(dock_motion_ctrl);

        overlay.add_overlay(&top_dock_container);

        let loading_box = Box::new(Orientation::Vertical, 12);
        loading_box.set_halign(gtk4::Align::Center);
        loading_box.set_valign(gtk4::Align::Center);
        loading_box.add_css_class("card");
        loading_box.set_margin_start(24);
        loading_box.set_margin_end(24);
        loading_box.set_margin_top(24);
        loading_box.set_margin_bottom(24);

        let spinner = Spinner::new();
        spinner.set_size_request(40, 40);
        spinner.start();
        loading_box.append(&spinner);

        let load_msg = Label::new(Some("Establishing Cloud PC session..."));
        load_msg.add_css_class("title-4");
        loading_box.append(&load_msg);

        let load_sub = Label::new(Some("Authenticating with Entra ID & ARM Gateway"));
        load_sub.add_css_class("dim-label");
        loading_box.append(&load_sub);

        overlay.add_overlay(&loading_box);

        window.set_child(Some(&overlay));

        // Fullscreen toggle logic
        let win_fs = window.clone();
        let header_fs = header.clone();
        let fs_btn_icon = fullscreen_btn.clone();
        let strip_fs = hover_strip.clone();
        let revealer_fs = top_dock_revealer.clone();

        let toggle_fullscreen = Rc::new(move || {
            if win_fs.is_fullscreen() {
                win_fs.unfullscreen();
                header_fs.set_visible(true);
                fs_btn_icon.set_icon_name("view-fullscreen-symbolic");
                strip_fs.set_visible(false);
                revealer_fs.set_reveal_child(false);
            } else {
                win_fs.fullscreen();
                header_fs.set_visible(false);
                fs_btn_icon.set_icon_name("view-restore-symbolic");
                strip_fs.set_visible(true);
                strip_fs.set_opacity(1.0);
                revealer_fs.set_reveal_child(false);
            }
        });

        let tf_click = Rc::clone(&toggle_fullscreen);
        fullscreen_btn.connect_clicked(move |_| {
            tf_click();
        });

        let tf_dock = Rc::clone(&toggle_fullscreen);
        dock_restore_btn.connect_clicked(move |_| {
            tf_dock();
        });

        // Channel setup for thread-safe cross-thread frame, status, auth, and clipboard updates
        let (frame_tx, mut frame_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let (status_tx, mut status_rx) = tokio::sync::mpsc::unbounded_channel::<(i32, String)>();
        let (auth_tx, mut auth_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let (clip_tx, mut clip_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

        let has_first_frame = Arc::new(AtomicBool::new(false));
        let first_frame_cb = Arc::clone(&has_first_frame);

        let da_redraw = drawing_area.clone();
        let loading_hide = loading_box.clone();
        let status_ready = status_lbl.clone();

        glib::MainContext::default().spawn_local(async move {
            while frame_rx.recv().await.is_some() {
                if !first_frame_cb.swap(true, Ordering::SeqCst) {
                    loading_hide.set_visible(false);
                    status_ready.set_text("Connected • In-App FreeRDP 3");
                    debug!("First native RDP frame received; revealing canvas");
                }
                da_redraw.queue_draw();
            }
        });

        let status_ready_cb = status_lbl.clone();
        glib::MainContext::default().spawn_local(async move {
            while let Some((code, msg)) = status_rx.recv().await {
                if code == 1 {
                    status_ready_cb.set_text("Connected • In-App FreeRDP 3");
                } else if !msg.is_empty() {
                    status_ready_cb.set_text(&msg);
                }
            }
        });

        let on_frame_ready: FrameNotifier = Arc::new(move || {
            let _ = frame_tx.send(());
        });

        let on_status: StatusNotifier = Arc::new(move |code, msg| {
            let _ = status_tx.send((code, msg));
        });

        let on_auth_url: AuthNotifier = Arc::new(move |url| {
            let _ = auth_tx.send(url);
        });

        let on_clipboard: ClipboardNotifier = Arc::new(move |text| {
            let _ = clip_tx.send(text);
        });

        let bridge = match NativeRdpBridge::new(
            &cli_args,
            on_frame_ready,
            Some(on_auth_url),
            Some(on_status),
            Some(on_clipboard),
        ) {
            Some(b) => Rc::new(RefCell::new(Some(b))),
            None => {
                warn!("Failed to initialize in-process FreeRDP bridge context");
                status_lbl.set_text("Failed to initialize FreeRDP engine");
                Rc::new(RefCell::new(None))
            }
        };

        // Wire headless OAuth challenge handler
        let bridge_auth = Rc::clone(&bridge);
        let win_auth = window.clone();
        let status_auth = status_lbl.clone();
        let user_upn = pc
            .user_principal_name
            .clone()
            .or_else(crate::config::AppConfig::get_user_upn)
            .unwrap_or_default();

        glib::MainContext::default().spawn_local(async move {
            while let Some(mut auth_url) = auth_rx.recv().await {
                debug!("Received FreeRDP in-process authentication challenge: {auth_url}");
                if !user_upn.is_empty() && !auth_url.contains("login_hint=") {
                    let encoded_upn: String =
                        url::form_urlencoded::byte_serialize(user_upn.as_bytes()).collect();
                    let sep = if auth_url.contains('?') { "&" } else { "?" };
                    auth_url = format!("{auth_url}{sep}login_hint={encoded_upn}");
                }

                status_auth.set_text("Authenticating Cloud PC session...");
                let b_reply = Rc::clone(&bridge_auth);
                let s_reply = status_auth.clone();

                RdpAuthDialog::show(&win_auth, &auth_url, move |redirect_url| {
                    debug!("Submitting authentication redirect callback to FreeRDP bridge");
                    s_reply.set_text("Authentication verified. Connecting...");
                    if let Some(ref b) = *b_reply.borrow() {
                        b.provide_auth_result(&redirect_url);
                    }
                });
            }
        });

        // Inhibit system shortcuts when RDP window is active/focused so Meta / Super key
        // is delivered directly to the Cloud PC instead of opening GNOME Activities / App Grid
        let win_inhibit = window.clone();
        let inhibit_shortcuts = Rc::new(move |active: bool| {
            if let Some(surface) = win_inhibit.surface() {
                if let Ok(toplevel) = surface.downcast::<gtk4::gdk::Toplevel>() {
                    if active {
                        toplevel.inhibit_system_shortcuts(Option::<&gtk4::gdk::Event>::None);
                    } else {
                        toplevel.restore_system_shortcuts();
                    }
                }
            }
        });

        let is_realize = Rc::clone(&inhibit_shortcuts);
        window.connect_realize(move |_| {
            is_realize(true);
        });

        let is_active = Rc::clone(&inhibit_shortcuts);
        window.connect_is_active_notify(move |win| {
            is_active(win.is_active());
        });

        // Bidirectional Clipboard Integration
        let gdk_clipboard = gtk4::gdk::Display::default().map(|d| d.clipboard());
        let is_remote_updating = Rc::new(AtomicBool::new(false));

        if let Some(ref cb) = gdk_clipboard {
            // 1. Cloud PC -> Linux: Update local clipboard when remote copies text
            let cb_set = cb.clone();
            let is_rem_set = Rc::clone(&is_remote_updating);
            glib::MainContext::default().spawn_local(async move {
                while let Some(text) = clip_rx.recv().await {
                    debug!("Remote clipboard received text (len={}); setting local clipboard", text.len());
                    is_rem_set.store(true, Ordering::SeqCst);
                    cb_set.set_text(&text);
                }
            });

            // 2. Linux -> Cloud PC: Forward local clipboard changes to Cloud PC
            let b_clip_changed = Rc::clone(&bridge);
            let is_rem_changed = Rc::clone(&is_remote_updating);
            cb.connect_changed(move |clipboard| {
                if is_rem_changed.swap(false, Ordering::SeqCst) {
                    return;
                }
                let b_send = Rc::clone(&b_clip_changed);
                clipboard.read_text_async(None as Option<&gtk4::gio::Cancellable>, move |res| {
                    if let Ok(Some(text)) = res {
                        if let Some(ref b) = *b_send.borrow() {
                            b.send_clipboard_text(&text);
                        }
                    }
                });
            });

            // 3. Focus-in sync: Whenever RDP session gains focus, ensure latest local text is in Cloud PC
            let b_focus = Rc::clone(&bridge);
            let cb_focus = cb.clone();
            let is_rem_focus = Rc::clone(&is_remote_updating);
            window.connect_is_active_notify(move |win| {
                if win.is_active() {
                    let b_send = Rc::clone(&b_focus);
                    let is_rem = Rc::clone(&is_rem_focus);
                    cb_focus.read_text_async(None as Option<&gtk4::gio::Cancellable>, move |res| {
                        if is_rem.load(Ordering::SeqCst) {
                            return;
                        }
                        if let Ok(Some(text)) = res {
                            if let Some(ref b) = *b_send.borrow() {
                                b.send_clipboard_text(&text);
                            }
                        }
                    });
                }
            });
        }

        // Wire Cairo paint function and dynamic resolution resize debouncing
        let bridge_draw = Rc::clone(&bridge);
        let pending_size = Rc::new(RefCell::new((0i32, 0i32)));
        let resize_seq = Rc::new(RefCell::new(0u64));

        drawing_area.set_draw_func(move |_area, cr, w, h| {
            // Paint black letterbox background
            cr.set_source_rgb(0.0, 0.0, 0.0);
            let _ = cr.paint();

            // Dynamic resolution detection & debouncing
            if w >= 320 && h >= 240 {
                let mut last = pending_size.borrow_mut();
                if last.0 != w || last.1 != h {
                    *last = (w, h);
                    let mut seq = resize_seq.borrow_mut();
                    *seq = seq.wrapping_add(1);
                    let current_seq = *seq;

                    let b_resize = Rc::clone(&bridge_draw);
                    let seq_check = Rc::clone(&resize_seq);
                    glib::timeout_add_local_once(
                        std::time::Duration::from_millis(300),
                        move || {
                            if *seq_check.borrow() == current_seq {
                                if let Some(ref b) = *b_resize.borrow() {
                                    debug!(
                                        "Sending dynamic resolution resize to RDP server: {w}x{h}"
                                    );
                                    b.send_resize(w as u32, h as u32, 100);
                                }
                            }
                        },
                    );
                }
            }

            if let Some(ref b) = *bridge_draw.borrow() {
                if let Ok(frame) = b.frame_buffer.lock() {
                    if !frame.pixels.is_empty() && frame.width > 0 && frame.height > 0 {
                        unsafe {
                            let surface = cairo::ImageSurface::create_for_data_unsafe(
                                frame.pixels.as_ptr() as *mut u8,
                                cairo::Format::Rgb24,
                                frame.width as i32,
                                frame.height as i32,
                                frame.stride as i32,
                            );
                            if let Ok(surf) = surface {
                                if frame.width > 0 && frame.height > 0 && w > 0 && h > 0 {
                                    let sx = w as f64 / frame.width as f64;
                                    let sy = h as f64 / frame.height as f64;
                                    cr.scale(sx, sy);
                                    cr.set_operator(cairo::Operator::Source);
                                    let _ = cr.set_source_surface(&surf, 0.0, 0.0);
                                    let _ = cr.paint();
                                }
                            }
                        }
                    }
                }
            }
        });

        // Track last known mouse position for scroll event targeting
        let last_mouse_pos = Rc::new(RefCell::new((0u16, 0u16)));

        // Mouse Motion
        let bridge_motion = Rc::clone(&bridge);
        let da_motion = drawing_area.clone();
        let last_pos_motion = Rc::clone(&last_mouse_pos);

        let motion_ctrl = EventControllerMotion::new();
        motion_ctrl.connect_motion(move |_ctrl, x, y| {
            if let Some(ref b) = *bridge_motion.borrow() {
                let (rx, ry) = map_coords(x, y, da_motion.width(), da_motion.height(), b);
                *last_pos_motion.borrow_mut() = (rx, ry);
                b.send_mouse(PTR_FLAGS_MOVE, rx, ry);
            }
        });
        drawing_area.add_controller(motion_ctrl);

        // Mouse Click
        let bridge_click = Rc::clone(&bridge);
        let da_click = drawing_area.clone();
        let click_gesture = GestureClick::new();
        click_gesture.set_button(0); // Listen to all buttons
        let last_pos_click = Rc::clone(&last_mouse_pos);

        let b_press = Rc::clone(&bridge_click);
        let da_press = da_click.clone();
        let last_press_pos = Rc::clone(&last_pos_click);
        click_gesture.connect_pressed(move |gesture, _n_press, x, y| {
            gesture.set_state(gtk4::EventSequenceState::Claimed);
            let button = gesture.current_button();
            let flag = match button {
                1 => PTR_FLAGS_BUTTON1, // Left
                2 => PTR_FLAGS_BUTTON3, // Middle
                3 => PTR_FLAGS_BUTTON2, // Right
                _ => PTR_FLAGS_BUTTON1,
            };
            if let Some(ref b) = *b_press.borrow() {
                let (rx, ry) = map_coords(x, y, da_press.width(), da_press.height(), b);
                *last_press_pos.borrow_mut() = (rx, ry);
                b.send_mouse(flag | PTR_FLAGS_DOWN, rx, ry);
            }
        });

        let b_release = Rc::clone(&bridge_click);
        let da_release = da_click.clone();
        click_gesture.connect_released(move |gesture, _n_press, x, y| {
            let button = gesture.current_button();
            let flag = match button {
                1 => PTR_FLAGS_BUTTON1, // Left
                2 => PTR_FLAGS_BUTTON3, // Middle
                3 => PTR_FLAGS_BUTTON2, // Right
                _ => PTR_FLAGS_BUTTON1,
            };
            if let Some(ref b) = *b_release.borrow() {
                let (rx, ry) = map_coords(x, y, da_release.width(), da_release.height(), b);
                b.send_mouse(flag, rx, ry);
            }
        });
        drawing_area.add_controller(click_gesture);

        // Mouse Scroll (supports both vertical and horizontal wheel/touchpad gestures)
        let bridge_scroll = Rc::clone(&bridge);
        let last_pos_scroll = Rc::clone(&last_mouse_pos);
        let scroll_ctrl = EventControllerScroll::new(EventControllerScrollFlags::BOTH_AXES);
        scroll_ctrl.connect_scroll(move |_ctrl, dx, dy| {
            if let Some(ref b) = *bridge_scroll.borrow() {
                let (mx, my) = *last_pos_scroll.borrow();
                if dy != 0.0 {
                    let mut flags = PTR_FLAGS_WHEEL;
                    if dy > 0.0 {
                        flags |= PTR_FLAGS_WHEEL_NEGATIVE;
                    }
                    flags |= WHEEL_ROTATION_DELTA;
                    b.send_wheel(flags, mx, my);
                }
                if dx != 0.0 {
                    let mut flags = PTR_FLAGS_HWHEEL;
                    if dx > 0.0 {
                        flags |= PTR_FLAGS_WHEEL_NEGATIVE;
                    }
                    flags |= WHEEL_ROTATION_DELTA;
                    b.send_wheel(flags, mx, my);
                }
            }
            glib::Propagation::Stop
        });
        drawing_area.add_controller(scroll_ctrl);

        // Keyboard Controller - Native hardware scancode forwarding
        let bridge_key = Rc::clone(&bridge);
        let key_ctrl = EventControllerKey::new();

        let b_kpress = Rc::clone(&bridge_key);
        let win_kfs = window.clone();
        let tf_key = Rc::clone(&toggle_fullscreen);

        key_ctrl.connect_key_pressed(move |_ctrl, keyval, keycode, state| {
            if (win_kfs.is_fullscreen() && keyval == Key::Escape)
                || (state.contains(ModifierType::CONTROL_MASK | ModifierType::ALT_MASK)
                    && (keyval == Key::Return || keyval == Key::KP_Enter))
            {
                tf_key();
                return glib::Propagation::Stop;
            }

            if let Some(ref b) = *b_kpress.borrow() {
                b.send_key(true, keycode);
            }
            glib::Propagation::Stop
        });

        let b_krelease = Rc::clone(&bridge_key);
        key_ctrl.connect_key_released(move |_ctrl, _keyval, keycode, _state| {
            if let Some(ref b) = *b_krelease.borrow() {
                b.send_key(false, keycode);
            }
        });
        drawing_area.add_controller(key_ctrl);

        // Start connection
        if let Some(ref b) = *bridge.borrow() {
            info!("Starting FreeRDP C-FFI session in-process...");
            if !b.start() {
                warn!("freerdp_bridge_start returned false");
                status_lbl.set_text("Connection failed to start");
            }
        }

        // Handle window close & disconnect
        let bridge_close = Rc::clone(&bridge);
        let win_close = window.clone();
        let on_close_rc = Rc::new(on_close);
        let on_close_cb = Rc::clone(&on_close_rc);

        let do_disconnect = move || {
            if let Some(b) = bridge_close.borrow_mut().take() {
                b.stop();
            }
            on_close_cb();
            win_close.close();
        };

        let dd_clone = Rc::new(do_disconnect);
        let dd_dc = Rc::clone(&dd_clone);
        disconnect_btn.connect_clicked(move |_| {
            dd_dc();
        });

        let dd_dock = Rc::clone(&dd_clone);
        dock_disconnect_btn.connect_clicked(move |_| {
            dd_dock();
        });

        let dd_close = Rc::clone(&dd_clone);
        window.connect_close_request(move |_| {
            dd_close();
            glib::Propagation::Proceed
        });

        // Start RDP session in fullscreen mode immediately
        toggle_fullscreen();

        window.present();

        Rc::new(Self)
    }
}
