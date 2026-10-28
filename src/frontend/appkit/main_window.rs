//! The main window: a sidebar source list (library sections and Remote PC groups), the
//! connection grid or list, empty and loading states, and the unified toolbar.

use super::app::{self, Filter, SidebarCounts};
use super::cards::{self, Card};
use super::toolbar;
use super::util::{self, ns, rect};
use crate::frontend::common::library::{self, SectionView};
use crate::frontend::common::text;
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBackingStoreType, NSColor, NSControlTextEditingDelegate,
    NSImageView, NSMenu, NSMenuToolbarItem, NSProgressIndicator, NSProgressIndicatorStyle,
    NSScrollView, NSSearchToolbarItem, NSSegmentSwitchTracking, NSSegmentedControl,
    NSSplitViewController, NSSplitViewItem, NSStackView, NSStackViewGravity, NSTableCellView,
    NSTableColumn, NSTableView, NSTableViewDataSource, NSTableViewDelegate, NSTableViewStyle,
    NSTextAlignment, NSToolbarFlexibleSpaceItemIdentifier, NSToolbarItem,
    NSToolbarSidebarTrackingSeparatorItemIdentifier, NSToolbarToggleSidebarItemIdentifier,
    NSUserInterfaceLayoutOrientation, NSView, NSViewController, NSViewFrameDidChangeNotification,
    NSWindow, NSWindowDelegate, NSWindowStyleMask, NSWindowTitleVisibility,
};
use objc2_foundation::{
    NSArray, NSCopying, NSIndexSet, NSInteger, NSNotification, NSNotificationCenter, NSObject,
    NSObjectProtocol, NSSize, NSString,
};
use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::rc::{Rc, Weak};

/// Grid of tiles or list of rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ViewMode {
    Grid,
    List,
}

impl ViewMode {
    pub(super) fn from_config(value: &str) -> Self {
        if value == "list" {
            Self::List
        } else {
            Self::Grid
        }
    }

    pub(super) fn config_id(self) -> &'static str {
        match self {
            Self::Grid => "grid",
            Self::List => "list",
        }
    }
}

/// Everything the window shows.
pub(super) struct RenderState<'a> {
    pub sections: &'a [SectionView],
    pub mode: ViewMode,
    pub signed_in: bool,
    pub loading: bool,
    pub has_items: bool,
    pub searching: bool,
    pub counts: &'a SidebarCounts,
    pub filter: &'a Filter,
}

const PADDING: f64 = 20.0;
const SPACING: f64 = 16.0;
const MIN_TILE: f64 = 200.0;
const MAX_TILE: f64 = 280.0;

// ----- Flow layout of headers and cards -----

enum Block {
    Header(Retained<NSView>),
    Cards(Vec<Retained<Card>>),
}

