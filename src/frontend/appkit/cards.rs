//! Connection cards (grid tiles and list rows) and section headers of the main window.

use super::app;
use super::main_window::ViewMode;
use super::util::{self, ns, rect};
use crate::frontend::common::library::{CardView, ItemAction, ItemIcon};
use crate::models::StatusTone;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAccessibility, NSAttributedStringNSStringDrawing, NSAutoresizingMaskOptions, NSBezelStyle,
    NSBezierPath, NSButton, NSColor, NSEvent, NSFontWeightLight, NSImageScaling,
    NSImageSymbolConfiguration, NSImageView, NSMenu, NSProgressIndicator, NSProgressIndicatorStyle,
    NSTextField, NSTrackingArea, NSTrackingAreaOptions, NSView,
};
use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect};
use std::cell::Cell;

/// Height of a grid tile.
pub(super) const TILE_HEIGHT: f64 = 214.0;
/// Height of a list row.
pub(super) const ROW_HEIGHT: f64 = 56.0;
/// Height of a section header.
pub(super) const HEADER_HEIGHT: f64 = 28.0;

const PAD: f64 = 12.0;
const THUMB_HEIGHT: f64 = 110.0;
const STATUS_DOT: f64 = 7.0;

pub(super) struct CardIvars {
    card: CardView,
    mode: ViewMode,
    hovering: Cell<bool>,
    play: Retained<NSButton>,
    spinner: Retained<NSProgressIndicator>,
    /// Frame of the status text, for the coloured dot drawn before it.
    status: Option<Retained<NSTextField>>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements and the class has no Drop impl.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "WindowsAppCardView"]
    #[ivars = CardIvars]
    pub(super) struct Card;

    // SAFETY: NSObjectProtocol has no safety requirements.
    unsafe impl NSObjectProtocol for Card {}

    // SAFETY: The overridden methods have the signatures AppKit expects.
    impl Card {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.draw();
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            if event.clickCount() == 2 {
                self.connect();
            }
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _event: &NSEvent) {
            self.set_hovering(true);
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            self.set_hovering(false);
        }

        #[unsafe(method_id(menuForEvent:))]
        fn menu_for_event(&self, _event: &NSEvent) -> Option<Retained<NSMenu>> {
            self.menu()
        }
    }
);

impl Card {
    /// A tile (grid) or row (list) for `card`.
    pub(super) fn new(mtm: MainThreadMarker, card: CardView, mode: ViewMode) -> Retained<Self> {
        let width = 240.0;
        let height = match mode {
            ViewMode::Grid => TILE_HEIGHT,
            ViewMode::List => ROW_HEIGHT,
        };
        let play = NSButton::new(mtm);
        play.setBezelStyle(NSBezelStyle::Circular);
        play.setTitle(&ns(""));
        if let Some(image) = util::symbol("play.fill", "Connect") {
            play.setImage(Some(&image));
        }
        play.setToolTip(Some(&ns(card.tooltip)));
        play.setHidden(true);
        let spinner = NSProgressIndicator::new(mtm);
        spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        spinner.setDisplayedWhenStopped(false);
        let status_text = match (&card.status, &card.last_used) {
            (Some((status, _)), _) => Some(status.clone()),
            (None, Some(last_used)) => Some(format!("Last used {last_used}")),
            (None, None) => None,
        };
        let status = status_text.map(|text| {
            let label = util::label(mtm, &text);
            util::font(&label, 11.0, false);
            util::secondary(&label);
            label
        });
        let this = Self::alloc(mtm).set_ivars(CardIvars {
            card,
            mode,
            hovering: Cell::new(false),
            play,
            spinner,
            status,
        });
        // SAFETY: NSView's designated initializer has this signature.
        let this: Retained<Self> =
            unsafe { msg_send![super(this), initWithFrame: rect(0.0, 0.0, width, height)] };
        match mode {
            ViewMode::Grid => this.build_tile(width),
            ViewMode::List => this.build_row(width),
        }
        this.finish();
        this
    }

    fn card(&self) -> &CardView {
        &self.ivars().card
    }

