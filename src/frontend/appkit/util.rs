//! Small helpers shared by the AppKit frontend: hopping between the Tokio runtime and the
//! main thread, Rust closures as Objective-C action targets, and constructors for the
//! controls, menus and alerts every screen uses.

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchTime};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAlert, NSAlertStyle, NSButton, NSColor, NSControl, NSControlTextEditingDelegate, NSFont,
    NSImage, NSLineBreakMode, NSMenu, NSMenuItem, NSModalResponse, NSPasteboard,
    NSPasteboardTypeString, NSSearchField, NSSearchFieldDelegate, NSTextField, NSTextFieldDelegate,
    NSTextView, NSWindow, NSWorkspace,
};
use objc2_foundation::{
    NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURL,
};
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::sync::OnceLock;
use std::time::Duration;

static RUNTIME: OnceLock<tokio::runtime::Handle> = OnceLock::new();

type Completion = Box<dyn FnOnce(Box<dyn Any + Send>)>;

thread_local! {
    /// Main-thread continuations of background work, keyed by a ticket that travels
    /// through the (Send-only) dispatch queue instead of the non-Send closure itself.
    static PENDING: RefCell<HashMap<u64, Completion>> = RefCell::new(HashMap::new());
    static NEXT_TICKET: Cell<u64> = const { Cell::new(1) };
}

/// Remembers the Tokio runtime that [`spawn`] uses.
pub(super) fn set_runtime(handle: tokio::runtime::Handle) {
    if RUNTIME.set(handle).is_err() {
        log::warn!("The async runtime was already set");
    }
}

fn register(completion: Completion) -> u64 {
    let ticket = NEXT_TICKET.get();
    NEXT_TICKET.set(ticket.wrapping_add(1));
    PENDING.with_borrow_mut(|pending| pending.insert(ticket, completion));
    ticket
}

fn complete(ticket: u64, value: Box<dyn Any + Send>) {
    // Taken out first: the continuation may register new work.
    let completion = PENDING.with_borrow_mut(|pending| pending.remove(&ticket));
    if let Some(completion) = completion {
        completion(value);
    }
}

/// Runs `future` on the Tokio runtime and then `done` with its output on the main thread.
pub(super) fn spawn<T, F>(future: F, done: impl FnOnce(T) + 'static)
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static,
{
    let ticket = register(Box::new(move |value| match value.downcast::<T>() {
        Ok(value) => done(*value),
        Err(_) => log::error!("Background task returned an unexpected type"),
    }));
    let Some(handle) = RUNTIME.get() else {
        log::error!("The async runtime is not running");
        PENDING.with_borrow_mut(|pending| pending.remove(&ticket));
        return;
    };
    handle.spawn(async move {
        let value: Box<dyn Any + Send> = Box::new(future.await);
        DispatchQueue::main().exec_async(move || complete(ticket, value));
    });
}

/// Runs blocking `work` on the runtime's blocking pool, then `done` on the main thread.
pub(super) fn spawn_blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(T) + 'static,
) {
    spawn(
        async move { tokio::task::spawn_blocking(work).await },
        move |result| match result {
            Ok(value) => done(value),
            Err(e) => log::error!("Background work failed: {e}"),
        },
    );
}

/// Runs `f` on the main thread after `delay`.
pub(super) fn run_later(delay: Duration, f: impl FnOnce() + 'static) {
    let ticket = register(Box::new(move |_| f()));
    let scheduled = DispatchTime::try_from(delay)
        .map_err(|_| ())
        .and_then(|when| {
            DispatchQueue::main()
                .after(when, move || complete(ticket, Box::new(())))
                .map_err(|_| ())
        });
    if scheduled.is_err() {
        DispatchQueue::main().exec_async(move || complete(ticket, Box::new(())));
    }
}

/// Runs `f` on the main thread once the current event has been handled.
pub(super) fn defer(f: impl FnOnce() + 'static) {
    let ticket = register(Box::new(move |_| f()));
    DispatchQueue::main().exec_async(move || complete(ticket, Box::new(())));
}

// ----- Closures as targets -----

pub(super) struct TargetIvars {
    action: Box<dyn Fn(&AnyObject)>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and the class has no Drop impl.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WindowsAppActionTarget"]
    #[ivars = TargetIvars]
    pub(super) struct ActionTarget;

