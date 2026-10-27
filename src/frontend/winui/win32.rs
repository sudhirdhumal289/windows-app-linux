//! Win32 helpers for the WinUI frontend: message boxes and task dialogs, the clipboard,
//! file pickers, BMP encoding, window chrome, the main window's close guard and a deferral
//! queue that runs modal Win32 UI outside Reactor's update turn.
//!
//! Modal Win32 UI (file pickers, task dialogs, popup menus) runs its own message loop.
//! Starting one from inside a Reactor `update` would re-enter Reactor while it is busy, so
//! components hand such work to [`defer`]; it runs from a message posted to a hidden window.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use windows::core::{w, BOOL, HRESULT, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    GlobalFree, HANDLE, HGLOBAL, HMODULE, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMSBT_MAINWINDOW, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_USE_IMMERSIVE_DARK_MODE,
};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber,
    IsClipboardFormatAvailable, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::System::Ole::{CF_DIB, CF_UNICODETEXT};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Controls::{
    TASKDIALOGCONFIG, TASKDIALOGCONFIG_0, TASKDIALOG_BUTTON, TDF_ALLOW_DIALOG_CANCELLATION,
    TDF_POSITION_RELATIVE_TO_WINDOW, TD_INFORMATION_ICON, TD_WARNING_ICON,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FileOpenDialog, FileSaveDialog, IFileOpenDialog, IFileSaveDialog, ShellExecuteW,
    FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, CreateWindowExW, DefWindowProcW, EnumThreadWindows, GetClassNameW,
    GetSystemMetrics, IsWindow, IsWindowVisible, LoadImageW, MessageBoxW, PostMessageW,
    RegisterClassExW, SendMessageW, SetWindowLongPtrW, GWLP_WNDPROC, HWND_MESSAGE, ICON_BIG,
    ICON_SMALL, IDCANCEL, IDNO, IDOK, IDYES, IMAGE_ICON, LR_DEFAULTCOLOR, MB_ICONERROR,
    MB_ICONINFORMATION, MB_ICONWARNING, MB_OK, MB_OKCANCEL, MB_YESNOCANCEL, MESSAGEBOX_STYLE,
    SM_CXICON, SM_CXSMICON, SM_CYICON, SM_CYSMICON, SW_SHOWNORMAL, WINDOW_EX_STYLE, WINDOW_STYLE,
    WM_APP, WM_CLOSE, WM_SETICON, WNDCLASSEXW, WNDPROC,
};

/// Window class of the top-level windows created by WinUI 3 desktop apps.
const WINUI_WINDOW_CLASS: &str = "WinUIDesktopWin32WindowClass";
const DEFERRAL_CLASS: PCWSTR = w!("WindowsAppDeferral");
const WM_RUN_DEFERRED: u32 = WM_APP + 2;

type Deferred = Box<dyn FnOnce()>;

thread_local! {
    static DEFERRED: RefCell<VecDeque<Deferred>> = RefCell::new(VecDeque::new());
    static DEFERRAL_WINDOW: Cell<isize> = const { Cell::new(0) };
    static MAIN_WINDOW: Cell<isize> = const { Cell::new(0) };
    static CLOSE_GUARD: RefCell<Option<CloseGuard>> = const { RefCell::new(None) };
    static ALLOW_CLOSE: Cell<bool> = const { Cell::new(false) };
    static DARK_MODE: Cell<bool> = const { Cell::new(false) };
}

/// A NUL-terminated UTF-16 copy of `text` for `PCWSTR` parameters.
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Converts a window handle to a plain integer (for `Send` closures and map keys).
pub fn hwnd_key(hwnd: HWND) -> isize {
    hwnd.0 as isize
}

/// The window handle behind [`hwnd_key`].
pub fn hwnd_from_key(key: isize) -> HWND {
    HWND(key as *mut c_void)
}

// ----- Deferral -----

