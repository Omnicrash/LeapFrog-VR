// Passthrough hooks. The mechanism is Leapify's (InterfaceHook.cpp), without rcmp: the vtable
// slot is made writable, the original kept per vtable, the replacement written. Only the two
// watched controllers (and their skeletons) go through Rust; every other call, and LeapFrog's
// own, passes straight to the original.
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>

#include "hooks.h"
#include "shim.h"

#include <atomic>
#include <cstdarg>
#include <cstdio>
#include <cstring>
#include <cwchar>

using namespace vr;

namespace {

const uint32_t NONE = UINT32_MAX;
const uint64_t NO_HANDLE = 0;

thread_local int t_own_calls = 0;

// The devices and skeleton handles Rust wants to see, per hand; anything else is passed through
// without a copy, a call into Rust or a lock.
std::atomic<uint32_t> g_watched_device[2] = { NONE, NONE };
std::atomic<uint64_t> g_watched_skeleton[2] = { NO_HANDLE, NO_HANDLE };

bool watched_device(uint32_t device)
{
    return device == g_watched_device[0].load(std::memory_order_relaxed) || device == g_watched_device[1].load(std::memory_order_relaxed);
}

bool watched_skeleton(uint64_t handle)
{
    return handle != NO_HANDLE
        && (handle == g_watched_skeleton[0].load(std::memory_order_relaxed) || handle == g_watched_skeleton[1].load(std::memory_order_relaxed));
}

void log_line(const char* fmt, ...)
{
    char buf[320];
    std::strcpy(buf, "[LeapFrog] hook: ");
    va_list args;
    va_start(args, fmt);
    std::vsnprintf(buf + std::strlen(buf), sizeof(buf) - std::strlen(buf), fmt, args);
    va_end(args);
    lfh_log(buf);
}

// One patched vtable: its address and the originals of the slots we replaced.
struct Patched
{
    void** vtable;
    void* original[8];
};

Patched g_patched[8];
int g_patched_count = 0;

Patched* find(void* self)
{
    void** vtable = *static_cast<void***>(self);
    for (int i = 0; i < g_patched_count; i++) {
        if (g_patched[i].vtable == vtable) {
            return &g_patched[i];
        }
    }
    return nullptr;
}

bool already(void* object)
{
    return object && find(object) != nullptr;
}

Patched* claim(void* object)
{
    if (g_patched_count >= 8) {
        log_line("too many vtables; not hooking");
        return nullptr;
    }
    Patched& p = g_patched[g_patched_count++];
    std::memset(&p, 0, sizeof(p));
    p.vtable = *static_cast<void***>(object);
    return &p;
}

bool write_slot(void** entry, void* value)
{
    DWORD old = 0;
    if (!VirtualProtect(entry, sizeof(void*), PAGE_READWRITE, &old)) {
        return false;
    }
    *entry = value;
    DWORD ignored = 0;
    VirtualProtect(entry, sizeof(void*), old, &ignored);
    return true;
}

bool patch(Patched& p, size_t slot, void* replacement)
{
    void** entry = p.vtable + slot;
    void* original = *entry;
    if (!write_slot(entry, replacement)) {
        log_line("VirtualProtect failed on slot %zu: %lu", slot, GetLastError());
        return false;
    }
    p.original[slot] = original;
    return true;
}

using PoseUpdated = void (*)(void*, uint32_t, const DriverPose_t&, uint32_t);

PoseUpdated original_pose_updated(void* self)
{
    Patched* p = find(self);
    return reinterpret_cast<PoseUpdated>(p ? p->original[1] : nullptr);
}

// The host object each device's own driver sends its poses through: vrserver attributes a pose
// update to the calling driver, so poses LeapFrog sends for a lighthouse device through its own
// host object are dropped. Sending through the owner's object lands.
void* g_host_self[k_unMaxTrackedDeviceCount];

// IVRServerDriverHost_006, slot 1
void hook_pose_updated(void* self, uint32_t device, const DriverPose_t& pose, uint32_t size)
{
    PoseUpdated orig = original_pose_updated(self);
    if (!orig) {
        return;
    }
    if (t_own_calls > 0 || size != sizeof(DriverPose_t) || !watched_device(device)) {
        orig(self, device, pose, size);
        return;
    }
    if (device < k_unMaxTrackedDeviceCount) {
        g_host_self[device] = self;
    }
    DriverPose_t copy = pose;
    lf_hook_pose(device, &copy);
    orig(self, device, copy, size);
}

// IVRDriverInput_003/004/005, slot 5
EVRInputError hook_create_skeleton(void* self, PropertyContainerHandle_t container, const char* name, const char* skeleton_path,
                                   const char* base_pose_path, EVRSkeletalTrackingLevel level, const VRBoneTransform_t* grip_limits,
                                   uint32_t grip_limit_count, VRInputComponentHandle_t* handle)
{
    Patched* p = find(self);
    auto orig = reinterpret_cast<EVRInputError (*)(void*, PropertyContainerHandle_t, const char*, const char*, const char*,
        EVRSkeletalTrackingLevel, const VRBoneTransform_t*, uint32_t, VRInputComponentHandle_t*)>(p ? p->original[5] : nullptr);
    if (!orig) {
        return VRInputError_InvalidHandle;
    }
    if (t_own_calls > 0) {
        return orig(self, container, name, skeleton_path, base_pose_path, level, grip_limits, grip_limit_count, handle);
    }
    EVRSkeletalTrackingLevel wanted = static_cast<EVRSkeletalTrackingLevel>(lf_hook_skeleton_create(container, name, skeleton_path, level));
    EVRInputError result = orig(self, container, name, skeleton_path, base_pose_path, wanted, grip_limits, grip_limit_count, handle);
    if (result == VRInputError_None && handle) {
        lf_hook_skeleton_created(container, *handle);
    }
    return result;
}

// IVRDriverInput_003/004/005, slot 6
EVRInputError hook_update_skeleton(void* self, VRInputComponentHandle_t component, EVRSkeletalMotionRange range,
                                   const VRBoneTransform_t* bones, uint32_t count)
{
    Patched* p = find(self);
    auto orig = reinterpret_cast<EVRInputError (*)(void*, VRInputComponentHandle_t, EVRSkeletalMotionRange, const VRBoneTransform_t*, uint32_t)>(p ? p->original[6] : nullptr);
    if (!orig) {
        return VRInputError_InvalidHandle;
    }
    if (t_own_calls > 0 || bones == nullptr || count != 31 || !watched_skeleton(component)) {
        return orig(self, component, range, bones, count);
    }
    VRBoneTransform_t copy[31];
    std::memcpy(copy, bones, sizeof(copy));
    lf_hook_skeleton_update(component, static_cast<int32_t>(range), copy, count);
    return orig(self, component, range, copy, count);
}

void hook_host(void* object, const char* name)
{
    if (!object || already(object)) {
        return;
    }
    Patched* p = claim(object);
    if (p && patch(*p, 1, reinterpret_cast<void*>(&hook_pose_updated))) {
        log_line("%s: TrackedDevicePoseUpdated hooked", name);
    }
}

// Which interface versions have the slots above where we expect them. Host 006 and input 003 are
// the public headers; Valve's master header defines 004 as 003 plus appended methods; 005 (the
// SteamVR 2.18 beta's lighthouse, no public header) was read from vrserver 2.18.2: the same 11
// slots as 004, each a wrapper that forwards the 004 signature with a driver id in front. Any
// other version is logged with its skeleton slots and left alone.
bool known_host(const char* name) { return std::strcmp(name, "IVRServerDriverHost_006") == 0; }
bool known_input(const char* name)
{
    return std::strcmp(name, "IVRDriverInput_003") == 0 || std::strcmp(name, "IVRDriverInput_004") == 0 || std::strcmp(name, "IVRDriverInput_005") == 0;
}

// "module+offset" of a vtable entry, for the log.
void describe(void* fn, char* out, size_t size)
{
    HMODULE module = nullptr;
    wchar_t path[MAX_PATH] = L"";
    if (GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, static_cast<LPCWSTR>(fn), &module)
        && GetModuleFileNameW(module, path, MAX_PATH)) {
        const wchar_t* base = wcsrchr(path, L'\\');
        base = base ? base + 1 : path;
        std::snprintf(out, size, "%ls+0x%llx", base, static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(fn) - reinterpret_cast<uintptr_t>(module)));
    } else {
        std::snprintf(out, size, "%p", fn);
    }
}

