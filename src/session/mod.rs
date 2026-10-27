//! Toolkit-independent preparation of an RDP session (Cloud PC or saved Remote PC): the
//! private `.rdp` file and the FreeRDP command line handed to the embedded engine
//! ([`crate::rdp`]).

pub mod keys;

use crate::api::RdpLaunch;
use crate::config::{storage, AppConfig};
use crate::connections::{
    parse_gateway_address, AddressError, AuthMethod, CertificatePolicy, Endpoint, SavedConnection,
    SecurityProtocol,
};
use crate::overrides::Resolution;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Lowest/highest desktop scale Windows accepts (percent).
pub const MIN_SCALE: u32 = 100;
pub const MAX_SCALE: u32 = 500;
/// Stale `.rdp` files left by crashed sessions are removed after this long.
const STALE_RDP_FILE_AGE: Duration = Duration::from_secs(12 * 60 * 60);

/// How the remote desktop is encoded.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GraphicsMode {
    /// Graphics pipeline with H.264 (AVC444) when the host supports it. Cloud PC and
    /// Azure Virtual Desktop sessions fall back to RemoteFX when the engine cannot handle
    /// screen-capture protection (see [`SessionOptions::gfx_surface_protection`]).
    #[default]
    Auto,
    /// Classic RemoteFX surface bits.
    RemoteFx,
}

impl GraphicsMode {
    #[must_use]
    pub fn from_id(id: &str) -> Self {
        if id.eq_ignore_ascii_case("rfx") {
            Self::RemoteFx
        } else {
            Self::Auto
        }
    }

    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::RemoteFx => "rfx",
        }
    }
}

/// Connection quality preset: trades visual effects and colour depth for bandwidth.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Quality {
    /// Let the host measure the network and adapt.
    #[default]
    Auto,
    /// Everything on (wallpaper, animations, font smoothing, composition).
    Lan,
    Broadband,
    /// Visual effects off and 16-bit colour.
    Modem,
}

impl Quality {
    pub const ALL: [Self; 4] = [Self::Auto, Self::Lan, Self::Broadband, Self::Modem];

    #[must_use]
    pub fn from_id(id: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|q| q.id().eq_ignore_ascii_case(id))
            .unwrap_or_default()
    }

    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Lan => "lan",
            Self::Broadband => "broadband",
            Self::Modem => "modem",
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Automatic",
            Self::Lan => "Best (LAN)",
            Self::Broadband => "Balanced (broadband)",
            Self::Modem => "Low bandwidth",
        }
    }
}

/// A local folder shared with the session as a named drive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveShare {
    pub name: String,
    pub path: PathBuf,
}

/// A USB device id (`vid:pid`, lowercase hex).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UsbId(String);

