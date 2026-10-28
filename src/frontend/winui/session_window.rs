//! The remote desktop window: a plain Win32 window on the UI thread (WinUI pumps its
//! messages) that paints the session frame with GDI and forwards real keyboard scancodes
//! and mouse input to the [`SessionController`].
//!
//! Engine events wake the window with a posted message ([`WM_SESSION_WAKE`]); the window
//! procedure then drains them. The session menu lives in the window's system menu (click
//! the icon, Alt+Space when the session has ended, or Ctrl+Alt+Home at any time) and offers
//! key combinations, typing the clipboard, the display scale, view-only and full-screen
//! modes, screenshots and disconnecting.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreateSolidBrush,
    DeleteDC, DeleteObject, DrawTextW, EndPaint, EnumDisplayMonitors, FillRect, GetMonitorInfoW,
    InvalidateRect, MonitorFromWindow, SelectObject, SetBkMode, SetBrushOrgEx, SetStretchBltMode,
    SetTextColor, StretchDIBits, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CLEARTYPE_QUALITY,
    CLIP_DEFAULT_PRECIS, COLORONCOLOR, DEFAULT_CHARSET, DIB_RGB_COLORS, DT_CALCRECT, DT_CENTER,
    DT_NOPREFIX, DT_WORDBREAK, FW_NORMAL, FW_SEMIBOLD, HALFTONE, HDC, HMONITOR, MONITORINFO,
    MONITORINFOEXW, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, OUT_DEFAULT_PRECIS,
    PAINTSTRUCT, SRCCOPY, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, VIRTUAL_KEY, VK_CAPITAL, VK_CONTROL,
    VK_HOME, VK_MENU, VK_NUMLOCK, VK_RETURN, VK_SCROLL,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DeleteMenu, DestroyWindow,
    GetClientRect, GetMenuItemCount, GetSystemMenu, GetSystemMetrics, GetWindowLongPtrW,
    GetWindowPlacement, KillTimer, LoadCursorW, PostMessageW, RegisterClassExW,
    SetForegroundWindow, SetTimer, SetWindowLongPtrW, SetWindowPlacement, SetWindowPos,
    SetWindowTextW, ShowWindow, TrackPopupMenu, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, GWL_STYLE,
    HMENU, HWND_TOP, IDC_ARROW, MF_BYPOSITION, MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR,
    MF_STRING, MINMAXINFO, SC_KEYMENU, SIZE_MINIMIZED, SM_CMONITORS, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_FRAMECHANGED, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER, SW_SHOW, TPM_LEFTALIGN, TPM_RETURNCMD,
    TPM_TOPALIGN, USER_DEFAULT_SCREEN_DPI, WINDOWPLACEMENT, WINDOW_EX_STYLE, WM_APP, WM_CHAR,
    WM_CLOSE, WM_DESTROY, WM_DPICHANGED, WM_ERASEBKGND, WM_GETMINMAXINFO, WM_INITMENUPOPUP,
    WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP,
    WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT, WM_RBUTTONDOWN, WM_RBUTTONUP,
    WM_SETFOCUS, WM_SIZE, WM_SYSCHAR, WM_SYSCOMMAND, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER,
    WM_XBUTTONDOWN, WM_XBUTTONUP, WNDCLASSEXW, WS_OVERLAPPEDWINDOW, WS_POPUP, WS_VISIBLE,
};

use super::win32::{self, hwnd_from_key, hwnd_key, Icon};
use crate::frontend::common::launch::{self, Launch, SESSION_HEIGHT, SESSION_WIDTH};
use crate::frontend::common::session::{
    certificate_text, scale_menu, screenshot_path, SessionController, SessionState, Wake,
    CERTIFICATE_ANSWERS,
};
use crate::placement::{MonitorId, PlacementStore, WindowPlacement};
use crate::rdp::keymap::rdp_scancode_from_win32;
use crate::rdp::{
    BridgeEvent, CertificateDecision, CertificatePrompt, LocalClipboard, LockKeys, MonitorRect,
    MouseButton, RemoteClipboard,
};
use crate::session::keys::KeyCombo;
use crate::session::{PreparedSession, ScaleChoice};

/// Posted by the engine (from any thread) when events are queued.
const WM_SESSION_WAKE: u32 = WM_APP + 1;
const CLASS_NAME: PCWSTR = w!("WindowsAppSession");

const TIMER_RESIZE: usize = 1;
const TIMER_TITLE: usize = 2;
const RESIZE_DELAY: Duration = Duration::from_millis(300);
const TITLE_NOTE_DELAY: Duration = Duration::from_secs(4);

// System menu commands must be below 0xF000 with the low four bits clear.
const CMD_SEND_KEYS: usize = 0x1000;
const CMD_TYPE_CLIPBOARD: usize = 0x1100;
const CMD_SCALE: usize = 0x1200;
const CMD_SCALE_CUSTOM: usize = 0x1300;
const CMD_VIEW_ONLY: usize = 0x1400;
const CMD_FULLSCREEN: usize = 0x1410;
const CMD_SCREENSHOT: usize = 0x1420;
const CMD_NETWORK: usize = 0x1430;
const CMD_DISCONNECT: usize = 0x1440;
const CMD_STEP: usize = 0x10;

const MK_LBUTTON: usize = 0x0001;
const MK_RBUTTON: usize = 0x0002;
const MK_MBUTTON: usize = 0x0010;
const MK_XBUTTON1: usize = 0x0020;
const MK_XBUTTON2: usize = 0x0040;
const XBUTTON1: u16 = 1;

/// What the session windows tell the main window.
#[derive(Debug, Clone)]
pub enum Notice {
    /// The engine needs a Microsoft sign-in in a browser (session host authentication).
    AuthRequired { id: u64, name: String, url: String },
    /// The user chose "Custom…" in the display-scale menu.
    CustomScale { id: u64, name: String, current: u32 },
    /// The session window closed.
    Closed { name: String, message: String },
}

pub type Notifier = Rc<dyn Fn(Notice)>;

pub(crate) struct Fullscreen {
    style: isize,
    placement: WINDOWPLACEMENT,
    /// Monitors of a multi-monitor layout, relative to the window's own monitor.
    span: Option<Vec<MonitorRect>>,
}

