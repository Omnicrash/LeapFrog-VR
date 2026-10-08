// The C face of the panel's OpenVR side (panel.cpp), called from Rust (src/main.rs).
#pragma once
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

// Loads openvr_api.dll from the given path and inits as a background app (0) or an overlay (1).
// 0 on success; otherwise the init error's description lands in err.
int lfp_connect(const wchar_t* dll, int overlay, char* err, size_t err_len);
void lfp_disconnect(void);

// driver_leapfrog settings. The setters return 0 on success, else the IVRSettings error code; the
// getters write their value and return the same.
int lfp_set_float(const char* key, float value);
int lfp_set_bool(const char* key, int value);
int lfp_set_int(const char* key, int value);
int lfp_set_string(const char* key, const char* value);
int lfp_get_float(const char* key, float* value);
int lfp_get_bool(const char* key, int* value);
int lfp_get_int(const char* key, int* value);
int lfp_get_string(const char* key, char* value, size_t len);
const char* lfp_error_name(int code);

// The dashboard tab (overlay.cpp): open after an overlay-type connect, then call frame in a loop
// (returns 1 once SteamVR asks the process to quit); visible tells whether the tab is up.
int lfp_overlay_open(const char* thumbnail, char* err, size_t err_len);
int lfp_overlay_frame(void);
int lfp_overlay_visible(void);
void lfp_overlay_close(void);

// Any other OpenVR interface by version string, once connected; null with err filled otherwise.
void* lfp_interface(const char* version, char* err, size_t err_len);

// 1 once the given process has ended (or cannot be opened).
int lfp_process_gone(unsigned pid);

#ifdef __cplusplus
}
#endif
