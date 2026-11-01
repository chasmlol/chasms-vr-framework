//! The duel: the second melee mode (`docs/DUEL.md`; every timing and weight in
//! `rustvr_melee::duel`), played instead of the physics sandbox's blocks when the game asks for it
//! (`MeleeRules::mode`) or the player chooses it (`[melee] mode`). The runtime has the player's side:
//!
//! * **The parry.** A held blade pushed fast ([`tune::PARRY_SPEED`]) opens a parry's window
//!   ([`tune::PARRY_WINDOW`]); one that met nothing closes into a recovery when no new one opens.
//!   A creature's swing (`AI_ATTACKING`) the blade meets moving into it within its window is
//!   parried (`CE_BLOCK`, `CEF_PARRY`): its blows don't land, and the player's blade ricochets off
//!   it with the way for the counter, the weapon lighter in the hand for the riposte.
//! * **Beaten.** A swing met outside a window, or by a blade only held in its way, beats the blade
//!   aside and carries on into the player (`CE_BLOCK`, no flag). Nothing but a parry stops a blow.
//! * **Being parried.** The player's blade meeting a creature's parry (`AI_PARRYING`) is turned
//!   away (`CEF_PARRIED`) and staggered: heavy in the hand and harmless until it recovers, so the
//!   creature's counter lands before the player can swing again. It can still parry the counter.
//! * **Weight.** A weapon in a duel is held weaker than the sandbox holds it (it lags, swings on,
//!   sags by its mass), and the player walks slower with one drawn.
//!
//! Drags and accels need nothing: the blade is the player's own, a hit lands when it really reaches
//! the creature, and a creature that reads the blade's motion (the game's brain) is fooled by them.
//! The creatures' weapons are solid to nothing the player holds in a duel (`rig::duel`): the blades
//! meet only as these rules say.

use std::collections::HashMap;

use glam::Vec3;
use rustvr_melee::duel as tune;
use rustvr_protocol::*;

use crate::combat::{Pulse, closest_segments};
use crate::rig::feedback::Cue;

/// One of the player's hands in the duel.
#[derive(Clone, Copy, Debug, Default)]
struct Hand {
    /// The weapon it holds (object id; 0: none).
    object: u32,
    /// Its parry's window opened then; no new one opens until then (one that met nothing), nor
    /// before the blade has slowed since the last.
    window: Option<f64>,
    recover_until: f64,
    armed: bool,
    /// It was parried then (staggered since); it parried a blow then (its riposte since).
    staggered: Option<f64>,
    riposte: Option<f64>,
}

/// A held weapon sent off a clash: at this velocity (world units a second, the world's frame), its
/// turning damped.
#[derive(Clone, Copy, Debug)]
pub struct Kick {
    pub object: u32,
    pub vel: Vec3,
}

/// What happened in the duel so far (for the logs and the tests).
#[derive(Clone, Copy, Debug, Default)]
pub struct DuelStats {
    pub windows: u32,
    pub parries: u32,
    pub parried: u32,
    pub beaten: u32,
}

/// A creature's weapon as the clash sees it: the game's item, where its centre is, and its blades
/// (their ends and radius, world).
pub struct Theirs {
    pub item: ActorItem,
    pub centre: Vec3,
    pub blades: Vec<(Vec3, Vec3, f32)>,
}

#[derive(Default)]
pub struct Duel {
    /// The duel's rules are played.
    pub on: bool,
    hands: [Hand; 2],
    /// Creatures' weapons whose swing the player parried or that has landed, until when (that swing
    /// lands nowhere else).
    parried: HashMap<u32, f64>,
    /// A weapon of the player's and a creature's that met, until when they can meet again.
    met: HashMap<(u32, u32), f64>,
    /// Held weapons sent off clashes this frame, for the rig.
    pub kicks: Vec<Kick>,
    pub stats: DuelStats,
}

impl Duel {
    /// The duel's rules on or off; off forgets every window, stagger and riposte.
    pub fn set_on(&mut self, on: bool) {
        if on != self.on {
            tracing::info!("duel: the duel's rules {}", if on { "are played" } else { "are off: the physics sandbox" });
            *self = Duel { on, stats: self.stats, ..Default::default() };
        }
    }

