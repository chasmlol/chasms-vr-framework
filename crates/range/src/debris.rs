//! The pieces of a thing broken (`CE_BROKE`): the range's own, a handful of chunks of its colour
//! flying apart with its speed, tumbling, knocking about the room's floor and tables, and gone after
//! a while. (They're only to look at, and nothing else meets them.)

use glam::{Mat4, Quat, Vec3};

use crate::hits;

/// How long the pieces lie about (s).
const PIECES_FOR: f64 = 6.0;
/// What a piece keeps of its speed off the floor: into it, and along it.
const BOUNCE: f32 = 0.3;
const SCRAPE: f32 = 0.6;

struct Piece {
    at: Vec3,
    vel: Vec3,
    rot: Quat,
    spin: Vec3,
    size: Vec3,
    color: [f32; 4],
    until: f64,
}

#[derive(Default)]
pub struct Debris {
    pieces: Vec<Piece>,
    seed: u32,
}

impl Debris {
    fn rand(&mut self) -> f32 {
        self.seed = self.seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.seed >> 8) as f32 / (1u32 << 24) as f32
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.rand()
    }

    /// A thing `half` (half extents) of `color` broken at `at`, knocked along `dir` at `speed`: it
    /// falls apart into some eight to twenty pieces, each going on a share of the knock's way and
    /// flung a little out from where it was in the thing.
    pub fn burst(&mut self, at: Vec3, half: Vec3, color: [f32; 4], dir: Vec3, speed: f32, now: f64) {
        let size = half.length().clamp(0.03, 0.5);
        let n = (8.0 + 25.0 * size).min(20.0) as usize;
        for _ in 0..n {
            let off = Vec3::new(self.range(-half.x, half.x), self.range(-half.y, half.y), self.range(-half.z, half.z));
            let out = off.normalize_or(Vec3::Y);
            let vel = dir * speed * self.range(0.15, 0.45) + out * self.range(0.5, 2.0) + Vec3::Y * self.range(0.0, 1.0);
            let s = size * self.range(0.25, 0.5);
            let shade = self.range(0.7, 1.0);
            let spin = Vec3::new(self.range(-1.0, 1.0), self.range(-1.0, 1.0), self.range(-1.0, 1.0)) * 15.0;
            let piece = Piece {
                at: at + off,
                vel,
                rot: Quat::from_scaled_axis(spin * 0.1),
                spin,
                size: Vec3::new(s, s * self.range(0.3, 0.8), s * self.range(0.5, 1.0)),
                color: [color[0] * shade, color[1] * shade, color[2] * shade, 1.0],
                until: now + PIECES_FOR + self.range(0.0, 2.0) as f64,
            };
            self.pieces.push(piece);
        }
    }

    /// One step: they fall, tumble, and knock off the room's floor and tables.
    pub fn step(&mut self, dt: f32, now: f64, world: &hits::World) {
        self.pieces.retain(|p| p.until > now);
        for p in &mut self.pieces {
            p.vel.y -= 9.81 * dt;
            p.at += p.vel * dt;
            p.rot = (Quat::from_scaled_axis(p.spin * dt) * p.rot).normalize();
            let r = p.size.min_element() * 0.5;
            let hit = world.collide(p.at, r).map(|(push, n, _)| (push, n)).or_else(|| (p.at.y < r).then_some((Vec3::Y * (r - p.at.y), Vec3::Y)));
            if let Some((push, n)) = hit {
                p.at += push;
                let into = p.vel.dot(n);
                if into < 0.0 {
                    let along = p.vel - n * into;
                    p.vel = along * SCRAPE - n * into * BOUNCE;
                    p.spin *= SCRAPE;
                }
            }
        }
    }

    /// Each piece, as a box to draw (a unit cube's model, and its colour).
    pub fn draws(&self) -> impl Iterator<Item = (Mat4, [f32; 4])> + '_ {
        self.pieces.iter().map(|p| (Mat4::from_scale_rotation_translation(p.size, p.rot, p.at), p.color))
    }
}
