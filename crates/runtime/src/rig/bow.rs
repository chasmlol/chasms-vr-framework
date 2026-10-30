//! Bows ([`OBJ_BOW`]) and the arrows they shoot ([`OBJ_ARROW`]), the way the best-liked VR bows
//! do them (the research is in Oblivion's adapter, `docs/research/archery.md`): an arrow from the
//! quiver, nocked by bringing it to the string, the string drawn back by the hand that nocked it,
//! the bow turning so the arrow points from the string hand through its rest, loosed by letting go.
//!
//! * **Nocking** is magnetic: the arrow's nock within [`NOCK_REACH`] of the string seats it,
//!   eased on over [`NOCK_TIME`]; nothing has to be lined up.
//! * **Both hands aim**: the bow turns about its grip so the line from the string hand through the
//!   arrow's rest is the arrow, eased in over [`ALIGN_TIME`] as the hand takes the string, then
//!   exact; its roll is the bow wrist's own.
//! * **The string stops at full draw**: drawn further, the hand stays on it there (its ghost shows
//!   the controller). Full draw is the bow's own, or as far as the player's arm draws to the chin.
//! * **The arrow leaves exactly along the drawn arrow**, with the draw's share of full draw as its
//!   strength: the game fires its own arrow ([`CE_LOOSE`]). Short of [`MIN_DRAW`], letting go leaves
//!   the arrow on the string.
//!
//! Bow space is the guns' (+Z where the arrow flies, +Y the upper limb, X across), so the left hand
//! holds the mirror image of the right's hold; the arrow rests on the bow hand's side.

use glam::{Quat, Vec3};
use rustvr_protocol::*;

/// How near the string's nock point an arrow's nock must come to go on (metres).
pub const NOCK_REACH: f32 = 0.09;
/// From this near the string (metres), an arrow in the hand turns to lie along the bow, all the
/// way by [`NOCK_REACH`]: the Longbow's rotation blend starts as near.
pub const ALIGN_REACH: f32 = 0.15;
/// An arrow goes onto the string over this long (s).
pub const NOCK_TIME: f64 = 0.12;
/// The bow turns to the string hand over this long as the hand takes the string (s).
pub const ALIGN_TIME: f64 = 0.15;
/// Let go short of this share of the full draw, the arrow stays on the string: a short pull is no
/// shot (it stops Elven Assassin's "gatling" of instant arrows and flicks).
pub const MIN_DRAW: f32 = 0.2;
/// A tick in both hands each time the string moves this far (metres), the Longbow's centimetre.
pub const TICK: f32 = 0.01;
/// At full draw (this share of it and past), a low strain in the hands now and then.
pub const STRAIN_AT: f32 = 0.98;
/// Where the fingers hold an arrow's nock and the string, in the controller's grip pose: at the
/// top of the fist, where the index finger meets the thumb.
pub const PINCH: Vec3 = Vec3::new(0.0, 0.0, 0.035);
/// The full draw a player's arm reaches, drawing to the chin with the bow arm straight: this
/// share of the arm's length from the grip to the hand.
const ARM_DRAW: f32 = 0.92;

/// A bow's string and what's on it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BowState {
    /// The arrow on the string (its object), and the hand drawing it (None: resting on the
    /// string).
    pub nocked: Option<u32>,
    pub drawing: Option<usize>,
    /// When the arrow went on, and when the hand last took the string (each eases in from then).
    pub nocked_at: f64,
    pub drawn_at: f64,
    /// Where the arrow was as it went on (bow space), for easing it onto the string.
    pub nocked_from: Option<(Vec3, Quat)>,
    /// How far the string is drawn back from rest (metres), and how far it was at the last tick.
    pub draw: f32,
    pub ticked: f32,
    /// The full draw now (metres from rest).
    pub full: f32,
    /// The last shot: when, and how far the bow was bent then (its limbs spring back from there).
    pub loosed: Option<(f64, f32)>,
    /// Arrows loosed so far.
    pub shots: u32,
    /// When the next strain at full draw is felt.
    pub next_strain: f64,
}

impl BowState {
    /// How far the bow is bent (its state's `slide`): the string's draw as a share of the bow's own
    /// travel, so the string drawn is where the hand has it; after a shot, its limbs springing back
    /// as the game animates them (`spring`, from the bend it was loosed at).
    pub fn bend(&self, d: &ObjectDesc, spring: Option<&BowDesc>, now: f64) -> f32 {
        let drawn = if d.handle_travel > 1e-4 { self.draw / d.handle_travel } else { 0.0 };
        match (self.loosed, spring) {
            (Some((at, from)), Some(s)) if self.nocked.is_none() || self.draw < 1e-3 => {
                let t = (now - at) as f32;
                let k = s.spring_at(t);
                if k != 0.0 || t < 0.05 { k * from } else { drawn }
            }
            _ => drawn,
        }
    }