    /// Hand `hand`'s weapon is staggered (parried): it does no harm.
    pub fn harmless(&self, hand: usize, now: f64) -> bool {
        self.on && self.hands[hand].staggered.is_some_and(|s| now - s < tune::STAGGER)
    }

    /// Creature weapon `item`'s swing was parried, or has landed: it lands nowhere (else) now.
    pub fn swing_parried(&self, item: u32, now: f64) -> bool {
        self.on && self.parried.get(&item).is_some_and(|&t| t > now)
    }

    /// Creature weapon `item`'s swing struck the player: a blow lands once.
    pub fn landed(&mut self, item: u32, now: f64) {
        if self.on {
            self.parried.insert(item, now + tune::RELEASE);
        }
    }

    /// How strongly each hand holds its weapon, of the physics sandbox's hold (1 when the duel's off).
    pub fn drive(&self, now: f64) -> [f32; 2] {
        std::array::from_fn(|h| {
            if !self.on {
                return 1.0;
            }
            let hand = &self.hands[h];
            let staggered = hand.staggered.map(|s| now - s).filter(|&s| s < tune::STAGGER);
            let riposte = hand.riposte.map(|r| now - r).filter(|&r| r < tune::RIPOSTE_TIME);
            tune::drive(staggered, riposte)
        })
    }

    /// How fast the stick walks the player (of its own speed): slower with a melee weapon drawn.
    pub fn walk(&self, melee_drawn: bool) -> f32 {
        if self.on && melee_drawn { tune::MOVE_DRAWN } else { 1.0 }
    }

    /// The player's side for the game (`RtMelee::flags`).
    pub fn flags(&self, now: f64) -> u32 {
        if !self.on {
            return 0;
        }
        RTM_DUEL | if (0..2).any(|h| self.harmless(h, now)) { RTM_STAGGERED } else { 0 }
    }

    /// The frame's end: what's over is forgotten.
    pub fn tidy(&mut self, now: f64) {
        self.parried.retain(|_, t| *t > now);
        self.met.retain(|_, t| *t > now);
    }

