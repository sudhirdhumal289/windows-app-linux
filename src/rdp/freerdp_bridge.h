#ifndef FREERDP_BRIDGE_H
#define FREERDP_BRIDGE_H

#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

/*
 * Thin C glue around FreeRDP 3. Only code that needs FreeRDP's C API lives here
 * (context setup, event loop, channel wiring, input injection). OAuth URL and token
 * request construction, clipboard encoding (text, bitmaps, file lists), keyboard mapping
 * and argument handling are safe Rust (src/rdp/).
 *
 * Threading: FreeRDP runs on its own threads, and every callback below is invoked from a
 * FreeRDP thread: the session thread (status, frames, auth, certificates, network) or the
 * clipboard channel thread (clipboard and file callbacks; on_file_contents also comes from
 * the session thread when a request cannot be sent). The
 * freerdp_bridge_* functions may be called from any thread. Work that FreeRDP channels
 * expect on the session thread (monitor layout, clipboard format list, file contents
 * requests) is queued and performed there.
 *
 * Blocking prompts: on_auth and on_certificate block the session thread until the
 * frontend answers (freerdp_bridge_provide_token_request / freerdp_bridge_answer_certificate)
 * or the session is stopped; the callbacks themselves must return immediately.
 *
 * Synchronous data callbacks: on_clipboard_request and on_file_request are called on the
 * clipboard channel thread and must fill the C-allocated buffer before returning.
 */

typedef enum {
    RDP_BRIDGE_STATUS_FAILED = -1,   /* connection attempt failed; message has the reason */
    RDP_BRIDGE_STATUS_PROGRESS = 0,  /* informational progress message */
    RDP_BRIDGE_STATUS_CONNECTED = 1, /* session is active (also after an auto-reconnect) */
    RDP_BRIDGE_STATUS_ENDED = 2,     /* session closed; message has the reason */
} RdpBridgeStatus;

/* The GDI surface changed. `pixels` is the full BGRX32 surface (`stride * height` bytes);
 * (x, y, w, h) is the region that changed since the previous call. */
typedef void (*RdpBridgeFrameCallback)(void* user_data, const uint8_t* pixels, uint32_t width,
                                       uint32_t height, uint32_t stride, int32_t x, int32_t y,
                                       int32_t w, int32_t h);
/* AAD token needed. `client_id` is NULL when not configured; `req_cnf` may be NULL.
 * Answer with freerdp_bridge_provide_token_request(). */
typedef void (*RdpBridgeAuthCallback)(void* user_data, const char* client_id, const char* scope,
                                      const char* req_cnf);
/* `status_code` is an RdpBridgeStatus. */
typedef void (*RdpBridgeStatusCallback)(void* user_data, int status_code, const char* message);

/* Remote clipboard content passed to on_clipboard. */
typedef enum {
    RDP_BRIDGE_CLIP_CHANGED = 0,   /* the host announced new clipboard content (data is NULL;
                                    * earlier remote content, file lists included, is stale */
    RDP_BRIDGE_CLIP_TEXT = 1,      /* CF_UNICODETEXT (UTF-16LE) */
    RDP_BRIDGE_CLIP_DIB = 2,       /* CF_DIB */
    RDP_BRIDGE_CLIP_DIBV5 = 3,     /* CF_DIBV5 */
    RDP_BRIDGE_CLIP_FILE_LIST = 4, /* CLIPRDR_FILELIST ("FileGroupDescriptorW") */
} RdpBridgeClipKind;

/* Local clipboard content announced with freerdp_bridge_announce_clipboard(). */
typedef enum {
    RDP_BRIDGE_LOCAL_NONE = 0,
    RDP_BRIDGE_LOCAL_TEXT = 1,  /* CF_UNICODETEXT + CF_TEXT */
    RDP_BRIDGE_LOCAL_IMAGE = 2, /* CF_DIB */
    RDP_BRIDGE_LOCAL_FILES = 3, /* RDP_BRIDGE_FILE_LIST_FORMAT_ID */
} RdpBridgeLocalClip;

/* Client format id of the registered "FileGroupDescriptorW" format for local files. */
#define RDP_BRIDGE_FILE_LIST_FORMAT_ID 0xC0B0u

/* Remote clipboard content; `kind` is an RdpBridgeClipKind. */
typedef void (*RdpBridgeClipboardCallback)(void* user_data, int kind, const uint8_t* data,
                                           size_t len);
/* Local clipboard data for `format_id`. Returns the required size; writes into `buffer`
 * only when `buffer_len` is large enough. Returns 0 when no data is available. */
