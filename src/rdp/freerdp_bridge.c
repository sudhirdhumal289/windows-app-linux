#include "freerdp_bridge.h"

#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdarg.h>

#include <freerdp/client.h>
#include <freerdp/freerdp.h>
#include <freerdp/gdi/gdi.h>
#include <freerdp/input.h>
#include <freerdp/error.h>
#include <freerdp/settings_keys.h>
#include <freerdp/utils/aad.h>
#include <winpr/crt.h>
#include <winpr/synch.h>
#include <winpr/thread.h>
#include <winpr/string.h>
#include <winpr/collections.h>
#include <winpr/wlog.h>
#include <freerdp/client/disp.h>
#include <freerdp/client/cliprdr.h>
#include <freerdp/channels/cliprdr.h>
#include <winpr/user.h>
#include <freerdp/event.h>
#include <freerdp/locale/keyboard.h>
#include <freerdp/scancode.h>

#define BRIDGE_DEBUG(...) do { \
    static int s_bridge_debug = -1; \
    if (s_bridge_debug == -1) { \
        s_bridge_debug = (getenv("RDP_DEBUG") || getenv("DEBUG")) ? 1 : 0; \
    } \
    if (s_bridge_debug) { \
        fprintf(stderr, __VA_ARGS__); \
        fflush(stderr); \
    } \
} while (0)

#define BRIDGE_ERROR(...) do { \
    fprintf(stderr, __VA_ARGS__); \
    fflush(stderr); \
} while (0)

typedef struct {
    rdpClientContext common;
    RdpBridgeFrameCallback on_frame;
    RdpBridgeAuthCallback on_auth;
    RdpBridgeStatusCallback on_status;
    void* user_data;
    BOOL is_connected;
    HANDLE thread;
    HANDLE stop_event;
    HANDLE auth_done_event;
    char* auth_redirect_url;
    char* gateway_token;
    DispClientContext* disp;
    CliprdrClientContext* cliprdr;
    RdpBridgeClipboardCallback on_clipboard;
    pthread_mutex_t clip_lock;
    char* local_clipboard_text;
} BridgeClientContext;

struct RdpBridgeContext {
    rdpContext* rdp_ctx;
    RdpBridgeFrameCallback on_frame;
    RdpBridgeAuthCallback on_auth;
    RdpBridgeStatusCallback on_status;
    RdpBridgeClipboardCallback on_clipboard;
    void* user_data;
    BOOL running;
};

static char* bridge_asprintf(const char* fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    char* str = NULL;
    size_t slen = 0;
    winpr_vasprintf(&str, &slen, fmt, ap);
    va_end(ap);
    return str;
}

static char* bridge_extract_code_from_url(const char* url) {
    if (!url) return NULL;
    const char* p = strstr(url, "code=");
    if (!p) {
        p = strstr(url, "code%3D");
        if (p) p += 7;
        else return NULL;
    } else {
        p += 5;
    }
    const char* end = p;
    while (*end && *end != '&' && *end != ' ' && *end != '\r' && *end != '\n') {
        end++;
    }
    size_t len = (size_t)(end - p);
    if (len == 0) return NULL;
    char* code = (char*)malloc(len + 1);
    if (!code) return NULL;
    memcpy(code, p, len);
    code[len] = '\0';
    return code;
}

