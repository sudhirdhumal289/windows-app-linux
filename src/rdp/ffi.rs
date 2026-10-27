//! Raw declarations for `freerdp_bridge.h`. Only [`super::bridge`] uses these.

use std::ffi::{c_char, c_int, c_void};

/// Opaque `RdpBridgeContext`.
#[repr(C)]
pub(super) struct RdpBridgeContext {
    _private: [u8; 0],
}

pub(super) const STATUS_FAILED: c_int = -1;
pub(super) const STATUS_PROGRESS: c_int = 0;
pub(super) const STATUS_CONNECTED: c_int = 1;
pub(super) const STATUS_ENDED: c_int = 2;

pub(super) type FrameCallback = extern "C" fn(
    user_data: *mut c_void,
    pixels: *const u8,
    width: u32,
    height: u32,
    stride: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
);
pub(super) type AuthCallback = extern "C" fn(
    user_data: *mut c_void,
    client_id: *const c_char,
    scope: *const c_char,
    req_cnf: *const c_char,
);
pub(super) type StatusCallback =
    extern "C" fn(user_data: *mut c_void, status_code: c_int, message: *const c_char);
/// `RdpBridgeClipKind`.
pub(super) const CLIP_CHANGED: c_int = 0;
pub(super) const CLIP_TEXT: c_int = 1;
pub(super) const CLIP_DIB: c_int = 2;
pub(super) const CLIP_DIBV5: c_int = 3;
pub(super) const CLIP_FILE_LIST: c_int = 4;

/// `RdpBridgeLocalClip`.
pub(super) const LOCAL_NONE: u32 = 0;
pub(super) const LOCAL_TEXT: u32 = 1;
pub(super) const LOCAL_IMAGE: u32 = 2;
pub(super) const LOCAL_FILES: u32 = 3;

/// `RDP_BRIDGE_CERT_*`.
pub(super) const CERT_REJECT: c_int = 0;
pub(super) const CERT_ACCEPT: c_int = 1;
pub(super) const CERT_ACCEPT_ONCE: c_int = 2;

pub(super) type ClipboardCallback =
    extern "C" fn(user_data: *mut c_void, kind: c_int, data: *const u8, len: usize);
pub(super) type ClipboardRequestCallback = extern "C" fn(
    user_data: *mut c_void,
    format_id: u32,
    buffer: *mut u8,
    buffer_len: usize,
) -> usize;

pub(super) type FileRequestCallback = extern "C" fn(
    user_data: *mut c_void,
    list_index: u32,
    flags: u32,
    position: u64,
    buffer: *mut u8,
    buffer_len: usize,
) -> i64;
pub(super) type FileContentsCallback =
    extern "C" fn(user_data: *mut c_void, stream_id: u32, ok: bool, data: *const u8, len: usize);

/// `RdpBridgeCertificate`. Strings are NUL-terminated and non-null except
/// `old_fingerprint`.
#[repr(C)]
pub(super) struct RdpBridgeCertificate {
    pub host: *const c_char,
    pub port: u16,
    pub common_name: *const c_char,
    pub subject: *const c_char,
    pub issuer: *const c_char,
    pub fingerprint: *const c_char,
    pub changed: bool,
    pub old_fingerprint: *const c_char,
}
pub(super) type CertificateCallback =
    extern "C" fn(user_data: *mut c_void, cert: *const RdpBridgeCertificate);

pub(super) type NetworkCallback = extern "C" fn(
    user_data: *mut c_void,
    average_rtt_ms: u32,
    base_rtt_ms: u32,
    bandwidth_kbps: u32,
);

/// `RDP_BRIDGE_MAX_MONITORS`.
pub(super) const MAX_MONITORS: usize = 16;

/// `RdpBridgeMonitor`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct RdpBridgeMonitor {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

#[repr(C)]
pub(super) struct RdpBridgeCallbacks {
    pub on_frame: FrameCallback,
    pub on_auth: AuthCallback,
    pub on_status: StatusCallback,
    pub on_clipboard: ClipboardCallback,
    pub on_clipboard_request: ClipboardRequestCallback,
    pub on_network: NetworkCallback,
    pub on_certificate: CertificateCallback,
    pub on_file_request: FileRequestCallback,
    pub on_file_contents: FileContentsCallback,
    pub user_data: *mut c_void,
}

extern "C" {
    pub(super) fn freerdp_bridge_version() -> *const c_char;

    pub(super) fn freerdp_bridge_build_config() -> *const c_char;
    pub(super) fn freerdp_bridge_accepts_option(option: *const c_char) -> bool;
    pub(super) fn freerdp_bridge_supports_camera() -> bool;

    pub(super) fn freerdp_bridge_create(
        args: *const *const c_char,
        argc: c_int,
        gateway_token: *const c_char,
        callbacks: *const RdpBridgeCallbacks,
    ) -> *mut RdpBridgeContext;
    pub(super) fn freerdp_bridge_start(ctx: *mut RdpBridgeContext) -> bool;
    pub(super) fn freerdp_bridge_free(ctx: *mut RdpBridgeContext);

    pub(super) fn freerdp_bridge_provide_token_request(
        ctx: *mut RdpBridgeContext,
        token_request: *const c_char,
    );

    pub(super) fn freerdp_bridge_send_mouse(ctx: *mut RdpBridgeContext, flags: u16, x: u16, y: u16);
    pub(super) fn freerdp_bridge_send_extended_mouse(
        ctx: *mut RdpBridgeContext,
        flags: u16,
        x: u16,
        y: u16,
    );
    pub(super) fn freerdp_bridge_send_scancode(
        ctx: *mut RdpBridgeContext,
        down: bool,
        repeat: bool,
        rdp_scancode: u32,
    );
    pub(super) fn freerdp_bridge_send_focus_in(ctx: *mut RdpBridgeContext, lock_flags: u16);
    pub(super) fn freerdp_bridge_send_unicode(
        ctx: *mut RdpBridgeContext,
        down: bool,
        code_unit: u16,
    );
    pub(super) fn freerdp_bridge_request_resize(
        ctx: *mut RdpBridgeContext,
        width: u32,
        height: u32,
        desktop_scale: u32,
        device_scale: u32,
    );
    pub(super) fn freerdp_bridge_request_layout(
        ctx: *mut RdpBridgeContext,
        monitors: *const RdpBridgeMonitor,
        count: u32,
        desktop_scale: u32,
        device_scale: u32,
    );
    pub(super) fn freerdp_bridge_answer_certificate(ctx: *mut RdpBridgeContext, decision: c_int);
    pub(super) fn freerdp_bridge_announce_clipboard(ctx: *mut RdpBridgeContext, kind: u32);
    pub(super) fn freerdp_bridge_request_file_contents(
        ctx: *mut RdpBridgeContext,
        stream_id: u32,
        list_index: u32,
        flags: u32,
        position: u64,
        size: u32,
    ) -> bool;
}
