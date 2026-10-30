//! Bowls the hands make things in, and grinding in them ([`rustvr_protocol::Receptacle`]): a
//! mortar that keeps what's dropped into it and grinds it under its pestle, a pot, a cauldron.
//!
//! A bowl is a shape in its object's space: its inside floor's middle (`floor`), its mouth
//! straight up from there (the object's +Y), `radius` across inside and `depth` up to its rim, its
//! wall `wall` thick. Its collision is that shape ([`Bowl::parts`]: a base under the floor and a
//! ring of wall), so the hands and what they hold go into it; the game's own collision of it (a
//! convex hull, a lid over its mouth) would keep them out.
//!
//! What may go in, let go over its mouth (or falling into it), goes in and lies on its floor, the
//! first in the middle and the rest round it ([`Bowl::place`]), each shrunk to fit if it's bigger
//! than the bowl ([`fit`]). The bowl keeps it wherever it's carried, until it's turned over: then
//! what's in it falls out. A hand reaching into it takes what's in it. Its grinder (a pestle)
//! rests in it too, where the game's model has it, until a hand takes it, and goes back there when
//! let go over the mouth.
//!
//! **Grinding** ([`Grind`]): the grinder's tip pressed low in the bowl (within [`PRESS`] of its
//! floor, beyond the tip's own size) and worked about grinds what's in it by the distance the hand
//! takes the tip: round, back and forth, or pounding (a vertical stroke counts half). Rough circling
//! counts as well as neat circles, and thrashing no faster than [`MAX_SPEED`]. (The hand's, not the
//! tip's own: pressed into the floor, the tip is held there and shivers on it, which would grind of
//! itself.) Every [`STROKE`] of
//! grinding is a stroke: the game's sound, a pulse in the hand. What's ground shrinks as it is
//! ([`drawn_scale`]).

use glam::{Quat, Vec3};
use rustvr_protocol::Receptacle;

/// The pieces of wall round a bowl's collision.
pub const WALL_PIECES: usize = 12;
/// How far over the floor (beyond the tip's own size) the tip grinds: pressed in, not hovering.
pub const PRESS: f32 = 0.025;
/// The fastest grinding that counts (m/s): thrashing about grinds no faster.
pub const MAX_SPEED: f32 = 0.8;
/// Grinding (metres) that makes a stroke.
pub const STROKE: f32 = 0.09;
/// How far over the rim (metres) a thing let go over the mouth still goes in, and how far outside
/// the mouth its middle may be (let go over the rim, it tips in).
pub const MOUTH_ABOVE: f32 = 0.3;
pub const MOUTH_SLACK: f32 = 0.04;
/// A thing fully ground is drawn this big (of its size in the bowl).
pub const GROUND_SIZE: f32 = 0.45;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bowl {
    pub floor: Vec3,
    pub radius: f32,
    pub depth: f32,
    pub wall: f32,
}

impl Bowl {
    pub fn of(r: &Receptacle) -> Bowl {
        Bowl { floor: Vec3::from_array(r.floor), radius: r.radius.max(0.01), depth: r.depth.max(0.005), wall: r.wall.max(0.003) }
    }

    /// Whether a point (the bowl's object space) is in its mouth: within its radius (and `slack`)
    /// of its axis, from its floor up to `above` over its rim.
    pub fn in_mouth(&self, p: Vec3, above: f32, slack: f32) -> bool {
        let d = p - self.floor;
        let across = Vec3::new(d.x, 0.0, d.z).length();
        across <= self.radius + slack && d.y >= -0.01 && d.y <= self.depth + above
    }

    /// Where the `k`th of `n` things in it lies (the middle of its bottom, on the floor): alone in
    /// the middle, else spread round half way out, the first at the back.
    pub fn place(&self, k: usize, n: usize) -> Vec3 {
        if n <= 1 {
            return self.floor;
        }
        let a = std::f32::consts::TAU * k as f32 / n as f32;
        self.floor + Vec3::new(a.sin(), 0.0, a.cos()) * self.radius * 0.45
    }

    /// Its collision (object space): a base from `bottom` (the outside's lowest, along +Y) up to
    /// the floor, and [`WALL_PIECES`] boxes round its mouth up to the rim. Each a box's centre,
    /// rotation and half extents.
    pub fn parts(&self, bottom: f32) -> Vec<(Vec3, Quat, Vec3)> {
        let outer = self.radius + self.wall;
        let base_h = ((self.floor.y - bottom).max(self.wall) * 0.5).max(0.002);
        let mut out = vec![(Vec3::new(self.floor.x, self.floor.y - base_h, self.floor.z), Quat::IDENTITY, Vec3::new(outer, base_h, outer) * Vec3::new(0.75, 1.0, 0.75))];
        let half_chord = (self.radius + self.wall * 0.5) * (std::f32::consts::PI / WALL_PIECES as f32).tan() + 0.002;
        for k in 0..WALL_PIECES {
            let a = std::f32::consts::TAU * k as f32 / WALL_PIECES as f32;
            let dir = Vec3::new(a.sin(), 0.0, a.cos());
            let centre = self.floor + dir * (self.radius + self.wall * 0.5) + Vec3::Y * (self.depth * 0.5);
            out.push((centre, Quat::from_rotation_y(a), Vec3::new(half_chord, self.depth * 0.5, self.wall * 0.5)));
        }
        out
    }
}

/// How much a thing (its box's half extents, object space) is shrunk to lie in a bowl: as much as
/// keeps it inside the mouth (1: it fits).
pub fn fit(bowl: &Bowl, half: Vec3) -> f32 {
    let across = half.x.max(half.z).max(half.y).max(1e-3);
    (bowl.radius * 0.8 / across).min(1.0)
}

/// How big a thing in a bowl is drawn: shrunk to fit, and smaller as it's ground (`ground` 0..1).
pub fn drawn_scale(fit: f32, ground: f32) -> f32 {
    fit * (1.0 - (1.0 - GROUND_SIZE) * ground.clamp(0.0, 1.0))
}

/// One bowl's grinding: where its grinder's tip was, how far it's gone since its last stroke.
#[derive(Clone, Copy, Debug, Default)]
pub struct Grind {
    last: Option<Vec3>,
    travel: f32,
    pub strokes: u32,
    /// How fast it's grinding now (m/s of counted grinding), for what the hand feels.
    pub speed: f32,
}

impl Grind {
    /// One step: the grinder's tip (the bowl's object space; None: no hand has it), where the hand
    /// would have it (the same space), and its size. The grinding done (metres), and whether that
    /// finished a stroke.
    pub fn step(&mut self, bowl: &Bowl, tip: Option<Vec3>, aimed: Option<Vec3>, tip_radius: f32, dt: f32) -> (f32, bool) {
        let low = tip.filter(|t| {
            let d = *t - bowl.floor;
            Vec3::new(d.x, 0.0, d.z).length() <= bowl.radius + tip_radius && d.y <= tip_radius + PRESS && d.y >= -0.05
        });
        let low = low.and(aimed.or(low));
        let done = match (self.last, low) {
            (Some(a), Some(b)) => {
                let d = b - a;
                (Vec3::new(d.x, 0.0, d.z).length() + 0.5 * d.y.abs()).min(MAX_SPEED * dt.max(1e-4))
            }
            _ => 0.0,
        };
        self.last = low;
        self.speed = done / dt.max(1e-4);
        self.travel += done;
        let stroke = self.travel >= STROKE;
        if stroke {
            self.travel -= STROKE;
            self.strokes = self.strokes.wrapping_add(1);
        }
        (done, stroke)
    }
}