void hook_input(void* object, const char* name)
{
    if (!object || already(object)) {
        return;
    }
    if (!known_input(name)) {
        void** vtable = *static_cast<void***>(object);
        char create[96], update[96];
        describe(vtable[5], create, sizeof(create));
        describe(vtable[6], update, sizeof(update));
        log_line("%s requested: unknown version, not hooked (slot 5 %s, slot 6 %s)", name, create, update);
        return;
    }
    Patched* p = claim(object);
    if (p && patch(*p, 5, reinterpret_cast<void*>(&hook_create_skeleton)) && patch(*p, 6, reinterpret_cast<void*>(&hook_update_skeleton))) {
        log_line("%s: CreateSkeletonComponent and UpdateSkeletonComponent hooked", name);
    }
}

void on_interface(void* object, const char* name)
{
    if (!name) {
        return;
    }
    bool host = std::strncmp(name, "IVRServerDriverHost_", 20) == 0;
    bool input = std::strncmp(name, "IVRDriverInput_", 15) == 0;
    if (host && known_host(name)) {
        hook_host(object, name);
    } else if (input) {
        hook_input(object, name);
    } else if (host) {
        log_line("%s requested: unknown version, not hooked", name);
    }
}

// IVRDriverContext, slot 0
void* hook_get_generic_interface(void* self, const char* name, EVRInitError* error)
{
    Patched* p = find(self);
    auto orig = reinterpret_cast<void* (*)(void*, const char*, EVRInitError*)>(p ? p->original[0] : nullptr);
    if (!orig) {
        if (error) {
            *error = VRInitError_Init_InterfaceNotFound;
        }
        return nullptr;
    }
    void* object = orig(self, name, error);
    on_interface(object, name);
    return object;
}

} // namespace