/// Runs `task` on the UI thread after the current message (outside Reactor's update).
pub fn defer(task: impl FnOnce() + 'static) {
    DEFERRED.with_borrow_mut(|queue| queue.push_back(Box::new(task)));
    let Some(hwnd) = deferral_window() else {
        log::warn!("No deferral window; running the task immediately");
        run_deferred();
        return;
    };
    // SAFETY: posting a private message to a window owned by this thread.
    if let Err(e) = unsafe { PostMessageW(Some(hwnd), WM_RUN_DEFERRED, WPARAM(0), LPARAM(0)) } {
        log::warn!("Could not post a deferred task: {e}");
        run_deferred();
    }
}

fn run_deferred() {
    // One task per message keeps the borrow short; tasks may defer more work.
    while let Some(task) = DEFERRED.with_borrow_mut(VecDeque::pop_front) {
        task();
    }
}

fn deferral_window() -> Option<HWND> {
    let existing = DEFERRAL_WINDOW.get();
    if existing != 0 {
        return Some(hwnd_from_key(existing));
    }
    // SAFETY: registers a class with a valid window procedure and creates a message-only
    // window on this thread; all pointers outlive the calls.
    let hwnd = unsafe {
        let instance = GetModuleHandleW(None).ok()?;
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(deferral_proc),
            hInstance: instance.into(),
            lpszClassName: DEFERRAL_CLASS,
            ..Default::default()
        };
        RegisterClassExW(&class);
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            DEFERRAL_CLASS,
            w!(""),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance.into()),
            None,
        )
    };
    match hwnd {
        Ok(hwnd) => {
            DEFERRAL_WINDOW.set(hwnd_key(hwnd));
            Some(hwnd)
        }
        Err(e) => {
            log::error!("Could not create the deferral window: {e}");
            None
        }
    }
}

unsafe extern "system" fn deferral_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_RUN_DEFERRED {
        run_deferred();
        return LRESULT(0);
    }
    // SAFETY: default handling for every other message of this window.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

// ----- Main window -----

struct CloseGuard {
    hwnd: isize,
    previous: isize,
    /// Returns whether the window may close now.
    allow: Rc<dyn Fn() -> bool>,
}

/// Finds the main WinUI window of this thread and asks `allow` before it closes. Returns
/// whether the window was found (call again later if not).
/// Sets the application icon (from embedded resource ID 1) on the specified HWND.
pub fn set_window_icon(hwnd: HWND) {
    let Ok(instance) = (unsafe { GetModuleHandleW(None) }) else {
        return;
    };
    unsafe {
        let cx_big = GetSystemMetrics(SM_CXICON);
        let cy_big = GetSystemMetrics(SM_CYICON);
        if let Ok(handle) = LoadImageW(
            Some(instance.into()),
            PCWSTR(1 as *const u16),
            IMAGE_ICON,
            cx_big,
            cy_big,
            LR_DEFAULTCOLOR,
        ) {
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_BIG as usize)),
                Some(LPARAM(handle.0 as isize)),
            );
        }

        let cx_small = GetSystemMetrics(SM_CXSMICON);
        let cy_small = GetSystemMetrics(SM_CYSMICON);
        if let Ok(handle) = LoadImageW(
            Some(instance.into()),
            PCWSTR(1 as *const u16),
            IMAGE_ICON,
            cx_small,
            cy_small,
            LR_DEFAULTCOLOR,
        ) {
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_SMALL as usize)),
                Some(LPARAM(handle.0 as isize)),
            );
        }
    }
}

pub fn install_close_guard(allow: impl Fn() -> bool + 'static) -> bool {
    if CLOSE_GUARD.with_borrow(Option::is_some) {
        return true;
    }
    let Some(hwnd) = find_winui_window() else {
        return false;
    };
    set_window_icon(hwnd);
    MAIN_WINDOW.set(hwnd_key(hwnd));
    // SAFETY: replaces the window procedure of a window owned by this thread; the previous
    // procedure is kept and called for every message (`guard_proc`).
    let previous = unsafe {
        SetWindowLongPtrW(
            hwnd,
            GWLP_WNDPROC,
            guard_proc as *const () as usize as isize,
        )
    };
    if previous == 0 {
        log::warn!("Could not watch the main window for closing");
        return true;
    }
    CLOSE_GUARD.set(Some(CloseGuard {
        hwnd: hwnd_key(hwnd),
        previous,
        allow: Rc::new(allow),
    }));
    true
}

