//! Stage 2: which of the two should drive a hand while its controller is powered on. Pure
//! logic over positions and velocities, so it can be tested without SteamVR.
//!
//! Put down: the Leap sees the hand clearly away from the controller (farther than `away_m`)
//! while the controller lies still, for `away_s`. Pick up: the controller moves clearly
//! (faster than `pickup_mps`). A controller that is off is always "put down".

use glam::Vec3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Driver {
    Controller,
    Leap,
}

#[derive(Clone, Copy, Debug)]
pub struct Thresholds {
    pub away_m: f32,
    pub away_s: f32,
    pub still_mps: f32,
    pub pickup_mps: f32,
}

impl Default for Thresholds {
    fn default() -> Thresholds {
        Thresholds { away_m: 0.20, away_s: 0.5, still_mps: 0.15, pickup_mps: 0.4 }
    }
}

/// What one frame knows about one hand and its controller, in world space.
#[derive(Clone, Copy, Debug, Default)]
pub struct Observation {
    pub controller_on: bool,
    pub controller_position: Option<Vec3>,
    pub controller_velocity: Vec3,
    pub hand_position: Option<Vec3>,
}

#[derive(Debug)]
pub struct Switch {
    state: Driver,
    away_since: Option<f64>,
    thresholds: Thresholds,
}

impl Switch {
    pub fn new(thresholds: Thresholds) -> Switch {
        Switch { state: Driver::Controller, away_since: None, thresholds }
    }

    /// `now_s` is any monotonic clock in seconds.
    pub fn update(&mut self, now_s: f64, o: &Observation) -> Driver {
        if !o.controller_on {
            self.away_since = None;
            self.state = Driver::Leap;
            return self.state;
        }
        let t = &self.thresholds;
        let speed = o.controller_velocity.length();
        match self.state {
            Driver::Leap => {
                if speed > t.pickup_mps {
                    self.state = Driver::Controller;
                    self.away_since = None;
                }
            }
            Driver::Controller => {
                let away = match (o.hand_position, o.controller_position) {
                    (Some(hand), Some(controller)) => hand.distance(controller) > t.away_m && speed < t.still_mps,
                    _ => false,
                };
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
            controller_on: on,
            controller_position: Some(controller),
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
        let mut s = Switch::new(Thresholds::default());
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
    fn pick_up_is_a_clear_move() {
        let mut s = Switch::new(Thresholds::default());
        assert_eq!(s.update(0.0, &obs(false, Vec3::ZERO, 0.0, None)), Driver::Leap);
        assert_eq!(s.update(1.0, &obs(true, Vec3::ZERO, 0.2, None)), Driver::Leap);
        assert_eq!(s.update(1.1, &obs(true, Vec3::ZERO, 0.6, None)), Driver::Controller);
    }
}
