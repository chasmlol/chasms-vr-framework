//! The rig driven by scripted controllers: a player standing at a table with a bottle, a pistol and
//! a rifle (with magazine templates), in a room with a floor and a wall.

use super::*;
use bytemuck::Zeroable;
use rustvr_protocol::world::ColBox;

const DT: f32 = 1.0 / 90.0;
const BOTTLE: u32 = 1;
const PISTOL: u32 = 2;
const RIFLE: u32 = 3;
const RIFLE_MAG: u32 = 100;
const FAR_BOTTLE: u32 = 4;

struct Sim {
    rig: Rig,
    s: Settings,
    f: HmdFrame,
    now: f64,
    map: WorldMap,
    table: Box<ObjectTable>,
    out: RigOutput,
    crouched: bool,
    /// Every cue the rig emitted, in order.
    cues: Vec<Cue>,
}

fn obj(id: u32, kind: u32, pos: Vec3, half: [f32; 3], mass: f32) -> ObjectDesc {
    let mut d = ObjectDesc::zeroed();
    d.id = id;
    d.kind = kind;
    d.flags = OBJF_GRABBABLE;
    d.spawn_seq = 1;
    d.spawn_pose = pose(pos, Quat::IDENTITY);
    d.shape = SHAPE_BOX;
    d.half_extents = half;
    d.mass = mass;
    d
}

fn pistol() -> ObjectDesc {
    let mut d = obj(PISTOL, OBJ_WEAPON, Vec3::new(-0.3, 0.87, 0.5), [0.015, 0.07, 0.1], 1.0);
    d.flags |= OBJF_HOLSTERABLE;
    d.center = [0.0, 0.0, 0.05];
    d.muzzle = [0.0, 0.05, 0.16];
    d.sight = [0.0, 0.07, 0.0];
    d.weapon_flags = WPN_PISTOL | WPN_SLIDE_LOCKS | WPN_NEEDS_RACK;
    d.ammo_type = 2; // no template: rounds held internally
    d.capacity = 12;
    d.fire_rate = 10.0;
    d.recoil_pitch = 6.0;
    d
}

fn rifle() -> ObjectDesc {
    let mut d = obj(RIFLE, OBJ_WEAPON, Vec3::new(0.3, 0.87, 0.3), [0.03, 0.07, 0.42], 3.5);
    d.flags |= OBJF_TWO_HANDED | OBJF_HOLSTERABLE;
    d.center = [0.0, 0.0, 0.25];
    d.grip = [0.0, -0.05, 0.0];
    d.foregrip = [0.0, -0.03, 0.4];
    d.mag_well = [0.0, -0.06, 0.15];
    d.muzzle = [0.0, 0.03, 0.66];
    d.sight = [0.0, 0.08, 0.0];
    d.handle = [0.04, 0.03, 0.1];
    d.handle_travel = 0.08;
    d.weapon_flags = WPN_AUTOMATIC | WPN_NEEDS_RACK;
    d.ammo_type = 1;
    d.capacity = 5;
    d.fire_rate = 10.0;
    d.recoil_pitch = 3.0;
    d.recoil_back = 0.01;
    d
}

fn rifle_mag() -> ObjectDesc {
    let mut d = obj(RIFLE_MAG, OBJ_MAGAZINE, Vec3::ZERO, [0.015, 0.06, 0.03], 0.4);
    d.flags |= OBJF_TEMPLATE;
    d.mag_well = [0.0, 0.06, 0.0];
    d.ammo_type = 1;
    d.capacity = 5;
    d.rounds = 5;
    d
}

fn world_box(rig: &mut Rig, id: u32, center: [f32; 3], half: [f32; 3]) {
    let b = ColBox { id, center, half_extents: half, rot: [0.0, 0.0, 0.0, 1.0], ..Default::default() };
    rig.physics.apply_collision(COL_BOX, bytemuck::bytes_of(&b));
}

impl Sim {
    fn new() -> Sim {
        Sim::with(&[])
    }

    /// The standard room, plus `extra` objects.
    fn with(extra: &[ObjectDesc]) -> Sim {
        let mut rig = Rig::default();
        world_box(&mut rig, 1, [0.0, -0.5, 0.0], [20.0, 0.5, 20.0]); // floor
        world_box(&mut rig, 2, [0.0, 0.4, 0.6], [0.6, 0.4, 0.3]); // table, top at 0.8 m
        let mut f = HmdFrame { index: 0, dt: DT, user_present: true, ..Default::default() };
        f.hmd = pose(Vec3::new(0.0, 1.7, 0.0), Quat::IDENTITY);
        for (i, x) in [(0, -0.2), (1, 0.2)] {
            let p = pose(Vec3::new(x, 1.1, 0.2), Quat::IDENTITY);
            f.hands[i].grip = p;
            f.hands[i].aim = p;
            f.hands[i].connected = 1;
        }
        let mut table: Box<ObjectTable> = bytemuck::zeroed_box();
        let descs = [
            obj(BOTTLE, OBJ_PROP, Vec3::new(0.0, 0.9, 0.5), [0.03, 0.1, 0.03], 0.5),
            pistol(),
            rifle(),
            rifle_mag(),
            obj(FAR_BOTTLE, OBJ_PROP, Vec3::new(0.0, 0.1, 3.0), [0.04, 0.1, 0.04], 0.5),
        ];
        for (i, d) in descs.iter().chain(extra).enumerate() {
            table.descs[i] = *d;
        }
        table.count = (descs.len() + extra.len()) as u32;
        table.seq = 1;
        rig.sync(&table);
        let mut sim = Sim { rig, s: Settings::default(), f, now: 0.0, map: WorldMap::IDENTITY, table, out: RigOutput::default(), crouched: false, cues: Vec::new() };
        sim.step(45); // let everything settle on the table
        sim
    }

    fn step(&mut self, n: usize) {
        for _ in 0..n {
            self.f.index += 1;
            self.now += DT as f64;
            let inp = RigInput {
                frame: &self.f,
                map: self.map,
                dt: DT,
                now: self.now,
                settings: &self.s,
                eye_height: 1.7,
                crouched: self.crouched,
                paused: false,
                infinite_ammo: false,
            };
            self.out = self.rig.update(&inp);
            self.cues.extend(self.rig.feedback.iter().map(|f| f.cue));
        }
    }

    fn hand_pos(&self, i: usize) -> Vec3 {
        v3(self.f.hands[i].grip.pos)
    }

    fn set_hand(&mut self, i: usize, p: Vec3) {
        self.f.hands[i].grip.pos = p.to_array();
        self.f.hands[i].aim.pos = p.to_array();
    }

