//! The Rapier world the player interacts with, in game-world metres: the game's static collision
//! (streamed through the collision ring), the objects it registered, and the two hands.
//!
//! Rapier 0.36 does its maths with `glamx`, which pins an older glam than the rest of the runtime,
//! so values cross this module's boundary through the small conversions at the bottom.

use rapier3d::prelude::*;
use rustvr_protocol::*;
use std::collections::HashMap;

pub type V3 = glam::Vec3;
pub type Q = glam::Quat;

/// Collision groups: who touches whom.
pub const G_WORLD: Group = Group::GROUP_1;
pub const G_OBJECT: Group = Group::GROUP_2;
pub const G_HAND: [Group; 2] = [Group::GROUP_3, Group::GROUP_4];
pub const G_HELD: [Group; 2] = [Group::GROUP_5, Group::GROUP_6];

pub fn groups_world() -> InteractionGroups {
    InteractionGroups::new(G_WORLD, Group::ALL, InteractionTestMode::And)
}

/// A free object touches the world, other objects, hands and held objects.
pub fn groups_free() -> InteractionGroups {
    InteractionGroups::new(G_OBJECT, G_WORLD | G_OBJECT | G_HAND[0] | G_HAND[1] | G_HELD[0] | G_HELD[1], InteractionTestMode::And)
}

/// A part of one of the game's creatures, followed (`Rig::follow_actors`): the hands, what they
/// hold and loose objects meet it (and push the creature, through the game).
pub fn groups_actor() -> InteractionGroups {
    InteractionGroups::new(G_OBJECT, G_OBJECT | G_HAND[0] | G_HAND[1] | G_HELD[0] | G_HELD[1], InteractionTestMode::And)
}

/// An object held by `hand` touches the world and free objects, never the hands.
pub fn groups_held(hand: usize) -> InteractionGroups {
    InteractionGroups::new(G_HELD[hand], G_WORLD | G_OBJECT, InteractionTestMode::And)
}

/// A hand touches the world and free objects (not what either hand is holding).
pub fn groups_hand(hand: usize) -> InteractionGroups {
    InteractionGroups::new(G_HAND[hand], G_WORLD | G_OBJECT, InteractionTestMode::And)
}

/// What's solid to an empty `hand` as it's moved ([`Physics::reach`]): the world, and what the other
/// hand holds (the hand rests on it rather than going through it or knocking it).
pub fn solid_for_hand(hand: usize) -> InteractionGroups {
    InteractionGroups::new(Group::ALL, G_WORLD | G_HELD[hand ^ 1], InteractionTestMode::And)
}

/// The gap a hand keeps from what's solid to it.
const SKIN: f32 = 0.002;
/// How many times a hand's move goes round: put out of what it's in, swept, slid on along it.
const REACH_PASSES: usize = 4;

/// A finger of `hand`: it pushes free objects and nothing else.
pub fn groups_finger(hand: usize) -> InteractionGroups {
    InteractionGroups::new(G_HAND[hand], G_OBJECT, InteractionTestMode::And)
}

/// One of the game's own props while the game moves it (`OBJF_GAME_SIMULATED`): it touches the
/// world and loose objects, while the hands, fingers and what they hold go through it and push it
/// instead (`Rig::game_pushes`).
pub fn groups_game_prop() -> InteractionGroups {
    InteractionGroups::new(G_OBJECT, G_WORLD | G_OBJECT, InteractionTestMode::And)
}

pub fn groups_none() -> InteractionGroups {
    InteractionGroups::new(Group::NONE, Group::NONE, InteractionTestMode::And)
}

/// Where a ray met the static world.
#[derive(Clone, Copy, Debug)]
pub struct WorldHit {
    pub distance: f32,
    pub normal: V3,
    /// The triangle's material (`MAT_*`) when the game gave its meshes theirs.
    pub material: Option<u32>,
}

pub struct Physics {
    pub world: PhysicsWorld,
    meshes: HashMap<u32, (RigidBodyHandle, ColliderHandle)>,
    /// Each mesh's triangles' materials (`COL_MESH_MATERIALS`).
    materials: HashMap<u32, Vec<u8>>,
    boxes: HashMap<u32, (RigidBodyHandle, ColliderHandle)>,
    /// Number of static triangles loaded (diagnostics).
    pub triangles: usize,
}

impl Default for Physics {
    fn default() -> Self {
        let mut world = PhysicsWorld::new();
        world.gravity = Vector::new(0.0, -9.81, 0.0);
        Physics { world, meshes: HashMap::new(), materials: HashMap::new(), boxes: HashMap::new(), triangles: 0 }
    }
}

impl Physics {
    pub fn set_dt(&mut self, dt: f32) {
        self.world.integration_parameters.dt = dt.clamp(1.0 / 240.0, 1.0 / 30.0);
    }

    pub fn step(&mut self) {
        self.world.step();
    }

