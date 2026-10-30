//! Per-connection settings: values that replace the preferences for one connection
//! (resolution, graphics, quality, device redirection) and commands run around sessions.

use super::preferences_dialog::GRAPHICS_MODES;
use crate::config::AppConfig;
use crate::launcher::LaunchTarget;
use crate::overrides::{ConnectionOverrides, OverrideStore, Resolution};
use crate::session::{GraphicsMode, Quality};
use adw::prelude::*;
use gtk4::{Button, Label, StringList};
use std::rc::Rc;

/// A setting that is either inherited from the preferences or set for this connection.
struct Choice<T> {
    row: adw::ComboRow,
    /// Value per combo position after "Default".
    values: Vec<T>,
}

impl<T: Clone + PartialEq> Choice<T> {
    fn new(
        title: &str,
        default_label: &str,
        options: Vec<(T, String)>,
        current: Option<&T>,
    ) -> Self {
        let mut labels = vec![format!("Default ({default_label})")];
        labels.extend(options.iter().map(|(_, label)| label.clone()));
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        let row = adw::ComboRow::builder()
            .title(title)
            .model(&StringList::new(&labels))
            .build();
        let values: Vec<T> = options.into_iter().map(|(value, _)| value).collect();
        let selected = current
            .and_then(|c| values.iter().position(|v| v == c))
            .map_or(0, |i| i + 1);
        row.set_selected(selected as u32);
        Self { row, values }
    }

    fn value(&self) -> Option<T> {
        let index = self.row.selected() as usize;
        index
            .checked_sub(1)
            .and_then(|i| self.values.get(i).cloned())
    }
}

fn on_off(value: bool) -> &'static str {
    if value {
        "On"
    } else {
        "Off"
    }
}

fn toggle(title: &str, default: bool, current: Option<bool>) -> Choice<bool> {
    Choice::new(
        title,
        on_off(default),
        vec![(true, "On".into()), (false, "Off".into())],
        current.as_ref(),
    )
}