#[derive(Default)]
pub(super) struct FlowIvars {
    blocks: RefCell<Vec<Block>>,
    list: Cell<bool>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements and the class has no Drop impl.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "WindowsAppFlowView"]
    #[ivars = FlowIvars]
    struct FlowView;

    // SAFETY: NSObjectProtocol has no safety requirements.
    unsafe impl NSObjectProtocol for FlowView {}

    // SAFETY: The overridden method has the signature AppKit expects.
    impl FlowView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl FlowView {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(FlowIvars::default());
        // SAFETY: NSView's designated initializer has this signature.
        unsafe { msg_send![super(this), initWithFrame: rect(0.0, 0.0, 600.0, 400.0)] }
    }

    fn set_blocks(&self, blocks: Vec<Block>, list: bool) {
        let views: Vec<Retained<NSView>> = blocks
            .iter()
            .flat_map(|block| match block {
                Block::Header(view) => vec![view.clone()],
                Block::Cards(cards) => cards
                    .iter()
                    .map(|card| Retained::into_super(card.clone()))
                    .collect(),
            })
            .collect();
        self.setSubviews(&NSArray::from_retained_slice(&views));
        *self.ivars().blocks.borrow_mut() = blocks;
        self.ivars().list.set(list);
    }

    /// Places the headers and cards for a visible area of `size`.
    fn relayout(&self, size: NSSize) {
        let width = size.width.max(MIN_TILE + 2.0 * PADDING);
        let available = width - 2.0 * PADDING;
        let list = self.ivars().list.get();
        let mut y = PADDING - 8.0;
        for (index, block) in self.ivars().blocks.borrow().iter().enumerate() {
            match block {
                Block::Header(view) => {
                    if index > 0 {
                        y += 12.0;
                    }
                    view.setFrame(rect(PADDING, y, available, cards::HEADER_HEIGHT));
                    y += cards::HEADER_HEIGHT + 6.0;
                }
                Block::Cards(cards) if list => {
                    for card in cards {
                        card.setFrame(rect(PADDING, y, available, cards::ROW_HEIGHT));
                        y += cards::ROW_HEIGHT;
                    }
                }
                Block::Cards(cards) => {
                    // Truncation is intended: whole columns only.
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let columns =
                        (((available + SPACING) / (MIN_TILE + SPACING)).floor() as usize).max(1);
                    #[allow(clippy::cast_precision_loss)]
                    let gaps = SPACING * (columns - 1) as f64;
                    #[allow(clippy::cast_precision_loss)]
                    let tile = ((available - gaps) / columns as f64).min(MAX_TILE);
                    for (i, card) in cards.iter().enumerate() {
                        #[allow(clippy::cast_precision_loss)]
                        let column = (i % columns) as f64;
                        if i > 0 && i % columns == 0 {
                            y += cards::TILE_HEIGHT + SPACING;
                        }
                        card.setFrame(rect(
                            PADDING + column * (tile + SPACING),
                            y,
                            tile,
                            cards::TILE_HEIGHT,
                        ));
                    }
                    if !cards.is_empty() {
                        y += cards::TILE_HEIGHT;
                    }
                }
            }
        }
        y += PADDING;
        self.setFrameSize(NSSize::new(size.width, y.max(size.height)));
    }
}

// ----- Sidebar -----

#[derive(Clone, PartialEq)]
enum SidebarRow {
    Header(&'static str),
    Entry {
        title: String,
        symbol: &'static str,
        count: Option<usize>,
        filter: Filter,
    },
}

#[derive(Default)]
pub(super) struct SidebarIvars {
    rows: RefCell<Vec<SidebarRow>>,
    /// Set while the selection is changed programmatically.
    updating: Cell<bool>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and the class has no Drop impl.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WindowsAppSidebarSource"]
    #[ivars = SidebarIvars]
    struct SidebarSource;

    // SAFETY: NSObjectProtocol has no safety requirements.
    unsafe impl NSObjectProtocol for SidebarSource {}

    // SAFETY: The method signatures match the protocol.
    unsafe impl NSTableViewDataSource for SidebarSource {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn number_of_rows(&self, _table: &NSTableView) -> NSInteger {
            NSInteger::try_from(self.ivars().rows.borrow().len()).unwrap_or(0)
        }
    }

    // SAFETY: NSControlTextEditingDelegate has no required methods.
    unsafe impl NSControlTextEditingDelegate for SidebarSource {}

    // SAFETY: The method signatures match the protocol.
    unsafe impl NSTableViewDelegate for SidebarSource {
        #[unsafe(method_id(tableView:viewForTableColumn:row:))]
        fn view_for_row(
            &self,
            table: &NSTableView,
            _column: Option<&NSTableColumn>,
            row: NSInteger,
        ) -> Option<Retained<NSView>> {
            self.row(row).map(|row| sidebar_cell(table.mtm(), &row))
        }

        #[unsafe(method(tableView:isGroupRow:))]
        fn is_group_row(&self, _table: &NSTableView, row: NSInteger) -> bool {
            matches!(self.row(row), Some(SidebarRow::Header(_)))
        }

        #[unsafe(method(tableView:shouldSelectRow:))]
        fn should_select_row(&self, _table: &NSTableView, row: NSInteger) -> bool {
            matches!(self.row(row), Some(SidebarRow::Entry { .. }))
        }

