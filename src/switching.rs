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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thresholds {
    pub away_m: f32,
    pub near_m: f32,
    pub away_s: f32,
    pub still_mps: f32,
    pub pickup_mps: f32,
    /// Minimum time between two transitions of one hand, either direction.
    pub spacing_s: f32,
    /// Delay before the Leap takes over a controller without optical lock while the hand is
    /// seen (0 = at once).
    pub dead_s: f32,
}

impl Default for Thresholds {
    fn default() -> Thresholds {
        Thresholds { away_m: 0.20, near_m: 0.15, away_s: 0.2, still_mps: 0.3, pickup_mps: 0.4, spacing_s: 0.5, dead_s: 0.0 }
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
    away_since: Option<f64>,
    /// Since when the untracked controller has had the hand in view.
    dark_since: Option<f64>,
    controller_known: bool,
    /// No transition before this time (spacing).
    locked_until: f64,
    /// Wrist position in the controller's frame while held, and how many samples went in.
    grip: Vec3,
    grip_samples: u32,
    thresholds: Thresholds,
    /// Which rule set the current state, for the log.
    reason: &'static str,
}

/// Samples before the learned grip point is trusted over the prior.
const GRIP_SAMPLES: u32 = 45;

/// Where the wrist sits in the Index controller's frame while held, as learned on DCM1 across
/// sessions (within a few cm of this each time: 0.20-0.24 m from the tracked origin, mostly
/// forward). The reference until the grip is learned; before this prior the origin was the
/// reference and a held hand measured 0.22-0.25 m "away" (2026-10-08, 03:38).
const GRIP_PRIOR: Vec3 = Vec3::new(0.0, 0.07, 0.21);

/// The away threshold while the grip is still the prior, which may be off by a hand's width.
const AWAY_UNLEARNED_M: f32 = 0.35;

impl Switch {
    pub fn new(thresholds: Thresholds) -> Switch {
        Switch { state: Driver::Controller, away_since: None, dark_since: None, controller_known: false, locked_until: 0.0, grip: GRIP_PRIOR, grip_samples: 0, thresholds, reason: "start" }
    }

    /// The learned grip point (wrist in the controller frame) once trusted; before that the
    /// reference is [`GRIP_PRIOR`].
    pub fn grip(&self) -> Option<Vec3> {
        self.learned().then_some(self.grip)
    }

    /// New thresholds (live settings); the running timers keep going.
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