pub(crate) struct Session {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) controller: SessionController,
    pub(crate) notify: Notifier,
    pub(crate) fullscreen: Option<Fullscreen>,
    pub(crate) dock_hwnd: std::cell::Cell<Option<HWND>>,
    /// Item count of the unmodified system menu; ours follow.
    pub(crate) base_menu_items: i32,
    pub(crate) scale_choices: Vec<ScaleChoice>,
    pub(crate) network: Option<String>,
    pub(crate) clipboard_sequence: u32,
    pub(crate) note: Option<String>,
    pub(crate) target: Option<crate::launcher::LaunchTarget>,
    pub(crate) show_hud: bool,
}

thread_local! {
    static SESSIONS: RefCell<HashMap<isize, Rc<RefCell<Session>>>> = RefCell::new(HashMap::new());
    static NEXT_ID: Cell<u64> = const { Cell::new(1) };
    static CLASS_REGISTERED: Cell<bool> = const { Cell::new(false) };
}

fn session_for(hwnd: HWND) -> Option<Rc<RefCell<Session>>> {
    SESSIONS.with_borrow(|sessions| sessions.get(&hwnd_key(hwnd)).cloned())
}

fn session_by_id(id: u64) -> Option<(HWND, Rc<RefCell<Session>>)> {
    SESSIONS.with_borrow(|sessions| {
        sessions
            .iter()
            .find(|(_, s)| s.borrow().id == id)
            .map(|(key, s)| (hwnd_from_key(*key), Rc::clone(s)))
    })
}

fn all_windows() -> Vec<HWND> {
    SESSIONS.with_borrow(|sessions| sessions.keys().map(|key| hwnd_from_key(*key)).collect())
}

/// Names of the sessions that are still connected or connecting.
pub fn active_names() -> Vec<String> {
    SESSIONS.with_borrow(|sessions| {
        sessions
            .values()
            .filter_map(|s| {
                let s = s.borrow();
                (!matches!(s.controller.state(), SessionState::Ended(_))).then(|| s.name.clone())
            })
            .collect()
    })
}

/// Disconnects every session and closes its window.
pub fn end_all() {
    for hwnd in all_windows() {
        if let Some(session) = session_for(hwnd) {
            session.borrow_mut().controller.end();
        }
        // SAFETY: the window belongs to this thread; WM_DESTROY removes it from the registry.
        if let Err(e) = unsafe { DestroyWindow(hwnd) } {
            log::warn!("Could not close a session window: {e}");
        }
    }
}

/// Hands the result of the session host sign-in to the engine (`None`: cancelled).
pub fn provide_auth(id: u64, redirect_url: Option<String>) {
    let Some((hwnd, session)) = session_by_id(id) else {
        return;
    };
    session
        .borrow()
        .controller
        .provide_auth_result(redirect_url.as_deref());
    if redirect_url.is_none() {
        set_note(hwnd, &session, "Sign-in cancelled");
    }
}

/// Applies a custom display scale chosen for session `id`.
pub fn set_custom_scale(id: u64, percent: u32) {
    let Some((hwnd, session)) = session_by_id(id) else {
        return;
    };
    session
        .borrow_mut()
        .controller
        .set_scale(ScaleChoice::percent(percent));
    resize_now(hwnd, &session);
}

/// Opens the window of a prepared session and starts connecting.
pub fn open(launch: Launch, prepared: PreparedSession, notify: Notifier) -> Result<(), String> {
    register_class()?;
    let name = launch.name.clone();
    let target = launch.target.clone();
    let saved_monitor = target
        .as_ref()
        .and_then(|t| PlacementStore::default().get(t).monitor)
        .and_then(|id| id.connector);
    let monitor = saved_monitor
        .and_then(|device| {
            monitors()
                .into_iter()
                .find(|m| m.device == device)
                .map(|m| m.handle)
        })
        .or_else(|| {
            win32::main_window()
                // SAFETY: MonitorFromWindow accepts any window handle.
                .map(|hwnd| unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) })
        });
    let work = monitor.and_then(|m| monitor_info(m).map(|info| info.work));

    let title = HSTRING::from(name.as_str());
    // SAFETY: creates a window of our registered class on this thread; all pointers
    // outlive the call.
    let hwnd = unsafe {
        let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            CLASS_NAME,
            &title,
            WS_OVERLAPPEDWINDOW,
            work.map_or(CW_USEDEFAULT, |r| r.left),
            work.map_or(CW_USEDEFAULT, |r| r.top),
            SESSION_WIDTH as i32,
            SESSION_HEIGHT as i32,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .map_err(|e| format!("Could not open the session window: {e}"))?
    };

    let key = hwnd_key(hwnd);
    let wake: Wake = Arc::new(move || {
        // SAFETY: posting to a window handle is allowed from any thread; a stale handle
        // only makes the call fail.
        let _ = unsafe {
            PostMessageW(
                Some(hwnd_from_key(key)),
                WM_SESSION_WAKE,
                WPARAM(0),
                LPARAM(0),
            )
        };
    });
    let controller = match SessionController::start(launch, prepared, wake) {
        Ok(controller) => controller,
        Err(e) => {
            // SAFETY: the window was created above on this thread.
            let _ = unsafe { DestroyWindow(hwnd) };
            return Err(e);
        }
    };
    launch::record_start(&controller.launch);
    let id = NEXT_ID.get();
    NEXT_ID.set(id + 1);
    // SAFETY: plain menu query on our window.
    let base_menu_items = unsafe { GetMenuItemCount(Some(GetSystemMenu(hwnd, false))) };
    let session = Rc::new(RefCell::new(Session {
        id,
        name,
        controller,
        notify,
        fullscreen: None,
        dock_hwnd: std::cell::Cell::new(None),
        base_menu_items,
        scale_choices: Vec::new(),
        network: None,
        clipboard_sequence: 0,
        note: None,
        target,
        show_hud: false,
    }));
    let dock = super::dock::create_dock(hwnd, Rc::clone(&session));
    session.borrow().dock_hwnd.set(Some(dock));
    SESSIONS.with_borrow_mut(|sessions| sessions.insert(key, Rc::clone(&session)));

    win32::apply_window_chrome(hwnd);
    win32::set_window_icon(hwnd);
    center_on_work_area(hwnd, work);
    rebuild_menu(hwnd, &session);
    update_title(hwnd, &session);
    // SAFETY: showing and focusing our own window.
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
    // Like the other frontends, sessions start in full screen on their monitor.
    set_fullscreen(hwnd, &session, true);
    Ok(())
}

fn register_class() -> Result<(), String> {
    if CLASS_REGISTERED.get() {
        return Ok(());
    }
    // SAFETY: registers a window class whose procedure is `window_proc`; the class name is
    // a static string.
    unsafe {
        let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            return Err(format!(
                "Could not register the session window: {}",
                windows::core::Error::from_thread()
            ));
        }
    }
    CLASS_REGISTERED.set(true);
    Ok(())
}

