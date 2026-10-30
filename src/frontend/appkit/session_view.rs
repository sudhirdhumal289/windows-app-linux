//! The remote desktop view: draws the session's BGRX frame and forwards keyboard, mouse
//! and focus events to its [`Session`].

use super::session_window::Session;
use super::util::{self, ns};
use crate::rdp::keymap::rdp_scancode_from_mac;
use crate::rdp::{Frame, MouseButton};
use block2::RcBlock;
use objc2::rc::{Retained, Weak as ObjcWeak};
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameDarkAqua, NSAutoresizingMaskOptions,
    NSBox, NSBoxType, NSColor, NSCursor, NSEvent, NSEventMask, NSEventModifierFlags, NSEventType,
    NSGraphicsContext, NSImageView, NSProgressIndicator, NSProgressIndicatorStyle, NSResponder,
    NSStackView, NSTitlePosition, NSTrackingArea, NSTrackingAreaOptions,
    NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_core_foundation::{CFData, CFRetained, CGPoint, CGRect};
use objc2_core_graphics::{
    kCGColorSpaceSRGB, CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGContext,
    CGDataProvider, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo, CGInterpolationQuality,
};
use objc2_foundation::{NSArray, NSPoint, NSRect, NSSize};
use std::cell::RefCell;
use std::ptr::NonNull;
use std::rc::{Rc, Weak};

/// Pixels of a precise (trackpad) scroll that make one wheel notch.
const PRECISE_SCROLL_PER_NOTCH: f64 = 10.0;

/// Device-dependent modifier bits (`NX_DEVICE*KEYMASK`) of the left/right modifier keys.
const DEVICE_MODIFIER_BITS: usize = 0x1 | 0x2 | 0x4 | 0x8 | 0x10 | 0x20 | 0x40 | 0x2000;

/// What the view shows over the remote desktop.
pub(super) enum Overlay {
    None,
    Connecting { name: String, detail: String },
    Ended { message: String },
}

pub(super) struct ViewIvars {
    session: RefCell<Weak<Session>>,
    image: RefCell<Option<CFRetained<CGImage>>>,
    overlay: RefCell<Option<Retained<NSView>>>,
    key_monitor: RefCell<Option<Retained<AnyObject>>>,
    cursor_kind: RefCell<crate::rdp::CursorKind>,
}

