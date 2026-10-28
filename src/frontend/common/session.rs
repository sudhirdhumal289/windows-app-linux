//! One remote session, independent of the UI toolkit: the RDP engine, its events, input
//! state (held keys and buttons, wheel remainders), the session-only display scale and the
//! texts the session window shows.
//!
//! Engine events arrive on FreeRDP threads. [`SessionController::start`] queues them and
//! calls `wake`, which must schedule [`SessionController::drain_events`] on the UI thread
//! (e.g. `DispatcherQueue::TryEnqueue` on Windows, `dispatch_async` to the main queue on
//! macOS).

use super::launch::Launch;
use crate::rdp::keymap::PressedKeys;
use crate::rdp::{
    input, BridgeEvent, BridgeOptions, CertificateDecision, CertificatePrompt, LocalClipboard,
    LockKeys, MonitorRect, MouseButton, NativeRdpBridge, SessionStatus, WheelAccumulator,
};
use crate::session::diagnostics::SessionDiagnostics;
use crate::session::keys::{self, KeyCombo, KeyInput};
use crate::session::{PreparedSession, ScaleChoice};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

/// Wakes the UI thread so it drains the event queue.
pub type Wake = Arc<dyn Fn() + Send + Sync + 'static>;

/// Coarse session state for the window chrome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    Connecting(String),
    Connected,
    Ended(String),
}

impl SessionState {
    /// Concise subtitle for window title bars so errors don't overflow the title bar.
    #[must_use]
    pub fn title_subtitle(&self) -> &str {
        match self {
            Self::Connecting(_) => "Connecting...",
            Self::Connected => "",
            Self::Ended(message) if message.is_empty() => "Disconnected",
            Self::Ended(_) => "Connection failed",
        }
    }

    /// Full detailed status for placeholder / in-window display.
    #[must_use]
    pub fn subtitle(&self) -> &str {
        match self {
            Self::Connecting(message) if message.is_empty() => "Connecting...",
            Self::Connecting(message) => message,
            Self::Connected => "Connected",
            Self::Ended(message) if message.is_empty() => "Disconnected",
            Self::Ended(message) => message,
        }
    }
}

impl From<SessionStatus> for SessionState {
    fn from(status: SessionStatus) -> Self {
        match status {
            SessionStatus::Progress(message) => Self::Connecting(message),
            SessionStatus::Connected => Self::Connected,
            SessionStatus::Failed(message) | SessionStatus::Ended(message) => Self::Ended(message),
        }
    }
}

pub struct SessionController {
    pub launch: Launch,
    bridge: Option<NativeRdpBridge>,
    /// Keeps the `.rdp` file (Cloud PCs) for the session's lifetime.
    _prepared: PreparedSession,
    events: Arc<Mutex<VecDeque<BridgeEvent>>>,
    state: SessionState,
    scale: ScaleChoice,
    pressed: PressedKeys,
    held_buttons: Vec<MouseButton>,
    wheel: WheelAccumulator,
    pointer: (u16, u16),
    view_only: bool,
    has_frame: bool,
    diagnostics: SessionDiagnostics,
}

impl SessionController {
    /// Creates the engine for `prepared` and starts connecting.
    pub fn start(launch: Launch, prepared: PreparedSession, wake: Wake) -> Result<Self, String> {
        let events: Arc<Mutex<VecDeque<BridgeEvent>>> = Arc::default();
        let queue = Arc::clone(&events);
        let sink = Arc::new(move |event| {
            let was_empty = {
                let mut queue = queue.lock().unwrap_or_else(PoisonError::into_inner);
                let was_empty = queue.is_empty();
                queue.push_back(event);
                was_empty
            };
            // One wake per batch; the UI drains everything queued meanwhile.
            if was_empty {
                wake();
            }
        });
        let options = BridgeOptions {
            login_hint: launch.login_hint.clone(),
        };
        let bridge = NativeRdpBridge::new(&prepared.args, options, sink).map_err(|e| {
            log::error!("Could not create the RDP session: {e}");
            format!("Could not start the session: {e}")
        })?;
        // The engine has copied the arguments (which may include a password).
        let prepared = PreparedSession {
            args: Vec::new(),
            ..prepared
        };
        bridge.start().map_err(|e| e.to_string())?;
        super::launch::record_start(&launch);
        Ok(Self {
            launch,
            bridge: Some(bridge),
            _prepared: prepared,
            events,
            state: SessionState::Connecting("Connecting…".to_string()),
            scale: ScaleChoice::Auto,
            pressed: PressedKeys::default(),
            held_buttons: Vec::new(),
            wheel: WheelAccumulator::default(),
            pointer: (0, 0),
            view_only: false,
            has_frame: false,
            diagnostics: SessionDiagnostics::new(),
        })
    }

