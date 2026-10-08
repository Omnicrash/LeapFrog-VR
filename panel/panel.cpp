// The panel's OpenVR client side: openvr_api.dll loaded by hand (no import library, the runtime
// is wherever SteamVR is installed), the driver's settings section read and written through
// IVRSettings. vrserver keeps the settings in memory and writes the file itself, so a value set
// here reaches the driver's next read and survives a restart.
#include "panel.h"

#include <windows.h>
#include <openvr.h>

#include <cstdio>
#include <cstring>

namespace {

const char* const SECTION = "driver_leapfrog";

HMODULE g_api;
vr::IVRSettings* g_settings;
void (*g_shutdown)();

template <typename T>
T proc(const char* name) {
    return reinterpret_cast<T>(GetProcAddress(g_api, name));
}

}  // namespace

extern "C" int lfp_connect(const wchar_t* dll, int overlay, char* err, size_t err_len) {
    g_api = LoadLibraryW(dll);
    if (!g_api) {
        snprintf(err, err_len, "openvr_api.dll not loaded (error %lu)", GetLastError());
        return 1;
    }
    auto init = proc<uint32_t (*)(vr::EVRInitError*, vr::EVRApplicationType, const char*)>("VR_InitInternal2");
    auto iface = proc<void* (*)(const char*, vr::EVRInitError*)>("VR_GetGenericInterface");
    auto describe = proc<const char* (*)(vr::EVRInitError)>("VR_GetVRInitErrorAsEnglishDescription");
    g_shutdown = proc<void (*)()>("VR_ShutdownInternal");
    if (!init || !iface || !describe || !g_shutdown) {
        snprintf(err, err_len, "openvr_api.dll exports missing");
        return 1;
    }
    vr::EVRInitError error = vr::VRInitError_None;
    init(&error, overlay ? vr::VRApplication_Overlay : vr::VRApplication_Background, nullptr);
    if (error != vr::VRInitError_None) {
        snprintf(err, err_len, "%s", describe(error));
        return 1;
    }
    g_settings = static_cast<vr::IVRSettings*>(iface(vr::IVRSettings_Version, &error));
    if (!g_settings) {
        snprintf(err, err_len, "%s: %s", vr::IVRSettings_Version, describe(error));
        g_shutdown();
        return 1;
    }
    return 0;
}

extern "C" void lfp_disconnect(void) {
    if (g_shutdown) {
        g_shutdown();
    }
    g_settings = nullptr;
    g_shutdown = nullptr;
}

extern "C" int lfp_set_float(const char* key, float value) {
    vr::EVRSettingsError e = vr::VRSettingsError_None;
    g_settings->SetFloat(SECTION, key, value, &e);
    return e;
}

extern "C" int lfp_set_bool(const char* key, int value) {
    vr::EVRSettingsError e = vr::VRSettingsError_None;
    g_settings->SetBool(SECTION, key, value != 0, &e);
    return e;
}

extern "C" int lfp_set_int(const char* key, int value) {
    vr::EVRSettingsError e = vr::VRSettingsError_None;
    g_settings->SetInt32(SECTION, key, value, &e);
    return e;
}

extern "C" int lfp_set_string(const char* key, const char* value) {
    vr::EVRSettingsError e = vr::VRSettingsError_None;
    g_settings->SetString(SECTION, key, value, &e);
    return e;
}

extern "C" int lfp_get_float(const char* key, float* value) {
    vr::EVRSettingsError e = vr::VRSettingsError_None;
    *value = g_settings->GetFloat(SECTION, key, &e);
    return e;
}

extern "C" int lfp_get_bool(const char* key, int* value) {
    vr::EVRSettingsError e = vr::VRSettingsError_None;
    *value = g_settings->GetBool(SECTION, key, &e) ? 1 : 0;
    return e;
}

extern "C" int lfp_get_int(const char* key, int* value) {
    vr::EVRSettingsError e = vr::VRSettingsError_None;
    *value = g_settings->GetInt32(SECTION, key, &e);
    return e;
}

extern "C" int lfp_get_string(const char* key, char* value, size_t len) {
    vr::EVRSettingsError e = vr::VRSettingsError_None;
    g_settings->GetString(SECTION, key, value, static_cast<uint32_t>(len), &e);
    return e;
}

extern "C" const char* lfp_error_name(int code) {
    return g_settings ? g_settings->GetSettingsErrorNameFromEnum(static_cast<vr::EVRSettingsError>(code)) : "not connected";
}
