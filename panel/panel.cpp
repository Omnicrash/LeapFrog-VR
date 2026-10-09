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
void* (*g_iface)(const char*, vr::EVRInitError*);
const char* (*g_describe)(vr::EVRInitError);

template <typename T>
T proc(const char* name) {
    return reinterpret_cast<T>(GetProcAddress(g_api, name));
}

}  // namespace

extern "C" int lfp_connect(const wchar_t* dll, int overlay, char* err, size_t err_len) {
    if (!g_api) g_api = LoadLibraryW(dll);
    if (!g_api) {
        snprintf(err, err_len, "openvr_api.dll not loaded (error %lu)", GetLastError());
        return 1;
    }
    auto init = proc<uint32_t (*)(vr::EVRInitError*, vr::EVRApplicationType, const char*)>("VR_InitInternal2");
    g_iface = proc<void* (*)(const char*, vr::EVRInitError*)>("VR_GetGenericInterface");
    g_describe = proc<const char* (*)(vr::EVRInitError)>("VR_GetVRInitErrorAsEnglishDescription");
    g_shutdown = proc<void (*)()>("VR_ShutdownInternal");
    if (!init || !g_iface || !g_describe || !g_shutdown) {
        snprintf(err, err_len, "openvr_api.dll exports missing");
        return 1;
    }
    vr::EVRInitError error = vr::VRInitError_None;
    init(&error, overlay ? vr::VRApplication_Overlay : vr::VRApplication_Background, nullptr);
    if (error != vr::VRInitError_None) {
        snprintf(err, err_len, "%s", g_describe(error));
        return 1;
    }
    g_settings = static_cast<vr::IVRSettings*>(lfp_interface(vr::IVRSettings_Version, err, err_len));
    if (!g_settings) {
        g_shutdown();
        return 1;
    }
    return 0;
}

extern "C" void* lfp_interface(const char* version, char* err, size_t err_len) {
    vr::EVRInitError error = vr::VRInitError_None;
    void* p = g_iface(version, &error);
    if (!p) {
        snprintf(err, err_len, "%s: %s", version, g_describe(error));
    }
    return p;
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

// A whole number written by a tool is stored as an int and GetFloat refuses it, so this falls
// back to GetInt32: an int-stored key reads as a float.
extern "C" int lfp_get_float(const char* key, float* value) {
    vr::EVRSettingsError e = vr::VRSettingsError_None;
    *value = g_settings->GetFloat(SECTION, key, &e);
    if (e == vr::VRSettingsError_None) {
        return e;
    }
    e = vr::VRSettingsError_None;
    *value = static_cast<float>(g_settings->GetInt32(SECTION, key, &e));
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

// The driver's vrserver, which spawned this process: the panel ends with it even when no quit
// event arrives (a crash).
extern "C" void* lfp_process_open(unsigned pid) {
    return OpenProcess(SYNCHRONIZE, FALSE, pid);
}

// A failed wait counts as ended too, so a bad handle cannot turn the caller's loop into a spin.
extern "C" int lfp_process_wait(void* process, unsigned ms) {
    return WaitForSingleObject(process, ms) == WAIT_TIMEOUT ? 0 : 1;
}

extern "C" void lfp_process_close(void* process) {
    if (process) {
        CloseHandle(process);
    }
}

// The panel as a registered overlay app: SteamVR's "Choose Startup Overlay Apps" list and its
// own launch at start, instead of the driver's.
extern "C" int lfp_app_installed(const char* key) {
    char err[128];
    auto apps = static_cast<vr::IVRApplications*>(lfp_interface(vr::IVRApplications_Version, err, sizeof err));
    return apps && apps->IsApplicationInstalled(key) ? 1 : 0;
}

extern "C" int lfp_app_register(const char* manifest, const char* key, char* err, size_t err_len) {
    auto apps = static_cast<vr::IVRApplications*>(lfp_interface(vr::IVRApplications_Version, err, err_len));
    if (!apps) {
        return 1;
    }
    vr::EVRApplicationError e = apps->AddApplicationManifest(manifest, false);
    if (e == vr::VRApplicationError_None) {
        e = apps->SetApplicationAutoLaunch(key, true);
    }
    if (e != vr::VRApplicationError_None) {
        snprintf(err, err_len, "%s", apps->GetApplicationsErrorNameFromEnum(e));
        return 1;
    }
    return 0;
}

extern "C" int lfp_app_unregister(const char* manifest, char* err, size_t err_len) {
    auto apps = static_cast<vr::IVRApplications*>(lfp_interface(vr::IVRApplications_Version, err, err_len));
    if (!apps) {
        return 1;
    }
    vr::EVRApplicationError e = apps->RemoveApplicationManifest(manifest);
    if (e != vr::VRApplicationError_None) {
        snprintf(err, err_len, "%s", apps->GetApplicationsErrorNameFromEnum(e));
        return 1;
    }
    return 0;
}

// Asks SteamVR to launch the registered app (0 also when it already runs).
extern "C" int lfp_app_launch(const char* key, char* err, size_t err_len) {
    auto apps = static_cast<vr::IVRApplications*>(lfp_interface(vr::IVRApplications_Version, err, err_len));
    if (!apps) {
        return 1;
    }
    vr::EVRApplicationError e = apps->LaunchDashboardOverlay(key);
    if (e != vr::VRApplicationError_None && e != vr::VRApplicationError_ApplicationAlreadyRunning) {
        snprintf(err, err_len, "%s", apps->GetApplicationsErrorNameFromEnum(e));
        return 1;
    }
    return 0;
}