impl UsbId {
    /// Parses `vvvv:pppp` (hex), rejecting anything else.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let (vid, pid) = s.trim().split_once(':')?;
        let ok = |p: &str| p.len() == 4 && p.chars().all(|c| c.is_ascii_hexdigit());
        (ok(vid) && ok(pid)).then(|| Self(format!("{}:{}", vid, pid).to_ascii_lowercase()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// User-selected session features.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionOptions {
    /// Initial remote desktop size in device pixels.
    pub width: u32,
    pub height: u32,
    /// Remote desktop scale in percent.
    pub scale_percent: u32,
    pub clipboard: bool,
    pub audio: bool,
    pub microphone: bool,
    pub camera: bool,
    /// Fit the remote desktop to the window as it resizes (off for a fixed resolution).
    pub dynamic_resolution: bool,
    /// Local folders shared as drives.
    pub drives: Vec<DriveShare>,
    /// Share removable drives, including ones plugged in during the session.
    pub hotplug_drives: bool,
    pub quality: Quality,
    /// Username hint for the session host sign-in.
    pub username: Option<String>,
    pub graphics: GraphicsMode,
    /// The engine handles protected (screen-capture protection) graphics-pipeline
    /// surfaces, which Cloud PC and Azure Virtual Desktop hosts may send.
    pub gfx_surface_protection: bool,
    /// The engine supports H.264 (AVC444) in the graphics pipeline; otherwise the
    /// pipeline is negotiated without AVC codecs.
    pub gfx_h264: bool,
    pub usb_devices: Vec<UsbId>,
    pub smartcard: bool,
    pub printers: bool,
}

impl SessionOptions {
    /// Options from the user's preferences. `scale_percent` is the effective scale
    /// (the frontend resolves "automatic" from the display).
    #[must_use]
    pub fn from_config(
        config: &AppConfig,
        width: u32,
        height: u32,
        scale_percent: u32,
        username: Option<String>,
    ) -> Self {
        Self {
            width,
            height,
            scale_percent,
            clipboard: config.enable_clipboard,
            audio: config.enable_audio,
            microphone: config.enable_microphone,
            camera: config.enable_camera,
            dynamic_resolution: true,
            drives: shared_drives(config),
            hotplug_drives: config.enable_drive_redirection && config.hotplug_drives,
            quality: Quality::from_id(&config.quality),
            username: username.filter(|u| !u.trim().is_empty()),
            graphics: GraphicsMode::from_id(&config.graphics_mode),
            gfx_surface_protection: crate::rdp::supports_surface_protection(),
            gfx_h264: crate::rdp::supports_h264(),
            usb_devices: usb_devices(config),
            smartcard: config.enable_smartcard,
            printers: config.enable_printers,
        }
    }
}

impl SessionOptions {
    /// Uses a fixed desktop size instead of following the window.
    #[must_use]
    pub fn with_resolution(mut self, resolution: Option<Resolution>) -> Self {
        if let Some(size) = resolution.map(Resolution::clamped) {
            self.width = size.width;
            self.height = size.height;
            self.dynamic_resolution = false;
        }
        self
    }
}

/// Folders from the preferences that can be shared: the main shared folder (as
/// `Shared`) and the extra folders, skipping missing ones and repeated names.
/// USB devices to redirect, or none when the engine lacks the USB redirection channel.
fn usb_devices(config: &AppConfig) -> Vec<UsbId> {
    let devices: Vec<UsbId> = config
        .usb_devices
        .iter()
        .filter_map(|id| UsbId::parse(id))
        .collect();
    if !devices.is_empty() && !crate::rdp::supports_usb_redirection() {
        log::warn!("Not redirecting USB devices: FreeRDP was built without USB redirection");
        return Vec::new();
    }
    devices
}

/// A drive path for FreeRDP's comma-separated `/drive` option. Its parser treats `\` as an
/// escape, so a trailing one (`C:\`, `%TEMP%` on Windows) would swallow the end of the
/// option and fail the session; the same directory is named with a trailing `.` instead.
fn drive_path_arg(path: &Path) -> String {
    let mut value = path.display().to_string();
    if value.ends_with('\\') {
        if !cfg!(windows) {
            value.push('/');
        }
        value.push('.');
    }
    value
}

fn shared_drives(config: &AppConfig) -> Vec<DriveShare> {
    if !config.enable_drive_redirection {
        return Vec::new();
    }
    let main = crate::config::SharedFolder {
        name: "Shared".to_string(),
        path: config.shared_folder_path.clone(),
    };
    let mut drives: Vec<DriveShare> = Vec::new();
    for folder in std::iter::once(&main).chain(&config.extra_folders) {
        let path = PathBuf::from(&folder.path);
        let name = folder.name.trim();
        if !folder.is_valid() || !path.is_dir() {
            if !folder.path.is_empty() {
                log::warn!("Not sharing {:?}: missing or unsupported path", folder.path);
            }
            continue;
        }
        if drives.iter().any(|d| d.name.eq_ignore_ascii_case(name)) {
            continue;
        }
        drives.push(DriveShare {
            name: name.to_string(),
            path,
        });
    }
    drives
}

/// Clamps a desktop scale to the range Windows accepts.
#[must_use]
pub fn clamp_scale(percent: u32) -> u32 {
    percent.clamp(MIN_SCALE, MAX_SCALE)
}

/// Device scale factor FreeRDP accepts (100, 140 or 180) closest to `desktop_scale`.
#[must_use]
pub fn device_scale_for(desktop_scale: u32) -> u32 {
    match desktop_scale {
        0..=139 => 100,
        140..=179 => 140,
        _ => 180,
    }
}

/// Scale the user picked for a running session (never persisted).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScaleChoice {
    /// Follow the scale of the display the window is on.
    #[default]
    Auto,
    Percent(u32),
}

impl ScaleChoice {
    /// Scales offered in the session menu.
    pub const PRESETS: [u32; 5] = [100, 125, 150, 175, 200];
    /// Highest scale the session menu allows (custom values included).
    pub const MAX_PERCENT: u32 = 200;

    /// A fixed scale, clamped to `MIN_SCALE..=MAX_PERCENT`.
    #[must_use]
    pub fn percent(percent: u32) -> Self {
        Self::Percent(percent.clamp(MIN_SCALE, Self::MAX_PERCENT))
    }

    /// Effective desktop scale for a display with scale factor `display_scale` (1.0 = 100%).
    #[must_use]
    pub fn resolve(self, display_scale: f64) -> u32 {
        match self {
            Self::Auto => clamp_scale((display_scale * 100.0).round() as u32),
            Self::Percent(percent) => clamp_scale(percent),
        }
    }

    /// Whether this is a custom (non-preset) fixed scale.
    #[must_use]
    pub fn is_custom(self) -> bool {
        matches!(self, Self::Percent(p) if !Self::PRESETS.contains(&p))
    }

