//! Grid cards and boxed-list rows for Cloud PCs, Azure Virtual Desktop resources and
//! saved Remote PCs, each with a 3-dots actions menu. All kinds share one layout so they
//! sit side by side in the main window.

use crate::api::{AvdResource, AvdResourceKind};
use crate::connections::SavedConnection;
use crate::launcher::LaunchTarget;
use crate::models::{CloudPc, CloudPcAction, StatusTone};
use crate::platform;
use adw::prelude::*;
use gtk4::{
    gio, Box, GestureClick, Image, Label, MenuButton, Orientation, Separator, Spinner, Widget,
};
use std::rc::Rc;

type ActionHandler = Rc<dyn Fn(CardAction)>;
type PcActionFactory = fn(CloudPc) -> CardAction;
type ToggleFactory = fn(LaunchTarget, bool) -> CardAction;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardLayoutMode {
    Grid,
    List,
}

#[derive(Debug, Clone)]
pub enum CardAction {
    Connect(CloudPc),
    Restart(CloudPc),
    Rename(CloudPc),
    Inspect(CloudPc),
    Details(CloudPc),
    Troubleshoot(CloudPc),
    Restore(CloudPc),
    Reset(CloudPc),
    /// Start (`true`) or stop a Frontline Cloud PC.
    Power(CloudPc, bool),
    ConnectAvd(AvdResource),
    ConnectRemote(SavedConnection),
    EditRemote(SavedConnection),
    RemoveRemote(SavedConnection),
    /// Add or remove the app launcher entry for a connection.
    ToggleLauncher(LaunchTarget, bool),
    /// Connect (or stop connecting) when the user logs in.
    ToggleAutostart(LaunchTarget, bool),
    Settings,
    /// Per-connection settings of the target (named for the dialog title).
    ConnectionSettings(LaunchTarget, String),
    ExportRemote(SavedConnection),
    ForgetPassword(SavedConnection),
}

enum CardIcon {
    /// Windows logo glyph (Cloud PCs).
    Windows,
    Named(&'static str),
}

/// What a card shows, independent of the layout.
struct CardView {
    title: String,
    /// Account line (UPN or username).
    account: Option<String>,
    /// Kind shown under the icon (OS name, "Remote PC").
    kind: String,
    /// Short technical summary (hardware, address).
    detail: String,
    /// Prefix for `detail` on grid cards.
    detail_prefix: &'static str,
    icon: CardIcon,
    status: Option<Label>,
    connect_tooltip: &'static str,
    menu: gio::Menu,
    menu_tooltip: &'static str,
    /// When the connection was last used ("2 hours ago").
    last_used: Option<String>,
}

pub struct ConnectionCard;

impl ConnectionCard {
    /// Card (grid) or row (list) for a Cloud PC.
    pub fn cloud_pc(
        pc: &CloudPc,
        mode: CardLayoutMode,
        is_connecting: bool,
        last_used: Option<String>,
        on_action: impl Fn(CardAction) + 'static,
    ) -> Widget {
        let on_action: ActionHandler = Rc::new(on_action);
        let view = CardView {
            title: pc.display_name.clone(),
            account: pc.user_principal_name.clone(),
            kind: os_name(pc).to_string(),
            detail: pc.hardware_summary().to_string(),
            detail_prefix: "Spec: ",
            icon: CardIcon::Windows,
            status: Some(status_label(pc)),
            connect_tooltip: "Click to connect to Cloud PC",
            menu: cloud_pc_menu(pc),
            menu_tooltip: "Cloud PC Actions",
            last_used,
        };
        let group = cloud_pc_actions(pc, &on_action);
        let target = LaunchTarget::CloudPc(pc.id.clone());
        add_settings_actions(&group, &target, &pc.display_name, &on_action);
        add_desktop_actions(&group, &target, &on_action);
        let pc = pc.clone();
        let connect = move || on_action(CardAction::Connect(pc.clone()));
        build(&view, mode, is_connecting, &group, Rc::new(connect))
    }