    fn set_rot(&mut self, i: usize, q: Quat) {
        self.f.hands[i].grip.rot = q.to_array();
        self.f.hands[i].aim.rot = q.to_array();
    }

    /// Moves a hand smoothly to `to` over `frames` frames.
    fn move_hand(&mut self, i: usize, to: Vec3, frames: usize) {
        let from = self.hand_pos(i);
        for k in 1..=frames {
            self.set_hand(i, from.lerp(to, k as f32 / frames as f32));
            self.step(1);
        }
    }

    fn grip(&mut self, i: usize, v: f32) {
        self.f.hands[i].squeeze = v;
        self.step(1);
    }

    fn tap(&mut self, i: usize, bit: u32) {
        self.f.hands[i].buttons |= bit;
        self.step(1);
        self.f.hands[i].buttons &= !bit;
        self.step(1);
    }

    fn trigger(&mut self, i: usize, v: f32) {
        self.f.hands[i].trigger = v;
        self.step(1);
    }

    fn pos(&self, id: u32) -> Vec3 {
        self.rig.object_world_pose(id).unwrap().0
    }

    fn rot(&self, id: u32) -> Quat {
        self.rig.object_world_pose(id).unwrap().1
    }

    /// Picks up the rifle from the table with the right hand and lifts it clear.
    fn take_rifle(&mut self) {
        let top = self.pos(RIFLE) + Vec3::new(0.0, 0.12, 0.0);
        self.move_hand(1, top, 30);
        self.grip(1, 1.0);
        assert_eq!(self.rig.held(1), Some(RIFLE), "picked up the rifle");
        self.move_hand(1, Vec3::new(0.2, 1.3, 0.15), 30);
        self.step(20);
    }

    fn well_world(&self) -> Vec3 {
        self.pos(RIFLE) + self.rot(RIFLE) * v3(rifle().mag_well)
    }
}

#[test]
fn hands_follow_the_controllers_in_free_space() {
    let mut s = Sim::new();
    s.move_hand(1, Vec3::new(0.3, 1.3, 0.1), 20);
    let h = v3(s.out.hands[1].pose.pos);
    assert!(h.distance(s.hand_pos(1)) < 0.005, "1:1 in free space: {h:?}");
    assert!(s.out.hands[1].flags & HAND_BLOCKED == 0);
    assert!(s.out.ghosts[1].is_none());
}

#[test]
fn walls_stop_the_hand_and_show_a_ghost_then_the_leash_snaps_it_back() {
    let mut s = Sim::new();
    world_box(&mut s.rig, 3, [0.0, 1.5, -0.4], [2.0, 1.0, 0.05]); // a wall behind the player
    s.move_hand(1, Vec3::new(0.2, 1.3, -0.2), 15);
    s.move_hand(1, Vec3::new(0.2, 1.3, -0.6), 15); // push through it
    let h = v3(s.out.hands[1].pose.pos);
    assert!(h.z > -0.38, "the wall holds the hand back: {h:?}");
    assert!(s.out.hands[1].flags & HAND_BLOCKED != 0);
    assert!(s.out.ghosts[1].is_some(), "a ghost hand shows where the controller is");
    assert!(s.rig.debug.blocked_frames > 0);
    s.move_hand(1, Vec3::new(0.2, 1.3, -1.3), 10); // far past the leash
    s.step(2);
    let h = v3(s.out.hands[1].pose.pos);
    assert!(h.distance(s.hand_pos(1)) < 0.05, "snapped back to the controller: {h:?}");
}

#[test]
fn pick_up_carry_and_drop() {
    let mut s = Sim::new();
    let rest = s.pos(BOTTLE);
    assert!((rest.y - 0.9).abs() < 0.02, "the bottle stands on the table: {rest:?}");
    s.move_hand(0, rest + Vec3::new(0.0, 0.15, 0.0), 30);
    assert!(s.out.hand_targeting[0], "hovering the bottle");
    s.grip(0, 1.0);
    assert_eq!(s.rig.held(0), Some(BOTTLE));
    let offset = s.pos(BOTTLE) - s.hand_pos(0);
    s.move_hand(0, Vec3::new(-0.3, 1.4, 0.1), 40);
    s.step(10);
    let carried = s.pos(BOTTLE) - s.hand_pos(0);
    assert!(carried.distance(offset) < 0.02, "held where it was grabbed: {offset:?} vs {carried:?}");
    assert!(s.out.hands[0].flags & HAND_HOLDING != 0 && s.out.hands[0].holding == BOTTLE);
    s.grip(0, 0.0);
    assert_eq!(s.rig.held(0), None);
    s.step(90);
    assert!(s.pos(BOTTLE).y < 0.2, "dropped to the floor: {:?}", s.pos(BOTTLE));
    assert!(s.rig.debug.throws == 0, "a drop isn't a throw");
}

#[test]
fn landing_objects_make_impacts() {
    let mut s = Sim::new();
    s.move_hand(0, s.pos(BOTTLE) + Vec3::new(0.0, 0.15, 0.0), 30);
    s.grip(0, 1.0);
    s.move_hand(0, Vec3::new(-0.3, 1.5, 0.1), 40);
    s.step(10);
    s.grip(0, 0.0);
    let mut impacts = Vec::new();
    for _ in 0..120 {
        s.step(1);
        impacts.extend(s.rig.feedback.iter().filter(|f| f.cue == Cue::Impact).map(|f| (f.hand, f.intensity)));
    }
    assert!(!impacts.is_empty(), "the bottle hit the floor");
    assert!(impacts.iter().all(|(h, i)| h.is_none() && *i > 0.0 && *i <= 1.0), "{impacts:?}");
    assert!(impacts.len() < 6, "one impact per bounce, not one per frame: {}", impacts.len());
}

#[test]
fn grip_hysteresis_keeps_a_relaxed_hand_holding() {
    let mut s = Sim::new();
    s.move_hand(0, s.pos(BOTTLE) + Vec3::new(0.0, 0.15, 0.0), 30);
    s.grip(0, 0.65);
    assert_eq!(s.rig.held(0), Some(BOTTLE));
    s.grip(0, 0.45); // relaxed but above the release threshold
    assert_eq!(s.rig.held(0), Some(BOTTLE), "still held at 0.45");
    s.grip(0, 0.3);
    assert_eq!(s.rig.held(0), None);
}

