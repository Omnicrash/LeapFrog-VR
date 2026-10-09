//! Which of the two should drive a hand while its controller is powered on: the controller, or
//! the Leap (the composite device). Pure logic over positions and
//! velocities, so it can be tested without SteamVR. A wrong decision costs a moment of the
//! other data source, never a device change, so nothing here waits.
//!
//! Put down: the Leap sees the hand clearly away from the controller (farther than `away_m`
//! from the grip point) while the controller lies still. Pick up: the controller moves clearly
//! (faster than `pickup_mps`), or the Leap sees the hand at the controller (closer than
//! `near_m`). A controller without optical lock hands over as soon as the Leap sees the hand:
//! the hand is the better data then, held or not. A controller that is off is always "put
//! down"; one that has just been found powered on is taken to be in use.

use glam::{Quat, Vec3};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Driver {
    Controller,
    Leap,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thresholds {
    pub away_m: f32,
    pub near_m: f32,
    pub still_mps: f32,
    pub pickup_mps: f32,
}

impl Default for Thresholds {
    fn default() -> Thresholds {
        Thresholds { away_m: 0.20, near_m: 0.15, still_mps: 0.3, pickup_mps: 0.4 }
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
    pub hand_position: Option<Vec3>,
}

#[derive(Debug)]
pub struct Switch {
    state: Driver,
    controller_known: bool,
    /// Wrist position in the controller's frame while held, and how many samples went in.
    grip: Vec3,
    grip_samples: u32,
    thresholds: Thresholds,
    /// Which rule set the current state, for the log.
    reason: &'static str,
}

/// Samples before the learned grip point is trusted over the prior.
const GRIP_SAMPLES: u32 = 45;

/// Where the wrist sits in the Index controller's frame while held, as learned on the author's
/// setup across sessions (within a few cm of this each time: 0.20-0.24 m from the tracked
/// origin, mostly forward). The reference until the grip is learned; with the origin as the
/// reference a held hand measured 0.22-0.25 m "away".
const GRIP_PRIOR: Vec3 = Vec3::new(0.0, 0.07, 0.21);

/// The away threshold while the grip is still the prior, which may be off by a hand's width.
const AWAY_UNLEARNED_M: f32 = 0.35;

impl Switch {
    pub fn new(thresholds: Thresholds) -> Switch {
        Switch { state: Driver::Controller, controller_known: false, grip: GRIP_PRIOR, grip_samples: 0, thresholds, reason: "start" }
    }

    /// The learned grip point (wrist in the controller frame) once trusted; before that the
    /// reference is [`GRIP_PRIOR`].
    pub fn grip(&self) -> Option<Vec3> {
        self.learned().then_some(self.grip)
    }

    /// New thresholds (live settings).
    pub fn set_thresholds(&mut self, thresholds: Thresholds) {
        self.thresholds = thresholds;
    }

    fn learned(&self) -> bool {
        self.grip_samples >= GRIP_SAMPLES
    }

    /// Why the state is what it is (the rule that set it last).
    pub fn reason(&self) -> &'static str {
        self.reason
    }

    fn switch(&mut self, state: Driver, reason: &'static str) {
        self.state = state;
        self.reason = reason;
    }

    pub fn update(&mut self, o: &Observation) -> Driver {
        if !o.controller_on {
            self.controller_known = o.controller_known;
            self.switch(Driver::Leap, "controller off");
            return self.state;
        }
        if (o.controller_known && !self.controller_known) || o.force_controller {
            // Found powered on, or forced: in use until proven put down.
            self.switch(Driver::Controller, if o.force_controller { "forced" } else { "found on" });
        }
        self.controller_known = o.controller_known;
        if o.force_controller {
            return self.state;
        }
        let t = self.thresholds;
        let speed = o.controller_velocity.length();

        // No optical lock on the controller: its gyro-only position drifts and its velocity is
        // noise, so the distance rules are off; the hand is the better data whenever the Leap
        // sees it (the hand is where the hand is). No pick-up without
        // optical lock: the controller takes over once it is tracked again and moves or is at
        // the hand.
        let Some(controller) = o.controller_position else {
            if self.state == Driver::Controller && o.hand_position.is_some() {
                self.switch(Driver::Leap, "controller unseen, hand seen");
            }
            return self.state;
        };

        let distance = o.hand_position.map(|hand| {
            let local = o.controller_rotation.inverse() * (hand - controller);
            // Learn the grip while the controller drives and clearly moves with the hand in view:
            // quickly in the first second, then slowly and only near the point already known.
            if self.state == Driver::Controller && speed > 0.5 {
                if !self.learned() && local.length() < 0.3 {
                    // The first sample replaces the prior; the rest average in.
                    self.grip = self.grip.lerp(local, 1.0 / (self.grip_samples as f32 + 1.0));
                    self.grip_samples += 1;
                } else if self.learned() && local.distance(self.grip) < 0.25 {
                    self.grip = self.grip.lerp(local, 0.02);
                }
            }
            hand.distance(controller + o.controller_rotation * self.grip)
        });
        match self.state {
            Driver::Leap => {
                if speed > t.pickup_mps {
                    self.switch(Driver::Controller, "controller moving");
                } else if distance.is_some_and(|d| d < t.near_m) {
                    self.switch(Driver::Controller, "hand at controller");
                }
            }
            Driver::Controller => {
                let away_m = if self.learned() { t.away_m } else { t.away_m.max(AWAY_UNLEARNED_M) };
                if distance.is_some_and(|d| d > away_m) && speed < t.still_mps {
                    self.switch(Driver::Leap, "hand away, controller still");
                }
            }
        }
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where the wrist is while holding a controller at the origin, by the prior.
    const HELD: Vec3 = GRIP_PRIOR;

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

    fn far() -> Option<Vec3> {
        Some(Vec3::new(0.5, 0.0, 0.0))
    }

    #[test]
    fn controller_off_is_leap() {
        let mut s = Switch::new(Thresholds::default());
        assert_eq!(s.update(&obs(false, Vec3::ZERO, 0.0, None)), Driver::Leap);
        assert_eq!(s.update(&obs(true, Vec3::ZERO, 0.0, None)), Driver::Leap);
    }

    #[test]
    fn put_down_needs_distance_and_stillness() {
        let mut s = Switch::new(Thresholds::default());
        // Jitter in the controller: not a put-down.
        assert_eq!(s.update(&obs(true, Vec3::ZERO, 0.5, far())), Driver::Controller);
        // Still and away: at once.
        assert_eq!(s.update(&obs(true, Vec3::ZERO, 0.0, far())), Driver::Leap);
        // A hand where it is when holding (the prior), or 20 cm off that (an unlearned grip
        // can be off by that much), is not away.
        let mut held = Switch::new(Thresholds::default());
        for hand in [HELD, HELD + Vec3::new(0.2, 0.0, 0.0)] {
            for _ in 0..30 {
                assert_eq!(held.update(&obs(true, Vec3::ZERO, 0.0, Some(hand))), Driver::Controller);
            }
        }
    }

    #[test]
    fn hand_near_controller_stays_controller() {
        let mut s = Switch::new(Thresholds::default());
        for _ in 0..20 {
            assert_eq!(s.update(&obs(true, Vec3::ZERO, 0.0, Some(HELD))), Driver::Controller);
        }
    }

    #[test]
    fn controller_found_on_is_in_use_until_put_down() {
        let mut s = Switch::new(Thresholds::default());
        let unknown = Observation::default();
        assert_eq!(s.update(&unknown), Driver::Leap);
        // Found, hand not seen anywhere: the controller is assumed in hand.
        assert_eq!(s.update(&obs(true, Vec3::ZERO, 0.0, None)), Driver::Controller);
    }

    #[test]
    fn hand_at_controller_picks_it_up() {
        let mut s = Switch::new(Thresholds::default());
        assert_eq!(s.update(&obs(false, Vec3::ZERO, 0.0, None)), Driver::Leap);
        assert_eq!(s.update(&obs(true, Vec3::ZERO, 0.0, Some(HELD))), Driver::Controller);
    }

    #[test]
    fn pick_up_is_a_clear_move() {
        let mut s = Switch::new(Thresholds::default());
        assert_eq!(s.update(&obs(false, Vec3::ZERO, 0.0, None)), Driver::Leap);
        assert_eq!(s.update(&obs(true, Vec3::ZERO, 0.2, None)), Driver::Leap);
        assert_eq!(s.update(&obs(true, Vec3::ZERO, 0.6, None)), Driver::Controller);
    }

    #[test]
    fn forced_controller_wins() {
        let mut s = Switch::new(Thresholds::default());
        assert_eq!(s.update(&obs(false, Vec3::ZERO, 0.0, far())), Driver::Leap);
        let mut o = obs(true, Vec3::ZERO, 0.0, far());
        o.force_controller = true;
        assert_eq!(s.update(&o), Driver::Controller);
        assert_eq!(s.update(&o), Driver::Controller);
    }

    #[test]
    fn grip_is_learned_while_moving_and_used_for_distance() {
        let mut s = Switch::new(Thresholds::default());
        // Held and moving: wrist 25 cm "behind" the tracked origin, hand in view.
        let wrist = Some(Vec3::new(-0.25, 0.0, 0.0));
        for _ in 0..GRIP_SAMPLES + 5 {
            assert_eq!(s.update(&obs(true, Vec3::ZERO, 0.6, wrist)), Driver::Controller);
        }
        let grip = s.grip().expect("grip learned");
        assert!(grip.distance(Vec3::new(-0.25, 0.0, 0.0)) < 1e-3, "grip {grip}");
        // Still, hand where it is when holding: 25 cm from the origin, 0 from the grip point.
        for _ in 0..60 {
            assert_eq!(s.update(&obs(true, Vec3::ZERO, 0.0, wrist)), Driver::Controller);
        }
        // Put down: hand 25 cm from the grip point.
        assert_eq!(s.update(&obs(true, Vec3::ZERO, 0.0, Some(Vec3::new(-0.25, 0.25, 0.0)))), Driver::Leap);
    }

    #[test]
    fn grip_refines_only_near_itself() {
        let mut s = Switch::new(Thresholds::default());
        let wrist = Some(Vec3::new(-0.20, 0.0, 0.0));
        for _ in 0..GRIP_SAMPLES + 5 {
            s.update(&obs(true, Vec3::ZERO, 0.6, wrist));
        }
        let learned = s.grip().unwrap();
        // A mislabeled hand seen elsewhere while moving must not shift it.
        for _ in 0..200 {
            s.update(&obs(true, Vec3::ZERO, 0.6, Some(Vec3::new(0.1, 0.1, 0.0))));
        }
        assert_eq!(s.grip().unwrap(), learned);
        // An honest hand a few cm off does refine it, slowly.
        for _ in 0..300 {
            s.update(&obs(true, Vec3::ZERO, 0.6, Some(Vec3::new(-0.17, 0.0, 0.0))));
        }
        let refined = s.grip().unwrap();
        assert!(refined.x > learned.x + 0.02 && refined.x < -0.165, "refined {refined}");
    }

    #[test]
    fn untracked_controller_hands_over_when_the_hand_is_seen() {
        let dark = |speed: f32, hand: Option<Vec3>| {
            let mut o = obs(true, Vec3::ZERO, speed, hand);
            o.controller_position = None;
            o
        };
        // Controller driving, untracked, hand not seen: stays, whatever the (noise) velocity says.
        let mut s = Switch::new(Thresholds::default());
        for i in 0..300 {
            assert_eq!(s.update(&dark(if i % 2 == 0 { 0.0 } else { 0.8 }, None)), Driver::Controller);
        }
        // Hand seen while the controller is untracked: the Leap drives at once, and keeps
        // driving while dark, whatever the noise velocity, until the controller is tracked
        // again and picked up.
        let mut s = Switch::new(Thresholds::default());
        assert_eq!(s.update(&obs(true, Vec3::ZERO, 0.6, Some(HELD))), Driver::Controller);
        assert_eq!(s.update(&dark(0.0, far())), Driver::Leap);
        for _ in 0..100 {
            assert_eq!(s.update(&dark(0.8, far())), Driver::Leap);
        }
        assert_eq!(s.update(&obs(true, Vec3::ZERO, 0.0, Some(HELD))), Driver::Controller);
        // Leap driving (controller was off), controller comes on untracked: stays Leap.
        let mut s = Switch::new(Thresholds::default());
        assert_eq!(s.update(&obs(false, Vec3::ZERO, 0.0, far())), Driver::Leap);
        for _ in 1..300 {
            assert_eq!(s.update(&dark(0.8, far())), Driver::Leap);
        }
    }
}
