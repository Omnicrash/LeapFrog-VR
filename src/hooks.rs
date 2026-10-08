//! The Rust side of the LF-6 passthrough hooks (`shim/hooks.cpp`, doc/Passthrough.md).
//!
//! These run on other drivers' threads, in the middle of their host calls, so they keep their own
//! small mutex and never touch [`crate::STATE`]: the frame loop will write what they need here,
//! they only read. This scaffold passes everything through and logs each device, skeleton and
//! interface the first time it is seen.

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
    /// Keep the controller's own root and wrist bones (held placement) or replace all 31.
    pub fingers_only: bool,
}

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
    /// Per hand: the controller's property container (to find its skeleton) and what to inject.
    containers: [Option<u64>; 2],
    object_ids: [Option<u32>; 2],
    /// Each controller's world-from-driver transform, from the last pose lighthouse sent.
    frames: [Option<(Quat, Vec3)>; 2],
    /// Each controller's last pose as lighthouse sent it, before any rewrite (the real one).
    real: [Option<RealPose>; 2],
    injections: [Option<Injection>; 2],
    injected_poses: u64,
    /// Bone updates seen per hand since the last `take_bone_updates`.
    bone_updates: [u32; 2],
}

/// A controller's pose as its own driver sent it, composed into world space the way vrserver
/// does (world_from_driver * pose * driver_from_head), with the raw flags.
#[derive(Clone, Copy, Debug)]
pub struct RealPose {
    pub position: Vec3,
    pub rotation: Quat,
    pub speed: f32,
    pub angular_speed: f32,
    pub result: i32,
    pub valid: bool,
    pub connected: bool,
    pub at_us: i64,
}

impl Hooks {
    const fn new() -> Hooks {
        Hooks {
            pose_devices: Vec::new(),
            poses: 0,
            skeletons: Vec::new(),
            update_handles: Vec::new(),
            updates: 0,
            containers: [None, None],
            object_ids: [None, None],
            frames: [None, None],
            real: [None, None],
            injections: [None, None],
            injected_poses: 0,
            bone_updates: [0, 0],
        }
    }
}

/// The frame loop names a hand's controller (device id and property container), once known.
pub fn set_controller(hand: usize, object_id: Option<u32>, container: Option<u64>) {
    with_hooks(|h| {
        h.object_ids[hand] = object_id;
        h.containers[hand] = container;
        h.frames[hand] = None;
        h.real[hand] = None;
    });
}

/// The controller's last real pose (lighthouse's own, unmodified), for the switching while the
/// reported pose is ours.
pub fn real_pose(hand: usize) -> Option<RealPose> {
    with_hooks(|h| h.real[hand]).flatten()
}

/// The controller's driver frame (world-from-driver rotation and translation) as lighthouse
/// last reported it, for poses the frame loop sends itself while lighthouse sends none.
pub fn driver_frame(hand: usize) -> Option<(Quat, Vec3)> {
    with_hooks(|h| h.frames[hand]).flatten()
}

/// Writes an injection into a pose expressed in the given driver frame (world = frame * driver).
pub fn write_pose(p: &mut DriverPose, inj: &Injection, frame: (Quat, Vec3)) {
    let (wd_rotation, wd_translation) = frame;
    let rotation = (wd_rotation.inverse() * inj.rotation).normalize();
    let position = wd_rotation.inverse() * (inj.position - wd_translation);
    p.world_from_driver_rotation = host::Quaternion { w: wd_rotation.w as f64, x: wd_rotation.x as f64, y: wd_rotation.y as f64, z: wd_rotation.z as f64 };
    p.world_from_driver_translation = [wd_translation.x as f64, wd_translation.y as f64, wd_translation.z as f64];
    p.rotation = host::Quaternion { w: rotation.w as f64, x: rotation.x as f64, y: rotation.y as f64, z: rotation.z as f64 };
    p.position = [position.x as f64, position.y as f64, position.z as f64];
    p.velocity = [0.0; 3];
    p.acceleration = [0.0; 3];
    p.angular_velocity = [0.0; 3];
    p.angular_acceleration = [0.0; 3];
    // The injected pose is final: no driver-from-head offset on top (lighthouse's is logged
    // at first sight; it is what put SteamVR's controller 16 cm from the injection, 04:44).
    p.driver_from_head_rotation = host::Quaternion::IDENTITY;
    p.driver_from_head_translation = [0.0; 3];
    p.pose_is_valid = true;
    p.device_is_connected = true;
    p.result = host::TRACKING_RESULT_RUNNING_OK;
}

