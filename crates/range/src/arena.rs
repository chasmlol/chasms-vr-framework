//! The range's people (`crate::ragdoll`) and who they are: the training dummies at the start, the
//! bench's (two one behind the other to skewer, three in leather, chainmail and plate), and the
//! arena's fighters (`crate::brain`), who come in waves: first an Easy one-handed swordsman and an
//! Easy one with a shield; then Medium fighters of each stance (one-handed, sword and shield, a sword
//! in each hand); then Hard ones, again and again. A wave's fighters stand waiting until the player
//! comes near; when they're all dead (or knocked out), the next comes five seconds later.
//!
//! The fighters' weapons and shields are the game's own objects (`OBJF_GAME_SIMULATED`): the range
//! moves them (in a fighter's hand, or loose), the player's hands and weapons meet them, push them
//! and take them (the runtime then holds them, and the fighter's hand is dragged after them until it
//! lets go). A pool of them is registered once; each wave's fighters take theirs from it.

use glam::{Quat, Vec3};
use rustvr_protocol::*;

use crate::anim;
use crate::arenas;
use crate::brain::{self, Brain, Mode, Sight, Stance, Threat, Tier};
use crate::ragdoll::{Balance, Dummies, Happened, ItemSpec};

/// The training dummies' ids (as the game knows them) and where they stand.
pub const DUMMY_ID: u32 = 7001;
pub const DUMMY_AT: Vec3 = Vec3::new(0.15, 0.0, 1.15);
pub const DUMMY_LEFT_AT: Vec3 = Vec3::new(-0.85, 0.0, 1.88);
/// The bench's dummies and the arena's fighters.
pub const BENCH_ID: u32 = 7101;
pub const FIGHTER_ID: u32 = 7201;
/// The fighters' weapons' and shields' object ids.
pub const NPC_ITEM_BASE: u32 = 0x9000;

/// Fighters keep this far apart (m).
const SPACING: f32 = 0.9;

/// What the pool holds: (the arsenal's key, how many).
const POOL: [(&str, usize); 4] = [("builtin_sword", 6), ("kit_shield", 2), ("kit_mace", 1), ("kit_axe", 1)];

/// One fighter: its body (a person's index), its brain, and its stance's kit.
pub struct Fighter {
    pub person: usize,
    pub brain: Brain,
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
    /// What the people cried out this frame (`VOICE_*`, where), for the runtime's voices; and when
    /// each last spoke (a voice says one line at a time).
    pub said: Vec<(u32, Vec3)>,
    spoke: std::collections::HashMap<u32, f64>,
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
        let body = [anim::TORSO, anim::PELVIS, 3, 4, 6, 7, 9, 10, 11, 12];
        for (n, m) in [MAT_LEATHER, MAT_CHAINMAIL, MAT_PLATE].into_iter().enumerate() {
            let mut armour: Vec<(usize, u32)> = body.iter().map(|&p| (p, m)).collect();
            if m == MAT_PLATE {
                armour.push((anim::HEAD, MAT_PLATE));
            }
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
        let mut people = People { dummies, fighters: Vec::new(), pool, made, wave: 0, next_wave: Some(0.0), next_id: FIGHTER_ID, said: Vec::new(), spoke: Default::default() };
        people.next_wave(0.0);
        people
    }

    /// The fighters' weapons as the game's objects, for the armory to register: (id, the arsenal
    /// key), each lying in the arena until it's handed out.
    pub fn pool_objects(&self) -> Vec<(u32, &'static str)> {
        self.pool.iter().map(|(id, key, _)| (*id, *key)).collect()
    }