#[test]
fn throws_use_the_peak_hand_speed() {
    let mut s = Sim::new();
    s.move_hand(0, s.pos(BOTTLE) + Vec3::new(0.0, 0.15, 0.0), 30);
    s.grip(0, 1.0);
    s.move_hand(0, Vec3::new(-0.2, 1.3, -0.2), 30);
    s.step(10);
    // Swing forward at ~6 m/s, slowing in the last two frames before letting go.
    let mut p = s.hand_pos(0);
    for v in [2.0, 4.0, 6.0, 6.0, 6.0, 6.0, 4.0, 2.5] {
        p.z += v * DT;
        s.set_hand(0, p);
        s.step(1);
    }
    s.f.hands[0].squeeze = 0.0;
    s.step(1);
    let speed = s.rig.debug.last_throw_speed;
    assert_eq!(s.rig.debug.throws, 1);
    assert!(speed > 5.5, "thrown at the peak speed (x1.1), not the release frame's: {speed}");
    let before = s.pos(BOTTLE);
    s.step(10);
    assert!(s.pos(BOTTLE).z - before.z > 0.5, "it flies forward");
}

#[test]
fn guns_snap_into_the_hand_aimed_along_the_controller() {
    let mut s = Sim::new();
    s.take_rifle();
    let (p, q) = s.rig.object_world_pose(RIFLE).unwrap();
    let grip = p + q * v3(rifle().grip);
    assert!(grip.distance(s.hand_pos(1)) < 0.02, "grip on the palm: {grip:?}");
    assert!((q * Vec3::Z).angle_between(Vec3::Z) < 0.05, "barrel along the aim");
    let w = s.out.primary_weapon.expect("primary weapon reported");
    assert_eq!(w.id, RIFLE);
    assert!(s.out.hand_busy[1]);
}

#[test]
fn two_handed_aim_pivots_on_the_firing_hand() {
    let mut s = Sim::new();
    s.take_rifle();
    let fore = s.pos(RIFLE) + s.rot(RIFLE) * v3(rifle().foregrip);
    s.move_hand(0, fore + Vec3::new(0.0, -0.03, 0.0), 30);
    s.grip(0, 1.0);
    assert!(s.out.primary_weapon.unwrap().two_handed, "support hand on the foregrip");
    // Raise the support hand 15 cm: the barrel tilts up.
    let up = s.hand_pos(0) + Vec3::new(0.0, 0.15, 0.0);
    s.move_hand(0, up, 20);
    s.step(60); // the support hand is steadied (One-Euro), so it settles over a few tenths
    let fwd = s.rot(RIFLE) * Vec3::Z;
    assert!(fwd.y > 0.15, "barrel tilts up with the support hand: {fwd:?}");
    let grip_w = s.pos(RIFLE) + s.rot(RIFLE) * v3(rifle().grip);
    let on_line = s.pos(RIFLE) + s.rot(RIFLE) * v3(rifle().foregrip);
    let off = (on_line - grip_w).angle_between(s.hand_pos(0) - grip_w).to_degrees();
    assert!(off < 1.5, "the gun points its foregrip at the support hand: {off:.2} degrees off");
    let grip = s.pos(RIFLE) + s.rot(RIFLE) * v3(rifle().grip);
    assert!(grip.distance(s.hand_pos(1)) < 0.03, "still pivoting on the firing hand");
    // Letting go of the gun hand passes the gun to the support hand.
    s.grip(1, 0.0);
    assert_eq!(s.rig.held(0), Some(RIFLE));
}

#[test]
fn the_support_hand_locks_onto_the_foregrip() {
    let mut s = Sim::new();
    s.take_rifle();
    let fore = s.pos(RIFLE) + s.rot(RIFLE) * v3(rifle().foregrip);
    // Grab a few cm off the foregrip: the drawn hand locks exactly onto it.
    s.move_hand(0, fore + s.rot(RIFLE) * Vec3::new(-0.04, -0.02, 0.05), 30);
    s.grip(0, 1.0);
    s.step(5);
    let (wp, wr) = s.rig.object_world_pose(RIFLE).unwrap();
    let local = wr.inverse() * (v3(s.out.hands[0].pose.pos) - wp);
    assert!(local.distance(v3(rifle().foregrip)) < 0.005, "on the game's foregrip point: {local:?}");
    // Moving the controller along the barrel doesn't slide it.
    let fwd = s.rot(RIFLE) * Vec3::Z;
    s.move_hand(0, s.hand_pos(0) + fwd * 0.06, 20);
    s.step(5);
    let (wp, wr) = s.rig.object_world_pose(RIFLE).unwrap();
    let local = wr.inverse() * (v3(s.out.hands[0].pose.pos) - wp);
    assert!(local.distance(v3(rifle().foregrip)) < 0.005, "still locked: {local:?}");
    // Pulled well away, it lets go.
    s.move_hand(0, s.hand_pos(0) + Vec3::new(-0.5, -0.2, 0.0), 20);
    assert!(!s.out.primary_weapon.unwrap().two_handed, "let go of the foregrip");
}

#[test]
fn a_rigged_grip_puts_the_gun_where_the_hand_points() {
    let mut s = Sim::new();
    // Rigged: the controller's grip pose sits on the gun pitched 20 degrees down (a raked grip).
    let grip_rot = Quat::from_rotation_x(20f32.to_radians());
    s.table.descs[2].grip_rot = grip_rot.to_array();
    s.table.seq += 1;
    s.rig.sync(&s.table);
    s.take_rifle();
    s.step(20);
    let (wp, wr) = s.rig.object_world_pose(RIFLE).unwrap();
    let hand = s.rig.hands.as_ref().unwrap()[1].target;
    let expect = (hand.1 * grip_rot.inverse()).normalize();
    assert!(wr.angle_between(expect) < 0.05, "gun = hand * inverse(grip_rot)");
    assert!((wp + wr * v3(rifle().grip)).distance(hand.0) < 0.02, "grip on the palm");
    // The drawn hand is the gun's grip frame.
    let drawn = q4(s.out.hands[1].pose.rot);
    assert!(drawn.angle_between(wr * grip_rot) < 0.05);
}

#[test]
fn letting_go_with_a_swing_throws_instead_of_holstering() {
    let mut s = Sim::new();
    s.move_hand(1, s.pos(PISTOL) + Vec3::new(0.0, 0.12, 0.0), 40);
    s.grip(1, 1.0);
    s.move_hand(1, Vec3::new(0.25, 1.3, 0.3), 20);
    // Swing down past the hip fast and let go there.
    let mut p = s.hand_pos(1);
    let hip = Vec3::new(0.2, 1.16, 0.0);
    for _ in 0..6 {
        p = p.lerp(hip, 0.5);
        s.set_hand(1, p);
        s.step(1);
    }
    s.f.hands[1].squeeze = 0.0;
    s.step(1);
    assert_eq!(s.rig.holstered(SLOT_HIP_RIGHT), None, "a fast swing past the hip isn't holstering");
}

