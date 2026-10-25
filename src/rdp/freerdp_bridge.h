#ifndef FREERDP_BRIDGE_H
#define FREERDP_BRIDGE_H

#include <stdint.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef void (*RdpBridgeFrameCallback)(void* user_data, const uint8_t* pixels, uint32_t width, uint32_t height, uint32_t stride);
typedef void (*RdpBridgeAuthCallback)(void* user_data, const char* auth_url);
typedef void (*RdpBridgeStatusCallback)(void* user_data, int status_code, const char* message);
typedef void (*RdpBridgeClipboardCallback)(void* user_data, const char* text);

typedef struct RdpBridgeContext RdpBridgeContext;

RdpBridgeContext* freerdp_bridge_create(
    const char** args,
    int argc,
    RdpBridgeFrameCallback on_frame,
    RdpBridgeAuthCallback on_auth,
    RdpBridgeStatusCallback on_status,
    RdpBridgeClipboardCallback on_clipboard,
    void* user_data
);

bool freerdp_bridge_start(RdpBridgeContext* ctx);
void freerdp_bridge_stop(RdpBridgeContext* ctx);
void freerdp_bridge_free(RdpBridgeContext* ctx);

void freerdp_bridge_provide_auth_result(RdpBridgeContext* ctx, const char* redirect_url);

void freerdp_bridge_send_mouse(RdpBridgeContext* ctx, uint16_t flags, uint16_t x, uint16_t y);
void freerdp_bridge_send_wheel(RdpBridgeContext* ctx, uint16_t flags, uint16_t x, uint16_t y);
void freerdp_bridge_send_key(RdpBridgeContext* ctx, bool down, uint32_t keycode);
void freerdp_bridge_send_keyboard(RdpBridgeContext* ctx, uint16_t flags, uint8_t code);
void freerdp_bridge_send_unicode(RdpBridgeContext* ctx, uint16_t flags, uint16_t code);
void freerdp_bridge_send_resize(RdpBridgeContext* ctx, uint32_t width, uint32_t height, uint32_t desktop_scale);
void freerdp_bridge_send_clipboard_text(RdpBridgeContext* ctx, const char* text);

#ifdef __cplusplus
}
#endif

#endif /* FREERDP_BRIDGE_H */
