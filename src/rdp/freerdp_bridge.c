/* POSIX APIs (rwlocks, fseeko in WinPR headers) under strict C11. */
#define _POSIX_C_SOURCE 200809L
/* Fail the build if deprecated FreeRDP 3 APIs (removed in FreeRDP 4) are used. */
#define WITHOUT_FREERDP_3x_DEPRECATED

#include "freerdp_bridge.h"

#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <freerdp/autodetect.h>
#include <freerdp/channels/cliprdr.h>
#include <freerdp/channels/rail.h>
#include <freerdp/client.h>
#include <freerdp/client/cliprdr.h>
#include <freerdp/client/disp.h>
#include <freerdp/client/rail.h>
#include <freerdp/error.h>
#include <freerdp/event.h>
#include <freerdp/freerdp.h>
#include <freerdp/gdi/gdi.h>
#include <freerdp/input.h>
#include <freerdp/scancode.h>
#include <freerdp/settings_keys.h>
#include <winpr/synch.h>
#include <winpr/sysinfo.h>
#include <winpr/thread.h>
#include <winpr/user.h>
#include <winpr/wlog.h>

#if defined(_WIN32)
#include <windows.h>
#else
#include <pthread.h>
#endif

#define TAG "com.windowsapp.bridge"
#if defined(_MSC_VER)
#define BRIDGE_THREAD_LOCAL __declspec(thread)
#else
#define BRIDGE_THREAD_LOCAL _Thread_local
#endif
/* Interactive Entra ID sign-in (MFA included) must finish within this time. */
#define AUTH_TIMEOUT_MS (5 * 60 * 1000)
/* Unanswered certificate prompts are rejected after this time. */
#define CERT_TIMEOUT_MS (5 * 60 * 1000)
#define MAX_MONITOR_SIZE 8192
/* How often network characteristics are reported. */
#define NETWORK_POLL_MS 2000

/* ---- Portable locks ---------------------------------------------------------------------
 * pthreads on POSIX; slim reader/writer locks and one-time init on Windows, where MSVC has
 * no pthreads. Read and write unlocks are separate because SRW locks need the mode. */

#if defined(_WIN32)
typedef SRWLOCK BridgeMutex;
typedef SRWLOCK BridgeRwLock;
typedef INIT_ONCE BridgeOnce;
#define BRIDGE_MUTEX_INITIALIZER SRWLOCK_INIT
#define BRIDGE_ONCE_INITIALIZER INIT_ONCE_STATIC_INIT

static BOOL bridge_mutex_init(BridgeMutex* m) {
    InitializeSRWLock(m);
    return TRUE;
}
static void bridge_mutex_destroy(BridgeMutex* m) { (void)m; }
static void bridge_mutex_lock(BridgeMutex* m) { AcquireSRWLockExclusive(m); }
static void bridge_mutex_unlock(BridgeMutex* m) { ReleaseSRWLockExclusive(m); }
static BOOL bridge_rwlock_init(BridgeRwLock* l) {
    InitializeSRWLock(l);
    return TRUE;
}
static void bridge_rwlock_destroy(BridgeRwLock* l) { (void)l; }
static void bridge_rwlock_read(BridgeRwLock* l) { AcquireSRWLockShared(l); }
static void bridge_rwlock_read_unlock(BridgeRwLock* l) { ReleaseSRWLockShared(l); }
static void bridge_rwlock_write(BridgeRwLock* l) { AcquireSRWLockExclusive(l); }
static void bridge_rwlock_write_unlock(BridgeRwLock* l) { ReleaseSRWLockExclusive(l); }

typedef struct {
    void (*fn)(void);
} BridgeOnceFn;

static BOOL CALLBACK bridge_once_trampoline(PINIT_ONCE once, PVOID param, PVOID* context) {
    (void)once;
    (void)context;
    ((const BridgeOnceFn*)param)->fn();
    return TRUE;
}
static void bridge_once(BridgeOnce* once, void (*fn)(void)) {
    BridgeOnceFn call = { fn };
    InitOnceExecuteOnce(once, bridge_once_trampoline, &call, NULL);
}
#else
typedef pthread_mutex_t BridgeMutex;
typedef pthread_rwlock_t BridgeRwLock;
typedef pthread_once_t BridgeOnce;
#define BRIDGE_MUTEX_INITIALIZER PTHREAD_MUTEX_INITIALIZER
#define BRIDGE_ONCE_INITIALIZER PTHREAD_ONCE_INIT

static BOOL bridge_mutex_init(BridgeMutex* m) { return pthread_mutex_init(m, NULL) == 0; }
static void bridge_mutex_destroy(BridgeMutex* m) { pthread_mutex_destroy(m); }
static void bridge_mutex_lock(BridgeMutex* m) { pthread_mutex_lock(m); }
static void bridge_mutex_unlock(BridgeMutex* m) { pthread_mutex_unlock(m); }
static BOOL bridge_rwlock_init(BridgeRwLock* l) { return pthread_rwlock_init(l, NULL) == 0; }
static void bridge_rwlock_destroy(BridgeRwLock* l) { pthread_rwlock_destroy(l); }
static void bridge_rwlock_read(BridgeRwLock* l) { pthread_rwlock_rdlock(l); }
static void bridge_rwlock_read_unlock(BridgeRwLock* l) { pthread_rwlock_unlock(l); }
static void bridge_rwlock_write(BridgeRwLock* l) { pthread_rwlock_wrlock(l); }
static void bridge_rwlock_write_unlock(BridgeRwLock* l) { pthread_rwlock_unlock(l); }
static void bridge_once(BridgeOnce* once, void (*fn)(void)) { pthread_once(once, fn); }
#endif
/* Pending remote file contents requests (see freerdp_bridge_request_file_contents). */
#define MAX_FILE_REQUESTS 64
/* Largest file contents chunk requested from or sent to the host. */
#define MAX_FILE_CHUNK (8u * 1024u * 1024u)

/* Registered clipboard format of file lists (MS-RDPECLIP 1.3.1.2). */
static char g_file_list_format_name[] = "FileGroupDescriptorW";

typedef struct {
    UINT32 stream_id;
    UINT32 list_index;
    UINT32 flags;
    UINT64 position;
    UINT32 size;
} BridgeFileRequest;

typedef struct {
    rdpClientContext common;

    /* Immutable after freerdp_bridge_create(). */
    RdpBridgeCallbacks cb;
    char* gateway_token;
    HANDLE stop_event;      /* manual reset: session must end */
    HANDLE auth_done_event; /* manual reset: token request provided (or auth cancelled) */
    HANDLE cert_done_event; /* manual reset: certificate prompt answered */
    HANDLE work_event;      /* manual reset: queued work for the session thread */
    HANDLE thread;          /* owned by the caller of start/stop (one thread at a time) */

    BOOL locks_ready;
    BridgeMutex auth_lock;
    char* token_request; /* guarded by auth_lock */
    int cert_answer;     /* guarded by auth_lock; -1 while unanswered */

    /* Input may only be injected while the transport is up; guarded by input_lock. */
    BridgeRwLock input_lock;
    BOOL input_ready;

    /* Channel interfaces and queued work; guarded by work_lock. */
    BridgeMutex work_lock;
    DispClientContext* disp;
    BOOL disp_ready;      /* server sent its Display Control capabilities */
    UINT32 disp_max_monitors;
    CliprdrClientContext* cliprdr;
    BOOL resize_pending;
    DISPLAY_CONTROL_MONITOR_LAYOUT pending_layout[RDP_BRIDGE_MAX_MONITORS];
    UINT32 pending_count;
    DISPLAY_CONTROL_MONITOR_LAYOUT last_layout[RDP_BRIDGE_MAX_MONITORS];
    UINT32 last_count;
    BOOL layout_resent;   /* last_layout was re-sent after the host ignored it */
    BOOL clipboard_pending;
    UINT32 local_clip_kind;          /* RdpBridgeLocalClip announced to the host */
    UINT32 remote_requests_pending;  /* format data requests sent, not yet answered */
    UINT32 remote_request_kind;      /* RdpBridgeClipKind of the latest request */
    BridgeFileRequest file_requests[MAX_FILE_REQUESTS];
    UINT32 file_request_count;

    /* Last reported network characteristics; session thread only. */
    UINT64 network_polled_at;
    UINT32 network_last[3];

    /* Why the last connection attempt was refused (static string); session thread only. */
    const char* failure_hint;
} BridgeClientContext;

struct RdpBridgeContext {
    rdpContext* rdp_ctx;
    BOOL running;
};

static wLog* bridge_log(void) {
    return WLog_Get(TAG);
}

/* The last error FreeRDP logged on this thread. Several connection failures (gateway
 * discovery, TLS setup, ...) only log the cause and leave the error code at "Success". */
static BRIDGE_THREAD_LOCAL char g_last_log_error[256];

static BOOL bridge_log_message(const wLogMessage* msg) {
    const char* prefix = msg->PrefixString ? msg->PrefixString : "";
    const char* text = msg->TextString ? msg->TextString : "";
    (void)fprintf(stderr, "%s%s\n", prefix, text);
    (void)fflush(stderr);
#if defined(_WIN32)
    OutputDebugStringA(prefix);
    OutputDebugStringA(text);
    OutputDebugStringA("\n");
#endif
    if (msg->Level >= WLOG_ERROR && text[0] != '\0') {
        /* If we already captured a specific error on this thread, don't let a generic
         * wrapper message ("error retrieving ARM configuration" or "Failed to check FreeRDP file format")
         * overwrite it and hide the root cause from the user. Instead, prepend the wrapper context. */
        if (g_last_log_error[0] != '\0' &&
            (strstr(text, "error retrieving ARM configuration") != NULL ||
             strstr(text, "Failed to check FreeRDP file format") != NULL)) {
            char prev[256];
            (void)snprintf(prev, sizeof(prev), "%s", g_last_log_error);
            (void)snprintf(g_last_log_error, sizeof(g_last_log_error), "%s: %s", text, prev);
        } else {
            (void)snprintf(g_last_log_error, sizeof(g_last_log_error), "%s", text);
        }
        size_t len = strlen(g_last_log_error);
        while (len > 0 && (g_last_log_error[len - 1] == '\n' || g_last_log_error[len - 1] == '\r')) {
            g_last_log_error[--len] = '\0';
        }
    }
    return TRUE;
}