    /// Hand `hand`'s weapon `object` this frame: its damagers' points last frame and now (world),
    /// their radii (metres), against the creatures' weapons. Its parry's window opens and closes by
    /// how fast it's pushed, and where it meets one of theirs, the clash is judged: a parry, parried,
    /// or beaten aside. Each is told to the game, felt and heard.
    #[allow(clippy::too_many_arguments)]
    pub fn clash<const N: usize>(&mut self, hand: usize, object: u32, prev: &[[Vec3; N]], now_at: &[[Vec3; N]], radii: &[f32], theirs: &[Theirs], wpm: f32, dt: f32, now: f64, out: &mut Clashes) {
        if now_at.is_empty() || prev.len() != now_at.len() {
            return;
        }
        let h = &mut self.hands[hand];
        if h.object != object {
            *h = Hand { object, armed: true, ..Default::default() };
        }
        // How fast the blade's pushed: at the middle of its first damager.
        let mid = N / 2;
        let speed = (now_at[0][mid] - prev[0][mid]).length() / dt.max(1e-4) / wpm;
        if let Some(t) = h.window
            && now - t > tune::PARRY_WINDOW
        {
            h.window = None;
            h.recover_until = now + tune::PARRY_RECOVERY;
        }
        if speed < tune::PARRY_SPEED * 0.8 {
            h.armed = true;
        }
        let can_parry = h.staggered.is_none_or(|s| now - s >= tune::STAGGER_PARRY);
        if h.window.is_none() && h.armed && speed > tune::PARRY_SPEED && now >= h.recover_until && can_parry {
            h.window = Some(now);
            h.armed = false;
            self.stats.windows += 1;
            tracing::debug!("duel: hand {hand}'s parry opens ({speed:.1} m/s)");
        }
        if h.staggered.is_some_and(|s| now - s >= tune::STAGGER) {
            h.staggered = None;
        }
        if h.riposte.is_some_and(|r| now - r >= tune::RIPOSTE_TIME) {
            h.riposte = None;
        }
        for t in theirs {
            let item = t.item;
            let parrying = item.flags & AI_PARRYING != 0;
            let attacking = item.flags & AI_ATTACKING != 0;
            // (No passive blocks in a duel: a creature's blade at rest, or a shield, is met by nothing.)
            if item.flags & AI_SHIELD != 0 || !(parrying || attacking) || self.met.get(&(object, item.id)).is_some_and(|&u| u > now) {
                continue;
            }
            let reach = if parrying { tune::PARRY_REACH * wpm } else { 0.0 };
            if tracing::enabled!(tracing::Level::DEBUG) {
                let near = t.blades.iter().map(|&(a, b, _)| {
                    let (s, u) = closest_segments(now_at[0][0], now_at[0][N - 1], a, b);
                    now_at[0][0].lerp(now_at[0][N - 1], s).distance(a.lerp(b, u)) / wpm
                });
                tracing::debug!("duel: hand {hand}'s blade {:.2} m from {:#x}'s ({})", near.fold(f32::MAX, f32::min), item.id, if parrying { "parrying" } else { "swinging" });
            }
            let Some((p, q, vel)) = Self::meet(prev, now_at, radii, t, reach, wpm, dt) else { continue };
            let theirs_vel = Vec3::from_array(item.vel) + Vec3::from_array(item.ang_vel).cross(q - t.centre);
            let mine = vel / wpm;
            let toward = (q - p).normalize_or(mine.normalize_or_zero());
            let relative = (vel - theirs_vel).length() / wpm;
            if relative < 1.0 && mine.length() < 1.0 {
                // (Resting against it, not a blow either way.)
                continue;
            }
            self.met.insert((object, item.id), now + tune::CLASH_COOLDOWN);
            let h = &mut self.hands[hand];
            let mut ev = CombatEvent { kind: CE_BLOCK, actor: item.actor, object, what: item.id, hand: hand as u32, material: MAT_PLATE, strength: (relative / 6.0).min(1.5), speed: relative, point: p.to_array(), dir: mine.normalize_or_zero().to_array(), ..Default::default() };
            let loud = rustvr_melee::intensity(relative).max(0.3);
            if parrying && mine.length() < tune::BLOW_SPEED {
                // (A blade at rest isn't a blow to parry: it passes.)
                continue;
            } else if parrying {
                // Its parry turns the blow away: knocked back off it, and staggered.
                h.staggered = Some(now);
                h.window = None;
                h.riposte = None;
                ev.flags = CEF_PARRIED;
                self.stats.parried += 1;
                let back = (-toward * tune::DEFLECT - mine * 0.3) * wpm;
                self.kicks.push(Kick { object, vel: back });
                out.pulses.push(Pulse { hand, seconds: 0.3, frequency: 45.0, amplitude: 1.0 });
                tracing::info!("duel: the player's blow ({:.1} m/s) parried by {:#x}'s {:#x}: staggered for {:.2} s", mine.length(), item.actor, item.id, tune::STAGGER);
            } else {
                // Pushed into the swing inside a window: a parry. Else beaten aside.
                let way = mine.normalize_or_zero();
                let into = way.dot(toward).max(way.dot(-(theirs_vel / wpm).normalize_or_zero()));
                let opened = h.window.filter(|&t| now - t <= tune::PARRY_WINDOW);
                let open = opened.is_some();
                if open && into > tune::PARRY_INTO && mine.length() > tune::PARRY_SPEED * 0.5 {
                    h.window = None;
                    h.armed = false;
                    h.staggered = None;
                    h.riposte = Some(now);
                    ev.flags = CEF_PARRY;
                    self.stats.parries += 1;
                    self.parried.insert(item.id, now + tune::RELEASE + 0.1);
                    // The ricochet: off the other blade, the push turned back, with the way for the
                    // counter.
                    let bounce = (way - 2.0 * way.dot(toward) * toward).normalize_or(-toward);
                    let speed = (mine.length() * 0.6).max(tune::RICOCHET);
                    self.kicks.push(Kick { object, vel: bounce * speed * wpm });
                    out.pulses.push(Pulse { hand, seconds: 0.12, frequency: 90.0, amplitude: 1.0 });
                    tracing::info!("duel: the player parried {:#x}'s {:#x} ({:.1} m/s into it, {:.2} s into the window): the riposte's on", item.actor, item.id, mine.length(), now - opened.unwrap_or(now));
                } else {
                    // Beaten aside along the blow, which carries on.
                    let along = (theirs_vel / wpm).normalize_or(-toward);
                    self.kicks.push(Kick { object, vel: (along * tune::BEATEN + mine * 0.3) * wpm });
                    self.stats.beaten += 1;
                    out.pulses.push(Pulse { hand, seconds: 0.08, frequency: 70.0, amplitude: 0.6 });
                    tracing::info!(
                        "duel: the player's blade beaten aside by {:#x}'s swing ({})",
                        item.actor,
                        if !open { "no parry open" } else if into <= tune::PARRY_INTO { "not pushed into it" } else { "too slow" }
                    );
                }
            }
            out.sounds.push((Cue::Clash, p, loud, object));
            out.events.push(ev);
            break;
        }
    }

