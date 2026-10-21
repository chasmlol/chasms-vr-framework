//! Conversions between protocol types and glam, and the camera maths adapters need.

use glam::{Mat4, Quat, Vec3, Vec4};
use rustvr_protocol::{Fov, Pose};

pub fn v3(a: [f32; 3]) -> Vec3 {
    Vec3::from_array(a)
}

pub fn quat(a: [f32; 4]) -> Quat {
    let q = Quat::from_array(a);
    if q.length_squared() < 1e-12 { Quat::IDENTITY } else { q.normalize() }
}

pub fn pose_pos(p: &Pose) -> Vec3 {
    v3(p.pos)
}

pub fn pose_rot(p: &Pose) -> Quat {
    quat(p.rot)
}

pub fn pose(pos: Vec3, rot: Quat) -> Pose {
    Pose { pos: pos.to_array(), valid: 1, rot: rot.to_array() }
}

pub fn mat(a: &[f32; 16]) -> Mat4 {
    Mat4::from_cols_array(a)
}

/// Yaw (radians) of a direction: 0 = +Z, positive towards +X.
pub fn yaw_of_dir(d: Vec3) -> f32 {
    d.x.atan2(d.z)
}

pub fn yaw_of_quat(q: Quat) -> f32 {
    yaw_of_dir(q * Vec3::Z)
}

/// Wraps an angle to (-pi, pi].
pub fn wrap_angle(a: f32) -> f32 {
    let t = std::f32::consts::TAU;
    let r = a.rem_euclid(t);
    if r > std::f32::consts::PI { r - t } else { r }
}

/// The view matrix of a camera at `pos` with orientation `rot` (X right, Y up, Z forward).
pub fn view_matrix(pos: Vec3, rot: Quat) -> Mat4 {
    (Mat4::from_translation(pos) * Mat4::from_quat(rot)).inverse()
}

/// Left-handed off-centre perspective with D3D depth (0 near, 1 far), from FOV tangents.
/// Same as `XMMatrixPerspectiveOffCenterLH(l*n, r*n, d*n, u*n, n, f)`.
pub fn projection(fov: &Fov, near: f32, far: f32) -> Mat4 {
    let w = fov.right - fov.left;
    let h = fov.up - fov.down;
    let range = far / (far - near);
    Mat4::from_cols(
        Vec4::new(2.0 / w, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 2.0 / h, 0.0, 0.0),
        Vec4::new(-(fov.left + fov.right) / w, -(fov.up + fov.down) / h, range, 1.0),
        Vec4::new(0.0, 0.0, -range * near, 0.0),
    )
}

/// Reversed-Z variant (1 near, 0 far), which many modern engines use.
pub fn projection_reversed(fov: &Fov, near: f32, far: f32) -> Mat4 {
    let w = fov.right - fov.left;
    let h = fov.up - fov.down;
    let range = near / (near - far);
    Mat4::from_cols(
        Vec4::new(2.0 / w, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 2.0 / h, 0.0, 0.0),
        Vec4::new(-(fov.left + fov.right) / w, -(fov.up + fov.down) / h, range, 1.0),
        Vec4::new(0.0, 0.0, -range * far, 0.0),
    )
}

/// A symmetric FOV from a vertical angle and an aspect ratio.
pub fn symmetric_fov(vfov_deg: f32, aspect: f32) -> Fov {
    let t = (vfov_deg.to_radians() * 0.5).tan();
    Fov { left: -t * aspect, right: t * aspect, up: t, down: -t }
}

/// The union of both eyes' FOVs plus a margin: one frustum that culls correctly for both.
pub fn combined_fov(a: &Fov, b: &Fov, margin: f32) -> Fov {
    Fov {
        left: a.left.min(b.left) - margin,
        right: a.right.max(b.right) + margin,
        up: a.up.max(b.up) + margin,
        down: a.down.min(b.down) - margin,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_maps_frustum_edges() {
        let fov = Fov { left: -1.3, right: 1.05, up: 1.1, down: -1.2 };
        let p = projection(&fov, 0.1, 100.0);
        let clip = |v: Vec3| {
            let c = p * v.extend(1.0);
            c.truncate() / c.w
        };
        let near_right = clip(Vec3::new(1.05 * 0.1, 0.0, 0.1));
        assert!((near_right.x - 1.0).abs() < 1e-4 && near_right.z.abs() < 1e-4);
        let far_left_top = clip(Vec3::new(-1.3 * 100.0, 1.1 * 100.0, 100.0));
        assert!((far_left_top.x + 1.0).abs() < 1e-3 && (far_left_top.y - 1.0).abs() < 1e-3 && (far_left_top.z - 1.0).abs() < 1e-4);
        let r = projection_reversed(&fov, 0.1, 100.0);
        let c = r * Vec4::new(0.0, 0.0, 0.1, 1.0);
        assert!((c.z / c.w - 1.0).abs() < 1e-4, "reversed: near maps to 1");
    }

    #[test]
    fn view_matrix_inverts_the_camera() {
        let pos = Vec3::new(1.0, 2.0, 3.0);
        let rot = Quat::from_rotation_y(0.7) * Quat::from_rotation_x(0.2);
        let v = view_matrix(pos, rot);
        let ahead = pos + rot * Vec3::Z * 5.0;
        let in_view = v.transform_point3(ahead);
        assert!(in_view.abs_diff_eq(Vec3::new(0.0, 0.0, 5.0), 1e-4));
    }

    #[test]
    fn yaw_conventions() {
        assert!(yaw_of_dir(Vec3::Z).abs() < 1e-6);
        assert!((yaw_of_dir(Vec3::X) - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert!((yaw_of_quat(Quat::from_rotation_y(0.5)) - 0.5).abs() < 1e-5);
        assert!((wrap_angle(3.5) - (3.5 - std::f32::consts::TAU)).abs() < 1e-6);
    }
}
