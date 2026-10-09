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
// getters write their value and return the same. lfp_get_float also reads an int-stored key.
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
// (returns 1 once SteamVR asks the process to quit; it renders only after input, so polling it
// often is cheap); visible tells whether the tab is up. Close also after a failed open.
int lfp_overlay_open(const char* thumbnail, char* err, size_t err_len);
int lfp_overlay_frame(void);
int lfp_overlay_visible(void);
void lfp_overlay_close(void);

// Any other OpenVR interface by version string, once connected; null with err filled otherwise.
void* lfp_interface(const char* version, char* err, size_t err_len);

// The panel as a registered overlay app (manifest = full path of leapfrog-panel.vrmanifest).
int lfp_app_installed(const char* key);
int lfp_app_register(const char* manifest, const char* key, char* err, size_t err_len);
int lfp_app_unregister(const char* manifest, char* err, size_t err_len);
int lfp_app_launch(const char* key, char* err, size_t err_len);

// A process to wait on: open (null when it cannot be opened), wait up to ms (1 once it has
// ended), close.
void* lfp_process_open(unsigned pid);
int lfp_process_wait(void* process, unsigned ms);
void lfp_process_close(void* process);

#ifdef __cplusplus
}
#endif
