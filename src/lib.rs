//! LeapFrog: a minimal SteamVR driver that adds Ultraleap hands as hand-tracking devices.
//!
//! The `lf_*` functions are the driver's entry points, called by the C++ shim (`shim/shim.cpp`)
//! from vrserver's threads. Every one of them runs inside [`guard`]: a panic is logged and
//! latches the driver off (both hands stay reported disconnected) instead of aborting vrserver,
//! which a panic crossing `extern "C"` would do.

mod controllers;
mod hand;
mod hooks;
mod host;
mod leap;
mod log;
mod math;
mod service;
mod switching;

use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use controllers::Controllers;
use glam::Vec3;
use hand::Hand;
use host::TrackedDevicePose;
use log::log;
use math::Offsets;
use switching::{Driver, Observation, Switch, Thresholds};

static DISABLED: AtomicBool = AtomicBool::new(false);
static STATE: Mutex<Option<State>> = Mutex::new(None);

/// A sample older than this is a hand the Leap no longer sees.
const STALE_US: i64 = 300_000;

/// SteamVR counts binding loads per app process for its whole session and stops reloading
/// around 250 (doc/Background.md, "The FBT night"). Every hand transition costs VRChat about two
/// loads and the compositor one per two transitions, so warn from here on, every 25.
const TRANSITION_WARNING: u32 = 100;

struct State {
    hands: [Hand; 2],
    controllers: Controllers,
    #[cfg(not(feature = "host-stubs"))]
    leap: leap::Leap,
    poses: Vec<TrackedDevicePose>,
    switches: [Switch; 2],
    offsets: Offsets,
    pickup_enabled: bool,
    room_setup: bool,
    /// LF-6: the controllers stay the hand devices and get the Leap's data while put down.
    passthrough: bool,
    /// Per hand: whether the Leap drives the controller right now, and which controller.
    leap_drives: [bool; 2],
    controller_ids: [Option<u32>; 2],
    /// Hand orientation in the controller's frame while held (left hand; the right mirrors),
    /// yaw/pitch/roll in degrees from the settings, re-read once a second.
    grip_rotation_deg: Vec3,
    frame: u32,
    transitions: u32,
    debug_panic_pending: bool,
}

fn guard<T>(name: &str, fallback: T, f: impl FnOnce() -> T) -> T {
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => value,
        Err(payload) => {
            let msg = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "non-string panic".to_string());
            DISABLED.store(true, Ordering::SeqCst);
            log!("panic in {name}: {msg}; driver disabled, hands stay disconnected");
            fallback
        }
    }
}

/// The state, or None when poisoned (a panic while it was held) or not initialized.
fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> Option<T> {
    let mut guard = STATE.lock().ok()?;
    guard.as_mut().map(f)
}

/// The Ultraleap tracking hints from the settings: one performance mode (an axis, so a single
/// choice) plus independent flags. Gemini 5.20 knows exactly these four hint strings.
#[cfg_attr(feature = "host-stubs", allow(dead_code))]
fn tracking_hints() -> Option<String> {
    let mut hints: Vec<&str> = Vec::new();
    let performance = host::setting_string("trackingPerformance", "ultra");
    match performance.as_str() {
        "ultra" => hints.push("ultra_performance_mode"),
        "low" => hints.push("low_resource_usage"),
        "default" => {}
        other => log!("trackingPerformance \"{other}\" unknown (ultra, low, default); using the service default"),
    }
    if host::setting_bool("handOnObject", true) {
        hints.push("hand_on_object");
    }
    if host::setting_bool("fastHandMotion", false) {
        hints.push("fast_hand_motion");
    }
    (!hints.is_empty()).then(|| hints.join(","))
}

fn read_thresholds() -> Thresholds {
    let d = Thresholds::default();
    Thresholds {
        away_m: host::setting_float("awayDistance", d.away_m),
        near_m: host::setting_float("nearDistance", d.near_m),
        away_s: host::setting_float("awayTime", d.away_s),
        still_mps: host::setting_float("stillSpeed", d.still_mps),
        pickup_mps: host::setting_float("pickupSpeed", d.pickup_mps),
        spacing_s: host::setting_float("switchSpacing", d.spacing_s),
        dead_s: host::setting_float("deadSpotTime", d.dead_s),
    }
}