    /// Takes the queued engine events. Status events also update [`Self::state`]; when the
    /// session is over the engine is released (it disconnects in the background).
    pub fn drain_events(&mut self) -> Vec<BridgeEvent> {
        let events: Vec<BridgeEvent> = self
            .events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
            .collect();
        for event in &events {
            match event {
                BridgeEvent::Status(status) => {
                    self.state = SessionState::from(status.clone());
                    if matches!(self.state, SessionState::Ended(_)) {
                        self.end();
                    }
                }
                BridgeEvent::FrameUpdated => {
                    self.has_frame = true;
                    self.diagnostics.record_frame(std::time::Instant::now());
                }
                BridgeEvent::Network(stats) => {
                    self.diagnostics.update_network(*stats);
                }
                _ => {}
            }
        }
        events
    }

    /// The engine while the session runs.
    #[must_use]
    pub fn bridge(&self) -> Option<&NativeRdpBridge> {
        self.bridge.as_ref()
    }

    #[must_use]
    pub fn state(&self) -> &SessionState {
        &self.state
    }

    /// Whether a frame has been received (hide the "connecting" placeholder).
    #[must_use]
    pub fn has_frame(&self) -> bool {
        self.has_frame
    }

    #[must_use]
    pub fn diagnostics(&self) -> &SessionDiagnostics {
        &self.diagnostics
    }

    pub fn diagnostics_mut(&mut self) -> &mut SessionDiagnostics {
        &mut self.diagnostics
    }

    /// Disconnects (in the background) and runs the command after disconnecting.
    pub fn end(&mut self) {
        if self.bridge.take().is_some() {
            super::launch::run_post_command(&self.launch);
        }
        if !matches!(self.state, SessionState::Ended(_)) {
            self.state = SessionState::Ended(String::new());
        }
    }

    // ----- Engine requests -----

    pub fn provide_auth_result(&self, redirect_url: Option<&str>) {
        if let Some(bridge) = &self.bridge {
            bridge.provide_auth_result(redirect_url);
        }
    }

    pub fn answer_certificate(&self, decision: CertificateDecision) {
        if let Some(bridge) = &self.bridge {
            bridge.answer_certificate(decision);
        }
    }

    pub fn set_local_clipboard(&self, content: LocalClipboard) {
        if let Some(bridge) = &self.bridge {
            bridge.set_local_clipboard(content);
        }
    }

    // ----- Display -----

    #[must_use]
    pub fn scale(&self) -> ScaleChoice {
        self.scale
    }

    /// Changes the session-only scale; follow with [`Self::resize`].
    pub fn set_scale(&mut self, choice: ScaleChoice) {
        self.scale = choice;
    }

    /// Asks the host to match a view of `device_size` pixels on a display with scale factor
    /// `display_scale` (1.0 = 100%). Ignored for fixed-resolution connections.
    pub fn resize(&self, device_size: (u32, u32), display_scale: f64) {
        if !self.launch.dynamic_resolution {
            return;
        }
        let Some(bridge) = &self.bridge else { return };
        let (w, h) = device_size;
        if w == 0 || h == 0 {
            return;
        }
        let scale_percent = self.scale.resolve(display_scale);
        log::debug!("Requesting remote resize to {w}x{h} at {scale_percent}%");
        bridge.request_resize(w, h, scale_percent);
    }

    /// Asks the host for a multi-monitor layout (full screen across `monitors`).
    pub fn request_layout(&self, monitors: &[MonitorRect], display_scale: f64) {
        if !self.launch.dynamic_resolution {
            return;
        }
        if let Some(bridge) = &self.bridge {
            bridge.request_layout(monitors, self.scale.resolve(display_scale));
        }
    }

    // ----- Input -----

    #[must_use]
    pub fn view_only(&self) -> bool {
        self.view_only
    }

    /// In view-only mode no input reaches the session (releases still do).
    pub fn set_view_only(&mut self, view_only: bool) {
        self.view_only = view_only;
        if view_only {
            self.release_input();
        }
    }

    /// Pointer moved to `(x, y)` in a view of logical size `view_size` showing the frame.
    pub fn pointer_moved(&mut self, x: f64, y: f64, view_size: (f64, f64)) {
        let Some(bridge) = &self.bridge else { return };
        if self.view_only {
            return;
        }
        let position = input::map_coords(x, y, view_size, bridge.frame_size());
        self.pointer = position;
        bridge.send_pointer_move(position.0, position.1);
    }

