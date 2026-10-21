//! Converting a game's own coordinates into the protocol's (left-handed metres, X right, Y up,
//! Z forward) and back. Games that already use that convention (4A, Unity) use
//! [`Basis::IDENTITY`].

use glam::{Mat3, Quat, Vec3};

#[derive(Clone, Copy, Debug)]
pub struct Basis {
    /// Canonical axes from game axes (a rotation or a reflection).
    to_canonical: Mat3,
    /// Metres per game unit.
    meters_per_unit: f32,
}

impl Basis {
    pub const IDENTITY: Basis = Basis { to_canonical: Mat3::IDENTITY, meters_per_unit: 1.0 };

    /// A right-handed Z-up game whose X is east and Y is north (Bethesda's Creation Engine,
    /// Source): canonical = (x, z, y). Skyrim and Fallout use 70 units per metre... roughly;
    /// pass the game's own figure.
    pub fn z_up_right_handed(units_per_meter: f32) -> Basis {
        Basis {
            to_canonical: Mat3::from_cols(Vec3::X, Vec3::Z, Vec3::Y),
            meters_per_unit: 1.0 / units_per_meter,
        }
    }

    /// A left-handed Z-up game with X forward and Y right (Unreal): canonical = (y, z, x).
    pub fn unreal(units_per_meter: f32) -> Basis {
        Basis {
            to_canonical: Mat3::from_cols(Vec3::Z, Vec3::X, Vec3::Y),
            meters_per_unit: 1.0 / units_per_meter,
        }
    }

    pub fn point_from_game(&self, p: Vec3) -> Vec3 {
        self.to_canonical * p * self.meters_per_unit
    }

    pub fn point_to_game(&self, p: Vec3) -> Vec3 {
        self.to_canonical.transpose() * p / self.meters_per_unit
    }

    pub fn dir_from_game(&self, d: Vec3) -> Vec3 {
        self.to_canonical * d
    }

    pub fn dir_to_game(&self, d: Vec3) -> Vec3 {
        self.to_canonical.transpose() * d
    }

    /// A game rotation matrix (columns = the game's rotated axes) into canonical space.
    pub fn rot_from_game(&self, m: Mat3) -> Quat {
        Quat::from_mat3(&(self.to_canonical * m * self.to_canonical.transpose()))
    }

    /// A canonical rotation as a game rotation matrix.
    pub fn rot_to_game(&self, q: Quat) -> Mat3 {
        self.to_canonical.transpose() * Mat3::from_quat(q) * self.to_canonical
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn z_up_round_trips_and_keeps_meaning() {
        let b = Basis::z_up_right_handed(70.0);
        let up_game = Vec3::new(0.0, 0.0, 70.0);
        assert!(b.point_from_game(up_game).abs_diff_eq(Vec3::Y, 1e-6), "game up is canonical up");
        let p = Vec3::new(140.0, -35.0, 7.0);
        assert!(b.point_to_game(b.point_from_game(p)).abs_diff_eq(p, 1e-4));
        // A game yaw about its up axis is a rotation about canonical up.
        let game_yaw = Mat3::from_rotation_z(0.6);
        let north_game = Vec3::Y;
        let q = b.rot_from_game(game_yaw);
        let turned = b.dir_from_game(game_yaw * north_game);
        assert!((q * b.dir_from_game(north_game)).abs_diff_eq(turned, 1e-5));
        assert!(b.rot_to_game(q).abs_diff_eq(game_yaw, 1e-5));
    }
}
