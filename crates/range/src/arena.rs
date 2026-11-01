//! The range's people (`crate::ragdoll`) and who they are: the training dummies at the start, the
//! bench's (two one behind the other to skewer, three in leather, chainmail and plate), and the
//! arena's fighters (`crate::brain`), who come in waves: first an Easy one-handed swordsman and an
//! Easy one with a shield; then Medium fighters of each stance (one-handed, a sword in each hand,
//! and sword and shield in plate: `Tier::MediumHeavy`); then Hard ones (the plated one a
//! `Tier::HardHeavy`), again and again. A wave's fighters stand waiting until the player comes
//! near; when they're all dead, out cold or gone, the next comes five seconds later. One with no weapon and none
//! to take up runs for the arena's far corners and is gone.
//!
//! The fighters' weapons and shields are the game's own objects (`OBJF_GAME_SIMULATED`): the range
//! moves them (in a fighter's hand, or loose), the player's hands and weapons meet them, push them
//! and take them (the runtime then holds them, and the fighter's hand is dragged after them until it
//! lets go). A pool of them is registered once; each wave's fighters take theirs from it.

use glam::{Quat, Vec3};
use rustvr_melee::body::{Doing, Part as BodyPart};
use rustvr_protocol::*;

use crate::anim;
use crate::arenas;
use crate::brain::{self, Brain, Me, Mode, Sight, Stance, Threat, Tier};
use crate::duel::{Drive, Duelist};
use crate::ragdoll::{self, Balance, Dummies, Happened, ItemSpec};

/// The training dummies' ids (as the game knows them) and where they stand.
pub const DUMMY_ID: u32 = 7001;
pub const DUMMY_AT: Vec3 = Vec3::new(0.15, 0.0, 1.15);
pub const DUMMY_LEFT_AT: Vec3 = Vec3::new(-0.85, 0.0, 1.88);
/// The bench's dummies and the arena's fighters.
pub const BENCH_ID: u32 = 7101;
pub const FIGHTER_ID: u32 = 7201;
/// The fighters' weapons' and shields' object ids.
pub const NPC_ITEM_BASE: u32 = 0x9000;
/// The duel ring's duelist (`crate::duel`), and how long it lies dead before another comes (s).
pub const DUELIST_ID: u32 = 7301;
const DUELIST_AGAIN: f64 = 4.0;
/// Testing the duel (`--duel-test`), the duelist stands this far before the player's start (m).
const DUEL_TEST_GAP: f32 = 1.45;
/// Off its ground, the duelist still fights someone this near it (m).
const NEAR_DUELIST: f32 = 5.0;

/// Fighters keep this far apart (m).
const SPACING: f32 = 0.9;
/// Each line takes this long, and nothing new starts until it's over (a voice says one line at a
/// time), and an attack begins with a shout seven times in ten.
const GRUNT: f64 = 0.4;
const SHOUT: f64 = 0.7;
const LAST_BREATH: f64 = 1.5;
const SCREAM: f64 = 1.5;
const STRUGGLE: f64 = 0.5;
const SHOUT_CHANCE: f64 = 0.7;
/// A body falls screaming once its root drops faster than this (m/s), hushed as it lands: its root
/// this near the floor (m) and nearly still (m/s). Held, it cries out every 5 to 10 s.
const FALL_SCREAM: f32 = 5.0;
const LANDED: (f32, f32) = (0.5, 1.0);
const GRAPPLED_EVERY: (f64, f64) = (5.0, 10.0);

/// What the pool holds: (the arsenal's key, how many).
const POOL: [(&str, usize); 4] = [("builtin_sword", 6), ("kit_shield", 2), ("kit_mace", 1), ("kit_axe", 1)];

/// The armoured fighters' full plate: the body, arms and legs, and the head.
const PLATE: [usize; 11] = [anim::TORSO, anim::PELVIS, 3, 4, 6, 7, 9, 10, 11, 12, anim::HEAD];

/// One fighter: its body (a person's index), its brain, and which hands guard (for the runtime).
pub struct Fighter {
    pub person: usize,
    pub brain: Brain,
    guarding: [bool; 2],
}