#if defined(_WIN32)
static void bridge_sync_crt_env(const char* name) {
    char buf[1024];
    DWORD len = GetEnvironmentVariableA(name, buf, sizeof(buf));
    if (len > 0 && len < sizeof(buf)) {
        _putenv_s(name, buf);
    }
}

static void bridge_init_windows_environment(void) {
    bridge_sync_crt_env("SSL_CERT_FILE");
    bridge_sync_crt_env("SSL_CERT_DIR");
    bridge_sync_crt_env("OPENSSL_MODULES");
    bridge_sync_crt_env("OPENSSL_CONF");
    bridge_sync_crt_env("WLOG_LEVEL");
    bridge_sync_crt_env("WLOG_APPENDER");
    bridge_sync_crt_env("RDP_DEBUG");

    /* If SSL_CERT_FILE is still not set in CRT, check if the app's CA bundle exists */
    if (!getenv("SSL_CERT_FILE") && !getenv("SSL_CERT_DIR")) {
        char local_app_data[MAX_PATH];
        DWORD len = GetEnvironmentVariableA("LOCALAPPDATA", local_app_data, sizeof(local_app_data));
        if (len > 0 && len < sizeof(local_app_data)) {
            char ca_path[MAX_PATH + 32];
            (void)snprintf(ca_path, sizeof(ca_path), "%s\\windows-app\\ca-bundle.crt", local_app_data);
            DWORD attr = GetFileAttributesA(ca_path);
            if (attr != INVALID_FILE_ATTRIBUTES && !(attr & FILE_ATTRIBUTE_DIRECTORY)) {
                _putenv_s("SSL_CERT_FILE", ca_path);
            }
        }
    }

    /* Ensure OPENSSL_MODULES is populated next to the executable if not already set */
    if (!getenv("OPENSSL_MODULES")) {
        char exe_path[MAX_PATH];
        DWORD len = GetModuleFileNameA(NULL, exe_path, sizeof(exe_path));
        if (len > 0 && len < sizeof(exe_path)) {
            char* last_slash = strrchr(exe_path, '\\');
            if (last_slash) {
                *last_slash = '\0';
                char modules_path[MAX_PATH + 32];
                (void)snprintf(modules_path, sizeof(modules_path), "%s\\ossl-modules", exe_path);
                DWORD attr = GetFileAttributesA(modules_path);
                if (attr != INVALID_FILE_ATTRIBUTES && (attr & FILE_ATTRIBUTE_DIRECTORY)) {
                    _putenv_s("OPENSSL_MODULES", modules_path);
                } else {
                    char legacy_dll[MAX_PATH + 32];
                    (void)snprintf(legacy_dll, sizeof(legacy_dll), "%s\\legacy.dll", exe_path);
                    DWORD lattr = GetFileAttributesA(legacy_dll);
                    if (lattr != INVALID_FILE_ATTRIBUTES && !(lattr & FILE_ATTRIBUTE_DIRECTORY)) {
                        _putenv_s("OPENSSL_MODULES", exe_path);
                    }
                }
            }
        }
    }
}
#endif

static void bridge_init_logging(void) {
#if defined(_WIN32)
    bridge_init_windows_environment();
#endif
    wLog* root = WLog_GetRoot();
    if (root && !getenv("WLOG_LEVEL")) {
        WLog_SetLogLevel(root, WLOG_WARN);
    }
    /* Keep console output, but also remember errors (unless the user picked an appender). */
    if (root && !getenv("WLOG_APPENDER") && WLog_SetLogAppenderType(root, WLOG_APPENDER_CALLBACK)) {
        wLogCallbacks callbacks = { 0 };
        callbacks.message = bridge_log_message;
        wLogAppender* appender = WLog_GetLogAppender(root);
        if (!appender || !WLog_ConfigureAppender(appender, "callbacks", &callbacks)) {
            WLog_SetLogAppenderType(root, WLOG_APPENDER_CONSOLE);
        }
    }
    if (getenv("RDP_DEBUG")) {
        WLog_SetLogLevel(bridge_log(), WLOG_DEBUG);
        if (root) {
            WLog_SetLogLevel(root, WLOG_DEBUG);
        }
    }
}

static void bridge_status(BridgeClientContext* bctx, RdpBridgeStatus status, const char* message) {
    if (bctx->cb.on_status) {
        bctx->cb.on_status(bctx->cb.user_data, (int)status, message ? message : "");
    }
}

static void bridge_free_secret(char* str) {
    if (!str) return;
    memset(str, 0, strlen(str));
    free(str);
}

static char* bridge_take_token_request(BridgeClientContext* bctx) {
    bridge_mutex_lock(&bctx->auth_lock);
    char* req = bctx->token_request;
    bctx->token_request = NULL;
    bridge_mutex_unlock(&bctx->auth_lock);
    return req;
}

static void bridge_set_input_ready(BridgeClientContext* bctx, BOOL ready) {
    bridge_rwlock_write(&bctx->input_lock);
    bctx->input_ready = ready;
    bridge_rwlock_write_unlock(&bctx->input_lock);
}

/* Returns the input interface with the read lock held, or NULL (lock released). */
static rdpInput* bridge_input_begin(RdpBridgeContext* ctx) {
    if (!ctx || !ctx->rdp_ctx) return NULL;
    BridgeClientContext* bctx = (BridgeClientContext*)ctx->rdp_ctx;
    bridge_rwlock_read(&bctx->input_lock);
    if (bctx->input_ready && ctx->rdp_ctx->input) {
        return ctx->rdp_ctx->input;
    }
    bridge_rwlock_read_unlock(&bctx->input_lock);
    return NULL;
}

static void bridge_input_end(RdpBridgeContext* ctx) {
    bridge_rwlock_read_unlock(&((BridgeClientContext*)ctx->rdp_ctx)->input_lock);
}

/* ---- Authentication -------------------------------------------------------------------- */

static BOOL bridge_get_access_token(freerdp* instance, AccessTokenType tokenType, char** token,
                                    size_t count, ...) {
    if (!instance || !instance->context || !token) return FALSE;
    BridgeClientContext* bctx = (BridgeClientContext*)instance->context;
    rdpSettings* settings = instance->context->settings;

    if (tokenType == ACCESS_TOKEN_TYPE_AVD) {
        const char* gw_token = freerdp_settings_get_string(settings, FreeRDP_GatewayAccessToken);
        if (!gw_token || !gw_token[0]) {
            gw_token = bctx->gateway_token;
        }
        if (!gw_token || !gw_token[0]) {
            WLog_Print(bridge_log(), WLOG_WARN, "No AVD gateway token available");
            return FALSE;
        }
        *token = _strdup(gw_token);
        return *token != NULL;
    }

    if (tokenType != ACCESS_TOKEN_TYPE_AAD) {
        WLog_Print(bridge_log(), WLOG_WARN, "Unsupported access token type %d", (int)tokenType);
        return FALSE;
    }

    va_list ap;
    va_start(ap, count);
    const char* scope = (count >= 1) ? va_arg(ap, const char*) : NULL;
    const char* req_cnf = (count >= 2) ? va_arg(ap, const char*) : NULL;
    va_end(ap);
    if (!scope || !bctx->cb.on_auth) return FALSE;

    const char* client_id = freerdp_settings_get_string(settings, FreeRDP_GatewayAvdClientID);
    if (client_id && !client_id[0]) {
        client_id = NULL;
    }

    bridge_status(bctx, RDP_BRIDGE_STATUS_PROGRESS, "Signing in to the session host...");

    /* Rust builds the authorize URL, drives the sign-in UI and hands back the request body. */
    bridge_free_secret(bridge_take_token_request(bctx));
    ResetEvent(bctx->auth_done_event);
    bctx->cb.on_auth(bctx->cb.user_data, client_id, scope, req_cnf);

    HANDLE events[2] = { bctx->auth_done_event, bctx->stop_event };
    DWORD wait = WaitForMultipleObjects(2, events, FALSE, AUTH_TIMEOUT_MS);
    char* request = bridge_take_token_request(bctx);
    if (wait != WAIT_OBJECT_0 || !request) {
        WLog_Print(bridge_log(), WLOG_WARN, "Session host sign-in cancelled or timed out");
        bridge_free_secret(request);
        return FALSE;
    }

    BOOL rc = client_common_get_access_token(instance, request, token);
    bridge_free_secret(request);
    return rc;
}

/* The bridge never prompts on a terminal: credentials must come from the command line. */
static BOOL bridge_authenticate_ex(freerdp* instance, char** username, char** password,
                                   char** domain, rdp_auth_reason reason) {
    (void)domain;
    if (username && *username && **username && password && *password) {
        return TRUE;
    }
    WLog_Print(bridge_log(), WLOG_WARN, "Credentials requested (reason %d); not supported",
               (int)reason);
    if (instance && instance->context) {
        BridgeClientContext* bctx = (BridgeClientContext*)instance->context;
        bctx->failure_hint = "The remote computer requires a username and password";
        bridge_status(bctx, RDP_BRIDGE_STATUS_PROGRESS, bctx->failure_hint);
    }
    return FALSE;
}

