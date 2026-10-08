//! The Rust side of the LF-6 passthrough hooks (`shim/hooks.cpp`, doc/Passthrough.md).
//!
//! These run on other drivers' threads, in the middle of their host calls, so they keep their own
//! small mutex and never touch [`crate::STATE`]: the frame loop will write what they need here,
//! they only read. This scaffold passes everything through and logs each device, skeleton and
//! interface the first time it is seen.

use std::sync::Mutex;

use crate::guard;
use crate::host::{BoneTransform, DriverPose};
use crate::log::log;

static HOOKS: Mutex<Hooks> = Mutex::new(Hooks::new());

struct Hooks {
    /// Object ids whose pose has come through, with their first connected/valid flags.
    pose_devices: Vec<u32>,
    poses: u64,
    /// Skeleton components created by other drivers: (handle, container).
    skeletons: Vec<(u64, u64)>,
    /// Skeleton handles whose bones have come through.
    update_handles: Vec<u64>,
    updates: u64,
}

impl Hooks {
    const fn new() -> Hooks {
        Hooks { pose_devices: Vec::new(), poses: 0, skeletons: Vec::new(), update_handles: Vec::new(), updates: 0 }
    }
}

fn with_hooks<T>(f: impl FnOnce(&mut Hooks) -> T) -> Option<T> {
    HOOKS.lock().ok().map(|mut h| f(&mut h))
}

fn c_str(ptr: *const std::os::raw::c_char) -> String {
    if ptr.is_null() {
        return String::from("(null)");
    }
    // SAFETY: vrserver's callers pass NUL-terminated strings; the shim only forwards them.
    unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy().into_owned()
}

/// Another driver's pose update, which may be edited in place before vrserver sees it.
///
/// # Safety
/// `pose` points at a DriverPose_t copy the shim owns for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn lf_hook_pose(object_id: u32, pose: *mut DriverPose) {
    guard("hook_pose", (), || {
        if pose.is_null() {
            return;
        }
        // SAFETY: the caller's contract above.
        let p = unsafe { &mut *pose };
        with_hooks(|h| {
            h.poses += 1;
            if !h.pose_devices.contains(&object_id) {
                h.pose_devices.push(object_id);
                log!("device {object_id}: first pose through the hook (connected {}, valid {}, result {})", p.device_is_connected, p.pose_is_valid, p.result);
            }
        });
    })
}

/// Another driver creates a skeleton: returns the tracking level to register it with.
///
/// # Safety
/// `name` and `skeleton_path` are NUL-terminated or null.
#[no_mangle]
pub unsafe extern "C" fn lf_hook_skeleton_create(container: u64, name: *const std::os::raw::c_char, skeleton_path: *const std::os::raw::c_char, tracking_level: i32) -> i32 {
    guard("hook_skeleton_create", tracking_level, || {
        // Experiment 1 (doc/Passthrough.md): register other drivers' skeletons as Full and see
        // whether VRChat then treats the Index controllers as hand-tracking devices.
        let full = crate::host::setting_bool("indexSkeletonFull", true);
        let level = if full { crate::host::SKELETAL_TRACKING_FULL } else { tracking_level };
        log!("skeleton {} ({}) on container {container}: level {tracking_level} requested, {level} registered", c_str(name), c_str(skeleton_path));
        level
    })
}

#[no_mangle]
pub extern "C" fn lf_hook_skeleton_created(container: u64, handle: u64) {
    guard("hook_skeleton_created", (), || {
        with_hooks(|h| h.skeletons.push((handle, container)));
    })
}

/// Another driver's bones, 31 of them, which may be edited in place.
///
/// # Safety
/// `bones` points at `count` VRBoneTransform_t the shim owns for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn lf_hook_skeleton_update(handle: u64, motion_range: i32, bones: *mut BoneTransform, count: u32) {
    guard("hook_skeleton_update", (), || {
        if bones.is_null() || count == 0 {
            return;
        }
        with_hooks(|h| {
            h.updates += 1;
            if !h.update_handles.contains(&handle) {
                h.update_handles.push(handle);
                let container = h.skeletons.iter().find(|s| s.0 == handle).map(|s| s.1);
                log!("skeleton handle {handle} (container {container:?}): first bones through the hook, range {motion_range}, {count} bones");
            }
        });
    })
}
