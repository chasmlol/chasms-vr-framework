//! A gun's recoil as its game authored it (the adapter's [`KickTable`]), played on the gun in the
//! hand.
//!
//! Each shot starts one kick of each layer (the game's view kick and its gun kick), picked at
//! random among the gun's kicks for how the shot was fired: from the hip or aimed (shouldered, or
//! the sights at the eye), standing or crouched, and how far into a burst. View kicks run for their
//! own length and the ones of a burst overlap and add up, so a burst holds the gun up and shakes it
//! while you fire and it settles when you stop: you pull it back down. A gun kick (an animation)
//! replaces the one playing. The head never moves: what on a
//! flat screen is the camera's jolt is the gun's here.
//!
//! The game's numbers are for the way its player holds a gun: a long gun in both hands with the
//! stock at the shoulder (on a flat screen there is no other way). Held any other way it kicks
//! harder, by the hold's scale ([`HoldScale`]): one-handed much harder, off the shoulder harder.
//!
//! The runtime's own recoil ([`super::recoil`]) is the fallback, for guns without kicks or when the
//! game's recoil is turned off.

use glam::{Quat, Vec3};
use rustvr_protocol::*;

/// Shots this close together are one burst.
const BURST_GAP: f32 = 0.25;
/// A burst's shots from this one on take the long-burst kicks. Metro's configs mark kicks for the
/// first shot, the next ones and long bursts but not where a long burst starts (our pick):
/// half a second of AK fire.
const LONG_FROM: u32 = 6;
/// Kicks playing at once (a long burst of a fast gun runs about a dozen of each layer).
const MAX_ACTIVE: usize = 48;

/// How a shot was fired.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stance {
    pub aimed: bool,
    pub crouched: bool,
}

/// How much harder than the game's a shot kicks for how the gun's held: its turn and its push.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HoldScale {
    pub turn: f32,
    pub push: f32,
}

impl HoldScale {
    /// As the game's player holds it.
    pub const GAME: HoldScale = HoldScale { turn: 1.0, push: 1.0 };
}

impl Default for HoldScale {
    fn default() -> Self {
        HoldScale::GAME
    }
}

#[derive(Clone, Debug)]
pub struct GameRecoil {
    /// Playing: (index into the gun's kicks, authored time, how hard it was held to kick).
    active: Vec<(usize, f32, HoldScale)>,
    shot: u32,
    since_shot: f32,
    rng: u32,
}

impl Default for GameRecoil {
    fn default() -> Self {
        GameRecoil::new(0x2545_F491)
    }
}

impl GameRecoil {
    pub fn new(seed: u32) -> GameRecoil {
        GameRecoil { active: Vec::new(), shot: 0, since_shot: 10.0, rng: seed | 1 }
    }

    fn rand(&mut self, n: usize) -> usize {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        ((x >> 8) as usize) % n.max(1)
    }

    /// Consecutive shots in the current burst.
    pub fn burst(&self) -> u32 {
        self.shot
    }

    /// Kicks playing.
    pub fn playing(&self) -> usize {
        self.active.len()
    }

    /// One shot: starts a kick of each layer, `hold` times as hard as the game's.
    pub fn kick(&mut self, kicks: &[KickDesc], stance: Stance, hold: HoldScale) {
        if self.since_shot > BURST_GAP {
            self.shot = 0;
        }
        self.shot += 1;
        self.since_shot = 0.0;
        let stage = match self.shot {
            1 => KICK_FIRST,
            n if n >= LONG_FROM => KICK_LONG,
            _ => KICK_FOLLOW,
        };
        let aim = if stance.aimed { KICK_AIMED } else { KICK_HIP };
        let pose = if stance.crouched { KICK_CROUCHED } else { KICK_STANDING };
        for layer in [KICK_LAYER_VIEW, KICK_LAYER_GUN] {
            // The best match, relaxing the stance, then the aim, then the burst stage when the gun has
            // no kick for exactly how it was fired.
            let wants = [aim | pose | stage, aim | stage, stage, 0];
            let Some(found) = wants.iter().find_map(|&want| {
                let pick: Vec<usize> = (0..kicks.len()).filter(|&i| kicks[i].layer == layer && kicks[i].when & want == want).collect();
                (!pick.is_empty()).then_some(pick)
            }) else {
                continue;
            };
            let i = found[self.rand(found.len())];
            // A gun kick is an animation: the new one takes over.
            if layer == KICK_LAYER_GUN {
                self.active.retain(|&(j, _, _)| kicks.get(j).is_none_or(|k| k.layer != KICK_LAYER_GUN));
            }
            if self.active.len() >= MAX_ACTIVE {
                self.active.remove(0);
            }
            self.active.push((i, 0.0, hold));
        }
    }

