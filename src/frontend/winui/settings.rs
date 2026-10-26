//! The Settings page: sign-in profile, display, devices and shared folders, applied as
//! soon as they change, plus the About section.

use std::path::Path;

use windows_reactor::*;

use super::dialogs::hint_text;
use super::forms::GRAPHICS_MODES;
use super::win32;
use crate::config::{AppConfig, AuthProfile, SharedFolder};
use crate::frontend::common::text::{APP_NAME, COPYRIGHT, REPOSITORY_URL};
use crate::session::{GraphicsMode, Quality};

#[derive(Clone, PartialEq)]
pub struct SettingsPageInput {
    pub about: Callback<()>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Switch {
    MultiMonitor,
    Clipboard,
    Audio,
    Microphone,
    Camera,
    SmartCard,
    Printers,
    ShareFolder,
    RemovableDrives,
}

impl Switch {
    fn slot(self, config: &mut AppConfig) -> &mut bool {
        match self {
            Self::MultiMonitor => &mut config.multi_monitor,
            Self::Clipboard => &mut config.enable_clipboard,
            Self::Audio => &mut config.enable_audio,
            Self::Microphone => &mut config.enable_microphone,
            Self::Camera => &mut config.enable_camera,
            Self::SmartCard => &mut config.enable_smartcard,
            Self::Printers => &mut config.enable_printers,
            Self::ShareFolder => &mut config.enable_drive_redirection,
            Self::RemovableDrives => &mut config.hotplug_drives,
        }
    }
}

pub enum SettingsPageMsg {
    Profile(Option<usize>),
    Graphics(Option<usize>),
    Quality(Option<usize>),
    Switch(Switch, bool),
    BrowseSharedFolder,
    SharedFolderChosen(std::path::PathBuf),
    AddFolder,
    FolderAdded(std::path::PathBuf),
    RemoveFolder(usize),
    DismissError,
}

pub struct SettingsPage {
    config: AppConfig,
    error: Option<String>,
}

impl SettingsPage {
    fn save(&mut self) {
        self.error = self
            .config
            .save()
            .err()
            .map(|e| format!("Could not save the settings: {e}"));
        if let Some(error) = &self.error {
            log::warn!("{error}");
        }
    }

