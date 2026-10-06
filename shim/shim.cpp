// The only C++ in LeapFrog: OpenVR's driver classes, forwarding everything to Rust (lf_*), and
// C wrappers over the vrserver host interfaces for Rust to call (lfh_*). No logic lives here.
#include "openvr_driver.h"
#include "shim.h"

#include <cstring>

using namespace vr;

namespace {

class Hand final : public ITrackedDeviceServerDriver
{
public:
    explicit Hand(uint32_t index) : m_index(index) {}

    EVRInitError Activate(uint32_t unObjectId) override
    {
        return static_cast<EVRInitError>(lf_hand_activate(m_index, unObjectId));
    }

    void Deactivate() override { lf_hand_deactivate(m_index); }
    void EnterStandby() override { lf_hand_enter_standby(m_index); }
    void* GetComponent(const char*) override { return nullptr; }
    void DebugRequest(const char*, char* pchResponseBuffer, uint32_t unResponseBufferSize) override
    {
        if (unResponseBufferSize > 0) {
            pchResponseBuffer[0] = '\0';
        }
    }

    DriverPose_t GetPose() override
    {
        DriverPose_t pose;
        std::memset(&pose, 0, sizeof(pose));
        lf_hand_get_pose(m_index, &pose);
        return pose;
    }

private:
    uint32_t m_index;
};

Hand g_hands[2] = { Hand(LF_HAND_LEFT), Hand(LF_HAND_RIGHT) };

class Provider final : public IServerTrackedDeviceProvider
{
public:
    EVRInitError Init(IVRDriverContext* pDriverContext) override
    {
        VR_INIT_SERVER_DRIVER_CONTEXT(pDriverContext);
        return static_cast<EVRInitError>(lf_init());
    }

    void Cleanup() override
    {
        lf_cleanup();
        VR_CLEANUP_SERVER_DRIVER_CONTEXT();
    }

    const char* const* GetInterfaceVersions() override { return k_InterfaceVersions; }
    void RunFrame() override { lf_run_frame(); }
    bool ShouldBlockStandbyMode() override { return false; }
    void EnterStandby() override {}
    void LeaveStandby() override {}
};

Provider g_provider;

} // namespace

extern "C" __declspec(dllexport) void* HmdDriverFactory(const char* pInterfaceName, int* pReturnCode)
{
    if (std::strcmp(IServerTrackedDeviceProvider_Version, pInterfaceName) == 0) {
        return &g_provider;
    }
    if (pReturnCode) {
        *pReturnCode = VRInitError_Init_InterfaceNotFound;
    }
    return nullptr;
}

// Host side

extern "C" void lfh_layout(uint32_t* driver_pose, uint32_t* tracked_pose, uint32_t* bone_transform)
{
    *driver_pose = sizeof(DriverPose_t);
    *tracked_pose = sizeof(TrackedDevicePose_t);
    *bone_transform = sizeof(VRBoneTransform_t);
}

extern "C" void lfh_log(const char* msg)
{
    if (VRDriverLog()) {
        VRDriverLog()->Log(msg);
    }
}

extern "C" bool lfh_tracked_device_added(const char* serial, int32_t device_class, uint32_t hand)
{
    if (hand > LF_HAND_RIGHT) {
        return false;
    }
    return VRServerDriverHost()->TrackedDeviceAdded(serial, static_cast<ETrackedDeviceClass>(device_class), &g_hands[hand]);
}

extern "C" void lfh_pose_updated(uint32_t object_id, const void* driver_pose)
{
    VRServerDriverHost()->TrackedDevicePoseUpdated(object_id, *static_cast<const DriverPose_t*>(driver_pose), sizeof(DriverPose_t));
}

extern "C" void lfh_get_raw_poses(float predicted_seconds, void* tracked_poses, uint32_t count)
{
    VRServerDriverHost()->GetRawTrackedDevicePoses(predicted_seconds, static_cast<TrackedDevicePose_t*>(tracked_poses), count);
}

extern "C" uint64_t lfh_prop_container(uint32_t object_id)
{
    return VRProperties()->TrackedDeviceToPropertyContainer(object_id);
}

extern "C" int32_t lfh_set_prop_string(uint64_t container, int32_t prop, const char* value)
{
    return VRProperties()->SetStringProperty(container, static_cast<ETrackedDeviceProperty>(prop), value);
}

extern "C" int32_t lfh_set_prop_int32(uint64_t container, int32_t prop, int32_t value)
{
    return VRProperties()->SetInt32Property(container, static_cast<ETrackedDeviceProperty>(prop), value);
}

extern "C" int32_t lfh_set_prop_bool(uint64_t container, int32_t prop, bool value)
{
    return VRProperties()->SetBoolProperty(container, static_cast<ETrackedDeviceProperty>(prop), value);
}

extern "C" int32_t lfh_get_prop_int32(uint64_t container, int32_t prop, int32_t* value)
{
    ETrackedPropertyError error = TrackedProp_Success;
    *value = VRProperties()->GetInt32Property(container, static_cast<ETrackedDeviceProperty>(prop), &error);
    return error;
}

extern "C" uint32_t lfh_get_prop_string(uint64_t container, int32_t prop, char* buf, uint32_t len, int32_t* error)
{
    ETrackedPropertyError err = TrackedProp_Success;
    uint32_t n = VRProperties()->GetStringProperty(container, static_cast<ETrackedDeviceProperty>(prop), buf, len, &err);
    *error = err;
    return n;
}

extern "C" int32_t lfh_create_skeleton_component(uint64_t container, const char* name, const char* skeleton_path,
                                                 const char* base_pose_path, int32_t tracking_level, uint64_t* handle)
{
    return VRDriverInput()->CreateSkeletonComponent(container, name, skeleton_path, base_pose_path,
        static_cast<EVRSkeletalTrackingLevel>(tracking_level), nullptr, 0, handle);
}

extern "C" int32_t lfh_update_skeleton(uint64_t handle, int32_t motion_range, const void* bones, uint32_t count)
{
    return VRDriverInput()->UpdateSkeletonComponent(handle, static_cast<EVRSkeletalMotionRange>(motion_range),
        static_cast<const VRBoneTransform_t*>(bones), count);
}

extern "C" bool lfh_settings_bool(const char* section, const char* key, bool fallback)
{
    EVRSettingsError error = VRSettingsError_None;
    bool value = VRSettings()->GetBool(section, key, &error);
    return error == VRSettingsError_None ? value : fallback;
}

extern "C" float lfh_settings_float(const char* section, const char* key, float fallback)
{
    EVRSettingsError error = VRSettingsError_None;
    float value = VRSettings()->GetFloat(section, key, &error);
    return error == VRSettingsError_None ? value : fallback;
}

// Returns the length written (without the terminator), 0 when unset or too long.
extern "C" uint32_t lfh_settings_string(const char* section, const char* key, char* buf, uint32_t len)
{
    EVRSettingsError error = VRSettingsError_None;
    VRSettings()->GetString(section, key, buf, len, &error);
    if (error != VRSettingsError_None || len == 0) {
        return 0;
    }
    buf[len - 1] = '\0';
    return static_cast<uint32_t>(std::strlen(buf));
}
