//! Small signal filters: the One-Euro filter (two-handed aim steadying) and the velocity history
//! that throws are computed from.

use glam::Vec3;
use std::collections::VecDeque;

/// One-Euro filter (Casiez et al.): heavy smoothing when still, little lag when moving fast.
/// VRExpansionPlugin steadies the support hand of a two-handed gun with one.
#[derive(Clone, Copy, Debug)]
pub struct OneEuro {
    pub min_cutoff: f32,
    pub beta: f32,
    pub d_cutoff: f32,
    x: Option<Vec3>,
    dx: Vec3,
}

impl OneEuro {
    pub fn new(min_cutoff: f32, beta: f32, d_cutoff: f32) -> OneEuro {
        OneEuro { min_cutoff, beta, d_cutoff, x: None, dx: Vec3::ZERO }
    }

    fn alpha(cutoff: f32, dt: f32) -> f32 {
        let tau = 1.0 / (std::f32::consts::TAU * cutoff.max(1e-3));
        1.0 / (1.0 + tau / dt.max(1e-5))
    }

    pub fn reset(&mut self) {
        self.x = None;
        self.dx = Vec3::ZERO;
    }

    pub fn filter(&mut self, v: Vec3, dt: f32) -> Vec3 {
        let Some(prev) = self.x else {
            self.x = Some(v);
            return v;
        };
        let dx = (v - prev) / dt.max(1e-5);
        self.dx = self.dx.lerp(dx, Self::alpha(self.d_cutoff, dt));
        let cutoff = self.min_cutoff + self.beta * self.dx.length();
        let out = prev.lerp(v, Self::alpha(cutoff, dt));
        self.x = Some(out);
        out
    }
}

/// Recent controller motion (world space), for throw velocity.
#[derive(Default, Clone, Debug)]
pub struct MotionHistory {
    samples: VecDeque<(f64, Vec3, Vec3)>, // time, position, angular velocity
}

impl MotionHistory {
    pub fn push(&mut self, t: f64, pos: Vec3, ang_vel: Vec3) {
        self.samples.push_back((t, pos, ang_vel));
        while self.samples.len() > 12 {
            self.samples.pop_front();
        }
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// Release velocity: the peak-speed frame of the last ~5 frames averaged with its neighbours
    /// (HIGGS), so a wrist that's already slowing at the moment of release still throws hard.
    pub fn throw_velocity(&self) -> (Vec3, Vec3) {
        let s: Vec<_> = self.samples.iter().collect();
        if s.len() < 2 {
            return (Vec3::ZERO, s.last().map(|x| x.2).unwrap_or(Vec3::ZERO));
        }
        let vels: Vec<(Vec3, Vec3)> = s
            .windows(2)
            .map(|w| {
                let dt = (w[1].0 - w[0].0).max(1e-4) as f32;
                ((w[1].1 - w[0].1) / dt, w[1].2)
            })
            .collect();
        let recent = &vels[vels.len().saturating_sub(5)..];
        let (peak, _) = recent.iter().enumerate().fold((0, -1.0f32), |best, (i, v)| {
            let sp = v.0.length();
            if sp > best.1 { (i, sp) } else { best }
        });
        let lo = peak.saturating_sub(1);
        let hi = (peak + 2).min(recent.len());
        let n = (hi - lo) as f32;
        let lin = recent[lo..hi].iter().map(|v| v.0).sum::<Vec3>() / n;
        let ang = recent[lo..hi].iter().map(|v| v.1).sum::<Vec3>() / n;
        (lin, ang)
    }
}

/// α for an exponential filter with time constant `tau` (frame-rate independent).
pub fn alpha(dt: f32, tau: f32) -> f32 {
    1.0 - (-dt.clamp(0.0, 0.05) / tau.max(1e-4)).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_euro_steadies_jitter_but_follows_motion() {
        let mut f = OneEuro::new(1.0, 8.0, 1.0);
        let dt = 1.0 / 90.0;
        let mut worst = 0.0f32;
        for i in 0..200 {
            let jitter = Vec3::new(if i % 2 == 0 { 0.003 } else { -0.003 }, 0.0, 0.0);
            let out = f.filter(Vec3::new(0.0, 1.0, 0.5) + jitter, dt);
            if i > 50 {
                worst = worst.max((out.x).abs());
            }
        }
        assert!(worst < 0.002, "3 mm jitter damped to {worst}");
        // A fast 30 cm move is followed within a few frames.
        let mut out = Vec3::ZERO;
        for _ in 0..12 {
            out = f.filter(Vec3::new(0.3, 1.0, 0.5), dt);
        }
        assert!((out.x - 0.3).abs() < 0.03, "follows fast motion: {}", out.x);
    }

    #[test]
    fn throw_uses_the_peak_not_the_release_frame() {
        let mut h = MotionHistory::default();
        let mut x = 0.0f32;
        // Accelerate to 6 m/s, then decelerate in the last two frames before release.
        let speeds = [1.0, 3.0, 5.0, 6.0, 6.0, 4.0, 2.0];
        let mut t = 0.0;
        h.push(t, Vec3::ZERO, Vec3::ZERO);
        for s in speeds {
            t += 1.0 / 90.0;
            x += s / 90.0;
            h.push(t, Vec3::new(0.0, 0.0, x), Vec3::ZERO);
        }
        let (v, _) = h.throw_velocity();
        assert!(v.z > 5.0, "peak-based: {v:?}");
    }
}
