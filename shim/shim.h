// The C contract between the C++ shim (OpenVR's driver classes) and the Rust driver.
// lf_*  : implemented in Rust, called by the shim.
// lfh_* : implemented by the shim over the vrserver host interfaces, called from Rust.
// Poses and bones cross as opaque pointers to the OpenVR structs; Rust mirrors them #[repr(C)]
// and lfh_layout() lets it check the sizes at init.
#pragma once
#include <stdint.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

#define LF_HAND_LEFT 0u
#define LF_HAND_RIGHT 1u

int32_t lf_init(void);
void lf_cleanup(void);
void lf_run_frame(void);
int32_t lf_hand_activate(uint32_t hand, uint32_t object_id);
void lf_hand_deactivate(uint32_t hand);
void lf_hand_enter_standby(uint32_t hand);
void lf_hand_get_pose(uint32_t hand, void* driver_pose_out);

void lfh_layout(uint32_t* driver_pose, uint32_t* tracked_pose, uint32_t* bone_transform);
void lfh_log(const char* msg);
bool lfh_tracked_device_added(const char* serial, int32_t device_class, uint32_t hand);
void lfh_pose_updated(uint32_t object_id, const void* driver_pose);
void lfh_get_raw_poses(float predicted_seconds, void* tracked_poses, uint32_t count);
uint64_t lfh_prop_container(uint32_t object_id);
int32_t lfh_set_prop_string(uint64_t container, int32_t prop, const char* value);
int32_t lfh_set_prop_int32(uint64_t container, int32_t prop, int32_t value);
int32_t lfh_set_prop_bool(uint64_t container, int32_t prop, bool value);
int32_t lfh_get_prop_int32(uint64_t container, int32_t prop, int32_t* value);
uint32_t lfh_get_prop_string(uint64_t container, int32_t prop, char* buf, uint32_t len, int32_t* error);
int32_t lfh_create_skeleton_component(uint64_t container, const char* name, const char* skeleton_path,
                                      const char* base_pose_path, int32_t tracking_level, uint64_t* handle);
int32_t lfh_update_skeleton(uint64_t handle, int32_t motion_range, const void* bones, uint32_t count);
bool lfh_settings_bool(const char* section, const char* key, bool fallback);
float lfh_settings_float(const char* section, const char* key, float fallback);
uint32_t lfh_settings_string(const char* section, const char* key, char* buf, uint32_t len);

#ifdef __cplusplus
}
#endif