    // SAFETY: NSObjectProtocol has no safety requirements.
    unsafe impl NSObjectProtocol for ActionTarget {}

    impl ActionTarget {
        // SAFETY: Standard action signature (`- (void)fire:(id)sender`).
        #[unsafe(method(fire:))]
        fn fire(&self, sender: Option<&AnyObject>) {
            let fallback;
            let sender = match sender {
                Some(sender) => sender,
                None => {
                    fallback = NSObject::new();
                    &fallback
                }
            };
            (self.ivars().action)(sender);
        }
    }
);

impl ActionTarget {
    pub(super) fn new(
        mtm: MainThreadMarker,
        action: impl Fn(&AnyObject) + 'static,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TargetIvars {
            action: Box::new(action),
        });
        // SAFETY: NSObject's `init` signature is correct.
        unsafe { msg_send![super(this), init] }
    }
}

/// The selector [`ActionTarget`] responds to.
pub(super) fn fire_sel() -> Sel {
    sel!(fire:)
}

/// Keeps `object` alive as long as `owner` (targets are weak references in AppKit).
pub(super) fn retain_with(owner: &AnyObject, object: &AnyObject) {
    let owner_ptr = std::ptr::from_ref(owner).cast_mut();
    let object_ptr = std::ptr::from_ref(object).cast_mut();
    // SAFETY: Both pointers are valid Objective-C objects; the key is the unique address
    // of the retained object and the policy retains it non-atomically for `owner`'s life.
    unsafe {
        objc2::ffi::objc_setAssociatedObject(
            owner_ptr,
            object_ptr.cast_const().cast(),
            object_ptr,
            objc2::ffi::OBJC_ASSOCIATION_RETAIN_NONATOMIC,
        );
    }
}

/// Makes `control` call `action` (the sender is passed along).
pub(super) fn on_action(control: &NSControl, action: impl Fn(&AnyObject) + 'static) {
    let target = ActionTarget::new(control.mtm(), action);
    // SAFETY: The target is kept alive by `control` through the associated object and
    // responds to the selector.
    unsafe {
        control.setTarget(Some(&target));
        control.setAction(Some(fire_sel()));
    }
    retain_with(control, &target);
}

/// Makes the menu item call `action`.
pub(super) fn on_menu_action(item: &NSMenuItem, action: impl Fn() + 'static) {
    let target = ActionTarget::new(item.mtm(), move |_| action());
    // SAFETY: The target is kept alive by `item` and responds to the selector.
    unsafe {
        item.setTarget(Some(&target));
        item.setAction(Some(fire_sel()));
    }
    retain_with(item, &target);
}

// ----- Text editing delegate -----

pub(super) struct WatcherIvars {
    changed: Box<dyn Fn(&str)>,
    submitted: Option<Box<dyn Fn()>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and the class has no Drop impl.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WindowsAppTextWatcher"]
    #[ivars = WatcherIvars]
    pub(super) struct TextWatcher;

    // SAFETY: NSObjectProtocol has no safety requirements.
    unsafe impl NSObjectProtocol for TextWatcher {}

    // SAFETY: The method signatures match the protocol.
    unsafe impl NSControlTextEditingDelegate for TextWatcher {
        #[unsafe(method(controlTextDidChange:))]
        fn control_text_did_change(&self, notification: &NSNotification) {
            let text = notification
                .object()
                .and_then(|object| object.downcast::<NSControl>().ok())
                .map(|control| control.stringValue().to_string())
                .unwrap_or_default();
            (self.ivars().changed)(&text);
        }

        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn do_command(&self, _control: &NSControl, _view: &NSTextView, command: Sel) -> bool {
            match &self.ivars().submitted {
                Some(submitted) if command == sel!(insertNewline:) => {
                    submitted();
                    true
                }
                _ => false,
            }
        }
    }

    // SAFETY: The protocol has only optional methods.
    unsafe impl NSTextFieldDelegate for TextWatcher {}

    // SAFETY: The protocol has only optional methods.
    unsafe impl NSSearchFieldDelegate for TextWatcher {}
);