    /// Stable identifier for menus: `auto` or the percentage.
    #[must_use]
    pub fn id(self) -> String {
        match self {
            Self::Auto => "auto".to_string(),
            Self::Percent(percent) => percent.to_string(),
        }
    }

    /// Inverse of [`Self::id`]; out-of-range percentages are clamped.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "auto" => Some(Self::Auto),
            _ => id.parse().ok().map(Self::percent),
        }
    }
}

/// Where the session connects to.
enum Target<'a> {
    /// Cloud PC through the ARM gateway, described by an `.rdp` file.
    CloudPc {
        rdp_file: &'a Path,
        gateway_token: &'a str,
    },
    /// Remote PC reached directly (or through its Remote Desktop Gateway).
    Direct {
        connection: &'a SavedConnection,
        endpoint: &'a Endpoint,
        credentials: &'a Credentials,
    },
}

/// Passwords for one direct session (never stored in the argument builder).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Credentials {
    pub password: Option<String>,
    /// Gateway password when the gateway uses its own credentials.
    pub gateway_password: Option<String>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("password", &self.password.as_ref().map(|_| "******"))
            .field(
                "gateway_password",
                &self.gateway_password.as_ref().map(|_| "******"),
            )
            .finish()
    }
}

/// Why a direct session cannot be prepared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PrepareError {
    #[error(transparent)]
    Address(#[from] AddressError),
    #[error("Gateway passwords containing commas are not supported")]
    GatewayPassword,
}

/// Builds the FreeRDP argument vector (argv[0] included) for a Cloud PC.
#[must_use]
pub fn build_freerdp_args(
    rdp_file: &Path,
    gateway_token: &str,
    opts: &SessionOptions,
) -> Vec<String> {
    build_args(
        &Target::CloudPc {
            rdp_file,
            gateway_token,
        },
        opts,
    )
}

/// Builds the FreeRDP argument vector (argv[0] included) for a saved Remote PC.
pub fn build_direct_args(
    connection: &SavedConnection,
    credentials: &Credentials,
    opts: &SessionOptions,
) -> Result<Vec<String>, PrepareError> {
    let endpoint = connection.endpoint()?;
    if credentials
        .gateway_password
        .as_ref()
        .is_some_and(|p| p.contains(','))
    {
        return Err(PrepareError::GatewayPassword);
    }
    Ok(build_args(
        &Target::Direct {
            connection,
            endpoint: &endpoint,
            credentials,
        },
        opts,
    ))
}

/// Whether to negotiate the graphics pipeline. Microsoft-hosted desktops may enable
/// screen-capture protection, which engines without support for it cannot display.
fn use_graphics_pipeline(target: &Target<'_>, opts: &SessionOptions) -> bool {
    match opts.graphics {
        GraphicsMode::RemoteFx => false,
        GraphicsMode::Auto => {
            matches!(target, Target::Direct { .. }) || opts.gfx_surface_protection
        }
    }
}

/// Audio output/input channel argument. Linux uses PulseAudio (also served by PipeWire);
/// elsewhere FreeRDP picks the platform backend (WinMM on Windows, Core Audio on macOS).
fn audio_arg(channel: &str) -> String {
    if cfg!(target_os = "linux") {
        if channel == "microphone" {
            // FreeRDP audio input [MS-RDPEAI] on Linux:
            // format:1 enforces raw uncompressed 16-bit PCM (WAVE_FORMAT_PCM) instead of
            // lossy compression codecs (which cause noisy or muffled audio), and rate:44100
            // aligns with Windows Audio Service to prevent sample-rate resampling jitter.
            format!("/{channel}:sys:pulse,format:1,rate:44100")
        } else {
            format!("/{channel}:sys:pulse")
        }
    } else {
        format!("/{channel}")
    }
}