/* `message` must be a static string (kept as the failure hint). */
static DWORD bridge_reject_certificate(freerdp* instance, const char* host, UINT16 port,
                                       const char* message) {
    WLog_Print(bridge_log(), WLOG_ERROR, "Rejecting certificate for %s:%u: %s",
               host ? host : "?", (unsigned)port, message);
    if (instance && instance->context) {
        BridgeClientContext* bctx = (BridgeClientContext*)instance->context;
        bctx->failure_hint = message;
        bridge_status(bctx, RDP_BRIDGE_STATUS_PROGRESS, message);
    }
    return 0;
}

/* Asks the frontend whether to trust `cert` and blocks until it answers, the session is
 * stopped or the prompt times out. `reject_hint` must be a static string. */
static DWORD bridge_ask_certificate(freerdp* instance, const RdpBridgeCertificate* cert,
                                    const char* reject_hint) {
    if (!instance || !instance->context) return 0;
    BridgeClientContext* bctx = (BridgeClientContext*)instance->context;
    if (!bctx->cb.on_certificate) {
        return bridge_reject_certificate(instance, cert->host, cert->port, reject_hint);
    }

    bridge_mutex_lock(&bctx->auth_lock);
    bctx->cert_answer = -1;
    bridge_mutex_unlock(&bctx->auth_lock);
    ResetEvent(bctx->cert_done_event);
    bridge_status(bctx, RDP_BRIDGE_STATUS_PROGRESS, "Verifying the remote computer's certificate...");
    bctx->cb.on_certificate(bctx->cb.user_data, cert);

    HANDLE events[2] = { bctx->cert_done_event, bctx->stop_event };
    DWORD wait = WaitForMultipleObjects(2, events, FALSE, CERT_TIMEOUT_MS);
    bridge_mutex_lock(&bctx->auth_lock);
    int answer = bctx->cert_answer;
    bctx->cert_answer = -1;
    bridge_mutex_unlock(&bctx->auth_lock);

    if (wait == WAIT_OBJECT_0 &&
        (answer == RDP_BRIDGE_CERT_ACCEPT || answer == RDP_BRIDGE_CERT_ACCEPT_ONCE)) {
        WLog_Print(bridge_log(), WLOG_INFO, "Certificate for %s:%u accepted (%s)",
                   cert->host, (unsigned)cert->port,
                   answer == RDP_BRIDGE_CERT_ACCEPT ? "permanently" : "this session");
        return (DWORD)answer;
    }
    if (wait != WAIT_OBJECT_0) {
        WLog_Print(bridge_log(), WLOG_WARN, "Certificate prompt cancelled or timed out");
    }
    return bridge_reject_certificate(instance, cert->host, cert->port, reject_hint);
}

static DWORD bridge_verify_certificate(freerdp* instance, const char* host, UINT16 port,
                                       const char* common_name, const char* subject,
                                       const char* issuer, const char* fingerprint, DWORD flags) {
    (void)flags;
    RdpBridgeCertificate cert = {
        .host = host ? host : "",
        .port = port,
        .common_name = common_name ? common_name : "",
        .subject = subject ? subject : "",
        .issuer = issuer ? issuer : "",
        .fingerprint = fingerprint ? fingerprint : "",
        .changed = false,
        .old_fingerprint = NULL,
    };
    return bridge_ask_certificate(instance, &cert,
                                  "The remote computer's certificate is not trusted");
}

static DWORD bridge_verify_changed_certificate(freerdp* instance, const char* host, UINT16 port,
                                               const char* common_name, const char* subject,
                                               const char* issuer, const char* new_fingerprint,
                                               const char* old_subject, const char* old_issuer,
                                               const char* old_fingerprint, DWORD flags) {
    (void)old_subject; (void)old_issuer; (void)flags;
    RdpBridgeCertificate cert = {
        .host = host ? host : "",
        .port = port,
        .common_name = common_name ? common_name : "",
        .subject = subject ? subject : "",
        .issuer = issuer ? issuer : "",
        .fingerprint = new_fingerprint ? new_fingerprint : "",
        .changed = true,
        .old_fingerprint = old_fingerprint ? old_fingerprint : "",
    };
    return bridge_ask_certificate(
        instance, &cert, "The remote computer's certificate changed since the last connection");
}

/* ---- Graphics -------------------------------------------------------------------------- */

static HGDI_WND bridge_primary_hwnd(rdpGdi* gdi) {
    if (!gdi || !gdi->primary || !gdi->primary->hdc) return NULL;
    return gdi->primary->hdc->hwnd;
}

static void bridge_emit_frame(BridgeClientContext* bctx, rdpGdi* gdi, INT32 x, INT32 y, INT32 w,
                              INT32 h) {
    if (bctx->cb.on_frame && gdi->primary_buffer) {
        bctx->cb.on_frame(bctx->cb.user_data, gdi->primary_buffer, gdi->width, gdi->height,
                          gdi->stride, x, y, w, h);
    }
}

static BOOL bridge_begin_paint(rdpContext* context) {
    HGDI_WND hwnd = bridge_primary_hwnd(context->gdi);
    if (hwnd && hwnd->invalid) {
        hwnd->invalid->null = TRUE;
        hwnd->ninvalid = 0;
    }
    return TRUE;
}

static BOOL bridge_end_paint(rdpContext* context) {
    rdpGdi* gdi = context->gdi;
    HGDI_WND hwnd = bridge_primary_hwnd(gdi);
    if (!hwnd || !hwnd->invalid || hwnd->invalid->null || gdi->suppressOutput) {
        return TRUE;
    }
    HGDI_RGN r = hwnd->invalid;
    bridge_emit_frame((BridgeClientContext*)context, gdi, r->x, r->y, r->w, r->h);
    return TRUE;
}

/* Re-queues the last monitor layout once when the host resizes the desktop to something
 * else, e.g. back to the connection size when Windows completes the user sign-in. */
static void bridge_check_layout(BridgeClientContext* bctx, UINT32 width, UINT32 height) {
    BOOL resend = FALSE;
    bridge_mutex_lock(&bctx->work_lock);
    if (bctx->last_count > 0 && !bctx->resize_pending) {
        INT64 left = INT32_MAX, top = INT32_MAX, right = INT32_MIN, bottom = INT32_MIN;
        for (UINT32 i = 0; i < bctx->last_count; i++) {
            const DISPLAY_CONTROL_MONITOR_LAYOUT* l = &bctx->last_layout[i];
            if (l->Left < left) left = l->Left;
            if (l->Top < top) top = l->Top;
            if ((INT64)l->Left + l->Width > right) right = (INT64)l->Left + l->Width;
            if ((INT64)l->Top + l->Height > bottom) bottom = (INT64)l->Top + l->Height;
        }
        if (right - left == (INT64)width && bottom - top == (INT64)height) {
            bctx->layout_resent = FALSE;
        } else if (!bctx->layout_resent) {
            memcpy(bctx->pending_layout, bctx->last_layout,
                   bctx->last_count * sizeof(bctx->last_layout[0]));
            bctx->pending_count = bctx->last_count;
            bctx->last_count = 0;
            bctx->resize_pending = TRUE;
            bctx->layout_resent = TRUE;
            resend = TRUE;
        }
    }
    bridge_mutex_unlock(&bctx->work_lock);
    if (resend) {
        WLog_Print(bridge_log(), WLOG_DEBUG, "Host desktop is %ux%u; re-sending the layout",
                   width, height);
        SetEvent(bctx->work_event);
    }
}

static BOOL bridge_desktop_resize(rdpContext* context) {
    rdpGdi* gdi = context->gdi;
    if (!gdi || !context->settings) return FALSE;
    UINT32 width = freerdp_settings_get_uint32(context->settings, FreeRDP_DesktopWidth);
    UINT32 height = freerdp_settings_get_uint32(context->settings, FreeRDP_DesktopHeight);
    WLog_Print(bridge_log(), WLOG_DEBUG, "Desktop resized to %ux%u", width, height);
    bridge_check_layout((BridgeClientContext*)context, width, height);
    if (!gdi_resize(gdi, width, height)) {
        WLog_Print(bridge_log(), WLOG_ERROR, "gdi_resize(%u, %u) failed", width, height);
        return FALSE;
    }
    bridge_emit_frame((BridgeClientContext*)context, gdi, 0, 0, (INT32)gdi->width,
                      (INT32)gdi->height);
    return TRUE;
}

/* ---- Connection lifecycle -------------------------------------------------------------- */

static BOOL bridge_pre_connect(freerdp* instance) {
    /* Codec selection happened in bridge_parse_args(); the GDI renders either path. */
    return freerdp_client_load_channels(instance);
}

static BOOL bridge_post_connect(freerdp* instance) {
    rdpContext* context = instance->context;
    if (!gdi_init(instance, PIXEL_FORMAT_BGRX32)) {
        WLog_Print(bridge_log(), WLOG_ERROR, "gdi_init failed");
        return FALSE;
    }
    context->update->BeginPaint = bridge_begin_paint;
    context->update->EndPaint = bridge_end_paint;
    context->update->DesktopResize = bridge_desktop_resize;

    BridgeClientContext* bctx = (BridgeClientContext*)context;
    bridge_set_input_ready(bctx, TRUE);
    bridge_status(bctx, RDP_BRIDGE_STATUS_CONNECTED, "Connected");
    /* Pick up a resize requested while (re)connecting. */
    SetEvent(bctx->work_event);
    return TRUE;
}

static void bridge_post_disconnect(freerdp* instance) {
    if (!instance || !instance->context) return;
    bridge_set_input_ready((BridgeClientContext*)instance->context, FALSE);
    gdi_free(instance);
}

