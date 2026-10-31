//! The range's magic: the spells it gives the player ([`Spells`]) and what it does with their casts,
//! as a game would, so the runtime's magic (the hands holding spells, the wheel, charging, aiming)
//! can be played and tested without a game: each spell's magicka spent and coming back, a bolt that
//! flies from the palm and strikes a creature, a touch that goes into the one the hand touches, a
//! flash on the player for a spell on themselves; plenty of magicka and eighteen spells to play with,
//! and one that costs more than the player has (the game's "not enough magicka"). What a spell does
//! to a creature is `crate::combat`'s: fire spells are its fire bolt, the rest its
//! shock; fire and shock fill a blade a charged hand is held to (`SPELL_FIRE`, `SPELL_SHOCK`).

use glam::Vec3;
use rustvr_protocol::*;

use crate::arena::People;
use crate::effects::Effects;
use crate::hits::World;
use crate::ragdoll::Struck;

/// The fire bolt and the shock are the ones `crate::combat` knows (their hits, their imbuing).
pub const FIRE_BOLT: u32 = crate::combat::FIRE_BOLT;
pub const SHOCK: u32 = crate::combat::SHOCK;
pub const SHOCK_TOUCH: u32 = 3;
pub const HEAL: u32 = 4;
pub const LANTERN: u32 = 5;
pub const INFERNO: u32 = 6;
/// It conjures the range's sword in the hand that casts it, as a bound weapon is made.
pub const BOUND_SWORD: u32 = 17;

/// The player's magicka, and how fast it comes back (a second): plenty, to play with.
const MAGICKA: f32 = 1000.0;
const REGEN: f32 = 60.0;
/// How fast a bolt flies (m/s), how far before it fizzles out (m).
const BOLT_SPEED: f32 = 14.0;
const BOLT_RANGE: f32 = 40.0;
/// The table goes to the runtime this often (seconds) while the magicka moves.
const WRITE_EVERY: f64 = 0.1;

struct Bolt {
    spell: u32,
    pos: Vec3,
    dir: Vec3,
    color: [f32; 3],
    travelled: f32,
}

pub struct RangeMagic {
    spells: Vec<Spell>,
    magicka: f32,
    selected: u32,
    bolts: Vec<Bolt>,
    written: f64,
}

fn spell(id: u32, name: &str, delivery: u32, cost: f32, level: u32, cast_time: f32, recover: f32, color: [f32; 3], flags: u32) -> Spell {
    let mut s: Spell = bytemuck::Zeroable::zeroed();
    s.id = id;
    s.delivery = delivery;
    s.cost = cost;
    s.level = level;
    s.cast_time = cast_time;
    s.recover = recover;
    s.color = color;
    s.flags = flags;
    s.set_name(name);
    s
}

