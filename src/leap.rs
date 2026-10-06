//! The LeapC connection on its own thread. Publishes the newest sample per hand; the rules for
//! coming up next to a tracking service that may start later, or restart, are from
//! `doc/Background.md`:
//! - a connection opened before the service ran comes up once the service does but never gets
//!   hands: when the connection event arrives late (> 1 s after opening), open a fresh one;
//! - a lost connection (service stopped or restarted) gets a fresh connection too.
//!
//! The connection is multi-device aware and subscribes to each camera, which is what the service
//! requires before it accepts tracking hints (`trackingHint`, a comma-separated set; default
//! `ultra_performance_mode,hand_on_object`; unknown hints are ignored by the service).

#[cfg(not(feature = "host-stubs"))]
use leap_sys as sys;

/// No LeapC on the build host: `cargo test` gets a clock and nothing else.
#[cfg(feature = "host-stubs")]
pub fn now_us() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_micros() as i64).unwrap_or(0)
}

#[derive(Clone, Copy, Debug)]
#[allow(dead_code)] // next_joint and width: kept from LeapC for later use
pub struct Bone {
    pub prev_joint: [f32; 3],
    pub next_joint: [f32; 3],
    pub rotation: [f32; 4], // x, y, z, w
    pub width: f32,         // mm
}

const IDENTITY: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

impl Default for Bone {
    fn default() -> Bone {
        Bone { prev_joint: [0.0; 3], next_joint: [0.0; 3], rotation: IDENTITY, width: 15.0 }
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

/// What the thread needs from the settings.
#[derive(Clone, Debug)]
pub struct Config {
    pub manage_service: bool,
    pub hint: Option<String>,
}

// Not in leap-sys 0.2 (its header predates it); Gemini 5.x exports it. `hints` is a
// null-terminated array of C strings.
extern "C" {
    fn LeapSetDeviceHints(connection: sys::LEAP_CONNECTION, device: sys::LEAP_DEVICE, hints: *const *const std::ffi::c_char) -> sys::eLeapRS;
}

/// LeapC's clock, microseconds; the same clock as the samples' timestamps.
pub fn now_us() -> i64 {
    // SAFETY: plain call.
    unsafe { sys::LeapGetNow() }
}

impl Leap {
    pub fn start(config: Config) -> Leap {
        let frames = Arc::new(Mutex::new(Frames::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (frames, stop) = (frames.clone(), stop.clone());
            thread::Builder::new().name("leapfrog-leap".into()).spawn(move || run(frames, stop, config)).ok()
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
pub struct Connection(pub sys::LEAP_CONNECTION, Vec<sys::LEAP_DEVICE>);

impl Connection {
    pub fn open() -> Option<Connection> {
        let mut handle: sys::LEAP_CONNECTION = std::ptr::null_mut();
        let config = sys::LEAP_CONNECTION_CONFIG {
            size: std::mem::size_of::<sys::LEAP_CONNECTION_CONFIG>() as u32,
            flags: sys::_eLeapConnectionConfig_eLeapConnectionConfig_MultiDeviceAware as u32,
            server_namespace: std::ptr::null(),
        };
        // SAFETY: LeapC API as documented.
        unsafe {
            if sys::LeapCreateConnection(&config, &mut handle) != sys::_eLeapRS_eLeapRS_Success || handle.is_null() {
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
        Some(Connection(handle, Vec::new()))
    }

    /// Opens and subscribes to every camera the service lists, and hands it the hint. Called
    /// after each Device event; a service that restarted lists its camera without announcing it.
    pub fn subscribe_all(&mut self, hint: Option<&str>) {
        let mut refs: [sys::LEAP_DEVICE_REF; 4] = [sys::LEAP_DEVICE_REF { handle: std::ptr::null_mut(), id: 0 }; 4];
        let mut count = refs.len() as u32;
        // SAFETY: the array and its length are passed together.
        if unsafe { sys::LeapGetDeviceList(self.0, refs.as_mut_ptr(), &mut count) } != sys::_eLeapRS_eLeapRS_Success {
            return;
        }
        // The hint setting is a comma-separated set; one C string each, null-terminated array.
        let hint_cs: Vec<std::ffi::CString> = hint
            .map(|h| h.split(',').map(str::trim).filter(|h| !h.is_empty()).filter_map(|h| std::ffi::CString::new(h).ok()).collect())
            .unwrap_or_default();
        for r in refs.iter().take(count.min(4) as usize) {
            let mut device: sys::LEAP_DEVICE = std::ptr::null_mut();
            // SAFETY: LeapC API; the device handle is kept until the connection is dropped.
            unsafe {
                if sys::LeapOpenDevice(*r, &mut device) != sys::_eLeapRS_eLeapRS_Success || device.is_null() {
                    continue;
                }
                let subscribed = sys::LeapSubscribeEvents(self.0, device);
                let hinted = if hint_cs.is_empty() {
                    sys::_eLeapRS_eLeapRS_Success
                } else {
                    let mut hints: Vec<*const std::ffi::c_char> = hint_cs.iter().map(|h| h.as_ptr()).collect();
                    hints.push(std::ptr::null());
                    LeapSetDeviceHints(self.0, device, hints.as_ptr())
                };
                let id = r.id; // copied out: LEAP_DEVICE_REF is packed
                log!("Leap: device {id} subscribed ({:#x}), hints {} ({:#x})", subscribed as u32, hint.unwrap_or("none"), hinted as u32);
            }
            self.1.push(device);
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // SAFETY: the handles came from LeapOpenDevice / LeapCreateConnection and are dropped once.
        unsafe {
            for device in self.1.drain(..) {
                sys::LeapCloseDevice(device);
            }
            sys::LeapCloseConnection(self.0);
            sys::LeapDestroyConnection(self.0);
        }
    }
}

enum Next {
    Continue,
    Reopen(&'static str),
}

fn run(frames: Arc<Mutex<Frames>>, stop: Arc<AtomicBool>, config: Config) {
    if config.manage_service {
        crate::service::start();
    }
    let mut was_reopened = false;
    while !stop.load(Ordering::SeqCst) {
        let Some(mut connection) = Connection::open() else {
            log!("LeapC connection could not be created; retrying in 2 s");
            sleep_unless_stopped(&stop, Duration::from_secs(2));
            continue;
        };
        let reason = poll_loop(&mut connection, &frames, &stop, &mut was_reopened, config.hint.as_deref());
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
fn poll_loop(connection: &mut Connection, frames: &Mutex<Frames>, stop: &AtomicBool, was_reopened: &mut bool, hint: Option<&str>) -> Option<&'static str> {
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
                connection.subscribe_all(hint);
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
            let width = bone.width; // packed field, copied
            fingers[f][b] = Bone { prev_joint: vec3(bone.prev_joint), next_joint: vec3(bone.next_joint), rotation: quat(bone.rotation), width };
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
pub use live::{now_us, Config, Leap};