/// Lets the main window close without asking (after the user confirmed quitting).
pub fn allow_close() {
    ALLOW_CLOSE.set(true);
}

/// The main window, once [`install_close_guard`] found it.
pub fn main_window() -> Option<HWND> {
    let key = MAIN_WINDOW.get();
    // SAFETY: IsWindow accepts any handle value.
    (key != 0 && unsafe { IsWindow(Some(hwnd_from_key(key))) }.as_bool())
        .then(|| hwnd_from_key(key))
}

fn find_winui_window() -> Option<HWND> {
    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the `Vec<HWND>` passed by `find_winui_window` below, alive
        // for the duration of the enumeration.
        let found = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
        let mut class = [0u16; 64];
        // SAFETY: the buffer is valid for its length.
        let len = unsafe { GetClassNameW(hwnd, &mut class) };
        let len = usize::try_from(len).unwrap_or(0);
        if String::from_utf16_lossy(&class[..len]) == WINUI_WINDOW_CLASS {
            found.push(hwnd);
        }
        BOOL(1)
    }
    let mut found: Vec<HWND> = Vec::new();
    // SAFETY: the callback only writes to `found`, which outlives the call.
    unsafe {
        let _ = EnumThreadWindows(
            GetCurrentThreadId(),
            Some(collect),
            LPARAM(&mut found as *mut Vec<HWND> as isize),
        );
    }
    // SAFETY: IsWindowVisible accepts any window handle.
    let visible = found
        .iter()
        .copied()
        .find(|hwnd| unsafe { IsWindowVisible(*hwnd) }.as_bool());
    visible.or_else(|| found.first().copied())
}

unsafe extern "system" fn guard_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let previous = CLOSE_GUARD.with_borrow(|guard| {
        guard
            .as_ref()
            .filter(|g| g.hwnd == hwnd_key(hwnd))
            .map(|g| g.previous)
    });
    let Some(previous) = previous else {
        // SAFETY: not our window any more; default handling.
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    };
    if msg == WM_CLOSE && !ALLOW_CLOSE.get() {
        // The callback may borrow other state, so it runs without the guard borrowed.
        let allow = CLOSE_GUARD.with_borrow(|guard| guard.as_ref().map(|g| g.allow.clone()));
        let allowed = allow.is_none_or(|callback| callback());
        if !allowed {
            return LRESULT(0);
        }
    }
    // SAFETY: `previous` is the window procedure that was installed before ours.
    unsafe {
        let previous: WNDPROC = std::mem::transmute::<isize, WNDPROC>(previous);
        CallWindowProcW(previous, hwnd, msg, wparam, lparam)
    }
}

// ----- Theme and window chrome -----

/// Remembers the app's color scheme for Win32 windows (session windows, dialogs).
pub fn set_dark_mode(dark: bool) {
    DARK_MODE.set(dark);
}

pub fn dark_mode() -> bool {
    DARK_MODE.get()
}

/// Dark or light caption plus the Mica backdrop (Windows 11) for a Win32 window.
pub fn apply_window_chrome(hwnd: HWND) {
    let dark = BOOL::from(dark_mode());
    let backdrop = DWMSBT_MAINWINDOW;
    // SAFETY: both attributes take a pointer to a value of the given size; failures (older
    // Windows versions) only lose the visual effect.
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&dark as *const BOOL).cast(),
            std::mem::size_of::<BOOL>() as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &backdrop as *const _ as *const c_void,
            std::mem::size_of_val(&backdrop) as u32,
        );
    }
}

// ----- Message boxes and task dialogs -----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Information,
    Warning,
    Error,
}

/// A simple message with an OK button.
pub fn message(owner: Option<HWND>, title: &str, text: &str, icon: Icon) {
    let style = MB_OK
        | match icon {
            Icon::Information => MB_ICONINFORMATION,
            Icon::Warning => MB_ICONWARNING,
            Icon::Error => MB_ICONERROR,
        };
    message_box(owner, title, text, style);
}