define_class!(
    // SAFETY: NSView may be subclassed; the overridden methods keep their signatures and
    // the class has no Drop impl.
    #[unsafe(super(NSView, NSResponder, objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WindowsAppSessionView"]
    #[ivars = ViewIvars]
    pub(super) struct SessionView;

    impl SessionView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(isOpaque))]
        fn is_opaque(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(becomeFirstResponder))]
        fn become_first_responder(&self) -> bool {
            // SAFETY: NSResponder's `becomeFirstResponder` takes no arguments, returns BOOL.
            let accepted: bool = unsafe { msg_send![super(self), becomeFirstResponder] };
            if accepted {
                if let Some(session) = self.session() {
                    util::defer(move || session.focus_gained());
                }
            }
            accepted
        }

        #[unsafe(method(resignFirstResponder))]
        fn resign_first_responder(&self) -> bool {
            // SAFETY: NSResponder's `resignFirstResponder` takes no arguments, returns BOOL.
            let accepted: bool = unsafe { msg_send![super(self), resignFirstResponder] };
            if accepted {
                if let Some(session) = self.session() {
                    session.focus_lost();
                }
            }
            accepted
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.draw();
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: NSSize) {
            // SAFETY: NSView's `setFrameSize:` takes an NSSize and returns nothing.
            let () = unsafe { msg_send![super(self), setFrameSize: size] };
            if let Some(session) = self.session() {
                session.schedule_resize();
            }
        }

        #[unsafe(method(viewDidChangeBackingProperties))]
        fn view_did_change_backing_properties(&self) {
            // SAFETY: NSView's method takes no arguments and returns nothing.
            let () = unsafe { msg_send![super(self), viewDidChangeBackingProperties] };
            if let Some(session) = self.session() {
                session.schedule_resize();
            }
        }

        // ----- Keyboard -----

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            self.key_event(event, true);
        }

        #[unsafe(method(keyUp:))]
        fn key_up(&self, event: &NSEvent) {
            self.key_event(event, false);
        }

        #[unsafe(method(flagsChanged:))]
        fn flags_changed(&self, event: &NSEvent) {
            self.modifier_event(event);
        }

        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &NSEvent) -> bool {
            let forward = event.r#type() == NSEventType::KeyDown
                && event.modifierFlags().contains(NSEventModifierFlags::Command)
                && self.is_first_responder()
                && !is_local_shortcut(event);
            if forward {
                self.key_event(event, true);
                return true.into();
            }
            // SAFETY: NSView's `performKeyEquivalent:` takes an NSEvent and returns BOOL.
            unsafe { msg_send![super(self), performKeyEquivalent: event] }
        }

        // ----- Pointer -----

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            self.pointer_event(event);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            self.pointer_event(event);
        }

        #[unsafe(method(rightMouseDragged:))]
        fn right_mouse_dragged(&self, event: &NSEvent) {
            self.pointer_event(event);
        }

        #[unsafe(method(otherMouseDragged:))]
        fn other_mouse_dragged(&self, event: &NSEvent) {
            self.pointer_event(event);
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.take_focus();
            self.button_event(event, MouseButton::Left, true);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            self.button_event(event, MouseButton::Left, false);
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, event: &NSEvent) {
            self.take_focus();
            self.button_event(event, MouseButton::Right, true);
        }

        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, event: &NSEvent) {
            self.button_event(event, MouseButton::Right, false);
        }

        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, event: &NSEvent) {
            if let Some(button) = other_button(event) {
                self.take_focus();
                self.button_event(event, button, true);
            }
        }

        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up(&self, event: &NSEvent) {
            if let Some(button) = other_button(event) {
                self.button_event(event, button, false);
            }
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            let (mut dx, mut dy) = (event.scrollingDeltaX(), event.scrollingDeltaY());
            if event.hasPreciseScrollingDeltas() {
                dx /= PRECISE_SCROLL_PER_NOTCH;
                dy /= PRECISE_SCROLL_PER_NOTCH;
            }
            self.pointer_event(event);
            // AppKit reports positive deltas for scrolling up/left (natural scrolling is
            // already applied); the session expects positive values for down/right.
            self.with_controller(|controller| controller.wheel(-dx, -dy));
        }

        // The view handles its own right clicks; no context menu.
        #[unsafe(method_id(menuForEvent:))]
        fn menu_for_event(&self, _event: &NSEvent) -> Option<Retained<objc2_app_kit::NSMenu>> {
            None
        }

        #[unsafe(method(resetCursorRects))]
        fn reset_cursor_rects(&self) {
            let cursor = self.current_nscursor();
            unsafe {
                let _: () = msg_send![self, addCursorRect: self.bounds(), cursor: &*cursor];
            }
        }

        #[unsafe(method(cursorUpdate:))]
        fn cursor_update(&self, _event: &NSEvent) {
            self.apply_cursor();
        }
    }
);

