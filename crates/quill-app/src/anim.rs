//! State-driven animation: a value glides from wherever it currently is to a
//! new target, so interrupting an animation midway never jumps. Views read
//! `value()` while rendering and ask for another frame while `animating()`.

use std::time::{Duration, Instant};

use gpui_kit::Window;

/// Apple's standard motion: quick start, long soft landing.
pub const STANDARD: Duration = Duration::from_millis(300);
pub const QUICK: Duration = Duration::from_millis(160);

#[derive(Clone, Copy, Debug)]
pub struct Tween {
    from: f32,
    to: f32,
    start: Instant,
    duration: Duration,
}

impl Tween {
    pub fn new(value: f32) -> Tween {
        Tween { from: value, to: value, start: Instant::now(), duration: Duration::ZERO }
    }

    pub fn set(&mut self, to: f32, duration: Duration) {
        if duration.is_zero() {
            self.from = to;
            self.to = to;
            self.duration = Duration::ZERO;
            return;
        }
        if (to - self.to).abs() < f32::EPSILON {
            return;
        }
        self.from = self.value();
        self.to = to;
        self.start = Instant::now();
        self.duration = duration;
    }

    pub fn target(&self) -> f32 {
        self.to
    }

    pub fn progress(&self) -> f32 {
        if self.duration.is_zero() {
            return 1.;
        }
        (self.start.elapsed().as_secs_f32() / self.duration.as_secs_f32()).min(1.)
    }

    pub fn value(&self) -> f32 {
        self.from + (self.to - self.from) * ease_out(self.progress())
    }

    pub fn animating(&self) -> bool {
        self.progress() < 1.
    }

    /// The current value, scheduling another frame if still moving.
    pub fn sample(&self, window: &Window) -> f32 {
        if self.animating() {
            window.request_animation_frame();
        }
        self.value()
    }
}

/// Quintic ease-out: close to the curve of an over-damped spring.
pub fn ease_out(t: f32) -> f32 {
    1. - (1. - t).powi(5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eases_and_retargets_from_current_value() {
        assert_eq!(ease_out(0.), 0.);
        assert_eq!(ease_out(1.), 1.);
        assert!(ease_out(0.5) > 0.9, "most of the motion happens early");

        let mut t = Tween::new(1.);
        assert!(!t.animating());
        t.set(0., Duration::from_secs(60));
        assert!(t.animating());
        assert!(t.value() <= 1. && t.value() > 0.9);
        t.set(0., Duration::from_secs(1));
        assert_eq!(t.target(), 0., "same target keeps the running animation");
        t.set(0., Duration::ZERO);
        assert_eq!(t.value(), 0., "direct gestures settle even when the target is unchanged");
        assert!(!t.animating());
    }
}
