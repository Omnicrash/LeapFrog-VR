//! Which Index controller is on. Found through the host's raw poses and properties: a controller
//! of type "knuckles" with a left/right role hint. Its connected flag in the raw poses is the
//! power state SteamVR sees; a Leap hand device stands in while its controller is off.

use crate::host::{self, prop, Properties, TrackedDevicePose};
use crate::log::log;

pub struct Controllers {
    index: [Option<u32>; 2], // device index per hand, once found
    frame: u32,
}

impl Controllers {
    pub fn new() -> Controllers {
        Controllers { index: [None, None], frame: 0 }
    }

    /// Call once per frame with this frame's raw poses; rescans now and then while a hand has
    /// no controller or its controller is off (a second pair switched on gets the slot; the
    /// scan latched on the first pair for good before), since property lookups per device per
    /// frame would be wasteful.
    pub fn update(&mut self, poses: &[TrackedDevicePose], own: [Option<u32>; 2]) {
        self.frame = self.frame.wrapping_add(1);
        if self.frame % 30 == 1 && self.index.iter().any(|i| !Self::on(*i, poses)) {
            self.scan(poses, own);
        }
    }

    fn on(index: Option<u32>, poses: &[TrackedDevicePose]) -> bool {
        index.and_then(|i| poses.get(i as usize)).is_some_and(|p| p.device_is_connected)
    }

    /// This hand's controller's device index, once found.
    pub fn index(&self, hand: u32) -> Option<u32> {
        self.index[hand as usize]
    }

    /// This hand's controller in this frame's raw poses, once found.
    pub fn pose<'a>(&self, hand: u32, poses: &'a [TrackedDevicePose]) -> Option<&'a TrackedDevicePose> {
        self.index[hand as usize].and_then(|i| poses.get(i as usize))
    }

    fn scan(&mut self, poses: &[TrackedDevicePose], own: [Option<u32>; 2]) {
        for (i, _) in poses.iter().enumerate() {
            let i = i as u32;
            if own.contains(&Some(i)) || self.index.contains(&Some(i)) {
                continue;
            }
            let props = Properties::of(i);
            let Some(kind) = props.string(prop::CONTROLLER_TYPE) else { continue };
            if kind != "knuckles" {
                continue;
            }
            let hand = match props.int32(prop::CONTROLLER_ROLE_HINT) {
                Some(host::ROLE_LEFT_HAND) => 0,
                Some(host::ROLE_RIGHT_HAND) => 1,
                _ => continue,
            };
            // Take it when the hand has none, or when its own is off and this one is on.
            let connected = poses.get(i as usize).is_some_and(|p| p.device_is_connected);
            if self.index[hand].is_none() || (connected && !Self::on(self.index[hand], poses)) {
                self.index[hand] = Some(i);
                log!("{} Index controller is device {i}{}", if hand == 0 { "left" } else { "right" }, if connected { "" } else { " (off)" });
            }
        }
    }
}
