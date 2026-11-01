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
//!   leather, chainmail and plate, a wooden post, and a table at your left with a mace, a sword and
//!   a dagger.
//! * `climb`: the far right corner: a block as tall as you to climb onto with the hands: grip its
//!   top edge, pull yourself up, push down on it, and let go to step up onto it.
//! * `duel`: the left side of the room before the bench: a ground with one duelist (`crate::duel`)
//!   to fight by the duel's rules (the range asks for them while you're on it), and a table at your
//!   left with a sword and a longsword. It's large (six metres by twelve), so stepping about in a
//!   play area or walking back with the stick doesn't take you off it.
//!
//! The areas are fixed in the room (clear of the firing lane and its rails, the targets and the
//! crates), so the scenarios that shoot down the lane don't see them.

use glam::Vec3;

/// The melee rack: a table at your left hand, behind the throwables' table.
pub const RACK: (Vec3, Vec3) = (Vec3::new(-3.3, 0.0, -0.6), Vec3::new(-1.3, 0.85, 0.12));
/// The arena's floor (min, max), right of the firing lane's rail.
pub const ARENA: (Vec3, Vec3) = (Vec3::new(3.4, 0.0, 10.2), Vec3::new(9.6, 0.0, 16.8));
/// The bench: left of the lane, past the crate.
pub const BENCH: (Vec3, Vec3) = (Vec3::new(-9.6, 0.0, 15.2), Vec3::new(-3.4, 0.0, 24.6));
/// The block to climb (min, max).
pub const CLIMB_BLOCK: (Vec3, Vec3) = (Vec3::new(7.4, 0.0, 21.4), Vec3::new(9.9, 2.0, 24.9));
/// A table of weapons at the arena's start, at your left (min, max).
pub const ARENA_TABLE: (Vec3, Vec3) = (Vec3::new(4.6, 0.0, 11.0), Vec3::new(5.8, 0.85, 12.2));
/// A table of weapons at the bench's start, at your left (min, max).
pub const BENCH_TABLE: (Vec3, Vec3) = (Vec3::new(-7.3, 0.0, 18.4), Vec3::new(-6.7, 0.85, 19.6));
/// A wooden post on the bench, to chop and stab (min, max).
pub const POST: (Vec3, Vec3) = (Vec3::new(-4.3, 0.0, 22.6), Vec3::new(-4.0, 1.9, 22.9));
/// The duel ground's floor (min, max): the room's left side from the armory bench to before the
/// left target, left of the long table and the melee rack.
pub const DUEL: (Vec3, Vec3) = (Vec3::new(-9.4, 0.0, -0.4), Vec3::new(-3.4, 0.0, 12.0));
/// Where the player starts in it, and where its duelist stands waiting (facing the player).
pub const DUEL_START: Vec3 = Vec3::new(-7.3, 0.0, 4.3);
pub const DUELIST_AT: Vec3 = Vec3::new(-7.3, 0.0, 8.0);
/// A table of weapons at the ring's start, at your left hand (min, max).
pub const DUEL_TABLE: (Vec3, Vec3) = (Vec3::new(-8.3, 0.0, 4.3), Vec3::new(-7.55, 0.85, 5.1));

/// Whether `p` stands in the duel ring.
pub fn in_duel(p: Vec3) -> bool {
    let (lo, hi) = DUEL;
    (lo.x..=hi.x).contains(&p.x) && (lo.z..=hi.z).contains(&p.z)
}

/// The areas: (name, where the player's feet start, which way they face (yaw, radians)).
pub fn starts() -> [(&'static str, Vec3, f32); 6] {
    [
        ("range", Vec3::ZERO, 0.0),
        // At the melee rack, facing it.
        ("rack", Vec3::new(-2.2, 0.0, 0.62), std::f32::consts::PI),
        ("arena", Vec3::new(6.0, 0.0, 10.8), 0.0),
        // A step from the armoured dummies (chainmail ahead, leather at your left, plate at your
        // right), the bench's table at your left hand.
        ("bench", Vec3::new(-6.1, 0.0, 18.9), 0.0),
        ("climb", Vec3::new(8.6, 0.0, 20.95), 0.0),
        // Facing the duelist across the ring, the table at your left hand.
        ("duel", DUEL_START, 0.0),
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
    vec![(RACK.0, RACK.1, wood), (ARENA_TABLE.0, ARENA_TABLE.1, wood), (BENCH_TABLE.0, BENCH_TABLE.1, wood), (CLIMB_BLOCK.0, CLIMB_BLOCK.1, stone), (POST.0, POST.1, [140, 104, 66, 255]), (DUEL_TABLE.0, DUEL_TABLE.1, wood)]
}

/// The floor's height under `p` (the climbing block's top on it, else 0).
pub fn floor_under(p: Vec3, radius: f32) -> f32 {
    let (lo, hi) = CLIMB_BLOCK;
    let on = p.x + radius > lo.x && p.x - radius < hi.x && p.z + radius > lo.z && p.z - radius < hi.z;
    if on && p.y >= hi.y - 0.3 { hi.y } else { 0.0 }
}

/// Feet this far below a top, and this near its edge outside it, step up onto it when the hands
/// let go of a climb (m).
const MANTLE_BELOW: f32 = 1.0;
const MANTLE_OUT: f32 = 0.5;

/// Where feet at `p` step up to when the hands let go of a climb: onto the block's top, just in
/// from its edge, if they're near enough below it.
pub fn mantle(p: Vec3) -> Option<Vec3> {
    let (lo, hi) = CLIMB_BLOCK;
    let near = p.x > lo.x - MANTLE_OUT && p.x < hi.x + MANTLE_OUT && p.z > lo.z - MANTLE_OUT && p.z < hi.z + MANTLE_OUT;
    if !near || p.y < hi.y - MANTLE_BELOW || p.y > hi.y + 0.05 {
        return None;
    }
    let inside = Vec3::new(p.x.clamp(lo.x + 0.3, hi.x - 0.3), hi.y, p.z.clamp(lo.z + 0.3, hi.z - 0.3));
    Some(inside)
}

