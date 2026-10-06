//! Leap -> SteamVR math: hand pose and the 31-bone skeleton. Ported from Leapify
//! (Nyabsi/driver_leapify, MIT, `TrackedController.cpp` and `Math.hpp`), glm replaced by glam.
//! Leap frames arrive in millimeters in the Leap's HMD-mode basis; SteamVR wants meters in the
//! head's frame (+X right, +Y up, -Z forward) with a rotation per bone relative to its parent.

use glam::{Mat3, Mat4, Quat, Vec3, Vec4};

use crate::host::{BoneTransform, Quaternionf, TrackedDevicePose};
use crate::leap::HandSample;

pub const BONE_COUNT: usize = 31;

// Bone indices of SteamVR's hand skeleton.
pub const ROOT: usize = 0;
pub const WRIST: usize = 1;
const FINGER_FIRST_BONE: [usize; 5] = [2, 6, 11, 16, 21]; // thumb, index, middle, ring, pinky
const AUX_FIRST: usize = 26;

const PI: f32 = std::f32::consts::PI;
const PI_HALF: f32 = PI * 0.5;

/// The open hand SteamVR's Unity package ships (skeleton blending disabled), left hand; the right
/// hand is its mirror (`mirror_for_right`). Bones a frame doesn't drive keep these values.
const OPEN_HAND: [([f32; 4], [f32; 4]); BONE_COUNT] = [
    ([0.000000, 0.000000, 0.000000, 1.0], [1.000000, -0.000000, -0.000000, 0.000000]),
    ([-0.0445230342, 0.0301547553, 0.16438961, 1.0], [1.50656376e-07, -1.77612698e-08, 0.927827835, -0.373008907]),
    ([-0.012083, 0.028070, 0.025050, 1.0], [0.464112, 0.567418, 0.272106, 0.623374]),
    ([0.040406, 0.000000, -0.000000, 1.0], [0.994838, 0.082939, 0.019454, 0.055130]),
    ([0.032517, 0.000000, 0.000000, 1.0], [0.974793, -0.003213, 0.021867, -0.222015]),
    ([0.030464, -0.000000, -0.000000, 1.0], [1.000000, -0.000000, -0.000000, 0.000000]),
    ([0.000632, 0.026866, 0.015002, 1.0], [0.644251, 0.421979, -0.478202, 0.422133]),
    ([0.074204, -0.005002, 0.000234, 1.0], [0.995332, 0.007007, -0.039124, 0.087949]),
    ([0.043930, -0.000000, -0.000000, 1.0], [0.997891, 0.045808, 0.002142, -0.045943]),
    ([0.028695, 0.000000, 0.000000, 1.0], [0.999649, 0.001850, -0.022782, -0.013409]),
    ([0.022821, 0.000000, -0.000000, 1.0], [1.000000, -0.000000, 0.000000, -0.000000]),
    ([0.002177, 0.007120, 0.016319, 1.0], [0.546723, 0.541276, -0.442520, 0.460749]),
    ([0.070953, 0.000779, 0.000997, 1.0], [0.980294, -0.167261, -0.078959, 0.069368]),
    ([0.043108, 0.000000, 0.000000, 1.0], [0.997947, 0.018493, 0.013192, 0.059886]),
    ([0.033266, 0.000000, 0.000000, 1.0], [0.997394, -0.003328, -0.028225, -0.066315]),
    ([0.025892, -0.000000, 0.000000, 1.0], [0.999195, -0.000000, 0.000000, 0.040126]),
    ([0.000513, -0.006545, 0.016348, 1.0], [0.516692, 0.550143, -0.495548, 0.429888]),
    ([0.065876, 0.001786, 0.000693, 1.0], [0.990420, -0.058696, -0.101820, 0.072495]),
    ([0.040697, 0.000000, 0.000000, 1.0], [0.999545, -0.002240, 0.000004, 0.030081]),
    ([0.028747, -0.000000, -0.000000, 1.0], [0.999102, -0.000721, -0.012693, 0.040420]),
    ([0.022430, -0.000000, 0.000000, 1.0], [1.000000, 0.000000, 0.000000, 0.000000]),
    ([-0.002478, -0.018981, 0.015214, 1.0], [0.526918, 0.523940, -0.584025, 0.326740]),
    ([0.062878, 0.002844, 0.000332, 1.0], [0.986609, -0.059615, -0.135163, 0.069132]),
    ([0.030220, 0.000000, 0.000000, 1.0], [0.994317, 0.001896, -0.000132, 0.106446]),
    ([0.018187, 0.000000, 0.000000, 1.0], [0.995931, -0.002010, -0.052079, -0.073526]),
    ([0.018018, 0.000000, -0.000000, 1.0], [1.000000, 0.000000, 0.000000, 0.000000]),
    ([-0.006059, 0.056285, 0.060064, 1.0], [0.737238, 0.202745, 0.594267, 0.249441]),
    ([-0.040416, -0.043018, 0.019345, 1.0], [-0.290331, 0.623527, -0.663809, -0.293734]),
    ([-0.039354, -0.075674, 0.047048, 1.0], [-0.187047, 0.678062, -0.659285, -0.265683]),
    ([-0.038340, -0.090987, 0.082579, 1.0], [-0.183037, 0.736793, -0.634757, -0.143936]),
    ([-0.031806, -0.087214, 0.121015, 1.0], [-0.003659, 0.758407, -0.639342, -0.126678]),
];