/* `kind` is an RdpBridgeLocalClip. */
static UINT bridge_cliprdr_send_format_list(CliprdrClientContext* cliprdr, UINT32 kind) {
    CLIPRDR_FORMAT formats[2] = { 0 };
    UINT32 count = 0;
    switch (kind) {
        case RDP_BRIDGE_LOCAL_TEXT:
            formats[count++].formatId = CF_UNICODETEXT;
            formats[count++].formatId = CF_TEXT;
            break;
        case RDP_BRIDGE_LOCAL_IMAGE:
            formats[count++].formatId = CF_DIB;
            break;
        case RDP_BRIDGE_LOCAL_FILES:
            formats[count].formatId = RDP_BRIDGE_FILE_LIST_FORMAT_ID;
            formats[count++].formatName = g_file_list_format_name;
            break;
        default:
            break;
    }
    CLIPRDR_FORMAT_LIST list = { 0 };
    list.common.msgType = CB_FORMAT_LIST;
    list.numFormats = count;
    list.formats = formats;
    return cliprdr->ClientFormatList(cliprdr, &list);
}

static void bridge_file_contents_failed(BridgeClientContext* bctx, UINT32 stream_id) {
    if (bctx->cb.on_file_contents) {
        bctx->cb.on_file_contents(bctx->cb.user_data, stream_id, false, NULL, 0);
    }
}

static void bridge_send_file_requests(BridgeClientContext* bctx, CliprdrClientContext* cliprdr,
                                      const BridgeFileRequest* requests, UINT32 count) {
    const UINT32 mask = freerdp_settings_get_uint32(bctx->common.context.settings,
                                                    FreeRDP_ClipboardFeatureMask);
    /* FreeRDP silently drops file requests when remote-to-local files are disabled. */
    const BOOL allowed = (mask & CLIPRDR_FLAG_REMOTE_TO_LOCAL_FILES) != 0;
    for (UINT32 i = 0; i < count; i++) {
        const BridgeFileRequest* r = &requests[i];
        UINT rc = ERROR_INVALID_HANDLE;
        if (allowed && cliprdr && cliprdr->ClientFileContentsRequest) {
            CLIPRDR_FILE_CONTENTS_REQUEST req = { 0 };
            req.common.msgType = CB_FILECONTENTS_REQUEST;
            req.streamId = r->stream_id;
            req.listIndex = r->list_index;
            req.dwFlags = r->flags;
            req.nPositionLow = (UINT32)(r->position & 0xFFFFFFFFu);
            req.nPositionHigh = (UINT32)(r->position >> 32);
            req.cbRequested = r->size;
            rc = cliprdr->ClientFileContentsRequest(cliprdr, &req);
        }
        if (rc != CHANNEL_RC_OK) {
            WLog_Print(bridge_log(), WLOG_WARN, "File contents request failed: 0x%08X", rc);
            bridge_file_contents_failed(bctx, r->stream_id);
        }
    }
}

/* Runs queued UI requests on the session thread, where FreeRDP channels expect them. */
static void bridge_process_work(BridgeClientContext* bctx) {
    DISPLAY_CONTROL_MONITOR_LAYOUT layout[RDP_BRIDGE_MAX_MONITORS];
    UINT32 count = 0;
    BOOL send_layout = FALSE;
    BOOL announce = FALSE;
    UINT32 announce_kind = RDP_BRIDGE_LOCAL_NONE;
    DispClientContext* disp = NULL;
    CliprdrClientContext* cliprdr = NULL;
    BridgeFileRequest file_requests[MAX_FILE_REQUESTS];
    UINT32 file_request_count = 0;

    bridge_mutex_lock(&bctx->work_lock);
    ResetEvent(bctx->work_event);
    if (bctx->resize_pending && bctx->disp && bctx->disp_ready && bctx->disp->SendMonitorLayout) {
        bctx->resize_pending = FALSE;
        count = bctx->pending_count;
        /* The primary monitor is always first, so truncation keeps it. */
        if (bctx->disp_max_monitors > 0 && count > bctx->disp_max_monitors) {
            count = bctx->disp_max_monitors;
        }
        memcpy(layout, bctx->pending_layout, count * sizeof(layout[0]));
        if (count != bctx->last_count ||
            memcmp(layout, bctx->last_layout, count * sizeof(layout[0])) != 0) {
            memcpy(bctx->last_layout, layout, count * sizeof(layout[0]));
            bctx->last_count = count;
            disp = bctx->disp;
            send_layout = count > 0;
        }
    }
    cliprdr = bctx->cliprdr;
    if (bctx->clipboard_pending) {
        bctx->clipboard_pending = FALSE;
        announce = cliprdr && cliprdr->ClientFormatList;
        announce_kind = bctx->local_clip_kind;
    }
    file_request_count = bctx->file_request_count;
    memcpy(file_requests, bctx->file_requests, file_request_count * sizeof(file_requests[0]));
    bctx->file_request_count = 0;
    bridge_mutex_unlock(&bctx->work_lock);

    if (send_layout) {
        WLog_Print(bridge_log(), WLOG_DEBUG, "Monitor layout: %u monitor(s), primary %ux%u scale %u/%u",
                   count, layout[0].Width, layout[0].Height, layout[0].DesktopScaleFactor,
                   layout[0].DeviceScaleFactor);
        UINT rc = disp->SendMonitorLayout(disp, count, layout);
        if (rc != CHANNEL_RC_OK) {
            WLog_Print(bridge_log(), WLOG_WARN, "SendMonitorLayout failed: 0x%08X", rc);
        }
    }
    /* Channels are only disconnected on this thread, so `cliprdr` stays valid here. */
    if (announce) {
        bridge_cliprdr_send_format_list(cliprdr, announce_kind);
    }
    if (file_request_count > 0) {
        bridge_send_file_requests(bctx, cliprdr, file_requests, file_request_count);
    }
}

/* Reports the host's latest network measurements when they change. */
static void bridge_poll_network(BridgeClientContext* bctx) {
    if (!bctx->cb.on_network) return;
    UINT64 now = GetTickCount64();
    if (now - bctx->network_polled_at < NETWORK_POLL_MS) return;
    bctx->network_polled_at = now;
    rdpAutoDetect* ad = autodetect_get(&bctx->common.context);
    if (!ad) return;
    UINT32 values[3] = { ad->netCharAverageRTT, ad->netCharBaseRTT, ad->netCharBandwidth };
    if (memcmp(values, bctx->network_last, sizeof(values)) == 0) return;
    memcpy(bctx->network_last, values, sizeof(values));
    bctx->cb.on_network(bctx->cb.user_data, values[0], values[1], values[2]);
}

static const char* bridge_disconnect_reason(freerdp* instance) {
    UINT32 info = freerdp_error_info(instance);
    if (info != 0) {
        return freerdp_get_error_info_string(info);
    }
    UINT32 error = freerdp_get_last_error(instance->context);
    if (error != FREERDP_ERROR_SUCCESS) {
        return freerdp_get_last_error_string(error);
    }
    return "Session ended";
}

static BOOL bridge_can_auto_reconnect(freerdp* instance) {
    return freerdp_settings_get_bool(instance->context->settings,
                                     FreeRDP_AutoReconnectionEnabled) &&
           freerdp_error_info(instance) == 0;
}

static DWORD WINAPI bridge_client_thread(LPVOID arg) {
    rdpContext* context = (rdpContext*)arg;
    BridgeClientContext* bctx = (BridgeClientContext*)context;
    freerdp* instance = context->instance;

#if defined(_WIN32)
    bridge_init_windows_environment();
#endif

    bridge_status(bctx, RDP_BRIDGE_STATUS_PROGRESS, "Connecting...");
    bctx->failure_hint = NULL;
    g_last_log_error[0] = '\0';
    if (!freerdp_connect(instance)) {
        UINT32 error = freerdp_get_last_error(context);
        const char* reason = freerdp_get_last_error_string(error);
        static BRIDGE_THREAD_LOCAL char detail[512];
        if (bctx->failure_hint) {
            reason = bctx->failure_hint;
        } else if (g_last_log_error[0] != '\0') {
            if (error == FREERDP_ERROR_SUCCESS) {
                (void)snprintf(detail, sizeof(detail), "Connection failed: %s", g_last_log_error);
            } else {
                (void)snprintf(detail, sizeof(detail), "%s (%s)",
                               reason ? reason : "Connection failed", g_last_log_error);
            }
            reason = detail;
        } else if (!reason) {
            reason = "Connection failed";
        }
        WLog_Print(bridge_log(), WLOG_ERROR, "Connection failed: 0x%08X %s", error,
                   reason ? reason : "");
        bridge_status(bctx, RDP_BRIDGE_STATUS_FAILED, reason ? reason : "Connection failed");
        return 1;
    }
    if (!context->gdi) {
        /* freerdp_connect() can succeed through an internal reconnect without PostConnect. */
        freerdp_disconnect(instance);
        bridge_status(bctx, RDP_BRIDGE_STATUS_FAILED, "Connection failed");
        return 1;
    }

    HANDLE handles[MAXIMUM_WAIT_OBJECTS];
    while (!freerdp_shall_disconnect_context(context)) {
        DWORD count = freerdp_get_event_handles(context, handles, MAXIMUM_WAIT_OBJECTS - 2);
        if (count == 0) {
            WLog_Print(bridge_log(), WLOG_ERROR, "freerdp_get_event_handles failed");
            break;
        }
        handles[count] = bctx->stop_event;
        handles[count + 1] = bctx->work_event;
        DWORD status = WaitForMultipleObjects(count + 2, handles, FALSE, 100);
        if (status == WAIT_FAILED || status == WAIT_OBJECT_0 + count) {
            break;
        }
        if (status == WAIT_OBJECT_0 + count + 1) {
            bridge_process_work(bctx);
        }
        bridge_poll_network(bctx);

        if (!freerdp_check_event_handles(context)) {
            if (WaitForSingleObject(bctx->stop_event, 0) == WAIT_OBJECT_0) {
                break;
            }
            bridge_set_input_ready(bctx, FALSE);
            if (bridge_can_auto_reconnect(instance)) {
                bridge_status(bctx, RDP_BRIDGE_STATUS_PROGRESS, "Connection lost. Reconnecting...");
            }
            if (client_auto_reconnect_ex(instance, NULL)) {
                /* Reconnects skip PostConnect; the GDI surface is reused. */
                bridge_set_input_ready(bctx, TRUE);
                bridge_status(bctx, RDP_BRIDGE_STATUS_CONNECTED, "Reconnected");
                SetEvent(bctx->work_event);
                continue;
            }
            if (freerdp_error_info(instance) == 0 &&
                freerdp_get_last_error(context) == FREERDP_ERROR_SUCCESS) {
                freerdp_set_last_error_log(context, FREERDP_ERROR_CONNECT_FAILED);
            }
            break;
        }
    }

    const char* reason = bridge_disconnect_reason(instance);
    bridge_set_input_ready(bctx, FALSE);
    freerdp_disconnect(instance);
    bridge_status(bctx, RDP_BRIDGE_STATUS_ENDED, reason);
    return 0;
}