    // ---- static world ---------------------------------------------------------------------------
    /// Applies one collision-ring message.
    pub fn apply_collision(&mut self, kind: u32, payload: &[u8]) {
        match kind {
            COL_CLEAR => {
                for (_, (b, _)) in self.meshes.drain().chain(self.boxes.drain()) {
                    self.world.remove_body_with_colliders(b, true);
                }
                self.materials.clear();
                self.triangles = 0;
            }
            COL_MESH_MATERIALS if payload.len() >= size_of::<ColMesh>() => {
                let head: ColMesh = bytemuck::pod_read_unaligned(&payload[..size_of::<ColMesh>()]);
                let m = &payload[size_of::<ColMesh>()..];
                self.materials.insert(head.id, m[..(head.count as usize).min(m.len())].to_vec());
            }
            COL_MESH if payload.len() >= size_of::<ColMesh>() => {
                let head: ColMesh = bytemuck::pod_read_unaligned(&payload[..size_of::<ColMesh>()]);
                let tris_bytes = &payload[size_of::<ColMesh>()..];
                let count = (head.count as usize).min(tris_bytes.len() / size_of::<ColTriangle>());
                if let Some((b, _)) = self.meshes.remove(&head.id) {
                    self.world.remove_body_with_colliders(b, true);
                }
                // (Its materials come after it, if the game has them.)
                self.materials.remove(&head.id);
                if count == 0 {
                    return;
                }
                let mut verts = Vec::with_capacity(count * 3);
                let mut idx = Vec::with_capacity(count);
                for i in 0..count {
                    let t: ColTriangle = bytemuck::pod_read_unaligned(&tris_bytes[i * size_of::<ColTriangle>()..(i + 1) * size_of::<ColTriangle>()]);
                    let base = verts.len() as u32;
                    for k in 0..3 {
                        verts.push(Vector::new(t.v[k * 3], t.v[k * 3 + 1], t.v[k * 3 + 2]));
                    }
                    idx.push([base, base + 1, base + 2]);
                }
                // (As loose triangles: Rapier's joining them up to smooth their seams makes them
                // one-sided by their winding, and the game's face either way.)
                match ColliderBuilder::trimesh(verts, idx) {
                    Ok(c) => {
                        let h = self.world.insert(RigidBodyBuilder::fixed(), c.collision_groups(groups_world()).friction(0.8));
                        self.meshes.insert(head.id, h);
                        self.triangles += count;
                        tracing::info!("rig: collision mesh {:#x}: {count} triangles", head.id);
                    }
                    Err(e) => tracing::warn!("rig: collision mesh {}: {e:?}", head.id),
                }
            }
            COL_BOX if payload.len() >= size_of::<ColBox>() => {
                let b: ColBox = bytemuck::pod_read_unaligned(&payload[..size_of::<ColBox>()]);
                if let Some((h, _)) = self.boxes.remove(&b.id) {
                    self.world.remove_body_with_colliders(h, true);
                }
                let half = b.half_extents;
                if half.iter().any(|h| *h <= 0.0) {
                    return;
                }
                let pose = to_pose(V3::from_array(b.center), quat(b.rot));
                let body = RigidBodyBuilder::fixed().pose(pose);
                let c = ColliderBuilder::cuboid(half[0], half[1], half[2]).collision_groups(groups_world()).friction(0.8);
                let h = self.world.insert(body, c);
                self.boxes.insert(b.id, h);
            }
            _ => {}
        }
    }

    pub fn has_world(&self) -> bool {
        !self.meshes.is_empty() || !self.boxes.is_empty()
    }

    // ---- bodies ---------------------------------------------------------------------------------
    /// A dynamic body with the object's collider (`hulls`: its convex parts' points, for
    /// [`SHAPE_HULL`]).
    pub fn add_object(&mut self, d: &ObjectDesc, pos: V3, rot: Q, hulls: &[Vec<V3>]) -> (RigidBodyHandle, ColliderHandle) {
        let body = RigidBodyBuilder::dynamic()
            .pose(to_pose(pos, rot))
            .ccd_enabled(true)
            .linear_damping(0.05)
            .angular_damping(0.2);
        let c = object_collider(d, hulls).collision_groups(groups_free());
        self.world.insert(body, c)
    }

    /// Gives an object's collider its convex parts (each part's points, object space) and `mass`.
    /// False if they make no hull.
    pub fn set_hulls(&mut self, c: ColliderHandle, parts: &[Vec<V3>], mass: f32) -> bool {
        let (Some(shape), Some(col)) = (hulls_shape(parts), self.world.colliders.get_mut(c)) else { return false };
        col.set_shape(shape);
        col.set_translation_wrt_parent(Vector::ZERO);
        col.set_mass(mass.max(0.02));
        true
    }

    /// A hand: a light dynamic palm box with no gravity, driven by velocity. `palm`: the box in the
    /// hand's own space (centre, rotation, half sizes; [`super::fingers::palm`]).
    pub fn add_hand(&mut self, hand: usize, pos: V3, rot: Q, palm: (V3, Q, V3)) -> (RigidBodyHandle, ColliderHandle) {
        let body = RigidBodyBuilder::dynamic()
            .pose(to_pose(pos, rot))
            .gravity_scale(0.0)
            .ccd_enabled(true)
            .linear_damping(0.0)
            .angular_damping(0.0);
        // 0.5 kg like LÖVR's hand.
        let (at, q, half) = palm;
        let c = ColliderBuilder::cuboid(half.x, half.y, half.z).position(to_pose(at, q)).mass(0.5).friction(1.0).collision_groups(groups_hand(hand));
        self.world.insert(body, c)
    }

    /// A hand's collider as the palm and its fingers (`palm` its box: centre, rotation, half sizes;
    /// `fingers` each one's ends; all in the hand's own space), so the fingers meet what the hand
    /// does: nothing goes through them.
    pub fn set_hand_shape(&mut self, c: ColliderHandle, palm: (V3, Q, V3), fingers: &[(V3, V3)]) {
        let Some(col) = self.world.colliders.get_mut(c) else { return };
        col.set_position_wrt_parent(to_pose(V3::ZERO, Q::IDENTITY));
        let (at, q, half) = palm;
        let mut parts = vec![(to_pose(at, q), SharedShape::cuboid(half.x, half.y, half.z))];
        for &(a, b) in fingers {
            parts.push((to_pose(V3::ZERO, Q::IDENTITY), SharedShape::capsule(to_v(a), to_v(b), super::fingers::RADIUS)));
        }
        col.set_shape(SharedShape::compound(parts));
    }