    fn icon_name(&self) -> &'static str {
        match self.card().icon {
            ItemIcon::Windows => "pc",
            ItemIcon::Desktop => "desktopcomputer",
            ItemIcon::App => "app.badge",
            ItemIcon::RemotePc => "display",
        }
    }

    fn icon(&self, frame: NSRect, size: f64) -> Retained<NSImageView> {
        let mtm = self.mtm();
        let view = NSImageView::new(mtm);
        view.setFrame(frame);
        if let Some(image) = util::symbol(self.icon_name(), &self.card().kind) {
            view.setImage(Some(&image));
        }
        // SAFETY: Reading an immutable extern constant.
        let weight = unsafe { NSFontWeightLight };
        view.setSymbolConfiguration(Some(
            &NSImageSymbolConfiguration::configurationWithPointSize_weight(size, weight),
        ));
        view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
        view.setContentTintColor(Some(&NSColor::controlAccentColor()));
        view
    }

    fn place(&self, view: &NSView, frame: NSRect, mask: NSAutoresizingMaskOptions) {
        view.setFrame(frame);
        view.setAutoresizingMask(mask);
        self.addSubview(view);
    }

    fn build_tile(&self, width: f64) {
        let mtm = self.mtm();
        let card = self.card();
        let fixed_right = NSAutoresizingMaskOptions::ViewMinXMargin;
        let stretch = NSAutoresizingMaskOptions::ViewWidthSizable;
        let thumb = rect(PAD, PAD, width - 2.0 * PAD, THUMB_HEIGHT);
        self.place(&self.icon(thumb, 40.0), thumb, stretch);

        let button = 36.0;
        let corner = rect(
            width - PAD - 8.0 - button,
            PAD + THUMB_HEIGHT - 8.0 - button,
            button,
            button,
        );
        self.place(&self.ivars().play, corner, fixed_right);
        let spinner_frame = rect(corner.origin.x + 6.0, corner.origin.y + 6.0, 24.0, 24.0);
        self.place(&self.ivars().spinner, spinner_frame, fixed_right);

        let mut y = PAD + THUMB_HEIGHT + 10.0;
        let text_width = width - 2.0 * PAD - 28.0;
        let title = util::label(mtm, &card.title);
        util::font(&title, 13.0, true);
        self.place(&title, rect(PAD, y, text_width, 18.0), stretch);
        y += 19.0;
        let mut kind = card.kind.clone();
        if let Some(account) = &card.account {
            kind = format!("{kind} • {account}");
        }
        for text in [kind, format!("{}{}", card.detail_prefix, card.detail)] {
            let label = util::label(mtm, &text);
            util::font(&label, 11.0, false);
            util::secondary(&label);
            self.place(&label, rect(PAD, y, text_width, 15.0), stretch);
            y += 16.0;
        }
        if let Some(status) = &self.ivars().status {
            let indent = if card.status.is_some() {
                STATUS_DOT + 5.0
            } else {
                0.0
            };
            self.place(
                status,
                rect(PAD + indent, y, text_width - indent, 15.0),
                stretch,
            );
        }
        let more = self.more_button();
        self.place(
            &more,
            rect(width - PAD - 24.0, PAD + THUMB_HEIGHT + 8.0, 24.0, 22.0),
            fixed_right,
        );
        if card.is_favorite {
            let item = card.item.clone();
            let star = util::symbol_button(mtm, "star.fill", "Favorite", move |_| {
                app::with(|app| app.item_action(item.clone(), ItemAction::ToggleFavorite));
            });
            self.place(
                &star,
                rect(width - PAD - 50.0, PAD + THUMB_HEIGHT + 8.0, 24.0, 22.0),
                fixed_right,
            );
        }
    }

    fn build_row(&self, width: f64) {
        let mtm = self.mtm();
        let card = self.card();
        let fixed_right = NSAutoresizingMaskOptions::ViewMinXMargin;
        let stretch = NSAutoresizingMaskOptions::ViewWidthSizable;
        let icon = rect(PAD, 12.0, 32.0, 32.0);
        self.place(
            &self.icon(icon, 22.0),
            icon,
            NSAutoresizingMaskOptions::ViewNotSizable,
        );

        let right = 24.0 + 8.0 + 28.0 + 8.0 + 170.0;
        let text_x = PAD + 32.0 + 12.0;
        let text_width = (width - text_x - PAD - right).max(40.0);
        let title = util::label(mtm, &card.title);
        util::font(&title, 13.0, false);
        self.place(&title, rect(text_x, 9.0, text_width, 18.0), stretch);
        let subtitle = util::label(mtm, &card.subtitle().replace('\n', " • "));
        util::font(&subtitle, 11.0, false);
        util::secondary(&subtitle);
        self.place(&subtitle, rect(text_x, 29.0, text_width, 16.0), stretch);

        let more = self.more_button();
        let more_x = width - PAD - 24.0;
        self.place(&more, rect(more_x, 16.0, 24.0, 24.0), fixed_right);
        if card.is_favorite {
            let item = card.item.clone();
            let star = util::symbol_button(mtm, "star.fill", "Favorite", move |_| {
                app::with(|app| app.item_action(item.clone(), ItemAction::ToggleFavorite));
            });
            self.place(&star, rect(more_x - 30.0, 16.0, 24.0, 24.0), fixed_right);
        }
        let play_x = more_x - 8.0 - 28.0;
        self.place(
            &self.ivars().play,
            rect(play_x, 14.0, 28.0, 28.0),
            fixed_right,
        );
        self.place(
            &self.ivars().spinner,
            rect(play_x + 6.0, 20.0, 16.0, 16.0),
            fixed_right,
        );
        self.ivars()
            .spinner
            .setControlSize(objc2_app_kit::NSControlSize::Small);
        if let Some(status) = &self.ivars().status {
            status.setAlignment(objc2_app_kit::NSTextAlignment::Right);
            self.place(
                status,
                rect(play_x - 8.0 - 170.0, 20.0, 170.0, 16.0),
                fixed_right,
            );
        }
    }

    fn finish(&self) {
        let card = self.card();
        let item = card.item.clone();
        util::on_action(&self.ivars().play, move |_| {
            let item = item.clone();
            app::with(|app| app.item_action(item, ItemAction::Connect));
        });
        if card.busy {
            // SAFETY: Starting the animation of a spinner on the main thread.
            unsafe { self.ivars().spinner.startAnimation(None) };
        }
        self.setToolTip(Some(&ns(&card.subtitle())));
        self.setAccessibilityElement(true);
        self.setAccessibilityLabel(Some(&ns(&format!("{}, {}", card.title, card.kind))));
        let options = NSTrackingAreaOptions::MouseEnteredAndExited
            | NSTrackingAreaOptions::ActiveInKeyWindow
            | NSTrackingAreaOptions::InVisibleRect;
        // SAFETY: The owner is this view, which outlives its own tracking area.
        let area = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                NSRect::ZERO,
                options,
                Some(self),
                None,
            )
        };
        self.addTrackingArea(&area);
    }

    /// The "…" button showing the item's menu.
    fn more_button(&self) -> Retained<NSButton> {
        let item = self.card().item.clone();
        util::symbol_button(
            self.mtm(),
            "ellipsis.circle",
            "More Actions",
            move |sender| {
                let Some(app) = app::get() else { return };
                let Some(button) = sender.downcast_ref::<NSButton>() else {
                    return;
                };
                let menu = super::menus::item_menu(&app, &item);
                let bounds = button.bounds();
                let origin = NSPoint::new(0.0, bounds.size.height + 4.0);
                menu.popUpMenuPositioningItem_atLocation_inView(None, origin, Some(button));
            },
        )
    }

    fn menu(&self) -> Option<Retained<NSMenu>> {
        let app = app::get()?;
        Some(super::menus::item_menu(&app, &self.card().item))
    }

    fn connect(&self) {
        let item = self.card().item.clone();
        util::defer(move || app::with(|app| app.item_action(item, ItemAction::Connect)));
    }

    fn set_hovering(&self, hovering: bool) {
        self.ivars().hovering.set(hovering);
        self.ivars().play.setHidden(!hovering || self.card().busy);
        self.setNeedsDisplay(true);
    }

    fn draw(&self) {
        let bounds = self.bounds();
        let hovering = self.ivars().hovering.get();
        match self.ivars().mode {
            ViewMode::Grid => {
                let outline = bounds_inset(bounds, 0.5);
                let path =
                    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(outline, 12.0, 12.0);
                NSColor::controlBackgroundColor().setFill();
                path.fill();
                let border = if hovering {
                    NSColor::controlAccentColor()
                } else {
                    NSColor::separatorColor()
                };
                border.setStroke();
                path.setLineWidth(1.0);
                path.stroke();
                let thumb = rect(PAD, PAD, bounds.size.width - 2.0 * PAD, THUMB_HEIGHT);
                let thumb_path =
                    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(thumb, 8.0, 8.0);
                NSColor::quaternaryLabelColor().setFill();
                thumb_path.fill();
            }
            ViewMode::List => {
                if hovering {
                    let path =
                        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(bounds, 6.0, 6.0);
                    NSColor::quaternaryLabelColor().setFill();
                    path.fill();
                }
                let line = rect(
                    PAD + 44.0,
                    bounds.size.height - 1.0,
                    bounds.size.width - PAD - 44.0,
                    1.0,
                );
                NSColor::separatorColor().setFill();
                NSBezierPath::fillRect(line);
            }
        }
        if let (Some((_, tone)), Some(status)) = (&self.card().status, &self.ivars().status) {
            let frame = status.frame();
            let dot = rect(
                frame.origin.x - STATUS_DOT - 5.0,
                frame.origin.y + (frame.size.height - STATUS_DOT) / 2.0,
                STATUS_DOT,
                STATUS_DOT,
            );
            let dot = if self.ivars().mode == ViewMode::List {
                // Right-aligned text: the dot goes before the measured text.
                let text = status.attributedStringValue().size().width;
                rect(
                    frame.origin.x + frame.size.width - text - STATUS_DOT - 5.0,
                    dot.origin.y,
                    STATUS_DOT,
                    STATUS_DOT,
                )
            } else {
                dot
            };
            tone_color(*tone).setFill();
            NSBezierPath::bezierPathWithOvalInRect(dot).fill();
        }
    }
}

