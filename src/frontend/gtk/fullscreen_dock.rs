//! Top dock shown over a fullscreen session window.
//!
//! While collapsed only a few pixels of its bottom edge peek out at the top of the screen;
//! it slides down when the pointer reaches that sliver (or the top-centre edge), stays open
//! while the pointer is over (or near) it or one of its menus is open, and slides back up
//! shortly after the pointer moves away.

use adw::prelude::*;
use gtk4::{
    gio, glib, Box, Button, EventControllerMotion, Label, MenuButton, Orientation, Overlay,
    Popover, Scale, Separator,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

thread_local! {
    static CSS_INSTALLED: Cell<bool> = const { Cell::new(false) };
}

/// The dock uses built-in classes (opaque background, frame outline); the only custom rule
/// is an accent bottom edge so the collapsed sliver stays visible over remote desktops that
/// match the theme colour (white on light, black on dark).
fn install_css() {
    if CSS_INSTALLED.replace(true) {
        return;
    }
    let Some(display) = gtk4::gdk::Display::default() else {
        return;
    };
    let provider = gtk4::CssProvider::new();
    provider.load_from_string(".fullscreen-dock { border-bottom: 3px solid @accent_bg_color; }");
    gtk4::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

pub struct FullscreenDock {
    window: adw::Window,
    dock: Box,
    animation: adw::TimedAnimation,
    revealed: Cell<bool>,
    hide_timer: RefCell<Option<glib::SourceId>>,
    /// Last pointer position in window coordinates.
    last_pointer: Cell<(f64, f64)>,
    pub quality_icon: gtk4::Image,
    pub scale_button: MenuButton,
    pub session_button: MenuButton,
    #[allow(dead_code)]
    pub mic_button: Button,
    #[allow(dead_code)]
    pub volume_button: MenuButton,
    pub restore_button: Button,
    pub minimize_button: Button,
    pub disconnect_button: Button,
}

impl FullscreenDock {
    /// Builds the dock as an overlay child of `overlay` (which must fill `window`) and
    /// starts tracking the pointer. `scale_menu` and `session_menu` back the dock's display
    /// scale and session action buttons.
    pub fn new(
        window: &adw::Window,
        overlay: &Overlay,
        title: &str,
        scale_menu: &gio::MenuModel,
        session_menu: &gio::MenuModel,
    ) -> Rc<Self> {
        install_css();

        let dock_box = Box::new(Orientation::Horizontal, 8);
        for class in ["toolbar", "background", "frame", "fullscreen-dock"] {
            dock_box.add_css_class(class);
        }
        dock_box.set_visible(false);

        let title_label = Label::new(Some(title));
        title_label.add_css_class("heading");
        dock_box.append(&title_label);
        let quality_icon = super::session_window::quality_icon();
        dock_box.append(&quality_icon);
        dock_box.append(&Separator::new(Orientation::Vertical));

        let mic_muted = crate::platform::audio::is_mic_muted();
        let mic_button = Button::builder()
            .icon_name(if mic_muted {
                "microphone-disabled-symbolic"
            } else {
                "audio-input-microphone-symbolic"
            })
            .tooltip_text(if mic_muted {
                "Unmute Microphone (Currently Muted)"
            } else {
                "Mute Microphone"
            })
            .build();
        mic_button.add_css_class("flat");
        if mic_muted {
            mic_button.add_css_class("warning");
        }
        let mic_btn_weak = mic_button.downgrade();
        mic_button.connect_clicked(move |_| {
            let Some(btn) = mic_btn_weak.upgrade() else {
                return;
            };
            let muted = crate::platform::audio::toggle_mic_muted();
            if muted {
                btn.set_icon_name("microphone-disabled-symbolic");
                btn.set_tooltip_text(Some("Unmute Microphone (Currently Muted)"));
                btn.add_css_class("warning");
            } else {
                btn.set_icon_name("audio-input-microphone-symbolic");
                btn.set_tooltip_text(Some("Mute Microphone"));
                btn.remove_css_class("warning");
            }
        });
        dock_box.append(&mic_button);

        let current_vol = crate::platform::audio::get_volume();
        let volume_popover = Popover::new();
        let vol_box = Box::new(Orientation::Vertical, 6);
        vol_box.set_margin_top(8);
        vol_box.set_margin_bottom(8);
        vol_box.set_margin_start(12);
        vol_box.set_margin_end(12);

        let vol_label = Label::new(Some(&format!("Volume: {current_vol}%")));
        vol_label.add_css_class("heading");
        vol_box.append(&vol_label);

        let vol_scale = Scale::with_range(Orientation::Horizontal, 0.0, 100.0, 5.0);
        vol_scale.set_value(f64::from(current_vol));
        vol_scale.set_width_request(140);
        vol_box.append(&vol_scale);
        volume_popover.set_child(Some(&vol_box));

        let vol_icon = if current_vol == 0 {
            "audio-volume-muted-symbolic"
        } else if current_vol < 50 {
            "audio-volume-low-symbolic"
        } else {
            "audio-volume-high-symbolic"
        };
        let volume_button = MenuButton::builder()
            .icon_name(vol_icon)
            .tooltip_text("Session Volume")
            .popover(&volume_popover)
            .build();
        volume_button.add_css_class("flat");

        let vol_label_weak = vol_label.downgrade();
        let vol_btn_weak = volume_button.downgrade();
        vol_scale.connect_value_changed(move |scale| {
            let vol = scale.value().round().clamp(0.0, 100.0) as u32;
            crate::platform::audio::set_volume(vol);
            if let Some(lbl) = vol_label_weak.upgrade() {
                lbl.set_label(&format!("Volume: {vol}%"));
            }
            if let Some(btn) = vol_btn_weak.upgrade() {
                let icon = if vol == 0 {
                    "audio-volume-muted-symbolic"
                } else if vol < 50 {
                    "audio-volume-low-symbolic"
                } else {
                    "audio-volume-high-symbolic"
                };
                btn.set_icon_name(icon);
            }
        });
        dock_box.append(&volume_button);
        dock_box.append(&Separator::new(Orientation::Vertical));

        let scale_button = MenuButton::builder()
            .icon_name("zoom-in-symbolic")
            .tooltip_text("Display Scale")
            .menu_model(scale_menu)
            .build();
        scale_button.add_css_class("flat");
        dock_box.append(&scale_button);

        let session_button = MenuButton::builder()
            .icon_name("input-keyboard-symbolic")
            .tooltip_text("Keys and Session Actions")
            .menu_model(session_menu)
            .build();
        session_button.add_css_class("flat");
        dock_box.append(&session_button);

        let restore_button = Button::builder()
            .icon_name("view-restore-symbolic")
            .tooltip_text("Exit Fullscreen (Ctrl+Alt+Enter)")
            .build();
        restore_button.add_css_class("flat");
        dock_box.append(&restore_button);

        let minimize_button = Button::builder()
            .icon_name("window-minimize-symbolic")
            .tooltip_text("Minimize")
            .build();
        minimize_button.add_css_class("flat");
        dock_box.append(&minimize_button);

        let disconnect_button = Button::builder()
            .icon_name("system-log-out-symbolic")
            .tooltip_text("Disconnect")
            .build();
        disconnect_button.add_css_class("destructive-action");
        dock_box.append(&disconnect_button);

        let progress = Rc::new(Cell::new(0.0_f64));
        let overlay_anim = overlay.clone();
        let progress_anim = Rc::clone(&progress);
        let animation = adw::TimedAnimation::builder()
            .widget(&dock_box)
            .value_from(0.0)
            .value_to(1.0)
            .duration(Self::SLIDE_MS)
            .easing(adw::Easing::EaseOutCubic)
            .target(&adw::CallbackAnimationTarget::new(move |value| {
                progress_anim.set(value);
                overlay_anim.queue_allocate();
            }))
            .build();

        let dock_weak = dock_box.downgrade();
        overlay.connect_get_child_position(move |overlay, child| {
            let dock = dock_weak.upgrade()?;
            (child == dock.upcast_ref::<gtk4::Widget>())
                .then(|| Self::allocation(overlay, child, progress.get()))
        });
        overlay.set_overflow(gtk4::Overflow::Hidden);
        overlay.add_overlay(&dock_box);

        let dock = Rc::new(Self {
            window: window.clone(),
            dock: dock_box,
            animation,
            revealed: Cell::new(false),
            hide_timer: RefCell::new(None),
            last_pointer: Cell::new((0.0, f64::MAX)),
            quality_icon,
            scale_button,
            session_button,
            mic_button,
            volume_button,
            restore_button,
            minimize_button,
            disconnect_button,
        });

        // One window-level controller decides from the pointer position, so visibility
        // doesn't depend on enter/leave crossings between the canvas and the dock.
        let motion = EventControllerMotion::new();
        motion.set_propagation_phase(gtk4::PropagationPhase::Capture);
        let weak = Rc::downgrade(&dock);
        motion.connect_enter(move |_, x, y| {
            if let Some(dock) = weak.upgrade() {
                dock.track_pointer(x, y);
            }
        });
        let weak = Rc::downgrade(&dock);
        motion.connect_motion(move |_, x, y| {
            if let Some(dock) = weak.upgrade() {
                dock.track_pointer(x, y);
            }
        });
        let weak = Rc::downgrade(&dock);
        motion.connect_leave(move |_| {
            if let Some(dock) = weak.upgrade() {
                dock.pointer_left_window();
            }
        });
        window.add_controller(motion);

        // Menu popovers lie outside the dock; keep the dock open while one is shown.
        for button in [
            &dock.scale_button,
            &dock.session_button,
            &dock.volume_button,
        ] {
            let weak = Rc::downgrade(&dock);
            button.connect_active_notify(move |button| {
                if let Some(dock) = weak.upgrade() {
                    if button.is_active() {
                        dock.cancel_hide();
                    } else {
                        let (x, y) = dock.last_pointer.get();
                        dock.track_pointer(x, y);
                    }
                }
            });
        }
        dock
    }

    /// Shows the (collapsed) dock in fullscreen and hides it otherwise.
    pub fn set_fullscreen(&self, fullscreen: bool) {
        self.dock.set_visible(fullscreen);
        self.hide_now();
    }

    /// Height (px) of the dock that stays visible while collapsed.
    const PEEK_PX: i32 = 6;
    /// Distance from the top edge (in px) that counts as "touching" it.
    const EDGE_PX: f64 = 3.0;
    /// Extra margin around the open dock that still counts as hovering it.
    const SLACK_PX: f32 = 24.0;
    const HIDE_DELAY: std::time::Duration = std::time::Duration::from_millis(600);
    const SLIDE_MS: u32 = 200;

    /// Allocation for the dock inside `overlay`: horizontally centred, shifted up so that only
    /// `PEEK_PX` remain visible at `progress` 0 and fully shown at `progress` 1.
    fn allocation(overlay: &Overlay, dock: &gtk4::Widget, progress: f64) -> gtk4::gdk::Rectangle {
        let (_, natural) = dock.preferred_size();
        let width = natural.width().min(overlay.width());
        let height = natural.height();
        let hidden = f64::from((height - Self::PEEK_PX).max(0));
        let y = -(hidden * (1.0 - progress.clamp(0.0, 1.0))).round() as i32;
        gtk4::gdk::Rectangle::new((overlay.width() - width) / 2, y, width, height)
    }

    fn set_revealed(&self, reveal: bool) {
        if self.revealed.replace(reveal) == reveal {
            return;
        }
        // Start from the current position so reversing mid-slide doesn't jump.
        self.animation.set_value_from(self.animation.value());
        self.animation.set_value_to(if reveal { 1.0 } else { 0.0 });
        self.animation.play();
    }

    fn show(&self) {
        self.cancel_hide();
        if self.window.is_fullscreen() {
            self.set_revealed(true);
        }
    }

    fn hide_now(&self) {
        self.cancel_hide();
        self.set_revealed(false);
    }

    fn cancel_hide(&self) {
        if let Some(id) = self.hide_timer.take() {
            id.remove();
        }
    }

    fn schedule_hide(self: &Rc<Self>) {
        if !self.revealed.get()
            || self.hide_timer.borrow().is_some()
            || self.scale_button.is_active()
            || self.session_button.is_active()
            || self.volume_button.is_active()
        {
            return;
        }
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(Self::HIDE_DELAY, move || {
            if let Some(dock) = weak.upgrade() {
                // The source has already fired, so it must not be removed again.
                dock.hide_timer.take();
                dock.hide_now();
            }
        });
        *self.hide_timer.borrow_mut() = Some(id);
    }

    /// Whether (x, y) in window coordinates is over the dock, padded by `slack` on the sides
    /// and bottom. Everything above the dock up to the screen edge counts.
    fn over_dock(&self, x: f64, y: f64, slack: f32) -> bool {
        if !self.dock.is_visible() {
            return false;
        }
        let Some(bounds) = self.dock.compute_bounds(&self.window) else {
            return false;
        };
        let (x, y) = (x as f32, y as f32);
        x >= bounds.x() - slack
            && x <= bounds.x() + bounds.width() + slack
            && y <= bounds.y() + bounds.height() + slack
    }

    /// Pointer position in window coordinates (captured before any child handles it).
    fn track_pointer(self: &Rc<Self>, x: f64, y: f64) {
        self.last_pointer.set((x, y));
        if !self.window.is_fullscreen() {
            return;
        }
        let half_width = f64::from(self.window.width()) / 2.0;
        let at_top_centre =
            y <= Self::EDGE_PX && (x - half_width).abs() <= (half_width / 3.0).max(160.0);
        let slack = if self.revealed.get() {
            Self::SLACK_PX
        } else {
            0.0
        };
        if at_top_centre || self.over_dock(x, y, slack) {
            self.show();
        } else {
            self.schedule_hide();
        }
    }

    /// Leaving through the top edge (e.g. towards a monitor above) keeps the dock open.
    fn pointer_left_window(self: &Rc<Self>) {
        if self.last_pointer.get().1 > Self::EDGE_PX {
            self.schedule_hide();
        }
    }
}
