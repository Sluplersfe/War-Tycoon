//! Snapshot buffer for objects this client does not own.

use std::collections::VecDeque;

use glam::{Quat, Vec3};

/// Render remote objects this far in the past so two snapshots usually bracket it.
pub const INTERP_DELAY: f64 = 0.1;
const MAX_EXTRAPOLATE: f64 = 0.25;
const MAX_SNAPSHOTS: usize = 32;

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
}

impl Buffer {
    pub fn push(&mut self, s: Snapshot) {
        // Keep time-ordered; drop out-of-order arrivals.
        if let Some(last) = self.snaps.back() {
            if s.t <= last.t {
                return;
            }
        }
        self.snaps.push_back(s);
        while self.snaps.len() > MAX_SNAPSHOTS {
            self.snaps.pop_front();
        }
    }

    pub fn clear(&mut self) {
        self.snaps.clear();
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
                return Some((a.pos.lerp(b.pos, f), a.rot.slerp(b.rot, f)));
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
        assert!((b.sample(1.05).unwrap().0.x - 0.5).abs() < 1e-4);
        assert!((b.sample(1.2).unwrap().0.x - 2.0).abs() < 1e-4);
        assert!((b.sample(5.0).unwrap().0.x - 3.5).abs() < 1e-4, "extrapolation is capped");
    }
}
