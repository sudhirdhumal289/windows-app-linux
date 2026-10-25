//! Safe Rust wrapper for in-process FreeRDP 3 C bridge.

use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::{Arc, Mutex};

pub const PTR_FLAGS_HWHEEL: u16 = 0x0400;
pub const PTR_FLAGS_WHEEL: u16 = 0x0200;
pub const PTR_FLAGS_WHEEL_NEGATIVE: u16 = 0x0100;
pub const WHEEL_ROTATION_DELTA: u16 = 120; // 0x0078 standard Windows WHEEL_DELTA
pub const PTR_FLAGS_MOVE: u16 = 0x0800;
pub const PTR_FLAGS_DOWN: u16 = 0x8000;
pub const PTR_FLAGS_BUTTON1: u16 = 0x1000; // Left button
pub const PTR_FLAGS_BUTTON2: u16 = 0x2000; // Right button
pub const PTR_FLAGS_BUTTON3: u16 = 0x4000; // Middle button

pub const KBD_FLAGS_RELEASE: u16 = 0x8000;
pub const KBD_FLAGS_DOWN: u16 = 0x4000;

#[derive(Default, Clone)]
pub struct FrameBuffer {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub pixels: Vec<u8>,
}

pub type FrameNotifier = Arc<dyn Fn() + Send + Sync + 'static>;
pub type StatusNotifier = Arc<dyn Fn(i32, String) + Send + Sync + 'static>;
pub type AuthNotifier = Arc<dyn Fn(String) + Send + Sync + 'static>;
pub type ClipboardNotifier = Arc<dyn Fn(String) + Send + Sync + 'static>;

struct BridgeUserData {
    frame_buffer: Arc<Mutex<FrameBuffer>>,
    on_frame_ready: FrameNotifier,
    on_auth_url: Option<AuthNotifier>,
    on_status: Option<StatusNotifier>,
    on_clipboard: Option<ClipboardNotifier>,
}

#[repr(C)]
struct RdpBridgeContext {
    _unused: [u8; 0],
}

type RdpBridgeFrameCallback =
    extern "C" fn(user_data: *mut c_void, pixels: *const u8, width: u32, height: u32, stride: u32);
type RdpBridgeAuthCallback = extern "C" fn(user_data: *mut c_void, auth_url: *const c_char);
type RdpBridgeStatusCallback =
    extern "C" fn(user_data: *mut c_void, status_code: i32, message: *const c_char);
type RdpBridgeClipboardCallback = extern "C" fn(user_data: *mut c_void, text: *const c_char);

extern "C" {
    fn freerdp_bridge_create(
        args: *const *const c_char,
        argc: i32,
        on_frame: RdpBridgeFrameCallback,
        on_auth: RdpBridgeAuthCallback,
        on_status: RdpBridgeStatusCallback,
        on_clipboard: RdpBridgeClipboardCallback,
        user_data: *mut c_void,
    ) -> *mut RdpBridgeContext;

    fn freerdp_bridge_start(ctx: *mut RdpBridgeContext) -> bool;
    fn freerdp_bridge_stop(ctx: *mut RdpBridgeContext);
    fn freerdp_bridge_free(ctx: *mut RdpBridgeContext);

    fn freerdp_bridge_provide_auth_result(ctx: *mut RdpBridgeContext, redirect_url: *const c_char);

    fn freerdp_bridge_send_mouse(ctx: *mut RdpBridgeContext, flags: u16, x: u16, y: u16);
    fn freerdp_bridge_send_wheel(ctx: *mut RdpBridgeContext, flags: u16, x: u16, y: u16);
    fn freerdp_bridge_send_key(ctx: *mut RdpBridgeContext, down: bool, keycode: u32);
    fn freerdp_bridge_send_keyboard(ctx: *mut RdpBridgeContext, flags: u16, code: u8);
    fn freerdp_bridge_send_unicode(ctx: *mut RdpBridgeContext, flags: u16, code: u16);
    fn freerdp_bridge_send_resize(ctx: *mut RdpBridgeContext, width: u32, height: u32, desktop_scale: u32);
    fn freerdp_bridge_send_clipboard_text(ctx: *mut RdpBridgeContext, text: *const c_char);
}