pub struct People {
    pub dummies: Dummies,
    pub fighters: Vec<Fighter>,
    /// The pool's items: (object id, the arsenal key, its spec, a person's index holding it).
    pub pool: Vec<(u32, &'static str, ItemSpec)>,
    /// Each pool item's index among the dummies' items, once it's been made.
    made: Vec<Option<usize>>,
    wave: u32,
    next_wave: Option<f64>,
    next_id: u32,
    /// What the people cried out this frame (who, `VOICE_*`, where), for the runtime's voices; until
    /// when each one's line lasts; who's screaming as they fall; and when the held cry out next.
    pub said: Vec<(u32, u32, Vec3)>,
    speaking: std::collections::HashMap<u32, f64>,
    screaming: std::collections::HashSet<u32>,
    grappled: std::collections::HashMap<u32, f64>,
    /// `RUSTVR_DEBUG_BRAIN`: each fighter's state logged once a second, next then.
    debug: Option<f64>,
    /// The duel ring's duelist, how it's driven, the next one's id, and since when it's lain dead;
    /// a hand of the player's raised well over the head this frame (the tests' cue for its swing).
    pub duelist: Option<Duelist>,
    duel_drive: Drive,
    next_duelist: u32,
    duelist_dead: Option<f64>,
    pub duel_cue: bool,
}

/// The arsenal's weapon of `key` as a held thing: its mass, its boxes, its grip and blade.
fn spec_of(key: &str, id: u32) -> Option<ItemSpec> {
    let arm = crate::arsenal::all().into_iter().find(|a| a.thing.key == key)?;
    let d = arm.thing.desc;
    let main = arm.melee.handles().iter().find(|h| h.flags & HANDLE_MAIN != 0).copied()?;
    let grip = (Vec3::from_array(main.a) + Vec3::from_array(main.b)) * 0.5;
    let blade = arm.melee.blades().first().map_or((Vec3::ZERO, Vec3::Z), |b| (Vec3::from_array(b.a), Vec3::from_array(b.b)));
    let boxes = arm.thing.parts.iter().filter_map(|m| m.bounds(glam::Mat4::IDENTITY)).map(|(lo, hi)| ((lo + hi) * 0.5, (hi - lo) * 0.5)).collect();
    Some(ItemSpec { id, mass: d.mass, boxes, grip, shield: arm.melee.flags & MW_SHIELD != 0, blade })
}

impl People {
    pub fn new(world: &[(Vec3, Vec3)]) -> People {
        let mut dummies = Dummies::new(world);
        let facing = |turn: f32| Quat::from_rotation_y(std::f32::consts::PI + turn);
        dummies.spawn(DUMMY_ID, DUMMY_AT, facing(0.0), &[]);
        dummies.spawn(DUMMY_ID + 1, DUMMY_LEFT_AT, facing(0.35), &[]);
        // The bench: two one behind the other (to run through both), and three in armour.
        let bench = arenas::BENCH;
        let x0 = bench.0.x + 1.2;
        let z = 20.0;
        let face_player = Quat::from_rotation_y(std::f32::consts::PI);
        dummies.spawn(BENCH_ID, Vec3::new(x0, 0.0, z), face_player, &[]);
        dummies.spawn(BENCH_ID + 1, Vec3::new(x0, 0.0, z + 0.42), face_player, &[]);
        for (n, m) in [MAT_LEATHER, MAT_CHAINMAIL, MAT_PLATE].into_iter().enumerate() {
            let armour: Vec<(usize, u32)> = PLATE.iter().filter(|&&p| p != anim::HEAD || m == MAT_PLATE).map(|&p| (p, m)).collect();
            dummies.spawn(BENCH_ID + 2 + n as u32, Vec3::new(x0 + 1.3 + n as f32 * 1.0, 0.0, z), face_player, &armour);
        }
        let mut pool = Vec::new();
        let mut n = 0;
        for (key, count) in POOL {
            for _ in 0..count {
                if let Some(spec) = spec_of(key, NPC_ITEM_BASE + n) {
                    pool.push((NPC_ITEM_BASE + n, key, spec));
                }
                n += 1;
            }
        }
        let made = vec![None; pool.len()];
        let debug = std::env::var_os("RUSTVR_DEBUG_BRAIN").is_some().then_some(0.0);
        let mut people = People {
            dummies,
            fighters: Vec::new(),
            pool,
            made,
            wave: 0,
            next_wave: Some(0.0),
            next_id: FIGHTER_ID,
            said: Vec::new(),
            speaking: Default::default(),
            screaming: Default::default(),
            grappled: Default::default(),
            debug,
            duelist: None,
            duel_drive: Drive::Free,
            next_duelist: DUELIST_ID,
            duelist_dead: None,
            duel_cue: false,
        };
        people.new_duelist();
        people
    }

