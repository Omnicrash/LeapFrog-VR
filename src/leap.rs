//! The LeapC connection on its own thread. Publishes the newest sample per hand; the rules for
//! coming up next to a tracking service that may start later, or restart, are from
//! `doc/Background.md`:
//! - a connection opened before the service ran comes up once the service does but never gets
//!   hands: when the connection event arrives late (> 1 s after opening), open a fresh one;
//! - a lost connection (service stopped or restarted) gets a fresh connection too.

#[cfg(not(feature = "host-stubs"))]
use leap_sys as sys;

/// No LeapC on the build host: `cargo test` gets a clock and nothing else.
#[cfg(feature = "host-stubs")]
pub fn now_us() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_micros() as i64).unwrap_or(0)
}

#[derive(Clone, Copy, Debug)]
#[allow(dead_code)] // next_joint: fingertips, for later
pub struct Bone {
    pub prev_joint: [f32; 3],
    pub next_joint: [f32; 3],
    pub rotation: [f32; 4], // x, y, z, w
}

const IDENTITY: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

impl Default for Bone {
    fn default() -> Bone {
        Bone { prev_joint: [0.0; 3], next_joint: [0.0; 3], rotation: IDENTITY }
    }
}

/// One hand from one tracking frame, plain data copied out of LeapC's event.
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)] // elbow and next_joint are for the put-down detection of stage 2
pub struct HandSample {
    pub timestamp_us: i64,
    pub palm_position: [f32; 3],
    pub palm_orientation: [f32; 4],
    pub wrist: [f32; 3],
    pub elbow: [f32; 3],
    pub fingers: [[Bone; 4]; 5],
}

impl Default for HandSample {
    fn default() -> HandSample {
        HandSample {
            timestamp_us: 0,
            palm_position: [0.0; 3],
            palm_orientation: IDENTITY,
            wrist: [0.0; 3],
            elbow: [0.0; 3],
            fingers: [[Bone::default(); 4]; 5],
        }
    }
}

#[derive(Default)]
#[allow(dead_code)]
pub struct Frames {
    pub left: Option<HandSample>,
    pub right: Option<HandSample>,
    pub service_connected: bool,
    pub devices: i32,
}