fn bounds_inset(bounds: NSRect, inset: f64) -> NSRect {
    rect(
        bounds.origin.x + inset,
        bounds.origin.y + inset,
        bounds.size.width - 2.0 * inset,
        bounds.size.height - 2.0 * inset,
    )
}

fn tone_color(tone: StatusTone) -> Retained<NSColor> {
    match tone {
        StatusTone::Good => NSColor::systemGreenColor(),
        StatusTone::Neutral => NSColor::secondaryLabelColor(),
        StatusTone::Warning => NSColor::systemOrangeColor(),
        StatusTone::Error => NSColor::systemRedColor(),
    }
}

/// A section header: a disclosure button with the title and the number of items.
pub(super) fn section_header(
    mtm: MainThreadMarker,
    id: &'static str,
    title: &str,
    count: usize,
    expanded: bool,
    collapsible: bool,
) -> Retained<NSView> {
    let header = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, 400.0, HEADER_HEIGHT));
    let title_button = NSButton::new(mtm);
    title_button.setBordered(false);
    title_button.setTitle(&ns(title));
    title_button.setFont(Some(&objc2_app_kit::NSFont::boldSystemFontOfSize(13.0)));
    if collapsible {
        let chevron = if expanded {
            "chevron.down"
        } else {
            "chevron.right"
        };
        if let Some(image) = util::symbol(chevron, if expanded { "Collapse" } else { "Expand" }) {
            title_button.setImage(Some(&image));
        }
        title_button.setImagePosition(objc2_app_kit::NSCellImagePosition::ImageLeading);
        title_button.setImageHugsTitle(true);
        title_button.setToolTip(Some(&ns(if expanded {
            "Hide these connections"
        } else {
            "Show these connections"
        })));
        util::on_action(&title_button, move |_: &AnyObject| {
            app::with(|app| app.set_section_collapsed(id, expanded));
        });
        let state = ns(if expanded { "Expanded" } else { "Collapsed" });
        let state: &AnyObject = &state;
        // SAFETY: The accessibility value of a button is a string.
        unsafe { title_button.setAccessibilityValue(Some(state)) };
    } else {
        title_button.setEnabled(false);
    }
    title_button.sizeToFit();
    let size = title_button.frame().size;
    title_button.setFrame(rect(
        0.0,
        (HEADER_HEIGHT - size.height) / 2.0,
        size.width,
        size.height,
    ));
    header.addSubview(&title_button);
    let count_label = util::label(mtm, &count.to_string());
    util::font(&count_label, 11.0, false);
    util::secondary(&count_label);
    count_label.sizeToFit();
    let count_size = count_label.frame().size;
    count_label.setFrame(rect(
        size.width + 6.0,
        (HEADER_HEIGHT - count_size.height) / 2.0,
        count_size.width,
        count_size.height,
    ));
    header.addSubview(&count_label);
    header
}
