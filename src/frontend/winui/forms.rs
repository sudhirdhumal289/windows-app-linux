//! Form dialogs: the sign-in prompt of a direct connection, the Remote PC editor and the
//! settings of one connection. Each validates while the user types and enables its
//! primary button only when the values can be saved.

use windows_reactor::*;

use super::dialogs::{body_text, hint_text, shell, DialogMsg, Outcome};
use crate::config::AppConfig;
use crate::connections::{
    AddressError, AuthMethod, CertificatePolicy, Gateway, RemoteApp, SavedConnection,
    SecurityProtocol,
};
use crate::overrides::{ConnectionOverrides, Resolution};
use crate::session::{Credentials, GraphicsMode, Quality};

/// Graphics modes offered in the settings, with their labels.
pub const GRAPHICS_MODES: [(GraphicsMode, &str); 2] = [
    (GraphicsMode::Auto, "Automatic (H.264 / AVC444)"),
    (GraphicsMode::RemoteFx, "RemoteFX (low latency LAN)"),
];

fn cred_cb<E: 'static>(
    ctx: &ViewContext<crate::frontend::winui::app::MainApp>,
    f: impl Fn(E) -> CredentialsMsg + 'static,
) -> Callback<E> {
    ctx.callback(move |e| crate::frontend::winui::app::Msg::Dialog(DialogMsg::Credentials(f(e))))
}

fn remote_cb<E: 'static>(
    ctx: &ViewContext<crate::frontend::winui::app::MainApp>,
    f: impl Fn(E) -> RemotePcMsg + 'static,
) -> Callback<E> {
    ctx.callback(move |e| crate::frontend::winui::app::Msg::Dialog(DialogMsg::RemotePc(f(e))))
}

fn settings_cb<E: 'static>(
    ctx: &ViewContext<crate::frontend::winui::app::MainApp>,
    f: impl Fn(E) -> SettingsMsg + 'static,
) -> Callback<E> {
    ctx.callback(move |e| crate::frontend::winui::app::Msg::Dialog(DialogMsg::Settings(f(e))))
}

fn section_title(text: &str) -> TextBlock {
    TextBlock::new()
        .text(text)
        .font_weight(FontWeight::SEMI_BOLD)
        .margin(Thickness::new(0.0, 8.0, 0.0, 0.0))
}

fn text_field(label: &str, value: &str, on_change: Callback<String>, enabled: bool) -> View {
    TextBox::new()
        .text(value)
        .is_enabled(enabled)
        .on_text_changed(on_change)
        .slot(TextBoxSlot::Header, label)
}

fn password_field(label: &str, value: &str, on_change: Callback<String>) -> View {
    PasswordBox::new()
        .password(value)
        .on_password_changed(on_change)
        .slot(PasswordBoxSlot::Header, label)
}

fn combo(
    label: &str,
    options: Vec<String>,
    selected: usize,
    on_change: Callback<Option<usize>>,
) -> View {
    ComboBox::new()
        .items_source(options)
        .selected_index(selected)
        .horizontal_alignment(HorizontalAlignment::Stretch)
        .on_selection_changed(on_change)
        .slot(ComboBoxSlot::Header, label)
}

fn check(label: &str, checked: bool, enabled: bool, on_change: Callback<bool>) -> View {
    CheckBox::new()
        .is_checked(checked)
        .is_enabled(enabled)
        .on_is_checked_changed(on_change)
        .content(label)
}

fn index_of<T: PartialEq>(all: &[T], value: &T) -> usize {
    all.iter().position(|v| v == value).unwrap_or(0)
}

fn pick<T: Copy + Default>(all: &[T], index: usize) -> T {
    all.get(index).copied().unwrap_or_default()
}

// ----- Credentials -----

#[derive(Clone)]
pub enum CredentialsMsg {
    Username(String),
    Password(String),
    Gateway(String),
    Remember(bool),
    Closed(ContentDialogResult),
}

pub struct CredentialsDialog {
    pub connection: SavedConnection,
    pub can_remember: bool,
    pub username: String,
    pub password: String,
    pub gateway: String,
    pub remember: bool,
    pub closed: bool,
}

