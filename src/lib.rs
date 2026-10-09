//! LeapFrog: a minimal SteamVR driver that adds Ultraleap hands as hand-tracking devices.
//!
//! The `lf_*` functions are the driver's entry points, called by the C++ shim (`shim/shim.cpp`)
//! from vrserver's threads. Every one of them runs inside [`guard`]: a panic is logged and
//! latches the driver off (both hands stay reported disconnected, the hooks pass everything
//! through) instead of aborting vrserver, which a panic crossing `extern "C"` would do.

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
use glam::{Quat, Vec3};
use hand::Hand;
use host::TrackedDevicePose;
use log::log;
use math::Offsets;
use switching::{Driver, Observation, Switch, Thresholds};

static DISABLED: AtomicBool = AtomicBool::new(false);
static STATE: Mutex<Option<State>> = Mutex::new(None);

/// A sample older than this is a hand the Leap no longer sees.
const STALE_US: i64 = 300_000;

/// A real controller pose older than this means lighthouse is blind to the controller.
const UNSEEN_US: i64 = 100_000;

/// SteamVR counts binding loads per app process for its whole run and VRChat refuses them from
/// 100 (seen in vrclient_VRChat.txt: "Failed to load binding file ... load count=100"); a device
/// transition costs about one. With the passthrough on, transitions only happen on controller
/// power, so this rarely matters; warn from here on, every 25.
const TRANSITION_WARNING: u32 = 60;

struct State {
    hands: [Hand; 2],
    controllers: Controllers,
    #[cfg(not(feature = "host-stubs"))]
    leap: leap::Leap,
    poses: Vec<TrackedDevicePose>,
    switches: [Switch; 2],
    thresholds: Thresholds,
    offsets: Offsets,
    pickup_enabled: bool,
    manage_service: bool,
    /// Whether SteamVR's room setup is running, which needs the controllers.
    room_setup: service::ProcessWatch,
    room_setup_running: bool,
    /// The controllers stay the hand devices and get the Leap's data while put down.
    passthrough: bool,
    /// Per hand: whether the Leap drives the controller right now, and which controller.
    leap_drives: [bool; 2],
    controller_ids: [Option<u32>; 2],
    grip_logged: [bool; 2],
    /// When the live settings were last read (LeapC's clock).
    settings_read_us: i64,
    transitions: u32,
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
            hooks::clear();
            log!("panic in {name}: {msg}; driver disabled, hands stay disconnected");
            fallback
        }
    }
}

/// The state, or None before init. A panic while the lock was held leaves it poisoned; the data
/// is still usable (the disabled path only publishes disconnected hands), so recover it.
fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> Option<T> {
    let mut guard = STATE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.as_mut().map(f)
}

/// The Ultraleap tracking hints from the settings: one performance mode (an axis, so a single
/// choice) plus independent flags. Gemini 5.20 knows exactly these four hint strings.
#[cfg_attr(feature = "host-stubs", allow(dead_code))]
fn tracking_hints() -> Vec<std::ffi::CString> {
    let mut hints: Vec<&str> = Vec::new();
    let performance = host::setting_string(c"trackingPerformance", "ultra");
    match performance.as_str() {
        "ultra" => hints.push("ultra_performance_mode"),
        "low" => hints.push("low_resource_usage"),
        "default" => {}
        other => log!("trackingPerformance \"{other}\" unknown (ultra, low, default); using the service default"),
    }
    if host::setting_bool(c"handOnObject", true) {
        hints.push("hand_on_object");
    }
    if host::setting_bool(c"fastHandMotion", false) {
        hints.push("fast_hand_motion");
    }
    hints.into_iter().filter_map(|h| std::ffi::CString::new(h).ok()).collect()
}

fn read_thresholds() -> Thresholds {
    let d = Thresholds::default();
    Thresholds {
        away_m: host::setting_float(c"awayDistance", d.away_m),
        near_m: host::setting_float(c"nearDistance", d.near_m),
        still_mps: host::setting_float(c"stillSpeed", d.still_mps),
        pickup_mps: host::setting_float(c"pickupSpeed", d.pickup_mps),
    }
}