#[test]
fn rifles_go_on_the_back_over_the_shoulder() {
    let mut s = Sim::new();
    s.take_rifle();
    // Over the right shoulder, behind the head, and let go gently.
    s.move_hand(1, Vec3::new(0.25, 1.65, 0.1), 30);
    s.move_hand(1, Vec3::new(0.18, 1.62, -0.15), 30);
    s.step(5);
    s.grip(1, 0.0);
    assert_eq!(s.rig.holstered(SLOT_BACK_RIGHT), Some(RIFLE), "on the back");
    // Reach back and draw it again.
    s.move_hand(1, Vec3::new(0.25, 1.4, 0.3), 30);
    s.move_hand(1, Vec3::new(0.18, 1.62, -0.15), 30);
    s.grip(1, 1.0);
    assert_eq!(s.rig.held(1), Some(RIFLE));
}

#[test]
fn magazines_come_from_the_chest_pouch_and_eject_into_a_waiting_hand() {
    let mut s = Sim::new();
    s.take_rifle();
    // Eject with the left hand right under the gun: the magazine drops into it.
    let well = s.pos(RIFLE) + s.rot(RIFLE) * v3(rifle().mag_well);
    s.move_hand(0, well + Vec3::new(0.0, -0.12, 0.0), 30);
    s.grip(0, 1.0); // ready to catch
    let mag = s.rig.mag_of(RIFLE).unwrap();
    s.tap(1, CTRL_A);
    s.step(10);
    assert_eq!(s.rig.held(0), Some(mag), "the magazine came out into the hand (and didn't seat itself again)");
    assert_eq!(s.rig.mag_of(RIFLE), None);
    // Throw it away and reach to the chest: a fresh one.
    s.grip(0, 0.0);
    s.move_hand(0, Vec3::new(-0.05, 1.38, 0.18), 30);
    assert!(s.out.hand_targeting[0], "the chest pouch is in reach");
    s.grip(0, 1.0);
    let fresh = s.rig.held(0).expect("a magazine from the chest");
    assert_ne!(fresh, mag);
}

#[test]
fn a_shouldered_rifle_aims_from_the_shoulder() {
    let mut s = Sim::new();
    s.s.interaction.virtual_stock = true;
    // Mark the test rifle as having a stock.
    s.table.descs[2].weapon_flags |= WPN_STOCK;
    s.table.seq += 1;
    s.rig.sync(&s.table);
    s.take_rifle();
    // Shoulder it: the grip just in front of the right shoulder, support hand out on the foregrip.
    let shoulder = Vec3::new(0.18, 1.48, -0.06);
    s.move_hand(1, shoulder + Vec3::new(0.0, 0.0, 0.18), 30);
    let fore = s.pos(RIFLE) + s.rot(RIFLE) * v3(rifle().foregrip);
    s.move_hand(0, fore, 30);
    s.grip(0, 1.0);
    assert!(s.out.primary_weapon.unwrap().two_handed);
    // Point the support hand off to the left and up: the barrel follows the shoulder->hand line.
    let support = shoulder + Vec3::new(-0.2, 0.15, 0.45);
    s.move_hand(0, support, 30);
    s.step(20);
    let want = (support - shoulder).normalize();
    let fwd = s.rot(RIFLE) * Vec3::Z;
    assert!(fwd.angle_between(want) < 0.12, "barrel {fwd:?} vs shoulder line {want:?}");
}

#[test]
fn holster_draw_and_auto_return() {
    let mut s = Sim::new();
    s.s.interaction.auto_holster = true;
    // Pick up the pistol (left hand of the table) with the right hand.
    s.move_hand(1, s.pos(PISTOL) + Vec3::new(0.0, 0.12, 0.0), 40);
    s.grip(1, 1.0);
    assert_eq!(s.rig.held(1), Some(PISTOL));
    // Into the right hip holster: 0.55 m below the eyes, 0.2 m to the side.
    s.move_hand(1, Vec3::new(0.2, 1.3, 0.2), 20);
    s.move_hand(1, Vec3::new(0.21, 1.16, 0.04), 30);
    s.grip(1, 0.0);
    assert_eq!(s.rig.holstered(SLOT_HIP_RIGHT), Some(PISTOL), "holstered on the hip");
    s.move_hand(1, Vec3::new(0.25, 1.3, 0.3), 20);
    s.step(10);
    let mut states: Box<ObjectStates> = bytemuck::zeroed_box();
    s.rig.write_states(&mut states);
    let st = states.states[..states.count as usize].iter().find(|x| x.id == PISTOL).unwrap();
    assert!(st.flags & OS_HOLSTERED != 0 && st.flags & OS_TRACKING != 0);
    assert!((v3(st.pose.pos) - Vec3::new(0.2, 1.15, 0.03)).length() < 0.15, "rides on the hip: {:?}", st.pose.pos);
    // Draw it back out.
    s.move_hand(1, Vec3::new(0.2, 1.16, 0.04), 25);
    assert!(s.out.hand_targeting[1], "hovering the holster");
    s.grip(1, 1.0);
    assert_eq!(s.rig.held(1), Some(PISTOL));
    assert_eq!(s.rig.debug.drawn, 1);
    // Let go out in front, gently: it glides back to the hip.
    s.move_hand(1, Vec3::new(0.2, 1.3, 0.4), 40);
    s.step(10);
    s.grip(1, 0.0);
    s.step(45);
    assert_eq!(s.rig.holstered(SLOT_HIP_RIGHT), Some(PISTOL), "auto-returned");
    assert_eq!(s.rig.debug.returns, 1);
}

