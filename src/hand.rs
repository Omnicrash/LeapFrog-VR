//! One Leap hand as a SteamVR device: pose + Full skeleton, no buttons. This iteration only
//! registers the device and keeps it reported disconnected; poses and bones come next.

use crate::host::{self, prop, DriverPose, Properties};
use crate::log::log;

pub struct Hand {
    pub index: u32,
    pub object_id: Option<u32>,
    pub skeleton: Option<u64>,
    pub pose: DriverPose,
}

impl Hand {
    pub fn new(index: u32) -> Hand {
        Hand { index, object_id: None, skeleton: None, pose: DriverPose::disconnected() }
    }

    pub fn is_left(&self) -> bool {
        self.index == host::HAND_LEFT
    }

    pub fn side(&self) -> &'static str {
        if self.is_left() { "left" } else { "right" }
    }

    pub fn serial(&self) -> String {
        format!("LeapFrog_{}", if self.is_left() { "Left" } else { "Right" })
    }

    /// vrserver gives the device its index; properties and components are set up here.
    pub fn activate(&mut self, object_id: u32) -> i32 {
        self.object_id = Some(object_id);
        let props = Properties::of(object_id);
        props
            .set_string(prop::MANUFACTURER_NAME, "Ultraleap")
            .set_string(prop::TRACKING_SYSTEM_NAME, "leapfrog")
            .set_string(prop::SERIAL_NUMBER, &self.serial())
            .set_string(prop::MODEL_NUMBER, "LeapFrog Hand")
            .set_int32(prop::DEVICE_CLASS, host::DEVICE_CLASS_CONTROLLER)
            .set_int32(prop::CONTROLLER_ROLE_HINT, if self.is_left() { host::ROLE_LEFT_HAND } else { host::ROLE_RIGHT_HAND })
            // Fixed, as plain Leapify had it: the Leap hand wins the slot while it is connected.
            .set_int32(prop::CONTROLLER_HAND_SELECTION_PRIORITY, 999)
            .set_bool(prop::WILL_DRIFT_IN_YAW, false)
            .set_bool(prop::DEVICE_IS_WIRELESS, true)
            .set_bool(prop::DEVICE_PROVIDES_BATTERY_STATUS, false)
            .set_bool(prop::IDENTIFIABLE, false)
            .set_string(prop::RESOURCE_ROOT, "leapfrog")
            .set_string(prop::INPUT_PROFILE_PATH, "{leapfrog}/input/leapfrog_hand_profile.json")
            // Virtual Desktop's hand type: pose + skeleton only, and VRChat ships a binding for it.
            .set_string(prop::CONTROLLER_TYPE, "vd_hand_controller");

        let side = self.side();
        self.skeleton = props.create_skeleton(
            &format!("/input/skeleton/{side}"),
            &format!("/skeleton/hand/{side}"),
            "/pose/raw",
        );
        match self.skeleton {
            Some(_) => log!("{} hand activated as device {object_id}", side),
            None => log!("{} hand activated as device {object_id}, but the skeleton component failed", side),
        }
        host::INIT_ERROR_NONE
    }

    pub fn deactivate(&mut self) {
        log!("{} hand deactivated", self.side());
        self.object_id = None;
        self.skeleton = None;
    }

    /// Sends the current pose; called once per RunFrame.
    pub fn publish(&self) {
        if let Some(id) = self.object_id {
            host::pose_updated(id, &self.pose);
        }
    }
}