impl SessionView {
    pub(super) fn new(mtm: MainThreadMarker, session: Weak<Session>) -> Retained<Self> {
        let view = Self::alloc(mtm).set_ivars(ViewIvars {
            session: RefCell::new(session),
            image: RefCell::new(None),
            overlay: RefCell::new(None),
            key_monitor: RefCell::new(None),
            cursor_kind: RefCell::new(crate::rdp::CursorKind::Default),
        });
        // SAFETY: NSView's designated initializer takes the frame rectangle.
        let view: Retained<Self> =
            unsafe { msg_send![super(view), initWithFrame: util::rect(0.0, 0.0, 1280.0, 800.0)] };
        view.setWantsLayer(true);
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let options = NSTrackingAreaOptions::MouseMoved
            | NSTrackingAreaOptions::CursorUpdate
            | NSTrackingAreaOptions::ActiveInKeyWindow
            | NSTrackingAreaOptions::InVisibleRect;
        // SAFETY: The owner is the view itself, which outlives its tracking areas.
        let area = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                NSRect::ZERO,
                options,
                Some(&view),
                None,
            )
        };
        view.addTrackingArea(&area);
        view.install_key_up_monitor();
        view
    }

    fn session(&self) -> Option<Rc<Session>> {
        self.ivars().session.borrow().upgrade()
    }

    fn with_controller(
        &self,
        f: impl FnOnce(&mut crate::frontend::common::session::SessionController),
    ) {
        if let Some(session) = self.session() {
            session.with_controller(f);
        }
    }

    fn current_nscursor(&self) -> Retained<NSCursor> {
        let kind = *self.ivars().cursor_kind.borrow();
        match kind {
            crate::rdp::CursorKind::IBeam => NSCursor::IBeamCursor(),
            crate::rdp::CursorKind::Hand => NSCursor::pointingHandCursor(),
            crate::rdp::CursorKind::Cross => NSCursor::crosshairCursor(),
            crate::rdp::CursorKind::ResizeNs => NSCursor::rowResizeCursor(),
            crate::rdp::CursorKind::ResizeWe => NSCursor::columnResizeCursor(),
            crate::rdp::CursorKind::NotAllowed => NSCursor::operationNotAllowedCursor(),
            _ => NSCursor::arrowCursor(),
        }
    }

    pub(super) fn set_cursor_bitmap(&self, bitmap: crate::rdp::CursorBitmap) {
        let width = usize::try_from(bitmap.width).ok();
        let height = usize::try_from(bitmap.height).ok();
        let (Some(width), Some(height)) = (width, height) else {
            return;
        };
        let stride = match width.checked_mul(4) {
            Some(s) if s > 0 => s,
            _ => return,
        };
        let len = match stride.checked_mul(height) {
            Some(l) if bitmap.rgba.len() >= l => l,
            _ => return,
        };
        let data = CFData::from_bytes(&bitmap.rgba[..len]);
        let Some(provider) = CGDataProvider::with_cf_data(Some(&data)) else {
            return;
        };
        let Some(space) = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })) else {
            return;
        };
        let info = CGBitmapInfo(
            CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0,
        );
        let Some(cg_image) = (unsafe {
            CGImage::new(
                width,
                height,
                8,
                32,
                stride,
                Some(&space),
                info,
                Some(&provider),
                std::ptr::null(),
                false,
                CGColorRenderingIntent::RenderingIntentDefault,
            )
        }) else {
            return;
        };
        let image = objc2_app_kit::NSImage::initWithCGImage_size(
            objc2_app_kit::NSImage::alloc(),
            &cg_image,
            NSSize {
                width: width as f64,
                height: height as f64,
            },
        );
        let cursor = objc2_app_kit::NSCursor::initWithImage_hotSpot(
            objc2_app_kit::NSCursor::alloc(),
            &image,
            NSPoint {
                x: bitmap.hot_x as f64,
                y: bitmap.hot_y as f64,
            },
        );
        cursor.set();
    }

    pub(super) fn set_cursor_position(&self, _x: u32, _y: u32) {}

    pub(super) fn set_cursor(&self, kind: crate::rdp::CursorKind) {
        *self.ivars().cursor_kind.borrow_mut() = kind;
        self.apply_cursor();
        if let Some(window) = self.window() {
            unsafe {
                let _: () = msg_send![&window, invalidateCursorRectsForView: self];
            }
        }
    }

    fn apply_cursor(&self) {
        let kind = *self.ivars().cursor_kind.borrow();
        if kind == crate::rdp::CursorKind::Hidden {
            unsafe {
                NSCursor::setHiddenUntilMouseMoves(true);
            }
        } else {
            let cursor = self.current_nscursor();
            cursor.set();
        }
    }

    /// Stops forwarding input (the window is closing).
    pub(super) fn detach(&self) {
        *self.ivars().session.borrow_mut() = Weak::new();
        if let Some(monitor) = self.ivars().key_monitor.borrow_mut().take() {
            // SAFETY: The monitor was returned by `addLocalMonitorForEventsMatchingMask`.
            unsafe { NSEvent::removeMonitor(&monitor) };
        }
    }

    /// Whether the view receives the window's key events.
    pub(super) fn is_first_responder(&self) -> bool {
        let Some(responder) = self.window().and_then(|w| w.firstResponder()) else {
            return false;
        };
        std::ptr::eq(
            Retained::as_ptr(&responder).cast::<AnyObject>(),
            (self as *const Self).cast::<AnyObject>(),
        )
    }

    pub(super) fn take_focus(&self) {
        if let Some(window) = self.window() {
            if !self.is_first_responder() {
                window.makeFirstResponder(Some(self.as_ref()));
            }
        }
    }

    // ----- Frame -----

    /// Shows the engine's current frame.
    pub(super) fn show_frame(&self, frame: &Frame) {
        match image_from_frame(frame) {
            Some(image) => {
                *self.ivars().image.borrow_mut() = Some(image);
                self.setNeedsDisplay(true);
            }
            None => log::debug!("Skipping an empty or malformed frame"),
        }
    }

    /// The frame shown, for screenshots.
    pub(super) fn image(&self) -> Option<CFRetained<CGImage>> {
        self.ivars().image.borrow().clone()
    }

    fn draw(&self) {
        let Some(context) = NSGraphicsContext::currentContext() else {
            return;
        };
        let cg = context.CGContext();
        let ctx = Some(&*cg);
        let bounds = self.bounds();
        CGContext::set_rgb_fill_color(ctx, 0.0, 0.0, 0.0, 1.0);
        CGContext::fill_rect(ctx, bounds);
        let image = self.ivars().image.borrow();
        let Some(image) = image.as_deref() else {
            return;
        };
        // The view is flipped; Core Graphics draws images bottom-up.
        CGContext::save_g_state(ctx);
        CGContext::translate_ctm(ctx, bounds.origin.x, bounds.origin.y + bounds.size.height);
        CGContext::scale_ctm(ctx, 1.0, -1.0);
        CGContext::set_interpolation_quality(ctx, CGInterpolationQuality::Low);
        CGContext::draw_image(ctx, CGRect::new(CGPoint::ZERO, bounds.size), Some(image));
        CGContext::restore_g_state(ctx);
    }

    // ----- Input -----

    fn key_event(&self, event: &NSEvent, down: bool) {
        match rdp_scancode_from_mac(event.keyCode()) {
            Some(scancode) => self.with_controller(|controller| controller.key(scancode, down)),
            None => log::debug!("No scancode for macOS key code {:#x}", event.keyCode()),
        }
    }

    /// A modifier key changed: work out from the flags whether it went down or up.
    fn modifier_event(&self, event: &NSEvent) {
        let code = event.keyCode();
        let Some(scancode) = rdp_scancode_from_mac(code) else {
            return;
        };
        let flags = event.modifierFlags();
        if code == 0x39 {
            // Caps Lock reports its lock state, not the key; each change is one press.
            self.with_controller(|controller| {
                controller.key(scancode, true);
                controller.key(scancode, false);
            });
            return;
        }
        let (device_bit, flag) = match code {
            0x38 => (0x2, NSEventModifierFlags::Shift),
            0x3C => (0x4, NSEventModifierFlags::Shift),
            0x3B => (0x1, NSEventModifierFlags::Control),
            0x3E => (0x2000, NSEventModifierFlags::Control),
            0x3A => (0x20, NSEventModifierFlags::Option),
            0x3D => (0x40, NSEventModifierFlags::Option),
            0x37 => (0x8, NSEventModifierFlags::Command),
            0x36 => (0x10, NSEventModifierFlags::Command),
            _ => return,
        };
        let bits = flags.bits();
        let down = if bits & DEVICE_MODIFIER_BITS == 0 {
            // Some virtual keyboards only set the side-independent flags.
            flags.contains(flag)
        } else {
            bits & device_bit != 0
        };
        self.with_controller(|controller| controller.key(scancode, down));
    }

    /// AppKit swallows key-up events while Command is held; catch them before it does.
    fn install_key_up_monitor(&self) {
        let weak = ObjcWeak::new(self);
        let block = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
            // SAFETY: AppKit passes a valid event for the duration of the call.
            let current = unsafe { event.as_ref() };
            if let Some(view) = weak.load() {
                let command = current
                    .modifierFlags()
                    .contains(NSEventModifierFlags::Command);
                let key_window = view.window().is_some_and(|w| w.isKeyWindow());
                if command && key_window && view.is_first_responder() {
                    view.key_event(current, false);
                    return std::ptr::null_mut();
                }
            }
            event.as_ptr()
        });
        // SAFETY: The handler returns the event (or nil to consume it) as required; the
        // monitor is removed in `detach`.
        let monitor = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyUp, &block)
        };
        *self.ivars().key_monitor.borrow_mut() = monitor;
    }

    fn pointer_event(&self, event: &NSEvent) {
        let point = self.convertPoint_fromView(event.locationInWindow(), None);
        let size = self.bounds().size;
        self.with_controller(|controller| {
            controller.pointer_moved(point.x, point.y, (size.width, size.height));
        });
    }

    fn button_event(&self, event: &NSEvent, button: MouseButton, down: bool) {
        self.pointer_event(event);
        self.with_controller(|controller| controller.button(button, down));
    }

    // ----- Overlay -----

    /// Replaces the panel shown over the desktop.
    pub(super) fn set_overlay(&self, overlay: Overlay, on_close: impl Fn() + 'static) {
        if let Some(old) = self.ivars().overlay.borrow_mut().take() {
            old.removeFromSuperview();
        }
        let mtm = self.mtm();
        let views: Vec<Retained<NSView>> = match overlay {
            Overlay::None => return,
            Overlay::Connecting { name, detail } => {
                let spinner = NSProgressIndicator::new(mtm);
                spinner.setStyle(NSProgressIndicatorStyle::Spinning);
                spinner.setIndeterminate(true);
                spinner.sizeToFit();
                // SAFETY: Starting the animation of a spinner owned by this view.
                unsafe { spinner.startAnimation(None) };
                let title = util::label(mtm, &name);
                util::font(&title, 17.0, true);
                let detail = util::wrapping_label(mtm, &detail, 320.0);
                util::secondary(&detail);
                vec![
                    spinner.into_super(),
                    title.into_super().into_super(),
                    detail.into_super().into_super(),
                ]
            }
            Overlay::Ended { message } => {
                let message = if message.is_empty() {
                    "Disconnected".to_string()
                } else {
                    message
                };
                let icon = NSImageView::new(mtm);
                icon.setImage(util::symbol("network.slash", "Session ended").as_deref());
                icon.setFrameSize(NSSize::new(48.0, 48.0));
                // SAFETY: Reading an immutable extern CGFloat constant is sound.
                let weight = unsafe { objc2_app_kit::NSFontWeightLight };
                let config =
                    objc2_app_kit::NSImageSymbolConfiguration::configurationWithPointSize_weight(
                        36.0, weight,
                    );
                icon.setSymbolConfiguration(Some(&config));
                let title = util::label(mtm, "Session Ended");
                util::font(&title, 17.0, true);
                let detail = util::wrapping_label(mtm, &message, 320.0);
                util::secondary(&detail);
                let close = util::button(mtm, "Close", on_close);
                close.setKeyEquivalent(&ns("\r"));
                vec![
                    icon.into_super().into_super(),
                    title.into_super().into_super(),
                    detail.into_super().into_super(),
                    close.into_super().into_super(),
                ]
            }
        };
        let stack = NSStackView::stackViewWithViews(&NSArray::from_retained_slice(&views), mtm);
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        stack.setSpacing(10.0);
        stack.setFrameSize(stack.fittingSize());

        let panel = NSBox::new(mtm);
        panel.setBoxType(NSBoxType::Custom);
        panel.setTitlePosition(NSTitlePosition::NoTitle);
        panel.setBorderWidth(0.0);
        panel.setCornerRadius(14.0);
        panel.setFillColor(&NSColor::colorWithWhite_alpha(0.12, 0.92));
        // SAFETY: Reading an immutable extern string constant.
        let dark = NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua });
        panel.setAppearance(dark.as_deref());
        panel.setContentViewMargins(NSSize::new(28.0, 24.0));
        panel.setContentView(Some(&stack));
        panel.sizeToFit();
        let bounds = self.bounds();
        let size = panel.frame().size;
        panel.setFrameOrigin(CGPoint::new(
            ((bounds.size.width - size.width) / 2.0).round(),
            ((bounds.size.height - size.height) / 2.0).round(),
        ));
        panel.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin
                | NSAutoresizingMaskOptions::ViewMaxXMargin
                | NSAutoresizingMaskOptions::ViewMinYMargin
                | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        self.addSubview(&panel);
        *self.ivars().overlay.borrow_mut() = Some(panel.into_super());
    }
}

