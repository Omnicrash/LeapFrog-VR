//! LeapFrog: a minimal SteamVR driver that adds Ultraleap hands as hand-tracking devices.
//!
//! The `lf_*` functions are the driver's entry points, called by the C++ shim (`shim/shim.cpp`)
//! from vrserver's threads. Every one of them runs inside [`guard`]: a panic is logged and
//! latches the driver off (both hands stay reported disconnected) instead of aborting vrserver,
//! which a panic crossing `extern "C"` would do.

mod controllers;
mod hand;
mod hand_model;
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
    write_hand_model: bool,
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

fn read_thresholds() -> Thresholds {
    let d = Thresholds::default();
    Thresholds {
        away_m: host::setting_float("awayDistance", d.away_m),
        near_m: host::setting_float("nearDistance", d.near_m),
        away_s: host::setting_float("awayTime", d.away_s),
        unseen_s: host::setting_float("unseenTime", d.unseen_s),
        still_mps: host::setting_float("stillSpeed", d.still_mps),
        pickup_mps: host::setting_float("pickupSpeed", d.pickup_mps),
    }
}

fn read_offsets() -> Offsets {
    Offsets {
        position: Vec3::new(
            host::setting_float("handOffsetX", 0.0),
            host::setting_float("handOffsetY", 0.0),
            host::setting_float("handOffsetZ", -0.08),
        ),
        rotation_degrees: Vec3::new(
            host::setting_float("rotationOffsetX", 0.0),
            host::setting_float("rotationOffsetY", 0.0),
            host::setting_float("rotationOffsetZ", 0.0),
        ),
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
        // pickupSwitching off = stage 1 behavior, the controller's power alone decides.
        let thresholds = if host::setting_bool("pickupSwitching", true) {
            read_thresholds()
        } else {
            Thresholds { away_m: f32::INFINITY, ..Thresholds::default() }
        };
        let state = State {
            hands,
            controllers: Controllers::new(),
            #[cfg(not(feature = "host-stubs"))]
            leap: leap::Leap::start(leap::Config {
                manage_service: host::setting_bool("manageService", true),
                hint: Some(host::setting_string("trackingHint", "ultra_performance_mode")).filter(|h| !h.is_empty()),
            }),
            poses: vec![TrackedDevicePose::default(); host::MAX_TRACKED_DEVICE_COUNT],
            switches: [Switch::new(thresholds), Switch::new(thresholds)],
            offsets,
            write_hand_model: host::setting_bool("writeHandModel", false),
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

        for (i, hand) in self.hands.iter_mut().enumerate() {
            let hand_position = hand.locate(samples[i].as_ref(), head.as_ref(), &self.offsets);
            let controller = self.controllers.pose(i as u32, &self.poses);
            let observation = Observation {
                controller_known: controller.is_some(),
                controller_on: controller.map(|c| c.device_is_connected).unwrap_or(false),
                controller_position: controller.filter(|c| c.pose_is_valid).map(|c| {
                    let m = &c.device_to_absolute_tracking;
                    Vec3::new(m[0][3], m[1][3], m[2][3])
                }),
                controller_velocity: controller.map(|c| Vec3::from(c.velocity)).unwrap_or(Vec3::ZERO),
                hand_position,
            };
            let driver = self.switches[i].update(now_s, &observation);
            hand.apply(driver == Driver::Leap);
            hand.publish();
            if self.write_hand_model && !hand.model_written && hand_position.is_some() {
                hand.write_model();
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
