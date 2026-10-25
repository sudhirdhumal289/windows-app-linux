//! GTK4 widget displaying a Cloud PC card in Grid or List layout with auxiliary controls.

use crate::models::CloudPc;
use gtk4::prelude::*;
use gtk4::{
    Box, Button, GestureClick, Label, MenuButton, Orientation, Popover, Separator, Spinner, Widget,
};
use std::rc::Rc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardLayoutMode {
    Grid,
    List,
}

#[derive(Debug, Clone)]
pub enum CardAction {
    Connect(CloudPc),
    Restart(String),
    Rename(CloudPc),
    Inspect(CloudPc),
    Details(CloudPc),
    Settings,
}

pub struct CloudPcCardWidget;

impl CloudPcCardWidget {
    /// Builds either a Grid card or a List row based on `mode`.
    pub fn build<F>(pc: &CloudPc, mode: CardLayoutMode, is_connecting: bool, on_action: F) -> Widget
    where
        F: Fn(CardAction, Spinner, Box) + 'static + Clone,
    {
        match mode {
            CardLayoutMode::Grid => Self::build_grid_card(pc, is_connecting, on_action),
            CardLayoutMode::List => Self::build_list_row(pc, is_connecting, on_action),
        }
    }

    /// Constructs a Grid card with OS-specific banner (click-to-connect), hardware details, and aligned 3-dots menu.
    fn build_grid_card<F>(pc: &CloudPc, is_connecting: bool, on_action: F) -> Widget
    where
        F: Fn(CardAction, Spinner, Box) + 'static + Clone,
    {
        let card_box = Box::new(Orientation::Vertical, 0);
        card_box.add_css_class("card");
        card_box.add_css_class("cpc-grid-card");
        card_box.set_size_request(290, 250);

        // --- 1. Top OS-Specific Background Banner (Clickable Graphics for Connect) ---
        let banner_box = Box::new(Orientation::Vertical, 6);
        banner_box.set_size_request(290, 125);
        banner_box.add_css_class("cpc-grid-banner");
        banner_box.set_cursor_from_name(Some("pointer"));
        banner_box.set_tooltip_text(Some("Click to connect to Cloud PC"));

        // Top bar inside banner: Status Pill at top-right
        let banner_top = Box::new(Orientation::Horizontal, 8);
        banner_top.set_margin_start(12);
        banner_top.set_margin_end(12);
        banner_top.set_margin_top(10);

        let status_label = Label::new(Some(&format!("● {}", pc.display_status())));
        status_label.set_halign(gtk4::Align::End);
        status_label.set_hexpand(true);
        status_label.add_css_class("cpc-status-pill");
        if pc.display_status().to_lowercase().contains("provisioned")
            || pc.display_status().to_lowercase().contains("available")
            || pc.display_status().to_lowercase().contains("running")
        {
            status_label.add_css_class("success");
        }
        banner_top.append(&status_label);
        banner_box.append(&banner_top);

        // Center inside banner: Windows 11 Branding
        let center_branding = Box::new(Orientation::Vertical, 4);
        center_branding.set_valign(gtk4::Align::Center);
        center_branding.set_vexpand(true);

        let os_icon = Label::new(Some("⊞"));
        os_icon.add_css_class("cpc-banner-os-logo");

        let os_badge = Label::new(Some(
            pc.image_display_name
                .as_deref()
                .unwrap_or("Windows 11 Enterprise"),
        ));
        os_badge.add_css_class("cpc-banner-os-title");

        center_branding.append(&os_icon);
        center_branding.append(&os_badge);
        banner_box.append(&center_branding);
        card_box.append(&banner_box);

        // --- 2. Bottom Details Pane (Non-clickable, contains aligned aux action button) ---
        let details_box = Box::new(Orientation::Vertical, 4);
        details_box.add_css_class("cpc-grid-details");
        details_box.set_vexpand(true);

        let title_label = Label::new(Some(&pc.display_name));
        title_label.set_halign(gtk4::Align::Start);
        title_label.add_css_class("title-4");
        title_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);

