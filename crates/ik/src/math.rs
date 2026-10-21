//! Small maths shared by the solver: filters, yaw helpers and frame construction.

use glam::{Mat3, Quat, Vec3};

pub const UP: Vec3 = Vec3::Y;

/// Frame-rate independent smoothing factor, α = 1 − e^(−dt/τ). Every filter in the crate goes
/// through this (FRIK's 2026 fix: per-frame factors doubled the lag under reprojection).
pub fn smoothing(dt: f32, tau: f32) -> f32 {
    if tau <= 0.0 {
        return 1.0;
    }
    1.0 - (-dt / tau).exp()
}

/// The frame time the filters see: non-finite or negative becomes 0, and long frames are capped
/// so one hitch can't fling a filter (FRIK clamps to 3 nominal frames).
pub fn clamp_dt(dt: f32, max_dt: f32) -> f32 {
    if dt.is_finite() { dt.clamp(0.0, max_dt) } else { 0.0 }
}

/// Wraps an angle to (−π, π].
pub fn wrap_angle(a: f32) -> f32 {
    let w = (a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    if w <= -std::f32::consts::PI { w + std::f32::consts::TAU } else { w }
}

/// Yaw of a direction: 0 along +Z, positive towards +X (a turn to the right).
pub fn yaw_of(v: Vec3) -> f32 {
    v.x.atan2(v.z)
}

/// Horizontal unit direction for a yaw.
pub fn yaw_dir(yaw: f32) -> Vec3 {
    Vec3::new(yaw.sin(), 0.0, yaw.cos())
}

pub fn yaw_rot(yaw: f32) -> Quat {
    Quat::from_rotation_y(yaw)
}

pub fn flat(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z)
}

pub fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A rotation whose +Y is `y` and whose +Z is as close to `z_hint` as possible (X = Y × Z, the
/// protocol's convention). Falls back to `fallback_z` when the hint is parallel to `y`.
pub fn frame_yz(y: Vec3, z_hint: Vec3, fallback_z: Vec3) -> Quat {
    let y = y.normalize_or(UP);
    let z = z_hint
        .reject_from(y)
        .try_normalize()
        .or_else(|| fallback_z.reject_from(y).try_normalize())
        .unwrap_or_else(|| y.any_orthonormal_vector());
    let x = y.cross(z);
    Quat::from_mat3(&Mat3::from_cols(x, y, z)).normalize()
}

/// A unit quaternion from possibly bad input (not finite, or far from unit length).
pub fn sanitize_rot(q: Quat) -> Quat {
    let len = q.length();
    if q.is_finite() && len > 1e-3 { q / len } else { Quat::IDENTITY }
}

pub fn sanitize_pos(p: Vec3, fallback: Vec3) -> Vec3 {
    if p.is_finite() { p } else { fallback }
}

/// Signed angle from `a` to `b` around `axis` (all three need not be normalized; `a` and `b`
/// are projected onto the plane normal to `axis`).
pub fn signed_angle(a: Vec3, b: Vec3, axis: Vec3) -> f32 {
    let axis = axis.normalize_or_zero();
    let a = a.reject_from(axis);
    let b = b.reject_from(axis);
    a.cross(b).dot(axis).atan2(a.dot(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoothing_is_frame_rate_independent() {
        // Two 90 Hz frames move a filter exactly as far as one 45 Hz frame.
        let tau = 0.05;
        let two = 1.0 - (1.0 - smoothing(1.0 / 90.0, tau)).powi(2);
        let one = smoothing(2.0 / 90.0, tau);
        assert!((two - one).abs() < 1e-6);
        assert_eq!(smoothing(0.0, tau), 0.0);
        assert_eq!(smoothing(0.1, 0.0), 1.0);
    }

    #[test]
    fn dt_is_clamped() {
        assert_eq!(clamp_dt(f32::NAN, 0.03), 0.0);
        assert_eq!(clamp_dt(-1.0, 0.03), 0.0);
        assert_eq!(clamp_dt(10.0, 0.03), 0.03);
    }

    #[test]
    fn yaw_convention_matches_glam() {
        // Positive yaw turns +Z towards +X in this left-handed space.
        let q = yaw_rot(0.5);
        assert!((yaw_of(q * Vec3::Z) - 0.5).abs() < 1e-6);
        assert!((yaw_dir(0.5) - q * Vec3::Z).length() < 1e-6);
        assert!((wrap_angle(3.0 * std::f32::consts::PI) - std::f32::consts::PI).abs() < 1e-5);
    }

    #[test]
    fn frames_follow_the_protocol_axes() {
        let q = frame_yz(Vec3::X, Vec3::Z, Vec3::Y);
        assert!((q * Vec3::Y - Vec3::X).length() < 1e-6);
        assert!((q * Vec3::Z - Vec3::Z).length() < 1e-6);
        assert!((q * Vec3::X - Vec3::X.cross(Vec3::Z)).length() < 1e-6);
        // A hint parallel to Y uses the fallback.
        let q = frame_yz(Vec3::Y, Vec3::Y, Vec3::Z);
        assert!((q * Vec3::Z - Vec3::Z).length() < 1e-6);
    }
}
