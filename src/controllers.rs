//! Which Index controller is on. Found through the host's raw poses and properties only (no
//! hooks): a controller of type "knuckles" with a left/right role hint. Its connected flag in the
//! raw poses is the power state SteamVR sees; a Leap hand stands in while its controller is off.

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

    /// Call once per frame with this frame's raw poses; rescans for missing controllers now
    /// and then, since property lookups per device per frame would be wasteful.
    pub fn update(&mut self, poses: &[TrackedDevicePose], own: [Option<u32>; 2]) {
        self.frame = self.frame.wrapping_add(1);
        if self.index.iter().any(Option::is_none) && self.frame % 30 == 1 {
            self.scan(poses, own);
        }
    }

    /// Whether the controller for this hand is powered on (connected), as far as SteamVR knows.
    pub fn connected(&self, hand: u32, poses: &[TrackedDevicePose]) -> bool {
        self.index[hand as usize]
            .and_then(|i| poses.get(i as usize))
            .map(|p| p.device_is_connected)
            .unwrap_or(false)
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
            if self.index[hand].is_none() {
                self.index[hand] = Some(i);
                log!("{} Index controller is device {i}", if hand == 0 { "left" } else { "right" });
            }
        }
    }
}