static BOOL bridge_get_access_token(freerdp* instance, AccessTokenType tokenType, char** token, size_t count, ...) {
    BRIDGE_DEBUG("[BRIDGE DEBUG] bridge_get_access_token called: tokenType=%d count=%zu\n", (int)tokenType, count);
    fflush(stderr);
    if (!instance || !instance->context || !token) return FALSE;
    BridgeClientContext* bctx = (BridgeClientContext*)instance->context;

    if (tokenType == ACCESS_TOKEN_TYPE_AVD) {
        rdpSettings* settings = instance->context->settings;
        const char* gw_token = freerdp_settings_get_string(settings, FreeRDP_GatewayAccessToken);
        if (!gw_token || strlen(gw_token) == 0) {
            gw_token = bctx->gateway_token;
        }
        if (gw_token && strlen(gw_token) > 0) {
            BRIDGE_DEBUG("[BRIDGE DEBUG] Providing AVD gateway token (len=%zu)\n", strlen(gw_token));
            fflush(stderr);
            *token = strdup(gw_token);
            return TRUE;
        }
        BRIDGE_DEBUG("[BRIDGE DEBUG] No AVD gateway token found\n");
        fflush(stderr);
        return FALSE;
    }

    if (tokenType != ACCESS_TOKEN_TYPE_AAD) {
        BRIDGE_DEBUG("[BRIDGE DEBUG] Unknown tokenType=%d, returning FALSE\n", (int)tokenType);
        fflush(stderr);
        return FALSE;
    }

    va_list ap;
    va_start(ap, count);
    const char* scope = (count >= 1) ? va_arg(ap, const char*) : NULL;
    const char* req_cnf = (count >= 2) ? va_arg(ap, const char*) : NULL;
    va_end(ap);

    BRIDGE_DEBUG("[BRIDGE DEBUG] AAD token requested for scope=%s req_cnf=%s\n",
            scope ? scope : "(null)", req_cnf ? req_cnf : "(null)");
    fflush(stderr);

    if (!scope) return FALSE;

    rdpSettings* settings = instance->context->settings;
    const char* client_id = freerdp_settings_get_string(settings, FreeRDP_GatewayAvdClientID);
    if (!client_id || strlen(client_id) == 0) {
        client_id = "a85cf173-4192-42f8-81fa-777a763e6e2c";
    }

    char* redirect_uri = bridge_asprintf("ms-appx-web%%3a%%2f%%2fMicrosoft.AAD.BrokerPlugin%%2f%s", client_id);
    const char* endpoint = "https://login.microsoftonline.com/common/oauth2/v2.0/authorize";

    char* auth_url = bridge_asprintf("%s?client_id=%s&response_type=code&scope=%s&redirect_uri=%s",
                                     endpoint, client_id, scope, redirect_uri);

    BRIDGE_DEBUG("[BRIDGE DEBUG] Auth challenge URL constructed: %s\n", auth_url ? auth_url : "(null)");
    fflush(stderr);

    if (bctx->on_status) {
        bctx->on_status(bctx->user_data, 0, "Authenticating Cloud PC session...");
    }

    if (bctx->on_auth && auth_url) {
        BRIDGE_DEBUG("[BRIDGE DEBUG] Dispatching on_auth callback to UI\n");
        fflush(stderr);
        bctx->on_auth(bctx->user_data, auth_url);
    }

    // Wait for authentication response from GUI
    if (bctx->auth_done_event) {
        ResetEvent(bctx->auth_done_event);
        HANDLE wait_events[2];
        wait_events[0] = bctx->auth_done_event;
        wait_events[1] = bctx->stop_event;
        BRIDGE_DEBUG("[BRIDGE DEBUG] Waiting for auth response event...\n");
        fflush(stderr);
        DWORD wait_res = WaitForMultipleObjects(2, wait_events, FALSE, 120000); // 120s timeout

        if (wait_res != WAIT_OBJECT_0 || !bctx->auth_redirect_url) {
            BRIDGE_DEBUG("[BRIDGE DEBUG] Auth wait failed or timed out: wait_res=%u\n", (unsigned)wait_res);
            fflush(stderr);
            if (bctx->auth_redirect_url) {
                free(bctx->auth_redirect_url);
                bctx->auth_redirect_url = NULL;
            }
            free(auth_url);
            free(redirect_uri);
            return FALSE;
        }
    } else {
        free(auth_url);
        free(redirect_uri);
        return FALSE;
    }

    BRIDGE_DEBUG("[BRIDGE DEBUG] Auth response received: %s\n", bctx->auth_redirect_url);
    fflush(stderr);

    char* code = bridge_extract_code_from_url(bctx->auth_redirect_url);
    free(bctx->auth_redirect_url);
    bctx->auth_redirect_url = NULL;

    if (!code) {
        BRIDGE_DEBUG("[BRIDGE DEBUG] Failed to extract authorization code from redirect URL\n");
        fflush(stderr);
        free(auth_url);
        free(redirect_uri);
        return FALSE;
    }

    char* token_req = NULL;
    if (req_cnf && strlen(req_cnf) > 0) {
        token_req = bridge_asprintf("grant_type=authorization_code&code=%s&client_id=%s&scope=%s&redirect_uri=%s&req_cnf=%s",
                                    code, client_id, scope, redirect_uri, req_cnf);
    } else {
        token_req = bridge_asprintf("grant_type=authorization_code&code=%s&client_id=%s&scope=%s&redirect_uri=%s",
                                    code, client_id, scope, redirect_uri);
    }

    BRIDGE_DEBUG("[BRIDGE DEBUG] Calling client_common_get_access_token...\n");
    fflush(stderr);
    BOOL rc = client_common_get_access_token(instance, token_req, token);
    BRIDGE_DEBUG("[BRIDGE DEBUG] client_common_get_access_token returned rc=%d\n", (int)rc);
    fflush(stderr);

    free(token_req);
    free(code);
    free(auth_url);
    free(redirect_uri);
    return rc;
}

static BOOL bridge_end_paint(rdpContext* context) {
    if (!context || !context->gdi) return TRUE;
    BridgeClientContext* bctx = (BridgeClientContext*)context;
    rdpGdi* gdi = context->gdi;
    if (bctx && bctx->on_frame && gdi && gdi->primary_buffer) {
        bctx->on_frame(bctx->user_data, gdi->primary_buffer, gdi->width, gdi->height, gdi->stride);
    }
    return TRUE;
}

static BOOL bridge_desktop_resize(rdpContext* context) {
    if (!context || !context->gdi || !context->settings) return FALSE;
    UINT32 width = freerdp_settings_get_uint32(context->settings, FreeRDP_DesktopWidth);
    UINT32 height = freerdp_settings_get_uint32(context->settings, FreeRDP_DesktopHeight);
    BRIDGE_DEBUG("[BRIDGE DEBUG] bridge_desktop_resize called: new size %ux%u\n", width, height);
    fflush(stderr);
    if (!gdi_resize(context->gdi, width, height)) {
        BRIDGE_DEBUG("[BRIDGE DEBUG] gdi_resize failed!\n");
        fflush(stderr);
        return FALSE;
    }
    BridgeClientContext* bctx = (BridgeClientContext*)context;
    if (bctx && bctx->on_frame && context->gdi->primary_buffer) {
        bctx->on_frame(bctx->user_data, context->gdi->primary_buffer,
                       context->gdi->width, context->gdi->height, context->gdi->stride);
    }
    return TRUE;
}

