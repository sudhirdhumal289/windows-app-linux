//! Views of the main window: navigation items, connection tiles and list rows, the
//! collapsible library sections, the empty state and the account button.
//!
//! These are plain functions over the main component's view context; the state lives in
//! [`super::app::MainApp`].

use windows_reactor::*;

use super::app::{MainApp, Msg};
use crate::frontend::common::library::{
    CardView, Item, ItemAction, ItemIcon, MenuItem as LibraryMenuItem, SectionView, SECTION_AVD,
    SECTION_CLOUD_PCS, SECTION_REMOTE_PCS,
};
use crate::frontend::common::text::{empty_description, EMPTY_TITLE};
use crate::models::StatusTone;

pub const NAV_ALL: &str = "all";
pub const NAV_FAVORITES: &str = "favorites";
pub const NAV_SETTINGS: &str = "settings";

pub const ADD_CLOUD: &str = "Cloud PC or Azure Virtual Desktop…";
pub const ADD_REMOTE: &str = "Remote PC…";
pub const ADD_IMPORT: &str = "Import .rdp File…";

const NAV_PAGES: [(&str, &str, &str); 5] = [
    (NAV_ALL, "All connections", "\u{E80F}"),
    (NAV_FAVORITES, "Favorites", "\u{E735}"),
    (SECTION_CLOUD_PCS, "Cloud PCs", "\u{E753}"),
    (SECTION_AVD, "Azure Virtual Desktop", "\u{E7F4}"),
    (SECTION_REMOTE_PCS, "Remote PCs", "\u{E703}"),
];

const TILE_WIDTH: f64 = 288.0;
const TILE_HEIGHT: f64 = 132.0;
const TILE_GAP: f64 = 8.0;

/// Title of the page behind navigation tag `tag`.
pub fn page_title(tag: &str) -> &'static str {
    if tag == NAV_SETTINGS {
        return "Settings";
    }
    NAV_PAGES
        .iter()
        .find(|(t, _, _)| *t == tag)
        .map_or("All connections", |(_, title, _)| title)
}

/// The navigation pane's items; Settings comes last.
pub fn nav_items(selected: &str) -> Vec<(String, View)> {
    NAV_PAGES
        .iter()
        .map(|(tag, label, glyph)| (*tag, *label, *glyph))
        .chain(std::iter::once((NAV_SETTINGS, "Settings", "\u{E713}")))
        .map(|(tag, label, glyph)| {
            (
                tag.to_string(),
                NavigationViewItem::new()
                    .tag(tag)
                    .is_selected(tag == selected)
                    .slots([
                        SlotView::new(NavigationViewItemSlot::Icon, FontIcon::new().glyph(glyph)),
                        SlotView::new(NavigationViewItemSlot::Content, label),
                    ]),
            )
        })
        .collect()
}

/// The Add menu (title bar button and empty state).
pub fn add_menu(ctx: &ViewContext<MainApp>) -> Menu {
    Menu::new(
        [
            windows_reactor::MenuItem::item("cloud", ADD_CLOUD),
            windows_reactor::MenuItem::item("remote", ADD_REMOTE),
            windows_reactor::MenuItem::separator("sep"),
            windows_reactor::MenuItem::item("import", ADD_IMPORT),
        ],
        ctx.callback(Msg::AddMenu),
    )
}

/// The flyout entries of `item`. Desktop integration toggles are Linux-only and left out;
/// separators are tidied up afterwards.
pub fn item_menu(item: &Item, busy: bool) -> Vec<windows_reactor::MenuItem> {
    let mut entries = Vec::new();
    let mut pending_separator = false;
    for (index, entry) in item.menu(false, false, busy).into_iter().enumerate() {
        match entry {
            LibraryMenuItem::Action { action, enabled } => {
                if pending_separator && !entries.is_empty() {
                    entries.push(windows_reactor::MenuItem::separator(format!("sep{index}")));
                }
                pending_separator = false;
                let label = action.label();
                entries.push(if enabled {
                    windows_reactor::MenuItem::item(label, label)
                } else {
                    windows_reactor::MenuItem::disabled(label, label)
                });
            }
            LibraryMenuItem::Separator => pending_separator = true,
            LibraryMenuItem::Toggle { .. } => {}
        }
    }
    entries
}