fn center_on_work_area(hwnd: HWND, work: Option<RECT>) {
    // SAFETY: plain queries and a move of our own window.
    unsafe {
        let scale = f64::from(GetDpiForWindow(hwnd)) / f64::from(USER_DEFAULT_SCREEN_DPI);
        let width = (f64::from(SESSION_WIDTH) * scale) as i32;
        let height = (f64::from(SESSION_HEIGHT) * scale) as i32;
        let (x, y) = match work {
            Some(r) => (
                r.left + ((r.right - r.left - width) / 2).max(0),
                r.top + ((r.bottom - r.top - height) / 2).max(0),
            ),
            None => (CW_USEDEFAULT, CW_USEDEFAULT),
        };
        let _ = SetWindowPos(
            hwnd,
            None,
            x,
            y,
            width,
            height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

// ----- Window procedure -----

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let Some(session) = session_for(hwnd) else {
        // SAFETY: default handling before registration and after removal.
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    };
    match handle_message(hwnd, &session, msg, wparam, lparam) {
        Some(result) => result,
        // SAFETY: default handling of everything we do not consume.
        None => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn handle_message(
    hwnd: HWND,
    session: &Rc<RefCell<Session>>,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> Option<LRESULT> {
    let handled = Some(LRESULT(0));
    match msg {
        WM_SESSION_WAKE => {
            drain(hwnd, session);
            handled
        }
        WM_PAINT => {
            paint(hwnd, session);
            handled
        }
        WM_ERASEBKGND => Some(LRESULT(1)),
        WM_SIZE => {
            if wparam.0 as u32 != SIZE_MINIMIZED {
                // SAFETY: (re)starts a timer on our window.
                unsafe {
                    SetTimer(
                        Some(hwnd),
                        TIMER_RESIZE,
                        RESIZE_DELAY.as_millis() as u32,
                        None,
                    )
                };
            }
            None
        }
        WM_TIMER => {
            // SAFETY: stops a timer of our window.
            let _ = unsafe { KillTimer(Some(hwnd), wparam.0) };
            match wparam.0 {
                TIMER_RESIZE => resize_now(hwnd, session),
                TIMER_TITLE => {
                    if let Ok(mut s) = session.try_borrow_mut() {
                        s.note = None;
                        drop(s);
                        update_title(hwnd, session);
                    }
                }
                _ => {}
            }
            handled
        }
        WM_GETMINMAXINFO => {
            // SAFETY: for WM_GETMINMAXINFO, lParam points to a MINMAXINFO.
            let info = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
            info.ptMinTrackSize = POINT { x: 480, y: 320 };
            handled
        }
        WM_DPICHANGED => {
            if session.borrow().fullscreen.is_none() {
                // SAFETY: for WM_DPICHANGED, lParam points to the suggested window RECT.
                let r = unsafe { *(lparam.0 as *const RECT) };
                // SAFETY: moving our own window.
                let _ = unsafe {
                    SetWindowPos(
                        hwnd,
                        None,
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    )
                };
            }
            handled
        }
        WM_SETFOCUS => {
            focus_in(hwnd, session);
            handled
        }
        WM_KILLFOCUS => {
            if let Ok(mut s) = session.try_borrow_mut() {
                s.controller.release_input();
            }
            handled
        }
        WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP => {
            key(hwnd, session, msg, wparam, lparam)
        }
        // Text input is sent as scancodes; swallow the translated characters while live.
        WM_CHAR | WM_SYSCHAR => is_live(session).then_some(LRESULT(0)),
        WM_MOUSEMOVE => {
            pointer_moved(hwnd, session, lparam);
            handled
        }
        WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MBUTTONDOWN
        | WM_MBUTTONUP | WM_XBUTTONDOWN | WM_XBUTTONUP => {
            mouse_button(hwnd, session, msg, wparam, lparam);
            // XBUTTON messages must return TRUE when processed.
            Some(LRESULT(isize::from(matches!(
                msg,
                WM_XBUTTONDOWN | WM_XBUTTONUP
            ))))
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let notches = f64::from(high_word(wparam.0) as i16) / 120.0;
            let mut s = session.borrow_mut();
            if msg == WM_MOUSEWHEEL {
                // Windows: positive = away from the user (up); the controller: positive = down.
                s.controller.wheel(0.0, -notches);
            } else {
                s.controller.wheel(notches, 0.0);
            }
            handled
        }
        WM_INITMENUPOPUP => {
            // HIWORD(lParam) is set for the window (system) menu.
            if high_word(lparam.0 as usize) != 0 {
                rebuild_menu(hwnd, session);
            }
            None
        }
        WM_SYSCOMMAND => {
            let command = wparam.0 & 0xFFF0;
            if command == SC_KEYMENU as usize && is_live(session) {
                // F10 / Alt alone go to the session, not to the menu.
                return handled;
            }
            run_command(hwnd, session, command).then_some(LRESULT(0))
        }
        WM_CLOSE => {
            close(hwnd, session);
            handled
        }
        WM_DESTROY => {
            destroyed(hwnd, session);
            handled
        }
        _ => None,
    }
}

fn high_word(value: usize) -> u16 {
    ((value >> 16) & 0xFFFF) as u16
}

fn point_from_lparam(lparam: LPARAM) -> (i32, i32) {
    let x = i32::from((lparam.0 & 0xFFFF) as u16 as i16);
    let y = i32::from(((lparam.0 >> 16) & 0xFFFF) as u16 as i16);
    (x, y)
}

fn is_live(session: &Rc<RefCell<Session>>) -> bool {
    session.borrow().controller.bridge().is_some()
}

fn client_size(hwnd: HWND) -> (i32, i32) {
    let mut rect = RECT::default();
    // SAFETY: `rect` is a valid out pointer.
    let _ = unsafe { GetClientRect(hwnd, &mut rect) };
    win32::rect_size(&rect)
}

pub(crate) fn dpi_scale(hwnd: HWND) -> f64 {
    // SAFETY: plain query on our window.
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 {
        1.0
    } else {
        f64::from(dpi) / f64::from(USER_DEFAULT_SCREEN_DPI)
    }
}

// ----- Engine events -----

fn drain(hwnd: HWND, session: &Rc<RefCell<Session>>) {
    let events = session.borrow_mut().controller.drain_events();
    for event in events {
        match event {
            BridgeEvent::FrameUpdated => {
                // SAFETY: invalidating our own window.
                let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
            }
            BridgeEvent::Status(_) => status_changed(hwnd, session),
            BridgeEvent::AuthRequired { url } => {
                let (id, name, notify) = {
                    let s = session.borrow();
                    (s.id, s.name.clone(), Rc::clone(&s.notify))
                };
                set_note(hwnd, session, "Signing in…");
                notify(Notice::AuthRequired { id, name, url });
            }
            BridgeEvent::RemoteClipboard(content) => remote_clipboard(hwnd, content),
            BridgeEvent::CertificatePrompt(prompt) => ask_certificate(hwnd, session, &prompt),
            BridgeEvent::Network(stats) => {
                session.borrow_mut().network = Some(format!("Connection: {}", stats.summary()));
            }
        }
    }
}

fn status_changed(hwnd: HWND, session: &Rc<RefCell<Session>>) {
    let state = session.borrow().controller.state().clone();
    match state {
        SessionState::Connecting(_) => {}
        SessionState::Connected => {
            // The initial size was a guess; match the window now.
            resize_now(hwnd, session);
            // SAFETY: plain focus query.
            if unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() } == hwnd {
                sync_local_clipboard(hwnd, session, true);
            }
        }
        SessionState::Ended(_) => {
            set_fullscreen(hwnd, session, false);
            // SAFETY: releasing the mouse capture of this thread.
            let _ = unsafe { ReleaseCapture() };
        }
    }
    update_title(hwnd, session);
    // SAFETY: invalidating our own window.
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

fn remote_clipboard(hwnd: HWND, content: RemoteClipboard) {
    let result = match content {
        RemoteClipboard::Text(text) => win32::set_clipboard_text(hwnd, &text),
        RemoteClipboard::Image(image) => {
            win32::set_clipboard_image(hwnd, image.width, image.height, &image.pixels)
        }
        RemoteClipboard::Files(files) => {
            log::info!(
                "The session copied {} file(s); copying files out of a session is not \
                 supported on Windows yet",
                files.len()
            );
            Ok(())
        }
    };
    if let Err(e) = result {
        log::warn!("Could not copy from the session: {e}");
    }
}

fn ask_certificate(hwnd: HWND, session: &Rc<RefCell<Session>>, prompt: &CertificatePrompt) {
    set_note(hwnd, session, "Verifying the certificate…");
    let text = certificate_text(prompt);
    let labels: Vec<&str> = CERTIFICATE_ANSWERS
        .iter()
        .map(|(_, label)| *label)
        .collect();
    // Default to "Connect Once" for unknown certificates, "Cancel" for changed ones.
    let default = if text.changed { 0 } else { 1 };
    let name = session.borrow().name.clone();
    let body = text.body();
    let choice = win32::choose(
        Some(hwnd),
        &name,
        text.heading,
        &body,
        &labels,
        default,
        text.changed,
    );
    let decision = choice
        .and_then(|i| CERTIFICATE_ANSWERS.get(i))
        .map_or(CertificateDecision::Reject, |(decision, _)| *decision);
    session.borrow().controller.answer_certificate(decision);
    if let Ok(mut s) = session.try_borrow_mut() {
        s.note = None;
    }
    update_title(hwnd, session);
}

// ----- Title and painting -----

fn update_title(hwnd: HWND, session: &Rc<RefCell<Session>>) {
    let title = {
        let Ok(s) = session.try_borrow() else {
            return;
        };
        let mut title = s.name.clone();
        let subtitle = match (&s.note, s.controller.state()) {
            (Some(note), _) => note.clone(),
            (None, SessionState::Connected) => String::new(),
            (None, state) => state.title_subtitle().to_string(),
        };
        if !subtitle.is_empty() {
            title.push_str(" – ");
            title.push_str(&subtitle);
        }
        if s.controller.view_only() {
            title.push_str(" (View Only)");
        }
        title
    };
    // SAFETY: the string outlives the call.
    let _ = unsafe { SetWindowTextW(hwnd, &HSTRING::from(title)) };
}

/// Shows `note` in the title for a few seconds.
fn set_note(hwnd: HWND, session: &Rc<RefCell<Session>>, note: &str) {
    session.borrow_mut().note = Some(note.to_string());
    update_title(hwnd, session);
    // SAFETY: (re)starts a timer on our window.
    unsafe {
        SetTimer(
            Some(hwnd),
            TIMER_TITLE,
            TITLE_NOTE_DELAY.as_millis() as u32,
            None,
        )
    };
}

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16))
}

fn paint(hwnd: HWND, session: &Rc<RefCell<Session>>) {
    let mut ps = PAINTSTRUCT::default();
    // SAFETY: BeginPaint/EndPaint pair on our window during WM_PAINT.
    let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
    let (width, height) = client_size(hwnd);
    if width <= 0 || height <= 0 {
        // SAFETY: matches BeginPaint above.
        let _ = unsafe { EndPaint(hwnd, &ps) };
        return;
    }

    // Double buffering: compose off-screen in memory DC, then BitBlt atomically to screen HDC.
    let mem_dc = unsafe { CreateCompatibleDC(Some(hdc)) };
    let mem_bm = unsafe { CreateCompatibleBitmap(hdc, width, height) };
    let use_backbuffer = !mem_dc.is_invalid() && !mem_bm.is_invalid();
    let target_dc = if use_backbuffer { mem_dc } else { hdc };
    let old_bm = if use_backbuffer {
        unsafe { Some(SelectObject(mem_dc, mem_bm.into())) }
    } else {
        None
    };

    let drawn = {
        let s = session.borrow();
        match s.controller.bridge() {
            Some(bridge) if s.controller.has_frame() => {
                bridge.with_frame(|frame| paint_frame(target_dc, width, height, frame))
            }
            _ => false,
        }
    };
    let show_hud = session.borrow().show_hud;
    if show_hud {
        let text = session.borrow().controller.diagnostics().format_hud();
        paint_hud(target_dc, width, &text);
    }

    if !drawn {
        let s = session.borrow();
        let (heading, detail) = match s.controller.state() {
            SessionState::Ended(message) => (
                "Session Ended".to_string(),
                format!(
                    "{}\n\nClose this window to return to {}.",
                    if message.is_empty() {
                        "Disconnected"
                    } else {
                        message
                    },
                    crate::frontend::common::text::APP_NAME
                ),
            ),
            state => (s.name.clone(), state.subtitle().to_string()),
        };
        paint_placeholder(hwnd, target_dc, width, height, &heading, &detail);
    }

    if use_backbuffer {
        let _ = unsafe { BitBlt(hdc, 0, 0, width, height, Some(mem_dc), 0, 0, SRCCOPY) };
        if let Some(old_bm) = old_bm {
            let _ = unsafe { SelectObject(mem_dc, old_bm) };
        }
        let _ = unsafe { DeleteObject(mem_bm.into()) };
        let _ = unsafe { DeleteDC(mem_dc) };
    }

    // SAFETY: matches BeginPaint above.
    let _ = unsafe { EndPaint(hwnd, &ps) };
}

fn paint_hud(hdc: HDC, width: i32, text: &str) {
    let hud_w = 260;
    let hud_h = 160;
    let margin = 16;
    let rect = RECT {
        left: width - hud_w - margin,
        top: margin,
        right: width - margin,
        bottom: margin + hud_h,
    };
    // SAFETY: standard GDI fill and text drawing into target_dc.
    unsafe {
        let brush = CreateSolidBrush(rgb(24, 24, 27));
        FillRect(hdc, &rect, brush);
        let _ = DeleteObject(brush.into());

        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, rgb(240, 240, 240));
        let mut wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let mut text_rect = RECT {
            left: rect.left + 8,
            top: rect.top + 8,
            right: rect.right - 8,
            bottom: rect.bottom - 8,
        };
        DrawTextW(hdc, &mut wide, &mut text_rect, DT_NOPREFIX);
    }
}

fn paint_frame(hdc: HDC, width: i32, height: i32, frame: &crate::rdp::Frame) -> bool {
    if frame.width == 0 || frame.height == 0 || frame.stride < frame.width * 4 {
        return false;
    }
    let needed = frame.stride as usize * (frame.height as usize - 1) + frame.width as usize * 4;
    if frame.pixels.len() < needed {
        return false;
    }
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            // Rows are `stride` bytes apart; the extra columns are cropped below.
            biWidth: (frame.stride / 4) as i32,
            biHeight: -(frame.height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    // SAFETY: the pixel buffer holds `height` rows of `stride` bytes as described by
    // `info`; the device context is valid during WM_PAINT.
    unsafe {
        SetStretchBltMode(hdc, COLORONCOLOR);
        StretchDIBits(
            hdc,
            0,
            0,
            width,
            height,
            0,
            0,
            frame.width as i32,
            frame.height as i32,
            Some(frame.pixels.as_ptr().cast()),
            &info,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
    }
    true
}

fn paint_placeholder(hwnd: HWND, hdc: HDC, width: i32, height: i32, heading: &str, detail: &str) {
    let dark = win32::dark_mode();
    let (background, foreground, secondary) = if dark {
        (rgb(32, 32, 32), rgb(255, 255, 255), rgb(200, 200, 200))
    } else {
        (rgb(243, 243, 243), rgb(26, 26, 26), rgb(96, 96, 96))
    };
    let scale = dpi_scale(hwnd);
    let client = RECT {
        left: 0,
        top: 0,
        right: width,
        bottom: height,
    };
    // SAFETY: GDI objects created here are selected out and deleted before returning; the
    // text buffers outlive the DrawTextW calls.
    unsafe {
        let brush = CreateSolidBrush(background);
        FillRect(hdc, &client, brush);
        let _ = DeleteObject(brush.into());
        SetBkMode(hdc, TRANSPARENT);

        let font = |size: f64, weight: u32| {
            CreateFontW(
                -((size * scale).round() as i32),
                0,
                0,
                0,
                weight as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                0,
                w!("Segoe UI Variable Display"),
            )
        };
        let title_font = font(28.0, FW_SEMIBOLD.0);
        let body_font = font(15.0, FW_NORMAL.0);
        let margin = (48.0 * scale) as i32;
        let format = DT_CENTER | DT_WORDBREAK | DT_NOPREFIX;

        let measure = |text: &str, font| {
            let mut rect = RECT {
                left: margin,
                top: 0,
                right: (width - margin).max(margin + 1),
                bottom: 0,
            };
            let mut buffer: Vec<u16> = text.encode_utf16().collect();
            let previous = SelectObject(hdc, font);
            DrawTextW(hdc, &mut buffer, &mut rect, format | DT_CALCRECT);
            SelectObject(hdc, previous);
            rect.bottom - rect.top
        };
        let gap = (12.0 * scale) as i32;
        let title_height = measure(heading, title_font.into());
        let body_height = measure(detail, body_font.into());
        let mut top = ((height - title_height - gap - body_height) / 2).max(0);

        let draw = |text: &str, font, color, top: i32, block: i32| {
            let mut rect = RECT {
                left: margin,
                top,
                right: (width - margin).max(margin + 1),
                bottom: top + block,
            };
            let mut buffer: Vec<u16> = text.encode_utf16().collect();
            let previous = SelectObject(hdc, font);
            SetTextColor(hdc, color);
            DrawTextW(hdc, &mut buffer, &mut rect, format);
            SelectObject(hdc, previous);
        };
        draw(heading, title_font.into(), foreground, top, title_height);
        top += title_height + gap;
        draw(detail, body_font.into(), secondary, top, body_height);
        let _ = DeleteObject(title_font.into());
        let _ = DeleteObject(body_font.into());
    }
}

// ----- Input -----

fn focus_in(hwnd: HWND, session: &Rc<RefCell<Session>>) {
    // SAFETY: plain key state queries.
    let toggled = |key: VIRTUAL_KEY| unsafe { GetKeyState(i32::from(key.0)) } & 1 != 0;
    let locks = LockKeys {
        scroll_lock: toggled(VK_SCROLL),
        num_lock: toggled(VK_NUMLOCK),
        caps_lock: toggled(VK_CAPITAL),
    };
    session.borrow().controller.focus_in(locks);
    sync_local_clipboard(hwnd, session, false);
}

/// Offers the local clipboard text to the session when it changed (or `force`).
fn sync_local_clipboard(hwnd: HWND, session: &Rc<RefCell<Session>>, force: bool) {
    let sequence = win32::clipboard_sequence();
    {
        let Ok(mut s) = session.try_borrow_mut() else {
            return;
        };
        if s.controller.bridge().is_none() || (!force && s.clipboard_sequence == sequence) {
            return;
        }
        s.clipboard_sequence = sequence;
    }
    if let Some(text) = win32::clipboard_text(hwnd).filter(|t| !t.is_empty()) {
        if let Ok(s) = session.try_borrow() {
            s.controller.set_local_clipboard(LocalClipboard::Text(text));
        }
    }
}

fn key_down(key: VIRTUAL_KEY) -> bool {
    // SAFETY: plain key state query.
    let state = unsafe { GetKeyState(i32::from(key.0)) };
    state < 0
}

fn key(
    hwnd: HWND,
    session: &Rc<RefCell<Session>>,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> Option<LRESULT> {
    if !is_live(session) {
        return None;
    }
    let down = matches!(msg, WM_KEYDOWN | WM_SYSKEYDOWN);
    if down
        && key_down(VK_CONTROL)
        && key_down(windows::Win32::UI::Input::KeyboardAndMouse::VK_SHIFT)
    {
        let vk = wparam.0 as u16;
        if vk == b'D' as u16 || vk == b'd' as u16 {
            let mut s = session.borrow_mut();
            s.controller.release_input();
            s.show_hud = !s.show_hud;
            drop(s);
            unsafe { InvalidateRect(hwnd, None, false) };
            return Some(LRESULT(0));
        }
    }
    if down && key_down(VK_CONTROL) && key_down(VK_MENU) {
        let vk = wparam.0 as u16;
        if vk == VK_RETURN.0 || vk == VK_HOME.0 {
            // Local shortcuts: release the modifiers already sent to the session.
            session.borrow_mut().controller.release_input();
            if vk == VK_RETURN.0 {
                let fullscreen = session.borrow().fullscreen.is_some();
                set_fullscreen(hwnd, session, !fullscreen);
            } else {
                show_menu(hwnd, session);
            }
            return Some(LRESULT(0));
        }
    }
    let scan = ((lparam.0 >> 16) & 0xFF) as u32;
    let extended = (lparam.0 >> 24) & 1 != 0;
    match rdp_scancode_from_win32(scan, extended) {
        Some(code) => {
            if let Ok(mut s) = session.try_borrow_mut() {
                s.controller.key(code, down);
            }
        }
        None => log::debug!("No scancode for key {:#x} (scan {scan:#x})", wparam.0),
    }
    Some(LRESULT(0))
}

fn pointer_moved(hwnd: HWND, session: &Rc<RefCell<Session>>, lparam: LPARAM) {
    let (x, y) = point_from_lparam(lparam);
    let (width, height) = client_size(hwnd);
    if width <= 0 || height <= 0 {
        return;
    }
    if session.borrow().fullscreen.is_some() {
        if let Some(dock) = session.borrow().dock_hwnd.get() {
            super::dock::on_parent_pointer_motion(dock, x, y, width);
        }
    }
    if let Ok(mut s) = session.try_borrow_mut() {
        s.controller.pointer_moved(
            f64::from(x),
            f64::from(y),
            (f64::from(width), f64::from(height)),
        );
    }
}

fn mouse_button(
    hwnd: HWND,
    session: &Rc<RefCell<Session>>,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) {
    let (button, down) = match msg {
        WM_LBUTTONDOWN => (MouseButton::Left, true),
        WM_LBUTTONUP => (MouseButton::Left, false),
        WM_RBUTTONDOWN => (MouseButton::Right, true),
        WM_RBUTTONUP => (MouseButton::Right, false),
        WM_MBUTTONDOWN => (MouseButton::Middle, true),
        WM_MBUTTONUP => (MouseButton::Middle, false),
        _ => {
            let button = if high_word(wparam.0) == XBUTTON1 {
                MouseButton::X1
            } else {
                MouseButton::X2
            };
            (button, msg == WM_XBUTTONDOWN)
        }
    };
    pointer_moved(hwnd, session, lparam);
    if down {
        // SAFETY: capturing the mouse for our own window while a button is held.
        unsafe {
            SetCapture(hwnd);
            let _ = SetFocus(Some(hwnd));
        }
    } else if wparam.0 & (MK_LBUTTON | MK_RBUTTON | MK_MBUTTON | MK_XBUTTON1 | MK_XBUTTON2) == 0 {
        // SAFETY: releases the capture taken above.
        let _ = unsafe { ReleaseCapture() };
    }
    if let Ok(mut s) = session.try_borrow_mut() {
        s.controller.button(button, down);
    }
}

// ----- Size, full screen and monitors -----

struct Monitor {
    handle: HMONITOR,
    rect: RECT,
    work: RECT,
    device: String,
}

fn monitor_info(handle: HMONITOR) -> Option<Monitor> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    // SAFETY: MONITORINFOEXW starts with MONITORINFO and cbSize announces the larger type.
    let ok =
        unsafe { GetMonitorInfoW(handle, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) };
    if !ok.as_bool() {
        return None;
    }
    let len = info
        .szDevice
        .iter()
        .position(|c| *c == 0)
        .unwrap_or(info.szDevice.len());
    Some(Monitor {
        handle,
        rect: info.monitorInfo.rcMonitor,
        work: info.monitorInfo.rcWork,
        device: String::from_utf16_lossy(&info.szDevice[..len]),
    })
}

fn monitors() -> Vec<Monitor> {
    unsafe extern "system" fn collect(
        handle: HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        data: LPARAM,
    ) -> windows::core::BOOL {
        // SAFETY: `data` is the Vec passed by `monitors` below, alive during enumeration.
        let found = unsafe { &mut *(data.0 as *mut Vec<HMONITOR>) };
        found.push(handle);
        windows::core::BOOL(1)
    }
    let mut handles: Vec<HMONITOR> = Vec::new();
    // SAFETY: the callback only writes to `handles`, which outlives the call.
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(collect),
            LPARAM(&mut handles as *mut Vec<HMONITOR> as isize),
        );
    }
    handles.into_iter().filter_map(monitor_info).collect()
}

fn window_monitor(hwnd: HWND) -> Option<Monitor> {
    // SAFETY: MonitorFromWindow accepts any window handle.
    monitor_info(unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTOPRIMARY) })
}

