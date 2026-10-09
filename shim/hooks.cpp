// Passthrough hooks. The mechanism is Leapify's (InterfaceHook.cpp),
// without rcmp: the vtable slot is made writable, the original kept per vtable, the replacement
// written. Everything is passed through; Rust (lf_hook_*) may edit the pose or the bones in place
// and never sees LeapFrog's own calls.
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>

#include "hooks.h"
#include "shim.h"

#include <cstdarg>
#include <cstdio>
#include <cstring>

using namespace vr;

namespace {

thread_local int t_own_calls = 0;

void logf(const char* fmt, ...)
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
        logf("too many vtables; not hooking");
        return nullptr;
    }
    Patched& p = g_patched[g_patched_count++];
    std::memset(&p, 0, sizeof(p));
    p.vtable = *static_cast<void***>(object);
    return &p;
}

bool patch(Patched& p, size_t slot, void* replacement)
{
    void** entry = p.vtable + slot;
    DWORD old = 0;
    if (!VirtualProtect(entry, sizeof(void*), PAGE_READWRITE, &old)) {
        logf("VirtualProtect failed on slot %zu: %lu", slot, GetLastError());
        return false;
    }
    p.original[slot] = *entry;
    *entry = replacement;
    DWORD ignored = 0;
    VirtualProtect(entry, sizeof(void*), old, &ignored);
    return true;
}

// The host object each device's own driver sends its poses through: vrserver attributes a pose
// update to the calling driver, so poses LeapFrog sends for a lighthouse device through its own
// host object are dropped (2026-10-08, 04:59). Sending through the owner's object lands.
void* g_host_self[k_unMaxTrackedDeviceCount];

// IVRServerDriverHost_006, slot 1
void hook_pose_updated(void* self, uint32_t device, const DriverPose_t& pose, uint32_t size)
{
    if (device < k_unMaxTrackedDeviceCount && t_own_calls == 0) {
        g_host_self[device] = self;
    }
    Patched* p = find(self);
    auto orig = reinterpret_cast<void (*)(void*, uint32_t, const DriverPose_t&, uint32_t)>(p ? p->original[1] : nullptr);
    if (!orig) {
        return;
    }
    if (t_own_calls > 0 || size != sizeof(DriverPose_t)) {
        orig(self, device, pose, size);
        return;
    }
    DriverPose_t copy = pose;
    lf_hook_pose(device, &copy);
    orig(self, device, copy, size);
}

// IVRDriverInput_003/004, slot 5
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

// IVRDriverInput_003/004, slot 6
EVRInputError hook_update_skeleton(void* self, VRInputComponentHandle_t component, EVRSkeletalMotionRange range,
                                   const VRBoneTransform_t* bones, uint32_t count)
{
    Patched* p = find(self);
    auto orig = reinterpret_cast<EVRInputError (*)(void*, VRInputComponentHandle_t, EVRSkeletalMotionRange, const VRBoneTransform_t*, uint32_t)>(p ? p->original[6] : nullptr);
    if (!orig) {
        return VRInputError_InvalidHandle;
    }
    if (t_own_calls > 0 || bones == nullptr || count != 31) {
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
        logf("%s: TrackedDevicePoseUpdated hooked", name);
    }
}

void hook_input(void* object, const char* name)
{
    if (!object || already(object)) {
        return;
    }
    Patched* p = claim(object);
    if (p && patch(*p, 5, reinterpret_cast<void*>(&hook_create_skeleton)) && patch(*p, 6, reinterpret_cast<void*>(&hook_update_skeleton))) {
        logf("%s: CreateSkeletonComponent and UpdateSkeletonComponent hooked", name);
    }
}

// Which host interface versions have the slots above where we expect them: 006 and 003/004 are
// what this header and Leapify's hook were built against; anything else is logged and left alone.
bool known_host(const char* name) { return std::strcmp(name, "IVRServerDriverHost_006") == 0; }
bool known_input(const char* name) { return std::strcmp(name, "IVRDriverInput_003") == 0 || std::strcmp(name, "IVRDriverInput_004") == 0; }

char g_seen[64][48];
int g_seen_count = 0;

bool first_sight(const char* name)
{
    for (int i = 0; i < g_seen_count; i++) {
        if (std::strcmp(g_seen[i], name) == 0) {
            return false;
        }
    }
    if (g_seen_count < 64) {
        std::strncpy(g_seen[g_seen_count], name, sizeof(g_seen[0]) - 1);
        g_seen[g_seen_count][sizeof(g_seen[0]) - 1] = '\0';
        g_seen_count++;
    }
    return true;
}

void on_interface(void* object, const char* name)
{
    if (!name) {
        return;
    }
    bool host = std::strncmp(name, "IVRServerDriverHost_", 20) == 0;
    bool input = std::strncmp(name, "IVRDriverInput_", 15) == 0;
    if (first_sight(name) && ((host && !known_host(name)) || (input && !known_input(name)))) {
        logf("%s requested: unknown version, not hooked", name);
    }
    if (host && known_host(name)) {
        hook_host(object, name);
    } else if (input && known_input(name)) {
        hook_input(object, name);
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
    Patched* p = find(self);
    auto orig = reinterpret_cast<void (*)(void*, uint32_t, const DriverPose_t&, uint32_t)>(p ? p->original[1] : nullptr);
    if (!orig) {
        return false;
    }
    orig(self, object_id, *static_cast<const DriverPose_t*>(driver_pose), sizeof(DriverPose_t));
    return true;
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
    // them now. The context hook then catches what is requested later, and logs every version.
    const char* names[] = { "IVRServerDriverHost_006", "IVRDriverInput_003", "IVRDriverInput_004" };
    for (const char* name : names) {
        EVRInitError error = VRInitError_None;
        void* object = context->GetGenericInterface(name, &error);
        if (object && error == VRInitError_None) {
            on_interface(object, name);
        } else {
            logf("%s not offered by vrserver (%d)", name, static_cast<int>(error));
        }
    }
    Patched* p = claim(context);
    if (p && patch(*p, 0, reinterpret_cast<void*>(&hook_get_generic_interface))) {
        logf("IVRDriverContext: GetGenericInterface hooked");
    }
}

} // namespace lf_hooks