fn message_box(owner: Option<HWND>, title: &str, text: &str, style: MESSAGEBOX_STYLE) -> i32 {
    let title = HSTRING::from(title);
    let text = HSTRING::from(text);
    // SAFETY: both strings outlive the modal call.
    unsafe { MessageBoxW(owner, &text, &title, style) }.0
}

type TaskDialogIndirectFn =
    unsafe extern "system" fn(*const TASKDIALOGCONFIG, *mut i32, *mut i32, *mut BOOL) -> HRESULT;

fn task_dialog_indirect() -> Option<TaskDialogIndirectFn> {
    // SAFETY: loads comctl32 (version 6 through the app manifest) and looks up an export
    // whose signature matches `TaskDialogIndirectFn`. Without the manifest, version 5 is
    // loaded, which lacks the export, and the caller falls back to a message box.
    unsafe {
        let module: HMODULE = GetModuleHandleW(w!("comctl32.dll"))
            .or_else(|_| LoadLibraryW(w!("comctl32.dll")))
            .ok()?;
        let proc = GetProcAddress(module, windows::core::s!("TaskDialogIndirect"))?;
        Some(std::mem::transmute::<
            unsafe extern "system" fn() -> isize,
            TaskDialogIndirectFn,
        >(proc))
    }
}

/// Asks a question with custom buttons (in reading order; the first one cancels). Returns
/// the index of the chosen button, `None` when dismissed.
pub fn choose(
    owner: Option<HWND>,
    title: &str,
    heading: &str,
    body: &str,
    buttons: &[&str],
    default: usize,
    warning: bool,
) -> Option<usize> {
    if buttons.is_empty() {
        return None;
    }
    let Some(task_dialog) = task_dialog_indirect() else {
        return choose_with_message_box(owner, title, heading, body, buttons);
    };
    const FIRST_ID: i32 = 1000;
    let title_w = wide(title);
    let heading_w = wide(heading);
    let body_w = wide(body);
    let labels: Vec<Vec<u16>> = buttons.iter().map(|b| wide(b)).collect();
    let specs: Vec<TASKDIALOG_BUTTON> = labels
        .iter()
        .enumerate()
        .map(|(i, label)| TASKDIALOG_BUTTON {
            nButtonID: FIRST_ID + i as i32,
            pszButtonText: PCWSTR(label.as_ptr()),
        })
        .collect();
    let config = TASKDIALOGCONFIG {
        cbSize: std::mem::size_of::<TASKDIALOGCONFIG>() as u32,
        hwndParent: owner.unwrap_or_default(),
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION | TDF_POSITION_RELATIVE_TO_WINDOW,
        pszWindowTitle: PCWSTR(title_w.as_ptr()),
        Anonymous1: TASKDIALOGCONFIG_0 {
            pszMainIcon: if warning {
                TD_WARNING_ICON
            } else {
                TD_INFORMATION_ICON
            },
        },
        pszMainInstruction: PCWSTR(heading_w.as_ptr()),
        pszContent: PCWSTR(body_w.as_ptr()),
        cButtons: specs.len() as u32,
        pButtons: specs.as_ptr(),
        nDefaultButton: FIRST_ID + default.min(buttons.len() - 1) as i32,
        ..Default::default()
    };
    let mut pressed = 0i32;
    // SAFETY: `config` and every string and button it points to outlive the modal call.
    let result = unsafe {
        task_dialog(
            &config,
            &mut pressed,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if result.is_err() {
        log::warn!("Task dialog failed: {}", windows::core::Error::from(result));
        return choose_with_message_box(owner, title, heading, body, buttons);
    }
    usize::try_from(pressed - FIRST_ID)
        .ok()
        .filter(|i| *i < buttons.len())
}

/// Fallback without comctl32 v6: Yes/No/Cancel (or OK/Cancel) explained in the text.
fn choose_with_message_box(
    owner: Option<HWND>,
    title: &str,
    heading: &str,
    body: &str,
    buttons: &[&str],
) -> Option<usize> {
    let mut text = String::new();
    if !heading.is_empty() {
        text.push_str(heading);
        text.push_str("\n\n");
    }
    text.push_str(body);
    match buttons.len() {
        1 => {
            message_box(owner, title, &text, MB_OK | MB_ICONINFORMATION);
            Some(0)
        }
        2 => {
            text.push_str(&format!("\n\nOK: {}\nCancel: {}", buttons[1], buttons[0]));
            let answer = message_box(owner, title, &text, MB_OKCANCEL | MB_ICONWARNING);
            (answer == IDOK.0).then_some(1).or(Some(0))
        }
        _ => {
            text.push_str(&format!(
                "\n\nYes: {}\nNo: {}\nCancel: {}",
                buttons[2], buttons[1], buttons[0]
            ));
            let answer = message_box(owner, title, &text, MB_YESNOCANCEL | MB_ICONWARNING);
            match answer {
                a if a == IDYES.0 => Some(2),
                a if a == IDNO.0 => Some(1),
                a if a == IDCANCEL.0 => Some(0),
                _ => None,
            }
        }
    }
}

// ----- Clipboard -----

/// Counter that changes whenever any application changes the clipboard.
pub fn clipboard_sequence() -> u32 {
    // SAFETY: no preconditions.
    unsafe { GetClipboardSequenceNumber() }
}

struct OpenedClipboard;

impl OpenedClipboard {
    fn open(owner: HWND) -> Option<Self> {
        // Another application may hold the clipboard for a moment.
        for _ in 0..5 {
            // SAFETY: `owner` is a window of this thread.
            if unsafe { OpenClipboard(Some(owner)) }.is_ok() {
                return Some(Self);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        None
    }
}

impl Drop for OpenedClipboard {
    fn drop(&mut self) {
        // SAFETY: the clipboard was opened by `open`.
        let _ = unsafe { CloseClipboard() };
    }
}

/// The clipboard's text, if it holds any.
pub fn clipboard_text(owner: HWND) -> Option<String> {
    // SAFETY: format check without preconditions.
    unsafe { IsClipboardFormatAvailable(u32::from(CF_UNICODETEXT.0)) }.ok()?;
    let _clipboard = OpenedClipboard::open(owner)?;
    // SAFETY: the clipboard is open; the global memory is locked while it is read and its
    // size bounds the slice.
    unsafe {
        let handle = GetClipboardData(u32::from(CF_UNICODETEXT.0)).ok()?;
        let memory = HGLOBAL(handle.0);
        let data = GlobalLock(memory) as *const u16;
        if data.is_null() {
            return None;
        }
        let units = GlobalSize(memory) / 2;
        let slice = std::slice::from_raw_parts(data, units);
        let len = slice.iter().position(|c| *c == 0).unwrap_or(units);
        let text = String::from_utf16_lossy(&slice[..len]);
        let _ = GlobalUnlock(memory);
        Some(text)
    }
}

fn set_clipboard(owner: HWND, format: u32, bytes: &[u8]) -> Result<(), String> {
    // SAFETY: the allocation is large enough for `bytes`; ownership passes to the
    // clipboard on success and is freed here otherwise.
    unsafe {
        let memory = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)).map_err(|e| e.to_string())?;
        let target = GlobalLock(memory) as *mut u8;
        if target.is_null() {
            let _ = GlobalFree(Some(memory));
            return Err("could not lock clipboard memory".to_string());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len());
        let _ = GlobalUnlock(memory);
        let Some(_clipboard) = OpenedClipboard::open(owner) else {
            let _ = GlobalFree(Some(memory));
            return Err("the clipboard is in use".to_string());
        };
        let _ = EmptyClipboard();
        if let Err(e) = SetClipboardData(format, Some(HANDLE(memory.0))) {
            let _ = GlobalFree(Some(memory));
            return Err(e.to_string());
        }
    }
    Ok(())
}

/// Replaces the clipboard with `text`.
pub fn set_clipboard_text(owner: HWND, text: &str) -> Result<(), String> {
    let bytes: Vec<u8> = text
        .encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect();
    set_clipboard(owner, u32::from(CF_UNICODETEXT.0), &bytes)
}

/// Replaces the clipboard with an RGBA image (as a bottom-up 32-bit DIB).
pub fn set_clipboard_image(
    owner: HWND,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<(), String> {
    let row = width as usize * 4;
    if rgba.len() < row * height as usize {
        return Err("the image data is incomplete".to_string());
    }
    let mut bytes = Vec::with_capacity(40 + row * height as usize);
    bytes.extend_from_slice(&bitmap_info_header(width, height as i32));
    for y in (0..height as usize).rev() {
        for pixel in rgba[y * row..(y + 1) * row].as_chunks::<4>().0 {
            bytes.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
        }
    }
    set_clipboard(owner, u32::from(CF_DIB.0), &bytes)
}

// ----- Bitmaps -----

/// A `BITMAPINFOHEADER` for a 32-bit uncompressed bitmap (negative height = top-down).
fn bitmap_info_header(width: u32, height: i32) -> [u8; 40] {
    let mut header = [0u8; 40];
    let image_size = width
        .saturating_mul(height.unsigned_abs())
        .saturating_mul(4);
    header[0..4].copy_from_slice(&40u32.to_le_bytes());
    header[4..8].copy_from_slice(&(width as i32).to_le_bytes());
    header[8..12].copy_from_slice(&height.to_le_bytes());
    header[12..14].copy_from_slice(&1u16.to_le_bytes());
    header[14..16].copy_from_slice(&32u16.to_le_bytes());
    // Compression BI_RGB (0) is already zero.
    header[20..24].copy_from_slice(&image_size.to_le_bytes());
    header[24..28].copy_from_slice(&2835i32.to_le_bytes());
    header[28..32].copy_from_slice(&2835i32.to_le_bytes());
    header
}

/// Writes BGRX pixels (rows `stride` bytes apart) as a top-down 32-bit BMP file.
pub fn write_bmp(
    path: &Path,
    width: u32,
    height: u32,
    stride: u32,
    pixels: &[u8],
) -> std::io::Result<()> {
    let row = width as usize * 4;
    let stride = stride as usize;
    if width == 0
        || height == 0
        || stride < row
        || pixels.len() < stride * (height as usize - 1) + row
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "there is no picture to save yet",
        ));
    }
    let data_size = row * height as usize;
    let file_size = 14 + 40 + data_size;
    let mut out = Vec::with_capacity(file_size);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&u32::try_from(file_size).unwrap_or(u32::MAX).to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&bitmap_info_header(width, -(height as i32)));
    for y in 0..height as usize {
        for pixel in pixels[y * stride..y * stride + row].as_chunks::<4>().0 {
            out.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 0xFF]);
        }
    }
    std::fs::write(path, out)
}