#[cfg(not(feature = "host-stubs"))]
mod live {
    use super::*;
    use crate::log::log;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

pub struct Leap {
    frames: Arc<Mutex<Frames>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// LeapC's clock, microseconds; the same clock as the samples' timestamps.
pub fn now_us() -> i64 {
    // SAFETY: plain call.
    unsafe { sys::LeapGetNow() }
}

impl Leap {
    pub fn start() -> Leap {
        let frames = Arc::new(Mutex::new(Frames::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (frames, stop) = (frames.clone(), stop.clone());
            thread::Builder::new().name("leapfrog-leap".into()).spawn(move || run(frames, stop)).ok()
        };
        if thread.is_none() {
            log!("could not start the Leap thread");
        }
        Leap { frames, stop, thread }
    }

    pub fn snapshot(&self) -> (Option<HandSample>, Option<HandSample>) {
        match self.frames.lock() {
            Ok(f) => (f.left, f.right),
            Err(_) => (None, None),
        }
    }
}

impl Drop for Leap {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A LEAP_CONNECTION with its lifetime tied to the value.
pub struct Connection(pub sys::LEAP_CONNECTION);

impl Connection {
    pub fn open() -> Option<Connection> {
        let mut handle: sys::LEAP_CONNECTION = std::ptr::null_mut();
        // SAFETY: LeapC API as documented; a null config means defaults.
        unsafe {
            if sys::LeapCreateConnection(std::ptr::null(), &mut handle) != sys::_eLeapRS_eLeapRS_Success || handle.is_null() {
                return None;
            }
            if sys::LeapOpenConnection(handle) != sys::_eLeapRS_eLeapRS_Success {
                sys::LeapDestroyConnection(handle);
                return None;
            }
            sys::LeapSetTrackingMode(handle, sys::_eLeapTrackingMode_eLeapTrackingMode_HMD);
            sys::LeapSetPolicyFlags(
                handle,
                (sys::_eLeapPolicyFlag_eLeapPolicyFlag_OptimizeHMD | sys::_eLeapPolicyFlag_eLeapPolicyFlag_BackgroundFrames) as u64,
                0,
            );
        }
        Some(Connection(handle))
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // SAFETY: the handle came from LeapCreateConnection and is dropped once.
        unsafe {
            sys::LeapCloseConnection(self.0);
            sys::LeapDestroyConnection(self.0);
        }
    }
}

enum Next {
    Continue,
    Reopen(&'static str),
}

fn run(frames: Arc<Mutex<Frames>>, stop: Arc<AtomicBool>) {
    let mut was_reopened = false;
    while !stop.load(Ordering::SeqCst) {
        let Some(connection) = Connection::open() else {
            log!("LeapC connection could not be created; retrying in 2 s");
            sleep_unless_stopped(&stop, Duration::from_secs(2));
            continue;
        };
        let reason = poll_loop(&connection, &frames, &stop, &mut was_reopened);
        drop(connection);
        if let Ok(mut f) = frames.lock() {
            *f = Frames::default();
        }
        if let Some(reason) = reason {
            log!("Leap: {reason}; reconnecting");
            sleep_unless_stopped(&stop, Duration::from_millis(500));
        }
    }
}

fn sleep_unless_stopped(stop: &AtomicBool, total: Duration) {
    let until = Instant::now() + total;
    while !stop.load(Ordering::SeqCst) && Instant::now() < until {
        thread::sleep(Duration::from_millis(50));
    }
}

/// Polls one connection until it should be replaced (Some(reason)) or the driver stops (None).
fn poll_loop(connection: &Connection, frames: &Mutex<Frames>, stop: &AtomicBool, was_reopened: &mut bool) -> Option<&'static str> {
    let opened = Instant::now();
    let mut hands_logged = false;
    while !stop.load(Ordering::SeqCst) {
        // SAFETY: a zeroed message is what LeapC expects to fill; pointers inside are valid
        // until the next poll, and we copy everything out before that.
        let mut msg: sys::LEAP_CONNECTION_MESSAGE = unsafe { std::mem::zeroed() };
        let rs = unsafe { sys::LeapPollConnection(connection.0, 100, &mut msg) };
        if rs == sys::_eLeapRS_eLeapRS_Timeout {
            continue;
        }
        if rs != sys::_eLeapRS_eLeapRS_Success {
            // Not connected yet (service down): LeapC keeps trying on its own.
            thread::sleep(Duration::from_millis(100));
            continue;
        }
        let next = match msg.type_ {
            sys::_eLeapEventType_eLeapEventType_Connection => {
                if opened.elapsed() > Duration::from_secs(1) && !*was_reopened {
                    *was_reopened = true;
                    Next::Reopen("the tracking service started after this driver")
                } else {
                    *was_reopened = false;
                    log!("Leap: connected to the tracking service");
                    if let Ok(mut f) = frames.lock() {
                        f.service_connected = true;
                    }
                    Next::Continue
                }
            }
            sys::_eLeapEventType_eLeapEventType_ConnectionLost => Next::Reopen("connection to the tracking service lost"),
            sys::_eLeapEventType_eLeapEventType_Device => {
                log!("Leap: device connected");
                if let Ok(mut f) = frames.lock() {
                    f.devices += 1;
                }
                Next::Continue
            }
            sys::_eLeapEventType_eLeapEventType_DeviceLost => {
                log!("Leap: device lost");
                if let Ok(mut f) = frames.lock() {
                    f.devices -= 1;
                    f.left = None;
                    f.right = None;
                }
                Next::Continue
            }
            sys::_eLeapEventType_eLeapEventType_Tracking => {
                // SAFETY: for a Tracking message the union holds tracking_event, valid until the next poll.
                let event = unsafe { &*msg.__bindgen_anon_1.tracking_event };
                let (left, right) = read_hands(event);
                if !hands_logged && (left.is_some() || right.is_some()) {
                    hands_logged = true;
                    log!("Leap: hands tracked");
                }
                if let Ok(mut f) = frames.lock() {
                    f.left = left;
                    f.right = right;
                }
                Next::Continue
            }
            _ => Next::Continue,
        };
        if let Next::Reopen(reason) = next {
            return Some(reason);
        }
    }
    None
}

fn read_hands(event: &sys::LEAP_TRACKING_EVENT) -> (Option<HandSample>, Option<HandSample>) {
    let (mut left, mut right) = (None, None);
    let timestamp_us = event.info.timestamp;
    if event.pHands.is_null() {
        return (left, right);
    }
    // SAFETY: pHands points at nHands LEAP_HAND structs for the lifetime of the event.
    let hands = unsafe { std::slice::from_raw_parts(event.pHands, event.nHands as usize) };
    for hand in hands {
        let sample = sample_of(hand, timestamp_us);
        if hand.type_ == sys::_eLeapHandType_eLeapHandType_Left {
            left = Some(sample);
        } else {
            right = Some(sample);
        }
    }
    (left, right)
}

fn vec3(v: sys::LEAP_VECTOR) -> [f32; 3] {
    // SAFETY: both union views are three floats.
    unsafe { v.__bindgen_anon_1.v }
}

fn quat(q: sys::LEAP_QUATERNION) -> [f32; 4] {
    // SAFETY: both union views are four floats, x y z w.
    unsafe { q.__bindgen_anon_1.v }
}

fn sample_of(hand: &sys::LEAP_HAND, timestamp_us: i64) -> HandSample {
    // LEAP_HAND is packed: fields are copied out by value, never borrowed.
    let palm = hand.palm;
    let arm = hand.arm;
    // SAFETY: the digits array and the named fingers are two views of the same five digits.
    let digits = unsafe { hand.__bindgen_anon_1.digits };
    let mut fingers = [[Bone::default(); 4]; 5];
    for (f, digit) in digits.iter().enumerate() {
        // SAFETY: same union shape as above, four bones.
        let bones = unsafe { digit.__bindgen_anon_1.bones };
        for (b, bone) in bones.iter().enumerate() {
            fingers[f][b] = Bone { prev_joint: vec3(bone.prev_joint), next_joint: vec3(bone.next_joint), rotation: quat(bone.rotation) };
        }
    }
    HandSample {
        timestamp_us,
        palm_position: vec3(palm.position),
        palm_orientation: quat(palm.orientation),
        wrist: vec3(arm.next_joint),
        elbow: vec3(arm.prev_joint),
        fingers,
    }
}
}

#[cfg(not(feature = "host-stubs"))]
pub use live::{now_us, Leap};
