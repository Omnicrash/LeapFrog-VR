//! Which Index controller is on. Found through the host's raw poses and properties: a controller
//! of type "knuckles" with a left/right role hint. Its connected flag in the raw poses is the
//! power state SteamVR sees; a Leap hand device stands in while its controller is off.

use crate::host::{self, prop, Properties, TrackedDevicePose};
use crate::log::log;

/// What a device slot turned out to be; a slot's type does not change during a session, so
/// each is asked once.
#[derive(Clone, Copy, PartialEq)]
enum Slot {
    Unknown,
    Other,
    Knuckles(usize), // the hand it is for
}

pub struct Controllers {
    index: [Option<u32>; 2], // device index per hand, once found
    slots: [Slot; host::MAX_TRACKED_DEVICE_COUNT],
    frame: u32,
}

impl Controllers {
    pub fn new() -> Controllers {
        Controllers { index: [None, None], slots: [Slot::Unknown; host::MAX_TRACKED_DEVICE_COUNT], frame: 0 }
    }

    /// Call once per frame with this frame's raw poses; rescans now and then while a hand has
    /// no controller or its controller is off (a second pair switched on gets the slot; the
    /// scan latched on the first pair for good before). Only slots never classified cost a
    /// property lookup; the rest is decided from the raw poses.
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
        for i in 0..poses.len().min(self.slots.len()) {
            let id = i as u32;
            if own.contains(&Some(id)) || self.index.contains(&Some(id)) {
                continue;
            }
            if self.slots[i] == Slot::Unknown {
                self.slots[i] = Self::classify(id);
            }
            let Slot::Knuckles(hand) = self.slots[i] else { continue };
            // Take it when the hand has none, or when its own is off and this one is on.
            let connected = Self::on(Some(id), poses);
            if self.index[hand].is_none() || (connected && !Self::on(self.index[hand], poses)) {
                self.index[hand] = Some(id);
                log!("{} Index controller is device {id}{}", if hand == 0 { "left" } else { "right" }, if connected { "" } else { " (off)" });
            }
        }
    }

    /// Asks the host what a slot holds; a slot with no type yet stays unknown and is asked again.
    fn classify(id: u32) -> Slot {
        let props = Properties::of(id);
        let Some(kind) = props.string(prop::CONTROLLER_TYPE) else { return Slot::Unknown };
        if kind != "knuckles" {
            return Slot::Other;
        }
        match props.int32(prop::CONTROLLER_ROLE_HINT) {
            Some(host::ROLE_LEFT_HAND) => Slot::Knuckles(0),
            Some(host::ROLE_RIGHT_HAND) => Slot::Knuckles(1),
            _ => Slot::Other,
        }
    }
}