static int bridge_client_start(rdpContext* context) {
    BridgeClientContext* bctx = (BridgeClientContext*)context;
    if (bctx->thread) return -1;
    bctx->thread = CreateThread(NULL, 0, bridge_client_thread, context, 0, NULL);
    return bctx->thread ? 0 : -1;
}

static int bridge_client_stop(rdpContext* context) {
    BridgeClientContext* bctx = (BridgeClientContext*)context;
    SetEvent(bctx->stop_event);
    SetEvent(bctx->auth_done_event);
    SetEvent(bctx->cert_done_event);
    freerdp_abort_connect_context(context);
    if (bctx->thread) {
        /* The session thread references the context; it must be gone before anything is freed. */
        WaitForSingleObject(bctx->thread, INFINITE);
        CloseHandle(bctx->thread);
        bctx->thread = NULL;
    }
    bridge_free_secret(bridge_take_token_request(bctx));
    return 0;
}

/* ---- Channels -------------------------------------------------------------------------- */

static UINT bridge_display_control_caps(DispClientContext* disp, UINT32 max_monitors,
                                        UINT32 area_factor_a, UINT32 area_factor_b) {
    BridgeClientContext* bctx = (BridgeClientContext*)disp->custom;
    WLog_Print(bridge_log(), WLOG_DEBUG, "Display Control caps: monitors=%u area=%ux%u",
               max_monitors, area_factor_a, area_factor_b);
    if (bctx) {
        bridge_mutex_lock(&bctx->work_lock);
        bctx->disp_ready = TRUE;
        bctx->disp_max_monitors = max_monitors;
        bridge_mutex_unlock(&bctx->work_lock);
        SetEvent(bctx->work_event);
    }
    return CHANNEL_RC_OK;
}

static int bridge_clip_rank(UINT32 kind) {
    switch (kind) {
        case RDP_BRIDGE_CLIP_FILE_LIST: return 4;
        case RDP_BRIDGE_CLIP_TEXT: return 3;
        case RDP_BRIDGE_CLIP_DIBV5: return 2;
        case RDP_BRIDGE_CLIP_DIB: return 1;
        default: return 0;
    }
}

static UINT bridge_cliprdr_server_capabilities(CliprdrClientContext* cliprdr,
                                               const CLIPRDR_CAPABILITIES* capabilities) {
    (void)capabilities;
    CLIPRDR_GENERAL_CAPABILITY_SET general = { 0 };
    general.capabilitySetType = CB_CAPSTYPE_GENERAL;
    general.capabilitySetLength = CB_CAPSTYPE_GENERAL_LEN;
    general.version = CB_CAPS_VERSION_2;
    /* FreeRDP drops the flags the host does not support. */
    general.generalFlags = CB_USE_LONG_FORMAT_NAMES | CB_STREAM_FILECLIP_ENABLED |
                           CB_FILECLIP_NO_FILE_PATHS | CB_HUGE_FILE_SUPPORT_ENABLED;

    CLIPRDR_CAPABILITIES caps = { 0 };
    caps.cCapabilitiesSets = 1;
    caps.capabilitySets = (CLIPRDR_CAPABILITY_SET*)&general;
    return cliprdr->ClientCapabilities(cliprdr, &caps);
}

static UINT bridge_cliprdr_monitor_ready(CliprdrClientContext* cliprdr,
                                         const CLIPRDR_MONITOR_READY* monitor_ready) {
    (void)monitor_ready;
    BridgeClientContext* bctx = (BridgeClientContext*)cliprdr->custom;
    UINT32 kind = RDP_BRIDGE_LOCAL_NONE;
    if (bctx) {
        bridge_mutex_lock(&bctx->work_lock);
        kind = bctx->local_clip_kind;
        bctx->clipboard_pending = FALSE;
        bridge_mutex_unlock(&bctx->work_lock);
    }
    return bridge_cliprdr_send_format_list(cliprdr, kind);
}

static UINT bridge_cliprdr_server_format_list(CliprdrClientContext* cliprdr,
                                              const CLIPRDR_FORMAT_LIST* format_list) {
    BridgeClientContext* bctx = (BridgeClientContext*)cliprdr->custom;
    if (!bctx || !format_list) return CHANNEL_RC_OK;

    CLIPRDR_FORMAT_LIST_RESPONSE resp = { 0 };
    resp.common.msgType = CB_FORMAT_LIST_RESPONSE;
    resp.common.msgFlags = CB_RESPONSE_OK;
    UINT rc = cliprdr->ClientFormatListResponse(cliprdr, &resp);
    if (rc != CHANNEL_RC_OK) return rc;

    /* The host owns the clipboard now; local content is no longer on offer. */
    bridge_mutex_lock(&bctx->work_lock);
    bctx->local_clip_kind = RDP_BRIDGE_LOCAL_NONE;
    bctx->clipboard_pending = FALSE;
    bridge_mutex_unlock(&bctx->work_lock);
    if (bctx->cb.on_clipboard) {
        bctx->cb.on_clipboard(bctx->cb.user_data, RDP_BRIDGE_CLIP_CHANGED, NULL, 0);
    }

    /* One format is requested: files, then text, then bitmaps. Kind values are ordered by
     * preference. */
    UINT32 kind = RDP_BRIDGE_CLIP_CHANGED;
    UINT32 format_id = 0;
    for (UINT32 i = 0; i < format_list->numFormats; i++) {
        const CLIPRDR_FORMAT* f = &format_list->formats[i];
        UINT32 candidate = RDP_BRIDGE_CLIP_CHANGED;
        if (f->formatName && strcmp(f->formatName, g_file_list_format_name) == 0) {
            candidate = RDP_BRIDGE_CLIP_FILE_LIST;
        } else if (f->formatId == CF_UNICODETEXT) {
            candidate = RDP_BRIDGE_CLIP_TEXT;
        } else if (f->formatId == CF_DIBV5) {
            candidate = RDP_BRIDGE_CLIP_DIBV5;
        } else if (f->formatId == CF_DIB) {
            candidate = RDP_BRIDGE_CLIP_DIB;
        }
        if (bridge_clip_rank(candidate) > bridge_clip_rank(kind)) {
            kind = candidate;
            format_id = f->formatId;
        }
    }
    if (kind == RDP_BRIDGE_CLIP_CHANGED) return CHANNEL_RC_OK;

    /* Format data responses don't name their format; they answer requests in order. */
    bridge_mutex_lock(&bctx->work_lock);
    bctx->remote_requests_pending++;
    bctx->remote_request_kind = kind;
    bridge_mutex_unlock(&bctx->work_lock);

    CLIPRDR_FORMAT_DATA_REQUEST req = { 0 };
    req.common.msgType = CB_FORMAT_DATA_REQUEST;
    req.requestedFormatId = format_id;
    rc = cliprdr->ClientFormatDataRequest(cliprdr, &req);
    if (rc != CHANNEL_RC_OK) {
        bridge_mutex_lock(&bctx->work_lock);
        if (bctx->remote_requests_pending > 0) bctx->remote_requests_pending--;
        bridge_mutex_unlock(&bctx->work_lock);
    }
    return rc;
}

static UINT bridge_cliprdr_server_format_list_response(
    CliprdrClientContext* cliprdr, const CLIPRDR_FORMAT_LIST_RESPONSE* response) {
    (void)cliprdr;
    (void)response;
    return CHANNEL_RC_OK;
}

static UINT bridge_cliprdr_server_format_data_request(CliprdrClientContext* cliprdr,
                                                      const CLIPRDR_FORMAT_DATA_REQUEST* req) {
    BridgeClientContext* bctx = (BridgeClientContext*)cliprdr->custom;
    if (!bctx || !req) return CHANNEL_RC_OK;

    CLIPRDR_FORMAT_DATA_RESPONSE resp = { 0 };
    resp.common.msgType = CB_FORMAT_DATA_RESPONSE;
    resp.common.msgFlags = CB_RESPONSE_FAIL;

    /* Clipboard content is owned and encoded by Rust: query the size first, then fill. */
    BYTE* data = NULL;
    size_t needed = 0;
    if (bctx->cb.on_clipboard_request) {
        needed = bctx->cb.on_clipboard_request(bctx->cb.user_data, req->requestedFormatId, NULL, 0);
    }
    if (needed > 0 && needed <= UINT32_MAX && (data = (BYTE*)malloc(needed)) != NULL) {
        size_t written = bctx->cb.on_clipboard_request(bctx->cb.user_data,
                                                       req->requestedFormatId, data, needed);
        if (written > 0 && written <= needed) {
            resp.common.msgFlags = CB_RESPONSE_OK;
            resp.common.dataLen = (UINT32)written;
            resp.requestedFormatData = data;
        }
    }

    UINT rc = cliprdr->ClientFormatDataResponse(cliprdr, &resp);
    free(data);
    return rc;
}