static BOOL bridge_post_connect(freerdp* instance) {
    BRIDGE_DEBUG("[BRIDGE DEBUG] bridge_post_connect called\n");
    fflush(stderr);
    if (!instance || !instance->context) return FALSE;
    if (!gdi_init(instance, PIXEL_FORMAT_BGRX32)) {
        BRIDGE_DEBUG("[BRIDGE DEBUG] gdi_init failed!\n");
        fflush(stderr);
        return FALSE;
    }
    freerdp_keyboard_init(0);
    instance->context->update->DesktopResize = bridge_desktop_resize;
    instance->context->update->EndPaint = bridge_end_paint;
    BridgeClientContext* bctx = (BridgeClientContext*)instance->context;
    bctx->is_connected = TRUE;
    if (bctx->on_status) {
        bctx->on_status(bctx->user_data, 1, "Connected to remote desktop session");
    }
    BRIDGE_DEBUG("[BRIDGE DEBUG] bridge_post_connect succeeded\n");
    fflush(stderr);
    return TRUE;
}

static BOOL bridge_pre_connect(freerdp* instance) {
    BRIDGE_DEBUG("[BRIDGE DEBUG] bridge_pre_connect called\n");
    fflush(stderr);
    if (!instance || !instance->context || !instance->context->settings) return FALSE;
    freerdp_settings_set_bool(instance->context->settings, FreeRDP_SupportGraphicsPipeline, FALSE);
    freerdp_settings_set_bool(instance->context->settings, FreeRDP_RemoteFxCodec, TRUE);
    return freerdp_client_load_channels(instance);
}

static DWORD WINAPI bridge_client_thread(LPVOID lpParam) {
    BRIDGE_DEBUG("[BRIDGE DEBUG] bridge_client_thread started\n");
    fflush(stderr);
    rdpContext* context = (rdpContext*)lpParam;
    BridgeClientContext* bctx = (BridgeClientContext*)context;
    freerdp* instance = context->instance;

    if (bctx->on_status) {
        bctx->on_status(bctx->user_data, 0, "Connecting to Cloud PC session...");
    }

    BRIDGE_DEBUG("[BRIDGE DEBUG] Calling freerdp_connect...\n");
    fflush(stderr);
    if (!freerdp_connect(instance)) {
        UINT32 err = freerdp_get_last_error(context);
        const char* err_str = freerdp_get_last_error_string(err);
        BRIDGE_DEBUG("[BRIDGE DEBUG] freerdp_connect failed: err=%u err_str=%s\n",
                (unsigned)err, err_str ? err_str : "(null)");
        fflush(stderr);
        if (bctx->on_status) {
            bctx->on_status(bctx->user_data, -1, err_str ? err_str : "Connection failed");
        }
        bctx->is_connected = FALSE;
        return 1;
    }
    BRIDGE_DEBUG("[BRIDGE DEBUG] freerdp_connect succeeded! Entering event loop\n");
    fflush(stderr);

    HANDLE handles[MAXIMUM_WAIT_OBJECTS];
    while (!freerdp_shall_disconnect_context(context)) {
        DWORD count = freerdp_get_event_handles(context, handles, MAXIMUM_WAIT_OBJECTS - 1);
        if (count == 0) {
            break;
        }

        handles[count] = bctx->stop_event;
        DWORD status = WaitForMultipleObjects(count + 1, handles, FALSE, 100);

        if (status == WAIT_FAILED) {
            break;
        }
        if (status == WAIT_OBJECT_0 + count) {
            // Stop event was signaled
            break;
        }

        if (!freerdp_check_event_handles(context)) {
            if (freerdp_get_last_error(context) != FREERDP_ERROR_SUCCESS) {
                break;
            }
        }
    }

    BRIDGE_DEBUG("[BRIDGE DEBUG] Disconnecting session\n");
    fflush(stderr);
    freerdp_disconnect(instance);
    bctx->is_connected = FALSE;
    if (bctx->on_status) {
        bctx->on_status(bctx->user_data, 2, "Session ended");
    }
    return 0;
}

static int bridge_client_start(rdpContext* context) {
    BRIDGE_DEBUG("[BRIDGE DEBUG] bridge_client_start called\n");
    fflush(stderr);
    if (!context) return -1;
    BridgeClientContext* bctx = (BridgeClientContext*)context;
    bctx->stop_event = CreateEvent(NULL, TRUE, FALSE, NULL);
    bctx->auth_done_event = CreateEvent(NULL, TRUE, FALSE, NULL);
    bctx->thread = CreateThread(NULL, 0, bridge_client_thread, context, 0, NULL);
    if (!bctx->thread) {
        BRIDGE_DEBUG("[BRIDGE DEBUG] CreateThread failed!\n");
        fflush(stderr);
        if (bctx->stop_event) {
            CloseHandle(bctx->stop_event);
            bctx->stop_event = NULL;
        }
        if (bctx->auth_done_event) {
            CloseHandle(bctx->auth_done_event);
            bctx->auth_done_event = NULL;
        }
        return -1;
    }
    BRIDGE_DEBUG("[BRIDGE DEBUG] bridge_client_start finished successfully\n");
    fflush(stderr);
    return 0;
}