pub(crate) fn resize_now(hwnd: HWND, session: &Rc<RefCell<Session>>) {
    let scale = dpi_scale(hwnd);
    let s = session.borrow();
    if let Some(span) = s.fullscreen.as_ref().and_then(|f| f.span.as_ref()) {
        s.controller.request_layout(span, scale);
        return;
    }
    let (width, height) = client_size(hwnd);
    if width > 0 && height > 0 {
        s.controller.resize((width as u32, height as u32), scale);
    }
}

pub(crate) fn set_fullscreen(hwnd: HWND, session: &Rc<RefCell<Session>>, on: bool) {
    if on == session.borrow().fullscreen.is_some() {
        return;
    }
    if on {
        let Some(monitor) = window_monitor(hwnd) else {
            return;
        };
        let mut placement = WINDOWPLACEMENT {
            length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        // SAFETY: plain queries on our own window.
        let style = unsafe {
            let _ = GetWindowPlacement(hwnd, &mut placement);
            GetWindowLongPtrW(hwnd, GWL_STYLE)
        };
        let multi = session.borrow().controller.launch.multi_monitor;
        // SAFETY: plain system metric query.
        let monitor_count = unsafe { GetSystemMetrics(SM_CMONITORS) };
        let (rect, span) = if multi && monitor_count > 1 {
            span_layout(&monitor)
        } else {
            (monitor.rect, None)
        };
        session.borrow_mut().fullscreen = Some(Fullscreen {
            style,
            placement,
            span,
        });
        let popup = (style & !(WS_OVERLAPPEDWINDOW.0 as isize)) | WS_POPUP.0 as isize;
        let (width, height) = win32::rect_size(&rect);
        // SAFETY: restyling and moving our own window.
        unsafe {
            SetWindowLongPtrW(hwnd, GWL_STYLE, popup | WS_VISIBLE.0 as isize);
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOP),
                rect.left,
                rect.top,
                width,
                height,
                SWP_FRAMECHANGED | SWP_NOOWNERZORDER,
            );
        }
        save_placement(session, &monitor.device);
        if let Some(dock) = session.borrow().dock_hwnd.get() {
            super::dock::update_dock_position(dock, hwnd, &rect, true);
        }
    } else {
        let Some(saved) = session.borrow_mut().fullscreen.take() else {
            return;
        };
        // SAFETY: restoring the style and placement saved above.
        unsafe {
            SetWindowLongPtrW(hwnd, GWL_STYLE, saved.style);
            let _ = SetWindowPlacement(hwnd, &saved.placement);
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOOWNERZORDER,
            );
        }
        if let Some(dock) = session.borrow().dock_hwnd.get() {
            super::dock::update_dock_position(dock, hwnd, &RECT::default(), false);
        }
    }
    update_title(hwnd, session);
    // SAFETY: (re)starts the resize timer of our window.
    unsafe {
        SetTimer(
            Some(hwnd),
            TIMER_RESIZE,
            RESIZE_DELAY.as_millis() as u32,
            None,
        )
    };
}