    /// The duelist driven so from now on (the tests' cue or guard): it's put back where it starts,
    /// close before the player's start for a test.
    pub fn drive_duelist(&mut self, drive: Drive) {
        self.duel_drive = drive;
        if let Some(d) = self.duelist.take() {
            self.remove_people(vec![d.person]);
        }
        self.new_duelist();
    }

    /// A duelist in the ring with a sword, facing the player's start.
    fn new_duelist(&mut self) {
        let at = if self.duel_drive == Drive::Free { arenas::DUELIST_AT } else { arenas::DUEL_START + Vec3::Z * DUEL_TEST_GAP };
        let id = self.next_duelist;
        self.next_duelist += 1;
        let person = self.dummies.spawn(id, at, Quat::from_rotation_y(std::f32::consts::PI), &[(anim::TORSO, MAT_LEATHER), (anim::PELVIS, MAT_LEATHER)]);
        self.dummies.people[person].table = rustvr_melee::body::Table::Medium;
        self.hand_out(person, 1, "builtin_sword");
        self.duelist = Some(Duelist::new(person, id, self.duel_drive, self.dummies.time));
        self.duelist_dead = None;
        tracing::info!("range: the duelist {id:#x} stands in the ring ({:?})", self.duel_drive);
    }

    /// Whether a blow of the player's (`CE_HIT`) lands on the duelist: one a swing.
    pub fn duelist_takes(&mut self, ev: &CombatEvent) -> bool {
        let now = self.dummies.time;
        self.duelist.as_mut().filter(|d| d.id == ev.actor).is_none_or(|d| d.takes(ev.object, now))
    }

    /// Whether `id` is the duelist.
    pub fn is_duelist(&self, id: u32) -> bool {
        self.duelist.as_ref().is_some_and(|d| d.id == id)
    }

    /// The runtime's word on the duel (`CE_BLOCK` with the duelist's sword, `CE_HIT` on it,
    /// `CE_STRUCK` by it), for its brain.
    pub fn duel_event(&mut self, ev: &CombatEvent) {
        let now = self.dummies.time;
        let Some(d) = self.duelist.as_mut().filter(|d| d.id == ev.actor) else { return };
        match ev.kind {
            CE_BLOCK => d.clashed(now, ev.flags & CEF_PARRIED != 0, ev.flags & CEF_PARRY != 0),
            CE_HIT => d.hurt(now),
            CE_STRUCK => d.struck(),
            _ => {}
        }
    }