/// The action behind a menu label from [`item_menu`].
pub fn action_from_label(label: &str) -> Option<ItemAction> {
    const ACTIONS: [ItemAction; 19] = [
        ItemAction::Connect,
        ItemAction::Start,
        ItemAction::Stop,
        ItemAction::Restart,
        ItemAction::Rename,
        ItemAction::OpenInBrowser,
        ItemAction::Troubleshoot,
        ItemAction::Inspect,
        ItemAction::Details,
        ItemAction::Restore,
        ItemAction::Reset,
        ItemAction::Edit,
        ItemAction::Export,
        ItemAction::ForgetPassword,
        ItemAction::Remove,
        ItemAction::ShowInLauncher,
        ItemAction::ConnectAtLogin,
        ItemAction::ConnectionSettings,
        ItemAction::Preferences,
        ItemAction::ToggleFavorite,
    ];
    ACTIONS.into_iter().find(|action| action.label() == label)
}

fn glyph(icon: ItemIcon) -> &'static str {
    match icon {
        ItemIcon::Windows => "\u{E770}",
        ItemIcon::Desktop => "\u{E7F4}",
        ItemIcon::App => "\u{ECAA}",
        ItemIcon::RemotePc => "\u{E703}",
    }
}

fn tone_brush(tone: StatusTone) -> Brush {
    match tone {
        StatusTone::Good => Brush::Solid(Color::rgb(0x0F, 0x7B, 0x0F)),
        StatusTone::Warning => Brush::Solid(Color::rgb(0x9D, 0x5D, 0x00)),
        StatusTone::Error => ThemeBrush::SystemCritical.into(),
        _ => ThemeBrush::PrimaryText.into(),
    }
}

fn icon_box(icon: ItemIcon, size: f64) -> View {
    Border::new()
        .width(size)
        .height(size)
        .corner_radius(8.0)
        .background(ThemeBrush::SolidBackground)
        .border_brush(ThemeBrush::CardStroke)
        .border_thickness(1.0)
        .vertical_alignment(VerticalAlignment::Top)
        .content(
            Viewbox::new()
                .width(size / 2.0)
                .height(size / 2.0)
                .horizontal_alignment(HorizontalAlignment::Center)
                .vertical_alignment(VerticalAlignment::Center)
                .slot(ViewboxSlot::Child, FontIcon::new().glyph(glyph(icon))),
        )
}

fn caption(text: &str) -> TextBlock {
    TextBlock::new()
        .text(text)
        .font_size(12.0)
        .opacity(0.7)
        .text_trimming(TextTrimming::CharacterEllipsis)
}

fn favorite_button(ctx: &ViewContext<MainApp>, card: &CardView) -> View {
    let target = card.item.target();
    let glyph = if card.is_favorite {
        "\u{E735}"
    } else {
        "\u{E734}"
    };
    let tip = if card.is_favorite {
        "Remove from Favorites"
    } else {
        "Add to Favorites"
    };
    Button::new()
        .style(ButtonStyle::Subtle)
        .vertical_alignment(VerticalAlignment::Top)
        .content(FontIcon::new().glyph(glyph))
        .tooltip(tip)
        .on_click(ctx.callback(move |()| Msg::ToggleFavorite(target.clone())))
        .into()
}

fn more_button(ctx: &ViewContext<MainApp>, card: &CardView) -> View {
    let target = card.item.target();
    Button::new()
        .style(ButtonStyle::Subtle)
        .vertical_alignment(VerticalAlignment::Top)
        .content(FontIcon::new().glyph("\u{E712}"))
        .menu(Menu::new(
            item_menu(&card.item, card.busy),
            ctx.callback(move |label: String| Msg::ItemMenu(target.clone(), label)),
        ))
}

