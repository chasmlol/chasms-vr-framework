//! What a bullet (or a bouncing casing) meets in the range: the room's boxes, the target plates and
//! the objects the runtime is simulating, each with the kind of surface it is (for the sparks,
//! dust and holes it gets); and, from the creatures' own world, their bodies and what they hold.

use glam::{Quat, Vec3};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    /// Floor, walls, ceiling.
    Stone,
    Wood,
    Metal,
    /// A steel target plate.
    Target,
    /// A creature's flesh (or what covers it that a round goes through).
    Flesh,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HitWhat {
    World(Surface),
    Target(usize),
    /// An object the runtime simulates (its id), and what it's made of.
    Object(u32, Surface),
    /// One of the creatures (its id: a person's, or what it holds), and what it's made of there.
    Body(u32, Surface),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub pos: Vec3,
    pub normal: Vec3,
    pub dist: f32,
    pub what: HitWhat,
}

impl Hit {
    pub fn surface(&self) -> Surface {
        match self.what {
            HitWhat::World(s) | HitWhat::Object(_, s) | HitWhat::Body(_, s) => s,
            HitWhat::Target(_) => Surface::Target,
        }
    }
}

/// A loose object as a box: where it is, and its collider.
#[derive(Clone, Copy, Debug)]
pub struct ObjBox {
    pub id: u32,
    pub pos: Vec3,
    pub rot: Quat,
    pub center: Vec3,
    pub half: Vec3,
    pub surface: Surface,
}

#[derive(Clone, Debug, Default)]
pub struct World {
    boxes: Vec<(Vec3, Vec3, Surface)>,
    /// Target plates: centre and radius (facing -Z, a couple of centimetres thick).
    targets: Vec<(Vec3, f32)>,
}

/// Slab test of a ray against an axis-aligned box: (distance, normal).
fn ray_aabb(o: Vec3, d: Vec3, lo: Vec3, hi: Vec3, max: f32) -> Option<(f32, Vec3)> {
    let (mut t0, mut t1) = (0.0f32, max);
    let mut normal = Vec3::ZERO;
    for a in 0..3 {
        if d[a].abs() < 1e-8 {
            if o[a] < lo[a] || o[a] > hi[a] {
                return None;
            }
            continue;
        }
        let inv = 1.0 / d[a];
        let (mut near, mut far) = ((lo[a] - o[a]) * inv, (hi[a] - o[a]) * inv);
        let mut n = Vec3::ZERO;
        n[a] = -d[a].signum();
        if near > far {
            std::mem::swap(&mut near, &mut far);
        }
        if near > t0 {
            t0 = near;
            normal = n;
        }
        t1 = t1.min(far);
        if t0 > t1 {
            return None;
        }
    }
    (normal != Vec3::ZERO).then_some((t0, normal))
}

impl World {
    /// `boxes`: (min, max, surface) for the shell and the furniture; `targets`: centre and radius.
    pub fn new(boxes: Vec<(Vec3, Vec3, Surface)>, targets: Vec<(Vec3, f32)>) -> World {
        World { boxes, targets }
    }

    /// The nearest thing along a ray (unit `dir`) within `max` metres.
    pub fn raycast(&self, origin: Vec3, dir: Vec3, max: f32, objects: &[ObjBox]) -> Option<Hit> {
        let mut best: Option<Hit> = None;
        let mut consider = |h: Hit| {
            if best.is_none_or(|b| h.dist < b.dist) {
                best = Some(h);
            }
        };
        for (lo, hi, s) in &self.boxes {
            if let Some((t, n)) = ray_aabb(origin, dir, *lo, *hi, max) {
                consider(Hit { pos: origin + dir * t, normal: n, dist: t, what: HitWhat::World(*s) });
            }
        }
        for (i, (c, r)) in self.targets.iter().enumerate() {
            let (lo, hi) = (*c - Vec3::new(*r, *r, 0.0), *c + Vec3::new(*r, *r, 0.02));
            if let Some((t, n)) = ray_aabb(origin, dir, lo, hi, max) {
                let p = origin + dir * t;
                if (p - *c).truncate().length() <= *r {
                    consider(Hit { pos: p, normal: n, dist: t, what: HitWhat::Target(i) });
                }
            }
        }
        for o in objects {
            // In the box's own frame.
            let inv = o.rot.inverse();
            let (lo, hi) = (o.center - o.half, o.center + o.half);
            if let Some((t, n)) = ray_aabb(inv * (origin - o.pos), inv * dir, lo, hi, max) {
                consider(Hit { pos: origin + dir * t, normal: o.rot * n, dist: t, what: HitWhat::Object(o.id, o.surface) });
            }
        }
        best
    }

