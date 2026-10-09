//! The Rust side of the passthrough hooks (`shim/hooks.cpp`).
//!
//! These run on other drivers' threads, in the middle of their host calls, so they keep their own
//! small mutex and never touch [`crate::STATE`]: the frame loop writes what they need here, they
//! only read. The shim only passes the two watched controllers (and their skeletons) in; every
//! other device goes straight through.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use glam::{Quat, Vec3};

use crate::guard;
use crate::host::{self, BoneTransform, DriverPose};
use crate::log::log;
use crate::math::BONE_COUNT;

/// What the frame loop wants written into a controller while the Leap drives its hand: the
/// Leap hand device's own pose (world space) and its full skeleton, root and wrist included,
/// so the controller is that device while put down.
#[derive(Clone, Copy)]
pub struct Injection {
    pub object_id: u32,
    pub rotation: Quat,
    pub position: Vec3,
    pub bones: [BoneTransform; BONE_COUNT],
}

/// `exposeAsFullTrackingMode`, read once at init: the level is fixed per skeleton anyway.
pub static EXPOSE_FULL: AtomicBool = AtomicBool::new(false);

static HOOKS: Mutex<Hooks> = Mutex::new(Hooks::new());

struct Hooks {
    /// Skeleton components created by other drivers: (handle, container).
    skeletons: Vec<(u64, u64)>,
    /// Per hand: the controller's property container (to find its skeleton) and device id.
    containers: [Option<u64>; 2],
    object_ids: [Option<u32>; 2],
    /// Each controller's world-from-driver transform, from the last pose lighthouse sent.
    frames: [Option<(Quat, Vec3)>; 2],
    /// Each controller's last pose as lighthouse sent it, before any rewrite (the real one).
    real: [Option<RealPose>; 2],
    injections: [Option<Injection>; 2],
}

/// A controller's pose as its own driver sent it, composed into world space the way vrserver
/// does (world_from_driver * pose * driver_from_head), with the raw flags.
#[derive(Clone, Copy, Debug)]
pub struct RealPose {
    pub position: Vec3,
    pub rotation: Quat,
    pub speed: f32,
    pub result: i32,
    pub valid: bool,
    pub connected: bool,
    pub at_us: i64,
}

impl Hooks {
    const fn new() -> Hooks {
        Hooks { skeletons: Vec::new(), containers: [None, None], object_ids: [None, None], frames: [None, None], real: [None, None], injections: [None, None] }
    }

    /// Tells the shim which device and skeleton handle to pass in for this hand.
    fn watch(&self, hand: usize) {
        let object_id = self.object_ids[hand].unwrap_or(u32::MAX);
        let skeleton = self.containers[hand].and_then(|c| self.skeletons.iter().find(|s| s.1 == c)).map(|s| s.0).unwrap_or(0);
        host::hooks_watch(hand as u32, object_id, skeleton);
    }
}

/// The frame loop names a hand's controller (device id and property container), once known.
pub fn set_controller(hand: usize, object_id: Option<u32>, container: Option<u64>) {
    with_hooks(|h| {
        h.object_ids[hand] = object_id;
        h.containers[hand] = container;
        h.frames[hand] = None;
        h.real[hand] = None;
        h.watch(hand);
    });
}

/// Forgets both controllers and their injections: the driver is disabled or shutting down, so
/// the hooks pass everything through from here on.
pub fn clear() {
    with_hooks(|h| {
        for hand in 0..2 {
            h.object_ids[hand] = None;
            h.containers[hand] = None;
            h.frames[hand] = None;
            h.real[hand] = None;
            h.injections[hand] = None;
            h.watch(hand);
        }
    });
}

/// The controller's last real pose (lighthouse's own, unmodified), for the switching while the
/// reported pose is ours.
pub fn real_pose(hand: usize) -> Option<RealPose> {
    with_hooks(|h| h.real[hand]).flatten()
}

/// The frame loop sets (or clears) what the hooks write into a hand's controller, and gets the
/// controller's driver frame (world-from-driver rotation and translation, as lighthouse last
/// reported it) back for the poses it sends itself while lighthouse sends none.
pub fn set_injection(hand: usize, injection: Option<Injection>) -> Option<(Quat, Vec3)> {
    with_hooks(|h| {
        if injection.is_some() || h.injections[hand].is_some() {
            h.injections[hand] = injection;
        }
        h.frames[hand]
    })
    .flatten()
}