/// Status line (Cloud PCs) or last use.
fn status_line(card: &CardView) -> View {
    match &card.status {
        Some((text, tone)) => TextBlock::new()
            .text(text.as_str())
            .font_size(12.0)
            .foreground(tone_brush(*tone))
            .text_trimming(TextTrimming::CharacterEllipsis)
            .into(),
        None => caption(card.last_used.as_deref().unwrap_or_default()).into(),
    }
}

fn busy_indicator(card: &CardView) -> View {
    if card.busy {
        ProgressRing::new()
            .is_active(true)
            .width(16.0)
            .height(16.0)
            .vertical_alignment(VerticalAlignment::Center)
            .into()
    } else {
        View::empty()
    }
}

fn connect_button(ctx: &ViewContext<MainApp>, card: &CardView, content: View) -> View {
    let target = card.item.target();
    Button::new()
        .style(ButtonStyle::Subtle)
        .horizontal_alignment(HorizontalAlignment::Stretch)
        .vertical_alignment(VerticalAlignment::Stretch)
        .horizontal_content_alignment(HorizontalAlignment::Stretch)
        .vertical_content_alignment(VerticalAlignment::Stretch)
        .is_enabled(!card.busy)
        .on_click(ctx.callback(move |()| Msg::Connect(target.clone())))
        .content(content)
        .tooltip(card.tooltip)
}

fn card_frame(padding: Thickness, corner: f64) -> Border {
    Border::new()
        .padding(padding)
        .corner_radius(corner)
        .background(ThemeBrush::CardBackground)
        .border_brush(ThemeBrush::CardStroke)
        .border_thickness(1.0)
}

/// A 288×132 tile: icon, name and kind, account, status and a menu button.
fn tile(ctx: &ViewContext<MainApp>, card: &CardView) -> View {
    let detail = format!("{}{}", card.detail_prefix, card.detail);
    let body = Grid::new()
        .columns([GridLength::Auto, GridLength::Star(1.0)])
        .rows([
            GridLength::Auto,
            GridLength::Auto,
            GridLength::Star(1.0),
            GridLength::Auto,
            GridLength::Auto,
        ])
        .column_spacing(12.0)
        .row_spacing(2.0)
        .children((
            Border::new()
                .grid_row_span(3)
                .content(icon_box(card.icon, 48.0)),
            StackPanel::new()
                .grid_column(1)
                .orientation(Orientation::Horizontal)
                .spacing(8.0)
                .children((
                    TextBlock::new()
                        .text(card.title.as_str())
                        .font_weight(FontWeight::SEMI_BOLD)
                        .text_trimming(TextTrimming::CharacterEllipsis),
                    busy_indicator(card),
                )),
            caption(&card.kind).grid_row(1).grid_column(1),
            caption(card.account.as_deref().unwrap_or_default())
                .grid_row(2)
                .grid_column(1),
            caption(&detail).grid_row(3).grid_column_span(2),
            Border::new()
                .grid_row(4)
                .grid_column_span(2)
                .content(status_line(card)),
        ));
    card_frame(Thickness::uniform(0.0), 8.0)
        .width(TILE_WIDTH)
        .height(TILE_HEIGHT)
        .content(
            Grid::new()
                .columns([GridLength::Star(1.0), GridLength::Auto])
                .children((
                    connect_button(ctx, card, body),
                    StackPanel::new()
                        .grid_column(1)
                        .orientation(Orientation::Horizontal)
                        .margin(Thickness::new(0.0, 6.0, 6.0, 0.0))
                        .spacing(2.0)
                        .children((favorite_button(ctx, card), more_button(ctx, card))),
                )),
        )
}