    /// A finger: a kinematic capsule (along its +Y, `half_height` of straight part), touching
    /// nothing until given [`groups_finger`].
    pub fn add_finger(&mut self, half_height: f32) -> RigidBodyHandle {
        let body = RigidBodyBuilder::kinematic_position_based();
        let c = ColliderBuilder::capsule_y(half_height, super::fingers::RADIUS).friction(0.8).collision_groups(groups_none());
        self.world.insert(body, c).0
    }

    /// Where `pusher` overlaps `other`, if it does: the point on `other` (world), the direction out
    /// of the pusher into it, and how deep.
    pub fn overlap(&self, pusher: ColliderHandle, other: ColliderHandle) -> Option<(V3, V3, f32)> {
        let (a, b) = (self.world.colliders.get(pusher)?, self.world.colliders.get(other)?);
        let c = rapier3d::parry::query::contact(a.position(), a.shape(), b.position(), b.shape(), 0.0).ok()??;
        (c.dist < 0.0).then(|| (from_v(c.point2), from_v(c.normal1), -c.dist))
    }

    /// Every collider's surface as world triangles (the framework view): `tint` gives each its
    /// colour, or leaves it out.
    pub fn surface_triangles(&self, mut tint: impl FnMut(ColliderHandle, &Collider) -> Option<[f32; 3]>, out: &mut Vec<([V3; 3], [f32; 3])>) {
        for (h, c) in self.world.colliders.iter() {
            let Some(color) = tint(h, c) else { continue };
            let s = c.shape();
            let (verts, idx) = if let Some(s) = s.as_cuboid() {
                s.to_trimesh()
            } else if let Some(s) = s.as_ball() {
                s.to_trimesh(12, 8)
            } else if let Some(s) = s.as_capsule() {
                s.to_trimesh(10, 6)
            } else if let Some(s) = s.as_convex_polyhedron() {
                s.to_trimesh()
            } else if let Some(s) = s.as_trimesh() {
                (s.vertices().to_vec(), s.indices().to_vec())
            } else if let Some(s) = s.as_heightfield() {
                s.to_trimesh()
            } else {
                continue;
            };
            let pose = c.position();
            for t in idx {
                out.push((t.map(|i| from_v(pose.transform_point(verts[i as usize]))), color));
            }
        }
    }

    /// The first collider of `body`.
    pub fn collider_of(&self, body: RigidBodyHandle) -> Option<ColliderHandle> {
        self.world.bodies.get(body)?.colliders().first().copied()
    }

    /// Hangs `body` on a ball joint from the world point where its own point `local` (body space)
    /// is now. Returns the fixed anchor the joint holds on to.
    pub fn hang(&mut self, body: RigidBodyHandle, local: V3) -> RigidBodyHandle {
        let (pos, rot) = self.pose(body);
        let at = pos + rot * local;
        let anchor = self.world.insert_body(RigidBodyBuilder::fixed().translation(to_v(at)));
        let joint = SphericalJointBuilder::new().local_anchor1(to_v(V3::ZERO)).local_anchor2(to_v(local)).contacts_enabled(false);
        self.world.insert_impulse_joint(anchor, body, joint);
        anchor
    }

    /// A creature's part: a kinematic capsule along +Y, `half` its straight half length.
    pub fn add_actor_part(&mut self, pos: V3, rot: Q, half: f32, radius: f32) -> (RigidBodyHandle, ColliderHandle) {
        let body = RigidBodyBuilder::kinematic_position_based().pose(to_pose(pos, rot));
        let c = ColliderBuilder::capsule_y(half.max(0.001), radius.max(0.005)).friction(0.6).collision_groups(groups_actor());
        self.world.insert(body, c)
    }

    /// Where collider `c` (a hand), turned `rot`, gets to from where it is on its way to `to`: put
    /// out of anything solid it's in (turned into a wall, or something moved into it), then swept
    /// towards `to`, stopping at what's solid (`solid`) and sliding on along it. A controller pushed
    /// on through a wall leaves the hand on its face, going where the controller goes along it,
    /// never into it. `inside`: something solid it may be in (it just let go of it): it isn't put
    /// out of that, but it only ever moves out of it, never on in or through.
    pub fn reach(&self, c: ColliderHandle, rot: Q, to: V3, solid: InteractionGroups, inside: Option<ColliderHandle>) -> V3 {
        let Some(col) = self.world.colliders.get(c) else { return to };
        let shape = col.shape();
        let filter = QueryFilter { groups: Some(solid), exclude_collider: Some(c), ..Default::default() };
        let not_inside = |h: ColliderHandle, _: &Collider| Some(h) != inside;
        let pushed_out_of = QueryFilter { predicate: Some(&not_inside), ..filter };
        let mut pos = from_v(col.position().translation);
        // Out of what it's in, deepest first.
        for _ in 0..REACH_PASSES {
            let pose = to_pose(pos, rot);
            let deepest = self
                .world
                .intersect_shape(pose, shape, pushed_out_of)
                .filter_map(|(_, o)| rapier3d::parry::query::contact(&pose, shape, o.position(), o.shape(), 0.0).ok().flatten())
                .min_by(|a, b| a.dist.total_cmp(&b.dist));
            match deepest {
                Some(ct) if ct.dist < 0.0 => pos += from_v(ct.normal1) * (ct.dist - SKIN),
                _ => break,
            }
        }
        // Then on towards `to`, along whatever it meets.
        let options = rapier3d::parry::query::ShapeCastOptions { max_time_of_impact: 1.0, target_distance: SKIN, stop_at_penetration: false, compute_impact_geometry_on_penetration: true };
        let mut left = to - pos;
        for _ in 0..REACH_PASSES {
            if left.length_squared() < 1e-10 {
                break;
            }
            let Some((_, hit)) = self.world.cast_shape(&to_pose(pos, rot), to_v(left), shape, options, filter) else {
                pos += left;
                break;
            };
            let toi = hit.time_of_impact.clamp(0.0, 1.0);
            pos += left * toi;
            left *= 1.0 - toi;
            // What's left of the way, less what goes into the surface (a hair off it, so the next
            // sweep slides rather than catching on it).
            let n = from_v(hit.normal1).normalize_or_zero();
            let into = left.dot(n);
            if into < 0.0 {
                left -= n * into;
            }
            left += n * 1e-4;
        }
        pos
    }

