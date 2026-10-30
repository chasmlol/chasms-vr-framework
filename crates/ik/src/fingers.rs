//! Finger curls from controller buttons and touch sensors, and hold shapes for held objects.
//!
//! Touch controllers report the trigger (analog plus capacitive touch), the grip (analog) and
//! thumb contact on the face buttons, stick and thumbrest. That is enough for the poses players
//! read: thumbs up (thumb lifted), a fist (squeeze), and resting fingers on the controls. The index
//! rests as it does on the trigger whether it's there or not, curling only as the trigger is
//! pulled: a player found it lifting off by itself when the finger left the trigger unnatural, and
//! its pose jumping as the finger touched it, so the trigger's touch sensor doesn't move it. While
//! holding something, the fingers wrap to its shape instead, except that a gun's trigger finger
//! still follows the trigger.

use rustvr_protocol::Fingers;

use crate::math::{clamp_dt, smoothing};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FingerInput {
    /// Trigger pull, 0..1.
    pub trigger: f32,
    /// Grip squeeze, 0..1.
    pub squeeze: f32,
    /// The index finger is on the trigger (its touch sensor). The pose doesn't follow it: the index
    /// rests as on the trigger either way.
    pub touch_trigger: bool,
    /// The thumb touches a face button, the stick or the thumbrest.
    pub touch_thumb: bool,
    /// The thumb presses a face button or the stick.
    pub thumb_press: bool,
    /// The fingers as the headset's cameras see them (a hand tracked without its controller):
    /// curls, thumb .. little, which an empty hand takes as they are.
    pub tracked: Option<[f32; 5]>,
}

impl FingerInput {
    /// What a controller's trigger, grip and touch sensors say of the fingers.
    pub fn from_controller(c: &rustvr_protocol::Controller) -> Self {
        use rustvr_protocol::*;
        let touch_thumb = c.buttons & (CTRL_TOUCH_A | CTRL_TOUCH_B | CTRL_TOUCH_STICK | CTRL_TOUCH_REST) != 0;
        let thumb_press = c.buttons & (CTRL_A | CTRL_B | CTRL_STICK_CLICK) != 0;
        FingerInput {
            trigger: c.trigger,
            squeeze: c.squeeze,
            touch_trigger: c.buttons & CTRL_TOUCH_TRIGGER != 0 || c.trigger > 0.02,
            touch_thumb: touch_thumb || thumb_press,
            thumb_press,
            tracked: None,
        }
    }

    /// [`FingerInput::from_controller`], with the fingers as the headset tracks them when it does.
    pub fn from_hand(c: &rustvr_protocol::Controller, tracked: Option<[f32; 5]>) -> Self {
        FingerInput { tracked, ..Self::from_controller(c) }
    }
}

/// How the fingers wrap a held object.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum HoldShape {
    #[default]
    None,
    /// A gun's pistol grip: middle, ring and pinky wrapped, thumb round the far side, index on the
    /// trigger.
    PistolGrip,
    /// A vertical foregrip or handguard: all four fingers wrapped.
    Foregrip,
    /// A magazine: a thick box held loosely.
    Magazine,
    /// A small object (grenade, small prop).
    Small,
    /// A large object, fingers spread round it.
    Large,
}

/// Curls (thumb, index, middle, ring, pinky) and splay a hold shape wraps the hand to (est.,
/// chosen by eye for each shape).
fn hold_pose(hold: HoldShape) -> Option<([f32; 5], f32)> {
    Some(match hold {
        HoldShape::None => return None,
        HoldShape::PistolGrip => ([0.75, 0.0, 0.85, 0.88, 0.9], 0.1),
        HoldShape::Foregrip => ([0.7, 0.8, 0.82, 0.85, 0.88], 0.05),
        HoldShape::Magazine => ([0.45, 0.55, 0.58, 0.6, 0.62], 0.15),
        HoldShape::Small => ([0.55, 0.65, 0.68, 0.7, 0.72], 0.3),
        HoldShape::Large => ([0.25, 0.3, 0.32, 0.34, 0.36], 0.6),
    })
}

/// Index curl for a finger resting on a trigger: relaxed, curling in as the trigger is pulled.
fn trigger_curl(input: &FingerInput, rest: f32, pulled: f32) -> f32 {
    rest + (pulled - rest) * unit(input.trigger)
}