static int bridge_client_stop(rdpContext* context) {
    BRIDGE_DEBUG("[BRIDGE DEBUG] bridge_client_stop called\n");
    fflush(stderr);
    if (!context) return -1;
    BridgeClientContext* bctx = (BridgeClientContext*)context;
    if (bctx->stop_event) {
        SetEvent(bctx->stop_event);
    }
    if (bctx->auth_done_event) {
        SetEvent(bctx->auth_done_event);
    }
    freerdp_abort_connect_context(context);
    if (bctx->thread) {
        WaitForSingleObject(bctx->thread, 2000);
        CloseHandle(bctx->thread);
        bctx->thread = NULL;
    }
    if (bctx->stop_event) {
        CloseHandle(bctx->stop_event);
        bctx->stop_event = NULL;
    }
    if (bctx->auth_done_event) {
        CloseHandle(bctx->auth_done_event);
        bctx->auth_done_event = NULL;
    }
    if (bctx->auth_redirect_url) {
        free(bctx->auth_redirect_url);
        bctx->auth_redirect_url = NULL;
    }
    return 0;
}

static UINT bridge_display_control_caps(DispClientContext* disp, UINT32 maxNumMonitors,
                                        UINT32 maxMonitorAreaFactorA, UINT32 maxMonitorAreaFactorB) {
    BRIDGE_DEBUG("[BRIDGE DEBUG] DisplayControlCaps received: maxMonitors=%u areaA=%u areaB=%u\n",
            maxNumMonitors, maxMonitorAreaFactorA, maxMonitorAreaFactorB);
    fflush(stderr);
    return CHANNEL_RC_OK;
}

static UINT bridge_cliprdr_server_capabilities(CliprdrClientContext* cliprdr, const CLIPRDR_CAPABILITIES* capabilities) {
    (void)capabilities;
    CLIPRDR_CAPABILITIES clientCapabilities;
    CLIPRDR_GENERAL_CAPABILITY_SET generalCaps;
    memset(&clientCapabilities, 0, sizeof(clientCapabilities));
    memset(&generalCaps, 0, sizeof(generalCaps));

    generalCaps.capabilitySetType = CB_CAPSTYPE_GENERAL;
    generalCaps.capabilitySetLength = CB_CAPSTYPE_GENERAL_LEN;
    generalCaps.version = CB_CAPS_VERSION_2;
    generalCaps.generalFlags = CB_USE_LONG_FORMAT_NAMES;

    clientCapabilities.cCapabilitiesSets = 1;
    clientCapabilities.capabilitySets = (CLIPRDR_CAPABILITY_SET*)&generalCaps;

    return cliprdr->ClientCapabilities(cliprdr, &clientCapabilities);
}

static UINT bridge_cliprdr_monitor_ready(CliprdrClientContext* cliprdr, const CLIPRDR_MONITOR_READY* monitorReady) {
    (void)monitorReady;
    if (!cliprdr) return CHANNEL_RC_OK;
    BridgeClientContext* bctx = (BridgeClientContext*)cliprdr->custom;

    CLIPRDR_FORMAT_LIST formatList;
    CLIPRDR_FORMAT format[2];
    memset(&formatList, 0, sizeof(formatList));
    memset(format, 0, sizeof(format));

    format[0].formatId = CF_UNICODETEXT;
    format[1].formatId = CF_TEXT;
    formatList.common.msgFlags = CB_RESPONSE_OK;
    formatList.numFormats = 2;
    formatList.formats = format;

    return cliprdr->ClientFormatList(cliprdr, &formatList);
}

static UINT bridge_cliprdr_server_format_list(CliprdrClientContext* cliprdr, const CLIPRDR_FORMAT_LIST* formatList) {
    if (!cliprdr || !cliprdr->custom || !formatList) return CHANNEL_RC_OK;
    BridgeClientContext* bctx = (BridgeClientContext*)cliprdr->custom;

    CLIPRDR_FORMAT_LIST_RESPONSE resp;
    memset(&resp, 0, sizeof(resp));
    resp.common.msgType = CB_FORMAT_LIST_RESPONSE;
    resp.common.msgFlags = CB_RESPONSE_OK;
    cliprdr->ClientFormatListResponse(cliprdr, &resp);

    BOOL has_unicode = FALSE;
    for (UINT32 i = 0; i < formatList->numFormats; i++) {
        if (formatList->formats[i].formatId == CF_UNICODETEXT) {
            has_unicode = TRUE;
            break;
        }
    }

    if (has_unicode) {
        CLIPRDR_FORMAT_DATA_REQUEST req;
        memset(&req, 0, sizeof(req));
        req.common.msgType = CB_FORMAT_DATA_REQUEST;
        req.requestedFormatId = CF_UNICODETEXT;
        cliprdr->ClientFormatDataRequest(cliprdr, &req);
    }
    return CHANNEL_RC_OK;
}

static UINT bridge_cliprdr_server_format_list_response(CliprdrClientContext* cliprdr, const CLIPRDR_FORMAT_LIST_RESPONSE* formatListResponse) {
    (void)cliprdr;
    (void)formatListResponse;
    return CHANNEL_RC_OK;
}