/// The frame loop sets (or clears) what the hooks write into a hand's controller.
pub fn set_injection(hand: usize, injection: Option<Injection>) {
    with_hooks(|h| h.injections[hand] = injection);
}

/// The current injection for a hand, if any.
pub fn injection(hand: usize) -> Option<Injection> {
    with_hooks(|h| h.injections[hand]).flatten()
}

/// Bone updates the controller's own driver sent for this hand since the last call.
pub fn take_bone_updates(hand: usize) -> u32 {
    with_hooks(|h| std::mem::take(&mut h.bone_updates[hand])).unwrap_or(0)
}

/// How many controller poses have been replaced so far (for the log).
pub fn injected_poses() -> u64 {
    with_hooks(|h| h.injected_poses).unwrap_or(0)
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
            let frame = (quat_f32(p.world_from_driver_rotation), vec_f32(p.world_from_driver_translation));
            let dfh = (quat_f32(p.driver_from_head_rotation), vec_f32(p.driver_from_head_translation));
            if !h.pose_devices.contains(&object_id) {
                h.pose_devices.push(object_id);
                log!("device {object_id}: first pose through the hook (connected {}, valid {}, result {}; driver-from-head {:.1} cm, {:.0} deg; world-from-driver {:.2} {:.2} {:.2}, {:.0} deg)",
                    p.device_is_connected, p.pose_is_valid, p.result, dfh.1.length() * 100.0, dfh.0.angle_between(Quat::IDENTITY).to_degrees(),
                    frame.1.x, frame.1.y, frame.1.z, frame.0.angle_between(Quat::IDENTITY).to_degrees());
            }
            if let Some(hand) = h.object_ids.iter().position(|id| *id == Some(object_id)) {
                h.frames[hand] = Some(frame);
                let rotation = quat_f32(p.rotation);
                let position = vec_f32(p.position);
                h.real[hand] = Some(RealPose {
                    position: frame.0 * (rotation * dfh.1 + position) + frame.1,
                    rotation: (frame.0 * rotation * dfh.0).normalize(),
                    speed: vec_f32(p.velocity).length(),
                    angular_speed: vec_f32(p.angular_velocity).length(),
                    result: p.result,
                    valid: p.pose_is_valid,
                    connected: p.device_is_connected,
                    at_us: crate::leap::now_us(),
                });
                if let Some(inj) = h.injections[hand] {
                    write_pose(p, &inj, frame);
                    h.injected_poses += 1;
                }
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
            let container = h.skeletons.iter().find(|s| s.0 == handle).map(|s| s.1);
            if !h.update_handles.contains(&handle) {
                h.update_handles.push(handle);
                log!("skeleton handle {handle} (container {container:?}): first bones through the hook, range {motion_range}, {count} bones");
            }
            let hand = container.and_then(|c| h.containers.iter().position(|hc| *hc == Some(c)));
            if let Some(hand) = hand {
                h.bone_updates[hand] = h.bone_updates[hand].saturating_add(1);
            }
            if let Some(inj) = hand.and_then(|hand| h.injections[hand].as_ref()) {
                if count as usize == BONE_COUNT {
                    // SAFETY: `count` bones per the caller's contract.
                    let out = unsafe { std::slice::from_raw_parts_mut(bones, count as usize) };
                    let from = if inj.fingers_only { 2 } else { 0 };
                    out[from..].copy_from_slice(&inj.bones[from..]);
                }
            }
        });
    })
}

fn quat_f32(q: host::Quaternion) -> Quat {
    Quat::from_xyzw(q.x as f32, q.y as f32, q.z as f32, q.w as f32).normalize()
}

fn vec_f32(v: [f64; 3]) -> Vec3 {
    Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32)
}