    pub fn update(&mut self, dt: f32, kicks: &[KickDesc]) {
        let dt = dt.clamp(0.0, 0.1);
        self.since_shot += dt;
        for (i, t, _) in &mut self.active {
            *t += dt * kicks.get(*i).map_or(1.0, |k| if k.speed > 0.0 { k.speed } else { 1.0 });
        }
        self.active.retain(|&(i, t, _)| kicks.get(i).is_some_and(|k| t <= end(k)));
    }

    /// The offset of every kick playing, in weapon space: (rotation, translation), `x' = r * x + t`.
    /// `turn` and `push` scale the rotations and the translations.
    pub fn transform(&self, kicks: &[KickDesc], turn: f32, push: f32) -> (Quat, Vec3) {
        let (mut r, mut t) = (Quat::IDENTITY, Vec3::ZERO);
        for &(i, at, hold) in &self.active {
            let Some(k) = kicks.get(i) else { continue };
            let (turn, push) = (turn * hold.turn, push * hold.push);
            let c = |ch: usize| k.channels[ch].value(at);
            let q = Quat::from_rotation_y(c(KICK_YAW) * turn) * Quat::from_rotation_x(-c(KICK_PITCH) * turn) * Quat::from_rotation_z(c(KICK_ROLL) * turn);
            let pivot = Vec3::from_array(k.pivot);
            let move_by = Vec3::new(c(KICK_X), c(KICK_Y), c(KICK_Z)) * push;
            // This kick after the ones before it: x -> q * (r x + t - pivot) + pivot + move_by.
            t = q * (t - pivot) + pivot + move_by;
            r = (q * r).normalize();
        }
        (r, t)
    }
}