impl CredentialsDialog {
    pub fn new(connection: SavedConnection, can_remember: bool, known: Credentials) -> Self {
        Self {
            username: connection.username.clone(),
            password: known.password.clone().unwrap_or_default(),
            gateway: known.gateway_password.clone().unwrap_or_default(),
            remember: connection.remember_password,
            connection,
            can_remember,
            closed: false,
        }
    }

    pub fn update(&mut self, msg: CredentialsMsg) -> Option<Outcome> {
        match msg {
            CredentialsMsg::Username(text) => {
                self.username = text;
                None
            }
            CredentialsMsg::Password(text) => {
                self.password = text;
                None
            }
            CredentialsMsg::Gateway(text) => {
                self.gateway = text;
                None
            }
            CredentialsMsg::Remember(on) => {
                self.remember = on;
                None
            }
            CredentialsMsg::Closed(result) => {
                if self.closed {
                    return None;
                }
                self.closed = true;
                if result != ContentDialogResult::Primary {
                    return Some(Outcome::Cancelled);
                }
                let mut connection = self.connection.clone();
                connection.username = self.username.trim().to_string();
                connection.remember_password = self.can_remember && self.remember;
                let non_empty = |text: &str| Some(text.to_string()).filter(|t| !t.is_empty());
                let credentials = Credentials {
                    password: non_empty(&self.password),
                    gateway_password: non_empty(&self.gateway)
                        .filter(|_| connection.gateway_needs_password()),
                };
                self.password.clear();
                self.gateway.clear();
                Some(Outcome::Credentials {
                    connection,
                    credentials,
                })
            }
        }
    }

    pub fn view(&self, ctx: &ViewContext<crate::frontend::winui::app::MainApp>) -> View {
        let connection = &self.connection;
        let body = if self.can_remember {
            format!("Sign in to {}.", connection.address)
        } else {
            format!(
                "Sign in to {}. The password is used for this session only.",
                connection.address
            )
        };
        let mut fields: Vec<(&'static str, View)> = vec![
            ("body", body_text(&body).into()),
            (
                "username",
                text_field(
                    "Username",
                    &self.username,
                    cred_cb(ctx, CredentialsMsg::Username),
                    true,
                ),
            ),
            (
                "password",
                password_field(
                    "Password",
                    &self.password,
                    cred_cb(ctx, CredentialsMsg::Password),
                ),
            ),
        ];
        if connection.gateway_needs_password() {
            fields.push((
                "gateway",
                password_field(
                    "Gateway password",
                    &self.gateway,
                    cred_cb(ctx, CredentialsMsg::Gateway),
                ),
            ));
        }
        if self.can_remember {
            fields.push((
                "remember",
                check(
                    "Remember password",
                    self.remember,
                    true,
                    cred_cb(ctx, CredentialsMsg::Remember),
                ),
            ));
        }
        shell(
            &format!("Connect to {}", connection.display_name()),
            Some("Connect"),
            true,
            "Cancel",
            !self.closed,
            cred_cb(ctx, CredentialsMsg::Closed),
            StackPanel::new()
                .spacing(12.0)
                .min_width(360.0)
                .keyed_children(fields),
        )
    }
}

// ----- Remote PC -----

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Address,
    Name,
    Username,
    Domain,
    Group,
    GatewayAddress,
    GatewayUser,
    Program,
    Arguments,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Flag {
    Remember,
    UseGateway,
    SeparateGateway,
    UseRemoteApp,
    Admin,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    Security,
    Certificate,
    Auth,
}

#[derive(Clone)]
pub enum RemotePcMsg {
    Text(Field, String),
    Flag(Flag, bool),
    Pick(Pick, Option<usize>),
    Closed(ContentDialogResult),
}

pub struct RemotePcDialog {
    base: SavedConnection,
    editing: bool,
    address: String,
    name: String,
    username: String,
    domain: String,
    group: String,
    remember: bool,
    use_gateway: bool,
    gateway_address: String,
    separate_gateway: bool,
    gateway_user: String,
    use_remote_app: bool,
    program: String,
    arguments: String,
    admin: bool,
    security: usize,
    certificate: usize,
    auth: usize,
    closed: bool,
}

