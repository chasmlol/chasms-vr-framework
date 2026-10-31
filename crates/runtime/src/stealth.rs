//! Physical sneaking: leaning over or ducking in real life puts the game into its own crouch (its
//! sneak, where it has one), and standing up takes it out again. The game's mode is the game's,
//! with everything it brings (Oblivion's sneak eye, its detection, sneak attacks, its Sneak skill,
//! pickpocketing); this only decides when the player is in that posture, presses the game's own
//! Crouch toggle to keep the two in step, and keeps the view at the real head's height while the
//! game's camera goes down to its crouch. Design, trade-offs and what other games do:
//! `docs/research/stealth.md` in the Oblivion adapter's repository.
//!
//! * **The posture** is read off a point at the base of the neck, found from the head's pose (a
//!   fixed offset down and back from the eyes, [`NECK`]). Looking down pitches the head about the
//!   neck and barely moves that point; bending forward at the hips moves it forward and down along
//!   an arc round the hips; ducking moves it down. How far it has gone from where it is standing
//!   upright, down and ahead, is the lean's depth, against the setting's threshold (a slight lean
//!   to a deep crouch, [`threshold`]).
//! * **Upright** is learned: the neck's height standing (or sat up in a chair), and where the hips
//!   are under it. A lean held for minutes stays a lean: neither moves while the player is in it.
//!   Out of it, a slouch held still settles in slowly; standing up taller is taken after a second;
//!   a recentre takes the head as it is.
//! * **The hips** stay where they were while the neck moves along the arc a lean makes. Moving
//!   further than a lean that deep could (a step: the neck goes forward without going down), or
//!   moving steadily (walking) out of the posture, they are dragged along: walking about the room
//!   upright is never a lean, and creeping about in one stays one.
//! * **False leans:** a head pitched down (reading, the hands, a bench) counts less of the neck's
//!   forward movement (a little of it is the neck bending, not the hips), and a lean must be held a
//!   moment before it counts, longer the closer it is to the threshold. A quick glance down, a
//!   reach to a table, a moment's stoop don't flip the game's mode.
//! * **The game's toggle:** the body's posture is an order the game is kept to (pressed again if it
//!   didn't take, a few times at most), while the game decides what it can do. The stick's Crouch
//!   is the player's own and overrides the body until the body next changes; standing up doesn't
//!   undo a crouch the stick made. Where the game changes it by itself (a load, the water, a jump),
//!   a lean still held is put back after a moment; a load never takes a crouch away.

use glam::{Quat, Vec2, Vec3};

use crate::settings::Settings;

