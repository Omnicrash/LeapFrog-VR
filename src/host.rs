//! The vrserver side of the shim contract: `#[repr(C)]` mirrors of the OpenVR structs we pass
//! through, the raw `lfh_*` imports, and safe wrappers over them.

#![allow(dead_code)] // wrappers for the coming iterations (poses, bones, other devices)

use std::ffi::CString;

pub const HAND_LEFT: u32 = 0;
pub const HAND_RIGHT: u32 = 1;

// OpenVR enum values (openvr_driver.h); only the ones LeapFrog uses.
pub const INIT_ERROR_NONE: i32 = 0;
pub const INIT_ERROR_DRIVER_FAILED: i32 = 200;
pub const DEVICE_CLASS_CONTROLLER: i32 = 2;
pub const ROLE_LEFT_HAND: i32 = 1;
pub const ROLE_RIGHT_HAND: i32 = 2;
pub const TRACKING_RESULT_UNINITIALIZED: i32 = 1;
pub const TRACKING_RESULT_RUNNING_OK: i32 = 200;
pub const TRACKING_RESULT_OUT_OF_RANGE: i32 = 201;
pub const SKELETAL_TRACKING_PARTIAL: i32 = 1;
pub const SKELETAL_TRACKING_FULL: i32 = 2;
pub const SKELETAL_MOTION_RANGE_WITH_CONTROLLER: i32 = 0;
pub const SKELETAL_MOTION_RANGE_WITHOUT_CONTROLLER: i32 = 1;
pub const MAX_TRACKED_DEVICE_COUNT: usize = 64;

