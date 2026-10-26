//! Window toolbars: a delegate serving prebuilt items, and item constructors.

use super::util::{ns, on_menu_action, retain_with, symbol, ActionTarget};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSMenu, NSMenuToolbarItem, NSToolbar, NSToolbarDelegate, NSToolbarDisplayMode, NSToolbarItem,
    NSView, NSWindow, NSWindowToolbarStyle,
};
use objc2_foundation::{NSArray, NSObject, NSObjectProtocol, NSString};

pub(super) struct ToolbarIvars {
    items: Vec<Retained<NSToolbarItem>>,
    order: Vec<Retained<NSString>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and the class has no Drop impl.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WindowsAppToolbarDelegate"]
    #[ivars = ToolbarIvars]
    pub(super) struct ToolbarDelegate;

    // SAFETY: NSObjectProtocol has no safety requirements.
    unsafe impl NSObjectProtocol for ToolbarDelegate {}

    // SAFETY: The method signatures match the protocol.
    unsafe impl NSToolbarDelegate for ToolbarDelegate {
        #[unsafe(method_id(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:))]
        fn item_for_identifier(
            &self,
            _toolbar: &NSToolbar,
            identifier: &NSString,
            _will_insert: bool,
        ) -> Option<Retained<NSToolbarItem>> {
            self.ivars()
                .items
                .iter()
                .find(|item| &*item.itemIdentifier() == identifier)
                .cloned()
        }

        #[unsafe(method_id(toolbarDefaultItemIdentifiers:))]
        fn default_identifiers(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSString>> {
            NSArray::from_retained_slice(&self.ivars().order)
        }

        #[unsafe(method_id(toolbarAllowedItemIdentifiers:))]
        fn allowed_identifiers(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSString>> {
            NSArray::from_retained_slice(&self.ivars().order)
        }
    }
);

/// Installs a unified toolbar showing `order` (custom identifiers are served from `items`,
/// system identifiers such as the flexible space are created by AppKit).
pub(super) fn install(
    window: &NSWindow,
    identifier: &str,
    items: Vec<Retained<NSToolbarItem>>,
    order: Vec<Retained<NSString>>,
) {
    let mtm = window.mtm();
    let delegate = ToolbarDelegate::alloc(mtm).set_ivars(ToolbarIvars { items, order });
    // SAFETY: NSObject's `init` signature is correct.
    let delegate: Retained<ToolbarDelegate> = unsafe { msg_send![super(delegate), init] };
    let toolbar = NSToolbar::initWithIdentifier(NSToolbar::alloc(mtm), &ns(identifier));
    toolbar.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    toolbar.setDisplayMode(NSToolbarDisplayMode::IconOnly);
    toolbar.setAllowsUserCustomization(false);
    window.setToolbar(Some(&toolbar));
    window.setToolbarStyle(NSWindowToolbarStyle::Unified);
    // The toolbar's delegate is a weak reference.
    retain_with(window, &delegate);
}

/// A button item calling `action`.
pub(super) fn button_item(
    mtm: MainThreadMarker,
    identifier: &str,
    label: &str,
    symbol_name: &str,
    action: impl Fn() + 'static,
) -> Retained<NSToolbarItem> {
    let item = NSToolbarItem::initWithItemIdentifier(NSToolbarItem::alloc(mtm), &ns(identifier));
    item.setLabel(&ns(label));
    item.setPaletteLabel(&ns(label));
    item.setToolTip(Some(&ns(label)));
    item.setImage(symbol(symbol_name, label).as_deref());
    item.setBordered(true);
    let target = ActionTarget::new(mtm, move |_| action());
    // SAFETY: The target is kept alive by the item and responds to the selector.
    unsafe {
        item.setTarget(Some(&target));
        item.setAction(Some(super::util::fire_sel()));
    }
    retain_with(&item, &target);
    item
}

/// An item showing `menu` when clicked.
pub(super) fn menu_item(
    mtm: MainThreadMarker,
    identifier: &str,
    label: &str,
    symbol_name: &str,
    menu: &NSMenu,
) -> Retained<NSMenuToolbarItem> {
    let item =
        NSMenuToolbarItem::initWithItemIdentifier(NSMenuToolbarItem::alloc(mtm), &ns(identifier));
    item.setLabel(&ns(label));
    item.setPaletteLabel(&ns(label));
    item.setToolTip(Some(&ns(label)));
    item.setImage(symbol(symbol_name, label).as_deref());
    item.setMenu(menu);
    item.setShowsIndicator(true);
    item
}

/// An item hosting `view`.
pub(super) fn view_item(
    mtm: MainThreadMarker,
    identifier: &str,
    label: &str,
    view: &NSView,
) -> Retained<NSToolbarItem> {
    let item = NSToolbarItem::initWithItemIdentifier(NSToolbarItem::alloc(mtm), &ns(identifier));
    item.setLabel(&ns(label));
    item.setPaletteLabel(&ns(label));
    item.setView(Some(view));
    item
}

/// The overflow menu entry of an item (shown when the toolbar is too narrow).
pub(super) fn set_menu_form(item: &NSToolbarItem, title: &str, action: impl Fn() + 'static) {
    let entry = super::util::plain_item(item.mtm(), title, None, "");
    on_menu_action(&entry, action);
    item.setMenuFormRepresentation(Some(&entry));
}