#[test]
fn shooting_empties_the_gun_then_eject_pouch_seat_and_rack() {
    let mut s = Sim::new();
    s.take_rifle();
    assert_eq!(s.rig.weapon_rounds(RIFLE), Some((5, true, false)), "arrives with a seated magazine and one chambered");
    // Hold the trigger: full auto empties it (5 + 1 chambered), then it clicks.
    s.f.hands[1].trigger = 1.0;
    s.step(90);
    s.trigger(1, 0.0);
    assert_eq!(s.rig.debug.shots, 6);
    s.trigger(1, 1.0);
    assert!(s.rig.debug.dry_fires >= 1, "dry click on an empty chamber");
    s.trigger(1, 0.0);
    let (rounds, chambered, _) = s.rig.weapon_rounds(RIFLE).unwrap();
    assert!(rounds == 0 && !chambered);
    // Eject: A on the gun hand drops the empty magazine.
    let old_mag = s.rig.mag_of(RIFLE).unwrap();
    s.tap(1, CTRL_A);
    assert_eq!(s.rig.mag_of(RIFLE), None);
    assert!(s.rig.is_free(old_mag));
    assert_eq!(s.rig.debug.ejects, 1);
    // Reach over the left shoulder: the pouch hands over a magazine for this gun.
    s.move_hand(0, Vec3::new(-0.3, 1.45, 0.1), 20);
    s.move_hand(0, Vec3::new(-0.26, 1.6, -0.06), 20);
    assert!(s.out.hand_targeting[0], "the ammo pouch is in reach");
    s.grip(0, 1.0);
    let mag = s.rig.held(0).expect("a magazine from the pouch");
    assert_ne!(mag, old_mag);
    assert_eq!(s.rig.debug.mags_spawned, 1);
    // Bring it under the gun, the fist's axis up (the magazine upright), and push it into the well.
    s.set_rot(0, s.rot(RIFLE) * held_mag_rot().inverse());
    s.move_hand(0, s.well_world() + Vec3::new(0.0, -0.25, 0.0), 30);
    s.step(10);
    let top = s.pos(mag) + s.rot(mag) * v3(rifle_mag().mag_well);
    let target = s.hand_pos(0) + (s.well_world() - top);
    s.move_hand(0, target, 15);
    s.step(3);
    assert_eq!(s.rig.mag_of(RIFLE), Some(mag), "magazine seated");
    assert_eq!(s.rig.held(0), None, "the hand let go of it");
    assert_eq!(s.rig.debug.seats, 1);
    assert_eq!(s.rig.weapon_rounds(RIFLE), Some((5, false, false)), "seated, but this gun needs a rack");
    // Rack: grab the charging handle and pull it back along the barrel.
    s.grip(0, 0.0);
    let handle = s.pos(RIFLE) + s.rot(RIFLE) * v3(rifle().handle);
    s.move_hand(0, handle + Vec3::new(0.02, 0.0, 0.0), 25);
    s.grip(0, 1.0);
    let back = s.hand_pos(0) - s.rot(RIFLE) * Vec3::Z * 0.1;
    s.move_hand(0, back, 10);
    s.grip(0, 0.0);
    assert_eq!(s.rig.debug.racks, 1);
    assert_eq!(s.rig.weapon_rounds(RIFLE), Some((4, true, false)), "chambered");
    s.trigger(1, 1.0);
    assert_eq!(s.rig.debug.shots, 7, "fires again");
}

#[test]
fn the_pouch_ignores_hands_in_front_of_the_face() {
    let mut s = Sim::new();
    s.take_rifle();
    s.move_hand(0, Vec3::new(-0.05, 1.6, 0.15), 30);
    assert!(!s.out.hand_targeting[0], "a hand at the face isn't reaching for ammo");
    s.grip(0, 1.0);
    assert_eq!(s.rig.held(0), None);
}

#[test]
fn gravity_gloves_pull_and_catch() {
    let mut s = Sim::new();
    let target = s.pos(FAR_BOTTLE);
    let from = Vec3::new(0.2, 1.2, 0.2);
    s.move_hand(1, from, 20);
    s.set_rot(1, look_rotation(target - from, Vec3::Y));
    s.step(15);
    assert!(s.out.hand_targeting[1], "pointing at the far bottle");
    s.grip(1, 1.0);
    // Flick the hand back towards the body.
    let mut p = s.hand_pos(1);
    for _ in 0..6 {
        p.z -= 2.5 * DT;
        p.y += 1.0 * DT;
        s.set_hand(1, p);
        s.step(1);
    }
    assert_eq!(s.rig.debug.pulls, 1, "the flick launches it");
    for _ in 0..120 {
        s.step(1);
        if s.rig.held(1).is_some() {
            break;
        }
    }
    assert_eq!(s.rig.held(1), Some(FAR_BOTTLE), "caught: bottle at {:?}", s.pos(FAR_BOTTLE));
    assert_eq!(s.rig.debug.catches, 1);
}

#[test]
fn hands_report_how_they_hold_things() {
    let mut s = Sim::new();
    assert_eq!(s.out.holds, [rustvr_ik::HoldShape::None; 2]);
    s.take_rifle();
    assert_eq!(s.out.holds[1], rustvr_ik::HoldShape::PistolGrip);
}

#[test]
fn a_rifle_braced_at_the_shoulder_kicks_as_the_game_says_and_harder_otherwise() {
    let mut s = Sim::new();
    s.take_rifle();
    let i = s.s.interaction.clone();
    // Held out at the hip, one hand: the one-handed scale.
    s.move_hand(1, Vec3::new(0.3, 1.0, 0.45), 40);
    s.step(10);
    let loose = s.rig.hold_scale(RIFLE, 1, false, &i);
    assert!((loose.turn - i.recoil_one_hand).abs() < 1e-3, "{loose:?}");
    // Two hands, off the shoulder.
    assert!((s.rig.hold_scale(RIFLE, 1, true, &i).turn - i.recoil_unbraced).abs() < 1e-3);
    // The stock roughly at the right shoulder (eyes at 1.7 m): braced, the game's own.
    let shoulder = Vec3::new(0.18, 1.48, -0.06);
    s.move_hand(1, shoulder + Vec3::new(0.03, -0.04, 0.24), 40);
    s.step(10);
    let b = s.rig.brace(RIFLE, 1);
    assert!(b > 0.95, "braced: {b}");
    assert!((s.rig.hold_scale(RIFLE, 1, true, &i).turn - 1.0).abs() < 0.05);
    // No need to be exact: 10 cm off still counts.
    s.move_hand(1, shoulder + Vec3::new(0.12, -0.08, 0.26), 40);
    s.step(10);
    assert!(s.rig.brace(RIFLE, 1) > 0.9, "near enough: {}", s.rig.brace(RIFLE, 1));
}

#[test]
fn a_rifle_has_heft_and_a_bottle_doesnt() {
    // Swing the hand a quarter turn at once: the rifle follows over a few tenths of a second.
    let mut s = Sim::new();
    s.take_rifle();
    let r0 = s.rot(RIFLE);
    s.set_rot(1, Quat::from_rotation_y(1.57));
    s.step(9);
    let turned = s.rot(RIFLE).angle_between(r0);
    assert!(turned > 0.2 && turned < 1.0, "a tenth of a second in it's part way round: {turned:.2} rad");
    s.step(60);
    let target = (Quat::from_rotation_y(1.57) * r0 * Quat::IDENTITY).angle_between(r0);
    assert!((s.rot(RIFLE).angle_between(r0) - target).abs() < 0.1, "and gets there");
    // A bottle keeps up.
    let mut s = Sim::new();
    s.move_hand(0, s.pos(BOTTLE) + Vec3::new(0.0, 0.15, 0.0), 30);
    s.grip(0, 1.0);
    s.step(20);
    let r0 = s.rot(BOTTLE);
    s.set_rot(0, Quat::from_rotation_y(1.57));
    s.step(9);
    assert!(s.rot(BOTTLE).angle_between(r0) > 1.3, "a bottle all but keeps up: {:.2}", s.rot(BOTTLE).angle_between(r0));
}