typedef size_t (*RdpBridgeClipboardRequestCallback)(void* user_data, uint32_t format_id,
                                                    uint8_t* buffer, size_t buffer_len);
/* The host reads a file of the local file list (index `list_index` of the list last
 * returned for RDP_BRIDGE_FILE_LIST_FORMAT_ID). `flags` is FILECONTENTS_SIZE (write the
 * size as 8 bytes, little endian) or FILECONTENTS_RANGE (up to `buffer_len` bytes at
 * `position`). Returns the number of bytes written, or -1 on failure. */
typedef int64_t (*RdpBridgeFileRequestCallback)(void* user_data, uint32_t list_index,
                                                uint32_t flags, uint64_t position,
                                                uint8_t* buffer, size_t buffer_len);
/* Answer to freerdp_bridge_request_file_contents(). `data` is NULL when `ok` is false. */
typedef void (*RdpBridgeFileContentsCallback)(void* user_data, uint32_t stream_id, bool ok,
                                              const uint8_t* data, size_t len);

/* Server certificate that could not be trusted automatically. Strings are never NULL
 * except `old_fingerprint`, which is set only when `changed` is true. */
typedef struct {
    const char* host;
    uint16_t port;
    const char* common_name;
    const char* subject;
    const char* issuer;
    const char* fingerprint;
    bool changed; /* differs from the certificate trusted before */
    const char* old_fingerprint;
} RdpBridgeCertificate;

/* Answers to on_certificate (also FreeRDP's VerifyCertificateEx return values). */
#define RDP_BRIDGE_CERT_REJECT 0
#define RDP_BRIDGE_CERT_ACCEPT 1      /* accept and remember */
#define RDP_BRIDGE_CERT_ACCEPT_ONCE 2 /* accept for this session only */

/* Answer with freerdp_bridge_answer_certificate(); rejected after 5 minutes. */
typedef void (*RdpBridgeCertificateCallback)(void* user_data, const RdpBridgeCertificate* cert);

/* Network characteristics measured with the host (connect-time and continuous
 * auto-detection); 0 means not measured. Called when the values change. */
typedef void (*RdpBridgeNetworkCallback)(void* user_data, uint32_t average_rtt_ms,
                                         uint32_t base_rtt_ms, uint32_t bandwidth_kbps);

/* Remote mouse pointer shapes */
typedef enum {
    RDP_BRIDGE_CURSOR_DEFAULT = 0,
    RDP_BRIDGE_CURSOR_HIDDEN = 1,
    RDP_BRIDGE_CURSOR_IBEAM = 2,
    RDP_BRIDGE_CURSOR_HAND = 3,
    RDP_BRIDGE_CURSOR_WAIT = 4,
    RDP_BRIDGE_CURSOR_CROSS = 5,
    RDP_BRIDGE_CURSOR_RESIZE_NS = 6,
    RDP_BRIDGE_CURSOR_RESIZE_WE = 7,
    RDP_BRIDGE_CURSOR_RESIZE_NWSE = 8,
    RDP_BRIDGE_CURSOR_RESIZE_NESW = 9,
    RDP_BRIDGE_CURSOR_MOVE = 10,
    RDP_BRIDGE_CURSOR_NOT_ALLOWED = 11,
} RdpBridgeCursorKind;

typedef void (*RdpBridgeCursorCallback)(void* user_data, int cursor_kind);
typedef void (*RdpBridgeCursorBitmapCallback)(void* user_data, const uint8_t* rgba,
                                             uint32_t width, uint32_t height,
                                             uint32_t hot_x, uint32_t hot_y);
typedef void (*RdpBridgePointerPositionCallback)(void* user_data, uint32_t x, uint32_t y);

typedef struct {
    RdpBridgeFrameCallback on_frame;
    RdpBridgeAuthCallback on_auth;
    RdpBridgeStatusCallback on_status;
    RdpBridgeClipboardCallback on_clipboard;
    RdpBridgeClipboardRequestCallback on_clipboard_request;
    RdpBridgeNetworkCallback on_network;
    RdpBridgeCertificateCallback on_certificate;
    RdpBridgeFileRequestCallback on_file_request;
    RdpBridgeFileContentsCallback on_file_contents;
    RdpBridgeCursorCallback on_cursor;
    RdpBridgeCursorBitmapCallback on_cursor_bitmap;
    RdpBridgePointerPositionCallback on_pointer_position;
    /* Passed to every callback; must stay valid until freerdp_bridge_free() returns. */
    void* user_data;
} RdpBridgeCallbacks;

typedef struct RdpBridgeContext RdpBridgeContext;

/* Most monitors a layout may contain (MS-RDPEDISP allows 16). */
#define RDP_BRIDGE_MAX_MONITORS 16