fn read_offsets() -> Offsets {
    let d = Offsets::default();
    Offsets {
        position: Vec3::new(
            host::setting_float(c"handOffsetX", d.position.x),
            host::setting_float(c"handOffsetY", d.position.y),
            host::setting_float(c"handOffsetZ", d.position.z),
        ),
        rotation_degrees: Vec3::new(
            host::setting_float(c"rotationOffsetX", d.rotation_degrees.x),
            host::setting_float(c"rotationOffsetY", d.rotation_degrees.y),
            host::setting_float(c"rotationOffsetZ", d.rotation_degrees.z),
        ),
        inward: host::setting_float(c"handInwardOffset", d.inward),
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
        let manage_service = host::setting_bool(c"manageService", true);
        hooks::EXPOSE_FULL.store(host::setting_bool(c"exposeAsFullTrackingMode", false), Ordering::Relaxed);
        let state = State {
            hands,
            controllers: Controllers::new(),
            #[cfg(not(feature = "host-stubs"))]
            leap: leap::Leap::start(leap::Config {
                manage_service,
                resume_tracking: host::setting_bool(c"autoResumeTracking", true),
                hints: tracking_hints(),
            }),
            poses: vec![TrackedDevicePose::default(); host::MAX_TRACKED_DEVICE_COUNT],
            switches: [Switch::new(thresholds), Switch::new(thresholds)],
            thresholds,
            offsets,
            pickup_enabled: host::setting_bool(c"autoPickupSwitch", true),
            manage_service,
            room_setup: service::ProcessWatch::new("steamvr_room_setup.exe"),
            room_setup_running: false,
            passthrough: host::setting_bool(c"passthrough", true),
            leap_drives: [false, false],
            controller_ids: [None, None],
            grip_logged: [false, false],
            settings_read_us: leap::now_us(),
            transitions: 0,
        };
        *STATE.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(state);
        service::start_panel();
        log!("init: version {}, two hands registered, offsets {:?}, switching {:?}", env!("CARGO_PKG_VERSION"), offsets, thresholds);
        host::INIT_ERROR_NONE
    })
}

/// Whether the shim should install the passthrough hooks: the Rust side read the setting.
#[no_mangle]
pub extern "C" fn lf_hooks_wanted() -> bool {
    guard("hooks_wanted", false, || with_state(|state| state.passthrough).unwrap_or(false))
}

#[no_mangle]
pub extern "C" fn lf_cleanup() {
    guard("cleanup", (), || {
        // The hooks stay patched until the shim restores them right after this: let them pass
        // everything through from here on.
        hooks::clear();
        let manage_service = with_state(|state| state.manage_service).unwrap_or(false);
        *STATE.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None; // drops the Leap thread
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
            #[cfg(feature = "debug-panic")]
            debug_panic_once();
            state.frame();
        });
    })
}