/// The virtual screen and the monitor layout relative to `primary` (the window's monitor,
/// which the host shows at its origin).
fn span_layout(primary: &Monitor) -> (RECT, Option<Vec<MonitorRect>>) {
    // SAFETY: plain system metric queries.
    let virtual_screen = unsafe {
        let left = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let top = GetSystemMetrics(SM_YVIRTUALSCREEN);
        RECT {
            left,
            top,
            right: left + GetSystemMetrics(SM_CXVIRTUALSCREEN),
            bottom: top + GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    };
    let mut layout: Vec<MonitorRect> = monitors()
        .iter()
        .map(|m| {
            let (width, height) = win32::rect_size(&m.rect);
            MonitorRect {
                x: m.rect.left - primary.rect.left,
                y: m.rect.top - primary.rect.top,
                width: width.max(0) as u32,
                height: height.max(0) as u32,
                primary: m.device == primary.device,
            }
        })
        .collect();
    // The primary monitor first, as hosts expect.
    layout.sort_by_key(|m| !m.primary);
    (virtual_screen, Some(layout))
}

fn save_placement(session: &Rc<RefCell<Session>>, device: &str) {
    let Some(target) = session.borrow().target.clone() else {
        return;
    };
    let placement = WindowPlacement {
        monitor: Some(MonitorId {
            connector: Some(device.to_string()),
            ..Default::default()
        }),
        workspace: None,
    };
    if let Err(e) = PlacementStore::default().set(&target, placement) {
        log::warn!("Could not remember the session window placement: {e}");
    }
}

// ----- Session menu -----

fn append(
    menu: HMENU,
    flags: windows::Win32::UI::WindowsAndMessaging::MENU_ITEM_FLAGS,
    id: usize,
    label: &str,
) {
    let label = HSTRING::from(label);
    // SAFETY: the menu belongs to our window; the label outlives the call (it is copied).
    if let Err(e) = unsafe { AppendMenuW(menu, flags, id, &label) } {
        log::debug!("Could not add a menu item: {e}");
    }
}

/// Replaces our part of the system menu with the current state.
pub(crate) fn rebuild_menu(hwnd: HWND, session: &Rc<RefCell<Session>>) {
    // SAFETY: edits the system menu of our own window; removed submenus are destroyed by
    // DeleteMenu and new ones are owned by the menu they are appended to.
    unsafe {
        let menu = GetSystemMenu(hwnd, false);
        let base = session.borrow().base_menu_items;
        while GetMenuItemCount(Some(menu)) > base {
            if DeleteMenu(menu, base as u32, MF_BYPOSITION).is_err() {
                break;
            }
        }
        let s = &mut *session.borrow_mut();
        let live = s.controller.bridge().is_some();
        let enabled = |on: bool| if on { MF_STRING } else { MF_STRING | MF_GRAYED };

        append(menu, MF_SEPARATOR, 0, "");
        if let Ok(keys) = CreatePopupMenu() {
            for (i, combo) in KeyCombo::ALL.iter().enumerate() {
                append(keys, MF_STRING, CMD_SEND_KEYS + i * CMD_STEP, combo.label());
            }
            let flags = if live { MF_POPUP } else { MF_POPUP | MF_GRAYED };
            append(menu, flags, keys.0 as usize, "Send Keys");
        }
        append(
            menu,
            enabled(live),
            CMD_TYPE_CLIPBOARD,
            "Type Clipboard Text",
        );
        if let Ok(scales) = CreatePopupMenu() {
            let entries = scale_menu(s.controller.scale());
            s.scale_choices = entries.iter().map(|e| e.choice).collect();
            for (i, entry) in entries.iter().enumerate() {
                let flags = if entry.checked {
                    MF_STRING | MF_CHECKED
                } else {
                    MF_STRING
                };
                append(scales, flags, CMD_SCALE + i * CMD_STEP, &entry.label);
            }
            append(scales, MF_SEPARATOR, 0, "");
            append(scales, MF_STRING, CMD_SCALE_CUSTOM, "Custom…");
            let flags = if live && s.controller.launch.dynamic_resolution {
                MF_POPUP
            } else {
                MF_POPUP | MF_GRAYED
            };
            append(menu, flags, scales.0 as usize, "Display Scale");
        }
        let checked = |on: bool| {
            if on {
                MF_STRING | MF_CHECKED
            } else {
                MF_STRING
            }
        };
        append(
            menu,
            checked(s.controller.view_only()) | enabled(live),
            CMD_VIEW_ONLY,
            "View Only",
        );
        append(
            menu,
            checked(s.fullscreen.is_some()) | enabled(live),
            CMD_FULLSCREEN,
            "Full Screen\tCtrl+Alt+Enter",
        );
        append(
            menu,
            enabled(live && s.controller.has_frame()),
            CMD_SCREENSHOT,
            "Save Screenshot",
        );
        if let Some(network) = &s.network {
            if live {
                append(menu, MF_STRING | MF_GRAYED, CMD_NETWORK, network);
            }
        }
        append(menu, MF_SEPARATOR, 0, "");
        append(menu, enabled(live), CMD_DISCONNECT, "Disconnect");
    }
}

/// Shows the session menu at the window's top-left corner (Ctrl+Alt+Home).
fn show_menu(hwnd: HWND, session: &Rc<RefCell<Session>>) {
    rebuild_menu(hwnd, session);
    let mut origin = POINT::default();
    // SAFETY: converts a point of our window and tracks its system menu; the command is
    // returned instead of posted.
    let command = unsafe {
        let _ = windows::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut origin);
        TrackPopupMenu(
            GetSystemMenu(hwnd, false),
            TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN,
            origin.x,
            origin.y,
            None,
            hwnd,
            None,
        )
    };
    let command = command.0 as usize;
    if command != 0 && !run_command(hwnd, session, command) {
        // A standard system command (move, size, close...).
        // SAFETY: posting a system command to our own window.
        let _ = unsafe { PostMessageW(Some(hwnd), WM_SYSCOMMAND, WPARAM(command), LPARAM(0)) };
    }
}