#[test]
fn pulls_fly_like_alyx() {
    // 0.6 s out to 2.5 m and 0.8 s at 10 m; never under half a second; gravity at least half.
    let (t, g) = alyx_pull(2.54);
    assert!((t - 0.6).abs() < 1e-3 && (g - 5.13).abs() < 0.05, "{t} {g}");
    let (t, g) = alyx_pull(10.16);
    assert!((t - 0.8).abs() < 1e-3 && g < 9.81 && g >= 4.9, "{t} {g}");
    let (t, g) = alyx_pull(0.5);
    assert!(t >= 0.5 && g >= 4.9 - 1e-3, "{t} {g}");
    // The throw lands on the hand: launched at d/t plus half g t up, under g.
    for d in [0.5f32, 1.5, 3.0, 6.0, 9.0] {
        let (t, g) = alyx_pull(d);
        let v = Vec3::new(0.0, 0.0, -d / t) + Vec3::Y * (0.5 * g * t);
        let end = Vec3::new(0.0, 0.0, d) + v * t - Vec3::Y * (0.5 * g * t * t);
        assert!(end.length() < 1e-3, "{d}: {end:?}");
    }
}

#[test]
fn a_locked_glove_target_hops_and_lets_go_when_you_point_away() {
    let mut s = Sim::new();
    let target = s.pos(FAR_BOTTLE);
    let from = Vec3::new(0.2, 1.2, 0.2);
    s.move_hand(1, from, 20);
    s.set_rot(1, look_rotation(target - from, Vec3::Y));
    s.step(15);
    s.grip(1, 1.0);
    let y0 = s.pos(FAR_BOTTLE).y;
    let mut top = y0;
    for _ in 0..20 {
        s.step(1);
        top = top.max(s.pos(FAR_BOTTLE).y);
    }
    assert!(top > y0 + 0.03, "it hops as it locks: {:.3} -> {:.3}", y0, top);
    assert!(s.out.hand_targeting[1], "still locked");
    // Point the hand 90 degrees away: the lock goes.
    s.set_rot(1, look_rotation(Vec3::X, Vec3::Y) * Quat::from_rotation_z(1.5));
    s.step(3);
    assert!(!s.out.hand_targeting[1], "let go");
    assert_eq!(s.rig.debug.pulls, 0);
}

#[test]
fn held_objects_are_reported_in_tracking_space() {
    let mut s = Sim::new();
    s.move_hand(0, s.pos(BOTTLE) + Vec3::new(0.0, 0.15, 0.0), 30);
    s.grip(0, 1.0);
    s.step(20);
    let mut st: Box<ObjectStates> = bytemuck::zeroed_box();
    s.rig.write_states(&mut st);
    let b = st.states[..st.count as usize].iter().find(|x| x.id == BOTTLE).unwrap();
    assert!(b.flags & OS_TRACKING != 0 && b.flags & OS_HELD_LEFT != 0);
    let tracked = v3(b.pose.pos);
    assert!(tracked.distance(s.hand_pos(0)) < 0.2);
    // Free objects are in world space; the far bottle hasn't moved.
    let far = st.states[..st.count as usize].iter().find(|x| x.id == FAR_BOTTLE).unwrap();
    assert!(far.flags & OS_TRACKING == 0);
    assert!(v3(far.pose.pos).distance(Vec3::new(0.0, 0.1, 3.0)) < 0.02);
}

#[test]
fn world_map_round_trips() {
    let m = WorldMap { origin: Vec3::new(0.1, 0.0, -0.2), body_yaw: 1.2, world_per_meter: 1.5, anchor: Vec3::new(10.0, 2.0, -4.0) };
    let p = Vec3::new(0.3, 1.6, 0.4);
    assert!(m.inv_point(m.point(p)).distance(p) < 1e-5);
    let q = Quat::from_rotation_x(0.4);
    assert!(m.inv_rot(m.rot(q)).dot(q).abs() > 1.0 - 1e-6);
}

#[test]
fn table_changes_respawn_and_remove_objects() {
    let mut s = Sim::new();
    // Bump the bottle's spawn_seq with a new spawn pose: it's placed there.
    s.table.descs[0].spawn_seq = 2;
    s.table.descs[0].spawn_pose = pose(Vec3::new(-0.4, 0.9, 0.6), Quat::IDENTITY);
    s.table.seq += 1;
    s.rig.sync(&s.table);
    assert!(s.pos(BOTTLE).distance(Vec3::new(-0.4, 0.9, 0.6)) < 1e-3);
    // Drop the pistol from the table: it's gone.
    s.table.descs[1] = s.table.descs[4];
    s.table.count = 4;
    s.table.seq += 1;
    s.rig.sync(&s.table);
    assert!(s.rig.object_world_pose(PISTOL).is_none());
    assert!(s.rig.object_world_pose(RIFLE).is_some());
}

#[test]
fn starting_loadout_goes_into_its_slots() {
    let mut s = Sim::new();
    let mut d = pistol();
    d.id = 50;
    d.start_slot = SLOT_HIP_RIGHT + 1;
    let n = s.table.count as usize;
    s.table.descs[n] = d;
    s.table.count += 1;
    s.table.seq += 1;
    s.rig.sync(&s.table);
    assert_eq!(s.rig.holstered(SLOT_HIP_RIGHT), Some(50));
    s.step(5);
    let hip = s.rig.anchor().zone_center(&zones::slot_zones(1.7, 1)[SLOT_HIP_RIGHT as usize]);
    assert!(s.pos(50).distance(hip) < 0.2, "on the hip: {:?}", s.pos(50));
}

// ---- throwables -------------------------------------------------------------------------------------
const GRENADE: u32 = 6;
const KNIFE: u32 = 7;
const MOLOTOV: u32 = 8;

/// A frag grenade on the table's right: a pin (its ring up on the left) and a lever, 3 s fuse.
fn frag() -> ObjectDesc {
    let mut d = obj(GRENADE, OBJ_THROWABLE, Vec3::new(0.45, 0.85, 0.75), [0.03, 0.045, 0.03], 0.4);
    d.flags |= OBJF_HOLSTERABLE | OBJF_PULLABLE;
    d.throw_flags = THROW_GRENADE | THROW_PIN | THROW_LEVER;
    d.handle = [-0.035, 0.05, 0.0];
    d.grip_rot = Quat::IDENTITY.to_array();
    d.fuse = 3.0;
    d
}