/// The hand's orientation in the controller's frame while held, as set (left hand, degrees).
fn read_grip_rotation() -> Vec3 {
    Vec3::new(host::setting_float("gripYaw", 2.0), host::setting_float("gripPitch", -48.0), host::setting_float("gripRoll", 87.0))
}

fn read_offsets() -> Offsets {
    Offsets {
        position: Vec3::new(
            host::setting_float("handOffsetX", 0.0),
            host::setting_float("handOffsetY", 0.0),
            host::setting_float("handOffsetZ", -0.01),
        ),
        rotation_degrees: Vec3::new(
            host::setting_float("rotationOffsetX", 0.0),
            host::setting_float("rotationOffsetY", 0.0),
            host::setting_float("rotationOffsetZ", 0.0),
        ),
        inward: host::setting_float("handInwardOffset", 0.01),
    }
}

#[no_mangle]
pub extern "C" fn lf_init() -> i32 {
    guard("init", host::INIT_ERROR_DRIVER_FAILED, || {
        if let Err(why) = host::layout_matches() {
            log!("init refused: {why}");
            return host::INIT_ERROR_DRIVER_FAILED;
        }
        let hands = [Hand::new(host::HAND_LEFT), Hand::new(host::HAND_RIGHT)];
        for hand in &hands {
            if !host::tracked_device_added(&hand.serial(), host::DEVICE_CLASS_CONTROLLER, hand.index) {
                log!("TrackedDeviceAdded refused {}", hand.serial());
            }
        }
        let offsets = read_offsets();
        let thresholds = read_thresholds();
        let state = State {
            hands,
            controllers: Controllers::new(),
            #[cfg(not(feature = "host-stubs"))]
            leap: leap::Leap::start(leap::Config {
                manage_service: host::setting_bool("manageService", true),
                hint: tracking_hints(),
            }),
            poses: vec![TrackedDevicePose::default(); host::MAX_TRACKED_DEVICE_COUNT],
            switches: [Switch::new(thresholds), Switch::new(thresholds)],
            offsets,
            pickup_enabled: host::setting_bool("pickupSwitching", true),
            room_setup: false,
            passthrough: host::setting_bool("passthrough", false),
            leap_drives: [false, false],
            controller_ids: [None, None],
            grip_rotation_deg: read_grip_rotation(),
            frame: 0,
            transitions: 0,
            debug_panic_pending: host::setting_bool("debugPanic", false),
        };
        *STATE.lock().unwrap() = Some(state);
        log!("init: version {}, two hands registered, offsets {:?}, switching {:?}", env!("CARGO_PKG_VERSION"), offsets, thresholds);
        host::INIT_ERROR_NONE
    })
}

#[no_mangle]
pub extern "C" fn lf_cleanup() {
    guard("cleanup", (), || {
        let manage_service = host::setting_bool("manageService", true);
        if let Ok(mut state) = STATE.lock() {
            *state = None; // drops the Leap thread
        }
        if manage_service {
            service::stop();
        }
        log!("cleanup");
    })
}

#[no_mangle]
pub extern "C" fn lf_run_frame() {
    guard("run_frame", (), || {
        with_state(|state| {
            if DISABLED.load(Ordering::SeqCst) {
                for hand in &mut state.hands {
                    hand.apply(false);
                    hand.publish();
                }
                return;
            }
            if state.debug_panic_pending {
                // DEBUG: proves the guard on a live SteamVR; driver_leapfrog.debugPanic in steamvr.vrsettings.
                state.debug_panic_pending = false;
                panic!("debugPanic requested");
            }
            state.frame();
        });
    })
}