    /// Whether a finger from `a` to `b` (world, `radius` thick) is in anything solid (`solid`, not
    /// `skip`).
    pub fn finger_in(&self, a: V3, b: V3, radius: f32, solid: InteractionGroups, skip: Option<ColliderHandle>) -> bool {
        let shape = rapier3d::parry::shape::Capsule::new(to_v(a), to_v(b), radius);
        let not_skipped = |h: ColliderHandle, _: &Collider| Some(h) != skip;
        let filter = QueryFilter { groups: Some(solid), predicate: Some(&not_skipped), ..Default::default() };
        self.world.intersect_shape(to_pose(V3::ZERO, Q::IDENTITY), &shape, filter).next().is_some()
    }

    /// How far apart two colliders are (0 touching or overlapping).
    pub fn distance_between(&self, a: ColliderHandle, b: ColliderHandle) -> f32 {
        let (Some(a), Some(b)) = (self.world.colliders.get(a), self.world.colliders.get(b)) else { return f32::MAX };
        rapier3d::parry::query::distance(a.position(), a.shape(), b.position(), b.shape()).map_or(f32::MAX, |d| d.distance)
    }

    /// The colliders touching `c` now (an active contact).
    pub fn touching(&self, c: ColliderHandle) -> Vec<ColliderHandle> {
        self.world
            .narrow_phase
            .contact_pairs_with(c)
            .filter(|p| p.has_any_active_contact())
            .map(|p| if p.collider1 == c { p.collider2 } else { p.collider1 })
            .collect()
    }

    /// A finger's capsule length, as it curls.
    pub fn set_capsule_half_height(&mut self, b: RigidBodyHandle, half: f32) {
        let Some(&c) = self.world.bodies[b].colliders().first() else { return };
        if let Some(col) = self.world.colliders.get_mut(c)
            && let Some(cap) = col.shape_mut().as_capsule_mut()
        {
            let r = cap.radius;
            *cap = rapier3d::parry::shape::Capsule::new_y(half, r);
        }
    }

    pub fn set_damping(&mut self, b: RigidBodyHandle, linear: f32, angular: f32) {
        let rb = &mut self.world.bodies[b];
        rb.set_linear_damping(linear);
        rb.set_angular_damping(angular);
    }

    pub fn remove(&mut self, body: RigidBodyHandle) {
        self.world.remove_body_with_colliders(body, true);
    }

    pub fn pose(&self, b: RigidBodyHandle) -> (V3, Q) {
        let p = self.world.bodies[b].position();
        from_pose(p)
    }

    pub fn velocity(&self, b: RigidBodyHandle) -> (V3, V3) {
        let rb = &self.world.bodies[b];
        (from_v(rb.linvel()), from_v(rb.angvel()))
    }

    pub fn set_pose(&mut self, b: RigidBodyHandle, pos: V3, rot: Q) {
        self.world.bodies[b].set_position(to_pose(pos, rot), true);
    }

    /// Turns a body to `rot` where it is, its spin stopped.
    pub fn set_rotation(&mut self, b: RigidBodyHandle, rot: Q) {
        let rb = &mut self.world.bodies[b];
        rb.set_rotation(to_q(rot.normalize()), true);
        rb.set_angvel(Vector::ZERO, true);
    }

    pub fn set_velocity(&mut self, b: RigidBodyHandle, lin: V3, ang: V3) {
        let rb = &mut self.world.bodies[b];
        rb.set_linvel(to_v(lin), true);
        rb.set_angvel(to_v(ang), true);
    }

    pub fn set_groups(&mut self, c: ColliderHandle, g: InteractionGroups) {
        if let Some(col) = self.world.colliders.get_mut(c) {
            col.set_collision_groups(g);
            col.set_solver_groups(g);
        }
    }

    pub fn set_gravity(&mut self, b: RigidBodyHandle, scale: f32) {
        self.world.bodies[b].set_gravity_scale(scale, true);
    }

    /// A body that yields (`true`) stops at what it meets without pushing it (Rapier's dominance:
    /// every other body treats it as weightless).
    pub fn set_yielding(&mut self, b: RigidBodyHandle, yielding: bool) {
        self.world.bodies[b].set_dominance_group(if yielding { -1 } else { 0 });
    }

    pub fn set_angular_damping(&mut self, b: RigidBodyHandle, d: f32) {
        self.world.bodies[b].set_angular_damping(d);
    }