/// Proves the panic guard on a live SteamVR: a build with the `debug-panic` feature panics in
/// its first frame, logs it, and leaves vrserver alive with the hands disconnected.
#[cfg(feature = "debug-panic")]
fn debug_panic_once() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::SeqCst) {
        panic!("debug-panic feature: deliberate panic in the first frame");
    }
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

        let second = now - self.settings_read_us >= 1_000_000;
        if second {
            self.settings_read_us = now;
            self.reload_settings();
        }
        let force_controller = !self.pickup_enabled || self.room_setup_running;

        for i in 0..2 {
            let hand_position = self.hands[i].locate(samples[i].as_ref(), head.as_ref(), &self.offsets);
            let observation = self.observe(i, now, force_controller, hand_position);
            let driver = self.switches[i].update(&observation);
            let controller_id = self.controllers.index(i as u32);
            if controller_id != self.controller_ids[i] {
                self.controller_ids[i] = controller_id;
                hooks::set_controller(i, controller_id, controller_id.map(|id| host::Properties::of(id).container()));
            }
            if !self.grip_logged[i] {
                if let Some(grip) = self.switches[i].grip() {
                    self.grip_logged[i] = true;
                    log!("{} grip point learned (wrist in the controller frame): {:.3} {:.3} {:.3} m", self.hands[i].side(), grip.x, grip.y, grip.z);
                }
            }
            // Passthrough: with its controller on, the controller stays the hand device and gets
            // the Leap's pose and fingers while the Leap drives; the Leap device only connects
            // for a controller that is off.
            let composite = self.passthrough && observation.controller_on;
            let leap_drives = composite && driver == Driver::Leap;
            self.drive_controller(i, leap_drives, controller_id, second);
            if leap_drives != self.leap_drives[i] {
                self.leap_drives[i] = leap_drives;
                log!("{} hand: {} drives the controller ({})", self.hands[i].side(), if leap_drives { "the Leap" } else { "the controller" }, self.switches[i].reason());
            }
            if self.hands[i].apply(driver == Driver::Leap && !composite) {
                self.transitions += 1;
                log!("{} hand {} ({})", self.hands[i].side(), if driver == Driver::Leap { "takes over" } else { "steps back" }, self.switches[i].reason());
                if self.transitions >= TRANSITION_WARNING && (self.transitions - TRANSITION_WARNING) % 25 == 0 {
                    log!("{} device transitions this session: VRChat refuses binding loads from 100 per run (about one per transition); restart VRChat when convenient", self.transitions);
                }
            }
            self.hands[i].publish();
        }
    }

    /// Once a second: the live switch (autoPickupSwitch off = controller power alone decides),
    /// the offsets and thresholds (so the panel tunes them by eye without restarts), and whether
    /// SteamVR's room setup is running.
    fn reload_settings(&mut self) {
        self.pickup_enabled = host::setting_bool(c"autoPickupSwitch", true);
        let offsets = read_offsets();
        if offsets != self.offsets {
            self.offsets = offsets;
            log!("offsets changed: {offsets:?}");
        }
        let thresholds = read_thresholds();
        if thresholds != self.thresholds {
            self.thresholds = thresholds;
            for switch in &mut self.switches {
                switch.set_thresholds(thresholds);
            }
            log!("switching thresholds changed: {thresholds:?}");
        }
        let room_setup = self.room_setup.running();
        if room_setup != self.room_setup_running {
            self.room_setup_running = room_setup;
            log!("room setup {}: controllers {}", if room_setup { "running" } else { "closed" }, if room_setup { "hold the slots" } else { "switch again" });
        }
    }

    /// What the switching sees for one hand this frame. While the Leap drives a controller
    /// (passthrough), the pose SteamVR reports for it is ours, so the controller's real pose
    /// comes from the hook instead; without that the injected pose fed the pick-up rule and the
    /// hands flickered. A real pose older than [`UNSEEN_US`] means lighthouse is blind to the
    /// controller: then it is "unseen" (no position, no motion), never the reported pose.
    fn observe(&self, i: usize, now: i64, force_controller: bool, hand_position: Option<Vec3>) -> Observation {
        let controller = self.controllers.pose(i as u32, &self.poses);
        let mut o = Observation {
            force_controller,
            controller_known: controller.is_some(),
            controller_on: false,
            controller_position: None,
            controller_rotation: Quat::IDENTITY,
            controller_velocity: Vec3::ZERO,
            hand_position,
        };
        if let Some(r) = self.passthrough.then(|| hooks::real_pose(i)).flatten() {
            let seen = now - r.at_us < UNSEEN_US;
            o.controller_on = r.connected;
            o.controller_position = (seen && r.valid && r.result == host::TRACKING_RESULT_RUNNING_OK).then_some(r.position);
            o.controller_rotation = r.rotation;
            if seen {
                o.controller_velocity = Vec3::new(r.speed, 0.0, 0.0);
            }
        } else if let Some(c) = controller {
            o.controller_on = c.device_is_connected;
            if c.pose_is_valid {
                let head = math::Head::from_pose(c);
                // Only a fully tracked pose: a controller that lost the base stations
                // dead-reckons on its gyros and drifts, which must not look like a put-down.
                if c.tracking_result == host::TRACKING_RESULT_RUNNING_OK {
                    o.controller_position = Some(head.translation);
                }
                o.controller_rotation = head.rotation;
            }
            o.controller_velocity = Vec3::from(c.velocity);
        }
        o
    }

    /// What the hooks write into the controller: while the Leap drives, the Leap hand device's
    /// own pose and full skeleton, so the controller becomes that device while put down. While
    /// held, the controller's own data passes untouched: an injected pose while held turns the
    /// dashboard model and the pointer with the hand, and the Leap's bones on a held controller
    /// make VRChat drop its buttons. (The placement difference between a held hand, VRChat's
    /// rule, and a Leap hand stays open.)
    fn drive_controller(&mut self, i: usize, leap_drives: bool, controller_id: Option<u32>, second: bool) {
        let hand = &self.hands[i];
        let injection = if leap_drives {
            controller_id.zip(hand.world_pose()).map(|(object_id, (rotation, position))| hooks::Injection { object_id, rotation, position, bones: *hand.bones() })
        } else {
            None
        };
        let frame = hooks::set_injection(i, injection);
        // Lighthouse sends no poses for a controller it cannot see, which is the main case:
        // send the injected pose ourselves, in the controller's own frame.
        if let (Some(inj), Some(frame)) = (injection, frame) {
            let mut pose = host::DriverPose::disconnected();
            hooks::write_pose(&mut pose, &inj, frame);
            if !host::pose_updated_as(inj.object_id, &pose) && second {
                log!("{} hand: no host object known for controller {} yet, own pose not sent", hand.side(), inj.object_id);
            }
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
