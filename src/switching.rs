//! Stage 2: which of the two should drive a hand while its controller is powered on. Pure
//! logic over positions and velocities, so it can be tested without SteamVR.
//!
//! Put down: the Leap sees the hand clearly away from the controller (farther than `away_m`)
//! while the controller lies still, for `away_s` (short: it only has to ride out a mis-tracked
//! frame). Pick up: the controller moves clearly (faster than `pickup_mps`), or the Leap sees the
//! hand at the controller (closer than `near_m`; the wrist sits a hand's length from the
//! controller's tracked origin, and the head offset guess adds error, so this is generous). A controller that is off is always "put down"; one that has just been found
//! powered on is taken to be in use.

use glam::{Quat, Vec3};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Driver {
    Controller,
    Leap,
}

#[derive(Clone, Copy, Debug)]
pub struct Thresholds {
    pub away_m: f32,
    pub near_m: f32,
    pub away_s: f32,
    /// In Leap mode with the controller on: hand unseen this long -> controller.
    pub unseen_s: f32,
    /// After a hand-back, no new take-over for this long (rides out a brief pause in a move).
    pub lockout_s: f32,
    pub still_mps: f32,
    pub pickup_mps: f32,
}

impl Default for Thresholds {
    fn default() -> Thresholds {
        Thresholds { away_m: 0.20, near_m: 0.12, away_s: 0.5, unseen_s: 1.5, lockout_s: 1.0, still_mps: 0.08, pickup_mps: 0.4 }
    }
}

/// What one frame knows about one hand and its controller, in world space.
#[derive(Clone, Copy, Debug, Default)]
pub struct Observation {
    /// Controllers hold the slots no matter what (room setup running, switching turned off).
    pub force_controller: bool,
    /// Whether the controller has been identified at all; before that nothing is known.
    pub controller_known: bool,
    pub controller_on: bool,
    pub controller_position: Option<Vec3>,
    pub controller_rotation: Quat,
    pub controller_velocity: Vec3,
    pub hand_position: Option<Vec3>,
}

#[derive(Debug)]
pub struct Switch {
    state: Driver,
    away_since: Option<f64>,
    unseen_since: Option<f64>,
    locked_until: f64,
    controller_known: bool,
    /// Wrist position in the controller's frame while held, and how many samples went in.
    grip: Vec3,
    grip_samples: u32,
    thresholds: Thresholds,
}

/// Samples before the learned grip point is trusted over the tracked origin.
const GRIP_SAMPLES: u32 = 45;

impl Switch {
    pub fn new(thresholds: Thresholds) -> Switch {
        Switch { state: Driver::Controller, away_since: None, unseen_since: None, locked_until: 0.0, controller_known: false, grip: Vec3::ZERO, grip_samples: 0, thresholds }
    }

    /// The learned grip point (wrist in the controller frame) once trusted.
    pub fn grip(&self) -> Option<Vec3> {
        (self.grip_samples >= GRIP_SAMPLES).then_some(self.grip)
    }

