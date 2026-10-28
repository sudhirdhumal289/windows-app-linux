//! Preferences dialog for identity profile, display, device redirection and folder sharing.
//! Display scale is chosen per session from the session window.

use super::gnome_shell::{self, HelperState};
use crate::config::{AppConfig, AuthProfile, CloudEnvironment, SharedFolder};
use crate::platform;
use crate::session::{GraphicsMode, Quality};
use adw::prelude::*;
use gtk4::{Button, StringList, Window};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

/// Graphics modes in the order of the combo row.
pub(super) const GRAPHICS_MODES: [(GraphicsMode, &str); 2] = [
    (GraphicsMode::Auto, "Automatic (H.264 / AVC444)"),
    (GraphicsMode::RemoteFx, "RemoteFX (low latency LAN)"),
];

pub struct PreferencesDialog;

impl PreferencesDialog {
    pub fn show(parent: &impl IsA<Window>, on_save: impl Fn(AppConfig) + 'static) {
        let config = AppConfig::load();
        let parent_win: Window = parent.clone().upcast();
        let present_parent = parent_win.clone();

        let dialog = adw::Dialog::builder()
            .title("Preferences")
            .content_width(560)
            .content_height(680)
            .build();

        let header = adw::HeaderBar::builder()
            .show_start_title_buttons(false)
            .show_end_title_buttons(false)
            .build();

        let cancel_btn = Button::with_label("Cancel");
        let dlg_cancel = dialog.clone();
        cancel_btn.connect_clicked(move |_| {
            dlg_cancel.close();
        });
        header.pack_start(&cancel_btn);

        let save_btn = Button::with_label("Save");
        save_btn.add_css_class("suggested-action");
        save_btn.set_sensitive(false);
        header.pack_end(&save_btn);

        let page = adw::PreferencesPage::new();

        // Section 1: Authentication & Identity Profile
        let profile_group = adw::PreferencesGroup::builder()
            .title("Identity &amp; Authentication")
            .description("Select the platform identity profile and Microsoft cloud environment.")
            .build();
        let selected_profile = config.auth_profile().index();
        let profile_labels: Vec<&str> = AuthProfile::ALL.iter().map(|p| p.label()).collect();
        let profile_row = Self::combo_row("Sign-in Profile", &profile_labels, selected_profile);
        profile_group.add(&profile_row);

        let env_labels: Vec<&str> = CloudEnvironment::ALL.iter().map(|e| e.label()).collect();
        let selected_env = CloudEnvironment::ALL
            .iter()
            .position(|e| *e == config.cloud_environment)
            .unwrap_or(0) as u32;
        let env_row = Self::combo_row("Cloud Environment", &env_labels, selected_env);
        env_row.set_subtitle(
            "Commercial (Global), US Government (GCC High / DoD), or China (21Vianet)",
        );
        profile_group.add(&env_row);
        page.add(&profile_group);

        // Display
        let display_group = adw::PreferencesGroup::builder().title("Display").build();
        let multimon_row = Self::switch_row(
            "Use All Monitors",
            Some("Span the session across every monitor while in fullscreen"),
            config.multi_monitor,
        );
        display_group.add(&multimon_row);
        let graphics_labels: Vec<&str> = GRAPHICS_MODES.iter().map(|(_, l)| l).copied().collect();
        let selected_graphics = GRAPHICS_MODES
            .iter()
            .position(|(m, _)| *m == GraphicsMode::from_id(&config.graphics_mode))
            .unwrap_or(0) as u32;
        let graphics_row = Self::combo_row("Graphics", &graphics_labels, selected_graphics);
        display_group.add(&graphics_row);
        let quality_labels: Vec<&str> = Quality::ALL.iter().map(|q| q.label()).collect();
        let selected_quality = Quality::ALL
            .iter()
            .position(|q| *q == Quality::from_id(&config.quality))
            .unwrap_or(0) as u32;
        let quality_row = Self::combo_row("Connection Quality", &quality_labels, selected_quality);
        quality_row.set_subtitle("Visual effects and colour depth for the network in use");
        display_group.add(&quality_row);
        page.add(&display_group);

        // GNOME on Wayland can only restore workspaces through the GNOME Shell helper.
        let helper_state = Self::is_wayland()
            .then(gnome_shell::state)
            .filter(|state| *state != HelperState::Unavailable);
        let workspace_row = helper_state.map(|state| {
            let row = Self::switch_row(
                "Restore Workspaces",
                Some(Self::helper_subtitle(state)),
                state.is_enabled(),
            );
            row.set_sensitive(state != HelperState::NotInstalled);
            let group = adw::PreferencesGroup::builder()
                .title("Session Windows")
                .build();
            group.add(&row);
            page.add(&group);
            row
        });
        let init_workspaces = helper_state.is_some_and(HelperState::is_enabled);

        // Section 2: Peripherals & Redirection
        let periph_group = adw::PreferencesGroup::builder()
            .title("Peripherals &amp; Redirection")
            .build();
        let clipboard_row = Self::switch_row(
            "Clipboard Sharing",
            Some("Bidirectional copy and paste"),
            config.enable_clipboard,
        );
        let audio_row = Self::switch_row("Audio Output Redirection", None, config.enable_audio);
        let mic_row = Self::switch_row("Microphone Pass-Through", None, config.enable_microphone);
        let cam_row = Self::switch_row(
            "Camera / Webcam Pass-Through",
            Some("MS-RDPECAM"),
            config.enable_camera,
        );
        let smartcard_row = Self::switch_row(
            "Smart Cards",
            Some("Use local smart cards and security keys in the session"),
            config.enable_smartcard,
        );
        let printer_row = Self::switch_row(
            "Printers",
            Some("Make local printers available in the session"),
            config.enable_printers,
        );
        for row in [
            &clipboard_row,
            &audio_row,
            &mic_row,
            &cam_row,
            &smartcard_row,
            &printer_row,
        ] {
            periph_group.add(row);
        }
        page.add(&periph_group);

        // USB devices
        let usb_group = adw::PreferencesGroup::builder()
            .title("USB Devices")
            .description(
                "Redirect USB devices to the session. The device becomes unavailable on this \
                 computer while a session uses it.",
            )
            .build();
        let usb_selection = Rc::new(RefCell::new(
            config
                .usb_devices
                .iter()
                .filter_map(|id| crate::session::UsbId::parse(id))
                .map(|id| id.as_str().to_string())
                .collect::<BTreeSet<_>>(),
        ));
        let init_usb = usb_selection.borrow().clone();
        let mut usb_rows = Vec::new();
        let attached = platform::list_usb_devices();
        for device in &attached {
            let subtitle = if device.accessible {
                device.id.clone()
            } else {
                format!("{} · needs access permission (udev rule)", device.id)
            };
            let row = Self::switch_row(
                &glib_escape(&device.name),
                Some(&subtitle),
                init_usb.contains(&device.id),
            );
            usb_rows.push((device.id.clone(), row));
        }
        for id in &init_usb {
            if !attached.iter().any(|d| &d.id == id) {
                let row = Self::switch_row(id, Some("Not connected"), true);
                usb_rows.push((id.clone(), row));
            }
        }
        if usb_rows.is_empty() {
            let row = adw::ActionRow::builder()
                .title("No USB devices found")
                .build();
            row.add_css_class("dim-label");
            usb_group.add(&row);
        }
        for (_, row) in &usb_rows {
            usb_group.add(row);
        }
        page.add(&usb_group);

        // Section 3: Local Folder Sharing (Drive Redirection)
        let drive_group = adw::PreferencesGroup::builder()
            .title("Local Folder Sharing")
            .description(
                "Share a local Linux folder with your Cloud PC session. In Windows File Explorer, access it under 'This PC' in 'Redirected drives and folders' or by typing \\\\tsclient\\Shared in the address bar (or Run dialog).",
            )
            .build();
        let drive_row = Self::switch_row(
            "Share Local Folder with Cloud PC",
            None,
            config.enable_drive_redirection,
        );
        drive_group.add(&drive_row);

        let shared_path = Rc::new(RefCell::new(config.shared_folder_path.clone()));
        let folder_row = adw::ActionRow::builder()
            .title("Shared Folder")
            .subtitle(Self::folder_subtitle(&config.shared_folder_path))
            .subtitle_selectable(true)
            .build();
        folder_row.add_css_class("property");
        let browse_btn = Button::builder()
            .icon_name("folder-open-symbolic")
            .tooltip_text("Select Folder…")
            .valign(gtk4::Align::Center)
            .build();
        browse_btn.add_css_class("flat");
        folder_row.add_suffix(&browse_btn);
        folder_row.set_activatable_widget(Some(&browse_btn));
        folder_row.set_sensitive(config.enable_drive_redirection);
        drive_group.add(&folder_row);
        let hotplug_row = Self::switch_row(
            "Removable Drives",
            Some("Share USB sticks and other removable drives, including ones plugged in later"),
            config.hotplug_drives,
        );
        drive_group.add(&hotplug_row);
        page.add(&drive_group);

        let extra_folders = Rc::new(RefCell::new(config.extra_folders.clone()));
        let extra_group = adw::PreferencesGroup::builder()
            .title("More Shared Folders")
            .description("Each folder appears in the session as a drive with its own name.")
            .build();
        let add_folder_btn = Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("Add Folder…")
            .valign(gtk4::Align::Center)
            .build();
        add_folder_btn.add_css_class("flat");
        extra_group.set_header_suffix(Some(&add_folder_btn));
        page.add(&extra_group);

        // --- Dirty State Tracking (Enable Save button only if values changed) ---
        let init_clipboard = config.enable_clipboard;
        let init_audio = config.enable_audio;
        let init_mic = config.enable_microphone;
        let init_cam = config.enable_camera;
        let init_drive_redir = config.enable_drive_redirection;
        let init_shared_path = config.shared_folder_path.clone();

        let init_multimon = config.multi_monitor;
        let init_smartcard = config.enable_smartcard;
        let init_printers = config.enable_printers;
        let init_hotplug = config.hotplug_drives;
        let init_extra = config.extra_folders.clone();

        let check_dirty = {
            let save_btn = save_btn.clone();
            let mm = multimon_row.clone();
            let gr = graphics_row.clone();
            let sc = smartcard_row.clone();
            let pr = printer_row.clone();
            let usb = Rc::clone(&usb_selection);
            let init_usb = init_usb.clone();
            let p_row = profile_row.clone();
            let e_row = env_row.clone();
            let init_env = selected_env;
            let cl = clipboard_row.clone();
            let a = audio_row.clone();
            let m = mic_row.clone();
            let c = cam_row.clone();
            let dr = drive_row.clone();
            let path = Rc::clone(&shared_path);
            let q = quality_row.clone();
            let hp = hotplug_row.clone();
            let extra = Rc::clone(&extra_folders);
            let ws = workspace_row.clone();
            Rc::new(move || {
                let dirty = p_row.selected() != selected_profile
                    || e_row.selected() != init_env
                    || cl.is_active() != init_clipboard
                    || a.is_active() != init_audio
                    || m.is_active() != init_mic
                    || c.is_active() != init_cam
                    || dr.is_active() != init_drive_redir
                    || *path.borrow() != init_shared_path
                    || mm.is_active() != init_multimon
                    || gr.selected() != selected_graphics
                    || sc.is_active() != init_smartcard
                    || pr.is_active() != init_printers
                    || *usb.borrow() != init_usb
                    || q.selected() != selected_quality
                    || hp.is_active() != init_hotplug
                    || *extra.borrow() != init_extra
                    || ws
                        .as_ref()
                        .is_some_and(|ws| ws.is_active() != init_workspaces);
                save_btn.set_sensitive(dirty);
            })
        };

        let cd = Rc::clone(&check_dirty);
        profile_row.connect_selected_notify(move |_| cd());
        let cd = Rc::clone(&check_dirty);
        env_row.connect_selected_notify(move |_| cd());
        let cd = Rc::clone(&check_dirty);
        graphics_row.connect_selected_notify(move |_| cd());
        let cd = Rc::clone(&check_dirty);
        quality_row.connect_selected_notify(move |_| cd());
        for row in [
            &hotplug_row,
            &clipboard_row,
            &audio_row,
            &mic_row,
            &cam_row,
            &multimon_row,
            &smartcard_row,
            &printer_row,
        ]
        .into_iter()
        .chain(workspace_row.as_ref())
        {
            let cd = Rc::clone(&check_dirty);
            row.connect_active_notify(move |_| cd());
        }
        for (id, row) in &usb_rows {
            let cd = Rc::clone(&check_dirty);
            let usb = Rc::clone(&usb_selection);
            let id = id.clone();
            row.connect_active_notify(move |row| {
                if row.is_active() {
                    usb.borrow_mut().insert(id.clone());
                } else {
                    usb.borrow_mut().remove(&id);
                }
                cd();
            });
        }

        let cd = Rc::clone(&check_dirty);
        let folder_row_toggle = folder_row.clone();
        drive_row.connect_active_notify(move |row| {
            folder_row_toggle.set_sensitive(row.is_active());
            cd();
        });

        let folder_row_cb = folder_row.clone();
        let path_cb = Rc::clone(&shared_path);
        let cd_browse = Rc::clone(&check_dirty);
        browse_btn.connect_clicked(move |_| {
            let file_dialog = gtk4::FileDialog::builder()
                .title("Select Folder to Share with Cloud PC")
                .modal(true)
                .build();

            let folder_row_inner = folder_row_cb.clone();
            let path_inner = Rc::clone(&path_cb);
            let cd_inner = Rc::clone(&cd_browse);
            file_dialog.select_folder(
                Some(&parent_win),
                gtk4::gio::Cancellable::NONE,
                move |result| {
                    if let Ok(file) = result {
                        if let Some(path) = file.path() {
                            let path = path.to_string_lossy().to_string();
                            folder_row_inner.set_subtitle(&Self::folder_subtitle(&path));
                            *path_inner.borrow_mut() = path;
                            cd_inner();
                        }
                    }
                },
            );
        });

        let folder_list = FolderList {
            group: extra_group,
            folders: Rc::clone(&extra_folders),
            rows: RefCell::new(Vec::new()),
            on_change: {
                let cd = Rc::clone(&check_dirty);
                std::boxed::Box::new(move || cd())
            },
        };
        let folder_list = Rc::new(folder_list);
        folder_list.refresh();
        let list = Rc::clone(&folder_list);
        let parent_add = present_parent.clone();
        add_folder_btn.connect_clicked(move |_| {
            let list = Rc::clone(&list);
            gtk4::FileDialog::builder()
                .title("Select Folder to Share")
                .modal(true)
                .build()
                .select_folder(
                    Some(&parent_add),
                    gtk4::gio::Cancellable::NONE,
                    move |result| {
                        if let Some(path) = result.ok().and_then(|f| f.path()) {
                            list.add(&path);
                        }
                    },
                );
        });

        // Wire Save Action
        let dlg_save = dialog.clone();
        let on_save = Rc::new(on_save);
        save_btn.connect_clicked(move |_| {
            let mut updated_config = AppConfig::load();
            updated_config.auth_profile_index = profile_row.selected();
            if let Some(&env) = CloudEnvironment::ALL.get(env_row.selected() as usize) {
                updated_config.cloud_environment = env;
                updated_config.data_boundary = env.data_boundary().to_string();
            }
            updated_config.enable_clipboard = clipboard_row.is_active();
            updated_config.enable_audio = audio_row.is_active();
            updated_config.enable_microphone = mic_row.is_active();
            updated_config.enable_camera = cam_row.is_active();
            updated_config.enable_drive_redirection = drive_row.is_active();
            updated_config.shared_folder_path = shared_path.borrow().clone();
            updated_config.multi_monitor = multimon_row.is_active();
            updated_config.graphics_mode = GRAPHICS_MODES
                .get(graphics_row.selected() as usize)
                .map_or(GraphicsMode::Auto, |(m, _)| *m)
                .id()
                .to_string();
            updated_config.enable_smartcard = smartcard_row.is_active();
            updated_config.enable_printers = printer_row.is_active();
            updated_config.quality = Quality::ALL
                .get(quality_row.selected() as usize)
                .copied()
                .unwrap_or_default()
                .id()
                .to_string();
            updated_config.hotplug_drives = hotplug_row.is_active();
            updated_config.extra_folders = extra_folders.borrow().clone();
            updated_config.usb_devices = usb_selection.borrow().iter().cloned().collect();

            if let Err(e) = updated_config.save() {
                log::warn!("Failed to save configuration: {e}");
            }
            if let Some(row) = workspace_row.as_ref() {
                if row.is_active() != init_workspaces {
                    let result = if row.is_active() {
                        gnome_shell::enable()
                    } else {
                        gnome_shell::disable()
                    };
                    if let Err(e) = result {
                        log::warn!("{e}");
                    }
                }
            }

            on_save(updated_config);
            dlg_save.close();
        });

        let toolbar_view = adw::ToolbarView::new();
        toolbar_view.add_top_bar(&header);
        toolbar_view.set_content(Some(&page));
        dialog.set_child(Some(&toolbar_view));
        dialog.present(Some(&present_parent));
    }