/// The base of the neck from the head's pose (its frame: x right, y up, z forward), metres: a
/// grown-up's seventh vertebra, under and behind the eyes. A little long rather than short: a head
/// nodding about a joint above it then lifts the point a little and moves it back, never forward.
pub const NECK: Vec3 = Vec3::new(0.0, -0.18, -0.09);
/// Hips to the base of the neck for an eye height of [`EYE_HEIGHT`] (metres; scaled by the
/// player's).
const TORSO: f32 = 0.52;
/// A typical upright eye height, which the depths below are for (metres).
const EYE_HEIGHT: f32 = 1.63;
/// The lean's depth that counts, at the setting's two ends (metres, for [`EYE_HEIGHT`]): a slight
/// lean (some 17 degrees at the hips, or a bit less with the knees bent a little) and a deep crouch
/// (the head some 55 cm down).
const DEPTH_SLIGHT: f32 = 0.15;
const DEPTH_DEEP: f32 = 0.55;
/// Out of the posture again under this much of the threshold (and at least 5 cm under it).
const EXIT_FRACTION: f32 = 0.6;
/// How long a lean is held before it counts (s): just over the threshold, and half as deep again
/// or more; standing up, how long under the exit before it's over.
const HOLD_IN_SHALLOW: f32 = 0.5;
const HOLD_IN_DEEP: f32 = 0.1;
const HOLD_OUT: f32 = 0.3;
/// The hips' give: the neck may be as far from them as a lean this deep could take it (the arc's
/// horizontal reach for its drop) times this, plus this much (metres) for sway and noise.
const ARC_GIVE: f32 = 1.25;
const ARC_SLACK: f32 = 0.04;
/// The neck moving along the floor faster than this (m/s, over a third of a second) for this long
/// (s) is walking: the hips go with it.
const WALK_SPEED: f32 = 0.28;
const WALK_FOR: f32 = 0.45;
/// Standing tall (within this of upright, metres), the hips come back under the neck over this
/// long (s).
const TALL: f32 = 0.01;
const HIPS_EASE: f32 = 1.5;
/// Looking down more than the first (degrees below level), the neck's forward movement counts less
/// towards going into the posture, down to the given share at the second. (In it, looking at the
/// ground is still sneaking: all of it counts.)
const PITCH_FULL: f32 = 30.0;
const PITCH_LEAST: f32 = 65.0;
const PITCH_SHARE: f32 = 0.4;
/// Looking down more than this (degrees) with a hand out ahead of the neck (this far along the
/// floor, metres) and well under the eyes (this far): a reach for something, picking it up or
/// working at a bench, not a lean. One held this long (s) still is.
const REACH_PITCH: f32 = 25.0;
const REACH_AHEAD: f32 = 0.35;
const REACH_BELOW: f32 = 0.2;
const REACH_HOLD: f32 = 1.5;
/// The head's upright height (metres over the real floor): heights a worn headset's at, sat or
/// stood (not on a desk); still within this for this long (s), it's worn; held more than this
/// higher for this long (s), it's upright there now, reached at this speed (m/s).
const UPRIGHT_LOW: f32 = 0.9;
const UPRIGHT_HIGH: f32 = 2.3;
const UPRIGHT_STILL: f32 = 0.05;
const UPRIGHT_SETTLE: f32 = 1.5;
const UPRIGHT_RISE: f32 = 0.03;
const UPRIGHT_HOLD: f32 = 1.0;
const UPRIGHT_SPEED: f32 = 0.1;
/// Out of the posture and held still (the neck slower than this, m/s, for this long, s), lower
/// than upright settles in: a slouch (less than the first, metres) at this speed (m/s), anything
/// lower hardly at all.
const CALM_SPEED: f32 = 0.05;
const CALM_FOR: f32 = 2.0;
const SLOUCH: f32 = 0.12;
const SLOUCH_SPEED: f32 = 0.005;
const LOW_SPEED: f32 = 0.002;
/// Not updated for this long (s: a menu, a loading screen), what the game is now is its own.
const GAP: f64 = 0.3;
/// Presses of the game's toggle: never closer than the first (s), again after the second if the
/// game didn't take one (s, then twice that), at most this many for one order; a change the game
/// made by itself is left this long (s) before the body's order is put back.
const PRESS_MIN: f64 = 0.4;
const PRESS_GAP: f64 = 0.6;
const PRESS_TRIES: u32 = 3;
const SELF_GRACE: f64 = 0.75;
/// A change within this long (s) of a press is that press's.
const PRESS_SEEN: f64 = 1.0;

/// The threshold for the setting's amount (0 a slight lean .. 1 a deep crouch), metres of depth
/// at [`EYE_HEIGHT`].
pub fn threshold(amount: f32) -> f32 {
    DEPTH_SLIGHT + (DEPTH_DEEP - DEPTH_SLIGHT) * amount.clamp(0.0, 1.0)
}

/// The setting's amount in words (the menu's slider).
pub fn describe(amount: f32) -> String {
    let words = match amount {
        a if a < 0.15 => "a slight lean",
        a if a < 0.4 => "a lean",
        a if a < 0.65 => "a crouch",
        a if a < 0.9 => "a low crouch",
        _ => "a deep crouch",
    };
    format!("{words} ({:.0} cm)", threshold(amount) * 100.0)
}

pub struct StealthInput<'a> {
    pub head_pos: Vec3,
    pub head_rot: Quat,
    pub dt: f32,
    pub now: f64,
    pub settings: &'a Settings,
    /// The game's crouch (its sneak) now, while it's being played.
    pub game_crouched: Option<bool>,
    /// The game's eye height over its feet now (metres; 0 when it doesn't say).
    pub eye_above_feet: f32,
    /// The game could crouch the player now: not on a mount, not swimming.
    pub can_crouch: bool,
    /// Where the hands are (tracking space), those tracked.
    pub hands: [Option<Vec3>; 2],
}