impl RemotePcDialog {
    pub fn new(existing: Option<SavedConnection>) -> Self {
        let base = existing.clone().unwrap_or_else(SavedConnection::blank);
        let gateway = base.gateway.clone().unwrap_or_default();
        let app = base.remote_app.clone().unwrap_or_default();
        Self {
            editing: existing.is_some(),
            address: base.address.clone(),
            name: base.name.clone(),
            username: base.username.clone(),
            domain: base.domain.clone(),
            group: base.group.clone(),
            remember: base.remember_password,
            use_gateway: base.gateway.is_some(),
            gateway_address: gateway.address,
            separate_gateway: gateway.separate_credentials,
            gateway_user: gateway.username,
            use_remote_app: base.remote_app.is_some(),
            program: app.program,
            arguments: app.arguments,
            admin: base.admin_session,
            security: index_of(&SecurityProtocol::ALL, &base.security),
            certificate: index_of(&CertificatePolicy::ALL, &base.certificate),
            auth: index_of(&AuthMethod::ALL, &base.auth),
            base,
            closed: false,
        }
    }

    fn collect(&self) -> Result<SavedConnection, AddressError> {
        let mut connection = self.base.clone();
        connection.update(&self.name, &self.address, &self.username)?;
        connection.domain = self.domain.clone();
        connection.group = self.group.clone();
        connection.remember_password = self.remember;
        connection.gateway = self.use_gateway.then(|| Gateway {
            address: self.gateway_address.clone(),
            username: self.gateway_user.clone(),
            separate_credentials: self.separate_gateway,
        });
        connection.remote_app = self.use_remote_app.then(|| RemoteApp {
            program: self.program.clone(),
            arguments: self.arguments.clone(),
        });
        connection.admin_session = self.admin;
        connection.security = pick(&SecurityProtocol::ALL, self.security);
        connection.certificate = pick(&CertificatePolicy::ALL, self.certificate);
        connection.auth = pick(&AuthMethod::ALL, self.auth);
        connection.validate()?;
        Ok(connection)
    }

    fn field(&self, field: Field) -> &str {
        match field {
            Field::Address => &self.address,
            Field::Name => &self.name,
            Field::Username => &self.username,
            Field::Domain => &self.domain,
            Field::Group => &self.group,
            Field::GatewayAddress => &self.gateway_address,
            Field::GatewayUser => &self.gateway_user,
            Field::Program => &self.program,
            Field::Arguments => &self.arguments,
        }
    }

    fn field_mut(&mut self, field: Field) -> &mut String {
        match field {
            Field::Address => &mut self.address,
            Field::Name => &mut self.name,
            Field::Username => &mut self.username,
            Field::Domain => &mut self.domain,
            Field::Group => &mut self.group,
            Field::GatewayAddress => &mut self.gateway_address,
            Field::GatewayUser => &mut self.gateway_user,
            Field::Program => &mut self.program,
            Field::Arguments => &mut self.arguments,
        }
    }

    fn text(
        &self,
        ctx: &ViewContext<crate::frontend::winui::app::MainApp>,
        field: Field,
        label: &str,
        enabled: bool,
    ) -> View {
        text_field(
            label,
            self.field(field),
            remote_cb(ctx, move |text| RemotePcMsg::Text(field, text)),
            enabled,
        )
    }

    fn flag(
        &self,
        ctx: &ViewContext<crate::frontend::winui::app::MainApp>,
        flag: Flag,
        label: &str,
        checked: bool,
    ) -> View {
        check(
            label,
            checked,
            true,
            remote_cb(ctx, move |on| RemotePcMsg::Flag(flag, on)),
        )
    }

    fn pick_view<T: Copy>(
        ctx: &ViewContext<crate::frontend::winui::app::MainApp>,
        which: Pick,
        label: &str,
        all: &[T],
        name: fn(T) -> &'static str,
        selected: usize,
    ) -> View {
        combo(
            label,
            all.iter().map(|v| name(*v).to_string()).collect(),
            selected,
            remote_cb(ctx, move |index| RemotePcMsg::Pick(which, index)),
        )
    }