/// Runs a session menu command; `false` for commands that are not ours.
pub(crate) fn run_command(hwnd: HWND, session: &Rc<RefCell<Session>>, command: usize) -> bool {
    match command {
        c if (CMD_SEND_KEYS..CMD_SEND_KEYS + KeyCombo::ALL.len() * CMD_STEP).contains(&c) => {
            let combo = KeyCombo::ALL[(c - CMD_SEND_KEYS) / CMD_STEP];
            session.borrow().controller.send_combo(combo);
        }
        CMD_TYPE_CLIPBOARD => type_clipboard(hwnd, session),
        CMD_SCALE_CUSTOM => {
            let notice = {
                let s = session.borrow();
                Notice::CustomScale {
                    id: s.id,
                    name: s.name.clone(),
                    current: s.controller.scale().resolve(dpi_scale(hwnd)),
                }
            };
            let notify = Rc::clone(&session.borrow().notify);
            notify(notice);
        }
        c if (CMD_SCALE..CMD_SCALE_CUSTOM).contains(&c) => {
            let choice = session
                .borrow()
                .scale_choices
                .get((c - CMD_SCALE) / CMD_STEP)
                .copied();
            if let Some(choice) = choice {
                session.borrow_mut().controller.set_scale(choice);
                resize_now(hwnd, session);
            }
        }
        CMD_VIEW_ONLY => {
            {
                let mut s = session.borrow_mut();
                let view_only = !s.controller.view_only();
                s.controller.set_view_only(view_only);
            }
            update_title(hwnd, session);
        }
        CMD_FULLSCREEN => {
            let fullscreen = session.borrow().fullscreen.is_some();
            set_fullscreen(hwnd, session, !fullscreen);
        }
        CMD_SCREENSHOT => screenshot(hwnd, session),
        CMD_NETWORK => {}
        CMD_DISCONNECT => {
            // SAFETY: posting a close request to our own window.
            let _ = unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        }
        _ => return false,
    }
    true
}