static UINT bridge_cliprdr_server_format_data_request(CliprdrClientContext* cliprdr, const CLIPRDR_FORMAT_DATA_REQUEST* req) {
    if (!cliprdr || !cliprdr->custom || !req) return CHANNEL_RC_OK;
    BridgeClientContext* bctx = (BridgeClientContext*)cliprdr->custom;

    CLIPRDR_FORMAT_DATA_RESPONSE resp;
    memset(&resp, 0, sizeof(resp));
    resp.common.msgType = CB_FORMAT_DATA_RESPONSE;

    pthread_mutex_lock(&bctx->clip_lock);
    const char* text = bctx->local_clipboard_text;
    if (!text || strlen(text) == 0) {
        pthread_mutex_unlock(&bctx->clip_lock);
        resp.common.msgFlags = CB_RESPONSE_FAIL;
        return cliprdr->ClientFormatDataResponse(cliprdr, &resp);
    }

    if (req->requestedFormatId == CF_UNICODETEXT) {
        size_t wlen = 0;
        WCHAR* wstr = ConvertUtf8ToWCharAlloc(text, &wlen);
        pthread_mutex_unlock(&bctx->clip_lock);
        if (!wstr) {
            resp.common.msgFlags = CB_RESPONSE_FAIL;
            return cliprdr->ClientFormatDataResponse(cliprdr, &resp);
        }
        resp.common.msgFlags = CB_RESPONSE_OK;
        resp.common.dataLen = (UINT32)((wlen + 1) * sizeof(WCHAR));
        resp.requestedFormatData = (const BYTE*)wstr;
        UINT ret = cliprdr->ClientFormatDataResponse(cliprdr, &resp);
        free(wstr);
        return ret;
    } else if (req->requestedFormatId == CF_TEXT) {
        size_t len = strlen(text);
        char* cstr = strdup(text);
        pthread_mutex_unlock(&bctx->clip_lock);
        if (!cstr) {
            resp.common.msgFlags = CB_RESPONSE_FAIL;
            return cliprdr->ClientFormatDataResponse(cliprdr, &resp);
        }
        resp.common.msgFlags = CB_RESPONSE_OK;
        resp.common.dataLen = (UINT32)(len + 1);
        resp.requestedFormatData = (const BYTE*)cstr;
        UINT ret = cliprdr->ClientFormatDataResponse(cliprdr, &resp);
        free(cstr);
        return ret;
    } else {
        pthread_mutex_unlock(&bctx->clip_lock);
        resp.common.msgFlags = CB_RESPONSE_FAIL;
        return cliprdr->ClientFormatDataResponse(cliprdr, &resp);
    }
}

static UINT bridge_cliprdr_server_format_data_response(CliprdrClientContext* cliprdr, const CLIPRDR_FORMAT_DATA_RESPONSE* response) {
    if (!cliprdr || !cliprdr->custom || !response) return CHANNEL_RC_OK;
    BridgeClientContext* bctx = (BridgeClientContext*)cliprdr->custom;

    if ((response->common.msgFlags & CB_RESPONSE_FAIL) || !response->requestedFormatData || response->common.dataLen < sizeof(WCHAR)) {
        return CHANNEL_RC_OK;
    }

    size_t num_wchars = response->common.dataLen / sizeof(WCHAR);
    WCHAR* tmp_wstr = (WCHAR*)malloc((num_wchars + 1) * sizeof(WCHAR));
    if (tmp_wstr) {
        memcpy(tmp_wstr, response->requestedFormatData, num_wchars * sizeof(WCHAR));
        tmp_wstr[num_wchars] = 0;
        size_t utf_len = 0;
        char* utf8 = ConvertWCharToUtf8Alloc(tmp_wstr, &utf_len);
        free(tmp_wstr);
        if (utf8) {
            if (bctx->on_clipboard) {
                bctx->on_clipboard(bctx->user_data, utf8);
            }
            free(utf8);
        }
    }
    return CHANNEL_RC_OK;
}

static void bridge_cliprdr_init(BridgeClientContext* bctx, CliprdrClientContext* cliprdr) {
    cliprdr->custom = bctx;
    cliprdr->ServerCapabilities = bridge_cliprdr_server_capabilities;
    cliprdr->MonitorReady = bridge_cliprdr_monitor_ready;
    cliprdr->ServerFormatList = bridge_cliprdr_server_format_list;
    cliprdr->ServerFormatListResponse = bridge_cliprdr_server_format_list_response;
    cliprdr->ServerFormatDataRequest = bridge_cliprdr_server_format_data_request;
    cliprdr->ServerFormatDataResponse = bridge_cliprdr_server_format_data_response;
}

static void bridge_on_channel_connected(void* context, const ChannelConnectedEventArgs* e) {
    if (!context || !e) return;
    BridgeClientContext* bctx = (BridgeClientContext*)context;
    if (e->name && strcmp(e->name, DISP_DVC_CHANNEL_NAME) == 0) {
        bctx->disp = (DispClientContext*)e->pInterface;
        bctx->disp->DisplayControlCaps = bridge_display_control_caps;
        BRIDGE_DEBUG("[BRIDGE DEBUG] DisplayControl channel connected!\n");
        fflush(stderr);
    } else if (e->name && strcmp(e->name, CLIPRDR_SVC_CHANNEL_NAME) == 0) {
        CliprdrClientContext* cliprdr = (CliprdrClientContext*)e->pInterface;
        bctx->cliprdr = cliprdr;
        bridge_cliprdr_init(bctx, cliprdr);
        BRIDGE_DEBUG("[BRIDGE DEBUG] Clipboard (cliprdr) channel connected!\n");
        fflush(stderr);
    }
    freerdp_client_OnChannelConnectedEventHandler(context, e);
}