    fn combo_row(title: &str, options: &[&str], selected: u32) -> adw::ComboRow {
        let row = adw::ComboRow::builder()
            .title(title)
            .model(&StringList::new(options))
            .use_subtitle(true)
            .build();
        row.set_selected(selected);
        row
    }

    fn switch_row(title: &str, subtitle: Option<&str>, active: bool) -> adw::SwitchRow {
        let row = adw::SwitchRow::builder()
            .title(title)
            .active(active)
            .build();
        if let Some(subtitle) = subtitle {
            row.set_subtitle(subtitle);
        }
        row
    }

    fn is_wayland() -> bool {
        gtk4::gdk::Display::default()
            .is_some_and(|display| display.type_().name() == "GdkWaylandDisplay")
    }

    fn helper_subtitle(state: HelperState) -> &'static str {
        match state {
            HelperState::On => "Session windows reopen on the workspace they were last on",
            HelperState::Pending => "Log out and back in once to finish installing it",
            HelperState::Blocked => {
                "GNOME Shell is not running the helper: extensions are turned off or this \
                 GNOME version is not supported"
            }
            HelperState::NotInstalled => {
                "Needs the GNOME Shell helper that the app packages install"
            }
            HelperState::Off | HelperState::Unavailable => {
                "Reopen session windows on the workspace they were last on, using the \
                 GNOME Shell helper installed with this app"
            }
        }
    }

    fn folder_subtitle(path: &str) -> String {
        if path.is_empty() {
            "No folder selected".to_string()
        } else {
            gtk4::glib::markup_escape_text(path).to_string()
        }
    }
}