    pub fn update(&mut self, msg: RemotePcMsg) -> Option<Outcome> {
        match msg {
            RemotePcMsg::Text(field, text) => {
                *self.field_mut(field) = text;
                None
            }
            RemotePcMsg::Flag(flag, on) => {
                match flag {
                    Flag::Remember => self.remember = on,
                    Flag::UseGateway => self.use_gateway = on,
                    Flag::SeparateGateway => self.separate_gateway = on,
                    Flag::UseRemoteApp => self.use_remote_app = on,
                    Flag::Admin => self.admin = on,
                }
                None
            }
            RemotePcMsg::Pick(which, index) => {
                let index = index.unwrap_or(0);
                match which {
                    Pick::Security => self.security = index,
                    Pick::Certificate => self.certificate = index,
                    Pick::Auth => self.auth = index,
                }
                None
            }
            RemotePcMsg::Closed(result) => {
                if self.closed {
                    return None;
                }
                self.closed = true;
                let outcome = match (result, self.collect()) {
                    (ContentDialogResult::Primary, Ok(connection)) => {
                        Outcome::Connection(connection)
                    }
                    _ => Outcome::Cancelled,
                };
                Some(outcome)
            }
        }
    }

    pub fn view(&self, ctx: &ViewContext<crate::frontend::winui::app::MainApp>) -> View {
        let result = self.collect();
        // Nothing typed yet is not an error worth showing.
        let error = result
            .as_ref()
            .err()
            .filter(|e| **e != AddressError::Empty)
            .map(ToString::to_string);

        let mut rows: Vec<(&'static str, View)> =
            vec![
            ("address", self.text(ctx, Field::Address, "PC name or IP address", true)),
            ("name", self.text(ctx, Field::Name, "Display name (optional)", true)),
            ("username", self.text(ctx, Field::Username, "Username (optional)", true)),
            (
                "hint",
                hint_text(
                    "Examples: office-pc, pc01.contoso.com:3390, 192.168.1.20. Usernames can be \
                     written as DOMAIN\\user or user@domain.",
                    false,
                )
                .into(),
            ),
        ];
        if let Some(error) = &error {
            rows.push(("error", hint_text(error, true).into()));
        }
        rows.extend([
            ("sign-in", section_title("Sign-in").into()),
            (
                "domain",
                self.text(ctx, Field::Domain, "Domain (optional)", true),
            ),
            (
                "group",
                self.text(ctx, Field::Group, "Group (optional)", true),
            ),
            (
                "remember",
                self.flag(ctx, Flag::Remember, "Remember password", self.remember),
            ),
            ("session", section_title("Session").into()),
            (
                "use-gateway",
                self.flag(
                    ctx,
                    Flag::UseGateway,
                    "Connect through a Remote Desktop Gateway",
                    self.use_gateway,
                ),
            ),
        ]);
        if self.use_gateway {
            rows.extend([
                (
                    "gateway-address",
                    self.text(
                        ctx,
                        Field::GatewayAddress,
                        "Gateway address (port 443 if omitted)",
                        true,
                    ),
                ),
                (
                    "gateway-separate",
                    self.flag(
                        ctx,
                        Flag::SeparateGateway,
                        "Separate gateway credentials",
                        self.separate_gateway,
                    ),
                ),
                (
                    "gateway-user",
                    self.text(
                        ctx,
                        Field::GatewayUser,
                        "Gateway username",
                        self.separate_gateway,
                    ),
                ),
            ]);
        }
        rows.push((
            "use-app",
            self.flag(
                ctx,
                Flag::UseRemoteApp,
                "Start a single program (RemoteApp)",
                self.use_remote_app,
            ),
        ));
        if self.use_remote_app {
            rows.extend([
                (
                    "program",
                    self.text(ctx, Field::Program, "Program (path or ||alias)", true),
                ),
                (
                    "arguments",
                    self.text(ctx, Field::Arguments, "Arguments (optional)", true),
                ),
            ]);
        }
        rows.extend([
            (
                "admin",
                self.flag(
                    ctx,
                    Flag::Admin,
                    "Administrative session (console)",
                    self.admin,
                ),
            ),
            ("security-title", section_title("Security").into()),
            (
                "security",
                Self::pick_view(
                    ctx,
                    Pick::Security,
                    "Security",
                    &SecurityProtocol::ALL,
                    SecurityProtocol::label,
                    self.security,
                ),
            ),
            (
                "certificate",
                Self::pick_view(
                    ctx,
                    Pick::Certificate,
                    "Certificates",
                    &CertificatePolicy::ALL,
                    CertificatePolicy::label,
                    self.certificate,
                ),
            ),
            (
                "auth",
                Self::pick_view(
                    ctx,
                    Pick::Auth,
                    "Authentication",
                    &AuthMethod::ALL,
                    AuthMethod::label,
                    self.auth,
                ),
            ),
            (
                "auth-hint",
                hint_text("Kerberos needs a ticket for the domain.", false).into(),
            ),
        ]);

        shell(
            if self.editing {
                "Edit Remote PC"
            } else {
                "Add Remote PC"
            },
            Some(if self.editing { "Save" } else { "Add" }),
            result.is_ok(),
            "Cancel",
            !self.closed,
            remote_cb(ctx, RemotePcMsg::Closed),
            ScrollViewer::new().content(
                StackPanel::new()
                    .spacing(10.0)
                    .min_width(400.0)
                    .margin(Thickness::new(0.0, 0.0, 16.0, 0.0))
                    .keyed_children(rows),
            ),
        )
    }
}

// ----- Connection settings -----

/// On/off settings that can follow the preferences.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    MultiMonitor,
    Clipboard,
    Audio,
    Microphone,
    Camera,
    Drives,
    Printers,
    SmartCard,
}