impl TextWatcher {
    /// Calls `changed` with the field's text on every edit and `submitted` (if any) when
    /// Return is pressed. Attach with [`watch_text`].
    fn new(
        mtm: MainThreadMarker,
        changed: impl Fn(&str) + 'static,
        submitted: Option<Box<dyn Fn()>>,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(WatcherIvars {
            changed: Box::new(changed),
            submitted,
        });
        // SAFETY: NSObject's `init` signature is correct.
        unsafe { msg_send![super(this), init] }
    }
}

/// Observes edits of `field` (the watcher becomes its delegate).
pub(super) fn watch_text(
    field: &NSTextField,
    changed: impl Fn(&str) + 'static,
    submitted: Option<Box<dyn Fn()>>,
) {
    let watcher = TextWatcher::new(field.mtm(), changed, submitted);
    let delegate = ProtocolObject::from_ref(&*watcher);
    // SAFETY: The delegate is weak in AppKit; it is kept alive by `field` below.
    unsafe { field.setDelegate(Some(delegate)) };
    retain_with(field, &watcher);
}

/// Observes edits of a search field (the watcher becomes its delegate).
pub(super) fn watch_search(
    field: &NSSearchField,
    changed: impl Fn(&str) + 'static,
    submitted: Option<Box<dyn Fn()>>,
) {
    let watcher = TextWatcher::new(field.mtm(), changed, submitted);
    let delegate = ProtocolObject::from_ref(&*watcher);
    // SAFETY: The delegate is weak in AppKit; it is kept alive by `field` below.
    unsafe { field.setDelegate(Some(delegate)) };
    retain_with(field, &watcher);
}

// ----- Constructors -----

pub(super) fn ns(text: &str) -> Retained<NSString> {
    NSString::from_str(text)
}

pub(super) fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}

/// An SF Symbol.
pub(super) fn symbol(name: &str, description: &str) -> Option<Retained<NSImage>> {
    NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns(name), Some(&ns(description)))
}

/// A single-line label that truncates at the end.
pub(super) fn label(mtm: MainThreadMarker, text: &str) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&ns(text), mtm);
    label.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    label
}

/// A multi-line label of at most `width` points.
pub(super) fn wrapping_label(
    mtm: MainThreadMarker,
    text: &str,
    width: f64,
) -> Retained<NSTextField> {
    let label = NSTextField::wrappingLabelWithString(&ns(text), mtm);
    label.setPreferredMaxLayoutWidth(width);
    label
}

pub(super) fn secondary(label: &NSTextField) {
    label.setTextColor(Some(&NSColor::secondaryLabelColor()));
}

pub(super) fn font(label: &NSTextField, size: f64, bold: bool) {
    let font = if bold {
        NSFont::boldSystemFontOfSize(size)
    } else {
        NSFont::systemFontOfSize(size)
    };
    label.setFont(Some(&font));
}

/// A push button calling `action`.
pub(super) fn button(
    mtm: MainThreadMarker,
    title: &str,
    action: impl Fn() + 'static,
) -> Retained<NSButton> {
    let button = NSButton::new(mtm);
    button.setTitle(&ns(title));
    button.setBezelStyle(objc2_app_kit::NSBezelStyle::Push);
    on_action(&button, move |_| action());
    button.sizeToFit();
    button
}

/// A borderless button showing an SF Symbol.
pub(super) fn symbol_button(
    mtm: MainThreadMarker,
    name: &str,
    description: &str,
    action: impl Fn(&AnyObject) + 'static,
) -> Retained<NSButton> {
    let button = NSButton::new(mtm);
    button.setBordered(false);
    button.setTitle(&ns(""));
    if let Some(image) = symbol(name, description) {
        button.setImage(Some(&image));
    }
    button.setToolTip(Some(&ns(description)));
    on_action(&button, action);
    button
}

/// A checkbox.
pub(super) fn checkbox(mtm: MainThreadMarker, title: &str, checked: bool) -> Retained<NSButton> {
    let button = NSButton::new(mtm);
    button.setButtonType(objc2_app_kit::NSButtonType::Switch);
    button.setTitle(&ns(title));
    button.setState(if checked {
        objc2_app_kit::NSControlStateValueOn
    } else {
        objc2_app_kit::NSControlStateValueOff
    });
    button.sizeToFit();
    button
}

pub(super) fn is_checked(button: &NSButton) -> bool {
    button.state() == objc2_app_kit::NSControlStateValueOn
}