/// The thumb's curl resting on a button, the stick or the thumbrest (a relaxed thumb), with the
/// fingers closed in a fist, the more for pressing, and lifted off (below 0: its base stands it up
/// away from the palm) (est., by eye).
const THUMB_REST: f32 = 0.12;
const THUMB_FIST: f32 = 0.75;
const THUMB_PRESS: f32 = 0.1;
const THUMB_LIFTED: f32 = -0.5;
/// The index's curl resting on the trigger (a relaxed finger) (est., by eye).
const INDEX_REST: f32 = 0.05;

/// Curl smoothing: a few frames, quick enough to read as 1:1 on the trigger and slow enough that
/// a touch sensor flipping or a grab starting doesn't snap the fingers (est.).
const CURL_TAU: f32 = 0.05;
/// Longest frame the smoothing integrates over (three 90 Hz frames).
const MAX_DT: f32 = 3.0 / 90.0;

/// An analog value in 0..1 (NaN reads as released).
fn unit(x: f32) -> f32 {
    if x.is_finite() { x.clamp(0.0, 1.0) } else { 0.0 }
}

#[derive(Clone, Copy, Debug)]
pub struct FingerSolver {
    curl: [f32; 5],
    splay: f32,
    started: bool,
}

impl Default for FingerSolver {
    fn default() -> Self {
        Self::new()
    }
}

impl FingerSolver {
    pub fn new() -> Self {
        FingerSolver { curl: [0.0; 5], splay: 0.0, started: false }
    }

    /// Smoothed finger curls for one hand. While holding, fingers wrap to the hold shape
    /// (the index finger still follows the trigger when holding a gun).
    pub fn update(&mut self, dt: f32, input: &FingerInput, hold: HoldShape) -> Fingers {
        let (target, splay) = Self::target(input, hold);
        // The first update starts at the pose; after that every change is smoothed.
        let a = if self.started { smoothing(clamp_dt(dt, MAX_DT), CURL_TAU) } else { 1.0 };
        self.started = true;
        for (c, t) in self.curl.iter_mut().zip(target) {
            *c += (t - *c) * a;
        }
        self.splay += (splay - self.splay) * a;
        Fingers { curl: self.curl, splay: self.splay, _pad: [0.0; 2] }
    }