/// The editable list of extra shared folders.
struct FolderList {
    group: adw::PreferencesGroup,
    folders: Rc<RefCell<Vec<SharedFolder>>>,
    rows: RefCell<Vec<adw::ActionRow>>,
    on_change: std::boxed::Box<dyn Fn()>,
}

impl FolderList {
    fn refresh(self: &Rc<Self>) {
        for row in self.rows.take() {
            self.group.remove(&row);
        }
        let folders = self.folders.borrow().clone();
        let mut rows = Vec::new();
        if folders.is_empty() {
            let row = adw::ActionRow::builder().title("No extra folders").build();
            row.add_css_class("dim-label");
            rows.push(row);
        }
        for (index, folder) in folders.iter().enumerate() {
            let row = adw::ActionRow::builder()
                .title(glib_escape(&folder.name))
                .subtitle(glib_escape(&folder.path))
                .build();
            let remove = Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text("Stop Sharing")
                .valign(gtk4::Align::Center)
                .build();
            remove.add_css_class("flat");
            let weak = Rc::downgrade(self);
            remove.connect_clicked(move |_| {
                if let Some(list) = weak.upgrade() {
                    list.folders.borrow_mut().remove(index);
                    list.refresh();
                    (list.on_change)();
                }
            });
            row.add_suffix(&remove);
            rows.push(row);
        }
        for row in &rows {
            self.group.add(row);
        }
        *self.rows.borrow_mut() = rows;
    }