#[derive(Clone, Copy, Debug, Default)]
pub struct StealthOutput {
    /// Press the game's Crouch toggle this frame.
    pub press: bool,
    /// Metres to raise the view by: the game's crouch lowers its eye, the real head's already down.
    pub height_comp: f32,
    /// The game went in or out of its crouch (a buzz, if wanted).
    pub changed: bool,
}

pub struct Stealth {
    /// The neck's upright height (0: not known yet), and the head's then.
    stand_neck: f32,
    settled: bool,
    still: f32,
    above: f32,
    above_min: f32,
    recalibrate: bool,
    /// Where the hips are, along the floor (tracking x, z).
    hips: Option<Vec2>,
    last_neck: Option<Vec3>,
    /// The neck's speed along the floor and in all, smoothed; how long it's been fast (walking) or
    /// still (calm).
    speed: f32,
    speed_all: f32,
    moving: f32,
    calm: f32,
    drop: f32,
    /// The body's posture (in it, or not), and how long the lean's been past the threshold or back
    /// under the exit.
    lean: bool,
    over: f32,
    under: f32,
    /// What the body asks of the game (None: nothing, the stick or the game has it).
    order: Option<bool>,
    /// The game's crouch is the stick's: standing up leaves it.
    stick_owned: bool,
    stick_at: f64,
    pressed_at: f64,
    next_press: f64,
    tries: u32,
    game_was: Option<bool>,
    /// When to say what the game's crouch did to its eye and the view.
    report_at: Option<f64>,
    last_update: f64,
    /// The game's eye height standing (learned while it says and isn't crouched).
    stand_eye: f32,
    pub debug: StealthDebug,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct StealthDebug {
    pub depth: f32,
    pub drop: f32,
    pub ahead: f32,
    pub threshold: f32,
    pub walking: bool,
}

impl Default for Stealth {
    fn default() -> Self {
        Stealth {
            stand_neck: 0.0,
            settled: false,
            still: 0.0,
            above: 0.0,
            above_min: f32::MAX,
            recalibrate: false,
            hips: None,
            last_neck: None,
            speed: 0.0,
            speed_all: 0.0,
            moving: 0.0,
            calm: 0.0,
            drop: 0.0,
            lean: false,
            over: 0.0,
            under: 0.0,
            order: None,
            stick_owned: false,
            stick_at: f64::MIN,
            pressed_at: f64::MIN,
            next_press: 0.0,
            tries: 0,
            game_was: None,
            report_at: None,
            last_update: f64::MIN,
            stand_eye: 1.8,
            debug: StealthDebug::default(),
        }
    }
}

impl Stealth {
    /// The head's upright height (metres over the real floor; 0 until it's known): the base of the
    /// neck's, plus the head over it held level.
    pub fn stand_h(&self) -> f32 {
        if self.stand_neck > 0.0 { self.stand_neck - NECK.y } else { 0.0 }
    }

    /// The game's eye height standing, as it said last (or a guess).
    pub fn stand_eye(&self) -> f32 {
        self.stand_eye
    }

    /// The body's posture: leaning in or crouched.
    pub fn leaning(&self) -> bool {
        self.lean
    }

    /// A recentre: upright is where the head is now.
    pub fn recalibrate(&mut self) {
        self.recalibrate = true;
    }

    /// The player pressed the game's Crouch themself (the stick), with the game crouched or not:
    /// that's theirs until the body next changes.
    pub fn stick_pressed(&mut self, game_crouched: bool, now: f64) {
        self.order = None;
        self.stick_owned = !game_crouched;
        self.stick_at = now;
    }

