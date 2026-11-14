//! Snapshot buffer for objects this client does not own.

use std::collections::VecDeque;

use glam::{Quat, Vec3};

/// Render remote objects this far in the past so two snapshots usually bracket it.
pub const INTERP_DELAY: f64 = 0.1;
const MAX_EXTRAPOLATE: f64 = 0.25;
const MAX_SNAPSHOTS: usize = 32;
const MIN_DELAY: f64 = 0.06;
const MAX_DELAY: f64 = 0.30;

#[derive(Debug, Clone, Copy)]
pub struct Snapshot {
    pub t: f64,
    pub pos: Vec3,
    pub rot: Quat,
    pub vel: Vec3,
}

#[derive(Debug, Default, Clone)]
pub struct Buffer {
    snaps: VecDeque<Snapshot>,
    /// Smoothed gap between arrivals and its mean deviation (network jitter).
    interval: f64,
    jitter: f64,
}

impl Buffer {
    pub fn push(&mut self, s: Snapshot) {
        // Keep time-ordered; drop out-of-order arrivals.
        if let Some(last) = self.snaps.back() {
            if s.t <= last.t {
                return;
            }
        }
        if let Some(last) = self.snaps.back() {
            let gap = s.t - last.t;
            if gap < 1.0 {
                if self.interval == 0.0 {
                    self.interval = gap;
                } else {
                    self.jitter += ((gap - self.interval).abs() - self.jitter) * 0.1;
                    self.interval += (gap - self.interval) * 0.1;
                }
            }
        }
        self.snaps.push_back(s);
        while self.snaps.len() > MAX_SNAPSHOTS {
            self.snaps.pop_front();
        }
    }

    pub fn clear(&mut self) {
        self.snaps.clear();
        self.interval = 0.0;
        self.jitter = 0.0;
    }

    /// How far in the past to render: enough to usually hold two snapshots
    /// around that moment, growing with measured jitter.
    pub fn delay(&self) -> f64 {
        if self.interval == 0.0 {
            return INTERP_DELAY;
        }
        (self.interval * 1.5 + self.jitter * 2.0).clamp(MIN_DELAY, MAX_DELAY)
    }

    pub fn latest(&self) -> Option<&Snapshot> {
        self.snaps.back()
    }

    /// State at render time `t` (already delayed by the caller).
    pub fn sample(&self, t: f64) -> Option<(Vec3, Quat)> {
        let first = self.snaps.front()?;
        if t <= first.t {
            return Some((first.pos, first.rot));
        }
        for w in self.snaps.iter().zip(self.snaps.iter().skip(1)) {
            let (a, b) = w;
            if t >= a.t && t <= b.t {
                let f = ((t - a.t) / (b.t - a.t).max(1e-6)) as f32;
                // Cubic Hermite through both snapshots' velocities: smooth turns and no
                // visible speed changes at snapshot boundaries.
                let h = (b.t - a.t) as f32;
                let (f2, f3) = (f * f, f * f * f);
                let pos = a.pos * (2.0 * f3 - 3.0 * f2 + 1.0)
                    + a.vel * (h * (f3 - 2.0 * f2 + f))
                    + b.pos * (-2.0 * f3 + 3.0 * f2)
                    + b.vel * (h * (f3 - f2));
                // Fall back to a straight line if velocities are bogus (e.g. teleports).
                let line = a.pos.lerp(b.pos, f);
                let pos = if pos.distance(line) > 0.5 * a.pos.distance(b.pos).max(1.0) { line } else { pos };
                return Some((pos, a.rot.slerp(b.rot, f)));
            }
        }
        let last = self.snaps.back()?;
        let dt = (t - last.t).min(MAX_EXTRAPOLATE) as f32;
        Some((last.pos + last.vel * dt, last.rot))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(t: f64, x: f32) -> Snapshot {
        Snapshot { t, pos: Vec3::new(x, 0.0, 0.0), rot: Quat::IDENTITY, vel: Vec3::new(10.0, 0.0, 0.0) }
    }

    #[test]
    fn interpolates_and_extrapolates() {
        let mut b = Buffer::default();
        assert!(b.sample(0.0).is_none());
        b.push(snap(1.0, 0.0));
        b.push(snap(1.1, 1.0));
        b.push(snap(1.05, 99.0)); // out of order, ignored
        assert_eq!(b.sample(0.5).unwrap().0.x, 0.0);
        assert!((b.sample(1.05).unwrap().0.x - 0.5).abs() < 0.2);
        assert!((b.sample(1.2).unwrap().0.x - 2.0).abs() < 1e-4);
        assert!((b.sample(5.0).unwrap().0.x - 3.5).abs() < 1e-4, "extrapolation is capped");
    }

    #[test]
    fn delay_grows_with_jitter() {
        let mut steady = Buffer::default();
        let mut jittery = Buffer::default();
        let (mut t1, mut t2) = (0.0, 0.0);
        for i in 0..40 {
            t1 += 0.05;
            t2 += if i % 2 == 0 { 0.02 } else { 0.09 };
            steady.push(snap(t1, 0.0));
            jittery.push(snap(t2, 0.0));
        }
        assert!(jittery.delay() > steady.delay());
        assert!(steady.delay() >= 0.06 && jittery.delay() <= 0.3);
    }
}