    /// One frame of the duelist: what it sees, what it does with its body; dead a while, another
    /// comes.
    fn think_duel(&mut self, dt: f32, sight: &Sight) {
        let now = self.dummies.time;
        let Some(d) = self.duelist.as_mut() else { return };
        let p = &self.dummies.people[d.person];
        if !p.alive {
            let person = d.person;
            self.dummies.lead_item(person, 1, None);
            let since = *self.duelist_dead.get_or_insert(now);
            if now - since > DUELIST_AGAIN {
                let person = d.person;
                self.duelist = None;
                self.remove_people(vec![person]);
                self.new_duelist();
            }
            return;
        }
        if !matches!(p.balance, Balance::Standing | Balance::Rising(_)) || p.stun.is_some() {
            d.sleep(now);
            let person = d.person;
            self.dummies.lead_item(person, 1, None);
            return;
        }
        // Disarmed, it takes a sword up again.
        if p.hands[1].is_none() {
            let person = d.person;
            self.hand_out(person, 1, "builtin_sword");
            return;
        }
        let (at, face) = p.at;
        // (It fights someone on its ground, or come within a few steps of it; else it stands in its
        // guard.)
        let near = arenas::in_duel(sight.feet) || (sight.feet - at).with_y(0.0).length() < NEAR_DUELIST || d.drive != Drive::Free;
        let seen = if near { sight.clone() } else { Sight { feet: at + face * Vec3::Z * 3.0, ..Default::default() } };
        let mut t = d.think(now, dt, at, face, &seen, self.duel_cue);
        if !near {
            // (No one on its ground: it holds its place.)
            t.walk = Vec3::ZERO;
        }
        // Its sword goes where its keys put it, its hand with it.
        self.dummies.lead_item(d.person, 1, Some((&t.pose, t.sword)));
        let p = &mut self.dummies.people[d.person];
        p.pose = t.pose;
        p.doing = if t.attacking { Doing::Attacking } else { Doing::Combat };
        // Its sword arm firm in its guard and its parries, firmer swinging (the fighters' pins).
        p.arms = [(1.0, 1.0, 1.0), if t.attacking { ragdoll::ATTACK_ARM } else { ragdoll::GUARD_ARM }];
        let (lo, hi) = arenas::DUEL;
        let mut at = at + face * t.walk * dt;
        at.x = at.x.clamp(lo.x + 0.3, hi.x - 0.3);
        at.z = at.z.clamp(lo.z + 0.3, hi.z - 0.3);
        let face = match t.face_to.filter(|_| near) {
            Some(dir) => face.slerp(Quat::from_rotation_y(dir.x.atan2(dir.z)), (dt * 4.0).min(1.0)).normalize(),
            None => face,
        };
        p.at = (at, face);
    }

    /// The fighters' weapons as the game's objects, for the armory to register: (id, the arsenal
    /// key), each lying in the arena until it's handed out.
    pub fn pool_objects(&self) -> Vec<(u32, &'static str)> {
        self.pool.iter().map(|(id, key, _)| (*id, *key)).collect()
    }

    fn next_wave(&mut self) {
        self.next_wave = None;
        self.wave += 1;
        // Each wave's fighters by stance and brain; from the second, its shield-bearer is an
        // armoured one (`Tier::MediumHeavy` and `Tier::HardHeavy`: in plate, a step tougher).
        let fighters: Vec<(Stance, Tier)> = match self.wave {
            1 => vec![(Stance::OneHanded, Tier::Easy), (Stance::SwordShield, Tier::Easy)],
            2 => vec![(Stance::OneHanded, Tier::Medium), (Stance::SwordShield, Tier::MediumHeavy), (Stance::DualWield, Tier::Medium)],
            _ => vec![(Stance::DualWield, Tier::Hard), (Stance::SwordShield, Tier::HardHeavy), (Stance::OneHanded, Tier::Hard)],
        };
        tracing::info!("range: the arena's wave {}: {fighters:?}", self.wave);
        let (lo, hi) = arenas::ARENA;
        let n = fighters.len();
        for (i, (stance, tier)) in fighters.into_iter().enumerate() {
            let x = lo.x + 1.0 + (hi.x - lo.x - 2.0) * (i as f32 + 0.5) / n as f32;
            let at = Vec3::new(x, 0.0, hi.z - 1.5);
            let face = Quat::from_rotation_y(std::f32::consts::PI);
            let id = self.next_id;
            self.next_id += 1;
            let heavy = tier.difficulty().heavy;
            let armour: Vec<(usize, u32)> = if heavy { PLATE.iter().map(|&p| (p, MAT_PLATE)).collect() } else { vec![(anim::TORSO, MAT_LEATHER), (anim::PELVIS, MAT_LEATHER)] };
            let person = self.dummies.spawn(id, at, face, &armour);
            self.dummies.people[person].table = tier.table();
            let kit: &[(usize, &str)] = match stance {
                Stance::OneHanded => &[(1, "builtin_sword")],
                Stance::SwordShield => &[(1, "builtin_sword"), (0, "kit_shield")],
                Stance::DualWield => &[(1, "builtin_sword"), (0, "builtin_sword")],
            };
            for &(hand, key) in kit {
                self.hand_out(person, hand, key);
            }
            let mut brain = Brain::new(tier, stance, id);
            brain.mode = Mode::Waiting;
            self.fighters.push(Fighter { person, brain, guarding: [false; 2] });
        }
    }