impl Toggle {
    const DEVICES: [(Self, &'static str); 7] = [
        (Self::Clipboard, "Clipboard"),
        (Self::Audio, "Audio output"),
        (Self::Microphone, "Microphone"),
        (Self::Camera, "Camera"),
        (Self::Drives, "Shared folders"),
        (Self::Printers, "Printers"),
        (Self::SmartCard, "Smart cards"),
    ];

    fn default_value(self, config: &AppConfig) -> bool {
        match self {
            Self::MultiMonitor => config.multi_monitor,
            Self::Clipboard => config.enable_clipboard,
            Self::Audio => config.enable_audio,
            Self::Microphone => config.enable_microphone,
            Self::Camera => config.enable_camera,
            Self::Drives => config.enable_drive_redirection,
            Self::Printers => config.enable_printers,
            Self::SmartCard => config.enable_smartcard,
        }
    }

    fn slot(self, overrides: &mut ConnectionOverrides) -> &mut Option<bool> {
        match self {
            Self::MultiMonitor => &mut overrides.multi_monitor,
            Self::Clipboard => &mut overrides.clipboard,
            Self::Audio => &mut overrides.audio,
            Self::Microphone => &mut overrides.microphone,
            Self::Camera => &mut overrides.camera,
            Self::Drives => &mut overrides.drives,
            Self::Printers => &mut overrides.printers,
            Self::SmartCard => &mut overrides.smartcard,
        }
    }
}

fn on_off(value: bool) -> &'static str {
    if value {
        "On"
    } else {
        "Off"
    }
}

/// Combo index of an optional on/off value: 0 default, 1 on, 2 off.
fn tri_index(value: Option<bool>) -> usize {
    match value {
        None => 0,
        Some(true) => 1,
        Some(false) => 2,
    }
}

