//! The player's powers beside their hands and weapons (`docs/MELEE.md`), and stamina:
//!
//! * **Slowing time** (on a focus meter): clicking the off hand's stick slows the game's time to
//!   half, snapping in and easing out over 0.7 s. It runs on focus: it needs more than 10 to start,
//!   drains 6 a second (real time) and stops when it's spent; focus comes back at 3 a second (half
//!   while walking). The hands keep the player's real speed, so in the slowed world they swing
//!   twice as fast and hit twice as hard.
//! * **Stamina** (an option, off by default): swinging a weapon hard costs it (the setting's
//!   seconds of an arming sword swung hard empty it, a heavier weapon or a faster swing sooner),
//!   and it comes back once the player has rested a moment; a tired player's hands are weaker on
//!   their weapons. A game that keeps its own (Oblivion's Fatigue) gives it ([`MeleeRules`]) and
//!   is told what the swings spent.
//! * **Imbuing**: a hand charging a spell that's fire or shock ([`SPELL_FIRE`], [`SPELL_SHOCK`])
//!   held within 0.2 m of a held weapon's blade fills it (50 a second at full charge, twice that
//!   for a weapon of tier 1 to 3: full in a second), and it drains (1 a second at tier 2, a
//!   shock's at 0.4 of that; 3 a hit; 5 a second dropped). Its hits carry the spell
//!   ([`CEF_IMBUED`]); the game makes the burn or the shock.
//!
//! The game is told all of it each frame ([`RtMelee`]): the time scale, the focus, the stamina
//! spent, the imbued weapons.

use std::collections::HashMap;

use glam::Vec3;
use rustvr_protocol::*;

/// Slowed time's scale (0.5), how fast it eases back out (s).
pub const SLOW_SCALE: f32 = 0.5;
pub const SLOW_OUT: f32 = 0.7;
/// The focus meter (30 at the start, at most 60 (the start times 1 + 1), needs more than 10 to
/// slow time, drains 6 a second of real time, back at 3 a second of the game's always (half that
/// while walking): about 6.7 s of slowed time from 30).
pub const FOCUS_START: f32 = 30.0;
pub const FOCUS_MAX: f32 = 60.0;
pub const FOCUS_TO_START: f32 = 10.0;
pub const FOCUS_DRAIN: f32 = 6.0;
pub const FOCUS_REGEN: f32 = 3.0;
/// Imbuing (within 0.2 m; 50 a second; at most 100; a shock's constant loss x0.4; 5 a second
/// lying dropped).
pub const IMBUE_RADIUS: f32 = 0.2;
pub const IMBUE_RATE: f32 = 50.0;
pub const IMBUE_MAX: f32 = 100.0;
pub const IMBUE_SHOCK_LOSS: f32 = 0.4;
pub const IMBUE_DROPPED_LOSS: f32 = 5.0;
/// A blade's imbuing by its item's tier 0..5: how much faster it fills (tier 0 can't be filled at
/// all), what it loses a second, and what a hit takes.
pub const IMBUE_FILL: [f32; 6] = [0.0, 2.0, 2.0, 2.0, 3.0, 3.0];
pub const IMBUE_LOSS: [f32; 6] = [3.0, 2.0, 1.0, 0.45, 0.1, 0.05];
pub const IMBUE_HIT_LOSS: [f32; 6] = [30.0, 4.0, 3.0, 2.0, 0.5, 0.1];
/// Stamina (ours): a blade's ends faster than this (m/s) is a swing; one this fast, of a weapon
/// this heavy (an arming sword's 1.25 kg), swung the setting's seconds, empties it. It comes back
/// over the setting's refill time once nothing has been swung for this long (s).
const SWING_FROM: f32 = 3.0;
const SWING_HARD: f32 = 8.0;
const SWING_MASS: f32 = 1.25;
const REST_AFTER: f64 = 1.0;
/// Tired, a hand's hold is down to this share of its strength.
const TIRED_STRENGTH: f32 = 0.5;