    /// A small sphere (`r`) at `p` pushed out of the room's boxes: the push and the surface's
    /// normal, if it's in one.
    pub fn collide(&self, p: Vec3, r: f32) -> Option<(Vec3, Vec3, Surface)> {
        let mut out: Option<(Vec3, Vec3, Surface)> = None;
        for (lo, hi, s) in &self.boxes {
            let q = p.clamp(*lo, *hi);
            let d = p - q;
            let inside = d == Vec3::ZERO;
            if !inside && d.length() >= r {
                continue;
            }
            // Out the nearest face (inside), or along the separation (touching).
            let (n, depth) = if inside {
                let faces = [(p.x - lo.x, -Vec3::X), (hi.x - p.x, Vec3::X), (p.y - lo.y, -Vec3::Y), (hi.y - p.y, Vec3::Y), (p.z - lo.z, -Vec3::Z), (hi.z - p.z, Vec3::Z)];
                let (d, n) = faces.into_iter().min_by(|a, b| a.0.total_cmp(&b.0)).unwrap();
                (n, d + r)
            } else {
                (d.normalize(), r - d.length())
            };
            if out.is_none_or(|o| depth > o.0.length()) {
                out = Some((n * depth, n, *s));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room() -> World {
        World::new(
            vec![(Vec3::new(-10.0, -0.2, -5.0), Vec3::new(10.0, 0.0, 25.0), Surface::Stone), (Vec3::new(0.4, 0.0, 0.3), Vec3::new(1.5, 0.85, 1.4), Surface::Wood)],
            vec![(Vec3::new(0.0, 1.6, 10.0), 0.35)],
        )
    }

    #[test]
    fn rays_hit_the_nearest_thing_with_its_surface() {
        let w = room();
        let h = w.raycast(Vec3::new(0.0, 1.6, 0.0), Vec3::Z, 100.0, &[]).unwrap();
        assert_eq!(h.what, HitWhat::Target(0));
        assert!((h.pos.z - 10.0).abs() < 1e-4 && h.normal == -Vec3::Z);
        // Down onto the table.
        let h = w.raycast(Vec3::new(1.0, 1.5, 1.0), -Vec3::Y, 10.0, &[]).unwrap();
        assert_eq!(h.surface(), Surface::Wood);
        assert!((h.pos.y - 0.85).abs() < 1e-4 && h.normal == Vec3::Y);
        // A can in the way.
        let can = ObjBox { id: 7, pos: Vec3::new(0.0, 1.6, 5.0), rot: Quat::from_rotation_y(0.4), center: Vec3::ZERO, half: Vec3::splat(0.04), surface: Surface::Metal };
        let h = w.raycast(Vec3::new(0.0, 1.6, 0.0), Vec3::Z, 100.0, &[can]).unwrap();
        assert_eq!(h.what, HitWhat::Object(7, Surface::Metal));
        assert!(h.dist < 5.0 && h.dist > 4.9);
        // Past the plate's edge it goes on to the floor... or nothing.
        assert!(w.raycast(Vec3::new(2.0, 1.6, 0.0), Vec3::Z, 100.0, &[]).is_none());
    }

    #[test]
    fn small_things_are_pushed_out_of_the_floor_and_tables() {
        let w = room();
        let (push, n, s) = w.collide(Vec3::new(0.0, -0.002, 2.0), 0.005).unwrap();
        assert_eq!((n, s), (Vec3::Y, Surface::Stone));
        assert!((push.y - 0.007).abs() < 1e-4);
        assert!(w.collide(Vec3::new(0.0, 0.5, 2.0), 0.005).is_none());
        let (_, n, s) = w.collide(Vec3::new(1.0, 0.851, 1.0), 0.005).unwrap();
        assert_eq!((n, s), (Vec3::Y, Surface::Wood));
    }
}