impl Default for RangeMagic {
    fn default() -> Self {
        // (Oblivion's cast animations' times to their release and after it: self, touch, target.)
        let spells = vec![
            spell(FIRE_BOLT, "Fire bolt", DELIVER_TARGET, 12.0, 0, 0.4, 0.5, [1.0, 0.45, 0.1], SPELL_FAVOURITE | SPELL_HARMFUL | SPELL_FIRE),
            spell(HEAL, "Heal minor wounds", DELIVER_SELF, 15.0, 0, 0.37, 0.93, [0.5, 1.0, 0.55], SPELL_FAVOURITE),
            spell(SHOCK, "Shock", DELIVER_TARGET, 30.0, 1, 0.4, 0.5, [0.55, 0.65, 1.0], SPELL_HARMFUL | SPELL_SHOCK),
            spell(SHOCK_TOUCH, "Shocking touch", DELIVER_TOUCH, 10.0, 0, 0.27, 0.6, [0.6, 0.6, 1.0], SPELL_HARMFUL | SPELL_SHOCK),
            spell(LANTERN, "Lantern", DELIVER_SELF, 0.0, 0, 0.37, 0.93, [1.0, 0.9, 0.6], SPELL_POWER),
            spell(INFERNO, "Inferno", DELIVER_TARGET, 5000.0, 4, 0.4, 0.5, [1.0, 0.3, 0.05], SPELL_HARMFUL),
            // (More, for a second page of the wheel: each range, each level.)
            spell(7, "Fireball", DELIVER_TARGET, 60.0, 2, 0.4, 0.5, [1.0, 0.5, 0.15], SPELL_HARMFUL | SPELL_FIRE),
            spell(8, "Lightning bolt", DELIVER_TARGET, 55.0, 2, 0.4, 0.5, [0.7, 0.7, 1.0], SPELL_HARMFUL | SPELL_SHOCK),
            spell(9, "Ice storm", DELIVER_TARGET, 200.0, 4, 0.4, 0.5, [0.6, 0.85, 1.0], SPELL_HARMFUL),
            spell(10, "Spark", DELIVER_TARGET, 10.0, 0, 0.4, 0.5, [0.75, 0.75, 1.0], SPELL_HARMFUL | SPELL_SHOCK),
            spell(11, "Cold touch", DELIVER_TOUCH, 10.0, 0, 0.27, 0.6, [0.6, 0.85, 1.0], SPELL_HARMFUL),
            spell(12, "Searing grasp", DELIVER_TOUCH, 50.0, 2, 0.27, 0.6, [1.0, 0.45, 0.1], SPELL_HARMFUL | SPELL_FIRE),
            spell(13, "Paralyze", DELIVER_TOUCH, 150.0, 4, 0.27, 0.6, [0.9, 0.8, 0.5], SPELL_HARMFUL),
            spell(14, "Shield", DELIVER_SELF, 75.0, 3, 0.37, 0.93, [0.8, 0.8, 0.9], 0),
            spell(15, "Starlight", DELIVER_SELF, 10.0, 0, 0.37, 0.93, [1.0, 1.0, 0.85], 0),
            spell(16, "Summon skeleton", DELIVER_SELF, 45.0, 1, 0.37, 0.93, [0.7, 0.6, 1.0], 0),
            spell(BOUND_SWORD, "Bound sword", DELIVER_SELF, 175.0, 3, 0.37, 0.93, [0.6, 0.55, 1.0], 0),
            spell(18, "Convalescence", DELIVER_TARGET, 30.0, 1, 0.4, 0.5, [0.5, 1.0, 0.55], 0),
        ];
        RangeMagic { spells, magicka: MAGICKA, selected: FIRE_BOLT, bolts: Vec::new(), written: f64::NEG_INFINITY }
    }
}

/// Which of `crate::combat`'s spells a spell lands as on a creature: a fire spell its fire bolt,
/// the rest its shock.
fn lands_as(s: &Spell) -> u32 {
    if s.flags & SPELL_FIRE != 0 { FIRE_BOLT } else { SHOCK }
}

impl RangeMagic {
    /// The player's magicka now.
    pub fn magicka(&self) -> f32 {
        self.magicka
    }

    /// The player's spells now (what each costs against the magicka left, which is selected).
    pub fn table(&self) -> Spells {
        let mut t: Spells = bytemuck::Zeroable::zeroed();
        for (i, s) in self.spells.iter().take(MAX_SPELLS).enumerate() {
            let mut s = *s;
            s.flags &= !(SPELL_SELECTED | SPELL_CASTABLE);
            if s.id == self.selected {
                s.flags |= SPELL_SELECTED;
            }
            if s.cost <= self.magicka {
                s.flags |= SPELL_CASTABLE;
            }
            t.spells[i] = s;
        }
        t.count = self.spells.len().min(MAX_SPELLS) as u32;
        t.magicka = self.magicka;
        t.magicka_max = MAGICKA;
        t.cast_time = 0.4;
        t.recover = 0.5;
        t
    }

    /// The table, when it's time to send it again (`now` the range's clock).
    pub fn table_due(&mut self, now: f64) -> Option<Spells> {
        (now - self.written >= WRITE_EVERY).then(|| {
            self.written = now;
            self.table()
        })
    }

