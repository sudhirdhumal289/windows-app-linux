//! Dialog displaying platform-specific installation instructions when FreeRDP is missing.

use crate::rdp::RdpLauncher;
use gtk4::gdk::Display;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{Box, Button, HeaderBar, Image, Label, Orientation, ScrolledWindow, Separator, Window};

pub struct MissingDependencyDialog;

impl MissingDependencyDialog {
    /// Shows a modal dialog with platform-specific instructions for installing FreeRDP.
    pub fn show<P: IsA<Window>>(parent: Option<&P>) {
        let window = Window::builder()
            .title("FreeRDP Required - Windows App")
            .default_width(640)
            .default_height(560)
            .modal(true)
            .build();

        if let Some(p) = parent {
            window.set_transient_for(Some(p));
        }

        let header = HeaderBar::new();
        header.set_show_title_buttons(true);
        window.set_titlebar(Some(&header));

        let main_box = Box::new(Orientation::Vertical, 16);
        main_box.set_margin_start(24);
        main_box.set_margin_end(24);
        main_box.set_margin_top(20);
        main_box.set_margin_bottom(20);

        // Header icon & titles
        let hero_box = Box::new(Orientation::Vertical, 8);
        hero_box.set_halign(gtk4::Align::Center);

        let warn_icon = Image::from_icon_name("dialog-warning-symbolic");
        warn_icon.set_pixel_size(48);
        warn_icon.add_css_class("warning");
        hero_box.append(&warn_icon);

        let title_lbl = Label::new(Some("FreeRDP 3 is Required"));
        title_lbl.add_css_class("title-2");
        hero_box.append(&title_lbl);

        let desc_lbl = Label::new(Some(
            "Windows App connects to Windows 365 Cloud PCs and Azure Virtual Desktops\nusing the FreeRDP 3 engine.\nPlease ensure FreeRDP 3 runtime libraries are installed for your system:",
        ));
        desc_lbl.set_justify(gtk4::Justification::Center);
        desc_lbl.add_css_class("dim-label");
        hero_box.append(&desc_lbl);

        main_box.append(&hero_box);

        // Platform instruction list inside a scrolled window
        let scroll = ScrolledWindow::new();
        scroll.set_vexpand(true);
        scroll.set_hscrollbar_policy(gtk4::PolicyType::Never);

        let list_box = Box::new(Orientation::Vertical, 10);
        list_box.set_margin_start(4);
        list_box.set_margin_end(4);

        let platforms = [
            (
                "Debian / Ubuntu / Linux Mint",
                "sudo apt update && sudo apt install -y libfreerdp3-3 libwinpr3-3",
            ),
            (
                "Fedora / RHEL / CentOS Stream",
                "sudo dnf install -y freerdp",
            ),
            ("Arch Linux / Manjaro", "sudo pacman -S --needed freerdp"),
            (
                "openSUSE (Tumbleweed / Leap)",
                "sudo zypper install -y freerdp",
            ),
            ("macOS (Homebrew)", "brew install freerdp"),
            ("Windows / WSL2", "winget install FreeRDP.FreeRDP"),
        ];

        for (os_name, cmd) in platforms {
            let item_card = Box::new(Orientation::Vertical, 6);
            item_card.add_css_class("card");
            item_card.set_margin_bottom(2);

            let row_header = Box::new(Orientation::Horizontal, 8);
            row_header.set_margin_start(12);
            row_header.set_margin_end(12);
            row_header.set_margin_top(8);

            let os_lbl = Label::new(Some(os_name));
            os_lbl.add_css_class("heading");
            os_lbl.set_halign(gtk4::Align::Start);
            os_lbl.set_hexpand(true);
            row_header.append(&os_lbl);

            let copy_btn = Button::with_label("Copy");
            copy_btn.add_css_class("flat");
            let cmd_text = cmd.to_string();
            let btn_clone = copy_btn.clone();
            copy_btn.connect_clicked(move |_| {
                if let Some(display) = Display::default() {
                    display.clipboard().set_text(&cmd_text);
                    btn_clone.set_label("✓ Copied");
                    let b = btn_clone.clone();
                    glib::timeout_add_local_once(
                        std::time::Duration::from_millis(1500),
                        move || {
                            b.set_label("Copy");
                        },
                    );
                }
            });
            row_header.append(&copy_btn);
            item_card.append(&row_header);

            let cmd_box = Box::new(Orientation::Horizontal, 8);
            cmd_box.set_margin_start(12);
            cmd_box.set_margin_end(12);
            cmd_box.set_margin_bottom(10);

            let cmd_lbl = Label::new(Some(cmd));
            cmd_lbl.set_halign(gtk4::Align::Start);
            cmd_lbl.set_hexpand(true);
            cmd_lbl.set_selectable(true);
            cmd_lbl.add_css_class("monospace");
            cmd_lbl.add_css_class("dim-label");
            cmd_box.append(&cmd_lbl);

            item_card.append(&cmd_box);
            list_box.append(&item_card);
        }

        scroll.set_child(Some(&list_box));
        main_box.append(&scroll);

        // Footer buttons
        let sep = Separator::new(Orientation::Horizontal);
        main_box.append(&sep);

        let footer_box = Box::new(Orientation::Horizontal, 10);
        footer_box.set_halign(gtk4::Align::End);

        let status_feedback = Label::new(None);
        status_feedback.set_halign(gtk4::Align::Start);
        status_feedback.set_hexpand(true);
        footer_box.append(&status_feedback);

        let dismiss_btn = Button::with_label("Dismiss");
        let win_dismiss = window.clone();
        dismiss_btn.connect_clicked(move |_| {
            win_dismiss.close();
        });
        footer_box.append(&dismiss_btn);

        let check_btn = Button::with_label("Check Again");
        check_btn.add_css_class("suggested-action");
        let win_check = window.clone();
        let status_fb = status_feedback;
        check_btn.connect_clicked(move |_| {
            if let Some(client) = RdpLauncher::detect_any_rdp_client() {
                status_fb.set_text(&format!("✓ Detected {client}! Closing..."));
                let w = win_check.clone();
                glib::timeout_add_local_once(std::time::Duration::from_millis(800), move || {
                    w.close();
                });
            } else {
                status_fb.set_text("FreeRDP not found yet. Please install FreeRDP 3 and retry.");
            }
        });
        footer_box.append(&check_btn);

        main_box.append(&footer_box);

        window.set_child(Some(&main_box));
        window.present();
    }
}