    /// The draw as a share of the full draw: the strength a release now looses the arrow with.
    pub fn strength(&self) -> f32 {
        if self.full > 1e-4 { (self.draw / self.full).clamp(0.0, 1.0) } else { 0.0 }
    }
}

/// Where the arrow rests on the bow for the bow hand `hand`: on that hand's side of it (the
/// description's `muzzle` is the right hand's; the left's is the mirror image).
pub fn rest(d: &ObjectDesc, hand: usize) -> Vec3 {
    let m = Vec3::from_array(d.muzzle);
    if hand == 0 { Vec3::new(-m.x, m.y, m.z) } else { m }
}

/// The string's nock point drawn back `draw` metres from rest (bow space).
pub fn nock(d: &ObjectDesc, draw: f32) -> Vec3 {
    Vec3::from_array(d.handle) - Vec3::Z * draw
}

/// The full draw for an arm `arm` metres long from shoulder to grip (0: unknown): the bow's own
/// travel, or (`fit`) less when the arm can't draw that far: drawn to the chin with the bow arm
/// straight, the string hand is about the arm's length from the grip.
pub fn full_draw(d: &ObjectDesc, arm: f32, fit: bool) -> f32 {
    let own = d.handle_travel.max(0.05);
    if !fit || arm < 0.3 {
        return own;
    }
    let brace = Vec3::from_array(d.handle).length();
    (arm * ARM_DRAW - brace).clamp(own * 0.5, own)
}

/// Where an arrow (`arrow`: its description) lies on the bow with the string drawn `draw`, bow
/// space: its nock on the string, pointing through the rest.
pub fn nocked_pose(d: &ObjectDesc, arrow: &ObjectDesc, hand: usize, draw: f32) -> (Vec3, Quat) {
    let n = nock(d, draw);
    let dir = (rest(d, hand) - n).normalize_or(Vec3::Z);
    let rot = Quat::from_rotation_arc(Vec3::Z, dir);
    (n - rot * Vec3::from_array(arrow.mag_well), rot)
}

/// The bow held at `alone` (where the bow hand alone puts it, world) turned about its hold at
/// `pivot` (bow space) so the arrow points from the string hand's pinch `pinch` (world) through
/// its rest: the string is drawn back to the hand, up to `full`. Returns the bow's pose and the
/// draw. A hand ahead of the string (between it and the bow) doesn't turn it.
pub fn aim(d: &ObjectDesc, hand: usize, alone: (Vec3, Quat), pivot: Vec3, pinch: Vec3, full: f32) -> ((Vec3, Quat), f32) {
    let g = alone.0 + alone.1 * pivot;
    let r = rest(d, hand);
    let at_rest = (r - nock(d, 0.0)).length();
    let mut rot = alone.1;
    let mut draw = 0.0;
    // (The nock's distance from the grip changes with the draw: a few rounds settle both.)
    for _ in 0..3 {
        let cur = rot * (nock(d, draw) - pivot);
        let want = pinch - g;
        if cur.length() < 0.02 || want.length() < 0.02 {
            break;
        }
        rot = (Quat::from_rotation_arc(cur.normalize(), want.normalize()) * rot).normalize();
        let rw = g + rot * (r - pivot);
        draw = ((pinch - rw).length() - at_rest).clamp(0.0, full);
    }
    // Ahead of the string the hand isn't drawing it: the bow turns to it only as it comes back
    // past the string's rest (else a hand near the grip would spin the bow about).
    let behind = (pinch - g).length() / (nock(d, 0.0) - pivot).length().max(0.05);
    let k = ((behind - 0.6) / 0.4).clamp(0.0, 1.0);
    let k = k * k * (3.0 - 2.0 * k);
    let rot = alone.1.slerp(rot, k).normalize();
    ((g - rot * pivot, rot), if k < 1.0 { draw * k } else { draw })
}

/// A cheap repeatable number in 0..1 for `seed` (the strain's random gaps).
pub fn noise(seed: u64) -> f32 {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 29;
    (x >> 40) as f32 / (1u64 << 24) as f32
}

/// How an arrow out of the quiver sits in the hand (its pose from the controller's grip pose): its
/// nock between the index finger and the thumb ([`PINCH`]), pointing where the controller points
/// (`aim`: the controller's aim pose turned into its grip pose), the way an archer holds an arrow
/// by the nock and the Longbow puts it in the hand (its nock at the hand, its shaft out ahead of
/// the fist). So it points down into a quiver as it's taken, and lies level, out ahead along the
/// forearm, as the arm comes down to the string. (Along the fist's axis it stood up out of the
/// fist like a dagger.)
pub fn held_arrow(d: &ObjectDesc, aim: Quat) -> (Vec3, Quat) {
    (PINCH - aim * Vec3::from_array(d.mag_well), aim)
}