pub(crate) fn type_clipboard(hwnd: HWND, session: &Rc<RefCell<Session>>) {
    let Some(text) = win32::clipboard_text(hwnd).filter(|t| !t.is_empty()) else {
        win32::message(
            Some(hwnd),
            "Type Clipboard Text",
            "The clipboard holds no text.",
            Icon::Information,
        );
        return;
    };
    let result = session.borrow().controller.type_text(&text);
    if let Err(e) = result {
        win32::message(Some(hwnd), "Type Clipboard Text", &e, Icon::Warning);
    }
}

pub(crate) fn screenshot(hwnd: HWND, session: &Rc<RefCell<Session>>) {
    let (path, result) = {
        let s = session.borrow();
        let path = screenshot_path(&s.name).with_extension("bmp");
        let result = match s.controller.bridge() {
            Some(bridge) => bridge.with_frame(|frame| {
                win32::write_bmp(
                    &path,
                    frame.width,
                    frame.height,
                    frame.stride,
                    &frame.pixels,
                )
            }),
            None => Err(std::io::Error::other("the session has ended")),
        };
        (path, result)
    };
    match result {
        Ok(()) => {
            let file = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            set_note(hwnd, session, &format!("Screenshot saved as {file}"));
        }
        Err(e) => {
            log::warn!("Could not save the screenshot to {}: {e}", path.display());
            win32::message(
                Some(hwnd),
                "Save Screenshot",
                &format!("Could not save the screenshot: {e}"),
                Icon::Error,
            );
        }
    }
}