    /// A pool item of `key` (one no one holds, the player hasn't got, not lying where it fell) into
    /// person `person`'s hand.
    fn hand_out(&mut self, person: usize, hand: usize, key: &str) {
        let free = (0..self.pool.len()).find(|&n| {
            self.pool[n].1 == key
                && match self.made[n] {
                    None => true,
                    Some(k) => self.dummies.items[k].holder.is_none() && !self.dummies.items[k].players,
                }
        });
        let Some(n) = free else { return };
        let spec = self.pool[n].2.clone();
        let turn = if spec.shield { Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2) } else { Quat::IDENTITY };
        match self.made[n] {
            Some(k) => self.dummies.pick_up(person, hand, k),
            None => self.made[n] = Some(self.dummies.arm(person, hand, &spec, turn)),
        }
    }

    /// One frame of the fighters' brains: what each sees, what it does with its body.
    pub fn think(&mut self, dt: f32, sight: &Sight) {
        let now = self.dummies.time;
        // Loose items any fighter could take up.
        let loose: Vec<(usize, Vec3)> = self.dummies.items.iter().enumerate().filter(|(_, i)| i.holder.is_none() && !i.players && !i.shield).map(|(k, _)| (k, self.dummies.item_pose(k).0)).collect();
        // The ways out: the arena's far corners, where its fighters came in.
        let (lo, hi) = arenas::ARENA;
        let exits = vec![Vec3::new(lo.x + 0.3, 0.0, hi.z - 0.3), Vec3::new(hi.x - 0.3, 0.0, hi.z - 0.3)];
        // Which of them are at the player (awake, standing, and within their own safe distance of
        // them), and where each stands.
        let at_player: Vec<bool> = self
            .fighters
            .iter()
            .map(|f| {
                let p = &self.dummies.people[f.person];
                p.alive && p.balance == Balance::Standing && p.stun.is_none() && f.brain.mode == Mode::Fighting && (p.at.0 - sight.feet).with_y(0.0).length() <= f.brain.tier.difficulty().safe
            })
            .collect();
        let feet: Vec<Option<Vec3>> = self.fighters.iter().map(|f| {
            let p = &self.dummies.people[f.person];
            (p.alive && matches!(p.balance, Balance::Standing | Balance::Rising(_))).then_some(p.at.0)
        }).collect();
        let mut shouts = Vec::new();
        for (n, f) in self.fighters.iter_mut().enumerate() {
            let p = &self.dummies.people[f.person];
            if !p.alive || !matches!(p.balance, Balance::Standing | Balance::Rising(_)) {
                // Down, it stops whatever it was swinging.
                f.brain.sleep(now);
                f.guarding = [false; 2];
                continue;
            }
            if p.stun.is_some() {
                // Out cold on its feet: its brain off, it stands dazed.
                f.brain.sleep(now);
                f.guarding = [false; 2];
                let p = &mut self.dummies.people[f.person];
                p.pose = brain::dazed();
                p.doing = Doing::Idle;
                p.arms = [(1.0, 1.0, 1.0); 2];
                continue;
            }
            let (at, face) = p.at;
            let hands = [0, 1].map(|h| p.hands[h].map(|k| {
                let item = &self.dummies.items[k];
                let (ip, iq) = self.dummies.item_pose(k);
                brain::Hold { reach: item.blade.1.length(), mass: item.mass, shield: item.shield, players: item.players, blade: (ip + iq * item.blade.0, ip + iq * item.blade.1) }
            }));
            let staggered = p.stagger.is_some_and(|s| now - s.since < 0.6 / f.brain.tier.difficulty().recovery as f64) || self.dummies.is_shocked(f.person) || p.burning.is_some();
            let me = Me {
                at,
                face,
                hands,
                head: self.dummies.part_pose(f.person, anim::HEAD).0,
                staggered,
                rising: matches!(p.balance, Balance::Rising(_)),
                lost: 1.0 - (p.health / rustvr_melee::body::HEALTH).clamp(0.0, 1.0),
                held: (p.balance == Balance::Standing).then(|| self.dummies.hold_on(f.person, sight.feet)).flatten(),
            };
            let others: Vec<Vec3> = feet.iter().enumerate().filter(|&(m, _)| m != n).filter_map(|(_, f)| *f).collect();
            let engaged = at_player.iter().enumerate().filter(|&(m, &a)| m != n && a).count();
            let seen = Sight { loose: loose.clone(), allies: others, engaged, exits: exits.clone(), ..sight.clone() };
            f.brain.escaping = (now - p.struggled < 0.6).then_some(p.struggled);
            let t = f.brain.think(now, dt, &me, &seen);
            // A shout as it swings, seven times in ten.
            if t.shout && (now * 7919.0 + n as f64 * 0.37).fract() < SHOUT_CHANCE {
                shouts.push((p.id, me.head));
            }
            f.guarding = t.guarding;
            let p = &mut self.dummies.people[f.person];
            p.pose = t.pose;
            p.doing = f.brain.doing();
            // Its arms: stiff while it swings, stiffer while it guards.
            p.arms = [0, 1].map(|h| if t.attacking[h] { ragdoll::ATTACK_ARM } else if t.guarding[h] { ragdoll::GUARD_ARM } else { (1.0, 1.0, 1.0) });
            // It walks and turns.
            let mut at = at + face * t.walk * dt;
            let (lo, hi) = (crate::content::ROOM_MIN, crate::content::ROOM_MAX);
            at.x = at.x.clamp(lo.x, hi.x);
            at.z = at.z.clamp(lo.z, hi.z);
            let face = match t.face_to {
                Some(dir) => {
                    let want = Quat::from_rotation_y(dir.x.atan2(dir.z));
                    face.slerp(want, (dt * 6.0).min(1.0)).normalize()
                }
                None => face,
            };
            p.at = (at, face);
            // Stooped to its fetched weapon, it takes it up once its hand is there.
            if let Some(k) = brain::taking(f.brain.mode, now) {
                let (hand_at, _) = self.dummies.part_pose(f.person, anim::HAND[1]);
                if brain::within_reach(hand_at, self.dummies.item_pose(k).0) && self.dummies.people[f.person].hands[1].is_none() {
                    self.dummies.pick_up(f.person, 1, k);
                    tracing::info!("range: fighter {:#x} takes up a weapon", self.dummies.people[f.person].id);
                }
            }
        }
        for (id, at) in shouts {
            self.say(id, VOICE_ATTACK, at, SHOUT);
        }
        self.think_duel(dt, sight);
        // Gone: it ran out of the arena, and what it held is left.
        let gone: Vec<usize> = self.fighters.iter().filter(|f| f.brain.mode == Mode::Gone).map(|f| f.person).collect();
        self.remove_people(gone);
        // Fighters keep a body's width apart (they strafe and close in round the player, not
        // through each other).
        let n = self.fighters.len();
        for i in 0..n {
            for j in i + 1..n {
                let (a, b) = (self.fighters[i].person, self.fighters[j].person);
                let (pa, pb) = (self.dummies.people[a].at.0, self.dummies.people[b].at.0);
                let d = (pb - pa).with_y(0.0);
                let gap = d.length();
                if gap < SPACING && gap > 1e-3 {
                    let push = d / gap * (SPACING - gap) * 0.5 * (dt * 8.0).min(1.0);
                    self.dummies.people[a].at.0 -= push;
                    self.dummies.people[b].at.0 += push;
                }
            }
        }
        if let Some(t) = self.debug
            && now >= t
        {
            self.debug = Some(now + 1.0);
            for f in &self.fighters {
                let p = &self.dummies.people[f.person];
                tracing::info!("brain {:#x}: {:.2} m from the player, {:?}, health {:.0}{}, {}", p.id, (p.at.0 - sight.feet).with_y(0.0).length(), p.balance, p.health, if p.stun.is_some() { ", out cold" } else { "" }, f.brain.describe(now));
            }
        }
        // The wave's over when none of it can fight: dead, out cold, or gone. The next comes once the
        // player's in the arena (the room's other areas stay quiet).
        let done = self.fighters.iter().all(|f| {
            let p = &self.dummies.people[f.person];
            !p.alive || p.stun.is_some()
        });
        let (lo, hi) = arenas::ARENA;
        let in_arena = (lo.x..=hi.x).contains(&sight.feet.x) && (lo.z..=hi.z).contains(&sight.feet.z);
        match self.next_wave {
            None if done => self.next_wave = Some(now + 5.0),
            Some(t) if now >= t && in_arena => {
                // The fallen go (their bodies stay a while: they're cleared with the next wave).
                self.clear_fallen();
                self.next_wave();
            }
            _ => {}
        }
    }

