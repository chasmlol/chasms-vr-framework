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
                arm_length: 0.0,
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

    /// Reaches hand `i` for `to` the way a hand reaches to take something: over to 30 cm above it,
    /// then down onto it slowly.
    fn reach(&mut self, i: usize, to: Vec3) {
        self.move_hand(i, to + Vec3::new(0.0, 0.3, 0.0), 30);
        self.move_hand(i, to, 60);
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

    /// Where thing `id`'s grip is (world): the place to take it by.
    fn grip_at(&self, id: u32) -> Vec3 {
        let d = self.table.descs.iter().find(|d| d.id == id).copied().or_else(|| self.rig.objects.get(&id).map(|o| o.desc)).unwrap();
        self.pos(id) + self.rot(id) * v3(d.grip)
    }

    /// Picks up the rifle from the table with the right hand (by its grip) and lifts it clear.
    fn take_rifle(&mut self) {
        let top = self.grip_at(RIFLE) + Vec3::new(0.0, 0.03, 0.0);
        self.reach(1, top);
        self.grip(1, 1.0);
        assert_eq!(self.rig.held(1), Some(RIFLE), "picked up the rifle");
        self.move_hand(1, top + Vec3::new(0.0, 0.2, 0.0), 20);
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
fn walls_stop_the_hand_and_show_a_ghost_however_far_it_reaches_through() {
    let mut s = Sim::new();
    world_box(&mut s.rig, 3, [0.0, 1.5, -0.4], [2.0, 1.0, 0.05]); // a wall behind the player
    s.move_hand(1, Vec3::new(0.2, 1.3, -0.2), 15);
    s.move_hand(1, Vec3::new(0.2, 1.3, -0.6), 15); // push through it
    let h = v3(s.out.hands[1].pose.pos);
    assert!(h.z > -0.38, "the wall holds the hand back: {h:?}");
    assert!(s.out.hands[1].flags & HAND_BLOCKED != 0);
    assert!(s.out.ghosts[1].is_none(), "no ghost unless asked for");
    s.s.interaction.ghost_hands = true;
    s.step(1);
    assert!(s.out.ghosts[1].is_some(), "with ghost hands on, a ghost hand shows where the controller is");
    assert!(s.rig.debug.blocked_frames > 0);
    // Far past the leash, the controller is still through the wall: the hand stays against it.
    s.move_hand(1, Vec3::new(0.2, 1.3, -1.3), 10);
    s.step(30);
    let h = v3(s.out.hands[1].pose.pos);
    assert!(h.z > -0.38, "never snapped through the wall: {h:?}");
    // Back on this side, it follows again.
    s.move_hand(1, Vec3::new(0.2, 1.3, -0.1), 20);
    s.step(10);
    let h = v3(s.out.hands[1].pose.pos);
    assert!(h.distance(s.hand_pos(1)) < 0.01, "follows the controller again: {h:?}");
}

#[test]
fn a_hand_reached_round_a_corner_goes_round_it() {
    let mut s = Sim::new();
    // A wall on the right, its near end 30 cm ahead: round its end is out of the head's sight.
    world_box(&mut s.rig, 3, [0.55, 1.3, 0.75], [0.05, 0.6, 0.45]);
    let head = Vec3::new(0.0, 1.7, 0.0);
    let behind = Vec3::new(0.85, 1.3, 0.7);
    s.step(1);
    assert!(!s.rig.physics.reachable(head, behind), "the place behind the wall is out of sight");
    // Out to the right in front of the wall's end, then round it and along behind it, at an
    // ordinary reach's pace, in the air all the way.
    s.move_hand(1, Vec3::new(0.3, 1.3, 0.1), 30);
    s.move_hand(1, Vec3::new(0.85, 1.3, 0.15), 30);
    s.move_hand(1, behind, 30);
    s.step(20);
    let h = v3(s.out.hands[1].pose.pos);
    assert!(h.distance(behind) < 0.05, "the hand is round the corner with the controller: {h:?}");
    assert!(s.out.ghosts[1].is_none(), "no ghost: nothing holds it back");
    // Back out the way it went.
    s.move_hand(1, Vec3::new(0.85, 1.3, 0.15), 30);
    s.move_hand(1, Vec3::new(0.3, 1.3, 0.1), 30);
    s.step(10);
    assert!(v3(s.out.hands[1].pose.pos).distance(s.hand_pos(1)) < 0.02, "and back");
}

#[test]
fn a_hand_reached_tight_round_a_corner_clipping_it_still_gets_round() {
    let mut s = Sim::new();
    world_box(&mut s.rig, 3, [0.55, 1.3, 0.75], [0.05, 0.6, 0.45]);
    s.step(1);
    // Round the wall's end hugging it: the controller cuts 3 cm into the corner on the way.
    let behind = Vec3::new(0.85, 1.3, 0.7);
    s.move_hand(1, Vec3::new(0.3, 1.3, 0.1), 30);
    s.move_hand(1, Vec3::new(0.62, 1.3, 0.33), 20);
    s.move_hand(1, behind, 25);
    s.step(20);
    let h = v3(s.out.hands[1].pose.pos);
    assert!(h.distance(behind) < 0.05, "the hand got round with the controller: {h:?}");
}

#[test]
fn the_leash_frees_a_hand_snagged_behind_something_the_head_sees_past() {
    let mut s = Sim::new();
    // A low panel beside the player, too wide to slide round: the hand catches on it, the head
    // sees past it.
    world_box(&mut s.rig, 3, [0.3, 1.0, 0.4], [0.3, 0.3, 0.03]);
    s.move_hand(1, Vec3::new(0.3, 1.0, 0.2), 15);
    s.move_hand(1, Vec3::new(0.3, 1.0, 1.3), 10); // through the panel, past the leash
    s.step(5);
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
    // Taken just as it is in the hand: neither moves.
    let (before, turned, hand) = (s.pos(BOTTLE), s.rot(BOTTLE), v3(s.out.hands[0].pose.pos));
    s.grip(0, 1.0);
    assert_eq!(s.rig.held(0), Some(BOTTLE));
    s.step(2);
    assert!(s.pos(BOTTLE).distance(before) < 0.005, "the bottle didn't move: {:?} from {before:?}", s.pos(BOTTLE));
    assert!(v3(s.out.hands[0].pose.pos).distance(hand) < 0.005, "nor the hand");
    let offset = s.pos(BOTTLE) - s.hand_pos(0);
    s.move_hand(0, Vec3::new(-0.3, 1.4, 0.1), 40);
    s.step(10);
    let carried = s.pos(BOTTLE) - s.hand_pos(0);
    assert!(carried.distance(offset) < 0.02, "held as it was taken: {carried:?} vs {offset:?}");
    assert!(s.rot(BOTTLE).angle_between(turned) < 0.05, "as it was turned");
    assert!(s.out.hands[0].flags & HAND_HOLDING != 0 && s.out.hands[0].holding == BOTTLE);
    s.grip(0, 0.0);
    assert_eq!(s.rig.held(0), None);
    s.step(90);
    assert!(s.pos(BOTTLE).y < 0.2, "dropped to the floor: {:?}", s.pos(BOTTLE));
    assert!(s.rig.debug.throws == 0, "a drop isn't a throw");
}

/// A game's own physics prop (`OBJF_GAME_SIMULATED`): it stays where the game says until a hand
/// takes it, goes with the hand, leaves it with the throw's velocity in its state (for the game to
/// take it back with), then follows the game again.
#[test]
fn a_game_prop_follows_the_game_until_held_and_goes_back_with_the_throw() {
    const PROP: u32 = 30;
    let mut d = obj(PROP, OBJ_PROP, Vec3::new(-0.3, 0.86, 0.5), [0.05, 0.05, 0.05], 0.4);
    d.flags |= OBJF_GAME_SIMULATED;
    let mut s = Sim::with(&[d]);
    let mut poses: Box<GamePoses> = bytemuck::zeroed_box();
    let mut game_at = |s: &mut Sim, p: Vec3| {
        poses.count = 1;
        poses.poses[0] = GamePose { id: PROP, pose: pose(p, Quat::IDENTITY), ..Default::default() };
        s.rig.set_game_poses(&poses);
    };
    let state = |s: &Sim| {
        let mut st: Box<ObjectStates> = bytemuck::zeroed_box();
        s.rig.write_states(&mut st);
        st.states[..st.count as usize].iter().find(|o| o.id == PROP).copied().unwrap()
    };
    // The game moves it (something knocked it): the runtime's follows.
    let p1 = Vec3::new(-0.3, 0.86, 0.5);
    game_at(&mut s, p1);
    s.step(20);
    assert!(s.pos(PROP).distance(p1) < 0.01, "where the game has it: {:?}", s.pos(PROP));
    let p2 = Vec3::new(-0.25, 0.86, 0.52);
    game_at(&mut s, p2);
    s.step(2);
    assert!(s.pos(PROP).distance(p2) < 0.01, "follows the game: {:?}", s.pos(PROP));

    // Taken: the hand moves it, whatever the game last said.
    s.move_hand(0, p2 + Vec3::new(0.0, 0.12, 0.0), 30);
    s.grip(0, 1.0);
    assert_eq!(s.rig.held(0), Some(PROP), "picked up");
    s.move_hand(0, Vec3::new(-0.3, 1.4, 0.2), 30);
    assert!(s.pos(PROP).y > 1.2, "lifted with the hand: {:?}", s.pos(PROP));
    assert!(state(&s).flags & OS_HELD_LEFT != 0);

    // Thrown: its state carries the throw for the game.
    for k in 1..=8 {
        s.set_hand(0, Vec3::new(-0.3, 1.4 + 0.04 * k as f32, 0.2 + 0.06 * k as f32));
        s.step(1);
    }
    s.grip(0, 0.0);
    let st = state(&s);
    assert_eq!(st.flags & (OS_HELD_LEFT | OS_HELD_RIGHT | OS_PULLED), 0, "let go");
    let v = v3(st.lin_vel);
    assert!(v.length() > 2.0 && v.z > 1.0, "the throw's velocity for the game: {v:?}");

    // Then the game has it again.
    let p3 = Vec3::new(-1.1, 0.3, 1.0);
    game_at(&mut s, p3);
    s.step(30);
    assert!(s.pos(PROP).distance(p3) < 0.01, "follows the game again: {:?}", s.pos(PROP));

    // Out of reach on the floor: the gravity gloves bring it.
    // (Looking down its way: the gloves only take what you're looking at.)
    s.f.hmd.rot = (Quat::from_rotation_y(-0.83) * Quat::from_rotation_x(0.5)).to_array();
    let from = reach_at(0, p3, -0.83);
    s.move_hand(0, from, 20);
    s.set_rot(0, Quat::IDENTITY);
    s.step(15);
    assert!(s.out.hand_targeting[0], "pointing at it");
    s.grip(0, 1.0);
    let mut p = s.hand_pos(0);
    for _ in 0..6 {
        p.z -= 2.5 * DT;
        p.y += 1.0 * DT;
        s.set_hand(0, p);
        s.step(1);
    }
    for _ in 0..120 {
        game_at(&mut s, p3); // (the game still says where it was: the runtime has it now)
        s.step(1);
        if s.rig.held(0).is_some() {
            break;
        }
    }
    assert_eq!(s.rig.held(0), Some(PROP), "pulled into the hand: at {:?}", s.pos(PROP));
}

/// A hanging cable segment of the game's (`OBJF_GAME_SIMULATED`), upright in mid-air where the
/// game holds it, and the pushes the rig sends the game for it.
fn cable_sim() -> (Sim, Box<GamePoses>) {
    let at = Vec3::new(-0.3, 1.2, 0.1);
    let mut d = obj(CABLE, OBJ_PROP, at, [0.015, 0.5, 0.015], 10.0);
    d.flags |= OBJF_GAME_SIMULATED;
    let mut s = Sim::with(&[d]);
    let mut poses: Box<GamePoses> = bytemuck::zeroed_box();
    poses.count = 1;
    poses.poses[0] = GamePose { id: CABLE, pose: pose(at, Quat::IDENTITY), ..Default::default() };
    s.rig.set_game_poses(&poses);
    s.step(20);
    (s, poses)
}
const CABLE: u32 = 31;

fn cable_pushes(s: &Sim) -> Vec<GamePush> {
    s.rig.game_pushes.iter().filter(|p| p.id == CABLE).copied().collect()
}

#[test]
fn an_open_hand_sweeping_through_a_game_cable_pushes_it_along_and_isnt_stopped() {
    let (mut s, poses) = cable_sim();
    let cable = Vec3::new(-0.3, 1.2, 0.1);
    s.move_hand(0, cable + Vec3::new(-0.25, 0.0, 0.0), 30);
    s.step(10);
    assert!(cable_pushes(&s).is_empty(), "not touching yet");
    // Through it sideways, no grip, 1.5 m/s.
    let mut pushes = Vec::new();
    let mut p = s.hand_pos(0);
    for _ in 0..30 {
        p.x += 1.5 * DT;
        s.set_hand(0, p);
        s.rig.set_game_poses(&poses); // (the game keeps it where it was: the push is the game's to apply)
        s.step(1);
        pushes.extend(cable_pushes(&s));
    }
    assert!(!pushes.is_empty(), "the hand went through the cable");
    assert_eq!(s.rig.held(0), None, "pushed, not grabbed");
    for g in &pushes {
        let (n, speed) = (Vec3::from_slice(&g.normal[..3]), g.normal[3]);
        assert!(n.x > 0.7, "along the hand's way: {n:?}");
        assert!(speed > 1.0 && speed <= PUSH_MAX, "at least the hand's speed into it: {speed}");
        assert!(Vec3::from_slice(&g.point[..3]).distance(cable) < 0.6, "on the cable: {:?}", g.point);
    }
    let palm = s.rig.physics.pose(s.rig.hands.as_ref().unwrap()[0].body).0;
    assert!(palm.x > cable.x + 0.1, "the hand went on past it: {palm:?}");
    assert!(s.pos(CABLE).distance(cable) < 0.01, "the rig's copy stays where the game has it: {:?}", s.pos(CABLE));
}

#[test]
fn a_fingertip_alone_pushes_a_game_cable() {
    let (mut s, mut poses) = cable_sim();
    s.move_hand(0, Vec3::new(-0.3, 1.2, -0.3), 30);
    s.step(10);
    // The cable put just past the middle fingertip, along the fingers: only that finger reaches it.
    let (palm, body, finger) = {
        let h = &s.rig.hands.as_ref().unwrap()[0];
        (h.collider, h.body, h.fingers[2])
    };
    let (tip_mid, q) = s.rig.physics.pose(finger);
    let along = q * Vec3::Y;
    let at = tip_mid + along * 0.045;
    // Lying through the palm's facing, across the finger's end.
    let facing = s.rig.physics.pose(body).1 * Vec3::X;
    poses.poses[0].pose = pose(at, Quat::from_rotation_arc(Vec3::Y, facing));
    s.rig.set_game_poses(&poses);
    s.step(3);
    let _ = palm;
    // Poke it.
    let mut pushes = Vec::new();
    let mut p = s.hand_pos(0);
    for _ in 0..8 {
        p += along * (0.6 * DT);
        s.set_hand(0, p);
        s.rig.set_game_poses(&poses);
        s.step(1);
        pushes.extend(cable_pushes(&s));
    }
    assert!(!pushes.is_empty(), "the finger touched it");
    let n = Vec3::from_slice(&pushes[0].normal[..3]);
    assert!(n.dot(along) > 0.9, "pushed the way the finger points: {n:?} vs {along:?}");
}

#[test]
fn a_game_prop_collides_as_the_game_shapes_it_not_as_its_box() {
    // A thin rod lying diagonally, 1.1 m long: its box is 80 cm across, the rod 3 cm thick.
    const ROD: u32 = 33;
    let at = Vec3::new(-0.3, 1.2, 0.1);
    let mut d = obj(ROD, OBJ_PROP, at, [0.4, 0.4, 0.015], 10.0);
    d.flags |= OBJF_GAME_SIMULATED;
    d.shape = SHAPE_HULL;
    let mut s = Sim::with(&[d]);
    let mut hulls: Box<GameHulls> = bytemuck::zeroed_box();
    hulls.count = 1;
    hulls.hulls[0].id = ROD;
    let corners: Vec<[f32; 4]> = [-1.0f32, 1.0]
        .iter()
        .flat_map(|&e| [(-0.015f32, -0.015f32), (0.015, -0.015), (-0.015, 0.015), (0.015, 0.015)].map(move |(a, z)| [0.4 * e + a, 0.4 * e - a, z, 0.0]))
        .collect();
    hulls.hulls[0].count = corners.len() as u32;
    hulls.hulls[0].points[..corners.len()].copy_from_slice(&corners);
    s.rig.set_game_hulls(&hulls);
    let mut poses: Box<GamePoses> = bytemuck::zeroed_box();
    poses.count = 1;
    poses.poses[0] = GamePose { id: ROD, pose: pose(at, Quat::IDENTITY), ..Default::default() };
    let pushes = |s: &Sim| s.rig.game_pushes.iter().filter(|p| p.id == ROD).count();
    // In the box's empty corner, 40 cm off the rod: nothing.
    s.rig.set_game_poses(&poses);
    s.move_hand(0, at + Vec3::new(0.3, -0.3, 0.0), 40);
    let mut near = 0;
    for _ in 0..30 {
        s.rig.set_game_poses(&poses);
        s.step(1);
        near += pushes(&s);
    }
    assert_eq!(near, 0, "the hand is nowhere near the rod itself");
    // On the rod: pushed.
    s.move_hand(0, at + Vec3::new(0.05, 0.05, 0.0), 30);
    let mut on = 0;
    for _ in 0..10 {
        s.rig.set_game_poses(&poses);
        s.step(1);
        on += pushes(&s);
    }
    assert!(on > 0, "the hand on the rod pushes it");
}

#[test]
fn a_hanging_cable_swings_from_the_hand_and_stays_hung() {
    const HUNG: u32 = 32;
    let top = Vec3::new(-0.3, 1.7, 0.1);
    let mut d = obj(HUNG, OBJ_PROP, top - Vec3::new(0.0, 0.5, 0.0), [0.015, 0.5, 0.015], 1.0);
    d.flags |= OBJF_HANGING;
    d.handle = [0.0, 0.5, 0.0];
    let mut s = Sim::with(&[d]);
    s.step(30);
    let hung = s.pos(HUNG);
    assert!(hung.distance(top - Vec3::new(0.0, 0.5, 0.0)) < 0.02, "hangs still where it was: {hung:?}");
    // Swept through by an open hand at its lower end (come at from clear of it, left to settle).
    s.move_hand(0, Vec3::new(-0.6, 1.0, -0.4), 30);
    s.move_hand(0, Vec3::new(-0.6, 1.0, 0.1), 30);
    s.step(300);
    let hung = s.pos(HUNG);
    let mut p = s.hand_pos(0);
    let mut furthest = 0.0f32;
    for _ in 0..50 {
        p.x += 1.2 * DT;
        s.set_hand(0, p);
        s.step(1);
        furthest = furthest.max(s.pos(HUNG).x - hung.x);
    }
    assert!(furthest > 0.05, "swung away from the hand: {furthest}");
    for _ in 0..200 {
        s.step(1);
        let reach = s.pos(HUNG).distance(top);
        assert!((reach - 0.5).abs() < 0.03, "still hung from its point: {reach}");
    }
    assert_eq!(s.rig.held(0), None);
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
fn a_swing_lets_go_as_the_grip_eases_but_a_still_hand_or_a_gun_holds_on() {
    let mut s = Sim::new();
    s.move_hand(0, s.pos(BOTTLE) + Vec3::new(0.0, 0.15, 0.0), 30);
    s.grip(0, 1.0);
    s.move_hand(0, Vec3::new(-0.2, 1.3, -0.1), 30);
    s.step(10);
    // Still, easing to 0.7 keeps it.
    s.grip(0, 0.7);
    assert_eq!(s.rig.held(0), Some(BOTTLE), "a still hand keeps it");
    s.grip(0, 1.0);
    // Swinging, easing to 0.7 throws it, and the grip doesn't snatch it back.
    let mut p = s.hand_pos(0);
    for k in 0..8 {
        p.z += 5.0 * DT;
        s.set_hand(0, p);
        s.f.hands[0].squeeze = if k < 6 { 1.0 } else { 0.7 };
        s.step(1);
    }
    assert_eq!(s.rig.held(0), None, "let go mid-swing");
    assert_eq!(s.rig.debug.throws, 1);
    s.step(5);
    assert_eq!(s.rig.held(0), None, "not grabbed again until the grip opens");
    // A gun swung about at 0.7 stays in the hand.
    s.take_rifle();
    s.grip(1, 0.7);
    let mut p = s.hand_pos(1);
    for _ in 0..8 {
        p.z += 4.0 * DT;
        s.set_hand(1, p);
        s.step(1);
    }
    assert_eq!(s.rig.held(1), Some(RIFLE));
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
    s.move_hand(1, s.grip_at(PISTOL) + Vec3::new(0.0, 0.05, 0.0), 40);
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
    // Eject with the left hand right under the gun, palm up (clear of it): the magazine drops into
    // it.
    let well = s.pos(RIFLE) + s.rot(RIFLE) * v3(rifle().mag_well);
    s.set_rot(0, Quat::from_rotation_z(std::f32::consts::FRAC_PI_2));
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
    s.move_hand(1, s.grip_at(PISTOL) + Vec3::new(0.0, 0.05, 0.0), 40);
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

/// Where hand `i` is reaching its arm out at `target` (from its shoulder, the head at 1.7 m turned
/// `yaw` about +Y), half a metre from the shoulder.
fn reach_at(i: usize, target: Vec3, yaw: f32) -> Vec3 {
    let turn = Quat::from_rotation_y(yaw);
    let shoulder = Vec3::new(0.0, 1.45, 0.0) + turn * Vec3::new(if i == 0 { -0.18 } else { 0.18 }, 0.0, 0.0);
    shoulder + (target - shoulder).normalize() * 0.55
}

#[test]
fn gravity_gloves_pull_and_catch() {
    let mut s = Sim::new();
    let target = s.pos(FAR_BOTTLE);
    let from = reach_at(1, target, 0.0);
    s.move_hand(1, from, 20);
    s.set_rot(1, Quat::IDENTITY);
    s.step(15);
    assert!(s.out.hand_targeting[1], "reaching at the far bottle");
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
    // In the palm, not where it was when caught: its centre within its half height (and a bit).
    s.step(40);
    let off = s.pos(FAR_BOTTLE).distance(s.hand_pos(1));
    assert!(off < 0.14, "the caught bottle sits in the hand: {off:.2} m from it");
}

#[test]
fn a_held_rifle_walks_and_turns_with_the_player() {
    let mut s = Sim::new();
    s.s.interaction.weight = 1.0; // as heavy as it gets: it still mustn't trail the player
    s.take_rifle();
    s.step(30);
    let hand = |s: &Sim| s.map.point(s.hand_pos(1));
    let rel = |s: &Sim| s.map.inv_point(s.pos(RIFLE)) - s.hand_pos(1);
    let before = rel(&s);
    // Off at a run, then a turn on the spot.
    for _ in 0..60 {
        s.map.anchor.z += 3.5 * DT;
        s.step(1);
    }
    for _ in 0..20 {
        s.map.body_yaw += 0.08;
        s.step(1);
    }
    let lag = (rel(&s) - before).length();
    assert!(lag < 0.05, "the rifle stays in the hand: {lag:.3} m off (hand at {:?})", hand(&s));
}

#[test]
fn a_held_rifle_stays_in_the_hand_when_the_walk_changes_direction() {
    let mut s = Sim::new();
    // A game's mapping (the rig takes an all-zero one as none yet).
    s.map.origin.y += 0.5;
    s.map.anchor.y += 0.5;
    s.s.interaction.weight = 1.0;
    s.take_rifle();
    s.step(30);
    let rel = |s: &Sim| s.map.inv_point(s.pos(RIFLE)) - s.hand_pos(1);
    let before = rel(&s);
    let mut worst = 0.0f32;
    // Forward, then a sudden sidestep, then back: as a stick swung round.
    for v in [Vec3::new(0.0, 0.0, 3.5), Vec3::new(3.5, 0.0, 0.0), Vec3::new(-2.0, 0.0, -2.0), Vec3::ZERO] {
        for _ in 0..30 {
            s.map.anchor += v * DT;
            s.step(1);
            worst = worst.max((rel(&s) - before).length());
        }
    }
    assert!(worst < 0.02, "the rifle stays in the hand as the walk turns: {worst:.3} m off at worst");
}

#[test]
fn a_seated_magazine_stays_on_its_gun_on_the_walk() {
    let mut s = Sim::new();
    s.map.origin.y += 0.5;
    s.map.anchor.y += 0.5;
    s.take_rifle();
    let mag = s.rig.mag_of(RIFLE).expect("the rifle arrives with a magazine");
    // The magazine in the gun's own frame, as the state the renderer draws it from has it.
    let local = |s: &Sim| {
        let (gp, gq) = s.rig.object_world_pose(RIFLE).unwrap();
        let (mp, _) = s.rig.object_world_pose(mag).unwrap();
        gq.inverse() * (mp - gp)
    };
    let at_rest = local(&s);
    let mut worst = 0.0f32;
    for v in [Vec3::new(0.0, 0.0, 3.5), Vec3::new(3.5, 0.0, 0.0), Vec3::new(-2.0, 0.0, -2.0), Vec3::ZERO] {
        for _ in 0..30 {
            s.map.anchor += v * DT;
            s.step(1);
            worst = worst.max(local(&s).distance(at_rest));
        }
    }
    assert!(worst < 0.001, "the magazine shook {:.1} mm against its gun", worst * 1000.0);
}

#[test]
fn both_hands_on_the_gun_stay_put_on_the_walk() {
    // Held in both hands (the support hand on the foregrip), controllers still: walking off,
    // swinging round and stopping, the hands the renderer draws and the gun stay where they are
    // in the player's own space.
    let mut s = Sim::new();
    s.map.origin.y += 0.5;
    s.map.anchor.y += 0.5;
    s.take_rifle();
    let fore = s.pos(RIFLE) + s.rot(RIFLE) * v3(rifle().foregrip);
    s.move_hand(0, s.map.inv_point(fore), 30);
    s.grip(0, 1.0);
    s.step(90);
    assert!(s.out.primary_weapon.unwrap().two_handed, "both hands on it");
    let seen = |s: &Sim| {
        let (gp, _) = s.rig.object_world_pose(RIFLE).unwrap();
        [v3(s.out.hands[0].pose.pos), v3(s.out.hands[1].pose.pos), s.map.inv_point(gp)]
    };
    let at_rest = seen(&s);
    let mut worst = [0.0f32; 3];
    for v in [Vec3::new(0.0, 0.0, 3.5), Vec3::new(3.5, 0.0, 0.0), Vec3::new(-2.0, 0.0, -2.0), Vec3::ZERO] {
        for _ in 0..30 {
            s.map.anchor += v * DT;
            s.step(1);
            for (k, p) in seen(&s).iter().enumerate() {
                worst[k] = worst[k].max(p.distance(at_rest[k]));
            }
        }
    }
    let mm = worst.map(|w| (w * 1000.0).round());
    assert!(worst.iter().all(|w| *w < 0.003), "moved on the walk (mm): support hand {}, firing hand {}, gun {}", mm[0], mm[1], mm[2]);
}

#[test]
fn snap_turns_keep_the_gun_in_both_hands() {
    let mut s = Sim::new();
    s.map.origin.y += 0.5;
    s.map.anchor.y += 0.5;
    s.take_rifle();
    let fore = s.pos(RIFLE) + s.rot(RIFLE) * v3(rifle().foregrip);
    s.move_hand(0, s.map.inv_point(fore), 30);
    s.grip(0, 1.0);
    s.step(60);
    assert!(s.out.primary_weapon.unwrap().two_handed);
    let seen = |s: &Sim| {
        let (gp, gq) = s.rig.object_world_pose(RIFLE).unwrap();
        (s.map.inv_point(gp), s.map.inv_rot(gq))
    };
    let (p0, q0) = seen(&s);
    // Three snaps right, three back left, a few steps apart.
    for k in 0..6 {
        s.map.body_yaw += if k < 3 { 45f32 } else { -45.0 }.to_radians();
        s.step(8);
        assert_eq!(s.rig.held(1), Some(RIFLE), "snap {k}: still in the grip hand");
        assert!(s.out.primary_weapon.unwrap().two_handed, "snap {k}: still in both hands");
        let (p, q) = seen(&s);
        assert!(p.distance(p0) < 0.005 && q.angle_between(q0) < 0.01, "snap {k}: moved {:.3} m, {:.1} deg", p.distance(p0), q.angle_between(q0).to_degrees());
    }
}

#[test]
fn a_held_rifle_scraping_a_wall_on_the_walk_slides_along_it() {
    let mut s = Sim::new();
    // The same mapping, written as a game would (the rig takes an all-zero one as none yet).
    s.map.origin.y += 0.5;
    s.map.anchor.y += 0.5;
    s.take_rifle();
    // A long wall on the right, along the way the player walks, its face 12 cm right of the rifle.
    let r = s.pos(RIFLE);
    let face = r.x + 0.12;
    world_box(&mut s.rig, 7, [face + 0.05, r.y, r.z + 3.0], [0.05, 1.0, 4.0]);
    s.step(1);
    // The hand pushes the rifle into it, then the player walks on along it.
    s.move_hand(1, Vec3::new(0.32, 1.3, 0.15), 20);
    s.step(20);
    let rel = |s: &Sim| s.map.inv_point(s.pos(RIFLE));
    let mut last = rel(&s);
    let mut worst = 0.0f32;
    for _ in 0..90 {
        s.map.anchor.z += 2.5 * DT;
        s.step(1);
        let now = rel(&s);
        worst = worst.max(now.distance(last));
        last = now;
        assert!(s.pos(RIFLE).x < face, "the wall holds it: {:?}", s.pos(RIFLE));
    }
    assert!(worst < 0.01, "it slides along the wall with the player, no jumps: {:.3} m in a step at worst", worst);
}

#[test]
fn a_held_rifle_walked_into_a_post_isnt_dragged_behind() {
    let mut s = Sim::new();
    s.map.origin.y += 0.5;
    s.map.anchor.y += 0.5;
    s.take_rifle();
    // A post 40 cm ahead of the rifle, right in its way, then a walk on past it.
    let r = s.pos(RIFLE);
    world_box(&mut s.rig, 7, [r.x, r.y, r.z + 0.4], [0.03, 0.5, 0.03]);
    s.step(1);
    let rel = |s: &Sim| s.map.inv_point(s.pos(RIFLE));
    let start = rel(&s);
    let (mut last, mut worst_step, mut worst_off) = (start, 0.0f32, 0.0f32);
    for _ in 0..80 {
        s.map.anchor.z += 1.5 * DT;
        s.step(1);
        let now = rel(&s);
        worst_step = worst_step.max(now.distance(last));
        worst_off = worst_off.max(now.distance(start));
        last = now;
    }
    s.step(30);
    assert!(worst_off < 0.16, "held back {worst_off:.3} m at most: it goes through once the post holds it back too far");
    assert!(worst_step < 0.03, "and never jumps: {worst_step:.3} m in a step at worst");
    assert!(rel(&s).distance(start) < 0.01, "back in the hand past the post: {:.3} m off", rel(&s).distance(start));
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
fn pointing_the_controller_at_a_thing_isnt_reaching_for_it() {
    let mut s = Sim::new();
    let target = s.pos(FAR_BOTTLE);
    let from = Vec3::new(0.2, 1.2, 0.2);
    s.move_hand(1, from, 20);
    // The controller's nose towards it, the palm side-on to it.
    s.set_rot(1, look_rotation(target - from, Vec3::Y));
    s.step(15);
    assert!(!s.out.hand_targeting[1], "only the palm reaches");
}

#[test]
fn a_locked_glove_target_holds_while_the_grip_does_then_flicks_from_wherever_the_hand_is() {
    let mut s = Sim::new();
    let target = s.pos(FAR_BOTTLE);
    let from = reach_at(1, target, 0.0);
    s.move_hand(1, from, 20);
    s.set_rot(1, Quat::IDENTITY);
    s.step(15);
    s.grip(1, 1.0);
    assert!(s.out.hand_targeting[1], "locked");
    // The arm swung right away from it: still locked while the grip holds.
    let away = s.hand_pos(1) + Vec3::new(0.45, 0.0, 0.0);
    s.move_hand(1, away, 60);
    s.step(10);
    assert!(s.out.hand_targeting[1], "the grip keeps the lock");
    assert_eq!(s.rig.debug.pulls, 0);
    // The flick in towards the chest from there.
    let mut p = s.hand_pos(1);
    let head = Vec3::new(0.0, 1.7, 0.0);
    for _ in 0..6 {
        p += (head - p).normalize() * (2.8 * DT);
        s.set_hand(1, p);
        s.step(1);
    }
    assert_eq!(s.rig.debug.pulls, 1, "the flick pulls it");
}

#[test]
fn a_locked_glove_target_hops_and_lets_go_with_the_grip() {
    let mut s = Sim::new();
    let target = s.pos(FAR_BOTTLE);
    let from = reach_at(1, target, 0.0);
    s.move_hand(1, from, 20);
    s.set_rot(1, Quat::IDENTITY);
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
    // The grip opens: a moment later the lock goes.
    s.grip(1, 0.0);
    s.step(30);
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
        let at = self.grip_at(id);
        self.reach(hand, at + Vec3::new(0.0, 0.04, 0.0));
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

#[test]
fn a_knife_in_a_loose_crate_rides_with_it() {
    let mut c = obj(9, OBJ_PROP, Vec3::new(0.0, 0.25, 2.2), [0.25, 0.25, 0.25], 4.0);
    c.flags = OBJF_GRABBABLE;
    let mut s = Sim::with(&[knife(), c]);
    s.take(1, KNIFE);
    s.move_hand(1, Vec3::new(0.1, 1.3, 0.2), 30);
    s.step(20);
    s.throw(1, Vec3::new(-0.05, -0.32, 1.0), 8.0);
    s.step(45);
    let st = s.state(KNIFE);
    assert!(st.flags & OS_STUCK != 0 && st.parent == 9, "stuck in the crate: {:#x} parent {}", st.flags, st.parent);
    let (k0, c0) = (s.pos(KNIFE), s.pos(9));
    s.rig.impulse(9, Vec3::new(6.0, 0.0, 0.0), c0);
    s.step(30);
    let moved = s.pos(9) - c0;
    assert!(moved.x > 0.05, "the crate moved: {moved:?}");
    assert!((s.pos(KNIFE) - k0 - moved).length() < 0.03, "and the knife with it");
}

// ---- guns that don't cycle themselves (`rig::actions`) ------------------------------------------

const DOUBLE: u32 = 20;
const SHELL: u32 = 120;
const BOLT: u32 = 21;
const BOLT_MAG: u32 = 121;
const AIR: u32 = 22;
const AIR_MAG: u32 = 122;

/// A double-barrelled break action lying across the back of the table, barrels to the right: the
/// hinge under the breech, two side-by-side chambers, opening 100 degrees.
fn double() -> ObjectDesc {
    let mut d = obj(DOUBLE, OBJ_WEAPON, Vec3::new(-0.3, 0.87, 0.8), [0.03, 0.06, 0.35], 3.2);
    d.spawn_pose = pose(Vec3::new(-0.3, 0.87, 0.8), Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
    d.flags |= OBJF_TWO_HANDED | OBJF_HOLSTERABLE;
    d.center = [0.0, 0.0, 0.2];
    d.grip = [0.0, -0.05, 0.0];
    d.foregrip = [0.0, -0.04, 0.3];
    d.muzzle = [0.0, 0.0, 0.55];
    d.sight = [0.0, 0.03, 0.0];
    d.handle = [0.0, -0.03, 0.15];
    d.mag_well = [0.02, 0.0, 0.14];
    d.chamber_step = -0.04;
    d.action = ACTION_BREAK;
    d.action_angle = 100f32.to_radians();
    d.weapon_flags = WPN_LOADS_CHAMBERS | WPN_STOCK;
    d.ammo_type = 7;
    d.capacity = 2;
    d.fire_rate = 5.0;
    d
}

/// Its round: a shell, base at the first chamber's mouth (the gun's space).
fn shell() -> ObjectDesc {
    let mut d = obj(SHELL, OBJ_MAGAZINE, Vec3::ZERO, [0.01, 0.01, 0.035], 0.045);
    d.flags |= OBJF_TEMPLATE;
    d.center = [0.02, 0.0, 0.175];
    d.mag_well = [0.02, 0.0, 0.14];
    d.ammo_type = 7;
    d.capacity = 1;
    d.rounds = 1;
    d
}

/// A bolt-action rifle with a five-round magazine: the knob 9 cm right of the bore, lifting 90
/// degrees and pulling back 8 cm.
fn bolt_rifle() -> ObjectDesc {
    let mut d = obj(BOLT, OBJ_WEAPON, Vec3::new(0.0, 0.87, 0.75), [0.03, 0.06, 0.45], 4.0);
    d.spawn_pose = pose(Vec3::new(0.0, 0.87, 0.75), Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
    d.flags |= OBJF_TWO_HANDED;
    d.center = [0.0, 0.0, 0.3];
    d.grip = [0.0, -0.05, 0.0];
    d.foregrip = [0.0, -0.03, 0.4];
    d.mag_well = [0.0, -0.05, 0.15];
    d.muzzle = [0.0, 0.0, 0.75];
    d.sight = [0.0, 0.04, 0.0];
    d.handle = [0.09, 0.0, 0.08];
    d.handle_travel = 0.08;
    d.action = ACTION_BOLT;
    d.action_angle = 90f32.to_radians();
    d.weapon_flags = WPN_NEEDS_RACK | WPN_STOCK;
    d.ammo_type = 8;
    d.capacity = 5;
    d.fire_rate = 2.0;
    d
}

fn bolt_mag() -> ObjectDesc {
    let mut d = obj(BOLT_MAG, OBJ_MAGAZINE, Vec3::ZERO, [0.015, 0.04, 0.03], 0.25);
    d.flags |= OBJF_TEMPLATE;
    d.mag_well = [0.0, 0.04, 0.0];
    d.ammo_type = 8;
    d.capacity = 5;
    d.rounds = 5;
    d
}

/// An air rifle: its pump under the barrel pulled back 10 cm, a quarter of full pressure a stroke.
fn air_rifle() -> ObjectDesc {
    let mut d = rifle();
    d.id = AIR;
    d.spawn_pose = pose(Vec3::new(-0.3, 0.87, 0.82), Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
    d.handle = [0.0, -0.06, 0.45];
    d.handle_travel = 0.1;
    d.weapon_flags = WPN_PNEUMATIC | WPN_AUTOMATIC;
    d.pump_pressure = 0.25;
    d.shot_pressure = 0.1;
    d.min_pressure = 0.15;
    d.ammo_type = 9;
    d.capacity = 15;
    d
}

fn air_mag() -> ObjectDesc {
    let mut d = rifle_mag();
    d.id = AIR_MAG;
    d.ammo_type = 9;
    d.capacity = 15;
    d.rounds = 15;
    d
}

impl Sim {
    /// Picks gun `id` up off the table in the right hand and holds it out level, pointing ahead.
    fn take_gun(&mut self, id: u32) {
        let top = self.grip_at(id) + Vec3::new(0.0, 0.03, 0.0);
        self.reach(1, top);
        self.grip(1, 1.0);
        assert_eq!(self.rig.held(1), Some(id), "picked up {id}");
        // Lifted clear of the table before it's turned into the hand.
        self.move_hand(1, top + Vec3::new(0.0, 0.2, 0.0), 20);
        self.move_hand(1, Vec3::new(0.15, 1.3, 0.25), 30);
        self.step(30);
    }

    fn fire_once(&mut self) {
        self.trigger(1, 1.0);
        self.step(3);
        self.trigger(1, 0.0);
        self.step(30);
    }

    fn weapon(&self, id: u32) -> WeaponState {
        self.rig.weapon_state(id).unwrap().clone()
    }

    /// Reaches over the left shoulder into the pouch and takes what it hands over.
    fn from_the_pouch(&mut self) -> u32 {
        self.move_hand(0, Vec3::new(-0.3, 1.45, 0.1), 20);
        self.move_hand(0, Vec3::new(-0.26, 1.6, -0.06), 20);
        self.grip(0, 1.0);
        self.rig.held(0).expect("something from the pouch")
    }

    /// A snap of the right wrist about the world's X axis (positive: the muzzle down), out and back.
    fn flick(&mut self, rad_per_frame: f32) {
        let base = q4(self.f.hands[1].grip.rot);
        for k in (1..=4).chain((0..4).rev()) {
            self.set_rot(1, Quat::from_rotation_x(rad_per_frame * k as f32) * base);
            self.step(1);
        }
    }
}

#[test]
fn a_double_barrel_breaks_open_throws_its_cases_takes_two_shells_and_snaps_shut() {
    let mut s = Sim::with(&[double(), shell()]);
    s.take_gun(DOUBLE);
    assert_eq!(s.weapon(DOUBLE).chamber_bits(), 0b11, "it arrives with both barrels loaded");
    // One barrel per pull; then a click.
    s.fire_once();
    s.fire_once();
    assert_eq!(s.rig.debug.shots, 2);
    s.fire_once();
    assert_eq!(s.rig.debug.shots, 2, "both barrels fired");
    assert!(s.rig.debug.dry_fires >= 1);
    assert_eq!(s.weapon(DOUBLE).chamber_bits(), 0b11 << CHAMBER_SPENT_SHIFT, "the cases stay in");
    // B: the lever. Held level, the barrels fall open under their weight and the cases fly out.
    s.tap(1, CTRL_B);
    s.step(90);
    let h = s.weapon(DOUBLE).hinge.unwrap();
    assert!(!h.latched && h.angle.to_degrees() > 80.0, "fallen open: {:.1} deg", h.angle.to_degrees());
    assert_eq!(s.rig.debug.case_ejects, 2);
    assert_eq!(s.weapon(DOUBLE).chamber_bits(), 0);
    assert!(s.state(DOUBLE).flags & OS_ACTION_OPEN != 0 && s.state(DOUBLE).slide > 0.8);
    assert!(s.cues.contains(&Cue::BreakOpen) && s.cues.contains(&Cue::CaseEject));
    // It won't fire open.
    s.fire_once();
    assert_eq!(s.rig.debug.shots, 2);
    // Two shells from the pouch, one into each chamber: base to the mouth, along the bore.
    let d = double();
    for k in 0..2 {
        let round = s.from_the_pouch();
        assert!(s.rig.weapon_state(round).is_none(), "a round, not a gun");
        let (gp, gq) = s.rig.object_world_pose(DOUBLE).unwrap();
        let angle = s.weapon(DOUBLE).hinge.unwrap().angle;
        let turn = actions::hinge_rot(&d, angle);
        let bore = gq * turn * Vec3::Z;
        let mouth = gp + gq * actions::on_barrels(&d, angle, actions::chamber_mouth(&d, k));
        let rot = (gq * turn * held_round_rot().inverse()).normalize();
        s.set_rot(0, rot);
        let hand_at = |base: Vec3| base - rot * HELD_ROUND_BASE;
        s.move_hand(0, hand_at(mouth - bore * 0.12), 30);
        s.step(10);
        assert_eq!(s.rig.held(0), Some(round), "not in yet");
        s.move_hand(0, hand_at(mouth + bore * 0.02), 15);
        s.step(3);
        assert_eq!(s.rig.held(0), None, "shell {k} went in");
        s.grip(0, 0.0);
        s.set_rot(0, Quat::IDENTITY);
        s.move_hand(0, Vec3::new(-0.2, 1.1, 0.2), 15);
    }
    assert_eq!(s.weapon(DOUBLE).chamber_bits(), 0b11);
    assert_eq!(s.rig.debug.loads, 2);
    assert!(s.cues.contains(&Cue::ShellIn));
    // A flick up: they swing shut and latch.
    s.flick(-0.15);
    s.step(30);
    assert!(s.weapon(DOUBLE).hinge.unwrap().latched, "flicked shut");
    assert_eq!(s.rig.debug.closes, 1);
    assert!(s.cues.contains(&Cue::BreakClose));
    assert!(s.state(DOUBLE).flags & OS_ACTION_OPEN == 0);
    s.fire_once();
    s.fire_once();
    assert_eq!(s.rig.debug.shots, 4, "both barrels again");
}

#[test]
fn a_live_shell_comes_back_out_of_an_open_chamber_and_live_ones_stay_in_as_it_opens() {
    let mut s = Sim::with(&[double(), shell()]);
    s.take_gun(DOUBLE);
    s.tap(1, CTRL_B);
    s.step(90);
    assert_eq!(s.rig.debug.case_ejects, 0, "unfired shells aren't thrown out");
    assert_eq!(s.weapon(DOUBLE).chamber_bits(), 0b11);
    // The left hand on the first chamber's mouth: grip pulls its shell out.
    let d = double();
    let (gp, gq) = s.rig.object_world_pose(DOUBLE).unwrap();
    let angle = s.weapon(DOUBLE).hinge.unwrap().angle;
    let mouth = gp + gq * actions::on_barrels(&d, angle, actions::chamber_mouth(&d, 0));
    s.move_hand(0, mouth + Vec3::new(0.0, 0.01, 0.0), 30);
    s.grip(0, 1.0);
    let round = s.rig.held(0).expect("the shell is in the hand");
    assert!(s.rig.weapon_state(round).is_none());
    assert_eq!(s.weapon(DOUBLE).chamber_bits(), 0b10, "the first chamber is empty");
    assert_eq!(s.rig.debug.unloads, 1);
    assert!(s.cues.contains(&Cue::ShellOut));
}

#[test]
fn the_support_hand_pulls_the_barrels_open_and_pushes_them_shut() {
    let mut s = Sim::with(&[double(), shell()]);
    s.take_gun(DOUBLE);
    let (gp, gq) = s.rig.object_world_pose(DOUBLE).unwrap();
    s.move_hand(0, gp + gq * v3(double().foregrip) + Vec3::new(0.0, -0.02, 0.0), 30);
    s.grip(0, 1.0);
    assert!(s.out.primary_weapon.unwrap().two_handed, "on the foregrip");
    // The lever: the support hand now has the barrels; pulled down about the hinge they open.
    s.tap(1, CTRL_B);
    let (gp, gq) = s.rig.object_world_pose(DOUBLE).unwrap();
    let hinge = gp + gq * v3(double().handle);
    let at = s.hand_pos(0);
    let down = hinge + gq * Quat::from_rotation_x(50f32.to_radians()) * gq.inverse() * (at - hinge);
    s.move_hand(0, down, 30);
    s.step(5);
    let h = s.weapon(DOUBLE).hinge.unwrap();
    assert!((h.angle.to_degrees() - 50.0).abs() < 8.0, "the barrels followed the hand: {:.1}", h.angle.to_degrees());
    assert!(!s.out.primary_weapon.unwrap().two_handed, "it isn't steering the gun now");
    // Pushed home (a little past, as a hand does), they latch and the hand is on the foregrip again.
    let home = hinge + gq * Quat::from_rotation_x(-10f32.to_radians()) * gq.inverse() * (at - hinge);
    s.move_hand(0, home, 30);
    s.step(5);
    assert!(s.weapon(DOUBLE).hinge.unwrap().latched);
    assert!(s.out.primary_weapon.unwrap().two_handed, "back on the foregrip");
}

#[test]
fn a_bolt_action_needs_its_bolt_worked_between_shots() {
    let mut s = Sim::with(&[bolt_rifle(), bolt_mag()]);
    s.take_gun(BOLT);
    assert_eq!(s.rig.weapon_rounds(BOLT), Some((5, true, false)));
    s.fire_once();
    assert_eq!(s.rig.debug.shots, 1);
    s.fire_once();
    assert_eq!(s.rig.debug.shots, 1, "no second shot without the bolt");
    assert_eq!(s.state(BOLT).chambered, 1 << CHAMBER_SPENT_SHIFT, "a case in the chamber");
    // The left hand on the knob: up, back, forward, down.
    let d = bolt_rifle();
    let knob = |s: &Sim, lift: f32, back: f32| {
        let (gp, gq) = s.rig.object_world_pose(BOLT).unwrap();
        gp + gq * (Cycle::at(lift, back).knob(&d, v3(d.handle)) + Vec3::new(0.02, 0.0, 0.0))
    };
    let start = knob(&s, 0.0, 0.0);
    s.move_hand(0, start, 30);
    s.grip(0, 1.0);
    let path = [(1.0, 0.0), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0)];
    for (lift, back) in path {
        let from = s.weapon(BOLT).cycle.unwrap();
        for k in 1..=12 {
            let t = k as f32 / 12.0;
            // Round the bore for the lift (a straight line would cut inside the knob's circle).
            let at = knob(&s, from.lift + (lift - from.lift) * t, from.back + (back - from.back) * t);
            s.set_hand(0, at);
            s.step(1);
        }
        s.step(3);
    }
    s.grip(0, 0.0);
    let ws = s.weapon(BOLT);
    assert!(ws.cycle.unwrap().closed(), "home and down: {:?}", ws.cycle);
    assert!(ws.chambered && !ws.spent);
    assert_eq!(s.rig.weapon_rounds(BOLT), Some((4, true, false)), "the next round went in");
    assert_eq!(s.rig.debug.cycles, 1);
    assert_eq!(s.rig.debug.case_ejects, 1);
    for c in [Cue::BoltUp, Cue::BoltBack, Cue::BoltForward, Cue::BoltDown, Cue::CaseEject] {
        assert!(s.cues.contains(&c), "{c:?}");
    }
    s.fire_once();
    assert_eq!(s.rig.debug.shots, 2, "worked, it fires again");
}

const REVOLVER: u32 = 23;
const LOADER: u32 = 123;
const PUMP_GUN: u32 = 24;
const PUMP_MAG: u32 = 124;

/// A revolver whose cylinder swings out 105 degrees to the left on a crane under its front.
fn revolver() -> ObjectDesc {
    let mut d = obj(REVOLVER, OBJ_WEAPON, Vec3::new(-0.15, 0.87, 0.82), [0.015, 0.06, 0.13], 1.2);
    d.spawn_pose = pose(Vec3::new(-0.15, 0.87, 0.82), Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
    d.center = [0.0, 0.0, 0.1];
    d.muzzle = [0.0, 0.05, 0.25];
    d.sight = [0.0, 0.07, 0.0];
    d.handle = [-0.005, 0.01, 0.1];
    d.mag_well = [0.0, 0.04, 0.05];
    d.action = ACTION_SWING;
    d.action_angle = 105f32.to_radians();
    d.weapon_flags = WPN_PISTOL;
    d.ammo_type = 10;
    d.capacity = 6;
    d.fire_rate = 4.0;
    d
}

/// Its speed loader: six rounds, its top at the cylinder's back.
fn loader() -> ObjectDesc {
    let mut d = obj(LOADER, OBJ_MAGAZINE, Vec3::ZERO, [0.02, 0.02, 0.02], 0.16);
    d.flags |= OBJF_TEMPLATE;
    d.center = [0.0, 0.04, 0.07];
    d.mag_well = [0.0, 0.04, 0.05];
    d.ammo_type = 10;
    d.capacity = 6;
    d.rounds = 6;
    d
}

/// A pump-action shotgun: its forend under the barrel back 9 cm and forward after every shot.
fn pump_gun() -> ObjectDesc {
    let mut d = bolt_rifle();
    d.id = PUMP_GUN;
    d.handle = [0.0, -0.05, 0.35];
    d.handle_travel = 0.09;
    d.action = ACTION_PUMP;
    d.action_angle = 0.0;
    d.ammo_type = 11;
    d
}

fn pump_mag() -> ObjectDesc {
    let mut d = bolt_mag();
    d.id = PUMP_MAG;
    d.ammo_type = 11;
    d
}

#[test]
fn a_revolvers_cylinder_swings_out_drops_its_cases_takes_a_speed_loader_and_flicks_shut() {
    let mut s = Sim::with(&[revolver(), loader()]);
    s.take_gun(REVOLVER);
    for _ in 0..7 {
        s.fire_once();
    }
    assert_eq!(s.rig.debug.shots, 6, "six, then a click");
    assert!(s.rig.debug.dry_fires >= 1);
    // A with the cylinder shut swings it out (there's nothing to drop yet).
    let mag = s.rig.mag_of(REVOLVER).expect("a speed loader in it");
    s.tap(1, CTRL_A);
    s.step(60);
    let h = s.weapon(REVOLVER).hinge.unwrap();
    assert!(!h.latched && h.angle.to_degrees() > 90.0, "swung out: {:.1}", h.angle.to_degrees());
    assert_eq!(s.rig.mag_of(REVOLVER), Some(mag), "still in it");
    assert!(s.cues.contains(&Cue::SwingOpen));
    // The speed loader swings out with the cylinder.
    let d = revolver();
    let (gp, gq) = s.rig.object_world_pose(REVOLVER).unwrap();
    let well = gp + gq * actions::on_barrels(&d, h.angle, v3(d.mag_well));
    let top = s.pos(mag) + s.rot(mag) * v3(loader().mag_well);
    assert!(top.distance(well) < 0.005, "the loader is in the swung-out cylinder: {top} vs {well}");
    // A again: the cases drop out; the pouch hands a fresh loader, and it goes into the open cylinder.
    s.tap(1, CTRL_A);
    s.step(5);
    assert_eq!(s.rig.mag_of(REVOLVER), None);
    let fresh = s.from_the_pouch();
    assert_ne!(fresh, mag);
    let angle = s.weapon(REVOLVER).hinge.unwrap().angle;
    let (gp, gq) = s.rig.object_world_pose(REVOLVER).unwrap();
    let turn = actions::hinge_rot(&d, angle);
    let rot = (gq * turn * held_mag_rot().inverse()).normalize();
    s.set_rot(0, rot);
    let well = gp + gq * actions::on_barrels(&d, angle, v3(d.mag_well));
    s.move_hand(0, well - rot * HELD_MAG_TOP - gq * turn * Vec3::Z * 0.15, 30);
    s.move_hand(0, well - rot * HELD_MAG_TOP, 15);
    s.step(3);
    assert_eq!(s.rig.mag_of(REVOLVER), Some(fresh), "seated in the swung-out cylinder");
    s.grip(0, 0.0);
    s.set_rot(0, Quat::IDENTITY);
    s.move_hand(0, Vec3::new(-0.2, 1.1, 0.2), 15);
    // A roll of the wrist (the top to the right) swings it home.
    let base = q4(s.f.hands[1].grip.rot);
    for k in (1..=4).chain((0..4).rev()) {
        s.set_rot(1, Quat::from_rotation_z(-0.15 * k as f32) * base);
        s.step(1);
    }
    s.step(30);
    assert!(s.weapon(REVOLVER).hinge.unwrap().latched, "flicked shut");
    assert!(s.cues.contains(&Cue::SwingClose));
    s.fire_once();
    assert_eq!(s.rig.debug.shots, 7, "it fires from the fresh loader");
}

const TUBE: u32 = 25;
const TUBE_SHELL: u32 = 125;

/// A revolving shotgun whose six shells go in one at a time through a port at the top of its
/// cylinder's back.
fn tube_gun() -> ObjectDesc {
    let mut d = bolt_rifle();
    d.id = TUBE;
    d.handle = [0.0; 3];
    d.handle_travel = 0.0;
    d.action = ACTION_SELF_LOADING;
    d.action_angle = 0.0;
    d.mag_well = [0.0, 0.02, 0.2];
    d.weapon_flags = WPN_LOADS_ROUNDS | WPN_STOCK;
    d.ammo_type = 12;
    d.capacity = 6;
    d.fire_rate = 3.0;
    d
}

fn tube_shell() -> ObjectDesc {
    let mut d = shell();
    d.id = TUBE_SHELL;
    d.mag_well = [0.0, 0.02, 0.2];
    d.center = [0.0, 0.02, 0.235];
    d.ammo_type = 12;
    d
}

#[test]
fn a_magazine_that_stays_in_the_gun_takes_its_rounds_one_at_a_time() {
    let mut s = Sim::with(&[tube_gun(), tube_shell()]);
    s.take_gun(TUBE);
    assert_eq!(s.rig.weapon_rounds(TUBE), Some((5, true, false)), "arrives full: five in it, one chambered");
    for _ in 0..7 {
        s.fire_once();
    }
    assert_eq!(s.rig.debug.shots, 6, "six, then a click");
    let d = tube_gun();
    for n in 1..=2 {
        let round = s.from_the_pouch();
        let (gp, gq) = s.rig.object_world_pose(TUBE).unwrap();
        let port = gp + gq * v3(d.mag_well);
        let rot = (gq * held_round_rot().inverse()).normalize();
        s.set_rot(0, rot);
        let hand_at = |base: Vec3| base - rot * HELD_ROUND_BASE;
        s.move_hand(0, hand_at(port - gq * Vec3::Z * 0.12), 30);
        s.step(5);
        assert_eq!(s.rig.held(0), Some(round));
        s.move_hand(0, hand_at(port + gq * Vec3::Z * 0.01), 15);
        s.step(3);
        assert_eq!(s.rig.held(0), None, "shell {n} went in");
        s.grip(0, 0.0);
        s.set_rot(0, Quat::IDENTITY);
        s.move_hand(0, Vec3::new(-0.2, 1.1, 0.2), 15);
    }
    assert_eq!(s.rig.weapon_rounds(TUBE), Some((1, true, false)), "the first went straight under the hammer");
    assert_eq!(s.state(TUBE).rounds, 1);
    s.fire_once();
    s.fire_once();
    s.fire_once();
    assert_eq!(s.rig.debug.shots, 8, "the two it was given");
}

#[test]
fn a_pump_action_needs_its_forend_worked_between_shots() {
    let mut s = Sim::with(&[pump_gun(), pump_mag()]);
    s.take_gun(PUMP_GUN);
    s.fire_once();
    s.fire_once();
    assert_eq!(s.rig.debug.shots, 1, "no second shot without the pump");
    let d = pump_gun();
    let (gp, gq) = s.rig.object_world_pose(PUMP_GUN).unwrap();
    s.move_hand(0, gp + gq * v3(d.handle) + Vec3::new(0.0, -0.02, 0.0), 30);
    s.grip(0, 1.0);
    let (_, gq) = s.rig.object_world_pose(PUMP_GUN).unwrap();
    s.move_hand(0, s.hand_pos(0) - gq * Vec3::Z * 0.095, 10);
    s.move_hand(0, s.hand_pos(0) + gq * Vec3::Z * 0.095, 10);
    s.grip(0, 0.0);
    assert_eq!(s.rig.debug.cycles, 1);
    for c in [Cue::PumpBack, Cue::PumpForward, Cue::CaseEject] {
        assert!(s.cues.contains(&c), "{c:?}");
    }
    assert!(!s.cues.contains(&Cue::BoltUp), "a pump doesn't turn");
    s.fire_once();
    assert_eq!(s.rig.debug.shots, 2, "pumped, it fires again");
}

#[test]
fn an_air_rifle_needs_pumping_and_shots_spend_the_pressure() {
    let mut s = Sim::with(&[air_rifle(), air_mag()]);
    s.take_gun(AIR);
    s.fire_once();
    assert_eq!(s.rig.debug.shots, 0, "no pressure, no shot");
    assert!(s.rig.debug.hisses >= 1 && s.cues.contains(&Cue::AirHiss));
    // Three strokes of the pump.
    let d = air_rifle();
    let (gp, gq) = s.rig.object_world_pose(AIR).unwrap();
    let pump = gp + gq * v3(d.handle);
    s.move_hand(0, pump + Vec3::new(0.0, -0.02, 0.0), 30);
    s.grip(0, 1.0);
    for _ in 0..3 {
        let (_, gq) = s.rig.object_world_pose(AIR).unwrap();
        s.move_hand(0, s.hand_pos(0) - gq * Vec3::Z * 0.1, 10);
        s.move_hand(0, s.hand_pos(0) + gq * Vec3::Z * 0.1, 10);
    }
    s.grip(0, 0.0);
    assert_eq!(s.rig.debug.pumps, 3);
    assert!((s.state(AIR).fuse - 0.75).abs() < 1e-4, "pressure {}", s.state(AIR).fuse);
    s.fire_once();
    assert_eq!(s.rig.debug.shots, 1);
    assert!((s.rig.debug.last_strength - 0.75).abs() < 1e-4);
    assert!((s.state(AIR).fuse - 0.65).abs() < 1e-4, "a shot spent a tenth");
}

/// A gun with the game's ammunition (`OBJF_GAME_AMMO`, as a game's adapter registers its gun): it
/// arrives (and comes back when the game respawns it) holding every round the game's gun does, one
/// chambered and the rest in its magazine; and the pouch hands what the game's own reload would
/// load, which the game rewrites in its template as its ammunition changes: no respawning, and
/// nothing at all once the game's reserve is gone.
#[test]
fn a_gun_with_the_games_ammo_takes_its_rounds_from_the_game() {
    const GAME_RIFLE: u32 = 9;
    const GAME_MAG: u32 = 101;
    let mut game_rifle = rifle();
    game_rifle.id = GAME_RIFLE;
    game_rifle.flags |= OBJF_GAME_AMMO;
    game_rifle.spawn_pose = pose(Vec3::new(-0.3, 0.87, 0.6), Quat::IDENTITY);
    game_rifle.ammo_type = 3;
    game_rifle.capacity = 40;
    game_rifle.rounds = 18;
    let mut game_mag = rifle_mag();
    game_mag.id = GAME_MAG;
    game_mag.ammo_type = 3;
    game_mag.capacity = 40;
    game_mag.rounds = 40;
    let mut s = Sim::with(&[game_rifle, game_mag]);
    // (rounds in the chamber, rounds in the magazine)
    let held = |s: &Sim| {
        let st = s.state(GAME_RIFLE);
        (st.chambered & CHAMBER_LIVE_MASK != 0, st.rounds)
    };
    assert_eq!(held(&s), (true, 17), "18 in all: one chambered, 17 in the magazine");
    assert_eq!(s.state(RIFLE).rounds, 5, "a gun without the flag still arrives with a full magazine");
    let k = (0..s.table.count as usize).find(|&k| s.table.descs[k].id == GAME_RIFLE).unwrap();
    let m = (0..s.table.count as usize).find(|&k| s.table.descs[k].id == GAME_MAG).unwrap();
    // The game respawns it: with one round (chambered), then with none (empty and empty).
    for (rounds, want) in [(1, (true, 0)), (0, (false, 0)), (18, (true, 17))] {
        s.table.descs[k].rounds = rounds;
        s.table.descs[k].spawn_seq += 1;
        s.table.seq += 1;
        s.rig.sync(&s.table);
        s.step(45);
        assert_eq!(held(&s), want, "respawned with {rounds} in all");
        assert_eq!(s.state(RIFLE).rounds, 5, "the other gun isn't touched");
    }
    // Its reload would now load 22 (only the template changes): that's what the pouch hands.
    s.table.descs[m].rounds = 22;
    s.table.seq += 1;
    s.rig.sync(&s.table);
    s.take(1, GAME_RIFLE);
    s.move_hand(1, Vec3::new(0.2, 1.3, 0.15), 30);
    let mag = s.from_the_pouch();
    assert_eq!(s.state(mag).rounds, 22, "the template's rounds, not its capacity");
    // The game's reserve runs out: the pouch is empty.
    s.grip(0, 0.0);
    s.move_hand(0, Vec3::new(-0.2, 1.1, 0.2), 20);
    s.table.descs[m].rounds = 0;
    s.table.seq += 1;
    s.rig.sync(&s.table);
    let spawned = s.rig.debug.mags_spawned;
    s.cues.clear();
    s.move_hand(0, Vec3::new(-0.3, 1.45, 0.1), 20);
    s.move_hand(0, Vec3::new(-0.26, 1.6, -0.06), 20);
    s.grip(0, 1.0);
    assert_eq!(s.rig.held(0), None, "nothing out of an empty pouch");
    assert_eq!(s.rig.debug.mags_spawned, spawned);
    assert!(s.cues.contains(&Cue::Fail));
    assert_eq!(held(&s), (true, 17), "and the gun keeps what it has");
}

/// The magazine comes out of a gun set up as a game's adapter registers it (the game's ammunition,
/// drawn by the runtime, the table rewritten as the game's ammunition changes with every shot): A
/// on the gun hand ejects it whatever the table does, the pouch's magazine seats in its place, and
/// the gun fires from the new one. (A run where A did nothing was the headset emulator's: the
/// capture it saved at that moment held the frames up through the whole press, so no frame saw A.
/// Its script clock now stops for captures.)
#[test]
fn a_game_ammo_gun_ejects_its_magazine_while_the_game_rewrites_the_table() {
    const GAME_RIFLE: u32 = 9;
    const GAME_MAG: u32 = 101;
    let mut gun = rifle();
    gun.id = GAME_RIFLE;
    gun.flags |= OBJF_GAME_AMMO | OBJF_RUNTIME_DRAWS;
    gun.spawn_pose = pose(Vec3::new(-0.3, 0.87, 0.6), Quat::IDENTITY);
    gun.ammo_type = 3;
    gun.capacity = 40;
    gun.rounds = 18;
    let mut mag = rifle_mag();
    mag.id = GAME_MAG;
    mag.flags |= OBJF_RUNTIME_DRAWS;
    mag.ammo_type = 3;
    mag.capacity = 40;
    mag.rounds = 39;
    let mut s = Sim::with(&[gun, mag]);
    let k = (0..s.table.count as usize).find(|&k| s.table.descs[k].id == GAME_RIFLE).unwrap();
    let m = (0..s.table.count as usize).find(|&k| s.table.descs[k].id == GAME_MAG).unwrap();
    // The game's rewrite: its gun's rounds and what its reload would load (the same spawn).
    let rewrite = |s: &mut Sim| {
        let rounds = s.state(GAME_RIFLE).rounds + 1;
        s.table.descs[k].rounds = rounds;
        s.table.descs[m].rounds = 39;
        s.table.seq += 1;
        s.rig.sync(&s.table);
    };
    let first = s.rig.mag_of(GAME_RIFLE).expect("it arrives with a magazine");
    s.take(1, GAME_RIFLE);
    s.move_hand(1, Vec3::new(0.15, 1.3, 0.25), 30);
    for _ in 0..3 {
        s.fire_once();
        rewrite(&mut s);
    }
    assert_eq!(s.rig.debug.shots, 3);
    assert_eq!(s.rig.mag_of(GAME_RIFLE), Some(first));
    // A, held for a fifth of a second as the emulator's script holds it, the table rewritten as it
    // goes.
    s.f.hands[1].buttons |= CTRL_A;
    for _ in 0..20 {
        s.step(1);
        rewrite(&mut s);
    }
    s.f.hands[1].buttons &= !CTRL_A;
    s.step(10);
    assert_eq!(s.rig.mag_of(GAME_RIFLE), None, "A ejected the magazine");
    assert_eq!(s.rig.debug.ejects, 1);
    rewrite(&mut s);
    s.step(10);
    assert_eq!(s.rig.mag_of(GAME_RIFLE), None, "and a table rewrite doesn't put it back");
    // A fresh one from the pouch goes in, and the gun fires from it.
    let fresh = s.from_the_pouch();
    assert_eq!(s.state(fresh).rounds, 39);
    let well = s.pos(GAME_RIFLE) + s.rot(GAME_RIFLE) * v3(rifle().mag_well);
    let rot = (s.rot(GAME_RIFLE) * held_mag_rot().inverse()).normalize();
    s.set_rot(0, rot);
    s.move_hand(0, well - rot * HELD_MAG_TOP - Vec3::Y * 0.12, 30);
    s.move_hand(0, well - rot * HELD_MAG_TOP, 15);
    s.step(3);
    assert_eq!(s.rig.mag_of(GAME_RIFLE), Some(fresh), "the pouch's magazine seated");
    let shots = s.rig.debug.shots;
    s.fire_once();
    assert_eq!(s.rig.debug.shots, shots + 1, "and it fires from it");
}

fn valve_desc(id: u32, centre: Vec3, radius: f32, friction: f32) -> ObjectDesc {
    let mut d = ObjectDesc::zeroed();
    d.id = id;
    d.kind = OBJ_VALVE;
    d.spawn_seq = 1;
    // On a wall ahead, its axis towards the player.
    d.spawn_pose = pose(centre, Quat::from_rotation_y(std::f32::consts::PI));
    d.radius = radius;
    d.half_extents = [radius, radius, 0.02];
    d.mass = 2.0;
    d.friction = friction;
    d
}

/// Takes the valve's rim at its top with the right hand, then goes a quarter turn round it.
fn turn_valve(s: &mut Sim, centre: Vec3, radius: f32) {
    s.move_hand(1, centre + Vec3::new(0.0, radius + 0.1, -0.15), 30);
    s.move_hand(1, centre + Vec3::new(0.0, radius, -0.02), 20);
    s.grip(1, 1.0);
    s.step(20);
    for k in 1..=90 {
        let a = std::f32::consts::FRAC_PI_2 * k as f32 / 90.0;
        // Clockwise as the player sees it: from the top round to their right.
        s.set_hand(1, centre + Vec3::new(radius * a.sin(), radius * a.cos(), -0.02));
        s.step(1);
    }
}

#[test]
fn a_switch_knob_is_twisted_on_by_the_wrist_and_clicks_in() {
    // A rotary switch: a 3 cm knob on the wall ahead, a quarter turn from off to on.
    let centre = Vec3::new(0.25, 1.3, 0.55);
    let mut d = valve_desc(44, centre, 0.03, 0.05);
    (d.turn_min, d.turn_max, d.detent) = (0.0, std::f32::consts::FRAC_PI_2, 0.8);
    let mut s = Sim::with(&[d]);
    s.step(5);
    assert!(s.rig.objects[&44].valve.unwrap().twist, "that small, it's twisted");
    s.move_hand(1, centre + Vec3::new(0.0, 0.03, -0.15), 30);
    s.move_hand(1, centre + Vec3::new(0.0, 0.03, -0.03), 20);
    s.grip(1, 1.0);
    s.step(20);
    assert!(s.state(44).flags & OS_HELD_RIGHT != 0, "the hand has the knob");
    // The wrist turns a quarter turn and a bit about the knob's axle (towards the player).
    let axis = s.rig.objects[&44].valve.unwrap().axis();
    for k in 1..=45 {
        let a = 1.8 * k as f32 / 45.0;
        s.set_rot(1, Quat::from_axis_angle(axis, a));
        s.step(1);
    }
    s.step(30);
    let on = s.state(44).slide;
    assert!((on - std::f32::consts::FRAC_PI_2).abs() < 0.02, "turned on, to its stop: {on}");
    // Let go: it stays on, clicked in.
    s.grip(1, 0.0);
    s.step(90);
    let still = s.state(44).slide;
    assert!((still - std::f32::consts::FRAC_PI_2).abs() < 0.03, "and stays on: {still}");
}

#[test]
fn a_valve_turns_with_the_hand_on_its_rim() {
    let centre = Vec3::new(0.3, 1.2, 0.6);
    let mut s = Sim::with(&[valve_desc(40, centre, 0.2, 0.5)]);
    s.step(5);
    turn_valve(&mut s, centre, 0.2);
    let v = s.rig.objects[&40].valve.unwrap();
    assert!((v.angle.abs() - std::f32::consts::FRAC_PI_2).abs() < 0.2, "a quarter turn: {}", v.angle);
    let hand = v3(s.out.hands[1].pose.pos);
    assert!(hand.distance(centre) - 0.2 < 0.01, "the hand on the rim: {hand:?}");
    assert!(hand.distance(s.hand_pos(1)) < 0.05, "with the controller");
    // Let go: it stops, the hand's free.
    s.grip(1, 0.0);
    s.step(60);
    let a = s.rig.objects[&40].valve.unwrap().angle;
    s.step(30);
    assert!((s.rig.objects[&40].valve.unwrap().angle - a).abs() < 1e-3, "stopped");
}

#[test]
fn a_turning_valve_tells_the_game_who_has_it_how_far_and_how_fast() {
    // Geared to a game's mechanism at half a radian a second.
    let centre = Vec3::new(0.3, 1.2, 0.6);
    let mut d = valve_desc(43, centre, 0.2, 0.5);
    d.fire_rate = 0.5;
    let mut s = Sim::with(&[d]);
    s.step(5);
    assert_eq!(s.state(43).flags & (OS_HELD_LEFT | OS_HELD_RIGHT), 0, "nobody has it yet");
    s.move_hand(1, centre + Vec3::new(0.0, 0.3, -0.15), 30);
    s.move_hand(1, centre + Vec3::new(0.0, 0.2, -0.02), 20);
    s.grip(1, 1.0);
    s.step(20);
    for k in 1..=45 {
        let a = std::f32::consts::FRAC_PI_2 * k as f32 / 45.0;
        s.set_hand(1, centre + Vec3::new(0.2 * a.sin(), 0.2 * a.cos(), -0.02));
        s.step(1);
    }
    let st = s.state(43);
    let v = s.rig.objects[&43].valve.unwrap();
    assert!(st.flags & OS_HELD_RIGHT != 0, "the right hand has it");
    assert_eq!(st.slide, v.angle, "how far it's turned");
    let w = v3(st.ang_vel);
    assert!(w.dot(v.axis()).abs() > 0.4 && w.length() <= 0.5 + 1e-4, "turning along its axis, no faster than its gearing: {w:?}");
    assert!(v.angle.abs() < 0.5, "half a second's worth at most: {}", v.angle);
    s.grip(1, 0.0);
    s.step(5);
    assert_eq!(s.state(43).flags & (OS_HELD_LEFT | OS_HELD_RIGHT), 0, "let go");
}

#[test]
fn a_stiff_valve_holds_the_hand_on_its_rim_until_the_body_turns_it() {
    // 60 N m on a 20 cm rim: more than an arm's 36. Past the arm's stretch the body brings it round
    // with the controller, a stretch behind it.
    let centre = Vec3::new(0.3, 1.2, 0.6);
    let mut s = Sim::with(&[valve_desc(41, centre, 0.2, 60.0)]);
    s.step(5);
    turn_valve(&mut s, centre, 0.2);
    let v = s.rig.objects[&41].valve.unwrap();
    let behind = (valve::ARM_STRETCH / 0.2).asin();
    assert!((v.angle.abs() - (std::f32::consts::FRAC_PI_2 - behind)).abs() < 0.05, "the body turns it, a stretch behind: {}", v.angle);
    let (hand, controller) = (v3(s.out.hands[1].pose.pos), centre + Vec3::new(0.2, 0.0, -0.02));
    assert!(hand.distance(controller) < valve::ARM_STRETCH + 0.01, "the hand on its rim, a stretch from the controller: {hand:?}");
}

#[test]
fn a_valve_on_a_board_to_the_side_is_taken_by_its_rim() {
    // As the range has them: on a board, the axle out of it along -X.
    let centre = Vec3::new(1.0, 1.0, 0.3);
    let mut d = valve_desc(42, centre, 0.18, 0.3);
    d.spawn_pose = pose(centre, Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2));
    let mut s = Sim::with(&[d]);
    world_box(&mut s.rig, 9, [1.11, 1.0, 0.3], [0.05, 1.0, 1.0]);
    s.step(5);
    let top = centre + Vec3::new(-0.02, 0.18, 0.0);
    s.move_hand(1, top + Vec3::new(-0.15, 0.08, 0.0), 30);
    s.move_hand(1, top, 20);
    let hover = s.rig.hands.as_ref().unwrap()[1].hover;
    let v = s.rig.objects[&42].valve.unwrap();
    eprintln!("hover {hover:?}, rim distance {:?}, palm {:?}", v.nearest(s.rig.hands.as_ref().unwrap()[1].target.0), s.rig.hands.as_ref().unwrap()[1].target.0);
    assert_eq!(hover, Hover::Valve(42));
}

#[test]
fn reaching_the_arm_out_at_a_thing_targets_it_however_the_controller_sits() {
    let mut s = Sim::new();
    let bottle = s.pos(FAR_BOTTLE);
    // The right arm reaching out at it from the shoulder, the controller held as it naturally is
    // (nose ahead, palm to the side): neither the nose nor the palm is on it.
    let shoulder = Vec3::new(0.18, 1.45, 0.0);
    let hand = shoulder + (bottle - shoulder).normalize() * 0.55;
    s.move_hand(1, hand, 25);
    s.set_rot(1, Quat::IDENTITY);
    s.step(15);
    assert!(s.out.hand_targeting[1], "the reach finds it");
    // The same reach with the head turned well away: it isn't something you're looking at.
    s.f.hmd.rot = Quat::from_rotation_y(-1.6).to_array();
    s.step(15);
    assert!(!s.out.hand_targeting[1], "only what you look at");
}

#[test]
fn a_hand_glides_onto_a_valve_rim_over_the_grab_transition() {
    let centre = Vec3::new(0.3, 1.2, 0.6);
    let mut s = Sim::with(&[valve_desc(43, centre, 0.2, 0.5)]);
    s.step(5);
    // A hand 6 cm off the rim's top takes hold.
    let off = centre + Vec3::new(0.0, 0.26, -0.03);
    s.move_hand(1, off, 30);
    s.step(5);
    let rim = centre + Vec3::new(0.0, 0.2, 0.0);
    let before = v3(s.out.hands[1].pose.pos);
    s.grip(1, 1.0);
    let first = v3(s.out.hands[1].pose.pos);
    assert!(first.distance(before) < 0.03, "no jump the frame it takes hold: {before:?} -> {first:?}");
    s.step(20);
    assert!(v3(s.out.hands[1].pose.pos).distance(rim) < 0.01, "on the rim after the transition");
}

#[test]
fn transitions_take_longer_over_longer_distances() {
    assert_eq!(travel_time(0.15, 0.05), 0.15, "a short way: the setting");
    assert!((travel_time(0.15, 0.6) - 0.45).abs() < 1e-6, "three times as far, three times as long");
    assert!((travel_time(0.3, 0.6) - 0.9).abs() < 1e-6, "a slower setting, slower all the way");
    assert_eq!(travel_time(0.0, 0.6), 0.0, "none: at once");
}

#[test]
fn letting_go_of_a_valve_far_from_the_controller_glides_the_hand_back_at_a_hands_pace() {
    let centre = Vec3::new(0.3, 1.2, 0.6);
    let mut s = Sim::with(&[valve_desc(44, centre, 0.2, 60.0)]);
    s.step(5);
    turn_valve(&mut s, centre, 0.2);
    // The stiff valve held the hand on its rim; the controller went on round, and back out off
    // the wheel towards the player.
    let off = s.hand_pos(1) + Vec3::new(0.0, 0.0, -0.25);
    s.move_hand(1, off, 15);
    let hand = v3(s.out.hands[1].pose.pos);
    let controller = s.hand_pos(1);
    let apart = hand.distance(controller);
    assert!(apart > 0.25, "held back: {apart}");
    s.grip(1, 0.0);
    let first = v3(s.out.hands[1].pose.pos);
    assert!(first.distance(hand) < 0.05, "no jump on letting go: {:?} -> {first:?}", hand);
    // Half the way's time in, part way there.
    let time = travel_time(Settings::default().interaction.grab_transition, apart);
    s.step((time * 0.5 / DT) as usize);
    let mid = v3(s.out.hands[1].pose.pos);
    assert!(mid.distance(controller) > 0.05 && mid.distance(controller) < apart - 0.05, "on its way: {mid:?}");
    s.step((time * 0.6 / DT) as usize + 2);
    assert!(v3(s.out.hands[1].pose.pos).distance(controller) < 0.01, "back with the controller");
}

#[test]
fn a_thing_to_take_is_stowed_in_any_slot_even_a_full_one() {
    let mut loot = obj(BOTTLE, OBJ_PROP, Vec3::new(0.0, 0.9, 0.5), [0.03, 0.1, 0.03], 0.5);
    loot.flags |= OBJF_LOOT | OBJF_HOLSTERABLE | OBJF_PULLABLE;
    let mut s = Sim::with(&[]);
    s.table.descs[0] = loot;
    s.table.seq += 1;
    s.rig.sync(&s.table);
    s.step(20);
    // The pistol on the right hip first.
    s.move_hand(1, s.grip_at(PISTOL) + Vec3::new(0.0, 0.05, 0.0), 40);
    s.grip(1, 1.0);
    s.move_hand(1, Vec3::new(0.2, 1.3, 0.2), 20);
    s.move_hand(1, Vec3::new(0.21, 1.16, 0.04), 30);
    s.grip(1, 0.0);
    assert_eq!(s.rig.holstered(SLOT_HIP_RIGHT), Some(PISTOL));
    s.move_hand(1, Vec3::new(0.2, 1.1, 0.2), 20);
    // The bottle into the same, full, slot: stowed (the pistol stays).
    s.take(1, BOTTLE);
    s.move_hand(1, Vec3::new(0.2, 1.3, 0.2), 20);
    s.move_hand(1, Vec3::new(0.21, 1.16, 0.04), 30);
    s.grip(1, 0.0);
    let st = s.state(BOTTLE);
    assert!(st.flags & OS_STOWED != 0 && st.slot == SLOT_HIP_RIGHT, "stowed: {:#x} slot {}", st.flags, st.slot);
    assert!(st.flags & (OS_HELD_LEFT | OS_HELD_RIGHT | OS_HOLSTERED | OS_RUNTIME_DRAWN) == 0);
    assert_eq!(s.rig.holstered(SLOT_HIP_RIGHT), Some(PISTOL), "the pistol is still there");
    assert_eq!(s.rig.debug.stowed, 1);
    // It doesn't fall or come back to a hand.
    let at = s.pos(BOTTLE);
    s.step(60);
    assert!(s.pos(BOTTLE).distance(at) < 1e-3);
    assert_eq!(s.rig.held(1), None);
}

// ---- taking hold anywhere, on handles, with both hands (`rig::grips`) ------------------------------

const SWORD: u32 = 40;
const PLANK: u32 = 41;

/// A sword lying flat on the table, its blade towards -X: the handle from the pommel (-11 cm) to the
/// guard (-1.2 cm) along its +Z, the grip in the middle of it.
fn sword() -> ObjectDesc {
    let mut d = obj(SWORD, OBJ_MELEE, Vec3::new(0.05, 0.83, 0.8), [0.06, 0.02, 0.4], 1.2);
    d.spawn_pose = pose(Vec3::new(0.05, 0.83, 0.8), Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2));
    d.center = [0.0, 0.0, 0.23];
    d.grip = [0.0, 0.0, -0.06];
    d.muzzle = [0.0, 0.0, 0.63];
    d.grip_rot = rustvr_guns::fist(Vec3::Z, Vec3::Y).to_array();
    d.grip_line = [[0.0, 0.0, -0.11], [0.0, 0.0, -0.012]];
    d.grip_radius = 0.014;
    d
}

impl Sim {
    /// Where hand `i` holds thing `id` (its drawn grip pose in the thing's space).
    fn held_at(&self, i: usize, id: u32) -> (Vec3, Quat) {
        let (p, q) = self.rig.object_world_pose(id).unwrap();
        let h = self.out.hands[i].pose;
        (q.inverse() * (v3(h.pos) - p), (q.inverse() * q4(h.rot)).normalize())
    }

    /// A point on thing `id` (its space) in the world.
    fn on(&self, id: u32, local: Vec3) -> Vec3 {
        self.pos(id) + self.rot(id) * local
    }
}

#[test]
fn the_support_hand_takes_the_handguard_anywhere_along_it() {
    let mut s = Sim::new();
    s.take_rifle();
    // Taken a few cm off the handguard and 3 cm ahead of the game's foregrip: the drawn hand goes
    // onto the handguard there, as the gun's support hold, slid along.
    s.move_hand(0, s.on(RIFLE, v3(rifle().foregrip) + Vec3::new(-0.04, -0.02, 0.03)), 30);
    s.grip(0, 1.0);
    s.step(5);
    let want = v3(rifle().foregrip) + Vec3::new(0.0, 0.0, 0.03);
    assert!(s.held_at(0, RIFLE).0.distance(want) < 0.005, "on the handguard where it took it: {:?}", s.held_at(0, RIFLE).0);
    assert!(s.out.primary_weapon.unwrap().two_handed);
    // It stays where it took it: moving the controller along the barrel doesn't slide it.
    let fwd = s.rot(RIFLE) * Vec3::Z;
    s.move_hand(0, s.hand_pos(0) + fwd * 0.06, 20);
    s.step(5);
    assert!(s.held_at(0, RIFLE).0.distance(want) < 0.005, "still there: {:?}", s.held_at(0, RIFLE).0);
    // Pulled well away, it lets go.
    s.move_hand(0, s.hand_pos(0) + Vec3::new(-0.5, -0.2, 0.0), 20);
    assert!(!s.out.primary_weapon.unwrap().two_handed, "let go of the handguard");
    s.grip(0, 0.0);
    s.step(60);
    // Taken by the receiver instead (well off the handguard): held there, and it still aims the gun.
    let side = s.on(RIFLE, Vec3::new(-0.06, 0.0, 0.18));
    s.move_hand(0, side, 30);
    s.grip(0, 1.0);
    s.step(5);
    assert!(s.out.primary_weapon.unwrap().two_handed, "a second hand anywhere on it");
    let (at, _) = s.held_at(0, RIFLE);
    // (Just where the hand was on it: it doesn't move onto it.)
    assert!(at.distance(Vec3::new(-0.06, 0.0, 0.18)) < 0.01, "held on the receiver's side where it took it: {at:?}");
}

#[test]
fn a_gun_taken_anywhere_but_its_grip_is_held_there_and_the_trigger_does_nothing() {
    let mut s = Sim::new();
    // By the barrel, well ahead of the grip.
    let barrel = s.on(RIFLE, Vec3::new(0.0, 0.04, 0.5));
    s.reach(1, barrel);
    let before = s.rot(RIFLE);
    s.grip(1, 1.0);
    assert_eq!(s.rig.held(1), Some(RIFLE));
    s.move_hand(1, s.hand_pos(1) + Vec3::new(0.0, 0.3, 0.0), 30);
    s.step(20);
    assert!(s.rot(RIFLE).angle_between(before) < 0.1, "held as it lay, not turned into a firing grip");
    let (at, _) = s.held_at(1, RIFLE);
    assert!((at.z - 0.5).abs() < 0.03, "by the barrel where it took it: {at:?}");
    assert!(s.out.primary_weapon.is_none(), "not held to fire");
    assert_ne!(s.out.holds[1], rustvr_ik::HoldShape::PistolGrip);
    s.trigger(1, 1.0);
    s.step(5);
    s.trigger(1, 0.0);
    assert_eq!(s.rig.debug.shots, 0, "the finger isn't on the trigger");
}

#[test]
fn the_other_hand_takes_the_grip_of_a_gun_held_anywhere_and_fires_it() {
    let mut s = Sim::new();
    s.reach(0, s.on(RIFLE, Vec3::new(0.0, 0.04, 0.4)));
    s.grip(0, 1.0);
    assert_eq!(s.rig.held(0), Some(RIFLE));
    s.move_hand(0, Vec3::new(-0.05, 1.3, 0.55), 30);
    s.step(20);
    // The right hand onto the pistol grip: it's the gun hand now, the left holds on where it was.
    let grip = s.grip_at(RIFLE);
    s.move_hand(1, grip + Vec3::new(0.02, -0.02, -0.02), 40);
    s.grip(1, 1.0);
    s.step(20);
    assert_eq!(s.rig.held(1), Some(RIFLE), "the grip hand holds it");
    assert_eq!(s.rig.held(0), None);
    let w = s.out.primary_weapon.expect("held to fire");
    assert!(w.two_handed, "the left hand supports it where it was");
    assert!(s.grip_at(RIFLE).distance(s.hand_pos(1)) < 0.03, "the grip in the right hand");
    s.fire_once();
    assert_eq!(s.rig.debug.shots, 1);
    // The gun hand lets go: the left keeps it, where it has it.
    let left_at = s.held_at(0, RIFLE).0;
    s.grip(1, 0.0);
    s.step(10);
    assert_eq!(s.rig.held(0), Some(RIFLE));
    assert!(s.held_at(0, RIFLE).0.distance(left_at) < 0.01, "not moved in the hand");
}

#[test]
fn anything_goes_from_hand_to_hand() {
    let mut s = Sim::new();
    s.reach(0, s.pos(BOTTLE) + Vec3::new(0.0, 0.12, 0.0));
    s.grip(0, 1.0);
    s.move_hand(0, Vec3::new(-0.05, 1.3, 0.35), 30);
    s.step(20);
    let at = s.pos(BOTTLE);
    // The right hand takes it round the middle: both hands have it.
    s.move_hand(1, at + Vec3::new(0.05, 0.0, 0.0), 30);
    s.grip(1, 1.0);
    s.step(5);
    assert_eq!(s.rig.held(0), Some(BOTTLE));
    assert!(s.out.hands[1].flags & HAND_HOLDING != 0 && s.out.hands[1].holding == BOTTLE, "both hands on it");
    // The left lets go: the right has it, where it took it; it settles into that palm without a jump.
    s.grip(0, 0.0);
    let mut worst = 0.0f32;
    for _ in 0..30 {
        let p = s.pos(BOTTLE);
        s.step(1);
        worst = worst.max(s.pos(BOTTLE).distance(p));
    }
    assert_eq!(s.rig.held(1), Some(BOTTLE), "handed over");
    assert_eq!(s.rig.held(0), None);
    assert!(worst < 0.01, "no jump: {worst:.3} m in a step at worst");
    assert!(s.pos(BOTTLE).distance(at) < 0.06, "where the right hand took it: {:?} vs {at:?}", s.pos(BOTTLE));
    // And it goes with the right hand.
    s.move_hand(1, s.hand_pos(1) + Vec3::new(0.2, 0.0, 0.0), 30);
    s.step(10);
    assert!(s.pos(BOTTLE).x > at.x + 0.15, "carried by the right hand: {:?}", s.pos(BOTTLE));
}

#[test]
fn a_long_thing_in_both_hands_turns_with_both() {
    let mut plank = obj(PLANK, OBJ_PROP, Vec3::new(-0.2, 0.83, 0.8), [0.35, 0.02, 0.05], 1.0);
    plank.flags = OBJF_GRABBABLE;
    let mut s = Sim::with(&[plank]);
    let (l, r) = (s.on(PLANK, Vec3::new(-0.28, 0.0, 0.0)), s.on(PLANK, Vec3::new(0.28, 0.0, 0.0)));
    // Both palms down on its ends (the right hand rests on it once the left has it).
    s.set_rot(0, Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2));
    s.set_rot(1, Quat::from_rotation_z(std::f32::consts::FRAC_PI_2));
    s.reach(0, l + Vec3::new(0.0, 0.04, 0.0));
    s.grip(0, 1.0);
    s.move_hand(1, r + Vec3::new(0.0, 0.04, 0.0), 40);
    s.grip(1, 1.0);
    // Lift both, then raise the right end 20 cm: it tilts up to the right.
    s.move_hand(0, s.hand_pos(0) + Vec3::new(0.0, 0.3, 0.0), 30);
    s.move_hand(1, s.hand_pos(1) + Vec3::new(0.0, 0.5, 0.0), 40);
    s.step(60);
    let along = s.rot(PLANK) * Vec3::X;
    assert!(along.y > 0.25, "tilted up towards the higher hand: {along:?}");
    let right_end = s.on(PLANK, s.held_at(1, PLANK).0);
    let hand = v3(s.out.hands[1].pose.pos);
    assert!(right_end.distance(hand) < 0.03, "its far end is in the right hand: {right_end:?} vs {hand:?}");
    assert!(hand.distance(s.hand_pos(1)) < 0.08, "the right hand is where its controller is: {hand:?} vs {:?}", s.hand_pos(1));
}

#[test]
fn a_handle_takes_the_hand_anywhere_along_it_and_the_blade_where_it_is() {
    let mut s = Sim::with(&[sword()]);
    // (Leaning in: lying on the table it's past the player's reach from the shoulders, which would
    // let go of it, resting on something, at once.)
    s.f.hmd.pos = [0.0, 1.7, 0.25];
    // Near the pommel end of the handle, 3 cm off it: on the handle there, held as the sword is.
    s.reach(1, s.on(SWORD, Vec3::new(0.0, 0.03, -0.095)));
    s.grip(1, 1.0);
    assert_eq!(s.rig.held(1), Some(SWORD));
    s.move_hand(1, Vec3::new(0.2, 1.3, 0.3), 30);
    s.step(30);
    let (at, rot) = s.held_at(1, SWORD);
    assert!(at.distance(Vec3::new(0.0, 0.0, -0.095)) < 0.005, "on the handle where it took it: {at:?}");
    let held = q4(sword().grip_rot);
    assert!((rot * Vec3::Z).dot(held * Vec3::Z) > 0.99, "the fist along the handle, the blade out past the thumb: {rot:?}");
    s.grip(1, 0.0);
    s.step(90);
    // Come at it the other way round (the fist's axis the other way along it): a reversed grip.
    let lying = s.rot(SWORD) * Vec3::Z;
    s.set_rot(1, Quat::from_rotation_arc(Vec3::Z, -lying));
    s.reach(1, s.on(SWORD, Vec3::new(0.0, 0.03, -0.05)));
    s.grip(1, 1.0);
    s.step(30);
    let (_, rot) = s.held_at(1, SWORD);
    assert!((rot * Vec3::Z).dot(held * Vec3::Z) < -0.99, "reversed: {rot:?}");
    s.grip(1, 0.0);
    s.step(90);
    // By the blade, well off the handle: held there, as it lies.
    s.set_rot(1, Quat::IDENTITY);
    s.move_hand(1, Vec3::new(0.3, 1.3, 0.2), 30);
    s.step(60);
    let before = s.rot(SWORD);
    s.reach(1, s.on(SWORD, Vec3::new(0.0, 0.0, 0.35)) + Vec3::Y * 0.04);
    s.grip(1, 1.0);
    s.step(30);
    let (at, _) = s.held_at(1, SWORD);
    assert!((at.z - 0.35).abs() < 0.03, "by the blade where it took it: {at:?}");
    assert!(s.rot(SWORD).angle_between(before) < 0.1, "as it lay");
}

#[test]
fn the_fingers_close_on_what_the_hand_holds_and_open_as_it_lets_go() {
    let mut s = Sim::new();
    assert!(s.rig.grasps()[1].is_none());
    // The rifle by its grip: the fingers close round the grip, the index stays on the trigger.
    s.take_rifle();
    let g = s.rig.grasps()[1].clone().expect("a grasp on the grip");
    assert!((g.weight - 1.0).abs() < 1e-4, "closed on it");
    assert_eq!(g.fingers & (1 << 1), 0, "the index is the trigger's");
    let grip = s.out.hands[1].pose;
    assert!(g.distance(v3(grip.pos)) < 0.0, "the grip pose is inside what the fingers close on");
    // The left hand takes the bottle anywhere: all its fingers close on it, over a moment.
    s.reach(0, s.pos(BOTTLE) + Vec3::new(0.0, 0.12, 0.0));
    s.grip(0, 1.0);
    let g = s.rig.grasps()[0].clone().expect("a grasp on the bottle");
    assert!(g.weight < 0.5, "only starting to close: {}", g.weight);
    assert_eq!(g.fingers, rustvr_avatar::grasp::ALL_FINGERS);
    s.step(20);
    assert!((s.rig.grasps()[0].as_ref().unwrap().weight - 1.0).abs() < 1e-4);
    // Let go, it opens off it: closing on it no more (the fingers only stop on it, the hand still
    // over it).
    s.grip(0, 0.0);
    s.step(3);
    let w = s.rig.grasps()[0].as_ref().map(|g| g.weight).unwrap_or(0.0);
    assert!(w > 0.3 && w < 1.0, "opening: {w}");
    s.step(20);
    assert!(s.rig.grasps()[0].as_ref().is_none_or(|g| g.weight == 0.0));
}

#[test]
fn a_hand_laid_slowly_on_a_thing_rests_on_it_and_a_swipe_knocks_it() {
    let mut s = Sim::new();
    let at = s.pos(BOTTLE);
    // Down onto the bottle slowly, the controller on into it: the hand stops on its top (10 cm up),
    // and the bottle stays put.
    s.reach(0, at + Vec3::new(0.0, 0.05, 0.0));
    s.step(20);
    let hand = v3(s.out.hands[0].pose.pos);
    assert!(hand.y > at.y + 0.1, "the hand rests on its top: {hand:?}");
    assert!(s.out.hands[0].flags & HAND_BLOCKED != 0, "held back, a ghost at the controller");
    assert!(s.pos(BOTTLE).distance(at) < 0.01, "not shoved: {:?}", s.pos(BOTTLE));
    // Swept through it fast, the hand knocks it.
    s.move_hand(0, at + Vec3::new(-0.3, 0.0, 0.0), 40);
    s.step(30);
    let before = s.pos(BOTTLE);
    let mut p = s.hand_pos(0);
    for _ in 0..25 {
        p.x += 2.0 * DT;
        s.set_hand(0, p);
        s.step(1);
    }
    s.step(30);
    assert!(s.pos(BOTTLE).distance(before) > 0.05, "knocked: {:?} from {before:?}", s.pos(BOTTLE));
}

#[test]
fn a_thing_made_of_convex_parts_is_held_on_the_part_the_hand_took() {
    // An L (a pistol side on): a bar 30 cm long, a grip hanging 10 cm under its back end. Its box
    // would fill the corner under the bar's front; its parts don't.
    const ELL: u32 = 43;
    // Lying flat on the table (its thickness up), the bar along the table's depth.
    let at = Vec3::new(-0.45, 0.82, 0.35);
    let mut d = obj(ELL, OBJ_PROP, at, [0.015, 0.065, 0.15], 0.8);
    d.spawn_pose = pose(at, Quat::from_rotation_z(std::f32::consts::FRAC_PI_2));
    d.center = [0.0, -0.035, 0.15];
    d.shape = SHAPE_HULL;
    let block = |lo: Vec3, hi: Vec3| (0..8).map(|k| [[lo.x, hi.x][k & 1], [lo.y, hi.y][(k >> 1) & 1], [lo.z, hi.z][k >> 2], 0.0]).collect::<Vec<[f32; 4]>>();
    let mut hulls: Box<GameHulls> = bytemuck::zeroed_box();
    for (k, (lo, hi)) in [(Vec3::new(-0.015, 0.0, 0.0), Vec3::new(0.015, 0.03, 0.3)), (Vec3::new(-0.015, -0.1, 0.0), Vec3::new(0.015, 0.0, 0.04))].into_iter().enumerate() {
        let c = block(lo, hi);
        hulls.hulls[k].id = ELL;
        hulls.hulls[k].count = c.len() as u32;
        hulls.hulls[k].points[..c.len()].copy_from_slice(&c);
    }
    hulls.count = 2;
    let mut s = Sim::new();
    s.rig.set_game_hulls(&hulls);
    s.table.descs[s.table.count as usize] = d;
    s.table.count += 1;
    s.table.seq += 1;
    s.rig.sync(&s.table);
    s.step(60);
    // Taken from above the bar's front end (wherever it lies): held on the bar, not on its box.
    let front = s.on(ELL, Vec3::new(0.0, 0.015, 0.25));
    s.reach(1, front + Vec3::Y * 0.04);
    s.grip(1, 1.0);
    s.step(30);
    assert_eq!(s.rig.held(1), Some(ELL));
    let (held, _) = s.held_at(1, ELL);
    // (Held where the hand is drawn: its fingers on the bar, the hand itself above it.)
    let off_bar = (held - held.clamp(Vec3::new(-0.015, 0.0, 0.0), Vec3::new(0.015, 0.03, 0.3))).length();
    assert!(off_bar < 0.12 && held.z > 0.2, "the hand on the bar's front end: {held:?}");
    // The fingers close on its parts.
    let g = s.rig.grasps()[1].clone().unwrap();
    assert_eq!(g.solids.len(), 2, "both parts: {:?}", g.solids);
}

#[test]
fn a_creatures_body_stops_the_hand_and_the_hand_pushes_it() {
    let mut s = Sim::new();
    // A creature's chest standing over the table's far side: 15 cm thick, its front at z 0.75.
    s.rig.set_actor_parts([((9, 1), (Vec3::new(0.2, 1.0, 0.9), Vec3::new(0.2, 1.4, 0.9), 0.15))]);
    s.step(2);
    s.move_hand(1, Vec3::new(0.2, 1.2, 0.5), 30);
    // Pressed on into it, the controller 10 cm inside.
    s.move_hand(1, Vec3::new(0.2, 1.2, 0.85), 30);
    let hand = v3(s.out.hands[1].pose.pos);
    assert!(hand.z < 0.75, "stopped at its front: {hand:?}");
    assert!(s.out.hands[1].flags & HAND_BLOCKED != 0);
    let p = s.rig.game_pushes.iter().find(|p| p.id == 9 && p.part == 2).copied().expect("the game asked to push part 1");
    assert!(p.normal[2] > 0.5 && p.normal[3] > 0.1, "into it, the way the hand presses: {p:?}");
}

#[test]
fn a_blade_stuck_in_a_creature_drags_it_and_comes_out_backwards() {
    let mut s = Sim::new();
    // A bottle in the hand, held out; its front (3 cm ahead of its middle) stabbed 5 cm into a
    // chest just in front of it (as the combat would have a blade).
    s.reach(1, s.pos(BOTTLE) + Vec3::new(0.0, 0.12, 0.0));
    s.grip(1, 1.0);
    s.move_hand(1, Vec3::new(0.2, 1.3, 0.45), 40);
    s.step(20);
    let (p, q) = s.rig.object_world_pose(BOTTLE).unwrap();
    let front = p + q * Vec3::new(0.0, 0.0, 0.03);
    let axis = Vec3::new(front.x, 0.0, front.z + 0.15);
    s.rig.set_actor_parts([((9, 1), (axis + Vec3::Y * (front.y - 0.2), axis + Vec3::Y * (front.y + 0.2), 0.15))]);
    let r = embed::Request { object: BOTTLE, into: embed::Into::Actor { id: 9, part: 1 }, point: front, dir: Vec3::Z, sample: Vec3::new(0.0, 0.0, 0.03), depth: 0.05, max_depth: 0.12, dampers: embed::DAMPERS, slide: None, wounding: Default::default(), rock: (embed::ROCK.0, embed::ROCK.1.to_radians()) };
    s.rig.embed(&r);
    // The hand goes on in with the stab.
    s.set_hand(1, s.hand_pos(1) + Vec3::Z * 0.05);
    s.step(5);
    let stuck = s.pos(BOTTLE);
    // The hand pulled sideways 15 cm: the bottle stays in, and the chest is dragged that way.
    s.move_hand(1, s.hand_pos(1) + Vec3::new(0.15, 0.0, 0.0), 15);
    assert!(s.pos(BOTTLE).distance(stuck) < 0.01, "still in, where it went in: {:?}", s.pos(BOTTLE));
    // (The part's pushes: the drag where it went in, and the turn the blade's twist gives it.)
    let drags: Vec<GamePush> = s.rig.game_pushes.iter().filter(|p| p.id == 9 && p.part == 2).copied().collect();
    assert!(drags.iter().any(|p| p.normal[0] > 0.8 && p.normal[3] > 0.5), "dragged the way the hand pulls: {drags:?}");
    // Pulled back out the way it went in: out.
    s.move_hand(1, s.hand_pos(1) - Vec3::new(0.15, 0.0, 0.0), 10);
    s.move_hand(1, s.hand_pos(1) - Vec3::new(0.0, 0.0, 0.2), 20);
    s.step(5);
    assert!(s.rig.objects[&BOTTLE].embed.is_none(), "pulled out");
    assert_eq!(s.rig.held(1), Some(BOTTLE));
}

#[test]
fn nothing_held_goes_through_the_world_however_hard_its_pushed() {
    let mut s = Sim::new();
    s.reach(0, s.pos(BOTTLE) + Vec3::new(0.0, 0.12, 0.0));
    s.grip(0, 1.0);
    assert_eq!(s.rig.held(0), Some(BOTTLE));
    s.move_hand(0, s.hand_pos(0) + Vec3::Y * 0.2, 20);
    // Shoved down 40 cm into the table top (at 0.8 m) and held there.
    s.move_hand(0, s.hand_pos(0) - Vec3::Y * 0.6, 30);
    s.step(60);
    // Its lowest corner, however it's turned.
    let (p, q) = s.rig.object_world_pose(BOTTLE).unwrap();
    let lowest = (0..8).map(|k| (p + q * Vec3::new([-0.03, 0.03][k & 1], [-0.1, 0.1][(k >> 1) & 1], [-0.03, 0.03][k >> 2])).y).fold(f32::MAX, f32::min);
    assert!(lowest > 0.785, "it stays on the table top: its lowest corner at {lowest:.3}");
}

#[test]
fn an_open_hand_is_stopped_by_its_fingers() {
    let mut s = Sim::new();
    // The hand open (nothing squeezed), fingers down (the controller turned so), lowered onto the
    // table's empty end and on 10 cm into it.
    s.set_rot(1, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2));
    let over = Vec3::new(0.45, 1.1, 0.75);
    s.move_hand(1, over, 30);
    s.move_hand(1, Vec3::new(0.45, 0.7, 0.75), 60);
    s.step(10);
    let h = &s.rig.hands.as_ref().unwrap()[1];
    let tips = s.rig.physics.pose(h.fingers[2]).0;
    assert!(tips.y > 0.79, "the fingertips stop on the top: {tips:?}");
    assert!(s.out.hands[1].flags & HAND_BLOCKED != 0);
}

#[test]
fn a_blade_stuck_in_a_loose_thing_stays_in_and_drags_it() {
    const CRATE: u32 = 44;
    let mut c = obj(CRATE, OBJ_PROP, Vec3::new(-0.35, 0.95, 0.65), [0.12, 0.12, 0.12], 3.0);
    c.flags = OBJF_GRABBABLE;
    let mut s = Sim::with(&[c]);
    s.step(30);
    // A bottle in the right hand, its bottom stuck 5 cm down into the crate's top (as the combat
    // would have a blade).
    s.reach(1, s.pos(BOTTLE) + Vec3::new(0.0, 0.12, 0.0));
    s.grip(1, 1.0);
    s.move_hand(1, s.hand_pos(1) + Vec3::Y * 0.3, 30);
    s.step(10);
    let bottom = |s: &Sim| s.pos(BOTTLE) + s.rot(BOTTLE) * Vec3::new(0.0, -0.1, 0.0);
    let top = s.pos(CRATE) + Vec3::Y * 0.12;
    s.move_hand(1, s.hand_pos(1) + (top - bottom(&s)), 40);
    s.step(20);
    let r = embed::Request { object: BOTTLE, into: embed::Into::Object { id: CRATE }, point: bottom(&s), dir: Vec3::NEG_Y, sample: Vec3::new(0.0, -0.1, 0.0), depth: 0.05, max_depth: 0.12, dampers: embed::DAMPERS, slide: None, wounding: Default::default(), rock: (embed::ROCK.0, embed::ROCK.1.to_radians()) };
    s.rig.embed(&r);
    s.set_hand(1, s.hand_pos(1) - Vec3::Y * 0.05);
    s.step(5);
    let (crate_at, stuck) = (s.pos(CRATE), s.pos(BOTTLE) - s.pos(CRATE));
    // Pulled towards the player 20 cm: the bottle stays in it, and the crate comes along.
    s.move_hand(1, s.hand_pos(1) - Vec3::Z * 0.2, 40);
    s.step(10);
    assert!(s.rig.objects[&BOTTLE].embed.is_some(), "still in");
    assert!(s.pos(CRATE).z < crate_at.z - 0.04, "the crate dragged along: {:?} from {crate_at:?}", s.pos(CRATE));
    let _ = stuck;
    let inside = s.rot(CRATE).inverse() * (bottom(&s) - s.pos(CRATE));
    assert!(inside.abs().max_element() < 0.12, "its bottom still in the crate: {inside:?}");
}

/// A hand pressed on into a wall (the controller 30 cm behind its face) and moved about there, as a
/// real hand on a real controller would: (the controller's path, the hand's path, how far the hand
/// went into the wall at worst).
fn press_into_wall() -> (Vec<Vec3>, Vec<Vec3>, f32) {
    let mut s = Sim::new();
    // A wall above the table, its face at z = 0.70.
    world_box(&mut s.rig, 3, [0.2, 1.3, 0.75], [0.4, 0.3, 0.05]);
    // The palm towards it.
    let palm_on = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
    s.set_rot(1, palm_on);
    s.move_hand(1, Vec3::new(0.2, 1.3, 0.45), 20);
    s.move_hand(1, Vec3::new(0.2, 1.3, 1.0), 40);
    let (mut ctrl, mut hand, mut deepest) = (Vec::new(), Vec::new(), f32::MIN);
    let c = s.rig.hands.as_ref().unwrap()[1].collider;
    for k in 0..150 {
        let t = k as f32 * DT;
        let p = Vec3::new(0.2 + 0.04 * (t * 3.0).sin(), 1.3 + 0.03 * (t * 4.1).cos(), 1.0 + 0.06 * (t * 2.3).sin());
        s.set_hand(1, p);
        s.set_rot(1, palm_on * Quat::from_rotation_x(0.2 * (t * 2.7).sin()) * Quat::from_rotation_z(0.15 * (t * 3.3).cos()));
        s.step(1);
        ctrl.push(p);
        hand.push(v3(s.out.hands[1].pose.pos));
        deepest = deepest.max(depth_in_world(&s, c));
    }
    (ctrl, hand, deepest)
}

/// How far collider `c` is into the world at its deepest (0 out of it).
fn depth_in_world(s: &Sim, c: rapier3d::prelude::ColliderHandle) -> f32 {
    let w = &s.rig.physics.world;
    let me = &w.colliders[c];
    let world = physics::groups_world().memberships;
    w.colliders
        .iter()
        .filter(|(h, o)| *h != c && o.collision_groups().memberships == world)
        .filter_map(|(_, o)| rapier3d::parry::query::contact(me.position(), me.shape(), o.position(), o.shape(), 0.0).ok().flatten())
        .map(|ct| -ct.dist)
        .fold(0.0, f32::max)
}

#[test]
fn a_hand_pushed_on_through_a_wall_rests_on_it_calmly() {
    let (ctrl, hand, deepest) = press_into_wall();
    // How far each moved along the wall frame to frame, and how much that changed (shaking).
    let across = |v: &[Vec3]| v.windows(2).map(|w| (w[1] - w[0]) * Vec3::new(1.0, 1.0, 0.0)).collect::<Vec<_>>();
    let (c, h) = (across(&ctrl), across(&hand));
    let path = |v: &[Vec3]| v.iter().map(|d| d.length()).sum::<f32>();
    let shake = |v: &[Vec3]| v.windows(2).map(|w| (w[1] - w[0]).length()).sum::<f32>();
    let depth_moves = hand.windows(2).map(|w| (w[1].z - w[0].z).abs()).fold(0.0f32, f32::max);
    eprintln!("deepest {deepest:.4}, path ctrl {:.3} hand {:.3}, shake ctrl {:.4} hand {:.4}, biggest move into/out of the wall {depth_moves:.4}", path(&c), path(&h), shake(&c), shake(&h));
    assert!(deepest < 0.004, "the hand stays out of the wall: {deepest}");
    assert!(path(&h) < path(&c) * 1.3, "it slides along the wall with the controller, no more: {} vs {}", path(&h), path(&c));
    assert!(shake(&h) < shake(&c) * 2.0 + 0.01, "calmly, without shaking: {} vs {}", shake(&h), shake(&c));
    assert!(depth_moves < 0.01, "it doesn't bounce off the wall: {depth_moves}");
}

/// How far collider `a` is into collider `b` (0 apart or touching).
fn depth_in(s: &Sim, a: rapier3d::prelude::ColliderHandle, b: rapier3d::prelude::ColliderHandle) -> f32 {
    let w = &s.rig.physics.world;
    let (a, b) = (&w.colliders[a], &w.colliders[b]);
    rapier3d::parry::query::contact(a.position(), a.shape(), b.position(), b.shape(), 0.0).ok().flatten().map_or(0.0, |ct| (-ct.dist).max(0.0))
}

#[test]
fn the_free_hand_rests_on_what_the_other_hand_holds() {
    let mut s = Sim::new();
    s.reach(0, s.pos(BOTTLE) + Vec3::new(0.0, 0.12, 0.0));
    s.grip(0, 1.0);
    s.move_hand(0, Vec3::new(-0.05, 1.3, 0.35), 30);
    s.step(20);
    let at = s.pos(BOTTLE);
    let (hand, bottle) = (s.rig.hands.as_ref().unwrap()[1].collider, s.rig.objects[&BOTTLE].collider);
    // The right hand, palm towards it, pushed on 10 cm past its middle: it stops on it, and the
    // bottle doesn't get knocked about in the left hand.
    s.move_hand(1, at + Vec3::new(0.2, 0.0, 0.0), 20);
    let mut deepest = 0.0f32;
    for k in 1..=40 {
        s.set_hand(1, (at + Vec3::new(0.2, 0.0, 0.0)).lerp(at - Vec3::new(0.1, 0.0, 0.0), k as f32 / 40.0));
        s.step(1);
        deepest = deepest.max(depth_in(&s, hand, bottle));
    }
    s.step(20);
    let on = v3(s.out.hands[1].pose.pos);
    assert!(deepest < 0.003, "the hand stays out of it: {deepest}");
    assert!(s.pos(BOTTLE).distance(at) < 0.01, "the bottle stays where the left hand has it: {:?} vs {at:?}", s.pos(BOTTLE));
    assert!(on.x > at.x + 0.02, "the hand rests on its side: {on:?} vs {at:?}");
    // It takes it there: both hands have it.
    s.grip(1, 1.0);
    s.step(5);
    assert!(s.out.hands[1].flags & HAND_HOLDING != 0 && s.out.hands[1].holding == BOTTLE, "both hands on it");
    // Let go again, the controller still in it: the hand comes out of it smoothly, no jump.
    let mut last = v3(s.out.hands[1].pose.pos);
    s.grip(1, 0.0);
    let mut worst = 0.0f32;
    let from = s.hand_pos(1);
    for k in 1..=40 {
        s.set_hand(1, from.lerp(at + Vec3::new(0.25, 0.0, 0.0), (k as f32 / 30.0).min(1.0)));
        s.step(1);
        let now = v3(s.out.hands[1].pose.pos);
        worst = worst.max(now.distance(last));
        last = now;
    }
    assert_eq!(s.rig.held(0), Some(BOTTLE));
    assert!(worst < 0.02, "no jump out of it: {worst}");
}

#[test]
fn a_hand_held_back_catches_up_eased_not_all_at_once() {
    let mut s = Sim::new();
    // Palm down on the table (top at 0.8, its edge at x = 0.6), the controller 15 cm down in it.
    s.set_rot(1, Quat::from_rotation_z(std::f32::consts::FRAC_PI_2));
    s.move_hand(1, Vec3::new(0.3, 1.0, 0.6), 20);
    s.move_hand(1, Vec3::new(0.3, 0.7, 0.6), 30);
    s.step(10);
    let c = s.rig.hands.as_ref().unwrap()[1].collider;
    // Slid off the edge: once clear of it, the hand drops to the controller.
    let (mut worst, mut deepest) = (0.0f32, 0.0f32);
    let mut last = (v3(s.out.hands[1].pose.pos), s.hand_pos(1));
    for k in 1..=90 {
        s.set_hand(1, Vec3::new(0.3, 0.7, 0.6).lerp(Vec3::new(0.9, 0.7, 0.6), (k as f32 / 30.0).min(1.0)));
        s.step(1);
        let now = (v3(s.out.hands[1].pose.pos), s.hand_pos(1));
        // How much further the hand went than its controller did this frame.
        worst = worst.max(now.0.distance(last.0) - now.1.distance(last.1));
        deepest = deepest.max(depth_in_world(&s, c));
        last = now;
    }
    let hand = v3(s.out.hands[1].pose.pos);
    // (Quick: settled in about a tenth of a second, so a few centimetres a frame, not the whole
    // way in one.)
    assert!(worst < 0.045, "no jump: {worst} m further than the controller in a frame at worst");
    assert!(deepest < 0.003, "never through the table's edge: {deepest}");
    assert!(hand.distance(s.hand_pos(1)) < 0.01, "caught up with it: {hand:?} vs {:?}", s.hand_pos(1));
}

#[test]
fn fingers_give_way_pressed_on_a_wall_and_curl_back_after() {
    let mut s = Sim::new();
    world_box(&mut s.rig, 3, [0.2, 1.3, 0.75], [0.4, 0.3, 0.05]);
    // A relaxed hand, palm towards the wall, pushed 10 cm on into it.
    s.set_rot(1, Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
    s.move_hand(1, Vec3::new(0.2, 1.3, 0.45), 20);
    let relaxed = s.out.finger_give[1];
    assert!(relaxed.iter().all(|&g| g == 0.0), "nothing pushes them in the air: {relaxed:?}");
    s.move_hand(1, Vec3::new(0.2, 1.3, 0.8), 30);
    s.step(10);
    let c = s.rig.hands.as_ref().unwrap()[1].collider;
    let pressed = s.out.finger_give[1];
    // (The index, off the trigger, is straight already.)
    assert!(pressed[2..].iter().all(|&g| g < -0.05), "the fingers are pushed open: {pressed:?}");
    assert!(depth_in_world(&s, c) < 0.003, "and nothing's in the wall: {}", depth_in_world(&s, c));
    let palm_gap = s.rig.physics.world.cast_ray(&rapier3d::prelude::Ray::new(rapier3d::prelude::Vector::new(0.2, 1.3, 0.5), rapier3d::prelude::Vector::new(0.0, 0.0, 1.0)), 1.0, true, rapier3d::prelude::QueryFilter::default().groups(physics::groups_world())).map(|(_, t)| t);
    let hand_z = v3(s.out.hands[1].pose.pos).z;
    assert!(palm_gap.is_some() && hand_z > 0.66, "the palm comes onto the wall: the hand at z {hand_z}");
    // Taken off it, they curl back as they were.
    s.move_hand(1, Vec3::new(0.2, 1.3, 0.45), 20);
    s.step(30);
    let after = s.out.finger_give[1];
    assert!(after.iter().all(|&g| g > -0.01), "back as they were: {after:?}");
    // A clenched fist hardly gives.
    s.f.hands[1].squeeze = 1.0;
    s.f.hands[1].trigger = 1.0;
    s.f.hands[1].buttons |= CTRL_TOUCH_TRIGGER | CTRL_A;
    s.move_hand(1, Vec3::new(0.2, 1.3, 0.8), 30);
    s.step(10);
    let fist = s.out.finger_give[1];
    assert!(fist[1..].iter().all(|&g| g > -0.35), "a fist stays a fist: {fist:?}");
}