        #[unsafe(method(tableViewSelectionDidChange:))]
        fn selection_did_change(&self, notification: &NSNotification) {
            if self.ivars().updating.get() {
                return;
            }
            let Some(table) = notification
                .object()
                .and_then(|object| object.downcast::<NSTableView>().ok())
            else {
                return;
            };
            if let Some(SidebarRow::Entry { filter, .. }) = self.row(table.selectedRow()) {
                // Re-rendering reloads the table, so not while AppKit notifies.
                util::defer(move || app::with(|app| app.set_filter(filter)));
            }
        }
    }
);

impl SidebarSource {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(SidebarIvars::default());
        // SAFETY: NSObject's `init` signature is correct.
        unsafe { msg_send![super(this), init] }
    }

    fn row(&self, row: NSInteger) -> Option<SidebarRow> {
        let index = usize::try_from(row).ok()?;
        self.ivars().rows.borrow().get(index).cloned()
    }
}

fn sidebar_rows(counts: &SidebarCounts) -> Vec<SidebarRow> {
    let entry = |title: &str, symbol, count, filter| SidebarRow::Entry {
        title: title.to_string(),
        symbol,
        count,
        filter,
    };
    let mut rows = vec![
        entry("All Connections", "square.grid.2x2", None, Filter::All),
        entry(
            "Favorites",
            "star.fill",
            Some(counts.favorites),
            Filter::Favorites,
        ),
        SidebarRow::Header("Library"),
        entry(
            "Cloud PCs",
            "pc",
            Some(counts.cloud_pcs),
            Filter::Section(library::SECTION_CLOUD_PCS),
        ),
        entry(
            "Azure Virtual Desktop",
            "desktopcomputer",
            Some(counts.avd),
            Filter::Section(library::SECTION_AVD),
        ),
        entry(
            "Remote PCs",
            "display",
            Some(counts.remote_pcs),
            Filter::Section(library::SECTION_REMOTE_PCS),
        ),
    ];
    if !counts.groups.is_empty() {
        rows.push(SidebarRow::Header("Groups"));
        for (group, count) in &counts.groups {
            rows.push(entry(
                group,
                "folder",
                Some(*count),
                Filter::Group(group.clone()),
            ));
        }
    }
    rows
}

fn sidebar_cell(mtm: MainThreadMarker, row: &SidebarRow) -> Retained<NSView> {
    let cell = NSTableCellView::new(mtm);
    cell.setFrame(rect(0.0, 0.0, 200.0, 24.0));
    match row {
        SidebarRow::Header(title) => {
            let label = util::label(mtm, title);
            util::font(&label, 11.0, true);
            util::secondary(&label);
            label.setFrame(rect(4.0, 4.0, 192.0, 16.0));
            label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            cell.addSubview(&label);
            // SAFETY: The label is a subview of the cell.
            unsafe { cell.setTextField(Some(&label)) };
        }
        SidebarRow::Entry {
            title,
            symbol,
            count,
            ..
        } => {
            let image = NSImageView::new(mtm);
            if let Some(symbol) = util::symbol(symbol, title) {
                image.setImage(Some(&symbol));
            }
            image.setFrame(rect(4.0, 3.0, 18.0, 18.0));
            image.setContentTintColor(Some(&NSColor::controlAccentColor()));
            cell.addSubview(&image);
            let label = util::label(mtm, title);
            label.setFrame(rect(28.0, 4.0, 130.0, 16.0));
            label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            cell.addSubview(&label);
            // SAFETY: The views are subviews of the cell.
            unsafe {
                cell.setImageView(Some(&image));
                cell.setTextField(Some(&label));
            }
            if let Some(count) = count {
                let badge = util::label(mtm, &count.to_string());
                util::font(&badge, 11.0, false);
                util::secondary(&badge);
                badge.setAlignment(NSTextAlignment::Right);
                badge.setFrame(rect(160.0, 4.0, 36.0, 16.0));
                badge.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
                cell.addSubview(&badge);
            }
        }
    }
    Retained::into_super(cell)
}

// ----- Window delegate -----

define_class!(
    // SAFETY: NSObject has no subclassing requirements and the class has no Drop impl.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WindowsAppMainWindowDelegate"]
    struct MainWindowDelegate;

    // SAFETY: NSObjectProtocol has no safety requirements.
    unsafe impl NSObjectProtocol for MainWindowDelegate {}

    // SAFETY: The method signatures match the protocol.
    unsafe impl NSWindowDelegate for MainWindowDelegate {
        /// Closing only hides the window; the app keeps running for sessions and
        /// reopening (Dock icon, Window menu).
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, sender: &NSWindow) -> bool {
            sender.orderOut(None);
            false
        }
    }
);

