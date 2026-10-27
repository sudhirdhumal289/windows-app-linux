//! Connection bar / top dock shown over a fullscreen session window on Windows (WinUI/Win32).
//!
//! Replicates the Linux GTK fullscreen dock using native Win32 windowing and GDI:
//! slides down when the pointer reaches the top sliver, stays open while hovered or
//! while a menu is open, and slides back up shortly after the pointer leaves.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use super::session_window::{self, Session};
use crate::frontend::common::session::scale_menu;
use crate::session::keys::KeyCombo;

const DOCK_CLASS: PCWSTR = w!("WindowsAppFullscreenDock");
const TIMER_HIDE: usize = 101;
const HIDE_DELAY_MS: u32 = 600;
const USER_DEFAULT_SCREEN_DPI: u32 = 96;
const WM_MOUSELEAVE: u32 = 0x02A3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockButton {
    Mic,
    Volume,
    Scale,
    Actions,
    Restore,
    Minimize,
    Disconnect,
}

pub struct DockState {
    pub parent_hwnd: HWND,
    pub session: Rc<RefCell<Session>>,
    pub revealed: bool,
    pub menu_open: bool,
    pub hovered: Option<DockButton>,
    pub monitor_rect: RECT,
}

thread_local! {
    static DOCKS: RefCell<HashMap<isize, Rc<RefCell<DockState>>>> = RefCell::new(HashMap::new());
}

fn dock_key(hwnd: HWND) -> isize {
    hwnd.0 as isize
}

fn dock_for(hwnd: HWND) -> Option<Rc<RefCell<DockState>>> {
    DOCKS.with_borrow(|docks| docks.get(&dock_key(hwnd)).cloned())
}

fn register_dock_class() {
    static REGISTERED: std::sync::Once = std::sync::Once::new();
    REGISTERED.call_once(|| {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(dock_wndproc),
            lpszClassName: DOCK_CLASS,
            // SAFETY: System arrow cursor.
            hCursor: unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() },
            ..Default::default()
        };
        // SAFETY: Registering unique window class for this process.
        unsafe {
            let _ = RegisterClassExW(&wc);
        }
    });
}

/// Creates a new fullscreen dock window for `parent_hwnd`.
pub fn create_dock(parent_hwnd: HWND, session: Rc<RefCell<Session>>) -> HWND {
    register_dock_class();

    // SAFETY: Creating a tool window popup owned by parent_hwnd.
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            DOCK_CLASS,
            w!("Connection Bar"),
            WS_POPUP | WS_CLIPSIBLINGS,
            0,
            -100,
            640,
            36,
            Some(parent_hwnd),
            None,
            None,
            None,
        )
        .unwrap_or_default()
    };

    let state = Rc::new(RefCell::new(DockState {
        parent_hwnd,
        session,
        revealed: false,
        menu_open: false,
        hovered: None,
        monitor_rect: RECT::default(),
    }));

    DOCKS.with_borrow_mut(|docks| docks.insert(dock_key(hwnd), state));
    hwnd
}

fn dock_dpi_scale(hwnd: HWND) -> f64 {
    // SAFETY: Querying DPI for our own dock window.
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 {
        1.0
    } else {
        f64::from(dpi) / f64::from(USER_DEFAULT_SCREEN_DPI)
    }
}