static void bridge_on_channel_disconnected(void* context, const ChannelDisconnectedEventArgs* e) {
    if (!context || !e) return;
    BridgeClientContext* bctx = (BridgeClientContext*)context;
    if (e->name && strcmp(e->name, DISP_DVC_CHANNEL_NAME) == 0) {
        bctx->disp = NULL;
        BRIDGE_DEBUG("[BRIDGE DEBUG] DisplayControl channel disconnected\n");
        fflush(stderr);
    } else if (e->name && strcmp(e->name, CLIPRDR_SVC_CHANNEL_NAME) == 0) {
        if (bctx->cliprdr) {
            bctx->cliprdr->custom = NULL;
            bctx->cliprdr = NULL;
        }
        BRIDGE_DEBUG("[BRIDGE DEBUG] Clipboard (cliprdr) channel disconnected\n");
        fflush(stderr);
    }
    freerdp_client_OnChannelDisconnectedEventHandler(context, e);
}

static BOOL bridge_client_new(freerdp* instance, rdpContext* context) {
    BRIDGE_DEBUG("[BRIDGE DEBUG] bridge_client_new called\n");
    fflush(stderr);
    BridgeClientContext* bctx = (BridgeClientContext*)context;
    if (bctx) {
        pthread_mutex_init(&bctx->clip_lock, NULL);
        bctx->local_clipboard_text = NULL;
        bctx->cliprdr = NULL;
    }
    instance->PreConnect = bridge_pre_connect;
    instance->PostConnect = bridge_post_connect;
    instance->AuthenticateEx = client_cli_authenticate_ex;
    instance->VerifyCertificateEx = client_cli_verify_certificate_ex;
    instance->VerifyChangedCertificateEx = client_cli_verify_changed_certificate_ex;
    instance->GetAccessToken = bridge_get_access_token;
    if (context && context->pubSub) {
        PubSub_SubscribeChannelConnected(context->pubSub, bridge_on_channel_connected);
        PubSub_SubscribeChannelDisconnected(context->pubSub, bridge_on_channel_disconnected);
    }
    return TRUE;
}

static void bridge_client_free(freerdp* instance, rdpContext* context) {
    BRIDGE_DEBUG("[BRIDGE DEBUG] bridge_client_free called\n");
    fflush(stderr);
    (void)instance;
    if (!context) return;
    BridgeClientContext* bctx = (BridgeClientContext*)context;
    pthread_mutex_lock(&bctx->clip_lock);
    if (bctx->local_clipboard_text) {
        free(bctx->local_clipboard_text);
        bctx->local_clipboard_text = NULL;
    }
    pthread_mutex_unlock(&bctx->clip_lock);
    pthread_mutex_destroy(&bctx->clip_lock);
    bctx->cliprdr = NULL;
    if (bctx->stop_event) {
        CloseHandle(bctx->stop_event);
        bctx->stop_event = NULL;
    }
    if (bctx->auth_done_event) {
        CloseHandle(bctx->auth_done_event);
        bctx->auth_done_event = NULL;
    }
    if (bctx->auth_redirect_url) {
        free(bctx->auth_redirect_url);
        bctx->auth_redirect_url = NULL;
    }
    if (bctx->gateway_token) {
        free(bctx->gateway_token);
        bctx->gateway_token = NULL;
    }
    if (bctx->thread) {
        CloseHandle(bctx->thread);
        bctx->thread = NULL;
    }
    // pubSub is destroyed by freerdp_client_context_free
    bctx->disp = NULL;
}

static pthread_mutex_t g_bridge_lock = PTHREAD_MUTEX_INITIALIZER;