static UINT bridge_cliprdr_server_format_data_response(
    CliprdrClientContext* cliprdr, const CLIPRDR_FORMAT_DATA_RESPONSE* response) {
    BridgeClientContext* bctx = (BridgeClientContext*)cliprdr->custom;
    if (!bctx || !response) return CHANNEL_RC_OK;

    UINT32 kind = RDP_BRIDGE_CLIP_CHANGED;
    bridge_mutex_lock(&bctx->work_lock);
    if (bctx->remote_requests_pending > 0) {
        bctx->remote_requests_pending--;
        /* Answers to requests made before the latest format list are stale. */
        if (bctx->remote_requests_pending == 0) kind = bctx->remote_request_kind;
    }
    bridge_mutex_unlock(&bctx->work_lock);

    if (kind == RDP_BRIDGE_CLIP_CHANGED || (response->common.msgFlags & CB_RESPONSE_FAIL) ||
        !response->requestedFormatData || response->common.dataLen == 0) {
        return CHANNEL_RC_OK;
    }
    if (bctx->cb.on_clipboard) {
        bctx->cb.on_clipboard(bctx->cb.user_data, (int)kind, response->requestedFormatData,
                              response->common.dataLen);
    }
    return CHANNEL_RC_OK;
}

/* The host reads a file of the local file list; Rust reads it into a C-allocated buffer. */
static UINT bridge_cliprdr_server_file_contents_request(
    CliprdrClientContext* cliprdr, const CLIPRDR_FILE_CONTENTS_REQUEST* request) {
    BridgeClientContext* bctx = (BridgeClientContext*)cliprdr->custom;
    if (!request) return CHANNEL_RC_OK;

    static BYTE no_data;
    CLIPRDR_FILE_CONTENTS_RESPONSE resp = { 0 };
    resp.common.msgType = CB_FILECONTENTS_RESPONSE;
    resp.common.msgFlags = CB_RESPONSE_FAIL;
    resp.streamId = request->streamId;
    resp.requestedData = &no_data;

    size_t wanted = (request->dwFlags & FILECONTENTS_SIZE) ? sizeof(UINT64) : request->cbRequested;
    if (wanted > MAX_FILE_CHUNK) wanted = MAX_FILE_CHUNK;
    const UINT64 position = ((UINT64)request->nPositionHigh << 32) | request->nPositionLow;
    BYTE* data = (bctx && bctx->cb.on_file_request) ? (BYTE*)malloc(wanted ? wanted : 1) : NULL;
    if (data) {
        int64_t n = bctx->cb.on_file_request(bctx->cb.user_data, request->listIndex,
                                             request->dwFlags, position, data, wanted);
        if (n >= 0 && (uint64_t)n <= wanted) {
            resp.common.msgFlags = CB_RESPONSE_OK;
            resp.cbRequested = (UINT32)n;
            resp.requestedData = data;
        }
    }

    UINT rc = cliprdr->ClientFileContentsResponse(cliprdr, &resp);
    free(data);
    return rc;
}

static UINT bridge_cliprdr_server_file_contents_response(
    CliprdrClientContext* cliprdr, const CLIPRDR_FILE_CONTENTS_RESPONSE* response) {
    BridgeClientContext* bctx = (BridgeClientContext*)cliprdr->custom;
    if (!bctx || !response || !bctx->cb.on_file_contents) return CHANNEL_RC_OK;

    /* FreeRDP derives cbRequested from dataLen without checking that it holds the stream id. */
    const UINT32 data_len = response->common.dataLen;
    const BOOL ok = !(response->common.msgFlags & CB_RESPONSE_FAIL) && data_len >= 4 &&
                    response->cbRequested <= data_len - 4 &&
                    (response->cbRequested == 0 || response->requestedData);
    bctx->cb.on_file_contents(bctx->cb.user_data, response->streamId, ok,
                              ok ? response->requestedData : NULL,
                              ok ? response->cbRequested : 0);
    return CHANNEL_RC_OK;
}

/* Clipboard data locking is not advertised; these only acknowledge the PDUs. */
static UINT bridge_cliprdr_server_lock_clipboard_data(CliprdrClientContext* cliprdr,
                                                      const CLIPRDR_LOCK_CLIPBOARD_DATA* lock) {
    (void)cliprdr;
    (void)lock;
    return CHANNEL_RC_OK;
}

static UINT bridge_cliprdr_server_unlock_clipboard_data(
    CliprdrClientContext* cliprdr, const CLIPRDR_UNLOCK_CLIPBOARD_DATA* unlock) {
    (void)cliprdr;
    (void)unlock;
    return CHANNEL_RC_OK;
}

/* ---- RemoteApp (RAIL) ------------------------------------------------------------------
 * RemoteApp windows are drawn into the session surface like a desktop; the bridge only has
 * to start the program once the host is ready. */

static UINT bridge_rail_server_handshake(RailClientContext* rail,
                                         const RAIL_HANDSHAKE_ORDER* handshake) {
    (void)handshake;
    return client_rail_server_start_cmd(rail);
}

static UINT bridge_rail_server_handshake_ex(RailClientContext* rail,
                                            const RAIL_HANDSHAKE_EX_ORDER* handshake) {
    (void)handshake;
    return client_rail_server_start_cmd(rail);
}

static UINT bridge_rail_server_execute_result(RailClientContext* rail,
                                              const RAIL_EXEC_RESULT_ORDER* result) {
    BridgeClientContext* bctx = (BridgeClientContext*)rail->custom;
    if (bctx && result && result->execResult != RAIL_EXEC_S_OK) {
        WLog_Print(bridge_log(), WLOG_ERROR, "RemoteApp failed to start: %u (0x%08X)",
                   (unsigned)result->execResult, result->rawResult);
        bctx->failure_hint = "The remote application could not be started";
        bridge_status(bctx, RDP_BRIDGE_STATUS_PROGRESS, bctx->failure_hint);
    }
    return CHANNEL_RC_OK;
}

static void bridge_on_channel_connected(void* context, const ChannelConnectedEventArgs* e) {
    BridgeClientContext* bctx = (BridgeClientContext*)context;
    if (e->name && strcmp(e->name, DISP_DVC_CHANNEL_NAME) == 0) {
        DispClientContext* disp = (DispClientContext*)e->pInterface;
        disp->custom = bctx;
        disp->DisplayControlCaps = bridge_display_control_caps;
        bridge_mutex_lock(&bctx->work_lock);
        bctx->disp = disp;
        bctx->disp_ready = FALSE;
        bctx->disp_max_monitors = 0;
        /* A new connection (reconnect or redirection to the session host) starts at the
         * connection size, so the current layout has to be sent again. */
        if (bctx->last_count > 0 && !bctx->resize_pending) {
            memcpy(bctx->pending_layout, bctx->last_layout,
                   bctx->last_count * sizeof(bctx->last_layout[0]));
            bctx->pending_count = bctx->last_count;
            bctx->resize_pending = TRUE;
        }
        bctx->last_count = 0;
        bctx->layout_resent = FALSE;
        bridge_mutex_unlock(&bctx->work_lock);
    } else if (e->name && strcmp(e->name, RAIL_SVC_CHANNEL_NAME) == 0) {
        RailClientContext* rail = (RailClientContext*)e->pInterface;
        /* client_rail_server_start_cmd() expects the rdpClientContext here. */
        rail->custom = &bctx->common;
        rail->ServerHandshake = bridge_rail_server_handshake;
        rail->ServerHandshakeEx = bridge_rail_server_handshake_ex;
        rail->ServerExecuteResult = bridge_rail_server_execute_result;
    } else if (e->name && strcmp(e->name, CLIPRDR_SVC_CHANNEL_NAME) == 0) {
        CliprdrClientContext* cliprdr = (CliprdrClientContext*)e->pInterface;
        cliprdr->custom = bctx;
        cliprdr->ServerCapabilities = bridge_cliprdr_server_capabilities;
        cliprdr->MonitorReady = bridge_cliprdr_monitor_ready;
        cliprdr->ServerFormatList = bridge_cliprdr_server_format_list;
        cliprdr->ServerFormatListResponse = bridge_cliprdr_server_format_list_response;
        cliprdr->ServerFormatDataRequest = bridge_cliprdr_server_format_data_request;
        cliprdr->ServerFormatDataResponse = bridge_cliprdr_server_format_data_response;
        cliprdr->ServerFileContentsRequest = bridge_cliprdr_server_file_contents_request;
        cliprdr->ServerFileContentsResponse = bridge_cliprdr_server_file_contents_response;
        cliprdr->ServerLockClipboardData = bridge_cliprdr_server_lock_clipboard_data;
        cliprdr->ServerUnlockClipboardData = bridge_cliprdr_server_unlock_clipboard_data;
        bridge_mutex_lock(&bctx->work_lock);
        bctx->cliprdr = cliprdr;
        bctx->remote_requests_pending = 0;
        bridge_mutex_unlock(&bctx->work_lock);
    }
    freerdp_client_OnChannelConnectedEventHandler(context, e);
}

static void bridge_on_channel_disconnected(void* context, const ChannelDisconnectedEventArgs* e) {
    BridgeClientContext* bctx = (BridgeClientContext*)context;
    BridgeFileRequest dropped[MAX_FILE_REQUESTS];
    UINT32 dropped_count = 0;
    bridge_mutex_lock(&bctx->work_lock);
    if (e->name && strcmp(e->name, DISP_DVC_CHANNEL_NAME) == 0) {
        bctx->disp = NULL;
        bctx->disp_ready = FALSE;
    } else if (e->name && strcmp(e->name, CLIPRDR_SVC_CHANNEL_NAME) == 0 && bctx->cliprdr) {
        bctx->cliprdr->custom = NULL;
        bctx->cliprdr = NULL;
        bctx->remote_requests_pending = 0;
        dropped_count = bctx->file_request_count;
        memcpy(dropped, bctx->file_requests, dropped_count * sizeof(dropped[0]));
        bctx->file_request_count = 0;
    }
    bridge_mutex_unlock(&bctx->work_lock);
    for (UINT32 i = 0; i < dropped_count; i++) {
        bridge_file_contents_failed(bctx, dropped[i].stream_id);
    }
    freerdp_client_OnChannelDisconnectedEventHandler(context, e);
}