    pub fn update(&mut self, inp: &StealthInput) -> StealthOutput {
        let (dt, now) = (inp.dt.max(0.0), inp.now);
        let s = inp.settings;
        let mut out = StealthOutput::default();
        let gap = now - self.last_update > GAP;
        self.last_update = now;
        if gap {
            // Back from a menu or a loading screen: the game's crouch now is what it is, and the
            // neck may be anywhere.
            self.last_neck = None;
            self.over = 0.0;
            self.under = 0.0;
            self.game_was = None;
            self.order = None;
            self.tries = 0;
        }

        // ---- the neck, and how it moves
        let h = inp.head_pos.y;
        let likely = (UPRIGHT_LOW..UPRIGHT_HIGH).contains(&h);
        let neck = inp.head_pos + inp.head_rot * NECK;
        let along = Vec2::new(neck.x, neck.z);
        if let Some(last) = self.last_neck.filter(|_| dt > 1e-4) {
            let k = (dt / 0.33).min(1.0);
            self.speed += (Vec2::new(neck.x - last.x, neck.z - last.z).length() / dt - self.speed) * k;
            self.speed_all += ((neck - last).length() / dt - self.speed_all) * k;
        } else {
            self.speed = 0.0;
            self.speed_all = 0.0;
        }
        self.last_neck = Some(neck);
        self.moving = if self.speed > WALK_SPEED { self.moving + dt } else { 0.0 };
        self.calm = if self.speed_all < CALM_SPEED { self.calm + dt } else { 0.0 };
        let walking = self.moving >= WALK_FOR;

        // ---- upright: until the headset's been still at a likely height a while (worn: not on
        // the desk, nor being put on) it follows the head; then held higher a second or more
        // (stood up) it rises to there over a moment, and a moment higher (a stretch, on tiptoe, a
        // tracking glitch) doesn't count. Lower, out of the posture and held still, it settles: a
        // slouch slowly, a crouch hardly at all. In the posture it holds.
        let n = neck.y;
        if std::mem::take(&mut self.recalibrate) && likely {
            self.stand_neck = n;
            self.settled = true;
            self.above = 0.0;
            self.above_min = f32::MAX;
            self.hips = Some(along);
        }
        if !self.settled {
            self.still = if likely && (n - self.stand_neck).abs() < UPRIGHT_STILL { self.still + dt } else { 0.0 };
            self.stand_neck = if self.stand_neck == 0.0 { n } else { self.stand_neck + (n - self.stand_neck) * (dt / 0.25).min(1.0) };
            self.settled = self.still >= UPRIGHT_SETTLE;
            self.hips = Some(along);
        } else if n > self.stand_neck + UPRIGHT_RISE {
            self.above += dt;
            self.above_min = self.above_min.min(n);
            if self.above >= UPRIGHT_HOLD {
                self.stand_neck = (self.stand_neck + UPRIGHT_SPEED * dt).min(self.above_min);
            }
        } else {
            self.above = 0.0;
            self.above_min = f32::MAX;
            if n < self.stand_neck && !self.lean && self.calm >= CALM_FOR {
                let speed = if self.stand_neck - n < SLOUCH { SLOUCH_SPEED } else { LOW_SPEED };
                self.stand_neck = (self.stand_neck - speed * dt).max(n);
            }
        }
        let scale = (self.stand_h() / EYE_HEIGHT).clamp(0.75, 1.25);
        let torso = TORSO * scale;

        // ---- the hips: where a lean bends from
        let drop_now = self.stand_neck - n;
        self.drop += (drop_now - self.drop) * (dt / 0.2).min(1.0);
        let drop = self.drop.max(0.0);
        let hips = self.hips.get_or_insert(along);
        // (Walking in the posture is creeping: the lean goes with it.)
        let reach = if walking && !self.lean { ARC_SLACK } else { ARC_SLACK + ARC_GIVE * (2.0 * torso * drop).sqrt() };
        let off = along - *hips;
        if off.length() > reach {
            *hips = along - off * (reach / off.length());
        }
        if drop < TALL {
            *hips += (along - *hips) * (dt / HIPS_EASE).min(1.0);
        }
        // Ahead: how far the neck is from the hips, unless it's behind the way the head faces
        // (leaning back to look up is no lean); going in, less of it the further the head's
        // pitched down.
        let off = along - *hips;
        let fwd = inp.head_rot * Vec3::Z;
        let facing = Vec2::new(fwd.x, fwd.z).normalize_or_zero();
        let behind = off.length() > 1e-3 && off.normalize().dot(facing) < -0.35;
        let pitch_down = -fwd.y.clamp(-1.0, 1.0).asin().to_degrees();
        let share = 1.0 - (1.0 - PITCH_SHARE) * ((pitch_down - PITCH_FULL) / (PITCH_LEAST - PITCH_FULL)).clamp(0.0, 1.0);
        let ahead = if behind { 0.0 } else { off.length() };
        let ahead = if self.lean { ahead } else { ahead * share };
        let depth = (drop * drop + ahead * ahead).sqrt();

        // ---- the posture
        let thr = threshold(s.input.stealth_crouch_amount) * scale;
        let exit = (thr * EXIT_FRACTION).min(thr - 0.05);
        let was = self.lean;
        if !self.settled {
            self.lean = false;
            self.over = 0.0;
        } else if !self.lean {
            self.over = if depth >= thr { self.over + dt } else { 0.0 };
            let past = ((depth / thr - 1.0) / 0.5).clamp(0.0, 1.0);
            // (Looking down, a lean has to be held longer; reaching for something, longer still.)
            let reaching = pitch_down > REACH_PITCH
                && inp.hands.iter().flatten().any(|p| (Vec2::new(p.x, p.z) - along).dot(facing) > REACH_AHEAD * scale && p.y < inp.head_pos.y - REACH_BELOW * scale);
            let hold = (HOLD_IN_SHALLOW + (HOLD_IN_DEEP - HOLD_IN_SHALLOW) * past) * (2.0 - share);
            let hold = if reaching { hold.max(REACH_HOLD) } else { hold };
            if self.over >= hold {
                self.lean = true;
                self.under = 0.0;
            }
        } else {
            self.under = if depth < exit { self.under + dt } else { 0.0 };
            if self.under >= HOLD_OUT {
                self.lean = false;
                self.over = 0.0;
            }
        }
        self.debug = StealthDebug { depth, drop, ahead, threshold: thr, walking };
        if self.lean != was {
            tracing::info!(
                "stealth: {} (depth {depth:.2} m of {thr:.2}: {drop:.2} down, {ahead:.2} ahead, head {pitch_down:.0} degrees down{})",
                if self.lean { "leaning in" } else { "standing up" },
                if walking { ", walking" } else { "" }
            );
            self.order = if self.lean || !self.stick_owned { Some(self.lean) } else { None };
            self.tries = 0;
            self.next_press = self.next_press.min(now).max(self.pressed_at + PRESS_MIN);
        } else if gap && self.lean {
            // (A lean held through a menu or a load is still one.)
            self.order = Some(true);
        }

        // ---- the game's toggle
        let physical = s.input.physical_crouch && !s.comfort.seated;
        if !physical {
            self.order = None;
        }
        if let Some(game) = inp.game_crouched {
            if let Some(was) = self.game_was
                && was != game
            {
                out.changed = true;
                self.report_at = Some(now + 1.0);
                let ours = now - self.pressed_at < PRESS_SEEN || now - self.stick_at < PRESS_SEEN;
                if !ours {
                    tracing::info!("stealth: the game {} by itself", if game { "crouched" } else { "stood up" });
                    self.next_press = self.next_press.max(now + SELF_GRACE);
                    self.tries = 0;
                }
            }
            self.game_was = Some(game);
            if !game && now - self.stick_at > PRESS_SEEN {
                self.stick_owned = false;
            }
            match self.order {
                Some(want) if want != game && physical && inp.can_crouch => {
                    if self.tries >= PRESS_TRIES {
                        if now >= self.next_press {
                            tracing::warn!("stealth: the game didn't {} after {} presses: left as it is", if want { "crouch" } else { "stand up" }, self.tries);
                            self.order = None;
                        }
                    } else if now >= self.next_press {
                        out.press = true;
                        self.tries += 1;
                        self.pressed_at = now;
                        self.next_press = now + PRESS_GAP * self.tries as f64;
                    }
                }
                Some(want) if want == game => self.tries = 0,
                _ => {}
            }
            // The game's crouch lowers its eye; the real head's already as low as it is.
            if inp.eye_above_feet > 1.0 && inp.eye_above_feet < 2.5 && !game {
                self.stand_eye += (inp.eye_above_feet - self.stand_eye) * 0.05;
            }
            if physical && game && (self.lean || !self.stick_owned) && inp.eye_above_feet > 0.3 {
                out.height_comp = (self.stand_eye - inp.eye_above_feet).max(0.0);
            }
            if self.report_at.is_some_and(|t| now >= t) {
                self.report_at = None;
                tracing::info!(
                    "stealth: the game {} a second ago: its eye {:.2} m over its feet ({:.2} standing), the view held up {:.2} m; the real head {:.2} m under upright",
                    if game { "crouched" } else { "stood up" },
                    inp.eye_above_feet,
                    self.stand_eye,
                    out.height_comp,
                    self.stand_h() - h
                );
            }
        } else {
            self.game_was = None;
        }
        out
    }
}