    fn next_wave(&mut self, now: f64) {
        self.next_wave = None;
        self.wave += 1;
        let (tier, stances): (Tier, Vec<Stance>) = match self.wave {
            1 => (Tier::Easy, vec![Stance::OneHanded, Stance::SwordShield]),
            2 => (Tier::Medium, vec![Stance::OneHanded, Stance::SwordShield, Stance::DualWield]),
            _ => (Tier::Hard, vec![Stance::DualWield, Stance::SwordShield, Stance::OneHanded]),
        };
        tracing::info!("range: the arena's wave {} ({tier:?}): {stances:?}", self.wave);
        let (lo, hi) = arenas::ARENA;
        let n = stances.len();
        for (i, stance) in stances.into_iter().enumerate() {
            let x = lo.x + 1.0 + (hi.x - lo.x - 2.0) * (i as f32 + 0.5) / n as f32;
            let at = Vec3::new(x, 0.0, hi.z - 1.5);
            let face = Quat::from_rotation_y(std::f32::consts::PI);
            let id = self.next_id;
            self.next_id += 1;
            // (A person's slot is reused once its body's gone: the dummies keep a list.)
            let person = self.dummies.spawn(id, at, face, &[(anim::TORSO, MAT_LEATHER), (anim::PELVIS, MAT_LEATHER)]);
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
            self.fighters.push(Fighter { person, brain });
        }
        let _ = now;
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
        let sight = Sight { loose, ..sight.clone() };
        for f in &mut self.fighters {
            let p = &self.dummies.people[f.person];
            if !p.alive || !matches!(p.balance, Balance::Standing | Balance::Rising(_)) {
                f.brain.swing = None;
                continue;
            }
            let (at, face) = p.at;
            let hands = [0, 1].map(|h| p.hands[h].map(|k| {
                let item = &self.dummies.items[k];
                brain::Hold { reach: item.blade.1.length(), mass: item.mass, shield: item.shield }
            }));
            let staggered = p.stagger.is_some_and(|s| now - s.since < 0.6 / f.brain.tier.difficulty().recovery as f64);
            let swinging = f.brain.swing.is_some();
            let t = f.brain.think(now, dt, at, face, hands, &sight, staggered);
            // A shout as it swings, seven times in ten.
            if !swinging && f.brain.swing.is_some() && (now * 7919.0).fract() < 0.7 {
                let head = self.dummies.part_pose(f.person, anim::HEAD).0;
                self.said.push((VOICE_ATTACK, head));
            }
            let p = &mut self.dummies.people[f.person];
            p.pose = t.pose;
            p.doing = f.brain.doing(now);
            // Its arms: stiff while it swings, stiffer while it guards.
            p.arms = [0, 1].map(|h| if t.attacking == Some(h) { (2.0, 0.0, 10.0) } else if t.guarding { (3.0, 1.0, 30.0) } else { (1.0, 1.0, 1.0) });
            // It walks and turns.
            let walk = face * t.walk * dt;
            let mut at = at + walk;
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
            // A fetched weapon within reach of its hand: taken up.
            if let Mode::Fetching(k) = f.brain.mode {
                let (hand_at, _) = self.dummies.part_pose(f.person, anim::HAND[1]);
                if brain::within_reach(hand_at, self.dummies.item_pose(k).0) && self.dummies.people[f.person].hands[1].is_none() {
                    self.dummies.pick_up(f.person, 1, k);
                    tracing::info!("range: fighter {} takes up a weapon", self.dummies.people[f.person].id);
                }
            }
        }
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
        // The wave's over when none of it can fight: the next one comes.
        let done = self.fighters.iter().all(|f| {
            let p = &self.dummies.people[f.person];
            !p.alive || matches!(p.balance, Balance::Limp(_))
        });
        match self.next_wave {
            None if done && !self.fighters.is_empty() => self.next_wave = Some(now + 5.0),
            Some(t) if now >= t => {
                // The fallen go (their bodies stay a while: they're cleared with the next wave).
                self.clear_fallen();
                self.next_wave(now);
            }
            _ => {}
        }
    }

    /// The last wave's bodies go: their items fall loose first.
    fn clear_fallen(&mut self) {
        let fallen: Vec<usize> = self.fighters.iter().map(|f| f.person).collect();
        for k in fallen {
            for h in 0..2 {
                self.dummies.drop_item(k, h);
            }
            self.dummies.remove(k);
            for f in &mut self.fighters {
                if f.person > k {
                    f.person -= 1;
                }
            }
        }
        self.fighters.clear();
        // Item holders' person indices shift too (done in `remove`).
    }

    /// The fighter whose body is person `k`.
    pub fn brain_of(&mut self, k: usize) -> Option<&mut Brain> {
        self.fighters.iter_mut().find(|f| f.person == k).map(|f| &mut f.brain)
    }

    /// What the runtime should know about the fighters' weapons: which items swing now and which
    /// guard.
    pub fn items_for_runtime(&self) -> (ActorItems, Vec<(u32, Vec3, Quat)>) {
        let attacking = |k: usize| self.fighters.iter().any(|f| f.person == k && f.brain.swing.is_some_and(|s| s.hurting(self.dummies.time)));
        let blocking = |k: usize| self.fighters.iter().any(|f| f.person == k && f.brain.block > 0.3);
        self.dummies.held_items(attacking, blocking)
    }

    /// What happened, for the game's side (sounds, blood); drained. The people cry out at it: a
    /// grunt at every hurt, a last breath dying, one line at a time.
    pub fn happened(&mut self) -> Vec<Happened> {
        let out = std::mem::take(&mut self.dummies.happened);
        let now = self.dummies.time;
        for h in &out {
            let (id, voice) = match *h {
                Happened::Hurt { id, .. } => (id, VOICE_HURT),
                Happened::Died { id, .. } => (id, VOICE_DEATH),
                _ => continue,
            };
            if self.spoke.get(&id).is_some_and(|&t| now - t < 0.4) && voice != VOICE_DEATH {
                continue;
            }
            self.spoke.insert(id, now);
            if let Some(k) = self.dummies.find(id) {
                self.said.push((voice, self.dummies.part_pose(k, anim::HEAD).0));
            }
        }
        out
    }
}

/// The player as the fighters see them: the head, where they stand, and their weapons (each blade's
/// ends and how fast its tip goes), from the runtime's objects.
pub fn sight(head: Vec3, feet: Vec3, vel: Vec3, weapons: &[(Vec3, Vec3, Vec3)]) -> Sight {
    Sight { head, feet, vel, threats: weapons.iter().map(|&(a, b, vel)| Threat { a, b, vel }).collect(), loose: Vec::new() }
}