    /// Shares `path` under a drive name derived from the folder name.
    fn add_folder(&mut self, path: &Path) -> Result<(), String> {
        let path_text = path.to_string_lossy().into_owned();
        let folders = &self.config.extra_folders;
        if folders.iter().any(|f| f.path == path_text) {
            return Ok(());
        }
        let base = drive_name(path);
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
        let folder = SharedFolder {
            name,
            path: path_text,
        };
        if !folder.is_valid() {
            return Err(format!(
                "Cannot share {}: commas are not supported in paths.",
                folder.path
            ));
        }
        self.config.extra_folders.push(folder);
        Ok(())
    }
}

/// Drive name for a shared folder: its name with unsupported characters replaced.
fn drive_name(path: &Path) -> String {
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

/// A settings card: title and description on the left, the control on the right.
fn setting_card(title: &str, description: &str, control: impl Into<View>) -> View {
    let text = if description.is_empty() {
        StackPanel::new()
            .vertical_alignment(VerticalAlignment::Center)
            .children((TextBlock::new().text(title),))
    } else {
        StackPanel::new()
            .vertical_alignment(VerticalAlignment::Center)
            .spacing(2.0)
            .children((
                TextBlock::new().text(title),
                TextBlock::new()
                    .opacity(0.7)
                    .text(description)
                    .font_size(12.0)
                    .text_wrapping(TextWrapping::Wrap),
            ))
    };
    Border::new()
        .padding(Thickness::xy(16.0, 12.0))
        .corner_radius(4.0)
        .background(ThemeBrush::CardBackground)
        .border_brush(ThemeBrush::CardStroke)
        .border_thickness(1.0)
        .content(
            Grid::new()
                .columns([GridLength::Star(1.0), GridLength::Auto])
                .column_spacing(16.0)
                .children((
                    text,
                    Border::new()
                        .grid_column(1)
                        .vertical_alignment(VerticalAlignment::Center)
                        .content(control),
                )),
        )
}

fn group_title(text: &str) -> View {
    TextBlock::new()
        .text(text)
        .font_weight(FontWeight::SEMI_BOLD)
        .margin(Thickness::new(0.0, 16.0, 0.0, 4.0))
        .into()
}

fn combo(options: Vec<String>, selected: usize, on_change: Callback<Option<usize>>) -> View {
    ComboBox::new()
        .min_width(240.0)
        .items_source(options)
        .selected_index(selected)
        .on_selection_changed(on_change)
        .into()
}

impl Component for SettingsPage {
    type Input = SettingsPageInput;
    type Message = SettingsPageMsg;

    fn create(_input: &Self::Input, _ctx: &ComponentContext<Self>) -> Self {
        Self {
            config: AppConfig::load(),
            error: None,
        }
    }

    fn update(&mut self, msg: Self::Message, ctx: &ComponentContext<Self>) {
        match msg {
            SettingsPageMsg::Profile(index) => {
                let Some(index) = index.and_then(|i| u32::try_from(i).ok()) else {
                    return;
                };
                self.config.auth_profile_index = AuthProfile::from_index(index).index();
                self.save();
            }
            SettingsPageMsg::Graphics(index) => {
                let mode = index
                    .and_then(|i| GRAPHICS_MODES.get(i))
                    .map_or(GraphicsMode::Auto, |(m, _)| *m);
                self.config.graphics_mode = mode.id().to_string();
                self.save();
            }
            SettingsPageMsg::Quality(index) => {
                let quality = index
                    .and_then(|i| Quality::ALL.get(i).copied())
                    .unwrap_or_default();
                self.config.quality = quality.id().to_string();
                self.save();
            }
            SettingsPageMsg::Switch(switch, on) => {
                *switch.slot(&mut self.config) = on;
                self.save();
            }
            SettingsPageMsg::BrowseSharedFolder => {
                let sender = ctx.sender();
                win32::defer(move || {
                    if let Some(path) =
                        win32::pick_folder(win32::main_window(), "Choose the Folder to Share")
                    {
                        sender.send(SettingsPageMsg::SharedFolderChosen(path));
                    }
                });
            }
            SettingsPageMsg::SharedFolderChosen(path) => {
                self.config.shared_folder_path = path.to_string_lossy().into_owned();
                self.save();
            }
            SettingsPageMsg::AddFolder => {
                let sender = ctx.sender();
                win32::defer(move || {
                    if let Some(path) =
                        win32::pick_folder(win32::main_window(), "Choose a Folder to Share")
                    {
                        sender.send(SettingsPageMsg::FolderAdded(path));
                    }
                });
            }
            SettingsPageMsg::FolderAdded(path) => match self.add_folder(&path) {
                Ok(()) => self.save(),
                Err(e) => {
                    log::warn!("{e}");
                    self.error = Some(e);
                }
            },
            SettingsPageMsg::RemoveFolder(index) => {
                if index < self.config.extra_folders.len() {
                    self.config.extra_folders.remove(index);
                    self.save();
                }
            }
            SettingsPageMsg::DismissError => self.error = None,
        }
    }

    fn view(&self, input: &Self::Input, ctx: &mut ViewContext<Self>) -> View {
        let config = &self.config;
        let switch = |ctx: &mut ViewContext<Self>, which: Switch, on: bool| -> View {
            ToggleSwitch::new()
                .is_on(on)
                .on_toggled(ctx.callback(move |on| SettingsPageMsg::Switch(which, on)))
                .into()
        };
        let profile = AuthProfile::from_index(config.auth_profile_index).index() as usize;
        let graphics = GraphicsMode::from_id(&config.graphics_mode);
        let graphics = GRAPHICS_MODES
            .iter()
            .position(|(m, _)| *m == graphics)
            .unwrap_or(0);
        let quality = Quality::from_id(&config.quality);
        let quality = Quality::ALL.iter().position(|q| *q == quality).unwrap_or(0);
        let shared = if config.shared_folder_path.is_empty() {
            "No folder selected".to_string()
        } else {
            config.shared_folder_path.clone()
        };

        let mut rows: Vec<(String, View)> = vec![
            (
                "title".into(),
                TextBlock::new()
                    .text("Settings")
                    .font_size(28.0)
                    .font_weight(FontWeight::SEMI_BOLD)
                    .into(),
            ),
            (
                "error".into(),
                InfoBar::new()
                    .title("Settings")
                    .message(self.error.clone().unwrap_or_default())
                    .severity(InfoBarSeverity::Error)
                    .is_open(self.error.is_some())
                    .on_closed(ctx.callback(|()| SettingsPageMsg::DismissError))
                    .into(),
            ),
            ("identity".into(), group_title("Identity & Authentication")),
            (
                "profile".into(),
                setting_card(
                    "Sign-in profile",
                    "The platform identity used during Microsoft sign-in. Applies the next \
                     time Windows App starts.",
                    combo(
                        AuthProfile::ALL
                            .iter()
                            .map(|p| p.label().to_string())
                            .collect(),
                        profile,
                        ctx.callback(SettingsPageMsg::Profile),
                    ),
                ),
            ),
            ("display".into(), group_title("Display")),
            (
                "monitors".into(),
                setting_card(
                    "Use all monitors",
                    "Span the session across every monitor in full screen.",
                    switch(ctx, Switch::MultiMonitor, config.multi_monitor),
                ),
            ),
            (
                "graphics".into(),
                setting_card(
                    "Graphics",
                    "How the remote desktop is encoded.",
                    combo(
                        GRAPHICS_MODES
                            .iter()
                            .map(|(_, l)| (*l).to_string())
                            .collect(),
                        graphics,
                        ctx.callback(SettingsPageMsg::Graphics),
                    ),
                ),
            ),
            (
                "quality".into(),
                setting_card(
                    "Connection quality",
                    "Trades visual effects and colour depth for bandwidth.",
                    combo(
                        Quality::ALL.iter().map(|q| q.label().to_string()).collect(),
                        quality,
                        ctx.callback(SettingsPageMsg::Quality),
                    ),
                ),
            ),
            ("devices".into(), group_title("Devices")),
            (
                "clipboard".into(),
                setting_card(
                    "Clipboard sharing",
                    "Copy and paste text between this PC and the session.",
                    switch(ctx, Switch::Clipboard, config.enable_clipboard),
                ),
            ),
            (
                "audio".into(),
                setting_card(
                    "Audio output redirection",
                    "Play the session's sound on this PC.",
                    switch(ctx, Switch::Audio, config.enable_audio),
                ),
            ),
            (
                "microphone".into(),
                setting_card(
                    "Microphone pass-through",
                    "",
                    switch(ctx, Switch::Microphone, config.enable_microphone),
                ),
            ),
            (
                "camera".into(),
                setting_card(
                    "Camera / webcam pass-through",
                    "",
                    switch(ctx, Switch::Camera, config.enable_camera),
                ),
            ),
            (
                "smartcard".into(),
                setting_card(
                    "Smart cards",
                    "",
                    switch(ctx, Switch::SmartCard, config.enable_smartcard),
                ),
            ),
            (
                "printers".into(),
                setting_card(
                    "Printers",
                    "",
                    switch(ctx, Switch::Printers, config.enable_printers),
                ),
            ),
            ("folders".into(), group_title("Folder Sharing")),
            (
                "share".into(),
                setting_card(
                    "Share a local folder with the Cloud PC",
                    "",
                    switch(ctx, Switch::ShareFolder, config.enable_drive_redirection),
                ),
            ),
            (
                "shared-folder".into(),
                setting_card(
                    "Shared folder",
                    &shared,
                    Button::new()
                        .on_click(ctx.callback(|()| SettingsPageMsg::BrowseSharedFolder))
                        .content("Browse…"),
                ),
            ),
            (
                "removable".into(),
                setting_card(
                    "Removable drives",
                    "Share drives plugged in during the session.",
                    switch(ctx, Switch::RemovableDrives, config.hotplug_drives),
                ),
            ),
            (
                "more-folders".into(),
                setting_card(
                    "More shared folders",
                    "Each folder appears as its own drive in the session.",
                    Button::new()
                        .on_click(ctx.callback(|()| SettingsPageMsg::AddFolder))
                        .content("Add Folder…"),
                ),
            ),
        ];
        for (index, folder) in config.extra_folders.iter().enumerate() {
            rows.push((
                format!("folder-{}", folder.path),
                setting_card(
                    &folder.name,
                    &folder.path,
                    Button::new()
                        .on_click(ctx.callback(move |()| SettingsPageMsg::RemoveFolder(index)))
                        .content("Stop Sharing"),
                ),
            ));
        }
        rows.extend([
            ("about-title".into(), group_title("About")),
            (
                "about".into(),
                setting_card(
                    APP_NAME,
                    &format!("Version {} · {COPYRIGHT}", env!("CARGO_PKG_VERSION")),
                    StackPanel::new()
                        .orientation(Orientation::Horizontal)
                        .spacing(8.0)
                        .children((
                            HyperlinkButton::new()
                                .on_click(|| win32::open_uri(REPOSITORY_URL))
                                .content("Website"),
                            Button::new()
                                .on_click(input.about.clone())
                                .content("About Windows App"),
                        )),
                ),
            ),
            (
                "usb-hint".into(),
                hint_text(
                    "USB device redirection and desktop integration are available in the \
                     Linux version only.",
                    false,
                )
                .into(),
            ),
        ]);

        ScrollViewer::new().content(
            StackPanel::new()
                .spacing(4.0)
                .max_width(1000.0)
                .margin(Thickness::new(32.0, 24.0, 32.0, 32.0))
                .keyed_children(rows),
        )
    }
}