    /// The last wave's bodies go: their items fall loose first.
    fn clear_fallen(&mut self) {
        let fallen: Vec<usize> = self.fighters.iter().map(|f| f.person).collect();
        self.remove_people(fallen);
        self.fighters.clear();
    }

    /// People `ks` go, and with them their fighters: their items fall loose, and the people after each
    /// move up one (from the last, so each index still means who it did).
    fn remove_people(&mut self, mut ks: Vec<usize>) {
        ks.sort_unstable();
        ks.dedup();
        for &k in ks.iter().rev() {
            for h in 0..2 {
                self.dummies.drop_item(k, h);
            }
            tracing::info!("range: {:#x} goes", self.dummies.people[k].id);
            self.dummies.remove(k);
            self.fighters.retain(|f| f.person != k);
            for f in &mut self.fighters {
                if f.person > k {
                    f.person -= 1;
                }
            }
            if self.duelist.as_ref().is_some_and(|d| d.person == k) {
                self.duelist = None;
            }
            if let Some(d) = self.duelist.as_mut().filter(|d| d.person > k) {
                d.person -= 1;
            }
        }
    }

    /// The fighter whose body is person `k`.
    pub fn brain_of(&mut self, k: usize) -> Option<&mut Brain> {
        self.fighters.iter_mut().find(|f| f.person == k).map(|f| &mut f.brain)
    }