// ----- Closing -----

fn close(hwnd: HWND, session: &Rc<RefCell<Session>>) {
    let device = if session.borrow().fullscreen.is_some() {
        window_monitor(hwnd).map(|m| m.device)
    } else {
        None
    };
    if let Some(device) = device {
        save_placement(session, &device);
    }
    session.borrow_mut().controller.end();
    // SAFETY: destroying our own window; WM_DESTROY cleans up.
    if let Err(e) = unsafe { DestroyWindow(hwnd) } {
        log::warn!("Could not close the session window: {e}");
    }
}

fn destroyed(hwnd: HWND, session: &Rc<RefCell<Session>>) {
    if let Some(dock) = session.borrow().dock_hwnd.get() {
        let _ = unsafe { DestroyWindow(dock) };
    }
    SESSIONS.with_borrow_mut(|sessions| sessions.remove(&hwnd_key(hwnd)));
    // SAFETY: stops the timers of the window being destroyed.
    unsafe {
        let _ = KillTimer(Some(hwnd), TIMER_RESIZE);
        let _ = KillTimer(Some(hwnd), TIMER_TITLE);
    }
    let (notify, name, message) = {
        let mut s = session.borrow_mut();
        s.controller.end();
        let message = s.controller.state().subtitle().to_string();
        (Rc::clone(&s.notify), s.name.clone(), message)
    };
    notify(Notice::Closed { name, message });
}