fn build_args(target: &Target<'_>, opts: &SessionOptions) -> Vec<String> {
    let mut args = vec!["freerdp".to_string()];
    match target {
        Target::CloudPc {
            rdp_file,
            gateway_token,
        } => args.extend([
            rdp_file.display().to_string(),
            format!("/gateway:type:arm,bearer:{gateway_token}"),
            // AVD session hosts present self-signed certificates; the connection is already
            // authenticated end-to-end by the ARM gateway and Entra ID (RDSAAD).
            "/cert:ignore".to_string(),
        ]),
        Target::Direct {
            connection,
            endpoint,
            credentials,
        } => {
            args.push(format!("/v:{endpoint}"));
            direct_args(&mut args, connection, credentials);
        }
    }
    let scale = clamp_scale(opts.scale_percent);
    args.extend([
        format!("/size:{}x{}", opts.width.max(640), opts.height.max(480)),
        if opts.dynamic_resolution {
            "/dynamic-resolution"
        } else {
            "-dynamic-resolution"
        }
        .to_string(),
        format!("/scale-desktop:{scale}"),
        format!("/scale-device:{}", device_scale_for(scale)),
        if !use_graphics_pipeline(target, opts) {
            "/rfx"
        } else if opts.gfx_h264 {
            "/gfx:AVC444"
        } else {
            "/gfx"
        }
        .to_string(),
        // `auto` lets the host measure bandwidth and latency (shown as connection
        // quality); the presets also set the matching visual effects.
        format!("/network:{}", opts.quality.id()),
        "+auto-reconnect".to_string(),
        "/auto-reconnect-max-retries:5".to_string(),
        if opts.clipboard {
            "+clipboard"
        } else {
            "-clipboard"
        }
        .to_string(),
    ]);
    if opts.audio {
        args.push(audio_arg("sound"));
    }
    if opts.microphone {
        args.push(audio_arg("microphone"));
    }
    if opts.camera && crate::rdp::supports_camera() && matches!(target, Target::Direct { .. }) {
        args.push("/dvc:rdpecam".to_string());
    }
    if opts.quality == Quality::Modem {
        args.push("/bpp:16".to_string());
    } else {
        args.push("+fonts".to_string());
        args.push("+aero".to_string());
        args.push("+window-drag".to_string());
    }
    for drive in &opts.drives {
        args.push(format!(
            "/drive:{},{}",
            drive.name,
            drive_path_arg(&drive.path)
        ));
    }
    if opts.hotplug_drives {
        args.push("/drive:hotplug,*".to_string());
    }
    if !opts.usb_devices.is_empty() {
        let ids: Vec<&str> = opts.usb_devices.iter().map(UsbId::as_str).collect();
        args.push(format!("/usb:id:{}", ids.join("#")));
    }
    if opts.smartcard {
        args.push("/smartcard".to_string());
    }
    if opts.printers {
        args.push("/printer".to_string());
    }
    if let Some(user) = &opts.username {
        args.push(format!("/u:{user}"));
    }
    if let Target::Direct {
        credentials: Credentials {
            password: Some(password),
            ..
        },
        ..
    } = target
    {
        if !password.is_empty() {
            args.push(format!("/p:{password}"));
        }
    }
    args
}

/// Arguments specific to a saved Remote PC: certificate policy, security, gateway,
/// domain, console session and RemoteApp.
fn direct_args(args: &mut Vec<String>, connection: &SavedConnection, credentials: &Credentials) {
    match connection.certificate {
        // No option: the engine asks the frontend about unknown or changed certificates
        // and remembers accepted ones.
        CertificatePolicy::Ask => {}
        CertificatePolicy::TrustFirstUse => args.push("/cert:tofu".to_string()),
        CertificatePolicy::Ignore => args.push("/cert:ignore".to_string()),
    }
    match connection.security {
        SecurityProtocol::Auto => {}
        SecurityProtocol::Nla => args.push("/sec:nla".to_string()),
        SecurityProtocol::Tls => args.push("/sec:tls".to_string()),
        SecurityProtocol::Rdp => args.push("/sec:rdp".to_string()),
    }
    match connection.auth {
        AuthMethod::Auto => {}
        AuthMethod::Kerberos => args.push("/auth-pkg-list:kerberos,!ntlm".to_string()),
        AuthMethod::Ntlm => args.push("/auth-pkg-list:ntlm,!kerberos".to_string()),
    }
    if !connection.domain.is_empty() {
        args.push(format!("/d:{}", connection.domain));
    }
    if connection.admin_session {
        args.push("/admin".to_string());
    }
    if let Some(gateway) = &connection.gateway {
        if let Ok(endpoint) = parse_gateway_address(&gateway.address) {
            let mut option = format!("/gateway:g:{endpoint}");
            if gateway.separate_credentials {
                if !gateway.username.is_empty() {
                    option.push_str(&format!(",u:{}", gateway.username));
                }
                if let Some(password) = credentials
                    .gateway_password
                    .as_deref()
                    .filter(|p| !p.is_empty())
                {
                    option.push_str(&format!(",p:{password}"));
                }
            }
            args.push(option);
        }
    }
    if let Some(app) = &connection.remote_app {
        let mut option = format!("/app:program:{}", app.program);
        if !app.arguments.is_empty() {
            option.push_str(&format!(",cmd:{}", app.arguments));
        }
        args.push(option);
    }
}

/// Copy of `args` safe for logging (bearer token and password removed).
#[must_use]
pub fn redact_args(args: &[String]) -> Vec<String> {
    args.iter()
        .map(|a| {
            if a.starts_with("/gateway:type:arm") {
                "/gateway:type:arm,bearer:<redacted>".to_string()
            } else if let Some(gateway) = a.strip_prefix("/gateway:") {
                let kept: Vec<&str> = gateway
                    .split(',')
                    .map(|part| {
                        if part.starts_with("p:") {
                            "p:<redacted>"
                        } else {
                            part
                        }
                    })
                    .collect();
                format!("/gateway:{}", kept.join(","))
            } else if a.starts_with("/p:") {
                "/p:<redacted>".to_string()
            } else {
                a.clone()
            }
        })
        .collect()
}