    /// Card (grid) or row (list) for a saved Remote PC.
    pub fn remote_pc(
        connection: &SavedConnection,
        mode: CardLayoutMode,
        last_used: Option<String>,
        on_action: impl Fn(CardAction) + 'static,
    ) -> Widget {
        let on_action: ActionHandler = Rc::new(on_action);
        let view = CardView {
            title: connection.display_name().to_string(),
            account: Some(connection.username.clone()).filter(|u| !u.is_empty()),
            kind: match (connection.group.as_str(), &connection.remote_app) {
                ("", None) => "Remote PC".to_string(),
                ("", Some(_)) => "RemoteApp".to_string(),
                (group, None) => format!("Remote PC · {group}"),
                (group, Some(_)) => format!("RemoteApp · {group}"),
            },
            detail: connection.address.clone(),
            detail_prefix: "",
            icon: CardIcon::Named("preferences-desktop-remote-desktop-symbolic"),
            status: None,
            connect_tooltip: "Click to connect to Remote PC",
            menu: remote_pc_menu(connection),
            menu_tooltip: "Remote PC Actions",
            last_used,
        };
        let group = remote_pc_actions(connection, &on_action);
        let target = LaunchTarget::RemotePc(connection.id.clone());
        add_settings_actions(&group, &target, connection.display_name(), &on_action);
        add_desktop_actions(&group, &target, &on_action);
        let connection = connection.clone();
        let connect = move || on_action(CardAction::ConnectRemote(connection.clone()));
        build(&view, mode, false, &group, Rc::new(connect))
    }

    /// Card (grid) or row (list) for an Azure Virtual Desktop desktop or RemoteApp.
    pub fn avd_resource(
        resource: &AvdResource,
        mode: CardLayoutMode,
        is_connecting: bool,
        last_used: Option<String>,
        on_action: impl Fn(CardAction) + 'static,
    ) -> Widget {
        let on_action: ActionHandler = Rc::new(on_action);
        let (icon, tooltip) = match resource.kind {
            AvdResourceKind::Desktop => ("computer-symbolic", "Click to connect to the desktop"),
            AvdResourceKind::RemoteApp => (
                "application-x-executable-symbolic",
                "Click to start the app",
            ),
        };
        let view = CardView {
            title: resource.title.clone(),
            account: resource.folder.clone(),
            kind: resource.kind_label().to_string(),
            detail: resource.workspace.clone(),
            detail_prefix: "Workspace: ",
            icon: CardIcon::Named(icon),
            status: None,
            connect_tooltip: tooltip,
            menu: avd_menu(),
            menu_tooltip: "Azure Virtual Desktop Actions",
            last_used,
        };
        let group = gio::SimpleActionGroup::new();
        let r = resource.clone();
        add_action(&group, "connect", &on_action, move || {
            CardAction::ConnectAvd(r.clone())
        });
        let target = LaunchTarget::Avd(resource.id.clone());
        add_settings_actions(&group, &target, &resource.title, &on_action);
        add_desktop_actions(&group, &target, &on_action);
        let resource = resource.clone();
        let connect = move || on_action(CardAction::ConnectAvd(resource.clone()));
        build(&view, mode, is_connecting, &group, Rc::new(connect))
    }
}

fn build(
    view: &CardView,
    mode: CardLayoutMode,
    is_connecting: bool,
    actions: &gio::SimpleActionGroup,
    connect: Rc<dyn Fn()>,
) -> Widget {
    let widget = match mode {
        CardLayoutMode::Grid => grid_card(view, is_connecting, connect),
        CardLayoutMode::List => list_row(view, is_connecting, connect),
    };
    widget.insert_action_group("card", Some(actions));
    widget.set_sensitive(!is_connecting);
    widget
}

/// Card with an icon banner (click to connect), details and the actions menu.
fn grid_card(view: &CardView, is_connecting: bool, connect: Rc<dyn Fn()>) -> Widget {
    let card_box = Box::new(Orientation::Vertical, 0);
    card_box.set_size_request(290, 250);
    card_box.add_css_class("card");
    card_box.set_overflow(gtk4::Overflow::Hidden);

    let banner_box = Box::new(Orientation::Vertical, 6);
    banner_box.set_size_request(290, 125);
    banner_box.set_cursor_from_name(Some("pointer"));
    banner_box.set_tooltip_text(Some(view.connect_tooltip));

    let banner_top = Box::new(Orientation::Horizontal, 8);
    banner_top.set_margin_start(12);
    banner_top.set_margin_end(12);
    banner_top.set_margin_top(10);
    // Keeps the icon at the same height whether or not there is a status.
    banner_top.set_size_request(-1, 16);
    if let Some(status) = &view.status {
        status.set_halign(gtk4::Align::End);
        status.set_hexpand(true);
        banner_top.append(status);
    }
    banner_box.append(&banner_top);

    let center_branding = Box::new(Orientation::Vertical, 4);
    center_branding.set_valign(gtk4::Align::Center);
    center_branding.set_vexpand(true);
    center_branding.append(&icon_widget(&view.icon, true));
    let kind = Label::new(Some(&view.kind));
    kind.add_css_class("heading");
    center_branding.append(&kind);
    banner_box.append(&center_branding);
    card_box.append(&banner_box);
    card_box.append(&Separator::new(Orientation::Horizontal));

    let details_box = Box::new(Orientation::Vertical, 4);
    details_box.set_margin_start(14);
    details_box.set_margin_end(10);
    details_box.set_margin_top(10);
    details_box.set_margin_bottom(10);
    details_box.set_vexpand(true);

    let title_label = Label::new(Some(&view.title));
    title_label.set_halign(gtk4::Align::Start);
    title_label.add_css_class("title-4");
    title_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);