pub fn open_hand(left: bool) -> [BoneTransform; BONE_COUNT] {
    let mut bones = [BoneTransform::default(); BONE_COUNT];
    for (bone, (position, [w, x, y, z])) in bones.iter_mut().zip(OPEN_HAND) {
        bone.position = position;
        bone.orientation = Quaternionf { w, x, y, z };
    }
    if !left {
        mirror_for_right(&mut bones);
    }
    bones
}

/// Leapify's mirroring of the left template across the YZ plane.
fn mirror_for_right(bones: &mut [BoneTransform; BONE_COUNT]) {
    for (i, bone) in bones.iter_mut().enumerate().skip(1) {
        bone.position[0] = -bone.position[0];
        let q = &mut bone.orientation;
        if i == WRIST {
            q.y = -q.y;
            q.z = -q.z;
        } else if FINGER_FIRST_BONE.contains(&i) {
            q.z = -q.z;
            std::mem::swap(&mut q.x, &mut q.w);
            q.w = -q.w;
            std::mem::swap(&mut q.y, &mut q.z);
        }
    }
}

/// Leap millimeters to SteamVR meters: the Leap's HMD basis is SteamVR's rotated.
fn position(v: [f32; 3]) -> Vec3 {
    Vec3::new(-0.001 * v[0], -0.001 * v[2], -0.001 * v[1])
}

fn rotation(q: [f32; 4]) -> Quat {
    Quat::from_xyzw(0.0, 0.70106769, -0.70106769, 0.0) * Quat::from_xyzw(q[0], q[1], q[2], q[3])
}

/// glm::quat(vec3 euler), ported literally so the constants below mean what they meant there.
fn euler_glm(e: Vec3) -> Quat {
    let (cx, cy, cz) = ((e.x * 0.5).cos(), (e.y * 0.5).cos(), (e.z * 0.5).cos());
    let (sx, sy, sz) = ((e.x * 0.5).sin(), (e.y * 0.5).sin(), (e.z * 0.5).sin());
    Quat::from_xyzw(
        sx * cy * cz - cx * sy * sz,
        cx * sy * cz + sx * cy * sz,
        cx * cy * sz - sx * sy * cz,
        cx * cy * cz + sx * sy * sz,
    )
}

fn to_quatf(q: Quat) -> Quaternionf {
    Quaternionf { w: q.w, x: q.x, y: q.y, z: q.z }
}

fn to_vec4(v: Vec3) -> [f32; 4] {
    [v.x, v.y, v.z, 1.0]
}

fn from_quatf(q: Quaternionf) -> Quat {
    Quat::from_xyzw(q.x, q.y, q.z, q.w)
}

fn from_vec4(v: [f32; 4]) -> Vec3 {
    Vec3::new(v[0], v[1], v[2])
}

fn trs(t: Vec3, r: Quat) -> Mat4 {
    Mat4::from_translation(t) * Mat4::from_quat(r)
}

/// Offsets from the settings file: hand position in meters (head frame, both hands alike),
/// rotation in degrees, and an inward shift per hand (meters, toward the other hand).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Offsets {
    pub position: Vec3,
    pub rotation_degrees: Vec3,
    pub inward: f32,
}

/// The HMD's raw pose as the driver's world transform.
pub struct Head {
    pub rotation: Quat,
    pub translation: Vec3,
}

impl Head {
    pub fn from_pose(pose: &TrackedDevicePose) -> Head {
        let m = &pose.device_to_absolute_tracking;
        let col = |c: usize| Vec3::new(m[0][c], m[1][c], m[2][c]);
        Head {
            rotation: Quat::from_mat3(&Mat3::from_cols(col(0), col(1), col(2))).normalize(),
            translation: col(3),
        }
    }
}