    /// Kinematic bodies are moved only by us (holstered or seated objects ride along exactly).
    pub fn set_kinematic(&mut self, b: RigidBodyHandle, kinematic: bool) {
        let t = if kinematic { RigidBodyType::KinematicPositionBased } else { RigidBodyType::Dynamic };
        let rb = &mut self.world.bodies[b];
        if rb.body_type() != t {
            rb.set_body_type(t, true);
        }
    }

    pub fn move_kinematic(&mut self, b: RigidBodyHandle, pos: V3, rot: Q) {
        self.world.bodies[b].set_next_kinematic_position(to_pose(pos, rot));
    }

    /// Drives a dynamic body towards a target pose by setting its velocity: it arrives in one
    /// step when nothing is in the way (see [`Drive`]). Returns the carry's share of the velocity
    /// set (its pivot's, and the spin): next step's [`Drive::carried`].
    pub fn track(&mut self, b: RigidBodyHandle, target_pos: V3, target_rot: Q, dt: f32, d: &Drive) -> (V3, V3) {
        let rb = &mut self.world.bodies[b];
        let (pos, rot) = from_pose(rb.position());
        let dt = dt.max(1e-4);
        // What the player's own walk and turn do to it this step: its weight resists only the rest.
        let (pos_c, rot_c) = d.carry.unwrap_or((pos, rot));
        let pivot_now = pos + rot * d.pivot;
        let pivot_c = pos_c + rot_c * d.pivot;
        let carry_v = (pivot_c - pivot_now) / dt;
        let carry_w = scaled_axis((rot_c * rot.inverse()).normalize()) / dt;
        // How it moves now against the player (its pivot): less what the last carry gave it.
        let com = from_v(rb.center_of_mass());
        let w_now = from_v(rb.angvel());
        let v_pivot = from_v(rb.linvel()) + w_now.cross(pivot_now - com);
        let (was_v, was_w) = d.carried;

        let dq = (target_rot * rot_c.inverse()).normalize();
        let dq = if dq.w < 0.0 { -dq } else { dq };
        let (axis, angle) = dq.to_axis_angle();
        let mut ang = if angle.is_finite() && axis.is_finite() { axis * (angle / dt * d.ang_gain.clamp(0.02, 1.0)) } else { V3::ZERO };
        // Turning the same way: never faster than it can still stop in, and only so much spin
        // gained or lost per step, so a heavy thing swings round and settles instead of snapping.
        if angle.is_finite() {
            let brake = (2.0 * d.max_ang_accel * angle.abs()).sqrt();
            if ang.length() > brake {
                ang = ang.normalize_or_zero() * brake;
            }
            let cur = w_now - was_w;
            let dw = ang - cur;
            let max_dw = d.max_ang_accel * dt;
            if dw.length() > max_dw {
                ang = cur + dw.normalize() * max_dw;
            }
        }
        let ang = (ang + carry_w).clamp_length_max(60.0);

        // The pivot onto the target's.
        let to = target_pos + target_rot * d.pivot - pivot_c;
        let mut want = to / dt * d.gain;
        // Never faster than it could still stop in, so heavy (acceleration-limited) objects
        // settle onto the hand instead of overshooting and swinging back.
        let brake = (2.0 * d.max_accel * to.length()).sqrt();
        if want.length() > brake {
            want = want.normalize_or_zero() * brake;
        }
        let cur = v_pivot - was_v;
        let dv = want - cur;
        let max_dv = d.max_accel * dt;
        if dv.length() > max_dv {
            want = cur + dv.normalize() * max_dv;
        }
        // 20 m/s is beyond any real hand; it only guards against tracking glitches.
        let want = want.clamp_length_max(20.0) + carry_v;
        // The engine moves the centre of mass and turns the body about it: the velocity that,
        // turned as above, leaves the pivot where it's wanted.
        let rot_next = (Q::from_scaled_axis(ang * dt) * rot).normalize();
        let com_next = pivot_now + want * dt + rot_next * (from_v(rb.local_center_of_mass()) - d.pivot);
        rb.set_linvel(to_v((com_next - com) / dt), true);
        rb.set_angvel(to_v(ang), true);
        (carry_v, carry_w)
    }

    /// Collider `c`, where it is now, overlaps the static world.
    pub fn in_world(&self, c: ColliderHandle) -> bool {
        let Some(col) = self.world.colliders.get(c) else { return false };
        let filter = QueryFilter::default().groups(InteractionGroups::new(Group::ALL, G_WORLD, InteractionTestMode::And)).exclude_collider(c);
        self.world.intersect_shape(*col.position(), col.shape(), filter).next().is_some()
    }

    /// True if the collider currently touches the static world or a free object.
    pub fn in_contact(&self, c: ColliderHandle) -> bool {
        self.world.contact_pairs_with(c).any(|p| p.has_any_active_contact())
    }

    /// Distance from `p` to the surface of collider `c` (0 inside).
    pub fn distance_to(&self, c: ColliderHandle, p: V3) -> f32 {
        let Some(col) = self.world.colliders.get(c) else { return f32::MAX };
        let proj = col.shape().project_point(col.position(), to_v(p), true);
        if proj.is_inside { 0.0 } else { (from_v(proj.point) - p).length() }
    }

    /// The point of collider `c`'s surface nearest `p` (`p` itself when inside).
    pub fn closest_point(&self, c: ColliderHandle, p: V3) -> V3 {
        let Some(col) = self.world.colliders.get(c) else { return p };
        from_v(col.shape().project_point(col.position(), to_v(p), true).point)
    }

    /// The point of collider `c`'s surface nearest `p`, from inside it too.
    pub fn surface_point(&self, c: ColliderHandle, p: V3) -> V3 {
        let Some(col) = self.world.colliders.get(c) else { return p };
        from_v(col.shape().project_point(col.position(), to_v(p), false).point)
    }

