//! Preferences dialog for configuring identity profile, RDP engine, display settings, and folder sharing.

use crate::config::AppConfig;
use crate::rdp::RdpLauncher;
use gtk4::prelude::*;
use gtk4::{
    Box, Button, CheckButton, DropDown, Entry, HeaderBar, Label, Orientation, PolicyType,
    ScrolledWindow, Separator, StringList, Window,
};
use std::rc::Rc;

pub struct PreferencesDialog;

impl PreferencesDialog {
    pub fn show(parent: &impl IsA<Window>, on_save: impl Fn(AppConfig) + 'static) {
        let config = AppConfig::load();

        let window = Window::builder()
            .transient_for(parent)
            .modal(true)
            .title("Preferences - Windows App")
            .default_width(540)
            .default_height(600)
            .build();

        let header = HeaderBar::new();
        header.set_show_title_buttons(true);

        let cancel_btn = Button::with_label("Cancel");
        let win_cancel = window.clone();
        cancel_btn.connect_clicked(move |_| {
            win_cancel.close();
        });
        header.pack_start(&cancel_btn);

        let save_btn = Button::with_label("Save");
        save_btn.add_css_class("suggested-action");
        save_btn.set_sensitive(false);
        header.pack_end(&save_btn);

        window.set_titlebar(Some(&header));

        let main_box = Box::new(Orientation::Vertical, 16);
        main_box.set_margin_start(24);
        main_box.set_margin_end(24);
        main_box.set_margin_top(20);
        main_box.set_margin_bottom(24);

        // Section 1: Authentication & Identity Profile
        let profile_title = Label::new(Some("Identity & Authentication Profile"));
        profile_title.set_halign(gtk4::Align::Start);
        profile_title.add_css_class("title-4");
        main_box.append(&profile_title);

        let profile_desc = Label::new(Some(
            "Select the platform identity profile used during Microsoft sign-in.",
        ));
        profile_desc.set_halign(gtk4::Align::Start);
        profile_desc.add_css_class("dim-label");
        main_box.append(&profile_desc);

        let profile_options = StringList::new(&[
            "Windows 11 Windows App (WAM Broker) [Default]",
            "macOS Native (Remote Desktop for Mac)",
            "Legacy AVD Client (Multi-platform a85cf173)",
            "Common Endpoint (Fallback / All Tenants)",
        ]);
        let profile_dropdown = DropDown::new(Some(profile_options), gtk4::Expression::NONE);
        let selected_profile = if config.auth_profile_index <= 3 {
            config.auth_profile_index
        } else {
            0
        };
        profile_dropdown.set_selected(selected_profile);
        main_box.append(&profile_dropdown);

        main_box.append(&Separator::new(Orientation::Horizontal));

        // Section 2: RDP Connection Engine
        let rdp_title = Label::new(Some("RDP Connection Engine"));
        rdp_title.set_halign(gtk4::Align::Start);
        rdp_title.add_css_class("title-4");
        main_box.append(&rdp_title);

        let rdp_desc = Label::new(Some(
            "• In-App Native Window: FreeRDP 3 embedded directly inside Windows App with native GTK title bar, dynamic resizing, and controls.\n• IronRDP: Pure Rust client (direct connections only; Cloud PC ARM Gateway is not supported).",
        ));
        rdp_desc.set_halign(gtk4::Align::Start);
        rdp_desc.add_css_class("dim-label");
        main_box.append(&rdp_desc);

        let engine_options = StringList::new(&[
            "In-App Native Window (FreeRDP 3 C-FFI Engine) [Recommended]",
            "IronRDP (Pure Rust - Direct RDP only; AVD Gateway unsupported)",
        ]);
        let engine_dropdown = DropDown::new(Some(engine_options), gtk4::Expression::NONE);
        let engine_idx = match config.preferred_rdp_client.as_str() {
            "ironrdp" => 1,
            _ => 0,
        };
        engine_dropdown.set_selected(engine_idx);
        main_box.append(&engine_dropdown);

        main_box.append(&Separator::new(Orientation::Horizontal));

        // Section 3: Display & Scaling
        let scale_title = Label::new(Some("Display Scaling Factor"));
        scale_title.set_halign(gtk4::Align::Start);
        scale_title.add_css_class("title-4");
        main_box.append(&scale_title);

        let detected_scale = RdpLauncher::get_system_desktop_scale();
        let scale_desc = Label::new(Some(&format!(
            "Automatically detected GNOME scale: {detected_scale}%. Customizes Windows DPI.",
        )));
        scale_desc.set_halign(gtk4::Align::Start);
        scale_desc.add_css_class("dim-label");
        main_box.append(&scale_desc);

        let scale_options = StringList::new(&[
            "Auto (Detect from GNOME - Recommended)",
            "100% (Standard 96 DPI)",
            "125% (120 DPI)",
            "150% (144 DPI)",
            "175% (168 DPI)",
            "200% (HiDPI 192 DPI)",
        ]);
        let scale_dropdown = DropDown::new(Some(scale_options), gtk4::Expression::NONE);
        let scale_idx = match config.default_scale_factor {
            0 => 0,
            100 => 1,
            125 => 2,
            150 => 3,
            175 => 4,
            200 => 5,
            _ => 0,
        };
        scale_dropdown.set_selected(scale_idx);
        main_box.append(&scale_dropdown);

        main_box.append(&Separator::new(Orientation::Horizontal));

        // Section 4: Peripherals & Redirection
        let periph_title = Label::new(Some("Peripherals & Redirection"));
        periph_title.set_halign(gtk4::Align::Start);
        periph_title.add_css_class("title-4");
        main_box.append(&periph_title);

        let clipboard_check =
            CheckButton::with_label("Enable Bidirectional Clipboard Sharing (Copy/Paste)");
        clipboard_check.set_active(config.enable_clipboard);
        main_box.append(&clipboard_check);

        let audio_check = CheckButton::with_label("Enable Audio Output Redirection");
        audio_check.set_active(config.enable_audio);
        main_box.append(&audio_check);

        let mic_check = CheckButton::with_label("Enable Microphone Pass-Through");
        mic_check.set_active(config.enable_microphone);
        main_box.append(&mic_check);

        let cam_check = CheckButton::with_label("Enable Camera / Webcam Pass-Through (MS-RDPECAM)");
        cam_check.set_active(config.enable_camera);
        main_box.append(&cam_check);

        let multi_mon_check = CheckButton::with_label("Enable Multi-Monitor Support");
        multi_mon_check.set_active(config.multi_monitor);
        main_box.append(&multi_mon_check);

        main_box.append(&Separator::new(Orientation::Horizontal));

        // Section 5: Local Folder Sharing (Drive Redirection)
        let drive_title = Label::new(Some("Local Folder Sharing (Drive Redirection)"));
        drive_title.set_halign(gtk4::Align::Start);
        drive_title.add_css_class("title-4");
        main_box.append(&drive_title);

        let drive_desc = Label::new(Some(
            "Share a local Linux folder with your Cloud PC session. In Windows File Explorer, access it under 'This PC' in 'Redirected drives and folders' or by typing \\\\tsclient\\Shared in the address bar (or Run dialog).",
        ));
        drive_desc.set_halign(gtk4::Align::Start);
        drive_desc.add_css_class("dim-label");
        drive_desc.set_wrap(true);
        main_box.append(&drive_desc);

        let drive_check = CheckButton::with_label("Share Local Folder with Cloud PC");
        drive_check.set_active(config.enable_drive_redirection);
        main_box.append(&drive_check);

        let folder_box = Box::new(Orientation::Horizontal, 8);
        folder_box.set_hexpand(true);

        let folder_entry = Entry::builder()
            .text(&config.shared_folder_path)
            .placeholder_text("Select local folder...")
            .hexpand(true)
            .editable(false)
            .can_focus(false)
            .build();
        folder_box.append(&folder_entry);

        let browse_btn = Button::builder()
            .label("Browse...")
            .icon_name("folder-open-symbolic")
            .build();
        folder_box.append(&browse_btn);

        folder_box.set_sensitive(config.enable_drive_redirection);
        main_box.append(&folder_box);

        // --- Dirty State Tracking (Enable Save button only if values changed) ---
        let init_profile = selected_profile;
        let init_engine = engine_idx;
        let init_scale = scale_idx;
        let init_clipboard = config.enable_clipboard;
        let init_audio = config.enable_audio;
        let init_mic = config.enable_microphone;
        let init_cam = config.enable_camera;
        let init_multimon = config.multi_monitor;
        let init_drive_redir = config.enable_drive_redirection;
        let init_shared_path = config.shared_folder_path.clone();

        let check_dirty = {
            let save_btn = save_btn.clone();
            let p_dd = profile_dropdown.clone();
            let e_dd = engine_dropdown.clone();
            let s_dd = scale_dropdown.clone();
            let cl_ch = clipboard_check.clone();
            let a_ch = audio_check.clone();
            let m_ch = mic_check.clone();
            let c_ch = cam_check.clone();
            let mm_ch = multi_mon_check.clone();
            let dr_ch = drive_check.clone();
            let f_entry = folder_entry.clone();
            let init_path = init_shared_path.clone();
            Rc::new(move || {
                let dirty = p_dd.selected() != init_profile
                    || e_dd.selected() != init_engine
                    || s_dd.selected() != init_scale
                    || cl_ch.is_active() != init_clipboard
                    || a_ch.is_active() != init_audio
                    || m_ch.is_active() != init_mic
                    || c_ch.is_active() != init_cam
                    || mm_ch.is_active() != init_multimon
                    || dr_ch.is_active() != init_drive_redir
                    || f_entry.text().as_str() != init_path.as_str();
                save_btn.set_sensitive(dirty);
            })
        };

        let cd = Rc::clone(&check_dirty);
        profile_dropdown.connect_selected_notify(move |_| cd());

        let cd = Rc::clone(&check_dirty);
        engine_dropdown.connect_selected_notify(move |_| cd());

        let cd = Rc::clone(&check_dirty);
        scale_dropdown.connect_selected_notify(move |_| cd());

        let cd = Rc::clone(&check_dirty);
        clipboard_check.connect_toggled(move |_| cd());

        let cd = Rc::clone(&check_dirty);
        audio_check.connect_toggled(move |_| cd());

        let cd = Rc::clone(&check_dirty);
        mic_check.connect_toggled(move |_| cd());

        let cd = Rc::clone(&check_dirty);
        cam_check.connect_toggled(move |_| cd());

        let cd = Rc::clone(&check_dirty);
        multi_mon_check.connect_toggled(move |_| cd());

        let cd = Rc::clone(&check_dirty);
        let folder_box_toggle = folder_box.clone();
        drive_check.connect_toggled(move |chk| {
            folder_box_toggle.set_sensitive(chk.is_active());
            cd();
        });

        let win_browse = window.clone();
        let folder_entry_cb = folder_entry.clone();
        let cd_browse = Rc::clone(&check_dirty);
        browse_btn.connect_clicked(move |_| {
            let dialog = gtk4::FileDialog::builder()
                .title("Select Folder to Share with Cloud PC")
                .modal(true)
                .build();

            let folder_entry_inner = folder_entry_cb.clone();
            let cd_inner = Rc::clone(&cd_browse);
            dialog.select_folder(
                Some(&win_browse),
                gtk4::gio::Cancellable::NONE,
                move |result| {
                    if let Ok(file) = result {
                        if let Some(path) = file.path() {
                            folder_entry_inner.set_text(&path.to_string_lossy());
                            cd_inner();
                        }
                    }
                },
            );
        });

        // Wire Save Action
        let win_save = window.clone();
        let on_save = Rc::new(on_save);
        let on_save_cb = Rc::clone(&on_save);

        save_btn.connect_clicked(move |_| {
            let mut updated_config = AppConfig::load();
            updated_config.auth_profile_index = profile_dropdown.selected();
            updated_config.preferred_rdp_client = match engine_dropdown.selected() {
                1 => "ironrdp".to_string(),
                _ => "freerdp".to_string(),
            };
            updated_config.default_scale_factor = match scale_dropdown.selected() {
                1 => 100,
                2 => 125,
                3 => 150,
                4 => 175,
                5 => 200,
                _ => 0, // 0 = Auto!
            };
            updated_config.enable_clipboard = clipboard_check.is_active();
            updated_config.enable_audio = audio_check.is_active();
            updated_config.enable_microphone = mic_check.is_active();
            updated_config.enable_camera = cam_check.is_active();
            updated_config.multi_monitor = multi_mon_check.is_active();
            updated_config.enable_drive_redirection = drive_check.is_active();
            updated_config.shared_folder_path = folder_entry.text().to_string();

            if let Err(e) = updated_config.save() {
                log::warn!("Failed to save configuration: {e}");
            }

            on_save_cb(updated_config);
            win_save.close();
        });

        let scrolled = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .propagate_natural_height(true)
            .child(&main_box)
            .build();

        window.set_child(Some(&scrolled));
        window.present();
    }
}