/* ---- Context ---------------------------------------------------------------------------- */

static BOOL bridge_client_new(freerdp* instance, rdpContext* context) {
    BridgeClientContext* bctx = (BridgeClientContext*)context;
    if (!bridge_mutex_init(&bctx->auth_lock)) return FALSE;
    if (!bridge_mutex_init(&bctx->work_lock)) {
        bridge_mutex_destroy(&bctx->auth_lock);
        return FALSE;
    }
    if (!bridge_rwlock_init(&bctx->input_lock)) {
        bridge_mutex_destroy(&bctx->work_lock);
        bridge_mutex_destroy(&bctx->auth_lock);
        return FALSE;
    }
    bctx->locks_ready = TRUE;
    bctx->cert_answer = -1;
    bctx->stop_event = CreateEvent(NULL, TRUE, FALSE, NULL);
    bctx->auth_done_event = CreateEvent(NULL, TRUE, FALSE, NULL);
    bctx->cert_done_event = CreateEvent(NULL, TRUE, FALSE, NULL);
    bctx->work_event = CreateEvent(NULL, TRUE, FALSE, NULL);
    /* On failure ClientFree still runs and releases whatever was created. */
    if (!bctx->stop_event || !bctx->auth_done_event || !bctx->cert_done_event ||
        !bctx->work_event) {
        return FALSE;
    }

    instance->PreConnect = bridge_pre_connect;
    instance->PostConnect = bridge_post_connect;
    instance->PostDisconnect = bridge_post_disconnect;
    instance->AuthenticateEx = bridge_authenticate_ex;
    instance->VerifyCertificateEx = bridge_verify_certificate;
    instance->VerifyChangedCertificateEx = bridge_verify_changed_certificate;
    instance->GetAccessToken = bridge_get_access_token;
    PubSub_SubscribeChannelConnected(context->pubSub, bridge_on_channel_connected);
    PubSub_SubscribeChannelDisconnected(context->pubSub, bridge_on_channel_disconnected);
    return TRUE;
}

static void bridge_close_handle(HANDLE* handle) {
    if (*handle) {
        CloseHandle(*handle);
        *handle = NULL;
    }
}

static void bridge_client_free(freerdp* instance, rdpContext* context) {
    (void)instance;
    BridgeClientContext* bctx = (BridgeClientContext*)context;
    if (context->pubSub) {
        PubSub_UnsubscribeChannelConnected(context->pubSub, bridge_on_channel_connected);
        PubSub_UnsubscribeChannelDisconnected(context->pubSub, bridge_on_channel_disconnected);
    }
    bridge_free_secret(bctx->token_request);
    bctx->token_request = NULL;
    bridge_free_secret(bctx->gateway_token);
    bctx->gateway_token = NULL;
    bridge_close_handle(&bctx->thread);
    bridge_close_handle(&bctx->stop_event);
    bridge_close_handle(&bctx->auth_done_event);
    bridge_close_handle(&bctx->cert_done_event);
    bridge_close_handle(&bctx->work_event);
    if (bctx->locks_ready) {
        bridge_rwlock_destroy(&bctx->input_lock);
        bridge_mutex_destroy(&bctx->work_lock);
        bridge_mutex_destroy(&bctx->auth_lock);
        bctx->locks_ready = FALSE;
    }
}

/* FreeRDP context creation/destruction and command-line parsing touch process-global
 * state (channel addin registry, WLog), so they are serialised. */
static BridgeMutex g_context_lock = BRIDGE_MUTEX_INITIALIZER;
static BridgeOnce g_logging_once = BRIDGE_ONCE_INITIALIZER;

/* Replacing the root appender frees the old one, so it must happen before any thread can
 * log: every entry point that reaches FreeRDP calls this first. */
static void bridge_ensure_logging(void) {
    bridge_once(&g_logging_once, bridge_init_logging);
}

const char* freerdp_bridge_version(void) {
    return freerdp_get_version_string();
}

const char* freerdp_bridge_build_config(void) {
    return freerdp_get_build_config();
}

bool freerdp_bridge_accepts_option(const char* option) {
    if (!option) return false;
    bridge_ensure_logging();
    bridge_mutex_lock(&g_context_lock);
    rdpSettings* settings = freerdp_settings_new(0);
    if (!settings) {
        bridge_mutex_unlock(&g_context_lock);
        return false;
    }
    char program[] = "freerdp";
    char server[] = "/v:localhost";
    char* arg = _strdup(option);
    bool ok = false;
    if (arg) {
        char* argv[] = { program, server, arg, NULL };
        ok = freerdp_client_settings_parse_command_line(settings, 3, argv, FALSE) == 0;
    }
    free(arg);
    freerdp_settings_free(settings);
    bridge_mutex_unlock(&g_context_lock);
    return ok;
}

bool freerdp_bridge_supports_camera(void) {
#if defined(CHANNEL_RDPECAM_CLIENT)
    return true;
#else
    return false;
#endif
}

static BOOL bridge_parse_args(rdpContext* ctx, const char* const* args, int argc) {
    /* FreeRDP may rewrite argv in place (e.g. to mask passwords), so hand it private copies. */
    char** argv = (char**)calloc((size_t)argc + 1, sizeof(char*));
    if (!argv) return FALSE;
    BOOL ok = TRUE;
    BOOL gfx_requested = FALSE;
    for (int i = 0; i < argc && ok; i++) {
        argv[i] = _strdup(args[i] ? args[i] : "");
        ok = argv[i] != NULL;
        if (ok && i > 0 && (strcmp(argv[i], "/gfx") == 0 || strncmp(argv[i], "/gfx:", 5) == 0)) {
            gfx_requested = TRUE;
        }
    }
    if (ok) {
        int rc = freerdp_client_settings_parse_command_line(ctx->settings, argc, argv, FALSE);
        if (rc != 0) {
            WLog_Print(bridge_log(), WLOG_ERROR, "Invalid FreeRDP arguments (%d)", rc);
            ok = FALSE;
        }
    }
    for (int i = 0; i < argc; i++) {
        bridge_free_secret(argv[i]);
    }
    free(argv);
    if (!ok) return FALSE;

    rdpSettings* settings = ctx->settings;
    /* FreeRDP 3 enables the graphics pipeline by default; it is only used when the caller
     * asks for it, because engines without support for every GFX command (e.g. surface
     * protection before 3.27) lose the whole display when the host sends one. */
    if (!gfx_requested &&
        !freerdp_settings_set_bool(settings, FreeRDP_SupportGraphicsPipeline, FALSE)) {
        return FALSE;
    }
    /* GFX RemoteApp surfaces are mapped to windows, which the GDI output path does not
     * render; RemoteApps use classic drawing into the session surface instead. */
    if (freerdp_settings_get_bool(settings, FreeRDP_RemoteApplicationMode) &&
        !freerdp_settings_set_bool(settings, FreeRDP_SupportGraphicsPipeline, FALSE)) {
        return FALSE;
    }
    /* Without the graphics pipeline (/gfx) RemoteFX surface bits are the best codec. */
    if (!freerdp_settings_get_bool(settings, FreeRDP_SupportGraphicsPipeline) &&
        !freerdp_settings_set_bool(settings, FreeRDP_RemoteFxCodec, TRUE)) {
        return FALSE;
    }
    if (!freerdp_settings_set_bool(settings, FreeRDP_SupportDisplayControl, TRUE) ||
        !freerdp_settings_set_bool(settings, FreeRDP_DynamicResolutionUpdate, TRUE)) {
        return FALSE;
    }
    if (freerdp_settings_get_uint32(settings, FreeRDP_DeviceCount) > 0 &&
        !freerdp_settings_set_bool(settings, FreeRDP_DeviceRedirection, TRUE)) {
        return FALSE;
    }
    return TRUE;
}

RdpBridgeContext* freerdp_bridge_create(const char* const* args, int argc,
                                        const char* gateway_token,
                                        const RdpBridgeCallbacks* callbacks) {
    if (!args || argc < 1 || !callbacks) return NULL;
    bridge_ensure_logging();

    RdpBridgeContext* bridge = (RdpBridgeContext*)calloc(1, sizeof(RdpBridgeContext));
    if (!bridge) return NULL;

    RDP_CLIENT_ENTRY_POINTS ep = { 0 };
    ep.Size = sizeof(ep);
    ep.Version = RDP_CLIENT_INTERFACE_VERSION;
    ep.ContextSize = sizeof(BridgeClientContext);
    ep.ClientNew = bridge_client_new;
    ep.ClientFree = bridge_client_free;
    ep.ClientStart = bridge_client_start;
    ep.ClientStop = bridge_client_stop;

    bridge_mutex_lock(&g_context_lock);
    rdpContext* ctx = freerdp_client_context_new(&ep);
    bridge_mutex_unlock(&g_context_lock);
    if (!ctx) {
        free(bridge);
        return NULL;
    }
    bridge->rdp_ctx = ctx;

    BridgeClientContext* bctx = (BridgeClientContext*)ctx;
    bctx->cb = *callbacks;
    if (gateway_token && gateway_token[0]) {
        bctx->gateway_token = _strdup(gateway_token);
    }
    BOOL parsed = FALSE;
    if (!gateway_token || !gateway_token[0] || bctx->gateway_token) {
        bridge_mutex_lock(&g_context_lock);
        parsed = bridge_parse_args(ctx, args, argc);
        bridge_mutex_unlock(&g_context_lock);
    }
    if (!parsed) {
        freerdp_bridge_free(bridge);
        return NULL;
    }
    return bridge;
}