// ----- File pickers and the shell -----

/// Asks for an existing file. `filters`: (label, pattern) pairs such as
/// `("Remote Desktop files", "*.rdp")`.
pub fn pick_open_file(
    owner: Option<HWND>,
    title: &str,
    filters: &[(&str, &str)],
) -> Option<PathBuf> {
    pick(owner, title, filters, false).unwrap_or_else(|e| {
        log::warn!("The file picker failed: {e}");
        None
    })
}

/// Asks for a folder.
pub fn pick_folder(owner: Option<HWND>, title: &str) -> Option<PathBuf> {
    pick(owner, title, &[], true).unwrap_or_else(|e| {
        log::warn!("The folder picker failed: {e}");
        None
    })
}

fn pick(
    owner: Option<HWND>,
    title: &str,
    filters: &[(&str, &str)],
    folders: bool,
) -> windows::core::Result<Option<PathBuf>> {
    let names: Vec<Vec<u16>> = filters.iter().map(|(n, _)| wide(n)).collect();
    let specs: Vec<Vec<u16>> = filters.iter().map(|(_, s)| wide(s)).collect();
    let filter_specs: Vec<COMDLG_FILTERSPEC> = names
        .iter()
        .zip(&specs)
        .map(|(name, spec)| COMDLG_FILTERSPEC {
            pszName: PCWSTR(name.as_ptr()),
            pszSpec: PCWSTR(spec.as_ptr()),
        })
        .collect();
    let title = HSTRING::from(title);
    // SAFETY: COM calls on the UI thread (an STA); every string outlives the calls and
    // the returned path is freed with CoTaskMemFree.
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
        let mut options = dialog.GetOptions()? | FOS_FORCEFILESYSTEM;
        if folders {
            options |= FOS_PICKFOLDERS;
        }
        dialog.SetOptions(options)?;
        dialog.SetTitle(&title)?;
        if !filter_specs.is_empty() {
            dialog.SetFileTypes(&filter_specs)?;
        }
        if dialog.Show(owner).is_err() {
            return Ok(None);
        }
        let item = dialog.GetResult()?;
        Ok(Some(take_path(item.GetDisplayName(SIGDN_FILESYSPATH)?)))
    }
}