/// A menu item calling `action`.
pub(super) fn menu_item(
    mtm: MainThreadMarker,
    title: &str,
    key: &str,
    action: impl Fn() + 'static,
) -> Retained<NSMenuItem> {
    let item = plain_item(mtm, title, None, key);
    on_menu_action(&item, action);
    item
}

/// A menu item sending `action` through the responder chain (or doing nothing).
pub(super) fn plain_item(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<Sel>,
    key: &str,
) -> Retained<NSMenuItem> {
    // SAFETY: The selector (if any) is a standard responder-chain action.
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &ns(title),
            action,
            &ns(key),
        )
    }
}

/// A disabled caption item (e.g. the account name at the top of a menu).
pub(super) fn caption_item(mtm: MainThreadMarker, title: &str) -> Retained<NSMenuItem> {
    let item = plain_item(mtm, title, None, "");
    item.setEnabled(false);
    item
}

pub(super) fn menu(mtm: MainThreadMarker, title: &str) -> Retained<NSMenu> {
    NSMenu::initWithTitle(NSMenu::alloc(mtm), &ns(title))
}

// ----- Alerts -----

/// An alert with `buttons` (the first is the default; "Cancel" gets Escape).
pub(super) fn alert(
    mtm: MainThreadMarker,
    heading: &str,
    body: &str,
    buttons: &[&str],
) -> Retained<NSAlert> {
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&ns(heading));
    alert.setInformativeText(&ns(body));
    for title in buttons {
        let button = alert.addButtonWithTitle(&ns(title));
        if *title == "Cancel" {
            button.setKeyEquivalent(&ns("\u{1b}"));
        }
    }
    alert
}

/// Marks the alert's button at `index` as destructive (red, not the default).
pub(super) fn destructive(alert: &NSAlert, index: usize) {
    let buttons = alert.buttons();
    if index < buttons.count() {
        let button = buttons.objectAtIndex(index);
        button.setHasDestructiveAction(true);
    }
    alert.setAlertStyle(NSAlertStyle::Warning);
}

/// Index (0-based) of the button that closed an alert.
pub(super) fn button_index(response: NSModalResponse) -> usize {
    usize::try_from(response - objc2_app_kit::NSAlertFirstButtonReturn).unwrap_or(usize::MAX)
}

/// Shows `alert` as a sheet on `window` (or as a modal panel when there is no visible
/// window) and calls `done` with the index of the chosen button.
pub(super) fn present(
    alert: &NSAlert,
    window: Option<&NSWindow>,
    done: impl FnOnce(usize) + 'static,
) {
    match window.filter(|w| w.isVisible()) {
        Some(window) => {
            let done = Cell::new(Some(done));
            let handler = RcBlock::new(move |response: NSModalResponse| {
                if let Some(done) = done.take() {
                    done(button_index(response));
                }
            });
            alert.beginSheetModalForWindow_completionHandler(window, Some(&handler));
        }
        None => {
            let response = alert.runModal();
            done(button_index(response));
        }
    }
}

/// Shows a message with an OK button.
pub(super) fn inform(mtm: MainThreadMarker, window: Option<&NSWindow>, heading: &str, body: &str) {
    let alert = alert(mtm, heading, body, &["OK"]);
    present(&alert, window, |_| {});
}

// ----- System services -----

pub(super) fn pasteboard_text() -> Option<String> {
    let pasteboard = NSPasteboard::generalPasteboard();
    // SAFETY: Reading an immutable extern string constant.
    let kind = unsafe { NSPasteboardTypeString };
    pasteboard.stringForType(kind).map(|s| s.to_string())
}

pub(super) fn set_pasteboard_text(text: &str) {
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();
    // SAFETY: Reading an immutable extern string constant.
    let kind = unsafe { NSPasteboardTypeString };
    if !pasteboard.setString_forType(&ns(text), kind) {
        log::warn!("Could not copy to the clipboard");
    }
}

/// Opens `url` with the default handler.
pub(super) fn open_url(url: &str) {
    match NSURL::URLWithString(&ns(url)) {
        Some(url) => {
            if !NSWorkspace::sharedWorkspace().openURL(&url) {
                log::warn!("Could not open {url:?}");
            }
        }
        None => log::warn!("Invalid URL {url}"),
    }
}