/// A throwing knife on the table's left, blade along +Z.
fn knife() -> ObjectDesc {
    let mut d = obj(KNIFE, OBJ_THROWABLE, Vec3::new(-0.45, 0.82, 0.75), [0.01, 0.012, 0.12], 0.15);
    d.flags |= OBJF_HOLSTERABLE;
    d.throw_flags = THROW_KNIFE;
    d.grip = [0.0, 0.0, -0.06];
    d.muzzle = [0.0, 0.0, 0.12];
    d.grip_rot = Quat::IDENTITY.to_array();
    d
}

/// An incendiary that bursts on impact once lit (no pin: a button lights it).
fn molotov() -> ObjectDesc {
    let mut d = obj(MOLOTOV, OBJ_THROWABLE, Vec3::new(0.2, 0.9, 0.8), [0.03, 0.09, 0.03], 0.5);
    d.throw_flags = THROW_GRENADE | THROW_IMPACT;
    d.impact_delay = 0.1;
    d.grip_rot = Quat::IDENTITY.to_array();
    d
}

impl Sim {
    fn take(&mut self, hand: usize, id: u32) {
        let at = self.pos(id);
        self.move_hand(hand, at + Vec3::new(0.0, 0.06, 0.0), 30);
        self.grip(hand, 1.0);
        assert_eq!(self.rig.held(hand), Some(id), "picked up {id}");
    }

    fn state(&self, id: u32) -> ObjectState {
        let mut st: Box<ObjectStates> = bytemuck::zeroed_box();
        self.rig.write_states(&mut st);
        *st.states[..st.count as usize].iter().find(|x| x.id == id).unwrap()
    }

    /// Swings hand `i` along `dir`, up to `speed` over a few frames, and lets go at full speed.
    fn throw(&mut self, i: usize, dir: Vec3, speed: f32) {
        let mut p = self.hand_pos(i);
        for k in 0..8 {
            let v = speed * ((k + 1) as f32 / 4.0).min(1.0);
            p += dir.normalize() * v * DT;
            self.set_hand(i, p);
            self.step(1);
        }
        self.f.hands[i].squeeze = 0.0;
        p += dir.normalize() * speed * DT;
        self.set_hand(i, p);
        self.step(1);
    }
}

#[test]
fn the_other_hand_pulls_the_pin_and_the_lever_holds_the_fuse_until_the_throw() {
    let mut s = Sim::with(&[frag()]);
    s.take(1, GRENADE);
    s.move_hand(1, Vec3::new(0.2, 1.3, 0.3), 30);
    s.step(20);
    assert_eq!(s.out.holds[1], rustvr_ik::HoldShape::Small);
    // Held by its rig: the ring sits up on the left of the fist.
    let (gp, gr) = s.rig.object_world_pose(GRENADE).unwrap();
    let ring = gp + gr * v3(frag().handle);
    assert!(ring.distance(Vec3::new(0.165, 1.35, 0.3)) < 0.03, "ring at {ring:?}");
    s.move_hand(0, ring, 30);
    assert!(s.out.hand_targeting[0], "the left hand is on the pin");
    s.grip(0, 1.0);
    s.move_hand(0, ring + Vec3::new(-0.03, 0.0, 0.0), 5);
    assert_eq!(s.rig.debug.pins, 0, "not out yet");
    s.move_hand(0, ring + Vec3::new(-0.08, 0.0, 0.0), 5);
    assert_eq!(s.rig.debug.pins, 1, "pulled");
    assert!(s.cues.iter().filter(|c| **c == Cue::GrenadePin).count() == 2, "a click in each hand");
    let st = s.state(GRENADE);
    assert!(st.flags & OS_PIN_PULLED != 0 && st.flags & OS_ARMED == 0, "the lever holds it: {:#x}", st.flags);
    s.grip(0, 0.0);
    s.step(90);
    assert_eq!(s.rig.debug.detonations, 0, "nothing burns while the lever is held");
    // Throw it: the lever flies, the fuse starts.
    s.throw(1, Vec3::new(0.0, 0.5, 1.0), 6.0);
    let st = s.state(GRENADE);
    assert!(st.flags & (OS_ARMED | OS_THROWN) == OS_ARMED | OS_THROWN, "{:#x}", st.flags);
    assert!((st.fuse - 3.0).abs() < 0.05, "the game's 3 s fuse: {}", st.fuse);
    assert!(s.cues.contains(&Cue::GrenadeThrow) && s.cues.contains(&Cue::GrenadeArm));
    assert!(s.rig.debug.last_throw_gain > 1.3, "a grenade's gain: {}", s.rig.debug.last_throw_gain);
    s.step((2.8 * 90.0) as usize);
    assert!(s.cues.iter().filter(|c| **c == Cue::GrenadeTick).count() >= 5, "it ticks");
    assert_eq!(s.rig.debug.detonations, 0, "not yet");
    s.step(30);
    assert_eq!(s.rig.debug.detonations, 1, "it went off after its fuse");
    let st = s.state(GRENADE);
    assert!(st.flags & OS_SPENT != 0 && st.shots == 1, "the game is told: {:#x} shots {}", st.flags, st.shots);
    assert!(s.cues.contains(&Cue::Explosion) && s.cues.contains(&Cue::ExplosionRumble));
    let at = s.rig.debug.last_detonation.unwrap();
    assert!(at.z > 1.5, "down range: {at:?}");
    // The game puts it back: safe again, the count kept.
    s.table.descs[5].spawn_seq = 2;
    s.table.seq += 1;
    s.rig.sync(&s.table);
    let st = s.state(GRENADE);
    assert!(st.flags & (OS_SPENT | OS_ARMED | OS_PIN_PULLED) == 0 && st.shots == 1);
}

#[test]
fn pouches_take_safe_grenades_and_never_armed_ones() {
    let mut s = Sim::with(&[frag()]);
    s.take(1, GRENADE);
    let pouch = Vec3::new(0.125, 1.41, 0.11);
    s.move_hand(1, pouch, 40);
    s.step(5);
    s.grip(1, 0.0);
    assert_eq!(s.rig.holstered(SLOT_CHEST_RIGHT), Some(GRENADE), "in the chest pouch");
    s.step(10);
    s.grip(1, 1.0);
    assert_eq!(s.rig.held(1), Some(GRENADE), "and out again");
    // Armed (one-handed here), it won't go back in: it drops.
    s.s.interaction.grenade_arming = GrenadeArming::Button;
    s.trigger(1, 1.0);
    assert_eq!(s.rig.debug.arms, 1);
    s.step(5);
    s.grip(1, 0.0);
    assert_eq!(s.rig.holstered(SLOT_CHEST_RIGHT), None, "never stowed armed");
    assert!(s.rig.is_free(GRENADE));
}