extern "C" bool lfh_pose_updated_as(uint32_t object_id, const void* driver_pose)
{
    if (object_id >= k_unMaxTrackedDeviceCount || !g_host_self[object_id]) {
        return false;
    }
    void* self = g_host_self[object_id];
    PoseUpdated orig = original_pose_updated(self);
    if (!orig) {
        return false;
    }
    orig(self, object_id, *static_cast<const DriverPose_t*>(driver_pose), sizeof(DriverPose_t));
    return true;
}

extern "C" void lfh_hooks_watch(uint32_t hand, uint32_t object_id, uint64_t skeleton)
{
    if (hand < 2) {
        g_watched_device[hand].store(object_id, std::memory_order_relaxed);
        g_watched_skeleton[hand].store(skeleton, std::memory_order_relaxed);
    }
}

namespace lf_hooks {

OwnCall::OwnCall() { t_own_calls++; }
OwnCall::~OwnCall() { t_own_calls--; }

void install(IVRDriverContext* context)
{
    if (!context || already(context)) {
        return;
    }
    // Drivers loaded before us already hold their interfaces: ask for the same objects and patch
    // them now. The context hook then catches what is requested later.
    const char* names[] = { "IVRServerDriverHost_006", "IVRDriverInput_003", "IVRDriverInput_004", "IVRDriverInput_005" };
    for (const char* name : names) {
        EVRInitError error = VRInitError_None;
        void* object = context->GetGenericInterface(name, &error);
        if (object && error == VRInitError_None) {
            on_interface(object, name);
        } else {
            log_line("%s not offered by vrserver (%d)", name, static_cast<int>(error));
        }
    }
    Patched* p = claim(context);
    if (p && patch(*p, 0, reinterpret_cast<void*>(&hook_get_generic_interface))) {
        log_line("IVRDriverContext: GetGenericInterface hooked");
    }
}

// Puts every original slot back, so nothing points into this DLL once it is unloaded.
void uninstall()
{
    lfh_hooks_watch(0, NONE, NO_HANDLE);
    lfh_hooks_watch(1, NONE, NO_HANDLE);
    for (int i = 0; i < g_patched_count; i++) {
        Patched& p = g_patched[i];
        for (size_t slot = 0; slot < 8; slot++) {
            if (p.original[slot] && !write_slot(p.vtable + slot, p.original[slot])) {
                log_line("VirtualProtect failed restoring slot %zu", slot);
            }
        }
    }
    g_patched_count = 0;
}

} // namespace lf_hooks