extern "C" fn c_frame_callback(
    user_data: *mut c_void,
    pixels: *const u8,
    width: u32,
    height: u32,
    stride: u32,
) {
    if user_data.is_null() || pixels.is_null() || width == 0 || height == 0 {
        return;
    }
    let data = unsafe { &*(user_data as *const BridgeUserData) };
    let size = (stride * height) as usize;
    let slice = unsafe { std::slice::from_raw_parts(pixels, size) };

    if let Ok(mut fb) = data.frame_buffer.lock() {
        fb.width = width;
        fb.height = height;
        fb.stride = stride;
        fb.pixels.clear();
        fb.pixels.extend_from_slice(slice);
    }

    (data.on_frame_ready)();
}

extern "C" fn c_auth_callback(user_data: *mut c_void, auth_url: *const c_char) {
    if user_data.is_null() || auth_url.is_null() {
        return;
    }
    let data = unsafe { &*(user_data as *const BridgeUserData) };
    if let Some(ref cb) = data.on_auth_url {
        let url = unsafe { CStr::from_ptr(auth_url) }
            .to_string_lossy()
            .to_string();
        cb(url);
    }
}

extern "C" fn c_status_callback(user_data: *mut c_void, status_code: i32, message: *const c_char) {
    if user_data.is_null() {
        return;
    }
    let data = unsafe { &*(user_data as *const BridgeUserData) };
    if let Some(ref cb) = data.on_status {
        let msg = if !message.is_null() {
            unsafe { CStr::from_ptr(message) }
                .to_string_lossy()
                .to_string()
        } else {
            String::new()
        };
        cb(status_code, msg);
    }
}

extern "C" fn c_clipboard_callback(user_data: *mut c_void, text: *const c_char) {
    if user_data.is_null() || text.is_null() {
        return;
    }
    let data = unsafe { &*(user_data as *const BridgeUserData) };
    if let Some(ref cb) = data.on_clipboard {
        let t = unsafe { CStr::from_ptr(text) }
            .to_string_lossy()
            .to_string();
        cb(t);
    }
}


pub struct NativeRdpBridge {
    raw_ctx: *mut RdpBridgeContext,
    _user_data: Box<BridgeUserData>,
    pub frame_buffer: Arc<Mutex<FrameBuffer>>,
}

// Safety: The raw pointer is managed exclusively by NativeRdpBridge and FreeRDP handles its own thread safety.
unsafe impl Send for NativeRdpBridge {}
unsafe impl Sync for NativeRdpBridge {}

impl NativeRdpBridge {
    pub fn new(
        cli_args: &[String],
        on_frame_ready: FrameNotifier,
        on_auth_url: Option<AuthNotifier>,
        on_status: Option<StatusNotifier>,
        on_clipboard: Option<ClipboardNotifier>,
    ) -> Option<Self> {
        let mut final_args = Vec::new();
        // If the first argument is not "freerdp" / program name, prepend "freerdp" as argv[0]
        if cli_args.is_empty() || cli_args[0].starts_with('/') {
            final_args.push("freerdp".to_string());
        }
        final_args.extend(cli_args.iter().cloned());

        let cstrings: Vec<CString> = final_args
            .iter()
            .map(|arg| CString::new(arg.as_str()).unwrap_or_default())
            .collect();
        let c_ptrs: Vec<*const c_char> = cstrings.iter().map(|cs| cs.as_ptr()).collect();

        let frame_buffer = Arc::new(Mutex::new(FrameBuffer::default()));
        let mut user_data = Box::new(BridgeUserData {
            frame_buffer: Arc::clone(&frame_buffer),
            on_frame_ready,
            on_auth_url,
            on_status,
            on_clipboard,
        });

        let raw_ctx = unsafe {
            freerdp_bridge_create(
                c_ptrs.as_ptr(),
                c_ptrs.len() as i32,
                c_frame_callback,
                c_auth_callback,
                c_status_callback,
                c_clipboard_callback,
                &mut *user_data as *mut BridgeUserData as *mut c_void,
            )
        };

        if raw_ctx.is_null() {
            return None;
        }

        Some(Self {
            raw_ctx,
            _user_data: user_data,
            frame_buffer,
        })
    }