    fn switch(&mut self, state: Driver, now_s: f64, reason: &'static str) {
        self.state = state;
        self.reason = reason;
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
            self.reason = "controller off";
            return self.state;
        }
        if (o.controller_known && !self.controller_known) || o.force_controller {
            // Found powered on, or forced: in use until proven put down.
            self.state = Driver::Controller;
            self.reason = if o.force_controller { "forced" } else { "found on" };
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

        // No optical lock on the controller: its gyro-only position drifts and its velocity is
        // noise, so the distance rules are off. Then the hand is the better data whenever the
        // Leap sees it, held or not (user, 2026-10-08 07:15: the hand is where the hand is), so
        // the Leap drives, after `dead_s` if set. No pick-up without optical lock: the controller
        // takes over the normal way once it is tracked again and moves or is at the hand.
        let Some(controller) = o.controller_position else {
            self.away_since = None;
            let hand_seen = self.state == Driver::Controller && may_switch && o.hand_position.is_some();
            if !hand_seen {
                self.dark_since = None;
            } else {
                let since = *self.dark_since.get_or_insert(now_s);
                if now_s - since >= t.dead_s as f64 {
                    self.switch(Driver::Leap, now_s, "controller unseen, hand seen");
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
                if may_switch && speed > t.pickup_mps {
                    self.switch(Driver::Controller, now_s, "controller moving");
                } else if may_switch && distance.is_some_and(|d| d < t.near_m) {
                    self.switch(Driver::Controller, now_s, "hand at controller");
                }
            }
            Driver::Controller => {
                let away_m = if self.learned() { t.away_m } else { t.away_m.max(AWAY_UNLEARNED_M) };
                let away = may_switch && distance.is_some_and(|d| d > away_m) && speed < t.still_mps;
                if !away {
                    self.away_since = None;
                } else {
                    let delay = if self.learned() { t.away_s } else { t.away_s.max(0.5) };
                    let since = *self.away_since.get_or_insert(now_s);
                    if now_s - since >= delay as f64 {
                        self.switch(Driver::Leap, now_s, "hand away, controller still");
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

    /// The thresholds the tests were written against (the defaults before 2026-10-08).
    fn classic() -> Thresholds {
        Thresholds { away_m: 0.20, near_m: 0.15, away_s: 0.5, still_mps: 0.15, pickup_mps: 0.4, spacing_s: 2.0, dead_s: 0.0 }
    }

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

    #[test]
    fn controller_off_is_leap() {
        let mut s = Switch::new(classic());
        assert_eq!(s.update(0.0, &obs(false, Vec3::ZERO, 0.0, None)), Driver::Leap);
        assert_eq!(s.update(0.1, &obs(true, Vec3::ZERO, 0.0, None)), Driver::Leap);
    }

    #[test]
    fn put_down_needs_distance_stillness_and_time() {
        let mut s = Switch::new(Thresholds { away_s: 0.5, ..classic() });
        let far = Some(Vec3::new(0.5, 0.0, 0.0));
        assert_eq!(s.update(0.0, &obs(true, Vec3::ZERO, 0.0, far)), Driver::Controller);
        assert_eq!(s.update(0.3, &obs(true, Vec3::ZERO, 0.0, far)), Driver::Controller);
        // A hand where it is when holding (the prior), or 20 cm off that (an unlearned grip
        // can be off by that much), is not away.
        let mut held = Switch::new(Thresholds { away_s: 0.5, ..classic() });
        for (i, hand) in [HELD, HELD + Vec3::new(0.2, 0.0, 0.0)].iter().enumerate() {
            for j in 0..30 {
                assert_eq!(held.update(i as f64 * 10.0 + j as f64 * 0.1, &obs(true, Vec3::ZERO, 0.0, Some(*hand))), Driver::Controller);
            }
        }
        // Jitter in the controller resets the timer.
        assert_eq!(s.update(0.4, &obs(true, Vec3::ZERO, 0.3, far)), Driver::Controller);
        assert_eq!(s.update(0.8, &obs(true, Vec3::ZERO, 0.0, far)), Driver::Controller);
        assert_eq!(s.update(1.3, &obs(true, Vec3::ZERO, 0.0, far)), Driver::Leap);
    }

    #[test]
    fn hand_near_controller_stays_controller() {
        let mut s = Switch::new(classic());
        let near = Some(HELD);
        for i in 0..20 {
            assert_eq!(s.update(i as f64 * 0.1, &obs(true, Vec3::ZERO, 0.0, near)), Driver::Controller);
        }
    }

    #[test]
    fn controller_found_on_is_in_use_until_put_down() {
        let mut s = Switch::new(classic());
        let unknown = Observation::default();
        assert_eq!(s.update(0.0, &unknown), Driver::Leap);
        // Found, hand not seen anywhere: the controller is assumed in hand.
        assert_eq!(s.update(0.5, &obs(true, Vec3::ZERO, 0.0, None)), Driver::Controller);
    }

    #[test]
    fn hand_at_controller_picks_it_up() {
        let mut s = Switch::new(classic());
        assert_eq!(s.update(0.0, &obs(false, Vec3::ZERO, 0.0, None)), Driver::Leap);
        assert_eq!(s.update(1.0, &obs(true, Vec3::ZERO, 0.0, Some(HELD))), Driver::Controller);
    }

    #[test]
    fn forced_controller_wins() {
        let mut s = Switch::new(classic());
        let far = Some(Vec3::new(0.5, 0.0, 0.0));
        assert_eq!(s.update(0.0, &obs(false, Vec3::ZERO, 0.0, far)), Driver::Leap);
        let mut o = obs(true, Vec3::ZERO, 0.0, far);
        o.force_controller = true;
        assert_eq!(s.update(1.0, &o), Driver::Controller);
        assert_eq!(s.update(5.0, &o), Driver::Controller);
    }

    #[test]
    fn grip_is_learned_while_moving_and_used_for_distance() {
        let mut s = Switch::new(classic());
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
        let mut s = Switch::new(classic());
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
    fn untracked_controller_hands_over_when_the_hand_is_seen() {
        let far = Some(Vec3::new(0.5, 0.0, 0.0));
        let near = Some(HELD);
        let dark = |speed: f32, hand: Option<Vec3>| {
            let mut o = obs(true, Vec3::ZERO, speed, hand);
            o.controller_position = None;
            o
        };
        // Controller driving, untracked, hand not seen: stays, whatever the (noise) velocity says.
        let mut s = Switch::new(classic());
        for i in 0..300 {
            assert_eq!(s.update(i as f64 * 0.1, &dark(if i % 2 == 0 { 0.0 } else { 0.8 }, None)), Driver::Controller);
        }
        // Hand seen while the controller is untracked: the Leap drives at once (dead_s 0)...
        let mut s = Switch::new(Thresholds { dead_s: 0.0, ..classic() });
        assert_eq!(s.update(0.0, &obs(true, Vec3::ZERO, 0.6, near)), Driver::Controller);
        assert_eq!(s.update(3.0, &dark(0.0, far)), Driver::Leap);
        // ...and keeps driving while dark, whatever the noise velocity, until the controller is
        // tracked again and picked up.
        for i in 0..100 {
            assert_eq!(s.update(3.1 + i as f64 * 0.1, &dark(0.8, far)), Driver::Leap);
        }
        assert_eq!(s.update(20.0, &obs(true, Vec3::ZERO, 0.0, near)), Driver::Controller);
        // ...or after dead_s when set.
        let mut s = Switch::new(Thresholds { dead_s: 1.0, ..classic() });
        s.update(0.0, &obs(true, Vec3::ZERO, 0.6, near));
        assert_eq!(s.update(3.0, &dark(0.0, far)), Driver::Controller);
        assert_eq!(s.update(3.9, &dark(0.0, far)), Driver::Controller);
        assert_eq!(s.update(4.0, &dark(0.0, far)), Driver::Leap);
        // Leap driving (controller was off), controller comes on untracked: stays Leap.
        let mut s = Switch::new(classic());
        assert_eq!(s.update(0.0, &obs(false, Vec3::ZERO, 0.0, far)), Driver::Leap);
        for i in 1..300 {
            assert_eq!(s.update(i as f64 * 0.1, &dark(0.8, far)), Driver::Leap);
        }
    }

    #[test]
    fn transitions_are_spaced_in_both_directions() {
        let mut s = Switch::new(classic());
        let far = Some(Vec3::new(0.5, 0.0, 0.0));
        let near = Some(HELD);
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
        let mut s = Switch::new(classic());
        assert_eq!(s.update(0.0, &obs(false, Vec3::ZERO, 0.0, None)), Driver::Leap);
        assert_eq!(s.update(1.0, &obs(true, Vec3::ZERO, 0.2, None)), Driver::Leap);
        assert_eq!(s.update(1.1, &obs(true, Vec3::ZERO, 0.6, None)), Driver::Controller);
    }
}
