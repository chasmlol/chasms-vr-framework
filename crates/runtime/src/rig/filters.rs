//! Small signal filters: the One-Euro filter (two-handed aim steadying) and an exponential
//! smoothing factor. (Throws have their own estimator, `throwing`.)

use glam::Vec3;

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
}