    /// The curls (thumb .. little) and splay the fingers go to for `input` holding `hold`, before
    /// smoothing.
    pub fn target(input: &FingerInput, hold: HoldShape) -> ([f32; 5], f32) {
        let squeeze = unit(input.squeeze);
        // A tracked hand's fingers are where they are (holding nothing).
        if let (Some(t), HoldShape::None) = (input.tracked, hold) {
            let curl = t.map(unit);
            let open = 1.0 - (curl[1] + curl[2] + curl[3] + curl[4]) / 4.0;
            return (curl, 0.5 * open);
        }
        // A thumb resting on the controls is a relaxed thumb, closing with the fingers into a fist
        // and pressing a little further; lifted off, it stands up a little (all the way up in a
        // thumbs up).
        let thumb = if input.touch_thumb {
            THUMB_REST + (THUMB_FIST - THUMB_REST) * squeeze + if input.thumb_press { THUMB_PRESS } else { 0.0 }
        } else {
            THUMB_LIFTED
        };
        if let Some((mut curl, splay)) = hold_pose(hold) {
            if hold == HoldShape::PistolGrip {
                // The index rests on the trigger, curling as it's pulled.
                curl[1] = trigger_curl(input, 0.35, 0.8);
            }
            return (curl, splay);
        }
        // A relaxed finger, as on the trigger, curling in as it's pulled.
        let index = trigger_curl(input, INDEX_REST, 1.0);
        // The lower fingers close with the grip, the pinky a touch ahead of the middle finger.
        let grip = |lead: f32| (0.15 + 0.85 * squeeze * lead).min(1.0);
        let curl = [thumb, index, grip(1.0), grip(1.03), grip(1.06)];
        // An open hand relaxes its fingers apart.
        let open = 1.0 - (curl[1] + curl[2] + curl[3] + curl[4]) / 4.0;
        (curl, 0.5 * open)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settle(f: &mut FingerSolver, input: &FingerInput, hold: HoldShape) -> Fingers {
        let mut out = f.update(0.0, input, hold);
        for _ in 0..90 {
            out = f.update(1.0 / 90.0, input, hold);
        }
        out
    }

    #[test]
    fn open_hand_points_and_fists() {
        let mut f = FingerSolver::new();
        let open = settle(&mut f, &FingerInput::default(), HoldShape::None);
        assert!(open.curl.iter().all(|&c| c < 0.2), "{open:?}");
        assert!(open.splay > 0.3);
        // Squeezing without the trigger: the index stays relaxed, the others close.
        let point = settle(&mut f, &FingerInput { squeeze: 1.0, touch_thumb: true, ..Default::default() }, HoldShape::None);
        assert!(point.curl[1] < 0.1 && point.curl[2] > 0.95 && point.curl[0] > 0.5);
        // On the trigger or off it, the index rests the same.
        let touching = settle(&mut f, &FingerInput { squeeze: 1.0, touch_thumb: true, touch_trigger: true, ..Default::default() }, HoldShape::None);
        assert_eq!(touching.curl[1], point.curl[1]);
        // Fist: everything pressed.
        let fist = FingerInput { trigger: 1.0, squeeze: 1.0, touch_trigger: true, touch_thumb: true, thumb_press: true, tracked: None };
        let fist = settle(&mut f, &fist, HoldShape::None);
        assert!(fist.curl.iter().all(|&c| c > 0.8), "{fist:?}");
        // Thumbs up: squeeze and trigger, thumb lifted.
        let up = FingerInput { trigger: 1.0, squeeze: 1.0, touch_trigger: true, ..Default::default() };
        assert!(settle(&mut f, &up, HoldShape::None).curl[0] < 0.15);
    }

    #[test]
    fn gun_trigger_finger_follows_the_trigger() {
        let mut f = FingerSolver::new();
        let off = settle(&mut f, &FingerInput { squeeze: 1.0, ..Default::default() }, HoldShape::PistolGrip);
        let resting = settle(&mut f, &FingerInput { squeeze: 1.0, touch_trigger: true, ..Default::default() }, HoldShape::PistolGrip);
        let pulled = FingerInput { trigger: 1.0, squeeze: 1.0, touch_trigger: true, ..Default::default() };
        let pulled = settle(&mut f, &pulled, HoldShape::PistolGrip);
        assert!(off.curl[1] == resting.curl[1] && resting.curl[1] < pulled.curl[1]);
        // The other fingers stay wrapped whatever the trigger does.
        assert!((off.curl[3] - pulled.curl[3]).abs() < 1e-3 && pulled.curl[3] > 0.8);
        // A foregrip ignores the trigger.
        let a = settle(&mut f, &FingerInput::default(), HoldShape::Foregrip);
        let b = settle(&mut f, &FingerInput { trigger: 1.0, touch_trigger: true, ..Default::default() }, HoldShape::Foregrip);
        assert!((a.curl[1] - b.curl[1]).abs() < 1e-3);
    }

    #[test]
    fn curls_are_smoothed_and_finite() {
        let mut f = FingerSolver::new();
        f.update(1.0 / 90.0, &FingerInput::default(), HoldShape::None);
        let after_one = f.update(1.0 / 90.0, &FingerInput { squeeze: 1.0, ..Default::default() }, HoldShape::None);
        assert!(after_one.curl[2] > 0.15 && after_one.curl[2] < 0.9, "{after_one:?}");
        // dt = 0 changes nothing; a huge dt is clamped; NaN dt is ignored.
        let same = f.update(0.0, &FingerInput { squeeze: 1.0, ..Default::default() }, HoldShape::None);
        assert_eq!(same.curl, after_one.curl);
        let big = f.update(1e9, &FingerInput { squeeze: 1.0, ..Default::default() }, HoldShape::None);
        let nan = f.update(f32::NAN, &FingerInput { squeeze: f32::NAN, ..Default::default() }, HoldShape::Large);
        // (A lifted thumb curls back, below 0.)
        assert!(big.curl.iter().chain(nan.curl.iter()).all(|c| c.is_finite() && (-1.0..=1.0).contains(c)));
    }
}