#[derive(Debug)]
pub struct Powers {
    /// The focus meter, and whether time is slowed (since when it started easing out).
    pub focus: f32,
    slowing: bool,
    easing_out: Option<f64>,
    pub time_scale: f32,
    /// The runtime's own stamina (0..1), and what the swings spent in all (the game's units when it
    /// keeps its own).
    pub stamina: f32,
    spent: f32,
    /// Each weapon's imbue: the spell in it and how full (0..100).
    pub imbues: HashMap<u32, Filled>,
    /// Each hand's imbuing this frame (for the game).
    pub imbuing: [Imbue; 2],
    stick_was: [bool; 2],
    /// Each held weapon's blades' ends last frame (world), and when the player last swung.
    tips: HashMap<u32, Vec<Vec3>>,
    swung_at: f64,
}

impl Default for Powers {
    fn default() -> Self {
        Powers { focus: FOCUS_START, slowing: false, easing_out: None, time_scale: 1.0, stamina: 1.0, spent: 0.0, imbues: HashMap::new(), imbuing: Default::default(), stick_was: [false; 2], tips: HashMap::new(), swung_at: 0.0 }
    }
}

/// What the powers see each frame.
pub struct PowersInput<'a> {
    pub frame: &'a crate::backend::HmdFrame,
    pub settings: &'a crate::settings::Melee,
    pub rules: &'a MeleeRules,
    pub dominant: usize,
    /// Real seconds since the last frame, and now.
    pub dt: f32,
    pub now: f64,
    /// The player is walking (the stick moves them).
    pub walking: bool,
    /// Each hand's spell as it charges (from the combat): its spell, its charge, where its orb is
    /// (world); and whether that spell is fire or shock.
    pub charging: [Option<(u32, f32, Vec3, u32)>; 2],
    /// The held melee weapons: (id, its tier, its mass, each blade's ends (world)), and the world's
    /// units a metre.
    pub held: Vec<(u32, u32, f32, Vec<(Vec3, Vec3)>)>,
    pub world_per_meter: f32,
    /// The weapons lying loose (their ids), and the hits this frame (the weapon's id).
    pub loose: Vec<u32>,
    pub hits: Vec<u32>,
    pub paused: bool,
}

/// A weapon filled with a spell: which (and its element, `SPELL_*`), how full (0..100), its tier
/// (0..5).
#[derive(Clone, Copy, Debug)]
pub struct Filled {
    pub spell: u32,
    pub charge: f32,
    pub element: u32,
    pub tier: usize,
}