/// One hand's frame, converted to SteamVR's basis.
pub struct HandFrame {
    left: bool,
    wrist: Vec3,
    palm_position: Vec3,
    palm_rotation: Quat,
    bone_positions: [Vec3; 20],
    bone_rotations: [Quat; 20],
    bone_ends: [Vec3; 20],
    bone_widths: [f32; 20],
}

impl HandFrame {
    pub fn new(sample: &HandSample, left: bool) -> HandFrame {
        let mut bone_positions = [Vec3::ZERO; 20];
        let mut bone_rotations = [Quat::IDENTITY; 20];
        let mut bone_ends = [Vec3::ZERO; 20];
        let mut bone_widths = [0.0f32; 20];
        for (f, finger) in sample.fingers.iter().enumerate() {
            for (b, bone) in finger.iter().enumerate() {
                bone_positions[f * 4 + b] = position(bone.prev_joint);
                bone_rotations[f * 4 + b] = rotation(bone.rotation);
                bone_ends[f * 4 + b] = position(bone.next_joint);
                bone_widths[f * 4 + b] = bone.width * 0.001;
            }
        }
        HandFrame {
            left,
            wrist: position(sample.wrist),
            palm_position: position(sample.palm_position),
            palm_rotation: rotation(sample.palm_orientation).normalize(),
            bone_positions,
            bone_rotations,
            bone_ends,
            bone_widths,
        }
    }

    /// Every bone as (start, end, width) in the pose's own frame: origin at the wrist, axes of
    /// the palm rotation. What the render model writer draws.
    pub fn bones_in_pose_frame(&self) -> Vec<(Vec3, Vec3, f32)> {
        let inv = self.palm_rotation.inverse();
        let local = |p: Vec3| inv * (p - self.wrist);
        (0..20)
            .filter(|&i| (self.bone_ends[i] - self.bone_positions[i]).length() > 0.002) // the thumb's metacarpal has no length
            .map(|i| (local(self.bone_positions[i]), local(self.bone_ends[i]), self.bone_widths[i]))
            .collect()
    }

    /// World-from-driver = the head; the hand sits in the head's frame.
    pub fn pose(&self, head: &Head, offsets: &Offsets) -> (Quat, Vec3, Quat, Vec3) {
        let world_from_driver = head.rotation * euler_glm(offsets.rotation_degrees * (PI / 180.0));
        // Inward is toward the body's midline: +X (right) for the left hand, -X for the right.
        let inward = Vec3::X * (if self.left { offsets.inward } else { -offsets.inward });
        (world_from_driver, head.translation, self.palm_rotation, self.wrist + offsets.position + inward)
    }

    /// Fills the driven bones of `bones` (an `open_hand` template for this side).
    pub fn skeleton(&self, bones: &mut [BoneTransform; BONE_COUNT]) {
        let root_rotation = self.palm_rotation;
        let wrist_position = root_rotation.inverse() * (self.wrist - self.palm_position);
        let mut wrist_rotation = root_rotation.inverse() * self.palm_rotation;
        wrist_rotation = (Quat::from_axis_angle(Vec3::Z, if self.left { -PI_HALF } else { PI_HALF })
            * Quat::from_axis_angle(Vec3::Y, PI))
            * wrist_rotation;

        bones[ROOT] = BoneTransform { position: [0.0, 0.0, 0.0, 1.0], orientation: Quaternionf { w: 1.0, x: 0.0, y: 0.0, z: 0.0 } };
        bones[WRIST] = BoneTransform { position: to_vec4(wrist_position), orientation: to_quatf(wrist_rotation) };

        for finger in 0..5 {
            let first = FINGER_FIRST_BONE[finger];
            let thumb = finger == 0;
            let count = if thumb { 3 } else { 4 };
            for j in 0..count {
                // The thumb has no metacarpal in SteamVR's skeleton: its chain starts one bone later.
                let bone = if thumb { j + 1 } else { j };
                let mut r = self.local_rotation(finger, bone, thumb);
                self.change_orientation(&mut r);
                if j == 0 {
                    self.fix_metacarpal(&mut r);
                }
                bones[first + j].orientation = to_quatf(r);
                if j > 0 {
                    let mut p = self.local_position(finger, bone, thumb);
                    self.change_position(&mut p);
                    bones[first + j].position = to_vec4(p);
                }
            }
        }

        // Aux bones: each fingertip in the wrist's frame, from the chain of local transforms.
        let wrist = trs(from_vec4(bones[WRIST].position), from_quatf(bones[WRIST].orientation));
        for finger in 0..5 {
            let mut chain = wrist;
            let first = FINGER_FIRST_BONE[finger];
            for j in 0..(if finger == 0 { 3 } else { 4 }) {
                chain *= trs(from_vec4(bones[first + j].position), from_quatf(bones[first + j].orientation));
            }
            let mut p = (chain * Vec4::new(0.0, 0.0, 0.0, 1.0)).truncate();
            let mut r = Quat::from_mat4(&chain);
            if self.left {
                p.y = -p.y;
                p.z = -p.z;
                let (x, y, z, w) = (r.x, r.y, r.z, r.w);
                r = Quat::from_xyzw(w, -z, y, -x);
            }
            bones[AUX_FIRST + finger] = BoneTransform { position: to_vec4(p), orientation: to_quatf(r) };
        }
    }