    /// Collider `c` as solids a hand's fingers close on (world space).
    pub fn grasp_solids(&self, c: ColliderHandle, g: &mut rustvr_avatar::Grasp) {
        let Some(col) = self.world.colliders.get(c) else { return };
        add_solids(col.shape(), from_pose(col.position()), g);
    }

    /// Nothing of the static world lies between `from` and `to` (the player could reach `to`).
    pub fn reachable(&self, from: V3, to: V3) -> bool {
        let d = to - from;
        let len = d.length();
        len < 1e-4 || self.ray_world(from, d, len).is_none()
    }

    /// First static-world hit along a ray, as a distance.
    pub fn ray_world(&self, origin: V3, dir: V3, max: f32) -> Option<f32> {
        let ray = Ray::new(to_v(origin), to_v(dir.normalize_or_zero()));
        let filter = QueryFilter::default().groups(InteractionGroups::new(Group::ALL, G_WORLD, InteractionTestMode::And));
        self.world.cast_ray(&ray, max, true, filter).map(|(_, t)| t)
    }

    /// First hit along a ray against the world and loose objects (not the hands or what they
    /// hold), leaving out collider `skip`: (distance, collider, surface normal).
    pub fn ray_cast(&self, origin: V3, dir: V3, max: f32, skip: ColliderHandle) -> Option<(f32, ColliderHandle, V3)> {
        let ray = Ray::new(to_v(origin), to_v(dir.normalize_or_zero()));
        let filter = QueryFilter::default().groups(InteractionGroups::new(Group::ALL, G_WORLD | G_OBJECT, InteractionTestMode::And)).exclude_collider(skip);
        self.world.cast_ray_and_get_normal(&ray, max, true, filter).map(|(c, hit)| (hit.time_of_impact, c, from_v(hit.normal)))
    }

    /// First static-world hit along a ray: how far, its surface, and what it's made of.
    pub fn world_hit(&self, origin: V3, dir: V3, max: f32) -> Option<WorldHit> {
        let ray = Ray::new(to_v(origin), to_v(dir.normalize_or_zero()));
        let filter = QueryFilter::default().groups(InteractionGroups::new(Group::ALL, G_WORLD, InteractionTestMode::And));
        let (c, hit) = self.world.cast_ray_and_get_normal(&ray, max, true, filter)?;
        let material = match hit.feature {
            FeatureId::Face(i) => self.meshes.iter().find(|(_, (_, h))| *h == c).and_then(|(id, _)| self.materials.get(id)).and_then(|m| m.get(i as usize)).map(|&m| m as u32),
            _ => None,
        };
        Some(WorldHit { distance: hit.time_of_impact, normal: from_v(hit.normal), material })
    }

    /// The collider belongs to the static world.
    pub fn is_world(&self, c: ColliderHandle) -> bool {
        self.meshes.values().chain(self.boxes.values()).any(|(_, h)| *h == c)
    }

    /// A push on a dynamic body at a world point (N s).
    pub fn apply_impulse(&mut self, b: RigidBodyHandle, impulse: V3, at: V3) {
        if let Some(rb) = self.world.bodies.get_mut(b) {
            rb.apply_impulse_at_point(to_v(impulse), to_v(at), true);
        }
    }

    /// How far a sphere at `p` sinks into the static world (0 when clear).
    pub fn sphere_penetration(&self, p: V3, radius: f32) -> f32 {
        let filter = QueryFilter::default().groups(InteractionGroups::new(Group::ALL, G_WORLD, InteractionTestMode::And));
        match self.world.project_point(to_v(p), radius, true, filter) {
            Some((_, proj)) => {
                if proj.is_inside {
                    radius + (from_v(proj.point) - p).length()
                } else {
                    (radius - (from_v(proj.point) - p).length()).max(0.0)
                }
            }
            None => 0.0,
        }
    }
}

/// A shape at `at` (world) as solids for the fingers ([`Physics::grasp_solids`]). A cylinder (a
/// valve's wheel) is taken as its box.
fn add_solids(shape: &dyn Shape, at: (V3, Q), g: &mut rustvr_avatar::Grasp) {
    use rustvr_avatar::Solid;
    let (p, q) = at;
    if let Some(b) = shape.as_ball() {
        g.solids.push(Solid::Sphere { centre: p, radius: b.radius });
    } else if let Some(c) = shape.as_cuboid() {
        g.solids.push(Solid::Box { centre: p, rot: q, half: from_v(c.half_extents) });
    } else if let Some(c) = shape.as_capsule() {
        g.solids.push(Solid::Capsule { a: p + q * from_v(c.segment.a), b: p + q * from_v(c.segment.b), radius: c.radius });
    } else if let Some(c) = shape.as_cylinder() {
        g.solids.push(Solid::Box { centre: p, rot: q, half: V3::new(c.radius, c.half_height, c.radius) });
    } else if let Some(h) = shape.as_convex_polyhedron() {
        let first = g.planes.len() as u32;
        let (points, adj) = (h.points(), h.vertices_adj_to_face());
        for f in h.faces() {
            let Some(&v) = adj.get(f.first_vertex_or_edge as usize) else { continue };
            let Some(&on) = points.get(v as usize) else { continue };
            let n = q * from_v(f.normal);
            g.planes.push(n.extend(-n.dot(p + q * from_v(on))));
        }
        g.solids.push(Solid::Hull { first, count: g.planes.len() as u32 - first });
    } else if let Some(c) = shape.as_compound() {
        for (pose, part) in c.shapes() {
            let (pp, pq) = from_pose(pose);
            add_solids(part.as_ref(), (p + q * pp, (q * pq).normalize()), g);
        }
    }
}