    /// What the runtime should know about the fighters' weapons: which hands' items swing now (their
    /// damage windows) and which guard.
    pub fn items_for_runtime(&self) -> (ActorItems, Vec<(u32, Vec3, Quat)>) {
        let now = self.dummies.time;
        let fighter = |k: usize| self.fighters.iter().find(|f| f.person == k);
        let duelist = |k: usize, h: usize| self.duelist.as_ref().filter(|d| d.person == k && h == 1);
        let attacking = |k: usize, h: usize| fighter(k).is_some_and(|f| f.brain.hurting(now)[h]) || duelist(k, h).is_some_and(|d| d.attacking());
        let blocking = |k: usize, h: usize| fighter(k).is_some_and(|f| f.guarding[h]);
        let feinting = |k: usize| fighter(k).is_some_and(|f| f.brain.feinting());
        let (mut items, poses) = self.dummies.held_items(attacking, blocking, feinting);
        // The duelist's sword in its parry's window.
        if let Some(d) = self.duelist.as_ref().filter(|d| d.parrying()) {
            let id = d.id;
            let n = (items.count as usize).min(MAX_ACTOR_ITEMS);
            for i in items.items[..n].iter_mut().filter(|i| i.actor == id && i.hand == 1) {
                i.flags |= AI_PARRYING;
            }
        }
        (items, poses)
    }

    /// Person `id` says `voice` at `at`, a line `long` seconds long, unless it's saying something
    /// already, its voice is cut, or a hand is over its mouth.
    fn say(&mut self, id: u32, voice: u32, at: Vec3, long: f64) {
        let now = self.dummies.time;
        let Some(k) = self.dummies.find(id) else { return };
        let p = &self.dummies.people[k];
        if p.silenced || p.muffled || self.speaking.get(&id).is_some_and(|&t| now < t) {
            return;
        }
        self.speaking.insert(id, now + long);
        self.said.push((id, voice, at));
    }

    /// Person `id` stops what it's saying (its scream as it lands).
    fn hush(&mut self, id: u32, at: Vec3) {
        self.speaking.remove(&id);
        self.said.push((id, VOICE_HUSH, at));
    }