        let user_label = Label::new(pc.user_principal_name.as_deref());
        user_label.set_halign(gtk4::Align::Start);
        user_label.add_css_class("dim-label");
        user_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);

        // Bottom row inside details pane: Hardware spec on left, Spinner & 3-dots Menu on right
        let details_bottom_row = Box::new(Orientation::Horizontal, 8);
        details_bottom_row.set_valign(gtk4::Align::Center);

        let hardware_label = Label::new(Some(&format!("Spec: {}", pc.hardware_summary())));
        hardware_label.set_halign(gtk4::Align::Start);
        hardware_label.set_hexpand(true);
        hardware_label.add_css_class("dim-label");
        hardware_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        details_bottom_row.append(&hardware_label);

        let spinner = Spinner::new();
        spinner.set_size_request(20, 20);
        if is_connecting {
            spinner.set_visible(true);
            spinner.start();
            card_box.set_sensitive(false);
        } else {
            spinner.set_visible(false);
        }
        details_bottom_row.append(&spinner);

        let menu_btn = MenuButton::new();
        menu_btn.set_icon_name("view-more-symbolic");
        menu_btn.add_css_class("flat");
        menu_btn.set_tooltip_text(Some("Cloud PC Actions"));

        let popover = Popover::new();
        let menu_box = Box::new(Orientation::Vertical, 2);
        menu_box.set_margin_start(4);
        menu_box.set_margin_end(4);
        menu_box.set_margin_top(4);
        menu_box.set_margin_bottom(4);

        let pc_actions = pc.clone();
        let s_act = spinner.clone();
        let c_act = card_box.clone();
        let on_act_rc = Rc::new(on_action.clone());

        Self::populate_menu_box(&menu_box, &popover, &pc_actions, s_act, c_act, on_act_rc);

        popover.set_child(Some(&menu_box));
        menu_btn.set_popover(Some(&popover));
        details_bottom_row.append(&menu_btn);

        details_box.append(&title_label);
        details_box.append(&user_label);
        details_box.append(&details_bottom_row);
        card_box.append(&details_box);

        // GestureClick on banner_box (Graphics) ONLY
        let gesture = GestureClick::new();
        let pc_click = pc.clone();
        let s_click = spinner;
        let c_click = card_box.clone();
        let on_act_click = on_action;

        gesture.connect_pressed(move |_, _, _, _| {
            c_click.set_sensitive(false);
            s_click.set_visible(true);
            s_click.start();
            on_act_click(
                CardAction::Connect(pc_click.clone()),
                s_click.clone(),
                c_click.clone(),
            );
        });
        banner_box.add_controller(gesture);

        card_box.upcast()
    }

    /// Constructs a List row with OS icon, details, status badge, spinner, and 3-dots menu.
    fn build_list_row<F>(pc: &CloudPc, is_connecting: bool, on_action: F) -> Widget
    where
        F: Fn(CardAction, Spinner, Box) + 'static + Clone,
    {
        let row_box = Box::new(Orientation::Horizontal, 16);
        row_box.add_css_class("card");
        row_box.add_css_class("cpc-list-card");
        row_box.set_margin_start(4);
        row_box.set_margin_end(4);
        row_box.set_margin_top(4);
        row_box.set_margin_bottom(4);

        // Clickable Left & Center Area
        let click_area = Box::new(Orientation::Horizontal, 14);
        click_area.set_hexpand(true);
        click_area.set_cursor_from_name(Some("pointer"));
        click_area.set_margin_start(14);
        click_area.set_margin_top(10);
        click_area.set_margin_bottom(10);

        // 1. Left OS Icon
        let icon_box = Box::new(Orientation::Vertical, 0);
        icon_box.set_size_request(42, 42);
        icon_box.add_css_class("cpc-list-icon-badge");
        icon_box.set_valign(gtk4::Align::Center);

        let os_icon = Label::new(Some("⊞"));
        os_icon.add_css_class("cpc-list-os-logo");
        os_icon.set_valign(gtk4::Align::Center);
        os_icon.set_vexpand(true);
        icon_box.append(&os_icon);
        click_area.append(&icon_box);

        // 2. Center Details
        let center_box = Box::new(Orientation::Vertical, 2);
        center_box.set_hexpand(true);
        center_box.set_valign(gtk4::Align::Center);

        let title_label = Label::new(Some(&pc.display_name));
        title_label.set_halign(gtk4::Align::Start);
        title_label.add_css_class("title-4");

        let spec_text = format!(
            "{} • {}",
            pc.image_display_name
                .as_deref()
                .unwrap_or("Windows 11 Enterprise"),
            pc.hardware_summary()
        );
        let spec_label = Label::new(Some(&spec_text));
        spec_label.set_halign(gtk4::Align::Start);
        spec_label.add_css_class("dim-label");

        let user_label = Label::new(pc.user_principal_name.as_deref());
        user_label.set_halign(gtk4::Align::Start);
        user_label.add_css_class("dim-label");

        center_box.append(&title_label);
        center_box.append(&spec_label);
        center_box.append(&user_label);
        click_area.append(&center_box);

        row_box.append(&click_area);

        // 3. Right Controls (Outside Click Area)
        let right_box = Box::new(Orientation::Horizontal, 10);
        right_box.set_valign(gtk4::Align::Center);
        right_box.set_margin_end(14);

        let status_badge = Label::new(Some(&format!("● {}", pc.display_status())));
        status_badge.add_css_class("cpc-status-pill");
        if pc.display_status().to_lowercase().contains("provisioned")
            || pc.display_status().to_lowercase().contains("available")
            || pc.display_status().to_lowercase().contains("running")
        {
            status_badge.add_css_class("success");
        }
        right_box.append(&status_badge);

        let spinner = Spinner::new();
        spinner.set_size_request(22, 22);
        if is_connecting {
            spinner.set_visible(true);
            spinner.start();
            row_box.set_sensitive(false);
        } else {
            spinner.set_visible(false);
        }
        right_box.append(&spinner);

        let menu_btn = MenuButton::new();
        menu_btn.set_icon_name("view-more-symbolic");
        menu_btn.add_css_class("flat");
        menu_btn.set_tooltip_text(Some("Cloud PC Actions"));

        let popover = Popover::new();
        let menu_box = Box::new(Orientation::Vertical, 2);
        menu_box.set_margin_start(4);
        menu_box.set_margin_end(4);
        menu_box.set_margin_top(4);
        menu_box.set_margin_bottom(4);

        let pc_actions = pc.clone();
        let s_act = spinner.clone();
        let r_act = row_box.clone();
        let on_act_rc = Rc::new(on_action.clone());

        Self::populate_menu_box(&menu_box, &popover, &pc_actions, s_act, r_act, on_act_rc);

        popover.set_child(Some(&menu_box));
        menu_btn.set_popover(Some(&popover));
        right_box.append(&menu_btn);

        row_box.append(&right_box);

        // Attach GestureClick to click_area ONLY
        let gesture = GestureClick::new();
        let pc_click = pc.clone();
        let s_click = spinner;
        let r_click = row_box.clone();
        let on_act_click = on_action;

        gesture.connect_pressed(move |_, _, _, _| {
            r_click.set_sensitive(false);
            s_click.set_visible(true);
            s_click.start();
            on_act_click(
                CardAction::Connect(pc_click.clone()),
                s_click.clone(),
                r_click.clone(),
            );
        });
        click_area.add_controller(gesture);

        row_box.upcast()
    }

    /// Populates the 3-dots auxiliary menu options.
    fn populate_menu_box<F>(
        menu_box: &Box,
        popover: &Popover,
        pc: &CloudPc,
        spinner: Spinner,
        container: Box,
        on_action: Rc<F>,
    ) where
        F: Fn(CardAction, Spinner, Box) + 'static,
    {
        // 1. Connect
        let connect_item = Button::with_label("Connect");
        connect_item.add_css_class("flat");
        connect_item.set_halign(gtk4::Align::Fill);
        if let Some(lbl) = connect_item.child().and_downcast::<Label>() {
            lbl.set_halign(gtk4::Align::Start);
            lbl.set_xalign(0.0);
        }
        let pc_c = pc.clone();
        let s_c = spinner.clone();
        let c_c = container.clone();
        let a_c = Rc::clone(&on_action);
        let p_c = popover.clone();
        connect_item.connect_clicked(move |_| {
            p_c.popdown();
            c_c.set_sensitive(false);
            s_c.set_visible(true);
            s_c.start();
            a_c(CardAction::Connect(pc_c.clone()), s_c.clone(), c_c.clone());
        });
        menu_box.append(&connect_item);

        // 2. Restart
        let restart_item = Button::with_label("Restart");
        restart_item.add_css_class("flat");
        restart_item.set_halign(gtk4::Align::Fill);
        if let Some(lbl) = restart_item.child().and_downcast::<Label>() {
            lbl.set_halign(gtk4::Align::Start);
            lbl.set_xalign(0.0);
        }
        let pc_id = pc.id.clone();
        let s_r = spinner.clone();
        let c_r = container.clone();
        let a_r = Rc::clone(&on_action);
        let p_r = popover.clone();
        restart_item.connect_clicked(move |_| {
            p_r.popdown();
            a_r(CardAction::Restart(pc_id.clone()), s_r.clone(), c_r.clone());
        });
        menu_box.append(&restart_item);

        // 3. Rename
        let rename_item = Button::with_label("Rename");
        rename_item.add_css_class("flat");
        rename_item.set_halign(gtk4::Align::Fill);
        if let Some(lbl) = rename_item.child().and_downcast::<Label>() {
            lbl.set_halign(gtk4::Align::Start);
            lbl.set_xalign(0.0);
        }
        let pc_ren = pc.clone();
        let s_ren = spinner.clone();
        let c_ren = container.clone();
        let a_ren = Rc::clone(&on_action);
        let p_ren = popover.clone();
        rename_item.connect_clicked(move |_| {
            p_ren.popdown();
            a_ren(
                CardAction::Rename(pc_ren.clone()),
                s_ren.clone(),
                c_ren.clone(),
            );
        });
        menu_box.append(&rename_item);

        // 4. Inspect Connection
        let inspect_item = Button::with_label("Inspect Connection");
        inspect_item.add_css_class("flat");
        inspect_item.set_halign(gtk4::Align::Fill);
        if let Some(lbl) = inspect_item.child().and_downcast::<Label>() {
            lbl.set_halign(gtk4::Align::Start);
            lbl.set_xalign(0.0);
        }
        let pc_ins = pc.clone();
        let s_ins = spinner.clone();
        let c_ins = container.clone();
        let a_ins = Rc::clone(&on_action);
        let p_ins = popover.clone();
        inspect_item.connect_clicked(move |_| {
            p_ins.popdown();
            a_ins(
                CardAction::Inspect(pc_ins.clone()),
                s_ins.clone(),
                c_ins.clone(),
            );
        });
        menu_box.append(&inspect_item);

        // 5. View Details
        let details_item = Button::with_label("View Details");
        details_item.add_css_class("flat");
        details_item.set_halign(gtk4::Align::Fill);
        if let Some(lbl) = details_item.child().and_downcast::<Label>() {
            lbl.set_halign(gtk4::Align::Start);
            lbl.set_xalign(0.0);
        }
        let pc_det = pc.clone();
        let s_det = spinner.clone();
        let c_det = container.clone();
        let a_det = Rc::clone(&on_action);
        let p_det = popover.clone();
        details_item.connect_clicked(move |_| {
            p_det.popdown();
            a_det(
                CardAction::Details(pc_det.clone()),
                s_det.clone(),
                c_det.clone(),
            );
        });
        menu_box.append(&details_item);

        // 6. Open in Web Browser (if directLaunchURL is available)
        if let Some(url) = &pc.direct_launch_url {
            let web_item = Button::with_label("Open in Web Browser");
            web_item.add_css_class("flat");
            web_item.set_halign(gtk4::Align::Fill);
            if let Some(lbl) = web_item.child().and_downcast::<Label>() {
                lbl.set_halign(gtk4::Align::Start);
                lbl.set_xalign(0.0);
            }
            let launch_url = url.clone();
            let p_web = popover.clone();
            web_item.connect_clicked(move |_| {
                p_web.popdown();
                let _ = open::that(&launch_url);
            });
            menu_box.append(&web_item);
        }

        menu_box.append(&Separator::new(Orientation::Horizontal));

        // 7. Settings
        let settings_item = Button::with_label("Settings");
        settings_item.add_css_class("flat");
        settings_item.set_halign(gtk4::Align::Fill);
        if let Some(lbl) = settings_item.child().and_downcast::<Label>() {
            lbl.set_halign(gtk4::Align::Start);
            lbl.set_xalign(0.0);
        }
        let s_set = spinner;
        let c_set = container;
        let a_set = on_action;
        let p_set = popover.clone();
        settings_item.connect_clicked(move |_| {
            p_set.popdown();
            a_set(CardAction::Settings, s_set.clone(), c_set.clone());
        });
        menu_box.append(&settings_item);
    }
}