impl State {
    fn frame(&mut self) {
        #[cfg(not(feature = "host-stubs"))]
        let (left, right) = self.leap.snapshot();
        #[cfg(feature = "host-stubs")]
        let (left, right): (Option<leap::HandSample>, Option<leap::HandSample>) = (None, None);
        let now = leap::now_us();
        let fresh = |s: Option<leap::HandSample>| s.filter(|s| now - s.timestamp_us < STALE_US);
        let samples = [fresh(left), fresh(right)];

        // The head where it was when the Leap frame was taken; the same call serves the
        // controllers' power state.
        let age_s = samples.iter().flatten().map(|s| (now - s.timestamp_us) as f32 * 1e-6).fold(0.0, f32::max);
        host::raw_poses(-age_s, &mut self.poses);
        let own = [self.hands[0].object_id, self.hands[1].object_id];
        self.controllers.update(&self.poses, own);
        let head = self.poses.first().copied();
        let now_s = now as f64 * 1e-6;

        // Once a second: the live switch (pickupSwitching off = stage 1, controller power alone
        // decides) and whether SteamVR's room setup is running, which needs the controllers.
        self.frame = self.frame.wrapping_add(1);
        if self.frame % 90 == 0 {
            self.pickup_enabled = host::setting_bool("pickupSwitching", true);
            // Offsets are live too, so they can be tuned by eye without restarts.
            let offsets = read_offsets();
            if offsets != self.offsets {
                self.offsets = offsets;
                log!("offsets changed: {offsets:?}");
            }
            let grip_rotation = read_grip_rotation();
            if grip_rotation != self.grip_rotation_deg {
                self.grip_rotation_deg = grip_rotation;
                log!("grip rotation changed: yaw {} pitch {} roll {} deg", grip_rotation.x, grip_rotation.y, grip_rotation.z);
            }
            let room_setup = service::process_running("steamvr_room_setup.exe");
            if room_setup != self.room_setup {
                self.room_setup = room_setup;
                log!("room setup {}: controllers {}", if room_setup { "running" } else { "closed" }, if room_setup { "hold the slots" } else { "switch again" });
            }
        }
        let force_controller = !self.pickup_enabled || self.room_setup;

        for (i, hand) in self.hands.iter_mut().enumerate() {
            let hand_position = hand.locate(samples[i].as_ref(), head.as_ref(), &self.offsets);
            let controller = self.controllers.pose(i as u32, &self.poses);
            let observation = Observation {
                force_controller,
                controller_known: controller.is_some(),
                controller_on: controller.map(|c| c.device_is_connected).unwrap_or(false),
                // Only a fully tracked pose: a controller that lost the base stations dead-reckons
                // on its gyros and drifts, which must not look like a put-down.
                controller_position: controller.filter(|c| c.pose_is_valid && c.tracking_result == host::TRACKING_RESULT_RUNNING_OK).map(|c| {
                    let m = &c.device_to_absolute_tracking;
                    Vec3::new(m[0][3], m[1][3], m[2][3])
                }),
                controller_rotation: controller.filter(|c| c.pose_is_valid).map(|c| math::Head::from_pose(c).rotation).unwrap_or(glam::Quat::IDENTITY),
                controller_velocity: controller.map(|c| Vec3::from(c.velocity)).unwrap_or(Vec3::ZERO),
                controller_angular_velocity: controller.map(|c| Vec3::from(c.angular_velocity)).unwrap_or(Vec3::ZERO),
                hand_position,
                hand_rotation: hand.world_pose().map(|(r, _)| r),
            };
            let driver = self.switches[i].update(now_s, &observation);
            let controller_id = self.controllers.index(i as u32);
            if controller_id != self.controller_ids[i] {
                self.controller_ids[i] = controller_id;
                hooks::set_controller(i, controller_id, controller_id.map(|id| host::Properties::of(id).container()));
            }
            if self.frame % 900 == 0 {
                if let Some(grip) = self.switches[i].grip() {
                    let (y, x, z) = self.switches[i].grip_rotation().to_euler(glam::EulerRot::YXZ);
                    log!("{} grip point (wrist in the controller frame): {:.3} {:.3} {:.3} m, hand rotation yaw {:.0} pitch {:.0} roll {:.0} deg{}",
                        hand.side(), grip.x, grip.y, grip.z, y.to_degrees(), x.to_degrees(), z.to_degrees(),
                        if self.passthrough { format!(", {} poses injected", hooks::injected_poses()) } else { String::new() });
                }
            }
            // Passthrough (LF-6): with its controller on, the controller stays the hand device
            // and gets the Leap's pose and fingers while the Leap drives; the Leap device only
            // connects for a controller that is off.
            let composite = self.passthrough && observation.controller_on;
            let leap_drives = composite && driver == Driver::Leap;
            if composite {
                let switch = &self.switches[i];
                let injection = if leap_drives {
                    controller_id.zip(hand.world_pose()).map(|(object_id, (hand_rotation, wrist))| {
                        // Where the controller would be, held with the hand where the Leap sees it.
                        // The hand-to-controller rotation is a setting (left hand; mirrored for the
                        // right); the learned one is only logged, it was off by ~90 deg (04:19).
                        let d = self.grip_rotation_deg * (std::f32::consts::PI / 180.0);
                        let (yaw, roll) = if i == 0 { (d.x, d.z) } else { (-d.x, -d.z) };
                        let grip_rotation = glam::Quat::from_euler(glam::EulerRot::YXZ, yaw, d.y, roll);
                        let rotation = (hand_rotation * grip_rotation.inverse()).normalize();
                        hooks::Injection { object_id, rotation, position: wrist - rotation * switch.grip_vector(), bones: *hand.bones() }
                    })
                } else {
                    None
                };
                hooks::set_injection(i, injection);
                // Lighthouse sends no poses for a controller it cannot see, which is the main
                // case: send the injected pose ourselves, in the controller's own frame.
                if let (Some(inj), Some(frame)) = (injection, hooks::driver_frame(i)) {
                    let mut pose = host::DriverPose::disconnected();
                    hooks::write_pose(&mut pose, &inj, frame);
                    host::pose_updated(inj.object_id, &pose);
                }
            } else {
                hooks::set_injection(i, None);
            }
            if leap_drives != self.leap_drives[i] {
                self.leap_drives[i] = leap_drives;
                log!("{} hand: {} drives the controller", hand.side(), if leap_drives { "the Leap" } else { "the controller" });
                let (result, valid, on) = controller.map(|c| (c.tracking_result, c.pose_is_valid, c.device_is_connected)).unwrap_or((0, false, false));
                log!("  because: {}; controller result {result} valid {valid} on {on}, speed {:.2} m/s, turn {:.2} rad/s; hand {}, distance {}",
                    self.switches[i].reason(), observation.controller_velocity.length(), observation.controller_angular_velocity.length(),
                    if hand_position.is_some() { "seen" } else { "not seen" },
                    self.switches[i].last_distance().map(|d| format!("{d:.2} m")).unwrap_or_else(|| "n/a".to_string()));
            }
            if hand.apply(driver == Driver::Leap && !composite) {
                self.transitions += 1;
                // The inputs behind the decision, for reading a wrong one off the log.
                let (result, valid, on) = controller.map(|c| (c.tracking_result, c.pose_is_valid, c.device_is_connected)).unwrap_or((0, false, false));
                log!("  because: {}; controller result {result} valid {valid} on {on}, speed {:.2} m/s, turn {:.2} rad/s; hand {}, distance {}",
                    self.switches[i].reason(), observation.controller_velocity.length(), observation.controller_angular_velocity.length(),
                    if hand_position.is_some() { "seen" } else { "not seen" },
                    self.switches[i].last_distance().map(|d| format!("{d:.2} m")).unwrap_or_else(|| "n/a".to_string()));
                if self.transitions >= TRANSITION_WARNING && (self.transitions - TRANSITION_WARNING) % 25 == 0 {
                    log!("{} hand transitions this session: SteamVR stops rebinding apps after about 125 (VRChat) and 400 (dashboard); restart VRChat or SteamVR when convenient", self.transitions);
                }
            }
            hand.publish();
        }
    }
}