/// Asks where to save a file, suggesting `file_name`.
pub fn pick_save_file(
    owner: Option<HWND>,
    title: &str,
    file_name: &str,
    filter: (&str, &str),
    extension: &str,
) -> Option<PathBuf> {
    let name = wide(filter.0);
    let spec = wide(filter.1);
    let filter_spec = [COMDLG_FILTERSPEC {
        pszName: PCWSTR(name.as_ptr()),
        pszSpec: PCWSTR(spec.as_ptr()),
    }];
    let title = HSTRING::from(title);
    let file_name = HSTRING::from(file_name);
    let extension = HSTRING::from(extension);
    // SAFETY: as in `pick`.
    let result = unsafe {
        (|| -> windows::core::Result<Option<PathBuf>> {
            let dialog: IFileSaveDialog =
                CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)?;
            dialog.SetTitle(&title)?;
            dialog.SetFileTypes(&filter_spec)?;
            dialog.SetDefaultExtension(&extension)?;
            dialog.SetFileName(&file_name)?;
            if dialog.Show(owner).is_err() {
                return Ok(None);
            }
            let item = dialog.GetResult()?;
            Ok(Some(take_path(item.GetDisplayName(SIGDN_FILESYSPATH)?)))
        })()
    };
    result.unwrap_or_else(|e| {
        log::warn!("The save dialog failed: {e}");
        None
    })
}

