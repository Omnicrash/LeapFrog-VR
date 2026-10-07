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
    pub still_mps: f32,
    pub pickup_mps: f32,
    /// Minimum time between two transitions of one hand, either direction.
    pub spacing_s: f32,
    /// Dead spot: how long a controller must lie untracked and still, with the hand seen, before
    /// the hand takes over. 0 disables the rule.
    pub dead_s: f32,
}

impl Default for Thresholds {
    fn default() -> Thresholds {
        Thresholds { away_m: 0.20, near_m: 0.15, away_s: 0.5, still_mps: 0.15, pickup_mps: 0.4, spacing_s: 2.0, dead_s: 3.0 }
    }
}

/// What one frame knows about one hand and its controller, in world space.
#[derive(Clone, Copy, Debug, Default)]
pub struct Observation {
    /// Controllers hold the slots no matter what: SteamVR's room setup running (it needs the
    /// controllers in the hand slots), or switching turned off.
    pub force_controller: bool,
    /// Whether the controller has been identified at all; before that nothing is known.
    pub controller_known: bool,
    pub controller_on: bool,
    /// The controller's position when its pose is fully tracked (optical lock); None while
    /// degraded or invalid.
    pub controller_position: Option<Vec3>,
    pub controller_rotation: Quat,
    pub controller_velocity: Vec3,
    /// From the gyro, so it holds up without optical tracking (rad/s).
    pub controller_angular_velocity: Vec3,
    pub hand_position: Option<Vec3>,
}

#[derive(Debug)]
pub struct Switch {
    state: Driver,
    away_since: Option<f64>,
    /// Since when the untracked controller has lain still with the hand in view.
    dark_since: Option<f64>,
    controller_known: bool,
    /// No transition before this time (spacing).
    locked_until: f64,
    /// Wrist position in the controller's frame while held, and how many samples went in.
    grip: Vec3,
    grip_samples: u32,
    thresholds: Thresholds,
}

/// Samples before the learned grip point is trusted over the tracked origin.
const GRIP_SAMPLES: u32 = 45;

/// Gyro reading below which an untracked controller counts as lying still (rad/s); a hand holding
/// one is rarely this steady for `dead_s`, a table always is.
const DEAD_TURN_RPS: f32 = 0.3;

impl Switch {
    pub fn new(thresholds: Thresholds) -> Switch {
        Switch { state: Driver::Controller, away_since: None, dark_since: None, controller_known: false, locked_until: 0.0, grip: Vec3::ZERO, grip_samples: 0, thresholds }
    }

    /// The learned grip point (wrist in the controller frame) once trusted.
    pub fn grip(&self) -> Option<Vec3> {
        (self.grip_samples >= GRIP_SAMPLES).then_some(self.grip)
    }

    fn switch(&mut self, state: Driver, now_s: f64) {
        self.state = state;
        self.away_since = None;
        self.dark_since = None;
        self.locked_until = now_s + self.thresholds.spacing_s as f64;
    }