/// Shows the settings of `target` (shown as `name`).
pub fn show(parent: &impl IsA<gtk4::Widget>, target: LaunchTarget, name: &str) {
    let store = OverrideStore::default();
    let current = store.get(&target);
    let config = AppConfig::load();

    let dialog = adw::Dialog::builder()
        .title(format!("{name} Settings"))
        .content_width(520)
        .content_height(680)
        .build();
    let header = adw::HeaderBar::builder()
        .show_start_title_buttons(false)
        .show_end_title_buttons(false)
        .build();
    let cancel_btn = Button::with_label("Cancel");
    header.pack_start(&cancel_btn);
    let save_btn = Button::with_label("Save");
    save_btn.add_css_class("suggested-action");
    header.pack_end(&save_btn);

    // Display.
    let mut sizes: Vec<Resolution> = Resolution::PRESETS.to_vec();
    if let Some(size) = current.resolution.filter(|s| !sizes.contains(s)) {
        sizes.push(size);
    }
    let mut size_labels = vec!["Fit Window".to_string()];
    size_labels.extend(sizes.iter().map(ToString::to_string));
    size_labels.push("Custom…".to_string());
    let size_labels: Vec<&str> = size_labels.iter().map(String::as_str).collect();
    let resolution_row = adw::ComboRow::builder()
        .title("Resolution")
        .subtitle("A fixed size is scaled to the window")
        .model(&StringList::new(&size_labels))
        .build();
    let custom_index = sizes.len() as u32 + 1;
    resolution_row.set_selected(
        current
            .resolution
            .and_then(|r| sizes.iter().position(|s| *s == r))
            .map_or(0, |i| i as u32 + 1),
    );
    let custom_row = adw::EntryRow::builder()
        .title("Custom size, e.g. 1920x1080")
        .visible(false)
        .build();
    {
        let custom_row = custom_row.clone();
        resolution_row.connect_selected_notify(move |row| {
            custom_row.set_visible(row.selected() == custom_index);
        });
    }

    let default_graphics = GraphicsMode::from_id(&config.graphics_mode);
    let graphics = Choice::new(
        "Graphics",
        GRAPHICS_MODES
            .iter()
            .find(|(m, _)| *m == default_graphics)
            .map_or("Automatic", |(_, l)| l),
        GRAPHICS_MODES
            .iter()
            .map(|(m, l)| (m.id().to_string(), (*l).to_string()))
            .collect(),
        current.graphics_mode.as_ref(),
    );
    let quality = Choice::new(
        "Connection Quality",
        Quality::from_id(&config.quality).label(),
        Quality::ALL
            .iter()
            .map(|q| (q.id().to_string(), q.label().to_string()))
            .collect(),
        current.quality.as_ref(),
    );
    let multi_monitor = toggle(
        "Use All Monitors",
        config.multi_monitor,
        current.multi_monitor,
    );
    let display = adw::PreferencesGroup::builder().title("Display").build();
    display.add(&resolution_row);
    display.add(&custom_row);
    display.add(&graphics.row);
    display.add(&quality.row);
    display.add(&multi_monitor.row);

    // Regional: Time Zone & Location
    let regional = adw::PreferencesGroup::builder()
        .title("Time Zone & Location")
        .description("Default uses the value from Preferences.")
        .build();
    let timezone = Choice::new(
        "Time Zone",
        crate::session::timezone::timezone_label(&config.timezone),
        crate::session::timezone::COMMON_TIMEZONES
            .iter()
            .map(|(id, l)| (id.to_string(), l.to_string()))
            .collect(),
        current.timezone.as_ref(),
    );
    let location = Choice::new(
        "Location",
        crate::session::location::location_label(&config.location),
        crate::session::location::COMMON_LOCATIONS
            .iter()
            .map(|(id, l, _, _)| (id.to_string(), l.to_string()))
            .collect(),
        current.location.as_ref(),
    );
    regional.add(&timezone.row);
    regional.add(&location.row);

    // Devices.
    let devices = adw::PreferencesGroup::builder()
        .title("Devices")
        .description("Default uses the value from Preferences.")
        .build();
    let clipboard = toggle("Clipboard", config.enable_clipboard, current.clipboard);
    let audio = toggle("Audio Output", config.enable_audio, current.audio);
    let microphone = toggle("Microphone", config.enable_microphone, current.microphone);
    let camera = toggle("Camera", config.enable_camera, current.camera);
    let drives = toggle(
        "Shared Folders",
        config.enable_drive_redirection,
        current.drives,
    );
    let printers = toggle("Printers", config.enable_printers, current.printers);
    let smartcard = toggle("Smart Cards", config.enable_smartcard, current.smartcard);
    for choice in [
        &clipboard,
        &audio,
        &microphone,
        &camera,
        &drives,
        &printers,
        &smartcard,
    ] {
        devices.add(&choice.row);
    }

    // Commands.
    let commands = adw::PreferencesGroup::builder()
        .title("Commands")
        .description(
            "Shell commands run on this computer. The connection is aborted when the \
             command before connecting fails or runs longer than a minute. \
             WINDOWS_APP_NAME, WINDOWS_APP_ADDRESS and WINDOWS_APP_TARGET describe the \
             connection.",
        )
        .build();
    let pre_row = adw::EntryRow::builder()
        .title("Before connecting")
        .text(&current.pre_command)
        .build();
    let post_row = adw::EntryRow::builder()
        .title("After disconnecting")
        .text(&current.post_command)
        .build();
    commands.add(&pre_row);
    commands.add(&post_row);

    let error_label = Label::new(None);
    error_label.add_css_class("error");
    error_label.set_wrap(true);
    error_label.set_visible(false);
    commands.add(&error_label);

    // Tags, Folders & Favorites
    let org_group = adw::PreferencesGroup::builder()
        .title("Organization &amp; Tags")
        .build();
    let folder_row = adw::EntryRow::builder()
        .title("Folder / Category (e.g. Production, Lab)")
        .text(current.folder.as_deref().unwrap_or_default())
        .build();
    let tags_row = adw::EntryRow::builder()
        .title("Tags (comma separated)")
        .text(current.tags.join(", "))
        .build();
    let favorite_row = adw::SwitchRow::builder()
        .title("Favorite")
        .subtitle("Star this connection for quick access")
        .active(current.is_favorite)
        .build();
    org_group.add(&folder_row);
    org_group.add(&tags_row);
    org_group.add(&favorite_row);

    let page = adw::PreferencesPage::new();
    page.add(&org_group);
    page.add(&display);
    page.add(&devices);
    page.add(&regional);
    page.add(&commands);

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&page));
    dialog.set_child(Some(&toolbar_view));

    let dlg = dialog.downgrade();
    cancel_btn.connect_clicked(move |_| {
        if let Some(dialog) = dlg.upgrade() {
            dialog.close();
        }
    });

    let toggles = Rc::new([
        clipboard, audio, microphone, camera, drives, printers, smartcard,
    ]);
    let dlg = dialog.downgrade();
    save_btn.connect_clicked(move |_| {
        let selected = resolution_row.selected();
        let resolution = if selected == custom_index {
            match Resolution::parse(&custom_row.text()) {
                Some(size) => Some(size),
                None => {
                    error_label
                        .set_text("Enter the custom size as width x height, e.g. 1920x1080.");
                    error_label.set_visible(true);
                    return;
                }
            }
        } else {
            selected
                .checked_sub(1)
                .and_then(|i| sizes.get(i as usize).copied())
        };
        let [clipboard, audio, microphone, camera, drives, printers, smartcard] = &*toggles;
        let mut overrides = ConnectionOverrides {
            resolution,
            graphics_mode: graphics.value(),
            is_favorite: favorite_row.is_active(),
            tags: tags_row
                .text()
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            folder: Some(folder_row.text().to_string()).filter(|s| !s.is_empty()),
            quality: quality.value(),
            clipboard: clipboard.value(),
            audio: audio.value(),
            microphone: microphone.value(),
            camera: camera.value(),
            drives: drives.value(),
            printers: printers.value(),
            smartcard: smartcard.value(),
            multi_monitor: multi_monitor.value(),
            timezone: timezone.value(),
            location: location.value(),
            pre_command: pre_row.text().to_string(),
            post_command: post_row.text().to_string(),
        };
        if let Err(e) = overrides.normalize() {
            error_label.set_text(&e.to_string());
            error_label.set_visible(true);
            return;
        }
        if let Err(e) = store.set(&target, overrides) {
            error_label.set_text(&format!("Could not save the settings: {e}"));
            error_label.set_visible(true);
            return;
        }
        if let Some(dialog) = dlg.upgrade() {
            dialog.close();
        }
    });
    dialog.present(Some(parent));
}