/// Shortcuts that stay with the app while the session has the keyboard.
fn is_local_shortcut(event: &NSEvent) -> bool {
    let flags = event.modifierFlags();
    let key = event
        .charactersIgnoringModifiers()
        .map(|s| s.to_string().to_lowercase())
        .unwrap_or_default();
    if flags.contains(NSEventModifierFlags::Control) {
        if flags.contains(NSEventModifierFlags::Shift) && key == "d" {
            return true;
        }
        return key == "f";
    }
    matches!(key.as_str(), "q" | "w" | "h" | "m")
}

fn other_button(event: &NSEvent) -> Option<MouseButton> {
    match event.buttonNumber() {
        2 => Some(MouseButton::Middle),
        3 => Some(MouseButton::X1),
        4 => Some(MouseButton::X2),
        _ => None,
    }
}

/// A Core Graphics image of a BGRX frame (pixels copied).
pub(super) fn image_from_frame(frame: &Frame) -> Option<CFRetained<CGImage>> {
    let width = usize::try_from(frame.width).ok()?;
    let height = usize::try_from(frame.height).ok()?;
    let stride = usize::try_from(frame.stride).ok()?;
    let len = stride.checked_mul(height)?;
    if width == 0 || height == 0 || stride < width * 4 || frame.pixels.len() < len {
        return None;
    }
    let data = CFData::from_bytes(&frame.pixels[..len]);
    let provider = CGDataProvider::with_cf_data(Some(&data))?;
    // SAFETY: Reading an immutable extern string constant.
    let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))?;
    let info =
        CGBitmapInfo(CGImageAlphaInfo::NoneSkipFirst.0 | CGImageByteOrderInfo::Order32Little.0);
    // SAFETY: The geometry matches the data provider's bytes and `decode` may be null.
    unsafe {
        CGImage::new(
            width,
            height,
            8,
            32,
            stride,
            Some(&space),
            info,
            Some(&provider),
            std::ptr::null(),
            false,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }
}