/* One monitor of a remote layout, in device pixels. */
typedef struct {
    int32_t left;
    int32_t top;
    uint32_t width;
    uint32_t height;
    bool primary;
} RdpBridgeMonitor;

/* FreeRDP library version string. */
const char* freerdp_bridge_version(void);

/* FreeRDP build configuration (space separated `OPTION=VALUE` entries). */
const char* freerdp_bridge_build_config(void);

/* Whether this FreeRDP build's command line accepts `option` (e.g. "/usb:auto"); options
 * of channels that were not compiled in are unknown and would fail the whole session. */
bool freerdp_bridge_accepts_option(const char* option);

/* Whether this FreeRDP build was compiled with camera redirection support [MS-RDPECAM]. */
bool freerdp_bridge_supports_camera(void);

/* Parses `args` (argv[0] included) like the FreeRDP command line. Returns NULL on failure. */
RdpBridgeContext* freerdp_bridge_create(const char* const* args, int argc,
                                        const char* gateway_token,
                                        const RdpBridgeCallbacks* callbacks);

/* Starts the session thread. Can be called once. */
bool freerdp_bridge_start(RdpBridgeContext* ctx);
/* Disconnects and blocks until the session thread has exited. Idempotent. */
void freerdp_bridge_stop(RdpBridgeContext* ctx);
/* Stops the session (blocking) and releases everything. No callbacks fire afterwards. */
void freerdp_bridge_free(RdpBridgeContext* ctx);

/* `token_request` is the x-www-form-urlencoded token request body, or NULL to fail auth. */
void freerdp_bridge_provide_token_request(RdpBridgeContext* ctx, const char* token_request);

/* `decision` is an RDP_BRIDGE_CERT_* value answering the pending on_certificate. */
void freerdp_bridge_answer_certificate(RdpBridgeContext* ctx, int decision);

/* Input is dropped unless the session is connected. */
void freerdp_bridge_send_mouse(RdpBridgeContext* ctx, uint16_t flags, uint16_t x, uint16_t y);
void freerdp_bridge_send_extended_mouse(RdpBridgeContext* ctx, uint16_t flags, uint16_t x,
                                        uint16_t y);
/* `rdp_scancode` is a FreeRDP RDP_SCANCODE_* value (scancode | KBDEXT). */
void freerdp_bridge_send_scancode(RdpBridgeContext* ctx, bool down, bool repeat,
                                  uint32_t rdp_scancode);
/* Synchronizes lock keys (KBD_SYNC_* flags) and releases modifiers, e.g. on focus-in. */
void freerdp_bridge_send_focus_in(RdpBridgeContext* ctx, uint16_t lock_flags);
/* Sends one UTF-16 code unit as a Unicode keyboard event. */
void freerdp_bridge_send_unicode(RdpBridgeContext* ctx, bool down, uint16_t code_unit);

/* Requests a new remote monitor layout (Display Control channel); coalesced. */
void freerdp_bridge_request_resize(RdpBridgeContext* ctx, uint32_t width, uint32_t height,
                                   uint32_t desktop_scale, uint32_t device_scale);
/* Requests a multi-monitor layout. Positions are relative to the primary monitor (moved to
 * 0,0 if needed); extra monitors are dropped when the host supports fewer. The remote
 * desktop becomes the bounding box of all monitors. */
void freerdp_bridge_request_layout(RdpBridgeContext* ctx, const RdpBridgeMonitor* monitors,
                                   uint32_t count, uint32_t desktop_scale,
                                   uint32_t device_scale);
/* Announces new local clipboard content (`kind` is an RdpBridgeLocalClip); the host pulls
 * the data via on_clipboard_request and, for files, on_file_request. */
void freerdp_bridge_announce_clipboard(RdpBridgeContext* ctx, uint32_t kind);
/* Asks the host for remote file contents (CB_FILECONTENTS_REQUEST) of entry `list_index`
 * of the file list last delivered by on_clipboard. `flags` is FILECONTENTS_SIZE (size 8,
 * position 0) or FILECONTENTS_RANGE; `size` is at most 8 MiB. The answer arrives via
 * on_file_contents with the same `stream_id`. Returns false when the clipboard channel is
 * not available or too many requests are queued (on_file_contents is not called then). */
bool freerdp_bridge_request_file_contents(RdpBridgeContext* ctx, uint32_t stream_id,
                                          uint32_t list_index, uint32_t flags,
                                          uint64_t position, uint32_t size);

#ifdef __cplusplus
}
#endif

#endif /* FREERDP_BRIDGE_H */