/// When a kick is over: its length, or its last key if that's later.
fn end(k: &KickDesc) -> f32 {
    k.channels.iter().filter_map(|c| c.keys().last().map(|k| k[0])).fold(k.length, f32::max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;

    /// A Metro-like kick: `pitch` (deg) and `back` (m) at once, gone linearly by `len` seconds.
    fn kick(layer: u32, when: u32, pitch: f32, back: f32, len: f32) -> KickDesc {
        let mut k = KickDesc::zeroed();
        k.object = 1;
        k.layer = layer;
        k.when = when;
        k.speed = 1.0;
        k.length = len;
        let line = |c: &mut KickCurve, v: f32| {
            c.count = 2;
            c.keys[0] = [0.02, v, -v / (len - 0.02), -v / (len - 0.02)];
            c.keys[1] = [len, 0.0, -v / (len - 0.02), -v / (len - 0.02)];
        };
        line(&mut k.channels[KICK_PITCH], pitch.to_radians());
        line(&mut k.channels[KICK_Z], -back);
        k
    }

    const ANY_STAGE: u32 = KICK_FIRST | KICK_FOLLOW | KICK_LONG;
    const ANY_STANCE: u32 = KICK_HIP | KICK_AIMED | KICK_STANDING | KICK_CROUCHED;

    fn gun() -> Vec<KickDesc> {
        vec![
            kick(KICK_LAYER_VIEW, KICK_HIP | KICK_STANDING | KICK_FIRST, 1.0, 0.035, 0.27),
            kick(KICK_LAYER_VIEW, KICK_HIP | KICK_STANDING | KICK_FOLLOW, 1.1, 0.03, 0.67),
            kick(KICK_LAYER_VIEW, KICK_HIP | KICK_STANDING | KICK_LONG, 1.5, 0.03, 0.67),
            kick(KICK_LAYER_VIEW, KICK_AIMED | KICK_STANDING | ANY_STAGE, 0.8, 0.05, 0.27),
            kick(KICK_LAYER_GUN, KICK_HIP | KICK_STANDING | KICK_CROUCHED | ANY_STAGE, 2.0, 0.017, 0.11),
        ]
    }

    fn pitch_deg(r: Quat) -> f32 {
        let f = r * Vec3::Z;
        f.y.atan2(f.z).to_degrees()
    }

    /// Fires `shots` at `rate`, returns the muzzle pitch (deg) every 1/90 s for `seconds`.
    fn fire(kicks: &[KickDesc], stance: Stance, shots: u32, rate: f32, seconds: f32) -> Vec<f32> {
        let mut g = GameRecoil::new(3);
        let (dt, mut t, mut next, mut fired, mut out) = (1.0 / 90.0, 0.0f32, 0.0f32, 0, Vec::new());
        while t < seconds {
            if fired < shots && t >= next {
                g.kick(kicks, stance, HoldScale::GAME);
                fired += 1;
                next += 1.0 / rate;
            }
            g.update(dt, kicks);
            out.push(pitch_deg(g.transform(kicks, 1.0, 1.0).0));
            t += dt;
        }
        out
    }

    #[test]
    fn a_shot_plays_one_kick_of_each_layer_and_both_add_up() {
        let k = gun();
        let mut g = GameRecoil::new(1);
        g.kick(&k, Stance::default(), HoldScale::GAME);
        assert_eq!(g.playing(), 2);
        // Another shot: a second view kick on top, the gun kick restarted.
        let mut g2 = g.clone();
        g2.kick(&k, Stance::default(), HoldScale::GAME);
        assert_eq!(g2.playing(), 3);
        g.update(0.02, &k);
        let (r, t) = g.transform(&k, 1.0, 1.0);
        assert!((pitch_deg(r) - 3.0).abs() < 0.15, "the view's 1 degree and the gun's 2: {}", pitch_deg(r));
        assert!(t.z < -0.045, "pushed back by both: {t:?}");
    }

    #[test]
    fn it_springs_back_by_itself() {
        let p = fire(&gun(), Stance::default(), 1, 1.0, 1.0);
        assert!(p.iter().cloned().fold(0.0, f32::max) > 2.5);
        assert!(p.last().unwrap().abs() < 1e-3, "back where it was: {}", p.last().unwrap());
    }

    #[test]
    fn a_burst_holds_the_gun_up_and_long_bursts_kick_harder() {
        let k = gun();
        let p = fire(&k, Stance::default(), 12, 10.0, 2.0);
        let at = |s: f32| p[(s * 90.0) as usize];
        // Overlapping kicks hold the muzzle up while firing, well above one shot's.
        assert!(at(0.45) > 4.0, "half a second in: {:.1}", at(0.45));
        assert!(at(1.05) > at(0.45), "long-burst kicks are bigger: {:.1} -> {:.1}", at(0.45), at(1.05));
        assert!(at(1.95).abs() < 0.05, "and it settles when you stop: {:.2}", at(1.95));
        // A long burst stays in its plateau, it doesn't climb forever.
        assert!(at(1.15) < 15.0, "{:.1}", at(1.15));
    }

    #[test]
    fn the_stance_picks_the_kicks() {
        let k = gun();
        let mut g = GameRecoil::new(1);
        g.kick(&k, Stance { aimed: true, crouched: false }, HoldScale::GAME);
        assert!(g.active.iter().any(|&(i, _, _)| i == 3), "aimed: the aimed view kick");
        // Crouched and aimed: no exact match, so the aimed one regardless of stance.
        let mut g = GameRecoil::new(1);
        g.kick(&k, Stance { aimed: true, crouched: true }, HoldScale::GAME);
        assert!(g.active.iter().any(|&(i, _, _)| i == 3));
        // A gun with only "any shot" kicks still kicks.
        let any = vec![kick(KICK_LAYER_GUN, ANY_STANCE | ANY_STAGE, 2.0, 0.01, 0.1)];
        let mut g = GameRecoil::new(1);
        g.kick(&any, Stance { aimed: true, crouched: true }, HoldScale::GAME);
        assert_eq!(g.playing(), 1);
    }

    #[test]
    fn a_harder_hold_kicks_harder() {
        let k = gun();
        let peak = |hold: HoldScale| {
            let mut g = GameRecoil::new(5);
            g.kick(&k, Stance::default(), hold);
            g.update(0.02, &k);
            pitch_deg(g.transform(&k, 1.0, 1.0).0)
        };
        let game = peak(HoldScale::GAME);
        let one_hand = peak(HoldScale { turn: 3.0, push: 1.6 });
        assert!((one_hand - 3.0 * game).abs() < 0.1, "{game} {one_hand}");
    }

    #[test]
    fn kicks_turn_about_their_pivot_and_scale() {
        let mut k = gun();
        k.truncate(1);
        k[0].pivot = [0.0, 0.1, -0.3];
        let mut g = GameRecoil::new(1);
        g.kick(&k, Stance::default(), HoldScale::GAME);
        g.update(0.02, &k);
        let (r, t) = g.transform(&k, 1.0, 0.0);
        let p = Vec3::from_array(k[0].pivot);
        assert!((r * p + t - p).length() < 1e-5, "the pivot stays put");
        let (r2, _) = g.transform(&k, 0.5, 0.0);
        assert!((pitch_deg(r2) - pitch_deg(r) * 0.5).abs() < 0.01);
    }
}
