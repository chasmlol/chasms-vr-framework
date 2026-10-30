//! Sheets the fingers touch ([`rustvr_protocol::OBJF_SHEET`]): a page of a game's own, its card
//! drawn on the face of a thin box, lying on a table or held in a hand. The index fingertip of an
//! empty hand tapping its face is a touch: one each time it comes down and lifts off again, where
//! on the card it came down, for the game to read as it reads a click on its own page.
//!
//! The face is the box's +Y, the card on it with its top towards +Z and its left towards −X. The
//! fingertip is the finger's end. The hands go through a lying sheet
//! ([`super::physics::groups_sheet`]), so on a table a fingertip stops on the table, through the
//! page: within [`PRESS`] over the face, or as far through it as [`THROUGH`], it's down (felt);
//! lifted [`LIFT`] off it again, that's the touch. A hand coming down on the page to take it up
//! has its fingers on it too: taken (or slid off the page's edge) before it lifts, there's no
//! touch, so picking the page up never chooses what's written where the finger landed.

use glam::Vec3;

/// How near the face (metres) a fingertip is down on it, and how far through it.
pub const PRESS: f32 = 0.012;
pub const THROUGH: f32 = 0.008;
/// How far off the face a fingertip down on it lifts to touch it.
pub const LIFT: f32 = 0.025;
/// Off the face's edge by this much (metres), a fingertip down on it has slid off.
const EDGE: f32 = 0.01;

/// Where a fingertip at `tip` (the sheet's own space, metres) is over the face of a sheet of half
/// sizes `half`, across and down its card (0 .. 1), and how far over the face it is; None when
/// it's past the face's edges by more than `edge`, or further through the page than [`THROUGH`].
pub fn over(tip: Vec3, half: Vec3, edge: f32) -> Option<(f32, f32, f32)> {
    let (hx, hz) = (half.x.max(1e-4), half.z.max(1e-4));
    if tip.y < half.y - THROUGH || tip.x.abs() > hx + edge || tip.z.abs() > hz + edge {
        return None;
    }
    let across = ((tip.x + hx) / (2.0 * hx)).clamp(0.0, 1.0);
    let down = ((hz - tip.z) / (2.0 * hz)).clamp(0.0, 1.0);
    Some((across, down, tip.y - half.y))
}

/// What a hand's fingertip did on a sheet this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tap {
    None,
    /// Came down on its face.
    Down,
    /// Lifted off again: a touch.
    Touch,
}

/// A sheet's touches: how many so far, where the last was, and where each hand's fingertip came
/// down on it (while it's down).
#[derive(Clone, Copy, Debug, Default)]
pub struct Touches {
    pub count: u32,
    pub at: (f32, f32),
    down: [Option<(f32, f32)>; 2],
}

impl Touches {
    /// Hand `hand`'s fingertip this frame (the sheet's space; None: it can't touch, holding
    /// something or not there).
    pub fn step(&mut self, hand: usize, tip: Option<Vec3>, half: Vec3) -> Tap {
        let Some(tip) = tip else {
            self.down[hand] = None;
            return Tap::None;
        };
        match (self.down[hand], over(tip, half, if self.down[hand].is_some() { EDGE } else { 0.0 })) {
            (None, Some((across, down, height))) if height < PRESS => {
                self.down[hand] = Some((across, down));
                Tap::Down
            }
            (Some(at), Some((_, _, height))) if height >= LIFT => {
                self.down[hand] = None;
                self.count += 1;
                self.at = at;
                Tap::Touch
            }
            (Some(_), None) => {
                self.down[hand] = None;
                Tap::None
            }
            _ => Tap::None,
        }
    }
}