    /// Shares `path` under a drive name derived from the folder name.
    fn add(self: &Rc<Self>, path: &std::path::Path) {
        let path_text = path.to_string_lossy().into_owned();
        let base = drive_name(path);
        let folder = {
            let folders = self.folders.borrow();
            if folders.iter().any(|f| f.path == path_text) {
                return;
            }
            let taken = |name: &str| {
                name.eq_ignore_ascii_case("Shared")
                    || folders.iter().any(|f| f.name.eq_ignore_ascii_case(name))
            };
            let name = (1..)
                .map(|n| {
                    if n == 1 {
                        base.clone()
                    } else {
                        let suffix = format!(" {n}");
                        let keep = SharedFolder::MAX_NAME_LEN - suffix.len();
                        format!("{}{suffix}", base.chars().take(keep).collect::<String>())
                    }
                })
                .find(|name| !taken(name))
                .unwrap_or(base);
            SharedFolder {
                name,
                path: path_text,
            }
        };
        if !folder.is_valid() {
            log::warn!(
                "Cannot share {}: commas are not supported in paths",
                folder.path
            );
            return;
        }
        self.folders.borrow_mut().push(folder);
        self.refresh();
        (self.on_change)();
    }
}

/// A valid drive name for the folder at `path`.
fn drive_name(path: &std::path::Path) -> String {
    let name: String = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .take(SharedFolder::MAX_NAME_LEN)
        .collect();
    let name = name.trim().to_string();
    if name.is_empty() {
        "Folder".to_string()
    } else {
        name
    }
}

fn glib_escape(text: &str) -> String {
    gtk4::glib::markup_escape_text(text).to_string()
}