    /// `now_s` is any monotonic clock in seconds.
    pub fn update(&mut self, now_s: f64, o: &Observation) -> Driver {
        if !o.controller_on {
            self.away_since = None;
            self.unseen_since = None;
            self.controller_known = o.controller_known;
            self.state = Driver::Leap;
            return self.state;
        }
        if (o.controller_known && !self.controller_known) || o.force_controller {
            // Found powered on, or forced: in use until proven put down.
            self.state = Driver::Controller;
            self.away_since = None;
            self.controller_known = o.controller_known;
            if o.force_controller {
                return self.state;
            }
        }
        self.controller_known = o.controller_known;
        let t = &self.thresholds;
        let speed = o.controller_velocity.length();
        let distance = match (o.hand_position, o.controller_position) {
            (Some(hand), Some(controller)) => {
                let local = o.controller_rotation.inverse() * (hand - controller);
                // Learn the grip: controller driving, clearly moving, hand seen within reach.
                if self.state == Driver::Controller && speed > 0.3 && local.length() < 0.3 {
                    let alpha = if self.grip_samples < GRIP_SAMPLES { 1.0 / (self.grip_samples as f32 + 1.0) } else { 0.02 };
                    self.grip = self.grip.lerp(local, alpha);
                    self.grip_samples = self.grip_samples.saturating_add(1);
                }
                let reference = if self.grip_samples >= GRIP_SAMPLES { controller + o.controller_rotation * self.grip } else { controller };
                Some(hand.distance(reference))
            }
            _ => None,
        };
        match self.state {
            Driver::Leap => {
                let unseen_long = if o.hand_position.is_some() {
                    self.unseen_since = None;
                    false
                } else {
                    let since = *self.unseen_since.get_or_insert(now_s);
                    now_s - since >= t.unseen_s as f64
                };
                if speed > t.pickup_mps || distance.is_some_and(|d| d < t.near_m) || unseen_long {
                    self.state = Driver::Controller;
                    self.away_since = None;
                    self.unseen_since = None;
                    self.locked_until = now_s + t.lockout_s as f64;
                }
            }
            Driver::Controller => {
                let away = now_s >= self.locked_until && distance.is_some_and(|d| d > t.away_m) && speed < t.still_mps;
                if !away {
                    self.away_since = None;
                } else {
                    let since = *self.away_since.get_or_insert(now_s);
                    if now_s - since >= t.away_s as f64 {
                        self.state = Driver::Leap;
                        self.away_since = None;
                    }
                }
            }
        }
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(on: bool, controller: Vec3, speed: f32, hand: Option<Vec3>) -> Observation {
        Observation {
            force_controller: false,
            controller_known: true,
            controller_on: on,
            controller_position: Some(controller),
            controller_rotation: Quat::IDENTITY,
            controller_velocity: Vec3::new(speed, 0.0, 0.0),
            hand_position: hand,
        }
    }

    #[test]
    fn controller_off_is_leap() {
        let mut s = Switch::new(Thresholds::default());
        assert_eq!(s.update(0.0, &obs(false, Vec3::ZERO, 0.0, None)), Driver::Leap);
        assert_eq!(s.update(0.1, &obs(true, Vec3::ZERO, 0.0, None)), Driver::Leap);
    }

    #[test]
    fn put_down_needs_distance_stillness_and_time() {
        let mut s = Switch::new(Thresholds { away_s: 0.5, ..Thresholds::default() });
        let far = Some(Vec3::new(0.5, 0.0, 0.0));
        assert_eq!(s.update(0.0, &obs(true, Vec3::ZERO, 0.0, far)), Driver::Controller);
        assert_eq!(s.update(0.3, &obs(true, Vec3::ZERO, 0.0, far)), Driver::Controller);
        // Jitter in the controller resets the timer.
        assert_eq!(s.update(0.4, &obs(true, Vec3::ZERO, 0.3, far)), Driver::Controller);
        assert_eq!(s.update(0.8, &obs(true, Vec3::ZERO, 0.0, far)), Driver::Controller);
        assert_eq!(s.update(1.3, &obs(true, Vec3::ZERO, 0.0, far)), Driver::Leap);
    }

    #[test]
    fn hand_near_controller_stays_controller() {
        let mut s = Switch::new(Thresholds::default());
        let near = Some(Vec3::new(0.05, 0.0, 0.0));
        for i in 0..20 {
            assert_eq!(s.update(i as f64 * 0.1, &obs(true, Vec3::ZERO, 0.0, near)), Driver::Controller);
        }
    }

    #[test]
    fn controller_found_on_is_in_use_until_put_down() {
        let mut s = Switch::new(Thresholds::default());
        let unknown = Observation::default();
        assert_eq!(s.update(0.0, &unknown), Driver::Leap);
        // Found, hand not seen anywhere: the controller is assumed in hand.
        assert_eq!(s.update(0.5, &obs(true, Vec3::ZERO, 0.0, None)), Driver::Controller);
    }

    #[test]
    fn hand_at_controller_picks_it_up() {
        let mut s = Switch::new(Thresholds::default());
        assert_eq!(s.update(0.0, &obs(false, Vec3::ZERO, 0.0, None)), Driver::Leap);
        assert_eq!(s.update(1.0, &obs(true, Vec3::ZERO, 0.0, Some(Vec3::new(0.05, 0.0, 0.0)))), Driver::Controller);
    }

    #[test]
    fn unseen_hand_gives_the_controller_back() {
        let mut s = Switch::new(Thresholds::default());
        assert_eq!(s.update(0.0, &obs(false, Vec3::ZERO, 0.0, None)), Driver::Leap);
        // Controller on, still, hand nowhere in view: Leap for a while, then the controller.
        assert_eq!(s.update(1.0, &obs(true, Vec3::ZERO, 0.0, None)), Driver::Leap);
        assert_eq!(s.update(2.0, &obs(true, Vec3::ZERO, 0.0, None)), Driver::Leap);
        assert_eq!(s.update(2.6, &obs(true, Vec3::ZERO, 0.0, None)), Driver::Controller);
    }

    #[test]
    fn seen_hand_away_from_still_controller_stays_leap() {
        let mut s = Switch::new(Thresholds::default());
        let far = Some(Vec3::new(0.5, 0.0, 0.0));
        assert_eq!(s.update(0.0, &obs(false, Vec3::ZERO, 0.0, far)), Driver::Leap);
        for i in 1..50 {
            assert_eq!(s.update(i as f64 * 0.1, &obs(true, Vec3::ZERO, 0.0, far)), Driver::Leap);
        }
    }

    #[test]
    fn hand_back_locks_out_the_next_take_over() {
        let mut s = Switch::new(Thresholds::default());
        let far = Some(Vec3::new(0.5, 0.0, 0.0));
        assert_eq!(s.update(0.0, &obs(false, Vec3::ZERO, 0.0, far)), Driver::Leap);
        assert_eq!(s.update(1.0, &obs(true, Vec3::ZERO, 0.6, far)), Driver::Controller); // picked up
        assert_eq!(s.update(1.2, &obs(true, Vec3::ZERO, 0.0, far)), Driver::Controller); // brief pause
        assert_eq!(s.update(1.5, &obs(true, Vec3::ZERO, 0.0, far)), Driver::Controller);
        assert_eq!(s.update(2.3, &obs(true, Vec3::ZERO, 0.0, far)), Driver::Controller); // lockout over, timer starts
        assert_eq!(s.update(2.9, &obs(true, Vec3::ZERO, 0.0, far)), Driver::Leap);
    }

    #[test]
    fn forced_controller_wins() {
        let mut s = Switch::new(Thresholds::default());
        let far = Some(Vec3::new(0.5, 0.0, 0.0));
        assert_eq!(s.update(0.0, &obs(false, Vec3::ZERO, 0.0, far)), Driver::Leap);
        let mut o = obs(true, Vec3::ZERO, 0.0, far);
        o.force_controller = true;
        assert_eq!(s.update(1.0, &o), Driver::Controller);
        assert_eq!(s.update(5.0, &o), Driver::Controller);
    }

    #[test]
    fn grip_is_learned_while_moving_and_used_for_distance() {
        let mut s = Switch::new(Thresholds::default());
        // Held and moving: wrist 25 cm "behind" the tracked origin, hand in view.
        let wrist = Some(Vec3::new(-0.25, 0.0, 0.0));
        for i in 0..GRIP_SAMPLES + 5 {
            assert_eq!(s.update(i as f64 * 0.02, &obs(true, Vec3::ZERO, 0.5, wrist)), Driver::Controller);
        }
        let grip = s.grip().expect("grip learned");
        assert!(grip.distance(Vec3::new(-0.25, 0.0, 0.0)) < 1e-3, "grip {grip}");
        // Now still, hand where it is when holding: 25 cm from the origin but 0 from the grip point.
        for i in 0..60 {
            assert_eq!(s.update(10.0 + i as f64 * 0.05, &obs(true, Vec3::ZERO, 0.0, wrist)), Driver::Controller);
        }
        // Put down: hand 25 cm from the grip point.
        let away = Some(Vec3::new(-0.25, 0.25, 0.0));
        s.update(20.0, &obs(true, Vec3::ZERO, 0.0, away));
        assert_eq!(s.update(20.6, &obs(true, Vec3::ZERO, 0.0, away)), Driver::Leap);
    }

    #[test]
    fn pick_up_is_a_clear_move() {
        let mut s = Switch::new(Thresholds::default());
        assert_eq!(s.update(0.0, &obs(false, Vec3::ZERO, 0.0, None)), Driver::Leap);
        assert_eq!(s.update(1.0, &obs(true, Vec3::ZERO, 0.2, None)), Driver::Leap);
        assert_eq!(s.update(1.1, &obs(true, Vec3::ZERO, 0.6, None)), Driver::Controller);
    }
}