bool freerdp_bridge_start(RdpBridgeContext* ctx) {
    if (!ctx || !ctx->rdp_ctx || ctx->running) return false;
    ctx->running = freerdp_client_start(ctx->rdp_ctx) == 0;
    return ctx->running;
}

void freerdp_bridge_stop(RdpBridgeContext* ctx) {
    if (!ctx || !ctx->rdp_ctx || !ctx->running) return;
    freerdp_client_stop(ctx->rdp_ctx);
    ctx->running = FALSE;
}

void freerdp_bridge_free(RdpBridgeContext* ctx) {
    if (!ctx) return;
    /* Joining the session thread can take a while; don't hold the global lock for it. */
    freerdp_bridge_stop(ctx);
    if (ctx->rdp_ctx) {
        bridge_mutex_lock(&g_context_lock);
        freerdp_client_context_free(ctx->rdp_ctx);
        bridge_mutex_unlock(&g_context_lock);
    }
    free(ctx);
}

void freerdp_bridge_provide_token_request(RdpBridgeContext* ctx, const char* token_request) {
    if (!ctx || !ctx->rdp_ctx) return;
    BridgeClientContext* bctx = (BridgeClientContext*)ctx->rdp_ctx;
    char* copy = token_request ? _strdup(token_request) : NULL;
    bridge_mutex_lock(&bctx->auth_lock);
    char* old = bctx->token_request;
    bctx->token_request = copy;
    bridge_mutex_unlock(&bctx->auth_lock);
    bridge_free_secret(old);
    SetEvent(bctx->auth_done_event);
}

void freerdp_bridge_answer_certificate(RdpBridgeContext* ctx, int decision) {
    if (!ctx || !ctx->rdp_ctx) return;
    BridgeClientContext* bctx = (BridgeClientContext*)ctx->rdp_ctx;
    if (decision != RDP_BRIDGE_CERT_ACCEPT && decision != RDP_BRIDGE_CERT_ACCEPT_ONCE) {
        decision = RDP_BRIDGE_CERT_REJECT;
    }
    bridge_mutex_lock(&bctx->auth_lock);
    bctx->cert_answer = decision;
    bridge_mutex_unlock(&bctx->auth_lock);
    SetEvent(bctx->cert_done_event);
}

void freerdp_bridge_send_mouse(RdpBridgeContext* ctx, uint16_t flags, uint16_t x, uint16_t y) {
    rdpInput* input = bridge_input_begin(ctx);
    if (!input) return;
    freerdp_input_send_mouse_event(input, flags, x, y);
    bridge_input_end(ctx);
}

void freerdp_bridge_send_extended_mouse(RdpBridgeContext* ctx, uint16_t flags, uint16_t x,
                                        uint16_t y) {
    rdpInput* input = bridge_input_begin(ctx);
    if (!input) return;
    freerdp_input_send_extended_mouse_event(input, flags, x, y);
    bridge_input_end(ctx);
}

void freerdp_bridge_send_scancode(RdpBridgeContext* ctx, bool down, bool repeat,
                                  uint32_t rdp_scancode) {
    if (RDP_SCANCODE_CODE(rdp_scancode) == 0) return;
    rdpInput* input = bridge_input_begin(ctx);
    if (!input) return;
    if (rdp_scancode == RDP_SCANCODE_PAUSE) {
        /* Pause has no break code; the whole make/break sequence is sent on press. */
        if (down) freerdp_input_send_keyboard_pause_event(input);
    } else {
        freerdp_input_send_keyboard_event_ex(input, down ? TRUE : FALSE, repeat ? TRUE : FALSE,
                                             rdp_scancode);
    }
    bridge_input_end(ctx);
}

void freerdp_bridge_send_focus_in(RdpBridgeContext* ctx, uint16_t lock_flags) {
    rdpInput* input = bridge_input_begin(ctx);
    if (!input) return;
    freerdp_input_send_focus_in_event(input, lock_flags);
    bridge_input_end(ctx);
}

void freerdp_bridge_send_unicode(RdpBridgeContext* ctx, bool down, uint16_t code_unit) {
    rdpInput* input = bridge_input_begin(ctx);
    if (!input) return;
    freerdp_input_send_unicode_keyboard_event(input, down ? 0 : KBD_FLAGS_RELEASE, code_unit);
    bridge_input_end(ctx);
}

static UINT32 bridge_clamp_dimension(UINT32 value, UINT32 min) {
    if (value < min) return min;
    if (value > MAX_MONITOR_SIZE) return MAX_MONITOR_SIZE;
    return value;
}

void freerdp_bridge_request_layout(RdpBridgeContext* ctx, const RdpBridgeMonitor* monitors,
                                   uint32_t count, uint32_t desktop_scale,
                                   uint32_t device_scale) {
    if (!ctx || !ctx->rdp_ctx || !monitors || count == 0) return;
    BridgeClientContext* bctx = (BridgeClientContext*)ctx->rdp_ctx;
    if (count > RDP_BRIDGE_MAX_MONITORS) count = RDP_BRIDGE_MAX_MONITORS;
    if (desktop_scale < 100) desktop_scale = 100;
    if (desktop_scale > 500) desktop_scale = 500;
    if (device_scale != 140 && device_scale != 180) device_scale = 100;

    uint32_t primary = 0;
    for (uint32_t i = 0; i < count; i++) {
        if (monitors[i].primary) {
            primary = i;
            break;
        }
    }

    /* MS-RDPEDISP: exactly one primary monitor at (0,0), listed first here. */
    DISPLAY_CONTROL_MONITOR_LAYOUT layout[RDP_BRIDGE_MAX_MONITORS] = { 0 };
    const int64_t origin_x = monitors[primary].left;
    const int64_t origin_y = monitors[primary].top;
    for (uint32_t i = 0, n = 0; i < count; i++) {
        const RdpBridgeMonitor* m = &monitors[(primary + i) % count];
        DISPLAY_CONTROL_MONITOR_LAYOUT* l = &layout[n++];
        int64_t left = (int64_t)m->left - origin_x;
        int64_t top = (int64_t)m->top - origin_y;
        if (left < -MAX_MONITOR_SIZE * 4) left = -MAX_MONITOR_SIZE * 4;
        if (left > MAX_MONITOR_SIZE * 4) left = MAX_MONITOR_SIZE * 4;
        if (top < -MAX_MONITOR_SIZE * 4) top = -MAX_MONITOR_SIZE * 4;
        if (top > MAX_MONITOR_SIZE * 4) top = MAX_MONITOR_SIZE * 4;
        l->Flags = (i == 0) ? DISPLAY_CONTROL_MONITOR_PRIMARY : 0;
        l->Left = (INT32)left;
        l->Top = (INT32)top;
        /* MS-RDPEDISP: the width must be even. */
        l->Width = bridge_clamp_dimension(m->width, DISPLAY_CONTROL_MIN_MONITOR_WIDTH) & ~1u;
        l->Height = bridge_clamp_dimension(m->height, DISPLAY_CONTROL_MIN_MONITOR_HEIGHT);
        l->Orientation = ORIENTATION_LANDSCAPE;
        l->DesktopScaleFactor = desktop_scale;
        l->DeviceScaleFactor = device_scale;
    }

    bridge_mutex_lock(&bctx->work_lock);
    memcpy(bctx->pending_layout, layout, sizeof(layout));
    bctx->pending_count = count;
    bctx->resize_pending = TRUE;
    bctx->layout_resent = FALSE;
    bridge_mutex_unlock(&bctx->work_lock);
    SetEvent(bctx->work_event);
}

void freerdp_bridge_request_resize(RdpBridgeContext* ctx, uint32_t width, uint32_t height,
                                   uint32_t desktop_scale, uint32_t device_scale) {
    RdpBridgeMonitor monitor = { 0, 0, width, height, true };
    freerdp_bridge_request_layout(ctx, &monitor, 1, desktop_scale, device_scale);
}

void freerdp_bridge_announce_clipboard(RdpBridgeContext* ctx, uint32_t kind) {
    if (!ctx || !ctx->rdp_ctx) return;
    BridgeClientContext* bctx = (BridgeClientContext*)ctx->rdp_ctx;
    if (kind > RDP_BRIDGE_LOCAL_FILES) kind = RDP_BRIDGE_LOCAL_NONE;
    bridge_mutex_lock(&bctx->work_lock);
    bctx->local_clip_kind = kind;
    bctx->clipboard_pending = TRUE;
    bridge_mutex_unlock(&bctx->work_lock);
    SetEvent(bctx->work_event);
}

bool freerdp_bridge_request_file_contents(RdpBridgeContext* ctx, uint32_t stream_id,
                                          uint32_t list_index, uint32_t flags,
                                          uint64_t position, uint32_t size) {
    if (!ctx || !ctx->rdp_ctx || size == 0 || size > MAX_FILE_CHUNK) return false;
    if (flags != FILECONTENTS_SIZE && flags != FILECONTENTS_RANGE) return false;
    if (flags == FILECONTENTS_SIZE && (size != sizeof(UINT64) || position != 0)) return false;
    BridgeClientContext* bctx = (BridgeClientContext*)ctx->rdp_ctx;
    bridge_mutex_lock(&bctx->work_lock);
    const BOOL queued = bctx->cliprdr && bctx->file_request_count < MAX_FILE_REQUESTS;
    if (queued) {
        BridgeFileRequest* r = &bctx->file_requests[bctx->file_request_count++];
        r->stream_id = stream_id;
        r->list_index = list_index;
        r->flags = flags;
        r->position = position;
        r->size = size;
    }
    bridge_mutex_unlock(&bctx->work_lock);
    if (queued) SetEvent(bctx->work_event);
    return queued;
}