    /// `now_s` is any monotonic clock in seconds.
    pub fn update(&mut self, now_s: f64, o: &Observation) -> Driver {
        if !o.controller_on {
            // Power decides at once; spacing applies to the distance rules only.
            self.away_since = None;
            self.dark_since = None;
            self.controller_known = o.controller_known;
            self.state = Driver::Leap;
            return self.state;
        }
        if (o.controller_known && !self.controller_known) || o.force_controller {
            // Found powered on, or forced: in use until proven put down.
            self.state = Driver::Controller;
            self.away_since = None;
            self.dark_since = None;
        }
        self.controller_known = o.controller_known;
        if o.force_controller {
            return self.state;
        }
        let t = self.thresholds;
        let speed = o.controller_velocity.length();
        let may_switch = now_s >= self.locked_until;

        // Degraded pose (controller on, no optical lock): its gyro-only position drifts and its
        // velocity is noise, so the distance rules are off. One rule remains, for a controller
        // put down where the base stations cannot see it (doc/Background.md, "The dead-spot left
        // hand"): lying still by its gyro, with the hand seen somewhere, for `dead_s`, the hand
        // takes over. It comes back the normal way, once the controller is tracked again and
        // moves or is near the hand; no pick-up from the gyro, which oscillated on the FBT night.
        let Some(controller) = o.controller_position else {
            self.away_since = None;
            let resting = self.state == Driver::Controller && may_switch && t.dead_s > 0.0
                && o.hand_position.is_some() && o.controller_angular_velocity.length() < DEAD_TURN_RPS;
            if !resting {
                self.dark_since = None;
            } else {
                let since = *self.dark_since.get_or_insert(now_s);
                if now_s - since >= t.dead_s as f64 {
                    self.switch(Driver::Leap, now_s);
                }
            }
            return self.state;
        };
        self.dark_since = None;

        let distance = o.hand_position.map(|hand| {
            let local = o.controller_rotation.inverse() * (hand - controller);
            // Learn the grip while the controller drives and clearly moves with the hand in view:
            // quickly in the first second, then slowly and only near the point already known.
            if self.state == Driver::Controller && speed > 0.5 {
                if self.grip_samples < GRIP_SAMPLES && local.length() < 0.3 {
                    self.grip = self.grip.lerp(local, 1.0 / (self.grip_samples as f32 + 1.0));
                    self.grip_samples += 1;
                } else if self.grip_samples >= GRIP_SAMPLES && local.distance(self.grip) < 0.25 {
                    self.grip = self.grip.lerp(local, 0.02);
                }
            }
            let reference = match self.grip() {
                Some(grip) => controller + o.controller_rotation * grip,
                None => controller,
            };
            hand.distance(reference)
        });
        match self.state {
            Driver::Leap => {
                if may_switch && (speed > t.pickup_mps || distance.is_some_and(|d| d < t.near_m)) {
                    self.switch(Driver::Controller, now_s);
                }
            }
            Driver::Controller => {
                let away = may_switch && distance.is_some_and(|d| d > t.away_m) && speed < t.still_mps;
                if !away {
                    self.away_since = None;
                } else {
                    let delay = if self.grip().is_some() { t.away_s } else { t.away_s.max(0.5) };
                    let since = *self.away_since.get_or_insert(now_s);
                    if now_s - since >= delay as f64 {
                        self.switch(Driver::Leap, now_s);
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
            controller_angular_velocity: Vec3::ZERO,
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
            assert_eq!(s.update(i as f64 * 0.02, &obs(true, Vec3::ZERO, 0.6, wrist)), Driver::Controller);
        }
        let grip = s.grip().expect("grip learned");
        assert!(grip.distance(Vec3::new(-0.25, 0.0, 0.0)) < 1e-3, "grip {grip}");
        // Still, hand where it is when holding: 25 cm from the origin, 0 from the grip point.
        for i in 0..60 {
            assert_eq!(s.update(10.0 + i as f64 * 0.05, &obs(true, Vec3::ZERO, 0.0, wrist)), Driver::Controller);
        }
        // Put down: hand 25 cm from the grip point; the short delay applies once learned.
        let away = Some(Vec3::new(-0.25, 0.25, 0.0));
        s.update(20.0, &obs(true, Vec3::ZERO, 0.0, away));
        assert_eq!(s.update(20.3, &obs(true, Vec3::ZERO, 0.0, away)), Driver::Controller);
        assert_eq!(s.update(20.6, &obs(true, Vec3::ZERO, 0.0, away)), Driver::Leap);
    }

    #[test]
    fn grip_refines_only_near_itself() {
        let mut s = Switch::new(Thresholds::default());
        let wrist = Some(Vec3::new(-0.20, 0.0, 0.0));
        for i in 0..GRIP_SAMPLES + 5 {
            s.update(i as f64 * 0.02, &obs(true, Vec3::ZERO, 0.6, wrist));
        }
        let learned = s.grip().unwrap();
        // A mislabeled hand seen elsewhere while moving must not shift it.
        for i in 0..200 {
            s.update(10.0 + i as f64 * 0.02, &obs(true, Vec3::ZERO, 0.6, Some(Vec3::new(0.1, 0.1, 0.0))));
        }
        assert_eq!(s.grip().unwrap(), learned);
        // An honest hand a few cm off does refine it, slowly.
        for i in 0..300 {
            s.update(20.0 + i as f64 * 0.02, &obs(true, Vec3::ZERO, 0.6, Some(Vec3::new(-0.17, 0.0, 0.0))));
        }
        let refined = s.grip().unwrap();
        assert!(refined.x > learned.x + 0.02 && refined.x < -0.165, "refined {refined}");
    }

    #[test]
    fn degraded_controller_decides_nothing_unless_resting() {
        let far = Some(Vec3::new(0.5, 0.0, 0.0));
        let degraded = |speed: f32, hand: Option<Vec3>, turn: f32| {
            let mut o = obs(true, Vec3::ZERO, speed, hand);
            o.controller_position = None;
            o.controller_angular_velocity = Vec3::new(0.0, turn, 0.0);
            o
        };
        // Controller driving, untracked for a long time, hand not seen: stays, whatever the
        // (noise) velocity says.
        let mut s = Switch::new(Thresholds::default());
        for i in 0..300 {
            assert_eq!(s.update(i as f64 * 0.1, &degraded(if i % 2 == 0 { 0.0 } else { 0.8 }, None, 0.0)), Driver::Controller);
        }
        // Hand seen, but the controller keeps turning (it is in a hand): stays.
        let mut s = Switch::new(Thresholds::default());
        for i in 0..300 {
            assert_eq!(s.update(i as f64 * 0.1, &degraded(0.0, far, 1.0)), Driver::Controller);
        }
        // Leap driving (controller was off), controller comes on untracked and still: stays Leap.
        let mut s = Switch::new(Thresholds::default());
        assert_eq!(s.update(0.0, &obs(false, Vec3::ZERO, 0.0, far)), Driver::Leap);
        for i in 1..300 {
            assert_eq!(s.update(i as f64 * 0.1, &degraded(0.8, far, 0.0)), Driver::Leap);
        }
    }

    #[test]
    fn dead_spot_hands_over_when_still_and_hand_seen() {
        let far = Some(Vec3::new(0.5, 0.0, 0.0));
        let near = Some(Vec3::new(0.05, 0.0, 0.0));
        let dark = |hand: Option<Vec3>, turn: f32| {
            let mut o = obs(true, Vec3::ZERO, 0.0, hand);
            o.controller_position = None;
            o.controller_angular_velocity = Vec3::new(turn, 0.0, 0.0);
            o
        };
        let mut s = Switch::new(Thresholds::default());
        assert_eq!(s.update(0.0, &obs(true, Vec3::ZERO, 0.6, near)), Driver::Controller);
        // Put down out of sight at t=3: still, hand in view, but not for 3 s yet.
        for i in 0..29 {
            assert_eq!(s.update(3.0 + i as f64 * 0.1, &dark(far, 0.0)), Driver::Controller, "at {i}");
        }
        // A nudge resets the timer: 3 s of rest again from here.
        assert_eq!(s.update(5.9, &dark(far, 0.5)), Driver::Controller);
        assert_eq!(s.update(6.0, &dark(far, 0.0)), Driver::Controller);
        assert_eq!(s.update(8.9, &dark(far, 0.0)), Driver::Controller);
        assert_eq!(s.update(9.0, &dark(far, 0.0)), Driver::Leap);
        // Still dark and still: no flip-flop.
        for i in 0..100 {
            assert_eq!(s.update(9.1 + i as f64 * 0.1, &dark(far, 0.0)), Driver::Leap);
        }
        // Picked up: tracked again with the hand at it (spacing is long over).
        assert_eq!(s.update(20.0, &obs(true, Vec3::ZERO, 0.0, near)), Driver::Controller);
        // deadSpotTime 0 disables the rule.
        let mut s = Switch::new(Thresholds { dead_s: 0.0, ..Thresholds::default() });
        s.update(0.0, &obs(true, Vec3::ZERO, 0.6, near));
        for i in 0..100 {
            assert_eq!(s.update(1.0 + i as f64 * 0.1, &dark(far, 0.0)), Driver::Controller);
        }
    }

    #[test]
    fn transitions_are_spaced_in_both_directions() {
        let mut s = Switch::new(Thresholds::default());
        let far = Some(Vec3::new(0.5, 0.0, 0.0));
        let near = Some(Vec3::new(0.05, 0.0, 0.0));
        assert_eq!(s.update(0.0, &obs(false, Vec3::ZERO, 0.0, far)), Driver::Leap);
        assert_eq!(s.update(1.0, &obs(true, Vec3::ZERO, 0.6, far)), Driver::Controller); // picked up
        // Put down right away: not before the spacing is over.
        for i in 1..20 {
            assert_eq!(s.update(1.0 + i as f64 * 0.1, &obs(true, Vec3::ZERO, 0.0, far)), Driver::Controller);
        }
        s.update(3.05, &obs(true, Vec3::ZERO, 0.0, far)); // spacing over: the put-down timer starts (0.5 s)
        assert_eq!(s.update(3.4, &obs(true, Vec3::ZERO, 0.0, far)), Driver::Controller);
        assert_eq!(s.update(3.6, &obs(true, Vec3::ZERO, 0.0, far)), Driver::Leap);
        // Grab it again at once: hand at the controller, but the spacing holds for 2 s.
        assert_eq!(s.update(3.8, &obs(true, Vec3::ZERO, 0.0, near)), Driver::Leap);
        assert_eq!(s.update(5.7, &obs(true, Vec3::ZERO, 0.0, near)), Driver::Controller);
    }

    #[test]
    fn pick_up_is_a_clear_move() {
        let mut s = Switch::new(Thresholds::default());
        assert_eq!(s.update(0.0, &obs(false, Vec3::ZERO, 0.0, None)), Driver::Leap);
        assert_eq!(s.update(1.0, &obs(true, Vec3::ZERO, 0.2, None)), Driver::Leap);
        assert_eq!(s.update(1.1, &obs(true, Vec3::ZERO, 0.6, None)), Driver::Controller);
    }
}