    /// Where the player's blade (its damagers' points last frame and now) first meets one of a
    /// creature's (`reach` further out than it is; where it was a frame ago taken from how it moves,
    /// about `centre`): the point on each, and how fast the player's moves there (world units a
    /// second). Both are swept through the frame, so blades that crossed between frames met.
    #[allow(clippy::too_many_arguments)]
    fn meet<const N: usize>(prev: &[[Vec3; N]], now: &[[Vec3; N]], radii: &[f32], theirs: &Theirs, reach: f32, wpm: f32, dt: f32) -> Option<(Vec3, Vec3, Vec3)> {
        let dt = dt.max(1e-4);
        let (v, w) = (Vec3::from_array(theirs.item.vel), Vec3::from_array(theirs.item.ang_vel));
        let back = |p: Vec3| p - (v + w.cross(p - theirs.centre)) * dt;
        for (di, (pd, nd)) in prev.iter().zip(now).enumerate() {
            let r = radii.get(di).copied().unwrap_or(0.0) * wpm;
            for &(ta, tb, tr) in &theirs.blades {
                let limit = tr + r + reach + 0.02 * wpm;
                let (pa, pb) = (back(ta), back(tb));
                for k in 0..=STEPS {
                    let f = k as f32 / STEPS as f32;
                    let (a, b) = (pd[0].lerp(nd[0], f), pd[N - 1].lerp(nd[N - 1], f));
                    let (c, d) = (pa.lerp(ta, f), pb.lerp(tb, f));
                    let (s, t) = closest_segments(a, b, c, d);
                    let (p, q) = (a.lerp(b, s), c.lerp(d, t));
                    if p.distance(q) <= limit {
                        let i = ((s * (N - 1) as f32).round() as usize).min(N - 1);
                        return Some((p, q, (nd[i] - pd[i]) / dt));
                    }
                }
            }
        }
        None
    }
}

/// A frame's sweep of two blades is tried at this many steps (each blade moving a few centimetres
/// a step at a swing's speed).
const STEPS: usize = 8;

/// The player's body as a duel's held weapons rest on it (world: ends, radius), from the solved
/// body's joints (tracking space): the head, the torso, and the line across the shoulders (where a
/// blade rests on one).
pub fn body_capsules(joints: &[Pose; JOINT_COUNT], map: &crate::rig::WorldMap) -> Vec<(Vec3, Vec3, f32)> {
    let at = |j: Joint| map.point(Vec3::from_array(joints[j as usize].pos));
    let w = map.world_per_meter;
    if joints[Joint::Head as usize].valid == 0 {
        return Vec::new();
    }
    let up = (at(Joint::Head) - at(Joint::Neck)).normalize_or(Vec3::Y);
    vec![
        (at(Joint::Neck) + up * 0.04 * w, at(Joint::Head) + up * 0.08 * w, tune::BODY_HEAD * w),
        (at(Joint::Hips), at(Joint::UpperChest), tune::BODY_TORSO * w),
        (at(Joint::LeftUpperArm), at(Joint::RightUpperArm), tune::BODY_SHOULDERS * w),
    ]
}

/// What a frame's clashes give the game and the player: events, buzzes, sounds (as `Combat` keeps
/// them).
pub struct Clashes<'a> {
    pub events: &'a mut Vec<CombatEvent>,
    pub pulses: &'a mut Vec<Pulse>,
    pub sounds: &'a mut Vec<(Cue, Vec3, f32, u32)>,
}