/// One convex hull, or a compound of them, for an object's convex parts.
fn hulls_shape(parts: &[Vec<V3>]) -> Option<SharedShape> {
    let hull = |p: &Vec<V3>| SharedShape::convex_hull(&p.iter().map(|q| to_v(*q)).collect::<Vec<_>>());
    match parts {
        [] => None,
        [one] => hull(one),
        many => {
            let shapes: Vec<_> = many.iter().filter_map(hull).map(|s| (to_pose(V3::ZERO, Q::IDENTITY), s)).collect();
            (!shapes.is_empty()).then(|| SharedShape::compound(shapes))
        }
    }
}

pub fn object_collider(d: &ObjectDesc, hulls: &[Vec<V3>]) -> ColliderBuilder {
    let he = d.half_extents;
    let hull = if d.shape == SHAPE_HULL { hulls_shape(hulls) } else { None };
    if let Some(shape) = hull {
        return ColliderBuilder::new(shape).mass(d.mass.max(0.02)).friction(0.7).restitution(0.15);
    }
    if d.kind == OBJ_VALVE {
        // The wheel: a disc about its +Z.
        return ColliderBuilder::cylinder(he[2].max(0.005), d.radius.max(0.02)).rotation(Vector::new(std::f32::consts::FRAC_PI_2, 0.0, 0.0)).mass(d.mass.max(0.1)).friction(0.9);
    }
    let shape = match d.shape {
        SHAPE_CAPSULE => ColliderBuilder::capsule_y(he[1].max(0.001), d.radius.max(0.001)),
        SHAPE_SPHERE => ColliderBuilder::ball(d.radius.max(0.001)),
        _ => ColliderBuilder::cuboid(he[0].max(0.002), he[1].max(0.002), he[2].max(0.002)),
    };
    shape
        .translation(to_v(V3::from_array(d.center)))
        .mass(d.mass.max(0.02))
        .friction(0.7)
        .restitution(0.15)
}

// ---- glam 0.34 <-> Rapier's glam ------------------------------------------------------------
/// How [`RigPhysics::track`] drives a body onto its target.
#[derive(Clone, Copy, Debug)]
pub struct Drive {
    /// Where a hand holds it (body space): the point driven onto the target's, which it turns about.
    pub pivot: V3,
    /// Where the player's own walk and turn take it this step (world pose): it goes along whatever
    /// its weight, as a velocity, so the world can still stop it.
    pub carry: Option<(V3, Q)>,
    /// The share of its velocity (its pivot's, and the spin) the last step's carry gave it, turned
    /// as the player has turned since: its weight resists changes to the rest only, so the player
    /// setting off, stopping or turning takes it along at once.
    pub carried: (V3, V3),
    /// < 1 softens it: the share of the way it makes up each step.
    pub gain: f32,
    /// How hard it may change speed and spin (its weight): m/s^2, rad/s^2.
    pub max_accel: f32,
    /// The share of the rotation it makes up each step.
    pub ang_gain: f32,
    pub max_ang_accel: f32,
}

impl Drive {
    /// Straight there in a step, however hard that is.
    pub const FREE: Drive = Drive { pivot: V3::ZERO, carry: None, carried: (V3::ZERO, V3::ZERO), gain: 1.0, max_accel: 1e5, ang_gain: 1.0, max_ang_accel: 1e5 };
}

/// A rotation as axis times angle (radians), the short way round.
pub fn scaled_axis(q: Q) -> V3 {
    let q = if q.w < 0.0 { -q } else { q };
    let v = q.to_scaled_axis();
    if v.is_finite() { v } else { V3::ZERO }
}

pub fn to_v(v: V3) -> Vector {
    Vector::new(v.x, v.y, v.z)
}

pub fn from_v(v: Vector) -> V3 {
    V3::new(v.x, v.y, v.z)
}

pub fn to_q(q: Q) -> Rotation {
    Rotation::from_xyzw(q.x, q.y, q.z, q.w)
}

pub fn from_q(q: Rotation) -> Q {
    Q::from_xyzw(q.x, q.y, q.z, q.w)
}

pub fn to_pose(p: V3, q: Q) -> rapier3d::math::Pose {
    rapier3d::math::Pose::from_parts(to_v(p), to_q(q.normalize()))
}

pub fn from_pose(p: &rapier3d::math::Pose) -> (V3, Q) {
    (from_v(p.translation), from_q(p.rotation))
}