    /// The other lines: a scream while a body falls faster than 5 m/s, hushed as it lands; and,
    /// held by the hands, a struggling cry every 5 to 10 s (not from a wave's fighters).
    fn cries(&mut self) {
        let now = self.dummies.time;
        for k in 0..self.dummies.people.len() {
            let p = &self.dummies.people[k];
            let (id, awake, standing, floor) = (p.id, p.alive && p.stun.is_none(), p.balance == Balance::Standing, p.at.0.y);
            let held = awake && self.dummies.handled(k) && !self.fighters.iter().any(|f| f.person == k);
            let head = self.dummies.part_pose(k, anim::HEAD).0;
            let root = (self.dummies.part_pose(k, anim::PELVIS).0, self.dummies.part_velocity(k, anim::PELVIS));
            if awake && !standing && root.1.y.abs() > FALL_SCREAM && !self.screaming.contains(&id) {
                self.screaming.insert(id);
                self.speaking.remove(&id);
                tracing::info!("range: {id:#x} falls screaming ({:.1} m/s)", root.1.y);
                self.say(id, VOICE_FALLING, head, SCREAM);
            } else if self.screaming.contains(&id) && (!awake || (root.0.y - floor < LANDED.0 && root.1.length() < LANDED.1)) {
                self.screaming.remove(&id);
                if awake {
                    tracing::info!("range: {id:#x} lands");
                    self.hush(id, head);
                }
            }
            if !held {
                self.grappled.remove(&id);
                continue;
            }
            let wait = |n: f64| GRAPPLED_EVERY.0 + (GRAPPLED_EVERY.1 - GRAPPLED_EVERY.0) * (n * 7919.0 + id as f64 * 0.37).fract();
            let next = *self.grappled.entry(id).or_insert_with(|| now + wait(now));
            if now >= next {
                self.grappled.insert(id, now + wait(now));
                self.say(id, VOICE_GRAPPLED, head, STRUGGLE);
            }
        }
    }

    /// What happened, for the game's side (sounds, blood); drained. The people cry out at it: a
    /// grunt at every hurt but a choke's, and dying their last line by what
    /// killed them: none after a stab through the throat (its voice cut), only a grunt from a stab to
    /// the head, breathless for a cut throat or a choke, the long one for a stab anywhere else, else
    /// its death. And their screams as they fall and cries as they're held.
    pub fn happened(&mut self) -> Vec<Happened> {
        self.cries();
        let out = std::mem::take(&mut self.dummies.happened);
        for h in &out {
            match *h {
                Happened::Hurt { id, voice: true, .. } => {
                    if let Some(k) = self.dummies.find(id) {
                        let at = self.dummies.part_pose(k, anim::HEAD).0;
                        self.say(id, VOICE_HURT, at, GRUNT);
                    }
                }
                Happened::Died { id, .. } => {
                    let Some(k) = self.dummies.find(id) else { continue };
                    let at = self.dummies.part_pose(k, anim::HEAD).0;
                    let line = match self.dummies.people[k].last_hit {
                        (DMG_PIERCE, BodyPart::Head) => VOICE_HURT,
                        (DMG_SLASH | 0, BodyPart::Neck) => VOICE_BREATHLESS,
                        (DMG_PIERCE, _) => VOICE_DEATH_LONG,
                        _ => VOICE_DEATH,
                    };
                    self.screaming.remove(&id);
                    // A last breath cuts off whatever it was saying.
                    self.speaking.remove(&id);
                    self.say(id, line, at, LAST_BREATH);
                }
                _ => {}
            }
        }
        out
    }
}

/// The player as the fighters see them: where they stand and how they move, their weapons (each
/// blade's ends, how fast its tip goes, whether it has a point), and their longest weapon's reach,
/// from the runtime's objects.
pub fn sight(feet: Vec3, vel: Vec3, weapons: &[(Vec3, Vec3, Vec3, bool)], reach: f32) -> Sight {
    Sight { feet, vel, threats: weapons.iter().map(|&(a, b, vel, point)| Threat { a, b, vel, point }).collect(), reach, ..Default::default() }
}