    /// The player put `id` in a hand from the wheel: the range's selected spell.
    pub fn select(&mut self, id: u32) {
        if self.spells.iter().any(|s| s.id == id) {
            self.selected = id;
            self.written = f64::NEG_INFINITY;
        }
    }

    /// The trigger pulled on a spell the player can't cast now: the range's "not enough magicka".
    pub fn refused(&self, id: u32) {
        if let Some(s) = self.spells.iter().find(|s| s.id == id) {
            tracing::info!("range: {} costs {} and the player has {:.0}: not enough magicka", s.name(), s.cost, self.magicka);
        }
    }

    /// A cast from a hand, as the game casts it: its magicka (or not enough: nothing), then a bolt
    /// from the palm, a jolt into what the hand touched, a flash on the player. Returns whether it
    /// was cast, and whether it went into a dummy at once (a touch).
    pub fn cast(&mut self, ev: &CombatEvent, effects: &mut Effects, people: &mut People) -> (bool, bool) {
        let Some(s) = self.spells.iter().find(|s| s.id == ev.what).copied() else { return (false, false) };
        if s.cost > self.magicka {
            tracing::info!("range: {} costs {} and the player has {:.0}: not enough magicka", s.name(), s.cost, self.magicka);
            return (false, false);
        }
        self.magicka -= s.cost;
        self.written = f64::NEG_INFINITY;
        let at = Vec3::from_array(ev.point);
        let dir = Vec3::from_array(ev.dir).normalize_or(Vec3::Z);
        tracing::info!("range: the player casts {} ({} magicka left)", s.name(), self.magicka.round());
        match s.delivery {
            DELIVER_TARGET => {
                self.bolts.push(Bolt { spell: s.id, pos: at, dir, color: s.color, travelled: 0.0 });
                (true, false)
            }
            DELIVER_TOUCH => {
                effects.burst(at, s.color);
                if ev.actor != 0 {
                    crate::combat::bolt(people, &CombatEvent { what: lands_as(&s), ..*ev });
                }
                (true, ev.actor != 0)
            }
            _ => {
                effects.burst(at, s.color);
                (true, false)
            }
        }
    }

    /// The magicka coming back, the bolts flying on: returns the dummies they struck this frame.
    pub fn update(&mut self, dt: f32, effects: &mut Effects, people: &mut People, world: &World) -> u32 {
        self.magicka = (self.magicka + REGEN * dt).min(MAGICKA);
        let spells = self.spells.clone();
        let mut struck = Vec::new();
        self.bolts.retain_mut(|b| {
            let step = BOLT_SPEED * dt;
            let from = b.pos;
            // (The world first: a wall stops it.)
            let wall = world.raycast(from, b.dir, step, &[]).map(|h| h.dist);
            let reach = wall.unwrap_or(step);
            // A creature on the way: it lands on it (`crate::combat`'s hit for it).
            if let Some((d, at, _, Struck::Part(k, part))) = people.dummies.raycast(from, b.dir, reach) {
                let id = people.dummies.people[k].id;
                effects.burst(at, b.color);
                let lands = spells.iter().find(|s| s.id == b.spell).map_or(SHOCK, lands_as);
                crate::combat::bolt(people, &CombatEvent { kind: CE_CAST, actor: id, what: lands, point: from.to_array(), dir: b.dir.to_array(), ..Default::default() });
                tracing::info!("range: the bolt of spell {} strikes {id:#x} (part {part}) after {:.1} m", b.spell, b.travelled + d);
                struck.push(id);
                return false;
            }
            if let Some(d) = wall {
                effects.burst(from + b.dir * d, b.color);
                return false;
            }
            b.pos += b.dir * step;
            b.travelled += step;
            effects.bolt(b.pos, b.color);
            b.travelled < BOLT_RANGE
        });
        struck.len() as u32
    }
}
