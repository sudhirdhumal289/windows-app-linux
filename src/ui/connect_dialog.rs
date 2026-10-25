//! Modal dialog for entering credentials to launch an NLA Direct RDP session.

use crate::config::{AppConfig, SavedCredentials};
use gtk4::prelude::*;
use gtk4::{Box, Button, CheckButton, Entry, HeaderBar, Label, Orientation, PasswordEntry, Window};
use std::rc::Rc;

pub struct ConnectDialog;

impl ConnectDialog {
    /// Displays a modal credentials prompt for connecting to a Cloud PC via NLA ARM Gateway.
    pub fn show<P, FConnect>(
        parent: &P,
        pc_display_name: &str,
        default_username: &str,
        on_connect: FConnect,
    ) where
        P: IsA<Window>,
        FConnect: Fn(String, Option<String>, bool) + 'static,
    {
        let window = Window::builder()
            .transient_for(parent)
            .modal(true)
            .title(&format!("Connect - {pc_display_name}"))
            .default_width(460)
            .resizable(false)
            .build();

        let header = HeaderBar::new();
        let cancel_btn = Button::with_label("Cancel");
        let win_cancel = window.clone();
        cancel_btn.connect_clicked(move |_| {
            win_cancel.close();
        });
        header.pack_start(&cancel_btn);

        let connect_btn = Button::with_label("Connect");
        connect_btn.add_css_class("suggested-action");
        header.pack_end(&connect_btn);

        window.set_titlebar(Some(&header));

        let main_box = Box::new(Orientation::Vertical, 16);
        main_box.set_margin_start(24);
        main_box.set_margin_end(24);
        main_box.set_margin_top(20);
        main_box.set_margin_bottom(24);

        // Header section with Cloud PC title
        let title_box = Box::new(Orientation::Vertical, 4);
        let title_label = Label::new(Some(pc_display_name));
        title_label.add_css_class("title-2");
        title_label.set_halign(gtk4::Align::Start);
        title_box.append(&title_label);

        let subtitle_label = Label::new(Some(
            "Azure Virtual Desktop Gateway (ARM) • Direct NLA Connection",
        ));
        subtitle_label.add_css_class("dim-label");
        subtitle_label.set_halign(gtk4::Align::Start);
        title_box.append(&subtitle_label);

        main_box.append(&title_box);

        // Info card explaining the connection
        let info_box = Box::new(Orientation::Vertical, 4);
        info_box.add_css_class("card");
        info_box.set_margin_top(4);
        info_box.set_margin_bottom(8);

        let info_inner = Box::new(Orientation::Vertical, 4);
        info_inner.set_margin_start(12);
        info_inner.set_margin_end(12);
        info_inner.set_margin_top(8);
        info_inner.set_margin_bottom(8);

        let info_title = Label::new(Some("Direct Desktop Access"));
        info_title.add_css_class("heading");
        info_title.set_halign(gtk4::Align::Start);
        info_inner.append(&info_title);

        let info_desc = Label::new(Some(
            "Connects to the Windows 11 host directly via CredSSP / NLA credentials over Microsoft's authorized ARM gateway, without requiring Intune MDM compliance.",
        ));
        info_desc.set_wrap(true);
        info_desc.add_css_class("dim-label");
        info_desc.set_halign(gtk4::Align::Start);
        info_inner.append(&info_desc);

        info_box.append(&info_inner);
        main_box.append(&info_box);

        // Form Fields
        let form_box = Box::new(Orientation::Vertical, 12);

        // Username
        let user_label = Label::new(Some("User name (AzureAD\\username@domain.com)"));
        user_label.set_halign(gtk4::Align::Start);
        user_label.add_css_class("heading");
        form_box.append(&user_label);

        let user_entry = Entry::new();
        user_entry.set_placeholder_text(Some("AzureAD\\user@domain.com"));

        // Check if there are stored credentials
        let saved = AppConfig::load_credentials();
        let initial_username = if let Some(ref creds) = saved {
            if !creds.username.is_empty() {
                creds.username.clone()
            } else {
                default_username.to_string()
            }
        } else {
            default_username.to_string()
        };
        user_entry.set_text(&initial_username);
        form_box.append(&user_entry);

        // Password
        let pass_label = Label::new(Some("Password"));
        pass_label.set_halign(gtk4::Align::Start);
        pass_label.add_css_class("heading");
        form_box.append(&pass_label);

        let pass_entry = PasswordEntry::new();
        pass_entry.set_show_peek_icon(true);
        if let Some(ref creds) = saved {
            if let Some(ref pass) = creds.password {
                pass_entry.set_text(pass);
            }
        }
        form_box.append(&pass_entry);

        // Remember checkbox
        let remember_check = CheckButton::with_label("Remember password for future sessions");
        if let Some(ref creds) = saved {
            remember_check.set_active(creds.remember);
        } else {
            remember_check.set_active(true);
        }
        form_box.append(&remember_check);

        // Status / Error label
        let err_label = Label::new(None);
        err_label.add_css_class("error");
        err_label.set_visible(false);
        err_label.set_halign(gtk4::Align::Start);
        form_box.append(&err_label);

        main_box.append(&form_box);
        window.set_child(Some(&main_box));

        // Connect action handler
        let on_connect_rc = Rc::new(on_connect);
        let win_close = window.clone();
        let user_entry_clone = user_entry.clone();
        let pass_entry_clone = pass_entry.clone();
        let remember_clone = remember_check.clone();
        let err_clone = err_label.clone();

        let perform_connect = Rc::new(move || {
            let mut username = user_entry_clone.text().trim().to_string();
            if username.is_empty() {
                err_clone.set_text("Please enter a username.");
                err_clone.set_visible(true);
                return;
            }

            // Ensure username is prefixed with AzureAD\ if it's an email / UPN
            if !username.starts_with("AzureAD\\") && !username.contains('\\') {
                username = format!("AzureAD\\{username}");
            }

            let password_text = pass_entry_clone.text().to_string();
            let password = if password_text.is_empty() {
                None
            } else {
                Some(password_text)
            };

            let remember = remember_clone.is_active();

            // Save credentials if remember is checked
            if remember {
                let creds = SavedCredentials {
                    username: username.clone(),
                    password: password.clone(),
                    remember: true,
                };
                let _ = AppConfig::save_credentials(&creds);
            } else {
                let creds = SavedCredentials {
                    username: username.clone(),
                    password: None,
                    remember: false,
                };
                let _ = AppConfig::save_credentials(&creds);
            }

            win_close.close();
            on_connect_rc(username, password, remember);
        });

        let pc_fn = Rc::clone(&perform_connect);
        connect_btn.connect_clicked(move |_| {
            pc_fn();
        });

        let pc_fn2 = Rc::clone(&perform_connect);
        user_entry.connect_activate(move |_| {
            pc_fn2();
        });

        let pc_fn3 = Rc::clone(&perform_connect);
        pass_entry.connect_activate(move |_| {
            pc_fn3();
        });

        window.present();
    }
}