impl MainWindowDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's `init` signature is correct.
        unsafe { msg_send![super(this), init] }
    }
}

// ----- The window -----

/// The empty state: symbol, title, description and actions.
struct EmptyState {
    view: Retained<NSStackView>,
    image: Retained<NSImageView>,
    title: Retained<objc2_app_kit::NSTextField>,
    body: Retained<objc2_app_kit::NSTextField>,
    sign_in: Retained<objc2_app_kit::NSButton>,
    add_remote: Retained<objc2_app_kit::NSButton>,
}

pub(super) struct MainWindow {
    pub window: Retained<NSWindow>,
    table: Retained<NSTableView>,
    sidebar: Retained<SidebarSource>,
    scroll: Retained<NSScrollView>,
    flow: Retained<FlowView>,
    empty: EmptyState,
    loading: Retained<NSStackView>,
    spinner: Retained<NSProgressIndicator>,
    account: Retained<NSMenuToolbarItem>,
    search: Retained<NSSearchToolbarItem>,
    mode: Retained<NSSegmentedControl>,
    observer: RefCell<Option<Retained<ProtocolObject<dyn NSObjectProtocol>>>>,
}

impl Drop for MainWindow {
    fn drop(&mut self) {
        if let Some(observer) = self.observer.take() {
            // SAFETY: The observer was registered with the default center.
            unsafe { NSNotificationCenter::defaultCenter().removeObserver(observer.as_ref()) };
        }
    }
}

fn centered(view: &NSView, parent: &NSView, size: NSSize) {
    let bounds = parent.bounds();
    view.setFrame(rect(
        ((bounds.size.width - size.width) / 2.0).max(0.0),
        ((bounds.size.height - size.height) / 2.0).max(0.0),
        size.width,
        size.height,
    ));
    view.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewMinXMargin
            | NSAutoresizingMaskOptions::ViewMaxXMargin
            | NSAutoresizingMaskOptions::ViewMinYMargin
            | NSAutoresizingMaskOptions::ViewMaxYMargin,
    );
    parent.addSubview(view);
}

fn vertical_stack(mtm: MainThreadMarker, views: &[&NSView], spacing: f64) -> Retained<NSStackView> {
    let stack = NSStackView::new(mtm);
    stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    stack.setAlignment(objc2_app_kit::NSLayoutAttribute::CenterX);
    stack.setSpacing(spacing);
    for view in views {
        stack.addArrangedSubview(view);
    }
    stack
}

fn empty_state(mtm: MainThreadMarker) -> EmptyState {
    let image = NSImageView::new(mtm);
    image.setSymbolConfiguration(Some(
        &objc2_app_kit::NSImageSymbolConfiguration::configurationWithPointSize_weight(
            48.0,
            // SAFETY: Reading an immutable extern constant.
            unsafe { objc2_app_kit::NSFontWeightLight },
        ),
    ));
    image.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
    let title = super::dialogs::heading(mtm, text::EMPTY_TITLE);
    let body = util::wrapping_label(mtm, "", 360.0);
    body.setAlignment(NSTextAlignment::Center);
    util::secondary(&body);
    let sign_in = util::button(mtm, "Sign In…", || app::with(super::app::App::sign_in));
    sign_in.setKeyEquivalent(&ns("\r"));
    let add_remote = util::button(mtm, "Add Remote PC…", || {
        app::with(|app| app.edit_remote_pc(None));
    });
    let import = util::button(mtm, "Import RDP File…", || {
        app::with(super::app::App::import_rdp_file);
    });
    let buttons = NSStackView::new(mtm);
    buttons.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    buttons.setSpacing(8.0);
    for button in [&sign_in, &add_remote, &import] {
        buttons.addView_inGravity(button, NSStackViewGravity::Center);
    }
    let view = vertical_stack(mtm, &[&image, &title, &body, &buttons], 10.0);
    view.setCustomSpacing_afterView(18.0, &body);
    EmptyState {
        view,
        image,
        title,
        body,
        sign_in,
        add_remote,
    }
}