fn with_hand<T>(index: u32, fallback: T, f: impl FnOnce(&mut Hand) -> T) -> T {
    with_state(|state| state.hands.get_mut(index as usize).map(f)).flatten().unwrap_or(fallback)
}

#[no_mangle]
pub extern "C" fn lf_hand_activate(hand: u32, object_id: u32) -> i32 {
    guard("hand_activate", host::INIT_ERROR_DRIVER_FAILED, || {
        with_hand(hand, host::INIT_ERROR_DRIVER_FAILED, |h| h.activate(object_id))
    })
}

#[no_mangle]
pub extern "C" fn lf_hand_deactivate(hand: u32) {
    guard("hand_deactivate", (), || with_hand(hand, (), |h| h.deactivate()))
}

#[no_mangle]
pub extern "C" fn lf_hand_enter_standby(_hand: u32) {}

/// Writes the hand's last pose into the DriverPose_t the shim hands over.
///
/// # Safety
/// `pose_out` must point at a vr::DriverPose_t (the shim's local).
#[no_mangle]
pub unsafe extern "C" fn lf_hand_get_pose(hand: u32, pose_out: *mut host::DriverPose) {
    guard("hand_get_pose", (), || {
        let pose = with_hand(hand, host::DriverPose::disconnected(), |h| h.pose());
        if !pose_out.is_null() {
            // SAFETY: the caller's contract above.
            unsafe { pose_out.write(pose) };
        }
    })
}