#[test]
fn a_button_arms_one_handed_and_a_cooked_grenade_goes_off_in_the_hand() {
    let mut s = Sim::with(&[frag()]);
    s.s.interaction.grenade_arming = GrenadeArming::Button;
    s.take(1, GRENADE);
    s.trigger(1, 1.0);
    let st = s.state(GRENADE);
    assert!(st.flags & OS_ARMED != 0, "the trigger armed it");
    s.step((3.1 * 90.0) as usize);
    assert_eq!(s.rig.debug.detonations, 1, "cooked too long");
    assert_eq!(s.rig.held(1), None, "it's out of the hand");
    // The pin scheme still arms one-handed while the other hand holds a gun.
    let mut s = Sim::with(&[frag()]);
    s.take_rifle();
    s.take(0, GRENADE);
    s.tap(0, CTRL_A);
    assert!(s.state(GRENADE).flags & OS_ARMED != 0, "A armed it with a gun in the other hand");
}

#[test]
fn a_cook_pops_the_lever_in_the_hand() {
    let mut s = Sim::with(&[frag()]);
    s.take(1, GRENADE);
    s.move_hand(1, Vec3::new(0.2, 1.3, 0.3), 30);
    let (gp, gr) = s.rig.object_world_pose(GRENADE).unwrap();
    let ring = gp + gr * v3(frag().handle);
    s.move_hand(0, ring, 30);
    s.grip(0, 1.0);
    s.move_hand(0, ring + Vec3::new(-0.08, 0.0, 0.0), 10);
    s.tap(1, CTRL_B);
    let st = s.state(GRENADE);
    assert!(st.flags & OS_ARMED != 0 && st.fuse < 3.0 && st.fuse > 2.9, "burning in the hand: {}", st.fuse);
}

#[test]
fn a_knife_thrown_as_a_dart_sticks_tip_first_and_pulls_back_out() {
    let mut s = Sim::with(&[knife()]);
    world_box(&mut s.rig, 9, [0.0, 1.2, 3.0], [2.0, 1.2, 0.05]); // a wall 3 m ahead, face at z 2.95
    s.take(1, KNIFE);
    s.move_hand(1, Vec3::new(0.2, 1.4, 0.0), 30);
    s.step(20);
    assert_eq!(s.out.holds[1], rustvr_ik::HoldShape::Foregrip);
    s.throw(1, Vec3::new(0.0, 0.15, 1.0), 8.0);
    assert_eq!(s.rig.debug.knife_throws, 1);
    assert!(s.cues.contains(&Cue::KnifeThrow));
    s.step(60);
    let st = s.state(KNIFE);
    assert!(st.flags & OS_STUCK != 0 && st.parent == 0, "stuck in the wall: {:#x}", st.flags);
    assert_eq!(s.rig.debug.sticks, 1);
    assert!(s.cues.contains(&Cue::KnifeStick));
    let (p, q) = s.rig.object_world_pose(KNIFE).unwrap();
    let tip = p + q * v3(knife().muzzle);
    assert!(tip.z > 2.95 && tip.z < 3.0, "the tip is in the wall: {tip:?}");
    assert!((q * Vec3::Z).z > 0.8, "blade first: {:?}", q * Vec3::Z);
    // It stays put, then the hand pulls it out.
    s.step(30);
    assert!(s.rig.object_world_pose(KNIFE).unwrap().0.distance(p) < 1e-3);
    let handle = p + q * v3(knife().grip);
    s.move_hand(1, handle, 60);
    s.grip(1, 1.0);
    assert_eq!(s.rig.held(1), Some(KNIFE), "pulled out");
}

#[test]
fn a_spinning_knife_brings_its_tip_round_and_sticks() {
    let mut s = Sim::with(&[knife()]);
    world_box(&mut s.rig, 9, [0.0, 1.2, 3.0], [2.0, 1.2, 0.05]);
    s.take(1, KNIFE);
    s.move_hand(1, Vec3::new(0.2, 1.4, 0.0), 30);
    s.step(20);
    // A wrist flick: end over end about the blade's flat.
    s.f.hands[1].ang_vel = [-18.0, 0.0, 0.0];
    s.throw(1, Vec3::new(0.0, 0.1, 1.0), 9.0);
    s.f.hands[1].ang_vel = [0.0; 3];
    s.step(60);
    assert!(s.state(KNIFE).flags & OS_STUCK != 0, "the tip came round: {:?}", s.rig.object_world_pose(KNIFE));
}

#[test]
fn an_impact_grenade_bursts_where_it_lands_but_never_unlit() {
    let mut s = Sim::with(&[molotov()]);
    s.take(1, MOLOTOV);
    s.move_hand(1, Vec3::new(0.2, 1.4, 0.3), 30);
    s.trigger(1, 1.0); // no pin: the button lights it, whatever the scheme
    assert_eq!(s.rig.debug.arms, 1);
    s.throw(1, Vec3::new(0.0, 0.6, 1.0), 5.0);
    for _ in 0..200 {
        s.step(1);
        if s.rig.debug.detonations > 0 {
            break;
        }
    }
    assert_eq!(s.rig.debug.detonations, 1, "burst on landing");
    let at = s.rig.debug.last_detonation.unwrap();
    assert!(at.z > 1.0 && at.y < 0.3, "on the floor down range: {at:?}");
    // Unlit, a throw does nothing.
    let mut s = Sim::with(&[molotov()]);
    s.take(1, MOLOTOV);
    s.move_hand(1, Vec3::new(0.2, 1.4, 0.3), 30);
    s.throw(1, Vec3::new(0.0, 0.6, 1.0), 5.0);
    s.step(180);
    assert_eq!(s.rig.debug.detonations, 0);
}

#[test]
fn the_games_blasts_push_loose_things_not_held_ones() {
    let mut s = Sim::new();
    s.take_rifle();
    let bottle = s.pos(BOTTLE);
    let rifle = s.pos(RIFLE);
    s.rig.blast(bottle + Vec3::new(0.0, -0.05, -0.3), 3.0, 4.0);
    s.step(6);
    assert!(s.pos(BOTTLE).distance(bottle) > 0.05, "the bottle flew: {:?}", s.pos(BOTTLE));
    assert!(s.pos(RIFLE).distance(rifle) < 0.05, "the held rifle didn't");
    // One push on one thing.
    let far = s.pos(FAR_BOTTLE);
    s.rig.impulse(FAR_BOTTLE, Vec3::new(0.0, 0.0, 2.0), far);
    s.step(6);
    assert!(s.pos(FAR_BOTTLE).z > far.z + 0.05);
}