/// A 56 px list row: icon, name with kind and account, detail and a menu button.
fn row(ctx: &ViewContext<MainApp>, card: &CardView) -> View {
    let detail = format!("{}{}", card.detail_prefix, card.detail);
    let subtitle = match &card.account {
        Some(account) => format!("{} • {account}", card.kind),
        None => card.kind.clone(),
    };
    let body = Grid::new()
        .columns([
            GridLength::Auto,
            GridLength::Star(1.0),
            GridLength::Auto,
            GridLength::Auto,
        ])
        .column_spacing(16.0)
        .children((
            Viewbox::new()
                .width(20.0)
                .height(20.0)
                .vertical_alignment(VerticalAlignment::Center)
                .slot(ViewboxSlot::Child, FontIcon::new().glyph(glyph(card.icon))),
            StackPanel::new()
                .grid_column(1)
                .vertical_alignment(VerticalAlignment::Center)
                .children((
                    StackPanel::new()
                        .orientation(Orientation::Horizontal)
                        .spacing(8.0)
                        .children((
                            TextBlock::new()
                                .text(card.title.as_str())
                                .text_trimming(TextTrimming::CharacterEllipsis),
                            busy_indicator(card),
                        )),
                    caption(&subtitle),
                )),
            StackPanel::new()
                .grid_column(2)
                .vertical_alignment(VerticalAlignment::Center)
                .max_width(280.0)
                .children((caption(&detail), status_line(card))),
        ));
    card_frame(Thickness::uniform(0.0), 4.0)
        .min_height(56.0)
        .content(
            Grid::new()
                .columns([GridLength::Star(1.0), GridLength::Auto])
                .children((
                    connect_button(ctx, card, body),
                    StackPanel::new()
                        .grid_column(1)
                        .orientation(Orientation::Horizontal)
                        .margin(Thickness::new(0.0, 0.0, 8.0, 0.0))
                        .vertical_alignment(VerticalAlignment::Center)
                        .spacing(2.0)
                        .children((favorite_button(ctx, card), more_button(ctx, card))),
                )),
        )
}

/// The cards of a section as tiles or rows.
fn cards(ctx: &ViewContext<MainApp>, cards: &[CardView], list: bool) -> View {
    let keyed = cards.iter().map(|card| {
        let view = if list {
            row(ctx, card)
        } else {
            tile(ctx, card)
        };
        (card.item.target().to_string(), view)
    });
    if list {
        StackPanel::new().spacing(4.0).keyed_children(keyed)
    } else {
        VariableSizedWrapGrid::new()
            .orientation(Orientation::Horizontal)
            .item_width(TILE_WIDTH + TILE_GAP)
            .item_height(TILE_HEIGHT + TILE_GAP)
            .keyed_children(keyed)
    }
}

/// A collapsible section with its count.
pub fn section(ctx: &ViewContext<MainApp>, section: &SectionView, list: bool) -> View {
    let id = section.id;
    let count = match section.cards.len() {
        1 => "1 item".to_string(),
        n => format!("{n} items"),
    };
    Expander::new()
        .horizontal_alignment(HorizontalAlignment::Stretch)
        .is_expanded(section.expanded)
        .on_is_expanded_changed(
            ctx.callback(move |expanded| Msg::SectionExpanded(id.to_string(), expanded)),
        )
        .slots([
            SlotView::new(
                ExpanderSlot::Header,
                StackPanel::new()
                    .orientation(Orientation::Horizontal)
                    .spacing(8.0)
                    .children((
                        TextBlock::new()
                            .text(section.title)
                            .font_weight(FontWeight::SEMI_BOLD),
                        caption(&count).vertical_alignment(VerticalAlignment::Center),
                    )),
            ),
            SlotView::new(ExpanderSlot::Content, cards(ctx, &section.cards, list)),
        ])
}

/// The cards of one section without the expander (section pages).
pub fn section_page(ctx: &ViewContext<MainApp>, section: &SectionView, list: bool) -> View {
    cards(ctx, &section.cards, list)
}