    let account_label = Label::new(view.account.as_deref());
    account_label.set_halign(gtk4::Align::Start);
    account_label.add_css_class("dim-label");
    account_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);

    let details_bottom_row = Box::new(Orientation::Horizontal, 8);
    details_bottom_row.set_valign(gtk4::Align::Center);

    let detail_label = Label::new(Some(&format!("{}{}", view.detail_prefix, view.detail)));
    detail_label.set_halign(gtk4::Align::Start);
    detail_label.set_hexpand(true);
    detail_label.add_css_class("dim-label");
    detail_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    details_bottom_row.append(&detail_label);
    details_bottom_row.append(&spinner(is_connecting));
    details_bottom_row.append(&menu_button(view));

    details_box.append(&title_label);
    details_box.append(&account_label);
    if let Some(last_used) = &view.last_used {
        let used_label = Label::new(Some(&format!("Last used {}", last_used.to_lowercase())));
        used_label.set_halign(gtk4::Align::Start);
        used_label.add_css_class("caption");
        used_label.add_css_class("dim-label");
        details_box.append(&used_label);
    }
    details_box.append(&details_bottom_row);
    card_box.append(&details_box);

    // Only the banner connects on click; the details area hosts the menu.
    let gesture = GestureClick::new();
    gesture.connect_released(move |_, _, _, _| connect());
    banner_box.add_controller(gesture);

    card_box.upcast()
}

/// Activatable boxed-list row with icon, details, status and the actions menu.
fn list_row(view: &CardView, is_connecting: bool, connect: Rc<dyn Fn()>) -> Widget {
    let mut subtitle = format!("{} • {}", view.kind, view.detail);
    if let Some(last_used) = &view.last_used {
        subtitle.push_str(" • ");
        subtitle.push_str(last_used);
    }
    if let Some(account) = &view.account {
        subtitle.push('\n');
        subtitle.push_str(account);
    }

    let row = adw::ActionRow::builder()
        .title(markup_escape(&view.title))
        .subtitle(markup_escape(&subtitle))
        .subtitle_lines(2)
        .activatable(true)
        .tooltip_text(view.connect_tooltip)
        .build();

    let icon = icon_widget(&view.icon, false);
    icon.set_width_request(42);
    row.add_prefix(&icon);

    if let Some(status) = &view.status {
        status.set_valign(gtk4::Align::Center);
        row.add_suffix(status);
    }
    let spinner = spinner(is_connecting);
    spinner.set_valign(gtk4::Align::Center);
    row.add_suffix(&spinner);
    let menu_btn = menu_button(view);
    menu_btn.set_valign(gtk4::Align::Center);
    row.add_suffix(&menu_btn);

    row.connect_activated(move |_| connect());
    row.upcast()
}

fn icon_widget(icon: &CardIcon, large: bool) -> Widget {
    match icon {
        CardIcon::Windows => {
            let label = Label::new(Some("⊞"));
            label.add_css_class(if large { "title-1" } else { "title-2" });
            label.upcast()
        }
        CardIcon::Named(name) => {
            let image = Image::from_icon_name(name);
            image.set_pixel_size(if large { 40 } else { 28 });
            image.upcast()
        }
    }
}

fn os_name(pc: &CloudPc) -> &str {
    pc.os_name
        .as_deref()
        .or(pc.image_display_name.as_deref())
        .unwrap_or("Windows")
}

/// Status badge coloured with the theme's semantic style classes.
fn status_label(pc: &CloudPc) -> Label {
    let (status, tone) = pc.status_summary(chrono::Utc::now());
    let label = Label::new(Some(&format!("● {status}")));
    label.add_css_class("caption-heading");
    label.add_css_class(match tone {
        StatusTone::Good => "success",
        StatusTone::Neutral => "dim-label",
        StatusTone::Warning => "warning",
        StatusTone::Error => "error",
    });
    label.set_tooltip_text(Some(&format!("Status: {}", pc.display_status())));
    label
}