/// Updates dock positioning and visibility when entering or leaving fullscreen.
pub fn update_dock_position(
    dock_hwnd: HWND,
    _parent_hwnd: HWND,
    monitor_rect: &RECT,
    visible: bool,
) {
    if !visible {
        // SAFETY: Hiding the dock window.
        unsafe {
            let _ = KillTimer(Some(dock_hwnd), TIMER_HIDE);
            let _ = ShowWindow(dock_hwnd, SW_HIDE);
        }
        return;
    }

    let scale = dock_dpi_scale(dock_hwnd);
    let width = (640.0 * scale) as i32;
    let height = (36.0 * scale) as i32;
    let peek_h = (6.0 * scale).round().max(4.0) as i32;

    let mon_w = monitor_rect.right - monitor_rect.left;
    let x = monitor_rect.left + (mon_w - width) / 2;
    let collapsed_y = monitor_rect.top - height + peek_h;

    if let Some(state) = dock_for(dock_hwnd) {
        let mut s = state.borrow_mut();
        s.monitor_rect = *monitor_rect;
        s.revealed = false;
        s.menu_open = false;
        s.hovered = None;
    }

    // Set rounded shape for modern floating appearance with flat top edge and curved bottom corners.
    // SAFETY: Region created and transferred to window.
    unsafe {
        let rgn = CreateRoundRectRgn(
            0,
            -(4.0 * scale) as i32,
            width + 1,
            height + 1,
            (10.0 * scale) as i32,
            (10.0 * scale) as i32,
        );
        let _ = SetWindowRgn(dock_hwnd, Some(rgn), true);
        let _ = SetWindowPos(
            dock_hwnd,
            Some(HWND_TOPMOST),
            x,
            collapsed_y,
            width,
            height,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
}

/// Called from parent session window pointer motion to reveal dock or schedule auto-hide.
pub fn on_parent_pointer_motion(dock_hwnd: HWND, x: i32, y: i32, win_w: i32) {
    let Some(state) = dock_for(dock_hwnd) else {
        return;
    };
    let scale = dock_dpi_scale(dock_hwnd);
    let half_w = win_w / 2;
    let trigger_zone = (win_w / 3).max((240.0 * scale) as i32);
    let at_top_centre = y <= (8.0 * scale) as i32 && (x - half_w).abs() <= trigger_zone;

    if at_top_centre {
        let _ = unsafe { KillTimer(Some(dock_hwnd), TIMER_HIDE) };
        set_revealed(dock_hwnd, true);
    } else {
        let s = state.borrow();
        if s.revealed && !s.menu_open {
            let mut win_r = RECT::default();
            let _ = unsafe { GetWindowRect(dock_hwnd, &mut win_r) };
            let height = win_r.bottom - win_r.top;
            let slack = (24.0 * scale) as i32;
            if y > height + slack {
                drop(s);
                let _ = unsafe { SetTimer(Some(dock_hwnd), TIMER_HIDE, HIDE_DELAY_MS, None) };
            }
        }
    }
}

fn set_revealed(dock_hwnd: HWND, reveal: bool) {
    let Some(state) = dock_for(dock_hwnd) else {
        return;
    };
    let mut s = state.borrow_mut();
    if s.revealed == reveal {
        return;
    }
    s.revealed = reveal;
    if !reveal {
        s.hovered = None;
    }

    let scale = dock_dpi_scale(dock_hwnd);
    let mut win_r = RECT::default();
    let _ = unsafe { GetWindowRect(dock_hwnd, &mut win_r) };
    let width = win_r.right - win_r.left;
    let height = win_r.bottom - win_r.top;
    let peek_h = (6.0 * scale).round().max(4.0) as i32;

    let target_y = if reveal {
        s.monitor_rect.top
    } else {
        s.monitor_rect.top - height + peek_h
    };

    unsafe {
        let _ = SetWindowPos(
            dock_hwnd,
            Some(HWND_TOPMOST),
            win_r.left,
            target_y,
            width,
            height,
            SWP_NOACTIVATE,
        );
        let _ = InvalidateRect(Some(dock_hwnd), None, false);
    }
}

fn button_rect(btn: DockButton, width: i32, height: i32, scale: f64) -> RECT {
    let top = (4.0 * scale) as i32;
    let bottom = height - (7.0 * scale) as i32;
    match btn {
        DockButton::Mic => RECT {
            left: width - (482.0 * scale) as i32,
            top,
            right: width - (416.0 * scale) as i32,
            bottom,
        },
        DockButton::Volume => RECT {
            left: width - (412.0 * scale) as i32,
            top,
            right: width - (324.0 * scale) as i32,
            bottom,
        },
        DockButton::Scale => RECT {
            left: width - (320.0 * scale) as i32,
            top,
            right: width - (234.0 * scale) as i32,
            bottom,
        },
        DockButton::Actions => RECT {
            left: width - (230.0 * scale) as i32,
            top,
            right: width - (134.0 * scale) as i32,
            bottom,
        },
        DockButton::Restore => RECT {
            left: width - (126.0 * scale) as i32,
            top,
            right: width - (90.0 * scale) as i32,
            bottom,
        },
        DockButton::Minimize => RECT {
            left: width - (86.0 * scale) as i32,
            top,
            right: width - (50.0 * scale) as i32,
            bottom,
        },
        DockButton::Disconnect => RECT {
            left: width - (46.0 * scale) as i32,
            top,
            right: width - (8.0 * scale) as i32,
            bottom,
        },
    }
}

const ALL_BUTTONS: [DockButton; 7] = [
    DockButton::Mic,
    DockButton::Volume,
    DockButton::Scale,
    DockButton::Actions,
    DockButton::Restore,
    DockButton::Minimize,
    DockButton::Disconnect,
];

fn hit_test(x: i32, y: i32, width: i32, height: i32, scale: f64) -> Option<DockButton> {
    for &btn in &ALL_BUTTONS {
        let r = button_rect(btn, width, height, scale);
        if x >= r.left && x < r.right && y >= r.top && y < r.bottom {
            return Some(btn);
        }
    }
    None
}

unsafe extern "system" fn dock_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            paint_dock(hwnd);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let mut client = RECT::default();
            // SAFETY: Querying client area.
            let _ = unsafe { GetClientRect(hwnd, &mut client) };
            let width = client.right - client.left;
            let height = client.bottom - client.top;
            let scale = dock_dpi_scale(hwnd);

            // Cancel pending hide and reveal dock if collapsed.
            let _ = unsafe { KillTimer(Some(hwnd), TIMER_HIDE) };
            set_revealed(hwnd, true);

            if let Some(state) = dock_for(hwnd) {
                let mut s = state.borrow_mut();
                let new_hovered = hit_test(x, y, width, height, scale);
                if new_hovered != s.hovered {
                    s.hovered = new_hovered;
                    // SAFETY: Invalidate dock for redraw.
                    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
                }
            }

            // Request WM_MOUSELEAVE.
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            // SAFETY: Tracking mouse leaving dock.
            let _ = unsafe { TrackMouseEvent(&mut tme) };
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            if let Some(state) = dock_for(hwnd) {
                let mut s = state.borrow_mut();
                s.hovered = None;
                let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
                if !s.menu_open {
                    // SAFETY: Start auto-hide timer.
                    let _ = unsafe { SetTimer(Some(hwnd), TIMER_HIDE, HIDE_DELAY_MS, None) };
                }
            }
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 == TIMER_HIDE {
                // SAFETY: Stop hide timer.
                let _ = unsafe { KillTimer(Some(hwnd), TIMER_HIDE) };
                if let Some(state) = dock_for(hwnd) {
                    let menu_open = state.borrow().menu_open;
                    if !menu_open {
                        // Check cursor position before hiding:
                        let mut pt = POINT::default();
                        let _ = unsafe { GetCursorPos(&mut pt) };
                        let mut win_r = RECT::default();
                        let _ = unsafe { GetWindowRect(hwnd, &mut win_r) };
                        let scale = dock_dpi_scale(hwnd);
                        let slack = (24.0 * scale) as i32;
                        let mon_y = state.borrow().monitor_rect.top;
                        let over_dock = pt.x >= win_r.left - slack
                            && pt.x <= win_r.right + slack
                            && pt.y >= mon_y
                            && pt.y <= win_r.bottom + slack;
                        if over_dock {
                            // Still near dock; reschedule hide.
                            let _ =
                                unsafe { SetTimer(Some(hwnd), TIMER_HIDE, HIDE_DELAY_MS, None) };
                        } else {
                            set_revealed(hwnd, false);
                        }
                    }
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let mut client = RECT::default();
            let _ = unsafe { GetClientRect(hwnd, &mut client) };
            let width = client.right - client.left;
            let height = client.bottom - client.top;
            let scale = dock_dpi_scale(hwnd);

            let btn = hit_test(x, y, width, height, scale);
            if let Some(btn) = btn {
                handle_button_click(hwnd, btn, scale);
            } else {
                // Dragging dock along horizontal top edge.
                // SAFETY: Send WM_NCLBUTTONDOWN with HTCAPTION for smooth native drag.
                unsafe {
                    let _ = ReleaseCapture();
                    let _ = PostMessageW(Some(hwnd), WM_NCLBUTTONDOWN, WPARAM(2), LPARAM(0));
                }
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            DOCKS.with_borrow_mut(|docks| docks.remove(&dock_key(hwnd)));
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn handle_button_click(hwnd: HWND, btn: DockButton, scale: f64) {
    let Some(state) = dock_for(hwnd) else { return };
    let (parent_hwnd, session) = {
        let s = state.borrow();
        (s.parent_hwnd, Rc::clone(&s.session))
    };

    match btn {
        DockButton::Mic => {
            let _ = crate::platform::audio::toggle_mic_muted();
            let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
        }
        DockButton::Volume => {
            show_volume_menu(hwnd, parent_hwnd, &session, scale);
        }
        DockButton::Scale => {
            show_scale_menu(hwnd, parent_hwnd, &session, scale);
        }
        DockButton::Actions => {
            show_actions_menu(hwnd, parent_hwnd, &session, scale);
        }
        DockButton::Restore => {
            // Exit fullscreen!
            session_window::set_fullscreen(parent_hwnd, &session, false);
        }
        DockButton::Minimize => {
            // Minimize session window!
            // SAFETY: Standard window minimize.
            let _ = unsafe { ShowWindow(parent_hwnd, SW_MINIMIZE) };
        }
        DockButton::Disconnect => {
            // End session!
            session.borrow_mut().controller.end();
        }
    }
}

fn show_volume_menu(
    dock_hwnd: HWND,
    _parent_hwnd: HWND,
    _session: &Rc<RefCell<Session>>,
    scale: f64,
) {
    if let Some(state) = dock_for(dock_hwnd) {
        state.borrow_mut().menu_open = true;
    }
    let mut client = RECT::default();
    let _ = unsafe { GetClientRect(dock_hwnd, &mut client) };
    let r = button_rect(
        DockButton::Volume,
        client.right - client.left,
        client.bottom - client.top,
        scale,
    );
    let mut pt = POINT {
        x: r.left,
        y: r.bottom,
    };
    let _ = unsafe { ClientToScreen(dock_hwnd, &mut pt) };

    unsafe {
        let menu = match CreatePopupMenu() {
            Ok(m) => m,
            Err(_) => {
                if let Some(state) = dock_for(dock_hwnd) {
                    state.borrow_mut().menu_open = false;
                }
                return;
            }
        };

        let current_vol = crate::platform::audio::get_volume();
        let levels = [
            (100, "100%"),
            (75, "75%"),
            (50, "50%"),
            (25, "25%"),
            (0, "Mute (0%)"),
        ];
        for (vol, label) in levels {
            let mut flags = MF_STRING;
            if current_vol == vol || (vol > 0 && current_vol.abs_diff(vol) < 13) {
                flags |= MF_CHECKED;
            }
            let _ = AppendMenuW(menu, flags, 3000 + vol as usize, &HSTRING::from(label));
        }

        let chosen = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN,
            pt.x,
            pt.y,
            None,
            dock_hwnd,
            None,
        );
        let _ = DestroyMenu(menu);

        if let Some(state) = dock_for(dock_hwnd) {
            state.borrow_mut().menu_open = false;
            let _ = SetTimer(Some(dock_hwnd), TIMER_HIDE, HIDE_DELAY_MS, None);
        }

        let chosen = chosen.0 as usize;
        if (3000..=3100).contains(&chosen) {
            let vol = (chosen - 3000) as u32;
            crate::platform::audio::set_volume(vol);
            let _ = InvalidateRect(Some(dock_hwnd), None, false);
        }
    }
}

fn show_scale_menu(dock_hwnd: HWND, parent_hwnd: HWND, session: &Rc<RefCell<Session>>, scale: f64) {
    if let Some(state) = dock_for(dock_hwnd) {
        state.borrow_mut().menu_open = true;
    }
    let mut client = RECT::default();
    let _ = unsafe { GetClientRect(dock_hwnd, &mut client) };
    let r = button_rect(
        DockButton::Scale,
        client.right - client.left,
        client.bottom - client.top,
        scale,
    );
    let mut pt = POINT {
        x: r.left,
        y: r.bottom,
    };
    let _ = unsafe { ClientToScreen(dock_hwnd, &mut pt) };

    // SAFETY: Building Win32 popup menu for display scales.
    unsafe {
        let menu = match CreatePopupMenu() {
            Ok(m) => m,
            Err(_) => {
                if let Some(state) = dock_for(dock_hwnd) {
                    state.borrow_mut().menu_open = false;
                }
                return;
            }
        };

        let current_scale = session.borrow().controller.scale_choice();
        let mon_scale = session_window::window_dpi_scale(parent_hwnd);
        let items = scale_menu(current_scale, mon_scale);

        for (i, item) in items.iter().enumerate() {
            let mut flags = MF_STRING;
            if item.is_active {
                flags |= MF_CHECKED;
            }
            let label = HSTRING::from(&item.label);
            let _ = AppendMenuW(menu, flags, 1000 + i, &label);
        }

        let chosen = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN,
            pt.x,
            pt.y,
            None,
            dock_hwnd,
            None,
        );
        let _ = DestroyMenu(menu);

        if let Some(state) = dock_for(dock_hwnd) {
            state.borrow_mut().menu_open = false;
            let _ = SetTimer(Some(dock_hwnd), TIMER_HIDE, HIDE_DELAY_MS, None);
        }

        let chosen = chosen.0 as usize;
        if (1000..1000 + items.len()).contains(&chosen) {
            let item = &items[chosen - 1000];
            session
                .borrow_mut()
                .controller
                .set_scale_choice(item.choice);
            session_window::rebuild_menu(parent_hwnd, session);
        }
    }
}

fn show_actions_menu(
    dock_hwnd: HWND,
    parent_hwnd: HWND,
    session: &Rc<RefCell<Session>>,
    scale: f64,
) {
    if let Some(state) = dock_for(dock_hwnd) {
        state.borrow_mut().menu_open = true;
    }
    let mut client = RECT::default();
    let _ = unsafe { GetClientRect(dock_hwnd, &mut client) };
    let r = button_rect(
        DockButton::Actions,
        client.right - client.left,
        client.bottom - client.top,
        scale,
    );
    let mut pt = POINT {
        x: r.left,
        y: r.bottom,
    };
    let _ = unsafe { ClientToScreen(dock_hwnd, &mut pt) };

    // SAFETY: Building Win32 popup menu for session actions.
    unsafe {
        let menu = match CreatePopupMenu() {
            Ok(m) => m,
            Err(_) => {
                if let Some(state) = dock_for(dock_hwnd) {
                    state.borrow_mut().menu_open = false;
                }
                return;
            }
        };

        // Keys submenu
        if let Ok(keys_menu) = CreatePopupMenu() {
            for (i, combo) in KeyCombo::ALL.iter().enumerate() {
                let label = HSTRING::from(combo.label());
                let _ = AppendMenuW(keys_menu, MF_STRING, 2000 + i, &label);
            }
            let _ = AppendMenuW(
                menu,
                MF_POPUP,
                keys_menu.0 as usize,
                &HSTRING::from("Send Keys"),
            );
        }

        let _ = AppendMenuW(menu, MF_STRING, 2100, &HSTRING::from("Type Clipboard Text"));
        let view_only = session.borrow().controller.view_only();
        let vo_flags = if view_only {
            MF_STRING | MF_CHECKED
        } else {
            MF_STRING
        };
        let _ = AppendMenuW(menu, vo_flags, 2101, &HSTRING::from("View Only"));
        let _ = AppendMenuW(menu, MF_STRING, 2102, &HSTRING::from("Save Screenshot"));

        let chosen = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN,
            pt.x,
            pt.y,
            None,
            dock_hwnd,
            None,
        );
        let _ = DestroyMenu(menu);

        if let Some(state) = dock_for(dock_hwnd) {
            state.borrow_mut().menu_open = false;
            let _ = SetTimer(Some(dock_hwnd), TIMER_HIDE, HIDE_DELAY_MS, None);
        }

        let chosen = chosen.0 as usize;
        if (2000..2000 + KeyCombo::ALL.len()).contains(&chosen) {
            let combo = KeyCombo::ALL[chosen - 2000];
            session.borrow().controller.send_combo(combo);
        } else if chosen == 2100 {
            session_window::type_clipboard(parent_hwnd, session);
        } else if chosen == 2101 {
            session.borrow_mut().controller.set_view_only(!view_only);
            session_window::rebuild_menu(parent_hwnd, session);
        } else if chosen == 2102 {
            session_window::screenshot(parent_hwnd, session);
        }
    }
}

fn paint_dock(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    // SAFETY: BeginPaint for dock window.
    let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
    let mut client = RECT::default();
    let _ = unsafe { GetClientRect(hwnd, &mut client) };
    let width = client.right - client.left;
    let height = client.bottom - client.top;
    let scale = dock_dpi_scale(hwnd);

    let Some(state) = dock_for(hwnd) else {
        let _ = unsafe { EndPaint(hwnd, &ps) };
        return;
    };
    let (name, hovered) = {
        let s = state.borrow();
        let name = s.session.borrow().name.clone();
        (name, s.hovered)
    };

    // Double-buffered drawing for zero flicker.
    unsafe {
        let mem_dc = CreateCompatibleDC(Some(hdc));
        let mem_bm = CreateCompatibleBitmap(hdc, width, height);
        let old_bm = SelectObject(mem_dc, mem_bm.into());

        // Background dark charcoal.
        let bg_brush = CreateSolidBrush(COLORREF(0x1F1F1F));
        let _ = FillRect(mem_dc, &client, bg_brush);
        let _ = DeleteObject(bg_brush.into());

        // Bottom accent line matching Linux: border-bottom: 3px solid @accent_bg_color;
        let accent_h = (3.0 * scale).round().max(2.0) as i32;
        let accent_brush = CreateSolidBrush(COLORREF(0xD47800)); // Win Blue BGR (RGB: 0x00, 0x78, 0xD4)
        let accent_rect = RECT {
            left: 0,
            top: height - accent_h,
            right: width,
            bottom: height,
        };
        let _ = FillRect(mem_dc, &accent_rect, accent_brush);
        let _ = DeleteObject(accent_brush.into());

        // Font setup.
        let font_h = -((12.0 * scale).round() as i32);
        let font = CreateFontW(
            font_h,
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            DEFAULT_PITCH.0 as u32,
            w!("Segoe UI"),
        );
        let old_font = SelectObject(mem_dc, font.into());
        let _ = SetBkMode(mem_dc, TRANSPARENT);

        // Draw buttons and backgrounds.
        for &btn in &ALL_BUTTONS {
            let r = button_rect(btn, width, height, scale);
            let is_hovered = hovered == Some(btn);

            if is_hovered {
                let hover_color = if btn == DockButton::Disconnect {
                    COLORREF(0x1C2BC4) // Soft red BGR
                } else if btn == DockButton::Mic && crate::platform::audio::is_mic_muted() {
                    COLORREF(0x1C2BC4) // Muted mic red BGR
                } else {
                    COLORREF(0x383838) // Hover gray
                };
                let hover_brush = CreateSolidBrush(hover_color);
                let _ = FillRect(mem_dc, &r, hover_brush);
                let _ = DeleteObject(hover_brush.into());
            } else if btn == DockButton::Mic && crate::platform::audio::is_mic_muted() {
                // Non-hovered muted mic subtle warning tint
                let warning_brush = CreateSolidBrush(COLORREF(0x1F2255));
                let _ = FillRect(mem_dc, &r, warning_brush);
                let _ = DeleteObject(warning_brush.into());
            }

            let _ = SetTextColor(mem_dc, COLORREF(0xFFFFFF));
            let mut draw_r = r;
            match btn {
                DockButton::Mic => {
                    let muted = crate::platform::audio::is_mic_muted();
                    let text = if muted { "🎤 Off" } else { "🎤 On" };
                    let mut text_w: Vec<u16> =
                        text.encode_utf16().chain(std::iter::once(0)).collect();
                    let _ = DrawTextW(
                        mem_dc,
                        &mut text_w,
                        &mut draw_r,
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                    );
                }
                DockButton::Volume => {
                    let vol = crate::platform::audio::get_volume();
                    let text = format!("🔊 {vol}% ▾");
                    let mut text_w: Vec<u16> =
                        text.encode_utf16().chain(std::iter::once(0)).collect();
                    let _ = DrawTextW(
                        mem_dc,
                        &mut text_w,
                        &mut draw_r,
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                    );
                }
                DockButton::Scale => {
                    let mut text_w: Vec<u16> =
                        "Scale ▾".encode_utf16().chain(std::iter::once(0)).collect();
                    let _ = DrawTextW(
                        mem_dc,
                        &mut text_w,
                        &mut draw_r,
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                    );
                }
                DockButton::Actions => {
                    let mut text_w: Vec<u16> = "Actions ▾"
                        .encode_utf16()
                        .chain(std::iter::once(0))
                        .collect();
                    let _ = DrawTextW(
                        mem_dc,
                        &mut text_w,
                        &mut draw_r,
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                    );
                }
                DockButton::Restore => {
                    let mut text_w: Vec<u16> =
                        "🗗".encode_utf16().chain(std::iter::once(0)).collect();
                    let _ = DrawTextW(
                        mem_dc,
                        &mut text_w,
                        &mut draw_r,
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                    );
                }
                DockButton::Minimize => {
                    let mut text_w: Vec<u16> =
                        "―".encode_utf16().chain(std::iter::once(0)).collect();
                    let _ = DrawTextW(
                        mem_dc,
                        &mut text_w,
                        &mut draw_r,
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                    );
                }
                DockButton::Disconnect => {
                    let mut text_w: Vec<u16> =
                        "✕".encode_utf16().chain(std::iter::once(0)).collect();
                    let _ = DrawTextW(
                        mem_dc,
                        &mut text_w,
                        &mut draw_r,
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                    );
                }
            }
        }

        // Title and green status dot.
        let title_r = RECT {
            left: (16.0 * scale) as i32,
            top: (4.0 * scale) as i32,
            right: width - (490.0 * scale) as i32,
            bottom: height - (7.0 * scale) as i32,
        };
        let title_str = format!("● {name}");
        let mut title_w: Vec<u16> = title_str.encode_utf16().chain(std::iter::once(0)).collect();
        let _ = SetTextColor(mem_dc, COLORREF(0xE0E0E0));
        let mut draw_title = title_r;
        let _ = DrawTextW(
            mem_dc,
            &mut title_w,
            &mut draw_title,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
        );

        // Blit to screen.
        let _ = BitBlt(hdc, 0, 0, width, height, Some(mem_dc), 0, 0, SRCCOPY);

        let _ = SelectObject(mem_dc, old_font);
        let _ = DeleteObject(font.into());
        let _ = SelectObject(mem_dc, old_bm);
        let _ = DeleteObject(mem_bm.into());
        let _ = DeleteDC(mem_dc);
    }

    let _ = unsafe { EndPaint(hwnd, &ps) };
}