RdpBridgeContext* freerdp_bridge_create(
    const char** args,
    int argc,
    RdpBridgeFrameCallback on_frame,
    RdpBridgeAuthCallback on_auth,
    RdpBridgeStatusCallback on_status,
    RdpBridgeClipboardCallback on_clipboard,
    void* user_data
) {
    pthread_mutex_lock(&g_bridge_lock);
    BRIDGE_DEBUG("[BRIDGE DEBUG] freerdp_bridge_create called with argc=%d\n", argc);
    fflush(stderr);
    RDP_CLIENT_ENTRY_POINTS ep;
    ZeroMemory(&ep, sizeof(ep));
    ep.Size = sizeof(ep);
    ep.Version = RDP_CLIENT_INTERFACE_VERSION;
    ep.ContextSize = sizeof(BridgeClientContext);
    ep.ClientNew = bridge_client_new;
    ep.ClientFree = bridge_client_free;
    ep.ClientStart = bridge_client_start;
    ep.ClientStop = bridge_client_stop;

    rdpContext* ctx = freerdp_client_context_new(&ep);
    if (!ctx) {
        BRIDGE_DEBUG("[BRIDGE DEBUG] freerdp_client_context_new failed!\n");
        fflush(stderr);
        return NULL;
    }

    if (!getenv("WLOG_LEVEL")) {
        wLog* root_log = WLog_GetRoot();
        if (root_log) {
            WLog_SetLogLevel(root_log, WLOG_WARN);
        }
    }


    BridgeClientContext* bctx = (BridgeClientContext*)ctx;
    bctx->on_frame = on_frame;
    bctx->on_auth = on_auth;
    bctx->on_status = on_status;
    bctx->on_clipboard = on_clipboard;
    bctx->user_data = user_data;
    bctx->is_connected = FALSE;
    bctx->auth_redirect_url = NULL;
    bctx->gateway_token = NULL;

    if (argc > 0 && args) {
        char** argv = (char**)calloc(argc, sizeof(char*));
        if (!argv) {
            freerdp_client_context_free(ctx);
            return NULL;
        }
        for (int i = 0; i < argc; i++) {
            if (args[i]) {
                argv[i] = strdup(args[i]);
                const char* b = strstr(args[i], "bearer:");
                if (b && !bctx->gateway_token) {
                    bctx->gateway_token = strdup(b + 7);
                }
            }
        }
        BRIDGE_DEBUG("[BRIDGE DEBUG] Parsing FreeRDP settings command line...\n");
        fflush(stderr);
        int res = freerdp_client_settings_parse_command_line(ctx->settings, argc, argv, TRUE);
        BRIDGE_DEBUG("[BRIDGE DEBUG] Parse command line returned res=%d\n", res);
        fflush(stderr);
        for (int i = 0; i < argc; i++) {
            if (argv[i]) free(argv[i]);
        }
        free(argv);

        if (res < 0) {
            BRIDGE_DEBUG("[BRIDGE DEBUG] Settings parse error (%d)\n", res);
            fflush(stderr);
            freerdp_client_context_free(ctx);
            return NULL;
        }
        freerdp_settings_set_bool(ctx->settings, FreeRDP_SupportGraphicsPipeline, FALSE);
        freerdp_settings_set_bool(ctx->settings, FreeRDP_RemoteFxCodec, TRUE);
        freerdp_settings_set_bool(ctx->settings, FreeRDP_SupportDisplayControl, TRUE);
        freerdp_settings_set_bool(ctx->settings, FreeRDP_DynamicResolutionUpdate, TRUE);

        if (freerdp_settings_get_uint32(ctx->settings, FreeRDP_DeviceCount) > 0) {
            freerdp_settings_set_bool(ctx->settings, FreeRDP_DeviceRedirection, TRUE);
            wLog* rdpdr_log = WLog_Get("com.freerdp.channels.rdpdr.client");
            if (rdpdr_log) WLog_SetLogLevel(rdpdr_log, WLOG_INFO);
            wLog* drive_log = WLog_Get("com.freerdp.channels.drive.client");
            if (drive_log) WLog_SetLogLevel(drive_log, WLOG_INFO);
        }
    }

    RdpBridgeContext* bridge = (RdpBridgeContext*)calloc(1, sizeof(RdpBridgeContext));
    if (!bridge) {
        freerdp_client_context_free(ctx);
        pthread_mutex_unlock(&g_bridge_lock);
        return NULL;
    }

    bridge->rdp_ctx = ctx;
    bridge->on_frame = on_frame;
    bridge->on_auth = on_auth;
    bridge->on_status = on_status;
    bridge->on_clipboard = on_clipboard;
    bridge->user_data = user_data;
    bridge->running = FALSE;

    BRIDGE_DEBUG("[BRIDGE DEBUG] freerdp_bridge_create finished successfully\n");
    fflush(stderr);
    pthread_mutex_unlock(&g_bridge_lock);
    return bridge;
}

bool freerdp_bridge_start(RdpBridgeContext* ctx) {
    BRIDGE_DEBUG("[BRIDGE DEBUG] freerdp_bridge_start called\n");
    fflush(stderr);
    if (!ctx || !ctx->rdp_ctx) return false;
    int res = freerdp_client_start(ctx->rdp_ctx);
    BRIDGE_DEBUG("[BRIDGE DEBUG] freerdp_client_start returned res=%d\n", res);
    fflush(stderr);
    if (res == 0) {
        ctx->running = TRUE;
        return true;
    }
    return false;
}

void freerdp_bridge_stop(RdpBridgeContext* ctx) {
    BRIDGE_DEBUG("[BRIDGE DEBUG] freerdp_bridge_stop called\n");
    fflush(stderr);
    if (!ctx || !ctx->rdp_ctx) return;
    if (ctx->running) {
        freerdp_client_stop(ctx->rdp_ctx);
        ctx->running = FALSE;
    }
}

void freerdp_bridge_free(RdpBridgeContext* ctx) {
    pthread_mutex_lock(&g_bridge_lock);
    BRIDGE_DEBUG("[BRIDGE DEBUG] freerdp_bridge_free called\n");
    fflush(stderr);
    if (!ctx) {
        pthread_mutex_unlock(&g_bridge_lock);
        return;
    }
    freerdp_bridge_stop(ctx);
    if (ctx->rdp_ctx) {
        freerdp_client_context_free(ctx->rdp_ctx);
        ctx->rdp_ctx = NULL;
    }
    free(ctx);
    pthread_mutex_unlock(&g_bridge_lock);
}