fn tri_value(index: usize) -> Option<bool> {
    match index {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

#[derive(Clone)]
pub enum SettingsMsg {
    Resolution(Option<usize>),
    CustomSize(String),
    Graphics(Option<usize>),
    Quality(Option<usize>),
    Toggle(Toggle, Option<usize>),
    PreCommand(String),
    PostCommand(String),
    Closed(ContentDialogResult),
}

pub struct SettingsDialog {
    name: String,
    config: AppConfig,
    sizes: Vec<Resolution>,
    overrides: ConnectionOverrides,
    /// 0 fits the window, then `sizes`, then custom.
    resolution: usize,
    custom: String,
    closed: bool,
}

impl SettingsDialog {
    pub fn new(name: String, current: ConnectionOverrides) -> Self {
        let mut sizes: Vec<Resolution> = Resolution::PRESETS.to_vec();
        if let Some(size) = current.resolution.filter(|s| !sizes.contains(s)) {
            sizes.push(size);
        }
        let resolution = current
            .resolution
            .and_then(|r| sizes.iter().position(|s| *s == r))
            .map_or(0, |i| i + 1);
        Self {
            name,
            config: AppConfig::load(),
            sizes,
            overrides: current,
            resolution,
            custom: String::new(),
            closed: false,
        }
    }

    fn custom_index(&self) -> usize {
        self.sizes.len() + 1
    }

    fn collect(&self) -> Result<ConnectionOverrides, String> {
        let mut overrides = self.overrides.clone();
        overrides.resolution = if self.resolution == self.custom_index() {
            Some(Resolution::parse(&self.custom).ok_or_else(|| {
                "Enter the custom size as width x height, e.g. 1920x1080.".to_string()
            })?)
        } else {
            self.resolution
                .checked_sub(1)
                .and_then(|i| self.sizes.get(i).copied())
        };
        overrides.normalize().map_err(|e| e.to_string())?;
        Ok(overrides)
    }

    fn graphics_options(&self) -> Vec<String> {
        let default = GraphicsMode::from_id(&self.config.graphics_mode);
        let default_label = GRAPHICS_MODES
            .iter()
            .find(|(m, _)| *m == default)
            .map_or("Automatic", |(_, l)| l);
        std::iter::once(format!("Default ({default_label})"))
            .chain(GRAPHICS_MODES.iter().map(|(_, l)| (*l).to_string()))
            .collect()
    }

    fn quality_options(&self) -> Vec<String> {
        let default = Quality::from_id(&self.config.quality).label();
        std::iter::once(format!("Default ({default})"))
            .chain(Quality::ALL.iter().map(|q| q.label().to_string()))
            .collect()
    }

    fn toggle_view(
        &self,
        ctx: &ViewContext<crate::frontend::winui::app::MainApp>,
        toggle: Toggle,
        label: &str,
    ) -> View {
        let mut overrides = self.overrides.clone();
        let current = *toggle.slot(&mut overrides);
        combo(
            label,
            vec![
                format!("Default ({})", on_off(toggle.default_value(&self.config))),
                "On".to_string(),
                "Off".to_string(),
            ],
            tri_index(current),
            settings_cb(ctx, move |index| SettingsMsg::Toggle(toggle, index)),
        )
    }

    pub fn update(&mut self, msg: SettingsMsg) -> Option<Outcome> {
        match msg {
            SettingsMsg::Resolution(index) => {
                self.resolution = index.unwrap_or(0);
                None
            }
            SettingsMsg::CustomSize(text) => {
                self.custom = text;
                None
            }
            SettingsMsg::Graphics(index) => {
                self.overrides.graphics_mode = index
                    .and_then(|i| i.checked_sub(1))
                    .and_then(|i| GRAPHICS_MODES.get(i))
                    .map(|(m, _)| m.id().to_string());
                None
            }
            SettingsMsg::Quality(index) => {
                self.overrides.quality = index
                    .and_then(|i| i.checked_sub(1))
                    .and_then(|i| Quality::ALL.get(i))
                    .map(|q| q.id().to_string());
                None
            }
            SettingsMsg::Toggle(toggle, index) => {
                *toggle.slot(&mut self.overrides) = tri_value(index.unwrap_or(0));
                None
            }
            SettingsMsg::PreCommand(text) => {
                self.overrides.pre_command = text;
                None
            }
            SettingsMsg::PostCommand(text) => {
                self.overrides.post_command = text;
                None
            }
            SettingsMsg::Closed(result) => {
                if self.closed {
                    return None;
                }
                self.closed = true;
                let outcome = match (result, self.collect()) {
                    (ContentDialogResult::Primary, Ok(overrides)) => Outcome::Overrides(overrides),
                    _ => Outcome::Cancelled,
                };
                Some(outcome)
            }
        }
    }

    pub fn view(&self, ctx: &ViewContext<crate::frontend::winui::app::MainApp>) -> View {
        let result = self.collect();
        let mut size_labels = vec!["Fit window".to_string()];
        size_labels.extend(self.sizes.iter().map(ToString::to_string));
        size_labels.push("Custom…".to_string());
        let graphics = self
            .overrides
            .graphics_mode
            .as_deref()
            .and_then(|id| GRAPHICS_MODES.iter().position(|(m, _)| m.id() == id))
            .map_or(0, |i| i + 1);
        let quality = self
            .overrides
            .quality
            .as_deref()
            .and_then(|id| Quality::ALL.iter().position(|q| q.id() == id))
            .map_or(0, |i| i + 1);

        let mut rows: Vec<(&'static str, View)> = vec![
            ("display", section_title("Display").into()),
            (
                "resolution",
                combo(
                    "Resolution (a fixed size is scaled to the window)",
                    size_labels,
                    self.resolution,
                    settings_cb(ctx, SettingsMsg::Resolution),
                ),
            ),
        ];
        if self.resolution == self.custom_index() {
            rows.push((
                "custom",
                text_field(
                    "Custom size, e.g. 1920x1080",
                    &self.custom,
                    settings_cb(ctx, SettingsMsg::CustomSize),
                    true,
                ),
            ));
        }
        rows.extend([
            (
                "graphics",
                combo(
                    "Graphics",
                    self.graphics_options(),
                    graphics,
                    settings_cb(ctx, SettingsMsg::Graphics),
                ),
            ),
            (
                "quality",
                combo(
                    "Connection quality",
                    self.quality_options(),
                    quality,
                    settings_cb(ctx, SettingsMsg::Quality),
                ),
            ),
            (
                "multi-monitor",
                self.toggle_view(ctx, Toggle::MultiMonitor, "Use all monitors"),
            ),
            ("devices", section_title("Devices").into()),
            (
                "devices-hint",
                hint_text("Default uses the value from Settings.", false).into(),
            ),
        ]);
        for (toggle, label) in Toggle::DEVICES {
            let key = match toggle {
                Toggle::Clipboard => "clipboard",
                Toggle::Audio => "audio",
                Toggle::Microphone => "microphone",
                Toggle::Camera => "camera",
                Toggle::Drives => "drives",
                Toggle::Printers => "printers",
                Toggle::SmartCard => "smartcard",
                Toggle::MultiMonitor => "monitors",
            };
            rows.push((key, self.toggle_view(ctx, toggle, label)));
        }
        rows.extend([
            ("commands", section_title("Commands").into()),
            (
                "commands-hint",
                hint_text(
                    "Commands run on this computer. The connection is aborted when the \
                     command before connecting fails or runs longer than a minute. \
                     WINDOWS_APP_NAME, WINDOWS_APP_ADDRESS and WINDOWS_APP_TARGET describe the \
                     connection.",
                    false,
                )
                .into(),
            ),
            (
                "pre",
                text_field(
                    "Before connecting",
                    &self.overrides.pre_command,
                    settings_cb(ctx, SettingsMsg::PreCommand),
                    true,
                ),
            ),
            (
                "post",
                text_field(
                    "After disconnecting",
                    &self.overrides.post_command,
                    settings_cb(ctx, SettingsMsg::PostCommand),
                    true,
                ),
            ),
        ]);
        if let Err(error) = &result {
            rows.push(("error", hint_text(error, true).into()));
        }

        shell(
            &format!("{} Settings", self.name),
            Some("Save"),
            result.is_ok(),
            "Cancel",
            !self.closed,
            settings_cb(ctx, SettingsMsg::Closed),
            ScrollViewer::new().content(
                StackPanel::new()
                    .spacing(10.0)
                    .min_width(400.0)
                    .margin(Thickness::new(0.0, 0.0, 16.0, 0.0))
                    .keyed_children(rows),
            ),
        )
    }
}
