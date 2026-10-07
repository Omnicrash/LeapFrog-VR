//! One Leap hand as a SteamVR device: pose + Full skeleton, no buttons.

use glam::Vec3;

use crate::host::{self, prop, BoneTransform, DriverPose, Properties, Quaternion, TrackedDevicePose};
use crate::leap::HandSample;
use crate::log::log;
use crate::math::{self, HandFrame, Head, Offsets, BONE_COUNT};

pub struct Hand {
    pub index: u32,
    pub object_id: Option<u32>,
    skeleton: Option<u64>,
    pose: DriverPose,
    bones: [BoneTransform; BONE_COUNT],
    candidate: Option<DriverPose>,
    was_connected: bool,
}

impl Hand {
    pub fn new(index: u32) -> Hand {
        Hand {
            index,
            object_id: None,
            skeleton: None,
            pose: DriverPose::disconnected(),
            bones: math::open_hand(index == host::HAND_LEFT),
            candidate: None,
            was_connected: false,
        }
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

    pub fn pose(&self) -> DriverPose {
        self.pose
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
            .set_string(prop::RESOURCE_ROOT, "leapfrog");
        // Which controller type the hand reports. vd_hand_controller (Virtual Desktop's hands):
        // pose + skeleton only, VRChat ships a binding for it. knuckles (the Index controller's
        // own type, Leapify's choice): an experiment, 2026-10-07. SteamVR makes every app reload
        // its bindings when a hand role changes device, and counts those loads per app for the
        // whole session (about 250, then no more reloads: VRChat's hands go limp). If a swap
        // between two devices of the same type does not count, switching becomes free.
        match host::setting_string("controllerType", "vd_hand_controller").as_str() {
            "knuckles" => {
                props
                    .set_string(prop::CONTROLLER_TYPE, "knuckles")
                    .set_string(prop::INPUT_PROFILE_PATH, "{indexcontroller}/input/index_controller_profile.json");
                log!("{} hand reports as knuckles (controllerType experiment)", self.side());
            }
            _ => {
                props
                    .set_string(prop::CONTROLLER_TYPE, "vd_hand_controller")
                    .set_string(prop::INPUT_PROFILE_PATH, "{leapfrog}/input/leapfrog_hand_profile.json");
            }
        }

        // Render model (handmodel/: the status icon's outline in 3D) and Leapify's status icons (MIT).
        let side = self.side();
        props.set_string(prop::RENDER_MODEL_NAME, &format!("{{leapfrog}}leapfrog_hand_{side}"));
        let icons = [
            (prop::ICON_OFF, "off"),
            (prop::ICON_SEARCHING, "standby"),
            (prop::ICON_SEARCHING_ALERT, "standby_alert"),
            (prop::ICON_READY, "ready"),
            (prop::ICON_READY_ALERT, "ready_alert"),
            (prop::ICON_NOT_READY, "error"),
            (prop::ICON_STANDBY, "standby"),
            (prop::ICON_ALERT_LOW, "ready_alert"),
            (prop::ICON_STANDBY_ALERT, "standby_alert"),
        ];
        for (prop, name) in icons {
            props.set_string(prop, &format!("{{leapfrog}}/icons/{side}_hand_status_{name}.png"));
        }

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

    /// Step one of a frame: where the Leap sees this hand, in world space, with the pose and
    /// bones computed and held back until `apply` decides whether this device drives the hand.
    pub fn locate(&mut self, sample: Option<&HandSample>, head: Option<&TrackedDevicePose>, offsets: &Offsets) -> Option<Vec3> {
        self.candidate = None;
        let (sample, head) = (sample?, head?);
        if !head.pose_is_valid {
            return None;
        }
        let frame = HandFrame::new(sample, self.is_left());
        let (world_rotation, world_translation, rotation, position) = frame.pose(&Head::from_pose(head), offsets);
        let mut pose = DriverPose::disconnected();
        pose.device_is_connected = true;
        pose.world_from_driver_rotation = quat(world_rotation);
        pose.world_from_driver_translation = vec(world_translation);
        pose.rotation = quat(rotation);
        pose.position = vec(position);
        pose.pose_is_valid = true;
        pose.result = host::TRACKING_RESULT_RUNNING_OK;
        frame.skeleton(&mut self.bones);
        self.candidate = Some(pose);
        Some(world_rotation * position + world_translation)
    }

    /// Step two: connected while this device drives the hand; the located pose if there is one,
    /// out of range otherwise. Returns whether this frame is a transition (connect or disconnect).
    pub fn apply(&mut self, connected: bool) -> bool {
        let transition = connected != self.was_connected;
        if transition {
            self.was_connected = connected;
            log!("{} hand {}", self.side(), if connected { "takes over" } else { "steps back" });
        }
        if !connected {
            self.pose = DriverPose::disconnected();
            return transition;
        }
        match self.candidate {
            Some(pose) => {
                self.pose = pose;
                if let Some(handle) = self.skeleton {
                    host::update_skeleton(handle, host::SKELETAL_MOTION_RANGE_WITHOUT_CONTROLLER, &self.bones);
                    host::update_skeleton(handle, host::SKELETAL_MOTION_RANGE_WITH_CONTROLLER, &self.bones);
                }
            }
            None => {
                let mut pose = DriverPose::disconnected();
                pose.device_is_connected = true;
                pose.result = host::TRACKING_RESULT_OUT_OF_RANGE;
                self.pose = pose;
            }
        }
        transition
    }

    /// Sends the current pose; called once per RunFrame.
    pub fn publish(&self) {
        if let Some(id) = self.object_id {
            host::pose_updated(id, &self.pose);
        }
    }
}

fn quat(q: glam::Quat) -> Quaternion {
    Quaternion { w: q.w as f64, x: q.x as f64, y: q.y as f64, z: q.z as f64 }
}

fn vec(v: Vec3) -> [f64; 3] {
    [v.x as f64, v.y as f64, v.z as f64]
}