/// Shown when there is nothing to list.
pub fn empty_state(ctx: &ViewContext<MainApp>, signed_in: bool) -> View {
    let mut buttons: Vec<(String, View)> = vec![(
        "add".into(),
        Button::new()
            .style(ButtonStyle::Accent)
            .content("Add Connection")
            .menu(add_menu(ctx)),
    )];
    if !signed_in {
        buttons.push((
            "signin".into(),
            Button::new()
                .on_click(ctx.callback(|()| Msg::SignIn))
                .content("Sign In"),
        ));
    }
    StackPanel::new()
        .horizontal_alignment(HorizontalAlignment::Center)
        .vertical_alignment(VerticalAlignment::Center)
        .max_width(420.0)
        .spacing(12.0)
        .margin(Thickness::uniform(32.0))
        .children((
            Viewbox::new()
                .width(64.0)
                .height(64.0)
                .horizontal_alignment(HorizontalAlignment::Center)
                .slot(ViewboxSlot::Child, FontIcon::new().glyph("\u{E7F4}")),
            TextBlock::new()
                .text(EMPTY_TITLE)
                .font_size(20.0)
                .font_weight(FontWeight::SEMI_BOLD)
                .horizontal_alignment(HorizontalAlignment::Center),
            TextBlock::new()
                .text(empty_description(signed_in))
                .opacity(0.7)
                .text_wrapping(TextWrapping::Wrap)
                .horizontal_alignment(HorizontalAlignment::Center),
            StackPanel::new()
                .orientation(Orientation::Horizontal)
                .horizontal_alignment(HorizontalAlignment::Center)
                .spacing(8.0)
                .keyed_children(buttons),
        ))
}

/// Loading indicator of the connections page.
pub fn loading() -> View {
    StackPanel::new()
        .horizontal_alignment(HorizontalAlignment::Center)
        .vertical_alignment(VerticalAlignment::Center)
        .spacing(12.0)
        .margin(Thickness::uniform(32.0))
        .children((
            ProgressRing::new()
                .is_active(true)
                .width(48.0)
                .height(48.0)
                .horizontal_alignment(HorizontalAlignment::Center),
            TextBlock::new()
                .text("Loading your connections…")
                .opacity(0.7)
                .horizontal_alignment(HorizontalAlignment::Center),
        ))
}

/// The account button of the navigation pane's footer.
pub fn account_button(
    ctx: &ViewContext<MainApp>,
    name: Option<&str>,
    upn: Option<&str>,
    menu: Vec<windows_reactor::MenuItem>,
    is_pane_open: bool,
) -> View {
    let title = name.or(upn).unwrap_or("Not signed in");
    let initials: String = title
        .split_whitespace()
        .filter_map(|word| word.chars().next())
        .filter(|c| c.is_alphanumeric())
        .take(2)
        .collect::<String>()
        .to_uppercase();
    let person_picture = PersonPicture::new()
        .display_name(title)
        .initials(initials)
        .width(32.0)
        .height(32.0);

    let content: View = if is_pane_open {
        let subtitle = if name.is_some() {
            upn.unwrap_or_default()
        } else {
            ""
        };
        let mut texts: Vec<(String, View)> = vec![(
            "name".into(),
            TextBlock::new()
                .text(title)
                .text_trimming(TextTrimming::CharacterEllipsis)
                .into(),
        )];
        if !subtitle.is_empty() {
            texts.push(("upn".into(), caption(subtitle).into()));
        }
        Grid::new()
            .columns([GridLength::Auto, GridLength::Star(1.0)])
            .column_spacing(12.0)
            .children((
                person_picture,
                StackPanel::new()
                    .grid_column(1)
                    .vertical_alignment(VerticalAlignment::Center)
                    .keyed_children(texts),
            ))
            .into()
    } else {
        person_picture.into()
    };

    if is_pane_open {
        Button::new()
            .style(ButtonStyle::Subtle)
            .horizontal_alignment(HorizontalAlignment::Stretch)
            .horizontal_content_alignment(HorizontalAlignment::Left)
            .margin(Thickness::xy(4.0, 4.0))
            .content(content)
            .menu(Menu::new(menu, ctx.callback(Msg::AccountMenu)))
            .into()
    } else {
        Button::new()
            .style(ButtonStyle::Subtle)
            .horizontal_alignment(HorizontalAlignment::Center)
            .horizontal_content_alignment(HorizontalAlignment::Center)
            .vertical_content_alignment(VerticalAlignment::Center)
            .margin(Thickness::uniform(0.0))
            .width(40.0)
            .height(40.0)
            .content(content)
            .menu(Menu::new(menu, ctx.callback(Msg::AccountMenu)))
            .into()
    }
}