pub fn quat(a: [f32; 4]) -> Q {
    let q = Q::from_array(a);
    if q.length_squared() < 1e-12 { Q::IDENTITY } else { q.normalize() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;

    fn floor(p: &mut Physics) {
        let b = ColBox { id: 1, center: [0.0, -0.5, 0.0], half_extents: [10.0, 0.5, 10.0], rot: [0.0, 0.0, 0.0, 1.0], ..Default::default() };
        p.apply_collision(COL_BOX, bytemuck::bytes_of(&b));
    }

    fn crate_desc() -> ObjectDesc {
        let mut d = ObjectDesc::zeroed();
        d.id = 5;
        d.kind = OBJ_PROP;
        d.shape = SHAPE_BOX;
        d.half_extents = [0.1, 0.1, 0.1];
        d.mass = 2.0;
        d
    }

    #[test]
    fn objects_fall_and_rest_on_the_floor() {
        let mut p = Physics::default();
        floor(&mut p);
        let (b, _) = p.add_object(&crate_desc(), V3::new(0.0, 1.0, 0.0), Q::IDENTITY, &[]);
        p.set_dt(1.0 / 90.0);
        for _ in 0..180 {
            p.step();
        }
        let (pos, _) = p.pose(b);
        assert!((pos.y - 0.1).abs() < 0.02, "rests on the floor: {pos:?}");
    }

    #[test]
    fn a_tracked_hand_reaches_its_target_unless_blocked() {
        let mut p = Physics::default();
        floor(&mut p);
        let wall = ColBox { id: 2, center: [0.0, 1.0, 1.0], half_extents: [2.0, 2.0, 0.05], rot: [0.0, 0.0, 0.0, 1.0], ..Default::default() };
        p.apply_collision(COL_BOX, bytemuck::bytes_of(&wall));
        let (h, _) = p.add_hand(1, V3::new(0.0, 1.0, 0.0), Q::IDENTITY, test_palm(1));
        p.set_dt(1.0 / 90.0);
        for _ in 0..5 {
            p.track(h, V3::new(0.0, 1.0, 0.4), Q::IDENTITY, 1.0 / 90.0, &Drive::FREE);
            p.step();
        }
        assert!(p.pose(h).0.distance(V3::new(0.0, 1.0, 0.4)) < 0.01, "free space: on target");
        for _ in 0..30 {
            p.track(h, V3::new(0.0, 1.0, 1.3), Q::IDENTITY, 1.0 / 90.0, &Drive::FREE);
            p.step();
        }
        let z = p.pose(h).0.z;
        assert!(z < 0.95, "the wall stops the hand: z = {z}");
    }

    /// Hand `i`'s palm on its grip, as the rig puts it.
    fn test_palm(i: usize) -> (V3, Q, V3) {
        let side = if i == 0 { rustvr_ik::Side::Left } else { rustvr_ik::Side::Right };
        let hand = rustvr_ik::HandOffsets::default().hand_frame(side, rustvr_ik::Pose::IDENTITY);
        super::super::fingers::palm((hand.pos, hand.rot))
    }

    /// A collision mesh as a game streams it: a quad of two one-sided triangles (no thickness).
    fn mesh_quad(p: &mut Physics, id: u32, corners: [[f32; 3]; 4]) {
        let [a, b, c, d] = corners;
        let tri = |x: [f32; 3], y: [f32; 3], z: [f32; 3]| ColTriangle { v: [x[0], x[1], x[2], y[0], y[1], y[2], z[0], z[1], z[2]] };
        let mut payload = bytemuck::bytes_of(&ColMesh { id, count: 2 }).to_vec();
        payload.extend_from_slice(bytemuck::bytes_of(&tri(a, b, c)));
        payload.extend_from_slice(bytemuck::bytes_of(&tri(a, c, d)));
        p.apply_collision(COL_MESH, &payload);
    }

    /// A hand driven the way the rig drives an empty hand, towards a controller held `behind` the
    /// surface, for two seconds: where it ends up.
    fn push_into(p: &mut Physics, start: V3, target: V3) -> Vec<V3> {
        let (h, _) = p.add_hand(0, start, Q::IDENTITY, test_palm(0));
        p.set_dt(1.0 / 90.0);
        let mut path = Vec::new();
        for _ in 0..180 {
            p.track(h, target, Q::IDENTITY, 1.0 / 90.0, &Drive::FREE);
            // (Turned as the controller is, whatever it meets.)
            p.set_rotation(h, Q::IDENTITY);
            p.step();
            path.push(p.pose(h).0);
        }
        path
    }

    #[test]
    fn a_hand_stays_on_the_near_side_of_a_thin_wall_and_floor() {
        // A wall at z = 1 and a floor at y = 0, as the game's triangle meshes (both windings).
        for flip in [false, true] {
            let mut p = Physics::default();
            let mut wall = [[-2.0, -1.0, 1.0], [2.0, -1.0, 1.0], [2.0, 3.0, 1.0], [-2.0, 3.0, 1.0]];
            let mut floor = [[-2.0, 0.0, -2.0], [2.0, 0.0, -2.0], [2.0, 0.0, 3.0], [-2.0, 0.0, 3.0]];
            if flip {
                wall.reverse();
                floor.reverse();
            }
            mesh_quad(&mut p, 1, wall);
            mesh_quad(&mut p, 2, floor);
            for depth in [0.05, 0.25, 0.4] {
                let path = push_into(&mut p, V3::new(0.0, 1.0, 0.5), V3::new(0.0, 1.0, 1.0 + depth));
                let worst = path.iter().map(|q| q.z).fold(f32::MIN, f32::max);
                assert!(worst < 1.0, "flip {flip}, {depth} m into the wall: the hand got to z {worst}");
                let path = push_into(&mut p, V3::new(0.5, 0.5, 0.0), V3::new(0.5, -depth, 0.0));
                let worst = path.iter().map(|q| q.y).fold(f32::MAX, f32::min);
                assert!(worst > 0.0, "flip {flip}, {depth} m into the floor: the hand got to y {worst}");
            }
        }
    }

    #[test]
    fn collision_mesh_messages_replace_and_remove() {
        let mut p = Physics::default();
        let mut payload = bytemuck::bytes_of(&ColMesh { id: 9, count: 1 }).to_vec();
        payload.extend_from_slice(bytemuck::bytes_of(&ColTriangle { v: [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0] }));
        p.apply_collision(COL_MESH, &payload);
        assert_eq!(p.triangles, 1);
        assert!(p.has_world());
        p.apply_collision(COL_MESH, bytemuck::bytes_of(&ColMesh { id: 9, count: 0 }));
        assert!(!p.has_world());
    }
}