fn spinner(is_connecting: bool) -> Spinner {
    let spinner = Spinner::new();
    spinner.set_size_request(20, 20);
    spinner.set_visible(is_connecting);
    if is_connecting {
        spinner.start();
    }
    spinner
}

/// 3-dots button for `view.menu` (actions live in the widget's "card" group).
fn menu_button(view: &CardView) -> MenuButton {
    MenuButton::builder()
        .icon_name("view-more-symbolic")
        .tooltip_text(view.menu_tooltip)
        .menu_model(&view.menu)
        .has_frame(false)
        .build()
}

fn settings_section() -> gio::Menu {
    let section = gio::Menu::new();
    section.append(
        Some("Connection Settings…"),
        Some("card.connection-settings"),
    );
    section.append(Some("Preferences"), Some("card.settings"));
    section
}

/// "Show in App Launcher" / "Connect at Login" check items (stateful actions).
fn desktop_section() -> gio::Menu {
    let section = gio::Menu::new();
    section.append(Some("Show in App Launcher"), Some("card.launcher"));
    section.append(Some("Connect at Login"), Some("card.autostart"));
    section
}

fn cloud_pc_menu(pc: &CloudPc) -> gio::Menu {
    let main_section = gio::Menu::new();
    main_section.append(Some("Connect"), Some("card.connect"));
    if pc.allows(CloudPcAction::Start) || pc.allows(CloudPcAction::Stop) {
        main_section.append(Some("Start"), Some("card.start"));
        main_section.append(Some("Stop"), Some("card.stop"));
    }
    main_section.append(Some("Restart"), Some("card.restart"));
    main_section.append(Some("Rename…"), Some("card.rename"));
    if pc.direct_launch_url.is_some() {
        main_section.append(Some("Open in Web Browser"), Some("card.open-web"));
    }

    let support_section = gio::Menu::new();
    support_section.append(Some("Troubleshoot"), Some("card.troubleshoot"));
    support_section.append(Some("Inspect Connection"), Some("card.inspect"));
    support_section.append(Some("View Details"), Some("card.details"));

    let recovery_section = gio::Menu::new();
    recovery_section.append(Some("Restore…"), Some("card.restore"));
    recovery_section.append(Some("Reset…"), Some("card.reset"));

    let menu = gio::Menu::new();
    menu.append_section(None, &main_section);
    menu.append_section(None, &support_section);
    menu.append_section(None, &recovery_section);
    menu.append_section(None, &desktop_section());
    menu.append_section(None, &settings_section());
    menu
}

fn avd_menu() -> gio::Menu {
    let main_section = gio::Menu::new();
    main_section.append(Some("Connect"), Some("card.connect"));
    let menu = gio::Menu::new();
    menu.append_section(None, &main_section);
    menu.append_section(None, &desktop_section());
    menu.append_section(None, &settings_section());
    menu
}

fn remote_pc_menu(connection: &SavedConnection) -> gio::Menu {
    let main_section = gio::Menu::new();
    main_section.append(Some("Connect"), Some("card.connect"));
    main_section.append(Some("Edit…"), Some("card.edit"));
    main_section.append(Some("Export as .rdp File…"), Some("card.export"));
    if connection.remember_password {
        main_section.append(Some("Forget Password"), Some("card.forget-password"));
    }
    let remove_section = gio::Menu::new();
    remove_section.append(Some("Remove…"), Some("card.remove"));
    let menu = gio::Menu::new();
    menu.append_section(None, &main_section);
    menu.append_section(None, &desktop_section());
    menu.append_section(None, &remove_section);
    menu.append_section(None, &settings_section());
    menu
}

fn add_action(
    group: &gio::SimpleActionGroup,
    name: &str,
    on_action: &ActionHandler,
    make: impl Fn() -> CardAction + 'static,
) {
    let action = gio::SimpleAction::new(name, None);
    let on_action = Rc::clone(on_action);
    action.connect_activate(move |_, _| on_action(make()));
    group.add_action(&action);
}