/// Value of a `key:type:value` line in an `.rdp` file.
#[must_use]
pub fn rdp_property<'a>(rdp_file: &'a str, key: &str) -> Option<&'a str> {
    rdp_file.lines().find_map(|line| {
        let mut parts = line.trim().splitn(3, ':');
        let (k, _ty, value) = (parts.next()?, parts.next()?, parts.next()?);
        k.eq_ignore_ascii_case(key).then_some(value.trim())
    })
}

/// Harmonizes an `.rdp` file downloaded from Cloud PC/AVD with the user's active session options.
/// Ensures standard RDP properties (`camerastoredirect`, `audiocapturemode`, etc.) match the user's choices,
/// preventing host-side policy defaults (such as empty `camerastoredirect:s:`) from overriding
/// or conflicting with FreeRDP dynamic virtual channels (`/dvc:rdpecam`).
#[must_use]
pub fn sync_rdp_file_content(content: &str, opts: &SessionOptions) -> String {
    let mut properties: Vec<(String, char, String)> = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut parts = trimmed.splitn(3, ':');
        if let (Some(k), Some(ty), Some(v)) = (parts.next(), parts.next(), parts.next()) {
            let key = k.trim().to_ascii_lowercase();
            let ty_char = ty.chars().next().unwrap_or('s');
            properties.push((key, ty_char, v.trim().to_string()));
        }
    }

    // Overrides / injections to guarantee alignment with SessionOptions:
    let desired: [(&str, char, String); 6] = [
        (
            "camerastoredirect",
            's',
            if opts.camera {
                "*".to_string()
            } else {
                String::new()
            },
        ),
        (
            "audiocapturemode",
            'i',
            if opts.microphone {
                "1".to_string()
            } else {
                "0".to_string()
            },
        ),
        (
            "audiomode",
            'i',
            if opts.audio {
                "0".to_string()
            } else {
                "1".to_string()
            },
        ),
        (
            "redirectclipboard",
            'i',
            if opts.clipboard {
                "1".to_string()
            } else {
                "0".to_string()
            },
        ),
        (
            "redirectprinters",
            'i',
            if opts.printers {
                "1".to_string()
            } else {
                "0".to_string()
            },
        ),
        (
            "redirectsmartcards",
            'i',
            if opts.smartcard {
                "1".to_string()
            } else {
                "0".to_string()
            },
        ),
    ];

    for (target_key, target_ty, target_val) in desired {
        let mut found = false;
        for (k, ty, v) in &mut properties {
            if k == target_key {
                *ty = target_ty;
                *v = target_val.clone();
                found = true;
                break;
            }
        }
        if !found {
            properties.push((target_key.to_string(), target_ty, target_val));
        }
    }

    let mut out = String::with_capacity(content.len() + 128);
    for (k, ty, v) in properties {
        out.push_str(&format!("{k}:{ty}:{v}\r\n"));
    }
    out
}

/// An owner-only `.rdp` file in the runtime directory, deleted when dropped.
#[derive(Debug)]
pub struct RdpFile {
    path: PathBuf,
}