/// Converts and frees a path returned by the shell.
///
/// # Safety
/// `path` must be a NUL-terminated string allocated with `CoTaskMemAlloc`.
unsafe fn take_path(path: PWSTR) -> PathBuf {
    // SAFETY: guaranteed by the caller.
    unsafe {
        let text = path.to_string().unwrap_or_default();
        CoTaskMemFree(Some(path.0 as *const c_void));
        PathBuf::from(text)
    }
}

/// Opens a web page or file with its default application.
pub fn open_uri(uri: &str) {
    let uri = HSTRING::from(uri);
    // SAFETY: all strings outlive the call.
    let result = unsafe { ShellExecuteW(None, w!("open"), &uri, None, None, SW_SHOWNORMAL) };
    if result.0 as isize <= 32 {
        log::warn!("Could not open {uri}");
    }
}

/// Converts a COM pointer of another `windows-core` version to this crate's `IUnknown`.
///
/// # Safety
/// `value` must be a COM interface pointer wrapper (a single non-null interface pointer
/// that releases its reference on drop), as all `windows-core` interface types are.
pub unsafe fn rebind_unknown<T>(value: T) -> Option<windows::core::IUnknown> {
    if std::mem::size_of::<T>() != std::mem::size_of::<*mut c_void>() {
        return None;
    }
    // SAFETY: guaranteed by the caller; the reference held by `value` moves to the result.
    unsafe {
        let raw: *mut c_void = std::mem::transmute_copy(&value);
        std::mem::forget(value);
        if raw.is_null() {
            return None;
        }
        Some(<windows::core::IUnknown as windows::core::Interface>::from_raw(raw))
    }
}

/// Rectangle helpers.
pub fn rect_size(rect: &RECT) -> (i32, i32) {
    (rect.right - rect.left, rect.bottom - rect.top)
}