/// Adds `name` for `pc`, disabled when the service does not allow `action` right now.
fn add_pc_action(
    group: &gio::SimpleActionGroup,
    name: &str,
    pc: &CloudPc,
    action: CloudPcAction,
    on_action: &ActionHandler,
    make: PcActionFactory,
) {
    let p = pc.clone();
    add_action(group, name, on_action, move || make(p.clone()));
    if let Some(added) = group.lookup_action(name) {
        if let Ok(simple) = added.downcast::<gio::SimpleAction>() {
            simple.set_enabled(pc.allows(action));
        }
    }
}

/// Boolean actions backing the launcher and login check items.
fn add_desktop_actions(
    group: &gio::SimpleActionGroup,
    target: &LaunchTarget,
    on_action: &ActionHandler,
) {
    let toggles: [(&str, bool, ToggleFactory); 2] = [
        (
            "launcher",
            platform::has_launcher(target),
            CardAction::ToggleLauncher,
        ),
        (
            "autostart",
            platform::has_autostart(target),
            CardAction::ToggleAutostart,
        ),
    ];
    for (name, enabled, make) in toggles {
        let action = gio::SimpleAction::new_stateful(name, None, &enabled.to_variant());
        let on_action = Rc::clone(on_action);
        let target = target.clone();
        action.connect_activate(move |action, _| {
            let enable = !action
                .state()
                .and_then(|s| s.get::<bool>())
                .unwrap_or(false);
            action.set_state(&enable.to_variant());
            on_action(make(target.clone(), enable));
        });
        group.add_action(&action);
    }
}

fn cloud_pc_actions(pc: &CloudPc, on_action: &ActionHandler) -> gio::SimpleActionGroup {
    let group = gio::SimpleActionGroup::new();
    let p = pc.clone();
    add_action(&group, "connect", on_action, move || {
        CardAction::Connect(p.clone())
    });
    let pc_actions: [(&str, CloudPcAction, PcActionFactory); 7] = [
        ("restart", CloudPcAction::Restart, CardAction::Restart),
        ("rename", CloudPcAction::Rename, CardAction::Rename),
        (
            "troubleshoot",
            CloudPcAction::Troubleshoot,
            CardAction::Troubleshoot,
        ),
        ("restore", CloudPcAction::Restore, CardAction::Restore),
        ("reset", CloudPcAction::Reset, CardAction::Reset),
        ("start", CloudPcAction::Start, |pc| {
            CardAction::Power(pc, true)
        }),
        ("stop", CloudPcAction::Stop, |pc| {
            CardAction::Power(pc, false)
        }),
    ];
    for (name, action, make) in pc_actions {
        add_pc_action(&group, name, pc, action, on_action, make);
    }
    let p = pc.clone();
    add_action(&group, "inspect", on_action, move || {
        CardAction::Inspect(p.clone())
    });
    let p = pc.clone();
    add_action(&group, "details", on_action, move || {
        CardAction::Details(p.clone())
    });
    if let Some(url) = pc.direct_launch_url.clone() {
        let action = gio::SimpleAction::new("open-web", None);
        action.connect_activate(move |_, _| super::open_uri(&url, None::<&gtk4::Window>));
        group.add_action(&action);
    }
    group
}

fn remote_pc_actions(
    connection: &SavedConnection,
    on_action: &ActionHandler,
) -> gio::SimpleActionGroup {
    let group = gio::SimpleActionGroup::new();
    let c = connection.clone();
    add_action(&group, "connect", on_action, move || {
        CardAction::ConnectRemote(c.clone())
    });
    let c = connection.clone();
    add_action(&group, "edit", on_action, move || {
        CardAction::EditRemote(c.clone())
    });
    let c = connection.clone();
    add_action(&group, "remove", on_action, move || {
        CardAction::RemoveRemote(c.clone())
    });
    let c = connection.clone();
    add_action(&group, "export", on_action, move || {
        CardAction::ExportRemote(c.clone())
    });
    let c = connection.clone();
    add_action(&group, "forget-password", on_action, move || {
        CardAction::ForgetPassword(c.clone())
    });
    group
}

/// "Connection Settings…" and "Preferences" items of every card.
fn add_settings_actions(
    group: &gio::SimpleActionGroup,
    target: &LaunchTarget,
    name: &str,
    on_action: &ActionHandler,
) {
    let (target, name) = (target.clone(), name.to_string());
    add_action(group, "connection-settings", on_action, move || {
        CardAction::ConnectionSettings(target.clone(), name.clone())
    });
    add_action(group, "settings", on_action, || CardAction::Settings);
}

fn markup_escape(text: &str) -> String {
    gtk4::glib::markup_escape_text(text).to_string()
}