impl RdpFile {
    pub fn create(content: &str) -> std::io::Result<Self> {
        let dir = crate::platform::runtime_dir();
        storage::create_private_dir(&dir)?;
        remove_stale_files(&dir);
        let path = dir.join(format!("session-{}.rdp", uuid::Uuid::new_v4()));
        storage::write_private(&path, content.as_bytes())?;
        Ok(Self { path })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for RdpFile {
    fn drop(&mut self) {
        if let Err(e) = storage::remove_if_exists(&self.path) {
            log::warn!("Could not remove {}: {e}", self.path.display());
        }
    }
}

fn remove_stale_files(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_rdp = path.extension().is_some_and(|e| e == "rdp");
        let is_old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > STALE_RDP_FILE_AGE);
        if is_rdp && is_old {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// A session ready to hand to the RDP engine. Keep it alive for the whole session:
/// dropping it deletes the `.rdp` file (Cloud PCs only).
pub struct PreparedSession {
    pub rdp_file: Option<RdpFile>,
    pub args: Vec<String>,
}

impl std::fmt::Debug for PreparedSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedSession")
            .field("rdp_file", &self.rdp_file)
            .field("args", &redact_args(&self.args))
            .finish()
    }
}

/// Prepares a Cloud PC session from the launch details returned by the service.
pub fn prepare(launch: &RdpLaunch, opts: &SessionOptions) -> std::io::Result<PreparedSession> {
    let synced = sync_rdp_file_content(&launch.rdp_file, opts);
    let rdp_file = RdpFile::create(&synced)?;
    let args = build_freerdp_args(rdp_file.path(), &launch.gateway_token, opts);
    Ok(PreparedSession {
        rdp_file: Some(rdp_file),
        args,
    })
}

/// Prepares a direct session to a saved Remote PC. `credentials` are used for this
/// session only; `opts.username` overrides the saved username.
pub fn prepare_direct(
    connection: &SavedConnection,
    credentials: &Credentials,
    opts: &SessionOptions,
) -> Result<PreparedSession, PrepareError> {
    Ok(PreparedSession {
        rdp_file: None,
        args: build_direct_args(connection, credentials, opts)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> SessionOptions {
        SessionOptions {
            width: 1920,
            height: 1080,
            scale_percent: 150,
            clipboard: true,
            audio: true,
            microphone: false,
            camera: true,
            dynamic_resolution: true,
            drives: vec![DriveShare {
                name: "Shared".into(),
                path: PathBuf::from("/home/u/Shared Docs"),
            }],
            hotplug_drives: false,
            quality: Quality::Auto,
            username: Some("a@b.com".into()),
            graphics: GraphicsMode::Auto,
            gfx_surface_protection: true,
            gfx_h264: true,
            usb_devices: vec![
                UsbId::parse("046D:C52B").unwrap(),
                UsbId::parse("1050:0407").unwrap(),
            ],
            smartcard: true,
            printers: true,
        }
    }

    #[test]
    fn builds_full_argument_list() {
        let args = build_freerdp_args(Path::new("/run/x.rdp"), "TOKEN", &opts());
        assert_eq!(args[0], "freerdp");
        assert_eq!(args[1], "/run/x.rdp");
        assert_eq!(args[2], "/gateway:type:arm,bearer:TOKEN");
        for expected in [
            "/size:1920x1080",
            "/scale-desktop:150",
            "/scale-device:140",
            "+clipboard",
            "+auto-reconnect",
            "+fonts",
            "+aero",
            "+window-drag",
            "/drive:Shared,/home/u/Shared Docs",
            "/u:a@b.com",
            "/gfx:AVC444",
            "/network:auto",
            "/usb:id:046d:c52b#1050:0407",
            "/smartcard",
            "/printer",
        ] {
            assert!(args.iter().any(|a| a == expected), "missing {expected}");
        }
        assert!(args.contains(&audio_arg("sound")));
        assert!(!args.iter().any(|a| a.starts_with("/microphone")));
    }

    #[test]
    fn direct_camera_argument() {
        let conn = SavedConnection::new("Office", "192.168.1.100", "user").unwrap();
        let mut o = opts();
        o.camera = true;
        let args = build_direct_args(&conn, &Credentials::default(), &o).unwrap();
        if crate::rdp::supports_camera() {
            assert!(args.contains(&"/dvc:rdpecam".to_string()));
        } else {
            assert!(!args.contains(&"/dvc:rdpecam".to_string()));
        }
    }

    #[test]
    fn minimal_arguments_and_redaction() {
        let o = SessionOptions {
            clipboard: false,
            audio: false,
            camera: false,
            drives: Vec::new(),
            username: None,
            scale_percent: 50,
            width: 10,
            height: 10,
            graphics: GraphicsMode::RemoteFx,
            usb_devices: Vec::new(),
            smartcard: false,
            printers: false,
            ..opts()
        };
        let args = build_freerdp_args(Path::new("f.rdp"), "SECRET", &o);
        assert!(args.contains(&"-clipboard".to_string()));
        assert!(args.contains(&"/scale-desktop:100".to_string()));
        assert!(args.contains(&"/size:640x480".to_string()));
        assert!(args.contains(&"/rfx".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("/gfx")
            || a.starts_with("/usb")
            || a == "/smartcard"
            || a == "/printer"));
        assert!(!args
            .iter()
            .any(|a| a.starts_with("/u:") || a.starts_with("/drive")));
        assert!(!redact_args(&args).iter().any(|a| a.contains("SECRET")));
    }

    #[test]
    fn cloud_pc_avoids_gfx_without_surface_protection() {
        let o = SessionOptions {
            gfx_surface_protection: false,
            ..opts()
        };
        let args = build_freerdp_args(Path::new("f.rdp"), "T", &o);
        assert!(args.contains(&"/rfx".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("/gfx")));

        let conn = SavedConnection::new("", "host", "").unwrap();
        let args = build_direct_args(&conn, &Credentials::default(), &o).unwrap();
        assert!(args.contains(&"/gfx:AVC444".to_string()));
    }

    #[test]
    fn graphics_pipeline_without_h264() {
        let o = SessionOptions {
            gfx_h264: false,
            ..opts()
        };
        let args = build_freerdp_args(Path::new("f.rdp"), "T", &o);
        assert!(args.contains(&"/gfx".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("/gfx:") || a == "/rfx"));
    }

    #[test]
    fn direct_arguments() {
        let conn = SavedConnection::new("Office", "[::1]:3390", "CORP\\me").unwrap();
        let o = SessionOptions {
            username: Some(conn.username.clone()),
            ..opts()
        };
        let credentials = Credentials {
            password: Some("pa:ss word".into()),
            gateway_password: None,
        };
        let prepared = prepare_direct(&conn, &credentials, &o).unwrap();
        let args = &prepared.args;
        assert!(prepared.rdp_file.is_none());
        assert_eq!(args[1], "/v:[::1]:3390");
        assert!(!args.iter().any(|a| a.starts_with("/cert")));
        assert!(args.contains(&"/u:CORP\\me".to_string()));
        assert!(args.contains(&"/p:pa:ss word".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("/gateway")));
        let redacted = redact_args(args);
        assert!(!redacted.iter().any(|a| a.contains("pa:ss")));
        assert!(!format!("{prepared:?}").contains("pa:ss"));

        let empty = Credentials {
            password: Some(String::new()),
            gateway_password: None,
        };
        let no_password = build_direct_args(&conn, &empty, &o).unwrap();
        assert!(!no_password.iter().any(|a| a.starts_with("/p:")));
        assert!(!format!("{credentials:?}").contains("pa:ss"));
    }

    #[test]
    fn advanced_direct_arguments() {
        let mut conn = SavedConnection::new("", "pc", "me").unwrap();
        conn.domain = "CORP".into();
        conn.admin_session = true;
        conn.security = SecurityProtocol::Nla;
        conn.auth = AuthMethod::Kerberos;
        conn.certificate = CertificatePolicy::TrustFirstUse;
        conn.gateway = Some(crate::connections::Gateway {
            address: "gw.corp.com".into(),
            username: "gwuser".into(),
            separate_credentials: true,
        });
        conn.remote_app = Some(crate::connections::RemoteApp {
            program: "||calc".into(),
            arguments: "/x y".into(),
        });
        let credentials = Credentials {
            password: None,
            gateway_password: Some("gw:secret".into()),
        };
        let args = build_direct_args(&conn, &credentials, &opts()).unwrap();
        for expected in [
            "/cert:tofu",
            "/sec:nla",
            "/auth-pkg-list:kerberos,!ntlm",
            "/d:CORP",
            "/admin",
            "/gateway:g:gw.corp.com:443,u:gwuser,p:gw:secret",
            "/app:program:||calc,cmd:/x y",
        ] {
            assert!(args.iter().any(|a| a == expected), "missing {expected}");
        }
        let redacted = redact_args(&args);
        assert!(redacted.contains(&"/gateway:g:gw.corp.com:443,u:gwuser,p:<redacted>".into()));
        assert!(!redacted.iter().any(|a| a.contains("secret")));

        let bad = Credentials {
            gateway_password: Some("a,b".into()),
            ..credentials
        };
        assert_eq!(
            build_direct_args(&conn, &bad, &opts()),
            Err(PrepareError::GatewayPassword)
        );
        conn.gateway.as_mut().unwrap().separate_credentials = false;
        conn.certificate = CertificatePolicy::Ignore;
        let args = build_direct_args(&conn, &Credentials::default(), &opts()).unwrap();
        assert!(args.contains(&"/gateway:g:gw.corp.com:443".to_string()));
        assert!(args.contains(&"/cert:ignore".to_string()));
    }

    #[test]
    fn quality_drives_and_fixed_resolution() {
        let o = SessionOptions {
            quality: Quality::Modem,
            hotplug_drives: true,
            drives: vec![
                DriveShare {
                    name: "Shared".into(),
                    path: "/a".into(),
                },
                DriveShare {
                    name: "Music".into(),
                    path: "/b".into(),
                },
            ],
            ..opts()
        }
        .with_resolution(Some(Resolution::new(1280, 720)));
        let args = build_freerdp_args(Path::new("f.rdp"), "T", &o);
        for expected in [
            "/network:modem",
            "/bpp:16",
            "/drive:Shared,/a",
            "/drive:Music,/b",
            "/drive:hotplug,*",
            "/size:1280x720",
            "-dynamic-resolution",
        ] {
            assert!(args.iter().any(|a| a == expected), "missing {expected}");
        }
        assert_eq!(Quality::from_id("BROADBAND"), Quality::Broadband);
        assert_eq!(Quality::from_id("?"), Quality::Auto);
    }

    #[test]
    fn shares_valid_existing_folders_once() {
        let dir = std::env::temp_dir();
        let config = AppConfig {
            enable_drive_redirection: true,
            shared_folder_path: dir.display().to_string(),
            hotplug_drives: true,
            extra_folders: vec![
                crate::config::SharedFolder {
                    name: "Temp".into(),
                    path: dir.display().to_string(),
                },
                crate::config::SharedFolder {
                    name: "shared".into(),
                    path: dir.display().to_string(),
                },
                crate::config::SharedFolder {
                    name: "Gone".into(),
                    path: "/no/such/dir".into(),
                },
            ],
            ..AppConfig::default()
        };
        let o = SessionOptions::from_config(&config, 800, 600, 100, None);
        let names: Vec<&str> = o.drives.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["Shared", "Temp"]);
        assert!(o.hotplug_drives);
        let off = AppConfig {
            enable_drive_redirection: false,
            ..config
        };
        let o = SessionOptions::from_config(&off, 800, 600, 100, None);
        assert!(o.drives.is_empty() && !o.hotplug_drives);
    }

    #[test]
    fn scale_choices() {
        assert_eq!(ScaleChoice::Auto.resolve(1.5), 150);
        assert_eq!(ScaleChoice::Auto.resolve(0.5), 100);
        assert_eq!(ScaleChoice::percent(170).resolve(1.0), 170);
        assert_eq!(ScaleChoice::percent(250), ScaleChoice::Percent(200));
        assert_eq!(ScaleChoice::percent(50), ScaleChoice::Percent(100));
        assert!(ScaleChoice::percent(170).is_custom());
        assert!(!ScaleChoice::percent(175).is_custom());
        assert!(!ScaleChoice::Auto.is_custom());
        for choice in [ScaleChoice::Auto, ScaleChoice::Percent(125)] {
            assert_eq!(ScaleChoice::from_id(&choice.id()), Some(choice));
        }
        assert_eq!(ScaleChoice::from_id("999"), Some(ScaleChoice::Percent(200)));
        assert_eq!(ScaleChoice::from_id("x"), None);
    }

    #[test]
    fn usb_ids_and_graphics_modes() {
        assert_eq!(UsbId::parse(" 046D:c52b ").unwrap().as_str(), "046d:c52b");
        for bad in [
            "046d",
            "046d:c52",
            "zzzz:0000",
            "046d:c52b:1",
            "046d:c52b#1",
        ] {
            assert_eq!(UsbId::parse(bad), None, "{bad}");
        }
        assert_eq!(GraphicsMode::from_id("rfx"), GraphicsMode::RemoteFx);
        assert_eq!(GraphicsMode::from_id("other"), GraphicsMode::Auto);
        let config = AppConfig {
            usb_devices: vec!["046d:c52b".into(), "bogus".into()],
            ..AppConfig::default()
        };
        let o = SessionOptions::from_config(&config, 800, 600, 100, None);
        let expected = usize::from(crate::rdp::supports_usb_redirection());
        assert_eq!(o.usb_devices.len(), expected);
    }

    #[test]
    fn drive_paths_never_end_in_an_escape() {
        assert_eq!(
            drive_path_arg(Path::new("/home/u/Shared")),
            "/home/u/Shared"
        );
        let expected = if cfg!(windows) { r"C:\." } else { r"C:\/." };
        assert_eq!(drive_path_arg(Path::new(r"C:\")), expected);
        assert_eq!(drive_path_arg(Path::new(r"C:\Temp")), r"C:\Temp");
    }

    #[test]
    fn device_scale_mapping() {
        assert_eq!(device_scale_for(100), 100);
        assert_eq!(device_scale_for(125), 100);
        assert_eq!(device_scale_for(150), 140);
        assert_eq!(device_scale_for(200), 180);
    }

    #[test]
    fn reads_rdp_properties() {
        let rdp = "full address:s:host\r\naadtenantid:s: abc-123 \r\nweird line\n";
        assert_eq!(rdp_property(rdp, "aadtenantid"), Some("abc-123"));
        assert_eq!(rdp_property(rdp, "full address"), Some("host"));
        assert_eq!(rdp_property(rdp, "missing"), None);
    }

    #[test]
    fn rdp_file_is_private_and_removed_on_drop() {
        let file = RdpFile::create("full address:s:x\n").unwrap();
        let path = file.path().to_path_buf();
        assert!(path.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        drop(file);
        assert!(!path.exists());
    }
    #[test]
    fn syncs_rdp_file_content_with_session_options() {
        let base = "full address:s:avd-host.cloud.microsoft\r\ncamerastoredirect:s:\r\naudiocapturemode:i:0\r\n";
        let synced = sync_rdp_file_content(base, &opts());
        assert_eq!(rdp_property(&synced, "camerastoredirect"), Some("*"));
        assert_eq!(rdp_property(&synced, "audiocapturemode"), Some("0"));
        assert_eq!(rdp_property(&synced, "audiomode"), Some("0"));
        assert_eq!(rdp_property(&synced, "redirectclipboard"), Some("1"));

        let mut o = opts();
        o.camera = false;
        o.microphone = true;
        let synced_no_cam = sync_rdp_file_content(base, &o);
        assert_eq!(rdp_property(&synced_no_cam, "camerastoredirect"), Some(""));
        assert_eq!(rdp_property(&synced_no_cam, "audiocapturemode"), Some("1"));
    }
}