pub mod prop {
    pub const TRACKING_SYSTEM_NAME: i32 = 1000;
    pub const MODEL_NUMBER: i32 = 1001;
    pub const SERIAL_NUMBER: i32 = 1002;
    pub const RENDER_MODEL_NAME: i32 = 1003;
    pub const WILL_DRIFT_IN_YAW: i32 = 1004;
    pub const MANUFACTURER_NAME: i32 = 1005;
    pub const DEVICE_IS_WIRELESS: i32 = 1010;
    pub const DEVICE_PROVIDES_BATTERY_STATUS: i32 = 1026;
    pub const DEVICE_CLASS: i32 = 1029;
    pub const RESOURCE_ROOT: i32 = 1035;
    pub const INPUT_PROFILE_PATH: i32 = 1037;
    pub const IDENTIFIABLE: i32 = 1043;
    pub const CONTROLLER_ROLE_HINT: i32 = 3007;
    pub const ICON_OFF: i32 = 5001;
    pub const ICON_SEARCHING: i32 = 5002;
    pub const ICON_SEARCHING_ALERT: i32 = 5003;
    pub const ICON_READY: i32 = 5004;
    pub const ICON_READY_ALERT: i32 = 5005;
    pub const ICON_NOT_READY: i32 = 5006;
    pub const ICON_STANDBY: i32 = 5007;
    pub const ICON_ALERT_LOW: i32 = 5008;
    pub const ICON_STANDBY_ALERT: i32 = 5009;
    pub const CONTROLLER_TYPE: i32 = 7000;
    pub const CONTROLLER_HAND_SELECTION_PRIORITY: i32 = 7002;
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Quaternion {
    pub w: f64,
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Quaternion {
    pub const IDENTITY: Quaternion = Quaternion { w: 1.0, x: 0.0, y: 0.0, z: 0.0 };
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Quaternionf {
    pub w: f32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct BoneTransform {
    pub position: [f32; 4],
    pub orientation: Quaternionf,
}

/// vr::DriverPose_t
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct DriverPose {
    pub pose_time_offset: f64,
    pub world_from_driver_rotation: Quaternion,
    pub world_from_driver_translation: [f64; 3],
    pub driver_from_head_rotation: Quaternion,
    pub driver_from_head_translation: [f64; 3],
    pub position: [f64; 3],
    pub velocity: [f64; 3],
    pub acceleration: [f64; 3],
    pub rotation: Quaternion,
    pub angular_velocity: [f64; 3],
    pub angular_acceleration: [f64; 3],
    pub result: i32,
    pub pose_is_valid: bool,
    pub will_drift_in_yaw: bool,
    pub should_apply_head_model: bool,
    pub device_is_connected: bool,
}

impl DriverPose {
    /// A device that is registered but not present: what SteamVR shows as "off".
    pub fn disconnected() -> DriverPose {
        DriverPose {
            pose_time_offset: 0.0,
            world_from_driver_rotation: Quaternion::IDENTITY,
            world_from_driver_translation: [0.0; 3],
            driver_from_head_rotation: Quaternion::IDENTITY,
            driver_from_head_translation: [0.0; 3],
            position: [0.0; 3],
            velocity: [0.0; 3],
            acceleration: [0.0; 3],
            rotation: Quaternion::IDENTITY,
            angular_velocity: [0.0; 3],
            angular_acceleration: [0.0; 3],
            result: TRACKING_RESULT_UNINITIALIZED,
            pose_is_valid: false,
            will_drift_in_yaw: false,
            should_apply_head_model: false,
            device_is_connected: false,
        }
    }
}

/// vr::TrackedDevicePose_t, as GetRawTrackedDevicePoses fills it.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TrackedDevicePose {
    pub device_to_absolute_tracking: [[f32; 4]; 3],
    pub velocity: [f32; 3],
    pub angular_velocity: [f32; 3],
    pub tracking_result: i32,
    pub pose_is_valid: bool,
    pub device_is_connected: bool,
}

impl Default for TrackedDevicePose {
    fn default() -> Self {
        TrackedDevicePose {
            device_to_absolute_tracking: [[0.0; 4]; 3],
            velocity: [0.0; 3],
            angular_velocity: [0.0; 3],
            tracking_result: TRACKING_RESULT_UNINITIALIZED,
            pose_is_valid: false,
            device_is_connected: false,
        }
    }
}

#[cfg(not(feature = "host-stubs"))]
pub mod ffi {
    use std::ffi::{c_char, c_void};

    extern "C" {
        pub fn lfh_layout(driver_pose: *mut u32, tracked_pose: *mut u32, bone_transform: *mut u32);
        pub fn lfh_log(msg: *const c_char);
        pub fn lfh_tracked_device_added(serial: *const c_char, device_class: i32, hand: u32) -> bool;
        pub fn lfh_pose_updated(object_id: u32, driver_pose: *const c_void);
        pub fn lfh_pose_updated_as(object_id: u32, driver_pose: *const c_void) -> bool;
        pub fn lfh_get_raw_poses(predicted_seconds: f32, tracked_poses: *mut c_void, count: u32);
        pub fn lfh_prop_container(object_id: u32) -> u64;
        pub fn lfh_set_prop_string(container: u64, prop: i32, value: *const c_char) -> i32;
        pub fn lfh_set_prop_int32(container: u64, prop: i32, value: i32) -> i32;
        pub fn lfh_set_prop_bool(container: u64, prop: i32, value: bool) -> i32;
        pub fn lfh_get_prop_int32(container: u64, prop: i32, value: *mut i32) -> i32;
        pub fn lfh_get_prop_string(container: u64, prop: i32, buf: *mut c_char, len: u32, error: *mut i32) -> u32;
        pub fn lfh_create_skeleton_component(container: u64, name: *const c_char, skeleton_path: *const c_char,
            base_pose_path: *const c_char, tracking_level: i32, handle: *mut u64) -> i32;
        pub fn lfh_update_skeleton(handle: u64, motion_range: i32, bones: *const c_void, count: u32) -> i32;
        pub fn lfh_settings_bool(section: *const c_char, key: *const c_char, fallback: bool) -> bool;
        pub fn lfh_settings_float(section: *const c_char, key: *const c_char, fallback: f32) -> f32;
        pub fn lfh_settings_string(section: *const c_char, key: *const c_char, buf: *mut c_char, len: u32) -> u32;
    }
}

/// Stand-ins for `cargo test` on the build host, where there is no vrserver and no shim.
#[cfg(feature = "host-stubs")]
#[allow(clippy::missing_safety_doc)]
pub mod ffi {
    use std::ffi::{c_char, c_void};

    pub unsafe fn lfh_layout(driver_pose: *mut u32, tracked_pose: *mut u32, bone_transform: *mut u32) {
        *driver_pose = std::mem::size_of::<super::DriverPose>() as u32;
        *tracked_pose = std::mem::size_of::<super::TrackedDevicePose>() as u32;
        *bone_transform = std::mem::size_of::<super::BoneTransform>() as u32;
    }
    pub unsafe fn lfh_log(msg: *const c_char) {
        eprintln!("{}", std::ffi::CStr::from_ptr(msg).to_string_lossy());
    }
    pub unsafe fn lfh_tracked_device_added(_: *const c_char, _: i32, _: u32) -> bool { true }
    pub unsafe fn lfh_pose_updated(_: u32, _: *const c_void) {}
    pub unsafe fn lfh_pose_updated_as(_: u32, _: *const c_void) -> bool { false }
    pub unsafe fn lfh_get_raw_poses(_: f32, _: *mut c_void, _: u32) {}
    pub unsafe fn lfh_prop_container(_: u32) -> u64 { 0 }
    pub unsafe fn lfh_set_prop_string(_: u64, _: i32, _: *const c_char) -> i32 { 0 }
    pub unsafe fn lfh_set_prop_int32(_: u64, _: i32, _: i32) -> i32 { 0 }
    pub unsafe fn lfh_set_prop_bool(_: u64, _: i32, _: bool) -> i32 { 0 }
    pub unsafe fn lfh_get_prop_int32(_: u64, _: i32, _: *mut i32) -> i32 { 1 }
    pub unsafe fn lfh_get_prop_string(_: u64, _: i32, _: *mut c_char, _: u32, _: *mut i32) -> u32 { 0 }
    pub unsafe fn lfh_create_skeleton_component(_: u64, _: *const c_char, _: *const c_char, _: *const c_char, _: i32, _: *mut u64) -> i32 { 0 }
    pub unsafe fn lfh_update_skeleton(_: u64, _: i32, _: *const c_void, _: u32) -> i32 { 0 }
    pub unsafe fn lfh_settings_bool(_: *const c_char, _: *const c_char, fallback: bool) -> bool { fallback }
    pub unsafe fn lfh_settings_float(_: *const c_char, _: *const c_char, fallback: f32) -> f32 { fallback }
    pub unsafe fn lfh_settings_string(_: *const c_char, _: *const c_char, _: *mut c_char, _: u32) -> u32 { 0 }
}

/// Compares the shim's struct sizes with ours; a mismatch means the header and the mirrors
/// drifted apart and nothing may be passed through.
pub fn layout_matches() -> Result<(), String> {
    let (mut dp, mut tp, mut bt) = (0u32, 0u32, 0u32);
    // SAFETY: three out-pointers to locals.
    unsafe { ffi::lfh_layout(&mut dp, &mut tp, &mut bt) };
    let ours = (
        std::mem::size_of::<DriverPose>() as u32,
        std::mem::size_of::<TrackedDevicePose>() as u32,
        std::mem::size_of::<BoneTransform>() as u32,
    );
    if (dp, tp, bt) == ours {
        Ok(())
    } else {
        Err(format!("struct sizes differ, shim {:?} vs rust {:?}", (dp, tp, bt), ours))
    }
}

fn cstr(s: &str) -> CString {
    CString::new(s).unwrap_or_default()
}

pub fn tracked_device_added(serial: &str, device_class: i32, hand: u32) -> bool {
    let serial = cstr(serial);
    // SAFETY: the shim keeps its own device objects; the string outlives the call.
    unsafe { ffi::lfh_tracked_device_added(serial.as_ptr(), device_class, hand) }
}

pub fn pose_updated(object_id: u32, pose: &DriverPose) {
    // SAFETY: DriverPose mirrors vr::DriverPose_t (checked by layout_matches at init).
    unsafe { ffi::lfh_pose_updated(object_id, pose as *const DriverPose as *const _) };
}

/// A pose for another driver's device, through that driver's own host object; false when the
/// hooks have not seen that device's owner yet.
pub fn pose_updated_as(object_id: u32, pose: &DriverPose) -> bool {
    // SAFETY: the pose outlives the call; the shim copies it.
    unsafe { ffi::lfh_pose_updated_as(object_id, pose as *const DriverPose as *const _) }
}

pub fn raw_poses(predicted_seconds: f32, poses: &mut [TrackedDevicePose]) {
    // SAFETY: the slice is exactly `count` mirrors of vr::TrackedDevicePose_t.
    unsafe { ffi::lfh_get_raw_poses(predicted_seconds, poses.as_mut_ptr() as *mut _, poses.len() as u32) };
}

pub struct Properties(u64);

impl Properties {
    pub fn of(object_id: u32) -> Properties {
        // SAFETY: plain call into the host.
        Properties(unsafe { ffi::lfh_prop_container(object_id) })
    }

    /// The raw property container handle (what the input hooks see).
    pub fn container(&self) -> u64 {
        self.0
    }

    pub fn set_string(&self, prop: i32, value: &str) -> &Self {
        let value = cstr(value);
        // SAFETY: the host copies the string.
        unsafe { ffi::lfh_set_prop_string(self.0, prop, value.as_ptr()) };
        self
    }

    pub fn set_int32(&self, prop: i32, value: i32) -> &Self {
        // SAFETY: plain call into the host.
        unsafe { ffi::lfh_set_prop_int32(self.0, prop, value) };
        self
    }

    pub fn set_bool(&self, prop: i32, value: bool) -> &Self {
        // SAFETY: plain call into the host.
        unsafe { ffi::lfh_set_prop_bool(self.0, prop, value) };
        self
    }

    pub fn int32(&self, prop: i32) -> Option<i32> {
        let mut value = 0i32;
        // SAFETY: out-pointer to a local.
        let error = unsafe { ffi::lfh_get_prop_int32(self.0, prop, &mut value) };
        (error == 0).then_some(value)
    }

    pub fn string(&self, prop: i32) -> Option<String> {
        let mut buf = [0u8; 128];
        let mut error = 0i32;
        // SAFETY: the buffer and its length are passed together.
        let n = unsafe { ffi::lfh_get_prop_string(self.0, prop, buf.as_mut_ptr() as *mut _, buf.len() as u32, &mut error) };
        if error != 0 || n == 0 {
            return None;
        }
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        Some(String::from_utf8_lossy(&buf[..end]).into_owned())
    }

    pub fn create_skeleton(&self, name: &str, skeleton_path: &str, base_pose_path: &str, tracking_level: i32) -> Option<u64> {
        let (name, skeleton, base) = (cstr(name), cstr(skeleton_path), cstr(base_pose_path));
        let mut handle = 0u64;
        // SAFETY: strings outlive the call; out-pointer to a local.
        let error = unsafe {
            ffi::lfh_create_skeleton_component(self.0, name.as_ptr(), skeleton.as_ptr(), base.as_ptr(), tracking_level, &mut handle)
        };
        (error == 0).then_some(handle)
    }
}

pub fn update_skeleton(handle: u64, motion_range: i32, bones: &[BoneTransform]) -> i32 {
    // SAFETY: BoneTransform mirrors vr::VRBoneTransform_t (checked at init).
    unsafe { ffi::lfh_update_skeleton(handle, motion_range, bones.as_ptr() as *const _, bones.len() as u32) }
}

pub fn setting_bool(key: &str, fallback: bool) -> bool {
    let (section, key) = (cstr("driver_leapfrog"), cstr(key));
    // SAFETY: strings outlive the call.
    unsafe { ffi::lfh_settings_bool(section.as_ptr(), key.as_ptr(), fallback) }
}

pub fn setting_float(key: &str, fallback: f32) -> f32 {
    let (section, key) = (cstr("driver_leapfrog"), cstr(key));
    // SAFETY: strings outlive the call.
    unsafe { ffi::lfh_settings_float(section.as_ptr(), key.as_ptr(), fallback) }
}

pub fn setting_string(key: &str, fallback: &str) -> String {
    let (section, key) = (cstr("driver_leapfrog"), cstr(key));
    let mut buf = [0u8; 256];
    // SAFETY: the buffer and its length are passed together; strings outlive the call.
    let n = unsafe { ffi::lfh_settings_string(section.as_ptr(), key.as_ptr(), buf.as_mut_ptr() as *mut _, buf.len() as u32) };
    if n == 0 {
        return fallback.to_string();
    }
    String::from_utf8_lossy(&buf[..n as usize]).into_owned()
}