    fn parent(&self, finger: usize, bone: usize, ignore_meta: bool) -> (Vec3, Quat) {
        if bone == 0 || (ignore_meta && bone == 1) {
            (self.wrist, self.palm_rotation)
        } else {
            let i = finger * 4 + bone - 1;
            (self.bone_positions[i], self.bone_rotations[i])
        }
    }

    fn local_rotation(&self, finger: usize, bone: usize, ignore_meta: bool) -> Quat {
        let (_, parent) = self.parent(finger, bone, ignore_meta);
        parent.inverse() * self.bone_rotations[finger * 4 + bone]
    }

    fn local_position(&self, finger: usize, bone: usize, ignore_meta: bool) -> Vec3 {
        let (parent_position, parent_rotation) = self.parent(finger, bone, ignore_meta);
        let i = finger * 4 + bone;
        let local = trs(parent_position, parent_rotation).inverse() * trs(self.bone_positions[i], self.bone_rotations[i]);
        (local * Vec4::new(0.0, 0.0, 0.0, 1.0)).truncate()
    }

    fn change_orientation(&self, r: &mut Quat) {
        let (x, y, z, w) = (r.x, r.y, r.z, r.w);
        *r = if self.left { Quat::from_xyzw(-z, -y, -x, w) } else { Quat::from_xyzw(z, y, -x, w) };
    }

    fn change_position(&self, p: &mut Vec3) {
        let (x, z) = (p.x, p.z);
        p.x = if self.left { -z } else { z };
        p.z = -x;
    }

    fn fix_metacarpal(&self, r: &mut Quat) {
        *r = euler_glm(Vec3::new(-PI_HALF, PI_HALF, 0.0)) * *r;
        if self.left {
            *r = euler_glm(Vec3::new(PI, 0.0, 0.0)) * *r;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leap_basis_maps_to_steamvr() {
        // Leap HMD mode: +X left, +Y forward (away from the face), +Z down, in mm.
        let forward = position([0.0, 300.0, 0.0]);
        assert!((forward - Vec3::new(0.0, 0.0, -0.3)).length() < 1e-6, "forward is -Z: {forward}");
        let left = position([100.0, 0.0, 0.0]);
        assert!((left - Vec3::new(-0.1, 0.0, 0.0)).length() < 1e-6, "Leap +X is SteamVR -X: {left}");
        let down = position([0.0, 0.0, 50.0]);
        assert!((down - Vec3::new(0.0, -0.05, 0.0)).length() < 1e-6, "Leap +Z is SteamVR -Y: {down}");
    }

    #[test]
    fn euler_matches_glm_for_axis_rotations() {
        let q = euler_glm(Vec3::new(PI_HALF, 0.0, 0.0));
        let r = Quat::from_axis_angle(Vec3::X, PI_HALF);
        assert!(q.abs_diff_eq(r, 1e-6));
        let q = euler_glm(Vec3::new(0.0, 0.0, PI_HALF));
        let r = Quat::from_axis_angle(Vec3::Z, PI_HALF);
        assert!(q.abs_diff_eq(r, 1e-6));
    }

    #[test]
    fn right_template_mirrors_x() {
        let (l, r) = (open_hand(true), open_hand(false));
        assert_eq!(l[ROOT].position, r[ROOT].position);
        for i in 1..BONE_COUNT {
            assert_eq!(l[i].position[0], -r[i].position[0]);
            assert_eq!(l[i].position[1], r[i].position[1]);
        }
    }

    #[test]
    fn skeleton_leaves_undriven_bones_from_template() {
        let sample = HandSample::default();
        let frame = HandFrame::new(&sample, true);
        let mut bones = open_hand(true);
        frame.skeleton(&mut bones);
        let template = open_hand(true);
        // Last bone of every finger (no effect) keeps the template.
        for i in [5usize, 10, 15, 20, 25] {
            assert_eq!(bones[i].position, template[i].position);
        }
        for bone in &bones {
            assert!(bone.position.iter().all(|v| v.is_finite()));
        }
    }
}
