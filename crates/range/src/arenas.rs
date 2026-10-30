//! The range's melee areas (`docs/MELEE.md`), all in the one room so a session can walk between
//! them, each with a start of its own (`--arena <name>`, or `rustvr-play --wait -- --arena <name>`):
//!
//! * `range`: where the range always starts. The training dummies stand ahead, and the melee rack
//!   at your left (behind the throwables' table) holds a weapon of each kind.
//! * `rack`: at the melee rack, facing it.
//! * `arena`: the right side of the room past the near crate: armed enemies, one of each stance
//!   (sword, sword and shield, two swords, a two-handed weapon), who come at you once you step in;
//!   when they're all down for good, the next wave comes, harder.
//! * `bench`: the far left: dummies to stab and skewer (two one behind the other), others in
//!   leather, chainmail and plate, and a wooden post.
//! * `climb`: the far right corner: a block taller than you to climb onto with the hands.
//!
//! The areas are fixed in the room (clear of the firing lane and its rails, the targets and the
//! crates), so the scenarios that shoot down the lane don't see them.

use glam::{Quat, Vec3};

/// The melee rack: a table at your left hand, behind the throwables' table.
pub const RACK: (Vec3, Vec3) = (Vec3::new(-3.3, 0.0, -0.6), Vec3::new(-1.3, 0.85, 0.12));
/// The arena's floor (min, max), right of the firing lane's rail.
pub const ARENA: (Vec3, Vec3) = (Vec3::new(3.4, 0.0, 10.2), Vec3::new(9.6, 0.0, 16.8));
/// The bench: left of the lane, past the crate.
pub const BENCH: (Vec3, Vec3) = (Vec3::new(-9.6, 0.0, 15.2), Vec3::new(-3.4, 0.0, 24.6));
/// The block to climb (min, max).
pub const CLIMB_BLOCK: (Vec3, Vec3) = (Vec3::new(7.4, 0.0, 21.4), Vec3::new(9.9, 2.6, 24.9));
/// A wooden post on the bench, to chop and stab (min, max).
pub const POST: (Vec3, Vec3) = (Vec3::new(-4.3, 0.0, 22.6), Vec3::new(-4.0, 1.9, 22.9));

/// The areas: (name, where the player's feet start, which way they face (yaw, radians)).
pub fn starts() -> [(&'static str, Vec3, f32); 5] {
    [
        ("range", Vec3::ZERO, 0.0),
        // At the melee rack, facing it.
        ("rack", Vec3::new(-2.2, 0.0, 0.62), std::f32::consts::PI),
        ("arena", Vec3::new(6.4, 0.0, 10.8), 0.0),
        ("bench", Vec3::new(-6.4, 0.0, 17.2), 0.0),
        ("climb", Vec3::new(8.6, 0.0, 19.6), 0.0),
    ]
}

/// Where area `name` starts (feet, yaw).
pub fn start(name: &str) -> Option<(Vec3, f32)> {
    starts().into_iter().find(|(n, ..)| *n == name).map(|(_, at, yaw)| (at, yaw))
}

/// The areas' furniture (min, max, colour): drawn, solid to the runtime's hands and what they hold,
/// and stood on.
pub fn furniture() -> Vec<(Vec3, Vec3, [u8; 4])> {
    let wood = [110, 85, 60, 255];
    let stone = [125, 122, 118, 255];
    vec![(RACK.0, RACK.1, wood), (CLIMB_BLOCK.0, CLIMB_BLOCK.1, stone), (POST.0, POST.1, [140, 104, 66, 255])]
}

/// Whether `p` (world) is in the arena (its floor, any height).
pub fn in_arena(p: Vec3) -> bool {
    p.x >= ARENA.0.x && p.x <= ARENA.1.x && p.z >= ARENA.0.z && p.z <= ARENA.1.z
}

/// The floor's height under `p` (the climbing block's top on it, else 0).
pub fn floor_under(p: Vec3, radius: f32) -> f32 {
    let (lo, hi) = CLIMB_BLOCK;
    let on = p.x + radius > lo.x && p.x - radius < hi.x && p.z + radius > lo.z && p.z - radius < hi.z;
    if on && p.y >= hi.y - 0.3 { hi.y } else { 0.0 }
}

/// Facing the player's start in the arena from `at`.
pub fn facing(at: Vec3, to: Vec3) -> Quat {
    let d = to - at;
    Quat::from_rotation_y(d.x.atan2(d.z))
}