    pub fn start(&self) -> bool {
        if self.raw_ctx.is_null() {
            return false;
        }
        unsafe { freerdp_bridge_start(self.raw_ctx) }
    }

    pub fn stop(&self) {
        if !self.raw_ctx.is_null() {
            unsafe { freerdp_bridge_stop(self.raw_ctx) }
        }
    }

    pub fn provide_auth_result(&self, redirect_url: &str) {
        if !self.raw_ctx.is_null() {
            let c_url = CString::new(redirect_url).unwrap_or_default();
            unsafe {
                freerdp_bridge_provide_auth_result(self.raw_ctx, c_url.as_ptr());
            }
        }
    }

    pub fn send_mouse(&self, flags: u16, x: u16, y: u16) {
        if !self.raw_ctx.is_null() {
            unsafe { freerdp_bridge_send_mouse(self.raw_ctx, flags, x, y) }
        }
    }

    pub fn send_wheel(&self, flags: u16, x: u16, y: u16) {
        if !self.raw_ctx.is_null() {
            unsafe { freerdp_bridge_send_wheel(self.raw_ctx, flags, x, y) }
        }
    }

    pub fn send_key(&self, down: bool, keycode: u32) {
        if !self.raw_ctx.is_null() {
            unsafe { freerdp_bridge_send_key(self.raw_ctx, down, keycode) }
        }
    }

    pub fn send_keyboard(&self, flags: u16, code: u8) {
        if !self.raw_ctx.is_null() {
            unsafe { freerdp_bridge_send_keyboard(self.raw_ctx, flags, code) }
        }
    }

    pub fn send_unicode(&self, flags: u16, code: u16) {
        if !self.raw_ctx.is_null() {
            unsafe { freerdp_bridge_send_unicode(self.raw_ctx, flags, code) }
        }
    }

    pub fn send_resize(&self, width: u32, height: u32, desktop_scale: u32) {
        if !self.raw_ctx.is_null() {
            unsafe { freerdp_bridge_send_resize(self.raw_ctx, width, height, desktop_scale) }
        }
    }

    pub fn send_clipboard_text(&self, text: &str) {
        if !self.raw_ctx.is_null() {
            let c_text = CString::new(text).unwrap_or_default();
            unsafe {
                freerdp_bridge_send_clipboard_text(self.raw_ctx, c_text.as_ptr());
            }
        }
    }

}

impl Drop for NativeRdpBridge {
    fn drop(&mut self) {
        if !self.raw_ctx.is_null() {
            unsafe {
                freerdp_bridge_free(self.raw_ctx);
            }
            self.raw_ctx = std::ptr::null_mut();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_bridge_creation_with_args() {
        let _guard = TEST_LOCK.lock().unwrap();
        let args = vec!["/v:127.0.0.1".to_string(), "/cert:ignore".to_string()];
        let bridge = NativeRdpBridge::new(&args, Arc::new(|| {}), None, None, None);
        assert!(
            bridge.is_some(),
            "Bridge creation should succeed with valid arguments"
        );
    }

    #[test]
    fn test_bridge_creation_with_gateway_token() {
        let _guard = TEST_LOCK.lock().unwrap();
        let args = vec![
            "/v:127.0.0.1".to_string(),
            "/gateway:type:arm,bearer:test_token_avd_bearer_12345".to_string(),
            "/cert:ignore".to_string(),
        ];
        let bridge = NativeRdpBridge::new(&args, Arc::new(|| {}), None, None, None);
        assert!(
            bridge.is_some(),
            "Bridge creation should succeed with gateway bearer token"
        );
    }
}
