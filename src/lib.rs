//! LeapFrog: a minimal SteamVR driver that adds Ultraleap hands as hand-tracking devices.
//!
//! The `lf_*` functions are the driver's entry points, called by the C++ shim (`shim/shim.cpp`)
//! from vrserver's threads. Every one of them runs inside [`guard`]: a panic is logged and
//! latches the driver off (both hands stay reported disconnected) instead of aborting vrserver,
//! which a panic crossing `extern "C"` would do.

mod controllers;
mod hand;
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
    frame: u32,
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
        lockout_s: host::setting_float("lockoutTime", d.lockout_s),
    }
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
            frame: 0,
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
                hand_position,
            };
            let driver = self.switches[i].update(now_s, &observation);
            if self.frame % 900 == 0 {
                if let Some(grip) = self.switches[i].grip() {
                    log!("{} grip point (wrist in the controller frame): {:.3} {:.3} {:.3} m", hand.side(), grip.x, grip.y, grip.z);
                }
            }
            hand.apply(driver == Driver::Leap);
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