/// Writes an injection into a pose expressed in the given driver frame (world = frame * driver).
pub fn write_pose(p: &mut DriverPose, inj: &Injection, frame: (Quat, Vec3)) {
    let (wd_rotation, wd_translation) = frame;
    let rotation = (wd_rotation.inverse() * inj.rotation).normalize();
    let position = wd_rotation.inverse() * (inj.position - wd_translation);
    p.world_from_driver_rotation = wd_rotation.into();
    p.world_from_driver_translation = host::vec_f64(wd_translation);
    p.rotation = rotation.into();
    p.position = host::vec_f64(position);
    p.velocity = [0.0; 3];
    p.acceleration = [0.0; 3];
    p.angular_velocity = [0.0; 3];
    p.angular_acceleration = [0.0; 3];
    // The injected pose is final: no driver-from-head offset on top (lighthouse's 16 cm offset
    // on an Index controller is what put SteamVR's controller away from the injection).
    p.driver_from_head_rotation = host::Quaternion::IDENTITY;
    p.driver_from_head_translation = [0.0; 3];
    p.pose_is_valid = true;
    p.device_is_connected = true;
    p.result = host::TRACKING_RESULT_RUNNING_OK;
}

/// The hooks' state; a panic while it was held (poisoned) is recovered from, the data is plain.
fn with_hooks<T>(f: impl FnOnce(&mut Hooks) -> T) -> Option<T> {
    let mut h = HOOKS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    Some(f(&mut h))
}

fn c_str(ptr: *const std::os::raw::c_char) -> String {
    if ptr.is_null() {
        return String::from("(null)");
    }
    // SAFETY: vrserver's callers pass NUL-terminated strings; the shim only forwards them.
    unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy().into_owned()
}

/// A watched controller's pose update, which may be edited in place before vrserver sees it.
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
            let Some(hand) = h.object_ids.iter().position(|id| *id == Some(object_id)) else { return };
            let frame = (Quat::from(p.world_from_driver_rotation), host::vec_f32(p.world_from_driver_translation));
            let dfh = (Quat::from(p.driver_from_head_rotation), host::vec_f32(p.driver_from_head_translation));
            let rotation = Quat::from(p.rotation);
            let position = host::vec_f32(p.position);
            h.frames[hand] = Some(frame);
            h.real[hand] = Some(RealPose {
                position: frame.0 * (rotation * dfh.1 + position) + frame.1,
                rotation: (frame.0 * rotation * dfh.0).normalize(),
                speed: host::vec_f32(p.velocity).length(),
                result: p.result,
                valid: p.pose_is_valid,
                connected: p.device_is_connected,
                at_us: crate::leap::now_us(),
            });
            if let Some(inj) = h.injections[hand].as_ref() {
                write_pose(p, inj, frame);
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
        // exposeAsFullTrackingMode: label the controller's skeleton Full instead of the level
        // its driver asked for (Partial for Index); the bone data is the same either way. Off,
        // the default: VRChat stays in controller mode, pointer and buttons as usual, and the
        // fingers are fully articulated from the skeleton, the Leap's while put down. On:
        // VRChat switches to its hand-tracking mode for the controller (pinch controls with it
        // down, but the pointer comes from the hand and the menu from a gesture).
        let level = if EXPOSE_FULL.load(Ordering::Relaxed) { host::SKELETAL_TRACKING_FULL } else { tracking_level };
        log!("skeleton {} ({}) on container {container}: level {tracking_level} requested, {} registered", c_str(name), c_str(skeleton_path), if level == host::SKELETAL_TRACKING_FULL { "Full (exposeAsFullTrackingMode)" } else { "as requested" });
        level
    })
}

#[no_mangle]
pub extern "C" fn lf_hook_skeleton_created(container: u64, handle: u64) {
    guard("hook_skeleton_created", (), || {
        with_hooks(|h| {
            h.skeletons.push((handle, container));
            if let Some(hand) = h.containers.iter().position(|c| *c == Some(container)) {
                h.watch(hand);
            }
        });
    })
}

/// A watched skeleton's bones, 31 of them, which may be edited in place.
///
/// # Safety
/// `bones` points at `count` VRBoneTransform_t the shim owns for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn lf_hook_skeleton_update(handle: u64, _motion_range: i32, bones: *mut BoneTransform, count: u32) {
    guard("hook_skeleton_update", (), || {
        if count as usize != BONE_COUNT {
            return;
        }
        with_hooks(|h| {
            let container = h.skeletons.iter().find(|s| s.0 == handle).map(|s| s.1);
            let Some(hand) = container.and_then(|c| h.containers.iter().position(|hc| *hc == Some(c))) else { return };
            if let Some(inj) = h.injections[hand].as_ref() {
                // SAFETY: `count` bones per the caller's contract.
                let out = unsafe { std::slice::from_raw_parts_mut(bones, count as usize) };
                out.copy_from_slice(&inj.bones);
            }
        });
    })
}