impl MainWindow {
    pub(super) fn new(mtm: MainThreadMarker, mode: ViewMode) -> Rc<Self> {
        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::Resizable
            | NSWindowStyleMask::FullSizeContentView;
        // SAFETY: Standard window creation; released-when-closed is disabled below
        // because the window is owned by `Retained`.
        let window = unsafe {
            let window = NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect(0.0, 0.0, 1040.0, 680.0),
                style,
                NSBackingStoreType::Buffered,
                false,
            );
            window.setReleasedWhenClosed(false);
            window
        };
        window.setTitle(&ns(text::APP_NAME));
        window.setSubtitle(&ns("Ready"));
        window.setTitleVisibility(NSWindowTitleVisibility::Visible);
        window.setMinSize(NSSize::new(640.0, 420.0));
        window.setTabbingMode(objc2_app_kit::NSWindowTabbingMode::Disallowed);

        // Sidebar.
        let sidebar = SidebarSource::new(mtm);
        let table = NSTableView::new(mtm);
        table.setStyle(NSTableViewStyle::SourceList);
        table.setHeaderView(None);
        table.setRowHeight(26.0);
        table.setFloatsGroupRows(false);
        let column = NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), &ns("main"));
        column.setWidth(200.0);
        table.addTableColumn(&column);
        // SAFETY: The source is kept alive by the table (associated object) and the
        // window state.
        unsafe {
            table.setDataSource(Some(ProtocolObject::from_ref(&*sidebar)));
            table.setDelegate(Some(ProtocolObject::from_ref(&*sidebar)));
        }
        util::retain_with(&table, &sidebar);
        let sidebar_scroll = NSScrollView::new(mtm);
        sidebar_scroll.setDocumentView(Some(&table));
        sidebar_scroll.setHasVerticalScroller(true);
        sidebar_scroll.setAutohidesScrollers(true);
        sidebar_scroll.setDrawsBackground(false);
        let sidebar_controller = NSViewController::new(mtm);
        sidebar_controller.setView(&sidebar_scroll);

        // Content.
        let content = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, 800.0, 680.0));
        let flow = FlowView::new(mtm);
        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), content.bounds());
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDrawsBackground(false);
        scroll.setDocumentView(Some(&flow));
        content.addSubview(&scroll);
        let empty = empty_state(mtm);
        centered(&empty.view, &content, NSSize::new(480.0, 260.0));
        let spinner = NSProgressIndicator::new(mtm);
        spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        let loading_label = util::label(mtm, "Loading your connections…");
        util::secondary(&loading_label);
        let loading = vertical_stack(mtm, &[&spinner, &loading_label], 12.0);
        centered(&loading, &content, NSSize::new(300.0, 90.0));
        let content_controller = NSViewController::new(mtm);
        content_controller.setView(&content);

        let split = NSSplitViewController::new(mtm);
        let sidebar_item = NSSplitViewItem::sidebarWithViewController(&sidebar_controller);
        sidebar_item.setMinimumThickness(180.0);
        sidebar_item.setMaximumThickness(320.0);
        sidebar_item.setCanCollapse(true);
        split.addSplitViewItem(&sidebar_item);
        let content_item = NSSplitViewItem::splitViewItemWithViewController(&content_controller);
        content_item.setMinimumThickness(420.0);
        split.addSplitViewItem(&content_item);
        window.setContentViewController(Some(&split));
        window.setContentSize(NSSize::new(1040.0, 680.0));
        window.center();
        window.setFrameAutosaveName(&ns("MainWindow"));

        // Toolbar.
        let add = toolbar::menu_item(mtm, "add", "Add", "plus", &super::menus::add_menu(mtm));
        add.setToolTip(Some(&ns("Add a connection")));
        let segmented = Self::mode_control(mtm, mode);
        let mode_item = toolbar::view_item(mtm, "mode", "View", &segmented);
        toolbar::set_menu_form(&mode_item, "Toggle Grid or List", || {
            app::with(|app| {
                app.set_mode(match app.mode() {
                    ViewMode::Grid => ViewMode::List,
                    ViewMode::List => ViewMode::Grid,
                });
            });
        });
        let refresh = toolbar::button_item(mtm, "refresh", "Refresh", "arrow.clockwise", || {
            app::with(super::app::App::load)
        });
        let account = toolbar::menu_item(
            mtm,
            "account",
            "Account",
            "person.crop.circle",
            &util::menu(mtm, "Account"),
        );
        account.setShowsIndicator(false);
        let search = NSSearchToolbarItem::initWithItemIdentifier(
            NSSearchToolbarItem::alloc(mtm),
            &ns("search"),
        );
        let field = search.searchField();
        field.setPlaceholderString(Some(&ns("Search or enter a PC address")));
        util::watch_search(
            &field,
            |text| app::with(|app| app.set_search(text)),
            Some(Box::new(|| {
                app::with(|app| {
                    let text = app.search_text();
                    if !text.trim().is_empty() {
                        app.connect_from_search(&text);
                    }
                });
            })),
        );
        // SAFETY: Reading immutable extern string constants.
        let (toggle, tracking, flexible) = unsafe {
            (
                NSToolbarToggleSidebarItemIdentifier,
                NSToolbarSidebarTrackingSeparatorItemIdentifier,
                NSToolbarFlexibleSpaceItemIdentifier,
            )
        };
        let order: Vec<Retained<NSString>> = vec![
            toggle.copy(),
            tracking.copy(),
            ns("add"),
            flexible.copy(),
            ns("mode"),
            ns("refresh"),
            ns("account"),
            ns("search"),
        ];
        let items: Vec<Retained<NSToolbarItem>> = vec![
            Retained::into_super(add),
            mode_item,
            refresh,
            Retained::into_super(account.clone()),
            Retained::into_super(search.clone()),
        ];
        toolbar::install(&window, "MainToolbar", items, order);

        let delegate = MainWindowDelegate::new(mtm);
        window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        util::retain_with(&window, &delegate);

        let this = Rc::new(Self {
            window,
            table,
            sidebar,
            scroll,
            flow,
            empty,
            loading,
            spinner,
            account,
            search,
            mode: segmented,
            observer: RefCell::new(None),
        });
        this.observe_resizes();
        this
    }

    fn mode_control(mtm: MainThreadMarker, mode: ViewMode) -> Retained<NSSegmentedControl> {
        let images: Vec<Retained<objc2_app_kit::NSImage>> =
            [("square.grid.2x2", "Grid"), ("list.bullet", "List")]
                .iter()
                .filter_map(|(name, label)| util::symbol(name, label))
                .collect();
        // SAFETY: No target or action; one is attached with `on_action` below.
        let control = unsafe {
            NSSegmentedControl::segmentedControlWithImages_trackingMode_target_action(
                &NSArray::from_retained_slice(&images),
                NSSegmentSwitchTracking::SelectOne,
                None,
                None,
                mtm,
            )
        };
        control.setToolTip_forSegment(Some(&ns("View as Grid")), 0);
        control.setToolTip_forSegment(Some(&ns("View as List")), 1);
        control.setSelectedSegment(match mode {
            ViewMode::Grid => 0,
            ViewMode::List => 1,
        });
        util::on_action(&control, |sender: &AnyObject| {
            let Some(control) = sender.downcast_ref::<NSSegmentedControl>() else {
                return;
            };
            let mode = if control.selectedSegment() == 1 {
                ViewMode::List
            } else {
                ViewMode::Grid
            };
            app::with(|app| app.set_mode(mode));
        });
        control
    }

    /// Re-flows the cards whenever the visible area changes size.
    fn observe_resizes(self: &Rc<Self>) {
        self.scroll.setPostsFrameChangedNotifications(true);
        let weak: Weak<Self> = Rc::downgrade(self);
        let block = RcBlock::new(move |_: NonNull<NSNotification>| {
            if let Some(this) = weak.upgrade() {
                this.relayout();
            }
        });
        // SAFETY: Reading an immutable extern string constant; the block is retained
        // by the notification center and removed again in `Drop`.
        let observer = unsafe {
            NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
                Some(NSViewFrameDidChangeNotification),
                Some(&self.scroll),
                None,
                &block,
            )
        };
        *self.observer.borrow_mut() = Some(observer);
    }

    fn relayout(&self) {
        self.flow.relayout(self.scroll.contentSize());
    }

    pub(super) fn present(&self) {
        self.window.makeKeyAndOrderFront(None);
        objc2_app_kit::NSApplication::sharedApplication(self.window.mtm()).activate();
    }

    /// Shows `message` as the window subtitle.
    pub(super) fn set_status(&self, message: &str) {
        self.window.setSubtitle(&ns(message));
    }

    pub(super) fn set_account_menu(&self, menu: &NSMenu) {
        self.account.setMenu(menu);
    }

    /// Focuses the search field.
    pub(super) fn focus_search(&self) {
        self.search.beginSearchInteraction();
    }

    pub(super) fn render(&self, state: &RenderState<'_>) {
        let mtm = self.window.mtm();
        self.render_sidebar(state);
        self.mode.setSelectedSegment(match state.mode {
            ViewMode::Grid => 0,
            ViewMode::List => 1,
        });

        let total: usize = state.sections.iter().map(|s| s.cards.len()).sum();
        let show_loading = state.loading && !state.has_items;
        let show_empty = !show_loading && total == 0;
        self.loading.setHidden(!show_loading);
        // SAFETY: Animating a spinner on the main thread.
        unsafe {
            if show_loading {
                self.spinner.startAnimation(None);
            } else {
                self.spinner.stopAnimation(None);
            }
        }
        self.empty.view.setHidden(!show_empty);
        self.scroll.setHidden(show_empty || show_loading);
        if show_empty {
            self.render_empty(state);
        }

        let collapsible = matches!(state.filter, Filter::All) && !state.searching;
        let mut blocks = Vec::new();
        for section in state.sections {
            if section.cards.is_empty() {
                continue;
            }
            blocks.push(Block::Header(cards::section_header(
                mtm,
                section.id,
                section.title,
                section.cards.len(),
                section.expanded,
                collapsible,
            )));
            if section.expanded {
                blocks.push(Block::Cards(
                    section
                        .cards
                        .iter()
                        .map(|card| Card::new(mtm, card.clone(), state.mode))
                        .collect(),
                ));
            }
        }
        self.flow.set_blocks(blocks, state.mode == ViewMode::List);
        self.relayout();
    }

    fn render_empty(&self, state: &RenderState<'_>) {
        let empty = &self.empty;
        let (symbol, title, body) = if state.searching {
            let search = app::get().map(|app| app.search_text()).unwrap_or_default();
            (
                "magnifyingglass",
                "No Results".to_string(),
                library::search_status(&search, 0),
            )
        } else if state.has_items {
            (
                "tray",
                "Nothing Here".to_string(),
                "This category has no connections.".to_string(),
            )
        } else {
            (
                "desktopcomputer",
                text::EMPTY_TITLE.to_string(),
                text::empty_description(state.signed_in).to_string(),
            )
        };
        if let Some(image) = util::symbol(symbol, &title) {
            empty.image.setImage(Some(&image));
        }
        empty.title.setStringValue(&ns(&title));
        empty.body.setStringValue(&ns(&body));
        let first_run = !state.searching && !state.has_items;
        empty.sign_in.setHidden(!first_run || state.signed_in);
        empty.add_remote.setHidden(state.searching);
    }

    fn render_sidebar(&self, state: &RenderState<'_>) {
        let rows = sidebar_rows(state.counts);
        let ivars = self.sidebar.ivars();
        ivars.updating.set(true);
        if *ivars.rows.borrow() != rows {
            *ivars.rows.borrow_mut() = rows;
            self.table.reloadData();
        }
        let selected = ivars.rows.borrow().iter().position(
            |row| matches!(row, SidebarRow::Entry { filter, .. } if filter == state.filter),
        );
        if let Some(index) = selected {
            if usize::try_from(self.table.selectedRow()).ok() != Some(index) {
                self.table.selectRowIndexes_byExtendingSelection(
                    &NSIndexSet::indexSetWithIndex(index),
                    false,
                );
            }
        }
        ivars.updating.set(false);
    }
}