    pub fn button(&mut self, button: MouseButton, down: bool) {
        if down {
            if self.view_only {
                return;
            }
            if !self.held_buttons.contains(&button) {
                self.held_buttons.push(button);
            }
        } else if let Some(i) = self.held_buttons.iter().position(|b| *b == button) {
            self.held_buttons.remove(i);
        } else {
            // Release without a press we sent (e.g. the press landed on local UI).
            return;
        }
        if let Some(bridge) = &self.bridge {
            bridge.send_button(button, down, self.pointer.0, self.pointer.1);
        }
    }

    /// Scrolls by `(dx, dy)` wheel notches (fractions accumulate; positive `dy` = down).
    pub fn wheel(&mut self, dx: f64, dy: f64) {
        if self.view_only {
            return;
        }
        let (horizontal, vertical) = self.wheel.push(dx, dy);
        let Some(bridge) = &self.bridge else { return };
        let (x, y) = self.pointer;
        if vertical != 0 {
            bridge.send_wheel(vertical, false, x, y);
        }
        if horizontal != 0 {
            bridge.send_wheel(horizontal, true, x, y);
        }
    }

    /// Key with RDP `scancode` pressed or released.
    pub fn key(&mut self, scancode: u32, down: bool) {
        if down && self.view_only {
            return;
        }
        let repeat = self.pressed.update(scancode, down);
        if let Some(bridge) = &self.bridge {
            bridge.send_key(scancode, down, repeat);
        }
    }

    /// The window gained focus: synchronise the lock keys.
    pub fn focus_in(&self, locks: LockKeys) {
        if let Some(bridge) = &self.bridge {
            bridge.send_focus_in(locks);
        }
    }

    /// Releases everything held in the session (call when the window loses focus, or
    /// e.g. Alt stays held remotely after Alt+Tab).
    pub fn release_input(&mut self) {
        let keys = self.pressed.take_all();
        let buttons = std::mem::take(&mut self.held_buttons);
        self.wheel.reset();
        let Some(bridge) = &self.bridge else { return };
        for scancode in keys {
            bridge.send_key(scancode, false, false);
        }
        for button in buttons {
            bridge.send_button(button, false, self.pointer.0, self.pointer.1);
        }
    }

    /// Sends a shortcut the local system would intercept (Ctrl+Alt+Del, ...).
    pub fn send_combo(&self, combo: KeyCombo) {
        self.send_inputs(&combo.events());
    }

    /// Types `text` as keystrokes (where pasting does not work).
    pub fn type_text(&self, text: &str) -> Result<(), String> {
        let inputs = keys::type_text(text).ok_or_else(|| {
            format!(
                "The clipboard text is too long to type (over {} characters).",
                keys::MAX_TYPED_CHARS
            )
        })?;
        self.send_inputs(&inputs);
        Ok(())
    }

    fn send_inputs(&self, inputs: &[KeyInput]) {
        let Some(bridge) = &self.bridge else { return };
        for input in inputs {
            match *input {
                KeyInput::Scancode { code, down } => bridge.send_key(code, down, false),
                KeyInput::Unicode { unit, down } => bridge.send_unicode(unit, down),
            }
        }
    }
}

/// Entry of the session's display-scale menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScaleEntry {
    pub label: String,
    pub choice: ScaleChoice,
    pub checked: bool,
}

/// "Match Display", the presets and (when chosen) the custom value, with the current one
/// checked. Add a "Custom…" item after these that asks for a percentage and applies
/// [`ScaleChoice::percent`] (capped at [`ScaleChoice::MAX_PERCENT`]).
#[must_use]
pub fn scale_menu(current: ScaleChoice) -> Vec<ScaleEntry> {
    let entry = |label: String, choice: ScaleChoice| ScaleEntry {
        label,
        choice,
        checked: choice == current,
    };
    let mut entries = vec![entry("Match Display".to_string(), ScaleChoice::Auto)];
    for percent in ScaleChoice::PRESETS {
        entries.push(entry(format!("{percent}%"), ScaleChoice::Percent(percent)));
    }
    if current.is_custom() {
        let percent = current.resolve(1.0);
        entries.push(entry(format!("{percent}% (Custom)"), current));
    }
    entries
}

