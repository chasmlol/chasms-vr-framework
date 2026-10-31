//! Climbing with the hands (`docs/MELEE.md`). An empty hand gripping the static world where there's
//! something to close on holds there: an edge within a few centimetres of the palm (it grips only
//! where there's something to close the fingers on, so a flat wall can't be held), or the top of a
//! ledge pressed on (a palm pushing down on a top holds as a hand on its edge would). The hand
//! stays where it took hold, whatever the controller does; letting go of the grip lets go.
//!
//! The player's body hangs from the holding hands on the player's joint: a spring of 5000 N/m and a
//! damper of 300 N s/m, at most 2000 N a hand, from where the controller is to where the hand
//! holds, on a 70 kg body with its weight. So pulling the controllers down lifts the body (one hand
//! holds the 687 N of it); stretched further than an arm's length the hand lets go. The runtime
//! works out how the arms move the player ([`RtMelee::climb`]) and the game moves them so, without
//! its own gravity, while any hand holds ([`RTM_CLIMBING`]). What the game did with it (a floor in
//! the way) is what the body goes on from: its velocity is the player's own, as the game moved
//! them, plus what the arms and its weight add.

use glam::{Quat, Vec3};

use super::Rig;

/// The player's joint on a static grip (N/m, N s/m, N a hand), the body it holds (kg), and how far
/// the controller may be from the hand holding before it lets go (m: an arm's length).
pub const CLIMB_SPRING: f32 = 5000.0;
pub const CLIMB_DAMPER: f32 = 300.0;
pub const CLIMB_MAX_FORCE: f32 = 2000.0;
pub const CLIMB_BODY: f32 = 70.0;
pub const CLIMB_STRETCH: f32 = 0.75;
/// How long the game's moving of the player is averaged over to see whether it held the body back
/// (s).
const CLIMB_SEEN: f32 = 0.1;
/// A hand holds the world this near its palm (m), where an edge is within [`EDGE_NEAR`] of where it
/// meets it, or where it meets a top (a face this upright: the cosine of its slope).
pub const GRIP_REACH: f32 = 0.07;
pub const EDGE_NEAR: f32 = 0.05;
pub const TOP_UP: f32 = 0.7;

impl Rig {
    /// Whether climbing's allowed now (the settings, and the game's say).
    pub fn set_climbing_allowed(&mut self, allowed: bool) {
        self.melee.climb_allowed = allowed;
        if !allowed {
            self.melee.climbs = [None; 2];
        }
    }

    /// Hand `i`, empty, gripping: where it'd hold the world from where it is (its pose), if there's
    /// something to close on there.
    pub(super) fn climb_grip(&self, i: usize) -> Option<(Vec3, Quat)> {
        if !self.melee.climb_allowed {
            return None;
        }
        let w = self.map.world_per_meter;
        let h = &self.hands.as_ref()?[i];
        let (p, q) = self.physics.pose(h.body);
        let s = self.physics.nearest_world(p, GRIP_REACH * w)?;
        let n = (p - s).normalize_or(Vec3::Y);
        let top = n.y >= TOP_UP;
        // An edge: going a little way along the surface, every way round, the surface ends.
        let edge = (0..8).any(|k| {
            let side = Quat::from_axis_angle(n, k as f32 * std::f32::consts::FRAC_PI_4) * n.any_orthonormal_vector();
            let from = s + n * 0.03 * w + side * EDGE_NEAR * w;
            self.physics.ray_world(from, -n, 0.06 * w).is_none()
        });
        (top || edge).then_some((p, q))
    }

    /// The arms climbing, once a frame before the hands are driven: a hand holding the world lets
    /// go stretched past an arm's length, and the body's velocity (its own, as the holding hands'
    /// pulls and its weight change it) is what the game's told to move the player by. Where the game
    /// didn't move them so (a floor under them), the body goes no faster that way than the game took
    /// it, as the game measures over a tenth of a second (frame by frame it's too uneven: the game's
    /// frames aren't the runtime's).
    pub(super) fn climb(&mut self, inp: &super::RigInput, dt: f32) {
        let w = self.map.world_per_meter;
        let dt = dt.max(1e-4);
        let at = inp.map.point(Vec3::ZERO);
        let moved = self.melee.climb_from.map_or(Vec3::ZERO, |p| (at - p) / w / dt);
        self.melee.climb_from = Some(at);
        // (A teleport or a turn's jump isn't the body moving.)
        if moved.length() < 20.0 {
            self.melee.climb_seen += (moved - self.melee.climb_seen) * (dt / CLIMB_SEEN).min(1.0);
        }
        let hands = self.hands.as_ref().unwrap();
        for i in 0..2 {
            if let Some((g, _)) = self.melee.climbs[i]
                && g.distance(hands[i].target.0) > CLIMB_STRETCH * w
            {
                tracing::info!("rig: hand {i} lets go of the world, stretched");
                self.melee.climbs[i] = None;
            }
        }
        // A weapon stuck fast in the world and held holds the body up as a grip on the world does
        // (the body is driven to the hand on something static: an axe in a wall to climb by).
        let hands = self.hands.as_ref().unwrap();
        let stuck: [Option<Vec3>; 2] = std::array::from_fn(|i| {
            if !self.melee.climb_allowed || self.melee.climbs[i].is_some() {
                return None;
            }
            let o = self.objects.get(&hands[i].holding?)?;
            o.embed.filter(|e| e.into == super::embed::Into::World)?;
            let (p, q) = self.physics.pose(o.body);
            match o.attach {
                super::Attach::Fixed(ap, aq) => Some(p + q * super::inverse((ap, aq)).0),
                super::Attach::Gun => None,
            }
        });
        let grips: [Option<Vec3>; 2] = std::array::from_fn(|i| self.melee.climbs[i].map(|c| c.0).or(stuck[i]));
        if grips.iter().all(|c| c.is_none()) || inp.paused {
            self.melee.climb = Vec3::ZERO;
            self.melee.climb_v = Vec3::ZERO;
            self.melee.climb_seen = Vec3::ZERO;
            return;
        }
        let mut v = self.melee.climb_v;
        // Held back by the game (moving that way slower than told, or not at all), it goes as the
        // game took it.
        let seen = self.melee.climb_seen;
        for k in 0..3 {
            if v[k].abs() > 0.2 && v[k] * seen[k] < v[k] * v[k] * 0.5 {
                v[k] = if v[k] * seen[k] > 0.0 { seen[k] } else { 0.0 };
            }
        }
        let mut f = Vec3::new(0.0, -CLIMB_BODY * super::GRAVITY, 0.0);
        for i in 0..2 {
            let Some(g) = grips[i] else { continue };
            // The hand stays at the grip: the body goes the way that brings the controller back
            // to it.
            let err = (g - hands[i].target.0) / w;
            f += (CLIMB_SPRING * err - CLIMB_DAMPER * v).clamp_length_max(CLIMB_MAX_FORCE);
        }
        v += f / CLIMB_BODY * dt;
        tracing::debug!("rig: climbing: seen {seen:.2}, force {f:.0}, now {v:.2}, the controllers {:.2?} from the grips", (0..2).filter_map(|i| grips[i].map(|g| (hands[i].target.0 - g) / w)).collect::<Vec<_>>());
        self.melee.climb_v = v;
        self.melee.climb = v * w;
    }
}