void freerdp_bridge_provide_auth_result(RdpBridgeContext* ctx, const char* redirect_url) {
    BRIDGE_DEBUG("[BRIDGE DEBUG] freerdp_bridge_provide_auth_result: url=%s\n", redirect_url ? redirect_url : "(null)");
    fflush(stderr);
    if (!ctx || !ctx->rdp_ctx) return;
    BridgeClientContext* bctx = (BridgeClientContext*)ctx->rdp_ctx;
    if (bctx->auth_redirect_url) {
        free(bctx->auth_redirect_url);
        bctx->auth_redirect_url = NULL;
    }
    if (redirect_url) {
        bctx->auth_redirect_url = strdup(redirect_url);
    }
    if (bctx->auth_done_event) {
        SetEvent(bctx->auth_done_event);
    }
}

void freerdp_bridge_send_mouse(RdpBridgeContext* ctx, uint16_t flags, uint16_t x, uint16_t y) {
    if (!ctx || !ctx->rdp_ctx || !ctx->rdp_ctx->input) return;
    freerdp_input_send_mouse_event(ctx->rdp_ctx->input, flags, x, y);
}

void freerdp_bridge_send_wheel(RdpBridgeContext* ctx, uint16_t flags, uint16_t x, uint16_t y) {
    if (!ctx || !ctx->rdp_ctx || !ctx->rdp_ctx->input) return;
    freerdp_input_send_mouse_event(ctx->rdp_ctx->input, flags, x, y);
}

void freerdp_bridge_send_keyboard(RdpBridgeContext* ctx, uint16_t flags, uint8_t code) {
    if (!ctx || !ctx->rdp_ctx || !ctx->rdp_ctx->input) return;
    freerdp_input_send_keyboard_event(ctx->rdp_ctx->input, flags, code);
}

void freerdp_bridge_send_key(RdpBridgeContext* ctx, bool down, uint32_t keycode) {
    if (!ctx || !ctx->rdp_ctx || !ctx->rdp_ctx->input) return;
    UINT32 rdp_scancode = freerdp_keyboard_get_rdp_scancode_from_x11_keycode(keycode);
    if (rdp_scancode == 0) return;
    freerdp_input_send_keyboard_event_ex(ctx->rdp_ctx->input, down ? TRUE : FALSE, FALSE, rdp_scancode);
}

void freerdp_bridge_send_unicode(RdpBridgeContext* ctx, uint16_t flags, uint16_t code) {
    if (!ctx || !ctx->rdp_ctx || !ctx->rdp_ctx->input) return;
    freerdp_input_send_unicode_keyboard_event(ctx->rdp_ctx->input, flags, code);
}

void freerdp_bridge_send_resize(RdpBridgeContext* ctx, uint32_t width, uint32_t height, uint32_t desktop_scale) {
    if (!ctx || !ctx->rdp_ctx) return;
    BridgeClientContext* bctx = (BridgeClientContext*)ctx->rdp_ctx;
    if (!bctx || !bctx->disp || !bctx->disp->SendMonitorLayout) return;

    if (width < DISPLAY_CONTROL_MIN_MONITOR_WIDTH || height < DISPLAY_CONTROL_MIN_MONITOR_HEIGHT) return;

    width = (width + 1) & ~1;
    height = (height + 1) & ~1;

    (void)desktop_scale;

    DISPLAY_CONTROL_MONITOR_LAYOUT layout;
    ZeroMemory(&layout, sizeof(layout));
    layout.Flags = DISPLAY_CONTROL_MONITOR_PRIMARY;
    layout.Left = 0;
    layout.Top = 0;
    layout.Width = width;
    layout.Height = height;
    layout.Orientation = ORIENTATION_LANDSCAPE;
    layout.DesktopScaleFactor = 100;
    layout.DeviceScaleFactor = 100;

    BRIDGE_DEBUG("[BRIDGE DEBUG] Sending dynamic resolution resize to server: %ux%u (scale=100%%)\n",
            width, height);
    fflush(stderr);
    bctx->disp->SendMonitorLayout(bctx->disp, 1, &layout);
}

void freerdp_bridge_send_clipboard_text(RdpBridgeContext* ctx, const char* text) {
    if (!ctx || !ctx->rdp_ctx) return;
    BridgeClientContext* bctx = (BridgeClientContext*)ctx->rdp_ctx;
    if (!bctx) return;

    pthread_mutex_lock(&bctx->clip_lock);
    if (bctx->local_clipboard_text) {
        free(bctx->local_clipboard_text);
        bctx->local_clipboard_text = NULL;
    }
    if (text && strlen(text) > 0) {
        bctx->local_clipboard_text = strdup(text);
    }
    pthread_mutex_unlock(&bctx->clip_lock);

    if (bctx->cliprdr && bctx->cliprdr->ClientFormatList) {
        CLIPRDR_FORMAT_LIST formatList;
        CLIPRDR_FORMAT format[2];
        memset(&formatList, 0, sizeof(formatList));
        memset(format, 0, sizeof(format));

        format[0].formatId = CF_UNICODETEXT;
        format[1].formatId = CF_TEXT;
        formatList.common.msgFlags = CB_RESPONSE_OK;
        formatList.numFormats = 2;
        formatList.formats = format;

        bctx->cliprdr->ClientFormatList(bctx->cliprdr, &formatList);
    }
}