/// Tooltip of the scale button.
#[must_use]
pub fn scale_tooltip(current: ScaleChoice) -> String {
    match current {
        ScaleChoice::Auto => "Display Scale: Match Display".to_string(),
        ScaleChoice::Percent(percent) => format!("Display Scale: {percent}%"),
    }
}

/// Texts of the certificate question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificateText {
    pub heading: &'static str,
    pub intro: String,
    /// Label/value rows (empty values skipped).
    pub details: Vec<(&'static str, String)>,
    /// The certificate changed: make "Always Trust" destructive.
    pub changed: bool,
}

impl CertificateText {
    /// Intro and details as plain text.
    #[must_use]
    pub fn body(&self) -> String {
        let mut body = self.intro.clone();
        body.push('\n');
        for (label, value) in &self.details {
            body.push_str(&format!("\n{label}: {value}"));
        }
        body
    }
}

#[must_use]
pub fn certificate_text(prompt: &CertificatePrompt) -> CertificateText {
    let host = if prompt.port == 3389 || prompt.port == 0 {
        prompt.host.clone()
    } else {
        format!("{}:{}", prompt.host, prompt.port)
    };
    let mut details = vec![
        ("Name", prompt.common_name.clone()),
        ("Subject", prompt.subject.clone()),
        ("Issuer", prompt.issuer.clone()),
        ("Fingerprint", prompt.fingerprint.clone()),
    ];
    if let Some(previous) = &prompt.previous_fingerprint {
        details.push(("Previous fingerprint", previous.clone()));
    }
    details.retain(|(_, value)| !value.is_empty());
    let (heading, intro) = if prompt.changed {
        (
            "Certificate Changed",
            format!(
                "{host} presented a different certificate than before. This can mean someone \
                 is intercepting the connection. Only continue if you know why it changed."
            ),
        )
    } else {
        (
            "Unknown Certificate",
            format!(
                "The identity of {host} cannot be verified. Check that the fingerprint \
                 matches the one your administrator gave you."
            ),
        )
    };
    CertificateText {
        heading,
        intro,
        details,
        changed: prompt.changed,
    }
}

/// Labels of the certificate answers, in button order.
pub const CERTIFICATE_ANSWERS: [(CertificateDecision, &str); 3] = [
    (CertificateDecision::Reject, "Cancel"),
    (CertificateDecision::AcceptOnce, "Connect Once"),
    (CertificateDecision::AcceptAlways, "Always Trust"),
];

/// Where to save a screenshot of the session `name`: the Pictures (or Pictures/Screenshots)
/// folder, named after the session and the time. Encoding is left to the platform.
#[must_use]
pub fn screenshot_path(name: &str) -> std::path::PathBuf {
    let pictures = directories::UserDirs::new()
        .and_then(|d| d.picture_dir().map(std::path::Path::to_path_buf))
        .or_else(|| directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()))
        .unwrap_or_else(std::env::temp_dir);
    let folder = Some(pictures.join("Screenshots"))
        .filter(|dir| dir.is_dir())
        .unwrap_or(pictures);
    let stamp = chrono::Local::now().format("%Y-%m-%d %H-%M-%S");
    let name: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c == ':' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    folder.join(format!("{} {stamp}.png", name.trim_start_matches('.')))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_menu_marks_current_and_custom() {
        let entries = scale_menu(ScaleChoice::Percent(150));
        assert!(entries.iter().any(|e| e.checked && e.label == "150%"));
        assert_eq!(entries.len(), 1 + ScaleChoice::PRESETS.len());

        let entries = scale_menu(ScaleChoice::percent(160));
        assert_eq!(entries.last().unwrap().label, "160% (Custom)");
        assert!(entries.last().unwrap().checked);
    }

    #[test]
    fn maps_status() {
        assert_eq!(
            SessionState::from(SessionStatus::Ended(String::new())).subtitle(),
            "Disconnected"
        );
        assert_eq!(
            SessionState::from(SessionStatus::Connected),
            SessionState::Connected
        );
    }

    #[test]
    fn words_certificates() {
        let prompt = CertificatePrompt {
            host: "pc".into(),
            port: 3390,
            common_name: "pc".into(),
            subject: String::new(),
            issuer: "CA".into(),
            fingerprint: "ab:cd".into(),
            changed: true,
            previous_fingerprint: Some("00".into()),
        };
        let text = certificate_text(&prompt);
        assert_eq!(text.heading, "Certificate Changed");
        assert!(text.intro.starts_with("pc:3390 "));
        assert!(text.details.iter().all(|(label, _)| *label != "Subject"));
        assert!(text.body().contains("Previous fingerprint: 00"));
    }
}
