//! The range's bow: the game side of `rustvr_runtime::rig::bow`, as a game with bows gives them
//! (Oblivion's adapter, its `archery.rs`). The bow starts on your back over the left shoulder; with
//! it in one hand, the other takes arrows from the runtime's quiver over its shoulder or at its hip.
//! The runtime nocks, draws and looses them; the range registers the bow and the arrows it hands
//! out, and draws them where it's told, the bow bent as far as its string is drawn. A loosed arrow
//! flies nowhere: the range has no arrows of its own to fly (a game fires its own on `CE_LOOSE`).

use bytemuck::Zeroable;
use glam::{Mat3, Mat4, Quat, Vec3};
use rustvr_protocol::*;

use crate::physical::{DrawItem, Kind};

pub const BOW_ID: u32 = 5000;
pub const ARROW_ID: u32 = 5001;
/// What the bow shoots: a kind of ammunition none of the guns take.
const ARROW_AMMO: u32 = 0x0A77_0001;

/// The bow (bow space, the guns': the arrow along +Z, up the limbs +Y): its tips this far up and
/// down, and bent this much further back at full draw (metres).
const TIP: f32 = 0.62;
const TIP_BEND: f32 = 0.1;
/// The riser's half height, where the limbs start.
const RISER: f32 = 0.11;
/// The string's nock point at rest (its brace height behind the grip: the tips are as far back,
/// the string straight between them) and the bow's full draw, as far as a grown archer draws to
/// the chin (the runtime fits it to a shorter arm).
const BRACE: f32 = 0.2;
const TRAVEL: f32 = 0.5;
const ARROW_LENGTH: f32 = 0.75;

const WOOD: [f32; 4] = [0.45, 0.28, 0.14, 1.0];
const STRING: [f32; 4] = [0.85, 0.82, 0.7, 1.0];
const SHAFT: [f32; 4] = [0.7, 0.6, 0.42, 1.0];

/// The controller grip rotation for a fist round a handle along `tube` (little finger to thumb),
/// the back of the hand towards `x` (as Oblivion's adapter makes its bows').
fn fist(tube: Vec3, x: Vec3) -> Quat {
    let z = tube.normalize();
    let x = (x - z * x.dot(z)).normalize();
    Quat::from_mat3(&Mat3::from_cols(x, z.cross(x), z)).normalize()
}

/// The bow (on the back over the left shoulder) and the arrows its quiver hands out.
pub fn objects() -> Vec<(ObjectDesc, Kind)> {
    let mut bow = ObjectDesc::zeroed();
    bow.id = BOW_ID;
    bow.kind = OBJ_BOW;
    bow.flags = OBJF_GRABBABLE | OBJF_PULLABLE | OBJF_HOLSTERABLE;
    bow.spawn_seq = 1;
    // (Its spawn pose is lying flat on the long table, pointing at the targets, should it ever
    // leave the back.)
    bow.spawn_pose = Pose { pos: [-1.35, 0.89, 2.55], valid: 1, rot: Quat::from_rotation_z(std::f32::consts::FRAC_PI_2).to_array() };
    bow.start_slot = 1 + SLOT_BACK_LEFT;
    // Its collision: the riser and the limbs, not the string's side of it (a hand goes there).
    bow.shape = SHAPE_CAPSULE;
    bow.radius = 0.03;
    bow.half_extents = [0.03, TIP - 0.1, 0.03];
    bow.center = [0.0, 0.0, -0.03];
    bow.mass = 0.8;
    // The fist below the arrow's line, thumb up, its top where the arrow crosses; the arrow rests
    // beside the riser on the bow hand's side (the right hand's here).
    bow.grip = [0.0, -0.055, 0.0];
    bow.grip_line = [[0.0, -RISER + 0.01, 0.0], [0.0, -0.04, 0.0]];
    bow.grip_radius = 0.018;
    bow.grip_rot = fist(Vec3::Y, Vec3::X).to_array();
    bow.muzzle = [0.022, 0.0, 0.0];
    bow.handle = [0.0, 0.0, -BRACE];
    bow.handle_travel = TRAVEL;
    bow.ammo_type = ARROW_AMMO;
    set_name(&mut bow.name, "bow");

    let mut arrow = ObjectDesc::zeroed();
    arrow.id = ARROW_ID;
    arrow.kind = OBJ_ARROW;
    arrow.flags = OBJF_TEMPLATE | OBJF_GRABBABLE;
    arrow.shape = SHAPE_BOX;
    // The tip at the origin, the shaft back along -Z; the nock a centimetre in from its back end,
    // the fingers a few more.
    arrow.half_extents = [0.004, 0.004, ARROW_LENGTH * 0.5];
    arrow.center = [0.0, 0.0, -ARROW_LENGTH * 0.5];
    arrow.mass = 0.03;
    arrow.muzzle = [0.0, 0.0, 0.0];
    arrow.mag_well = [0.0, 0.0, -ARROW_LENGTH + 0.01];
    arrow.grip = [0.0, 0.0, -ARROW_LENGTH + 0.04];
    arrow.ammo_type = ARROW_AMMO;
    set_name(&mut arrow.name, "arrow");

    vec![(bow, Kind::Bow), (arrow, Kind::Prop { color: SHAFT })]
}

/// A block from `a` to `b`, `thick` across (bow space, then `model`).
fn rod(model: Mat4, a: Vec3, b: Vec3, thick: f32, color: [f32; 4], glow: f32) -> DrawItem {
    let along = b - a;
    let rot = Quat::from_rotation_arc(Vec3::Y, along.normalize_or(Vec3::Y));
    let m = model * Mat4::from_scale_rotation_translation(Vec3::new(thick, along.length(), thick), rot, (a + b) * 0.5);
    (200, m, color, glow, false)
}

/// The bow drawn at `model`, bent `bend` (its state's `slide`: the string's draw as a share of its
/// full draw): the riser, the limbs bending back with the draw, and the string from their tips to
/// where the hand has it.
pub fn draws(model: Mat4, bend: f32, glow: f32) -> Vec<DrawItem> {
    let bend = bend.clamp(-0.3, 1.2);
    // (Bent, the tips come back and in towards each other.)
    let tip = |up: f32| Vec3::new(0.0, TIP * up * (1.0 - 0.08 * bend), -BRACE - TIP_BEND * bend);
    let limb = |up: f32| Vec3::new(0.0, RISER * up, -0.01);
    let nock = Vec3::new(0.0, 0.0, -BRACE - TRAVEL * bend.max(0.0));
    vec![
        rod(model, limb(-1.0), limb(1.0), 0.035, WOOD, glow),
        rod(model, limb(1.0), tip(1.0), 0.02, WOOD, glow),
        rod(model, limb(-1.0), tip(-1.0), 0.02, WOOD, glow),
        rod(model, tip(1.0), nock, 0.003, STRING, glow),
        rod(model, tip(-1.0), nock, 0.003, STRING, glow),
    ]
}