impl Powers {
    /// One frame: the slowing (by the off hand's stick click), the stamina, the imbuing.
    pub fn update(&mut self, inp: &PowersInput) {
        let dt = inp.dt.max(0.0);
        if inp.paused {
            return;
        }
        let allowed = inp.settings.slow_time && (inp.rules.flags & RULES_SAYS_SLOW == 0 || inp.rules.flags & RULES_SLOW_TIME != 0);
        // The off hand's stick, clicked (with the melee buttons the dominant hand's B): slow time
        // (if there's focus for it), or stop.
        let (hand, button) = if inp.rules.flags & RULES_KICK != 0 { (inp.dominant, CTRL_B) } else { (inp.dominant ^ 1, CTRL_STICK_CLICK) };
        for h in 0..2 {
            let down = inp.frame.hands[h].buttons & button != 0;
            if h == hand && down && !self.stick_was[h] && allowed {
                if self.slowing {
                    self.stop(inp.now);
                } else if self.focus > FOCUS_TO_START {
                    self.slowing = true;
                    self.easing_out = None;
                    tracing::info!("powers: time slows (focus {:.0})", self.focus);
                }
            }
            self.stick_was[h] = down;
        }
        // The game's own slowing, where it says (how slow, how long a full meter lasts, how long it
        // takes to fill), else ours.
        let r = inp.rules;
        let slow = if r.slow_scale > 0.0 { r.slow_scale.clamp(0.05, 1.0) } else { SLOW_SCALE };
        let drain = if r.focus_time > 0.0 { FOCUS_MAX / r.focus_time } else { FOCUS_DRAIN };
        let regen = if r.focus_refill > 0.0 { FOCUS_MAX / r.focus_refill } else { FOCUS_REGEN };
        // Regained always, in the game's time (half as fast while it's slowed, and while walking);
        // spent in real time while slowed.
        self.focus = (self.focus + regen * if inp.walking { 0.5 } else { 1.0 } * self.time_scale * dt).min(FOCUS_MAX);
        if self.slowing {
            self.focus -= drain * dt;
            if self.focus <= 0.0 || !allowed {
                self.focus = self.focus.max(0.0);
                self.stop(inp.now);
            }
        }
        // Snapped in; eased back out (slow then steep).
        self.time_scale = match (self.slowing, self.easing_out) {
            (true, _) => slow,
            (false, Some(t0)) => {
                let k = (((inp.now - t0) as f32) / SLOW_OUT).clamp(0.0, 1.0);
                if k >= 1.0 {
                    self.easing_out = None;
                }
                slow + (1.0 - slow) * k * k
            }
            (false, None) => 1.0,
        };
        self.stamina(inp, dt);
        self.imbue(inp, dt);
    }

    fn stop(&mut self, now: f64) {
        if self.slowing {
            self.slowing = false;
            self.easing_out = Some(now);
            tracing::info!("powers: time speeds up again (focus {:.0})", self.focus);
        }
    }

    /// Swings cost stamina (by how fast the weapon's blades go past a swing's speed, and its
    /// weight), and it comes back once the player has rested.
    fn stamina(&mut self, inp: &PowersInput, dt: f32) {
        let game = inp.rules.flags & RULES_STAMINA != 0 && inp.rules.stamina_max > 0.0;
        let on = inp.settings.stamina || game;
        let w = inp.world_per_meter.max(1e-3);
        // How hard the weapons are swung: each by its blades' fastest end, against an arming sword
        // swung hard (1).
        let mut effort = 0.0;
        let mut tips = HashMap::new();
        for (id, _, mass, blades) in &inp.held {
            let ends: Vec<Vec3> = blades.iter().flat_map(|&(a, b)| [a, b]).collect();
            if let Some(was) = self.tips.get(id).filter(|was| was.len() == ends.len()) {
                let speed = ends.iter().zip(was).map(|(p, q)| p.distance(*q)).fold(0.0, f32::max) / w / dt.max(1e-4);
                effort += ((speed - SWING_FROM) / (SWING_HARD - SWING_FROM)).max(0.0) * mass / SWING_MASS;
            }
            tips.insert(*id, ends);
        }
        self.tips = tips;
        if effort > 0.0 {
            self.swung_at = inp.now;
        }
        if !on {
            self.stamina = 1.0;
            return;
        }
        let used = effort * dt / inp.settings.stamina_time.max(1.0);
        if game {
            // The game's units: what a full meter is to it.
            self.spent += used * inp.rules.stamina_max * if inp.rules.swing_cost > 0.0 { inp.rules.swing_cost } else { 1.0 };
            self.stamina = (inp.rules.stamina / inp.rules.stamina_max).clamp(0.0, 1.0);
        } else {
            self.stamina -= used;
            self.spent += used;
            if inp.now - self.swung_at > REST_AFTER {
                self.stamina += dt / inp.settings.stamina_refill.max(0.5);
            }
            self.stamina = self.stamina.clamp(0.0, 1.0);
        }
    }

    /// A tired hand's strength on its weapon (1: fresh).
    pub fn strength(&self) -> f32 {
        TIRED_STRENGTH + (1.0 - TIRED_STRENGTH) * self.stamina
    }

    /// A charged hand by a held blade fills it with its spell; imbued weapons drain.
    fn imbue(&mut self, inp: &PowersInput, dt: f32) {
        self.imbuing = Default::default();
        for h in 0..2 {
            let Some((spell, charge, orb, element)) = inp.charging[h] else { continue };
            if element & (SPELL_FIRE | SPELL_SHOCK) == 0 || charge <= 0.0 {
                continue;
            }
            for (id, tier, _, blades) in &inp.held {
                let gap = blades
                    .iter()
                    .map(|&(a, b)| {
                        let ab = b - a;
                        let t = ((orb - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                        orb.distance(a + ab * t)
                    })
                    .fold(f32::INFINITY, f32::min);
                tracing::debug!("powers: hand {h}'s charge at {orb:.2} {gap:.2} m from {id:#x}'s blades {blades:.2?}");
                if gap > IMBUE_RADIUS {
                    continue;
                }
                let t = (*tier as usize).min(5);
                let rate = IMBUE_RATE * charge * IMBUE_FILL[t];
                if rate <= 0.0 {
                    continue;
                }
                if !self.imbues.contains_key(id) {
                    tracing::info!("powers: hand {h} fills {id:#x} with spell {spell:#x}");
                }
                let e = self.imbues.entry(*id).or_insert(Filled { spell, charge: 0.0, element, tier: t });
                // A different spell drains the one in it first.
                if e.spell != spell {
                    e.charge -= rate * dt;
                    if e.charge <= 0.0 {
                        *e = Filled { spell, charge: 0.0, element, tier: t };
                    }
                } else {
                    e.charge = (e.charge + rate * dt).min(IMBUE_MAX);
                }
                self.imbuing[h] = Imbue { object: *id, spell: e.spell, charge: e.charge / IMBUE_MAX, _pad: 0 };
            }
        }
        // Losses by the blade's tier: a little every second (a shock's less), a hit's, more lying
        // dropped.
        for (id, f) in &mut self.imbues {
            let element = if f.element & SPELL_SHOCK != 0 { IMBUE_SHOCK_LOSS } else { 1.0 };
            f.charge -= IMBUE_LOSS[f.tier] * element * dt;
            if inp.loose.contains(id) {
                f.charge -= IMBUE_DROPPED_LOSS * dt;
            }
            f.charge -= IMBUE_HIT_LOSS[f.tier] * inp.hits.iter().filter(|h| *h == id).count() as f32;
        }
        self.imbues.retain(|_, f| f.charge > 0.0);
    }

    /// The imbue of weapon `id`, as the combat reads it.
    pub fn imbue_of(&self, id: u32) -> Option<Imbue> {
        self.imbues.get(&id).map(|f| Imbue { object: id, spell: f.spell, charge: f.charge / IMBUE_MAX, _pad: 0 })
    }

    /// For the game: the time scale, the focus, the stamina, each hand's weapon's imbue.
    pub fn report(&self, held: [Option<u32>; 2]) -> RtMelee {
        let mut r: RtMelee = bytemuck::Zeroable::zeroed();
        r.flags = if self.time_scale < 0.999 { RTM_SLOW } else { 0 } | if self.stamina <= 0.05 { RTM_SPENT } else { 0 };
        r.time_scale = self.time_scale;
        r.focus = self.focus / FOCUS_MAX;
        r.stamina_spent = self.spent;
        r.stamina = self.stamina;
        for h in 0..2 {
            r.imbue[h] = held[h].and_then(|id| self.imbue_of(id)).unwrap_or(self.imbuing[h]);
        }
        let mut loose: Vec<Imbue> = self.imbues.keys().filter(|id| !held.contains(&Some(**id))).filter_map(|&id| self.imbue_of(id)).collect();
        loose.sort_by(|a, b| b.charge.total_cmp(&a.charge));
        for (k, i) in loose.into_iter().take(2).enumerate() {
            r.loose_imbues[k] = i;
        }
        r
    }
}
