//! Every tunable of the body solver, with defaults from the research's Tier 5 table
//! (`reports/VR player controller design.md`) and the notes in
//! `research_notes/VR player controller design/full_body_ik.md`. "Est." marks values no source
//! publishes. Lengths are metres for the reference body and scale with the player; angles are
//! degrees; time constants are seconds.

use glam::{Quat, Vec3};

use crate::{Pose, Side};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyParams {
    // ---- proportions ----
    /// The head joint, where the skull turns on the neck (the atlanto-occipital joint), this far
    /// below and behind the eyes at the reference size: the ear canal is 1.7 cm below the eyes
    /// (ANSUR II, 6,068 adults) and the occipital condyles some 3 cm below that, 7-9 cm back.
    /// (Parger's 0.13 m pivot, the LibOVR neck model's, is lower down the neck: the point a whole
    /// nod turns about, which here comes from the neck bending as the head does.)
    pub head_pivot: [f32; 2],
    /// Between the shoulder joints (the glenohumeral centres): 0.21 of stature, the biacromial
    /// breadth (ANSUR II: 0.237 of stature in men, 0.224 in women) less 1-2 cm each side. Parger's
    /// 0.31 m is a fifth narrower than a real body's, which puts the arms in too close to the neck.
    pub shoulder_width: f32,

    // ---- torso yaw ----
    /// The hands may turn the torso at most this far from the head (VRArmIK 80°, Parger 90°,
    /// FRIK 50°).
    pub torso_yaw_clamp_deg: f32,
    /// Fraction of the hand-derived yaw the torso takes; the rest follows the head (FRIK 0.7).
    pub torso_yaw_blend: f32,
    /// Torso yaw smoothing while at least one hand is tracked (est.).
    pub torso_yaw_tau: f32,
    /// With no hands the torso trails the head this slowly (shock2quest's body anchor, 0.5 s).
    pub torso_yaw_tau_head_only: f32,
    /// A hand this close to the eyes says nothing about the torso; its weight ramps up to full at
    /// twice the distance (FRIK zeroes hands within about 14 cm of the HMD).
    pub face_radius: f32,
    /// A hand's weight falls to zero this far above the eyes (FRIK: 0.05 per unit, about 29 cm).
    pub above_head_range: f32,
    /// Crossed hands lose weight, reaching zero at this much crossing (FRIK: 0.02 per unit,
    /// about 0.7 m).
    pub hand_cross_range: f32,
    /// A frame-to-frame jump of the hand yaw inside this window means both hands went behind the
    /// body, so the estimate is flipped back (VRArmIK 150–210°).
    pub flip_jump_min_deg: f32,
    pub flip_jump_max_deg: f32,
    /// The feet and hips turn only once the torso leaves this dead zone (VRIK 25° standing; VRIK
    /// Animated 10° while moving).
    pub root_dead_zone_standing_deg: f32,
    pub root_dead_zone_moving_deg: f32,
    /// Head or stick speed above which the body counts as moving (VRIK Animated move threshold).
    pub moving_speed: f32,

    // ---- spine ----
    /// Spine pitch per relative head drop: pitch = drop/h0 × this + `spine_bend_head_pitch` ×
    /// head pitch (Parger 135.3° and 0.333, FRIK 105.3° and 0.1; the defaults sit between).
    pub spine_bend_deg: f32,
    pub spine_bend_head_pitch: f32,
    /// The drop term applies in full only when the gaze is this far down, and not at all below
    /// `bend_gaze_start_deg`: dropping with a level gaze is a crouch (knees), dropping while
    /// looking down is a bend (spine). The research's crouch-versus-bend inference; angles est.
    pub bend_gaze_start_deg: f32,
    pub bend_gaze_full_deg: f32,
    /// In a crouch the hips move back by this fraction of the relative head drop times the spine
    /// length, leaning the spine a little (VRIK `moveBodyBackWhenCrouching` 0.5).
    pub pelvis_back: f32,
    /// The spine formulas see the head no lower than this (VRIK `minHeadHeight`).
    pub min_head_height: f32,
    /// Spine pitch smoothing (est.).
    pub spine_tau: f32,

    // ---- arms ----
    /// The elbow keeps at least this bend at full reach (the bones stretch instead). FRIK keeps 10°
    /// so a straight arm's roll doesn't spin; here the roll comes from the elbow's swivel
    /// (`TwoBone::fold`), so a player's straight arm is drawn straight.
    pub elbow_min_flexion_deg: f32,
    /// Near the shoulder the elbow bends at most this much and the arm shortens instead of
    /// flipping (FRIK 145°).
    pub elbow_max_flexion_deg: f32,
    /// How far the shoulder goes towards a hand past the arm's full reach (metres at reference
    /// size): forward or across the chest, back, up, down, out. A shoulder blade slides forward
    /// round the ribs some 10 cm reaching, and with the trunk leaning and turning into a reach
    /// further (est.); it shrugs up less and hardly moves outward.
    pub shoulder_reach: [f32; 5],
    /// The last stretch to a straight arm, a share of its length either side of full reach: over
    /// it the shoulder takes a growing share of the reaching, so the elbow straightens at a steady
    /// rate rather than snapping straight. The arms are measured so it's straight where the
    /// player's own arm is.
    pub arm_ease: f32,
    /// Elbow swivel smoothing (FRIK 0.75 per 90 Hz frame ≈ 39 ms; VRArmIK 80 ms). Only the swivel
    /// is smoothed, never the hand.
    pub swivel_tau: f32,
    /// Out of reach the bones stretch up to this factor, both alike, the elbow kept bent its least
    /// (FRIK's way); beyond it the hand stays on the controller anyway and the arm reports
    /// `reached = false`. Generous: the drawn arm lengthens evenly along both bones (the skin fitted
    /// to them), so an arm a little short of the player's real one reaches without the forearm
    /// alone making up the difference.
    pub arm_stretch: f32,
    /// A hand further than this many arm lengths from the shoulder is tracking loss: the arm hangs
    /// (FRIK 2.25 L).
    pub arm_tracking_loss: f32,

    // ---- legs ----
    /// Distance between the feet when standing (VRIK footDistance 0.3).
    pub foot_spacing: f32,
    /// A foot this far from its place under the body steps whatever the balance (VRIK steps at
    /// 0.4 m, which has feet stepping when real feet stay planted: people stand with their feet
    /// apart and staggered and sway over them; this is only a safety net, est.).
    pub step_threshold: f32,
    /// A foot steps once its ankle is further than this share of the leg's length from under its
    /// hip joint (the leg leaning some 35°; est.).
    pub step_reach: f32,
    /// A foot steps once the hips have twisted this far from it (the hip turns some 45° each way
    /// in standing; VRIK steps at 60° of root turn).
    pub step_angle_deg: f32,
    /// Step duration range (VRIK step speed 3–4.5 per second): urgent steps are faster.
    pub step_time_min: f32,
    pub step_time_max: f32,
    /// A step never lands nearer the other foot than this, nor across it (VRIK 0.25 m sphere).
    pub leg_cross_guard: f32,
    /// Peak foot lift during a step (est.; FRIK lifts up to about 13 cm, UE's Locomotor 6 cm).
    pub step_height: f32,
    /// The feet stay planted while the centre of mass is over them: within this much of the outline
    /// round both feet, heels to toes (est.).
    pub support_margin: f32,
    /// Once the body has been still for `settle_delay`, a foot further than this from its spot
    /// steps under the body (est.; VRIK leaves feet wherever the last balance step put them, and
    /// people at rest stand as their feet are unless they're well out of place).
    pub settle_distance: f32,
    pub settle_delay: f32,
    /// Stick locomotion walks once faster than this and stops below `gait_stop_speed` (FRIK 35
    /// and 20 units/s, about 0.5 and 0.29 m/s).
    pub gait_start_speed: f32,
    pub gait_stop_speed: f32,
    /// Step time = clamp(cos(speed / 2 m/s), min, max) (FRIK's cos(speed/140 units) with
    /// 0.28–0.50 s, units converted at about 70 per metre).
    pub gait_step_time_min: f32,
    pub gait_step_time_max: f32,
    /// Walking narrows the stance to this fraction of `foot_spacing` (FRIK pulls the feet 30%
    /// together each, leaving 40%).
    pub gait_spacing: f32,
    /// Knee bend limit (est.).
    pub knee_max_flexion_deg: f32,
    /// A planted foot whose ankle is out of reach rises onto its toes by up to this much before
    /// the hips come down (VRIK anchors the toes so the avatar can rise onto them; angle est.).
    pub heel_lift_max_deg: f32,
    /// Legs stretch by up to this factor before the hips are pulled down (est.).
    pub leg_stretch: f32,
    /// How fast the hips rise back after being pulled down to keep the feet planted (est.).
    pub hip_release_tau: f32,

    // ---- seated ----
    /// Seat height for seated mode; the virtual standing eye height is the seat plus the upper
    /// body, so sitting reads as upright rather than as a deep crouch.
    pub seat_height: f32,

    // ---- hands ----
    pub hand_offsets: HandOffsets,
    /// The forearm's share of the arm, shoulder to wrist (the drawn model's proportions: its skin
    /// then fits the arm without one segment stretching more than the other).
    pub forearm_share: f32,

    // ---- filters ----
    /// Nominal frame time; filters clamp dt to `max_dt_frames` of these (FRIK clamps to 3).
    pub nominal_dt: f32,
    pub max_dt_frames: f32,
}

impl Default for BodyParams {
    fn default() -> Self {
        BodyParams {
            head_pivot: [0.046, 0.087],
            shoulder_width: 0.38,

            torso_yaw_clamp_deg: 80.0,
            torso_yaw_blend: 0.7,
            torso_yaw_tau: 0.08,
            torso_yaw_tau_head_only: 0.5,
            face_radius: 0.15,
            above_head_range: 0.29,
            hand_cross_range: 0.7,
            flip_jump_min_deg: 150.0,
            flip_jump_max_deg: 210.0,
            root_dead_zone_standing_deg: 25.0,
            root_dead_zone_moving_deg: 10.0,
            moving_speed: 0.3,

            spine_bend_deg: 120.0,
            spine_bend_head_pitch: 0.2,
            bend_gaze_start_deg: 10.0,
            bend_gaze_full_deg: 40.0,
            pelvis_back: 0.5,
            min_head_height: 0.8,
            spine_tau: 0.05,

            elbow_min_flexion_deg: 0.0,
            elbow_max_flexion_deg: 145.0,
            shoulder_reach: [0.15, 0.05, 0.10, 0.04, 0.03],
            arm_ease: 0.02,
            swivel_tau: 0.06,
            arm_stretch: 1.3,
            arm_tracking_loss: 2.25,

            foot_spacing: 0.3,
            step_threshold: 0.8,
            step_reach: 0.55,
            step_angle_deg: 50.0,
            step_time_min: 0.22,
            step_time_max: 0.33,
            leg_cross_guard: 0.25,
            step_height: 0.08,
            support_margin: 0.03,
            settle_distance: 0.10,
            settle_delay: 1.0,
            gait_start_speed: 0.5,
            gait_stop_speed: 0.29,
            gait_step_time_min: 0.28,
            gait_step_time_max: 0.5,
            gait_spacing: 0.4,
            knee_max_flexion_deg: 150.0,
            heel_lift_max_deg: 35.0,
            leg_stretch: 1.02,
            hip_release_tau: 0.15,

            seat_height: 0.45,

            hand_offsets: HandOffsets::default(),
            forearm_share: 0.5,

            nominal_dt: 1.0 / 90.0,
            max_dt_frames: 3.0,
        }
    }
}

/// The measurements that place the hand joint on a controller. Tune these on the headset: they
/// decide whether the rendered hand matches the real one, and the elbow heuristics read the
/// hand's orientation, so they move the elbow too.
///
/// Grip space (the controller's grip pose, tracking-space convention): origin at the palm
/// centre; holding the controller with the arm forward and identity rotation, +Z runs forward
/// along the fist (pinky to thumb), +Y up and +X right, for both hands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HandGeometry {
    /// Palm centre to the wrist joint, back along the hand (7–9 cm).
    pub wrist_distance: f32,
    /// The wrist sits this far towards the back of the hand from the grip axis, which runs
    /// through the fist in front of the palm (est.).
    pub palm_depth: f32,
    /// A fist holds the handle obliquely: the fingers (wrist to knuckles) point this far from
    /// straight down the grip's -Y towards its +Z (est.).
    pub knuckle_angle_deg: f32,
    /// Extra rotation of the right hand in grip axes (yaw about +Y, pitch about +X, roll about
    /// +Z), applied in that order; the left hand gets the mirror image. Per-controller
    /// corrections go here (for example the Quest 3's ~15° outward point).
    pub yaw_deg: f32,
    pub pitch_deg: f32,
    pub roll_deg: f32,
}

impl Default for HandGeometry {
    fn default() -> Self {
        HandGeometry {
            wrist_distance: 0.08,
            palm_depth: 0.02,
            knuckle_angle_deg: 20.0,
            yaw_deg: 0.0,
            pitch_deg: 0.0,
            roll_deg: 0.0,
        }
    }
}

/// Grip pose -> hand joint pose, per side. `grip_to_hand[side]` is the hand joint in grip space:
/// `pos` is the wrist relative to the palm centre and `rot` maps the hand's canonical axes (+Y to
/// the fingers, +Z the thumb side, X = Y × Z) into grip axes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HandOffsets {
    /// [left, right].
    pub grip_to_hand: [Pose; 2],
}

impl Default for HandOffsets {
    fn default() -> Self {
        HandOffsets::from_geometry(HandGeometry::default())
    }
}

impl HandOffsets {
    /// Builds both sides from one set of measurements: the right hand directly, the left as its
    /// mirror image through the YZ plane.
    pub fn from_geometry(g: HandGeometry) -> Self {
        // At zero knuckle angle the right hand's fingers point down the grip's -Y, its thumb
        // along +Z and its palm (canonical +X for the right hand) faces -X: a half turn about Z.
        let base = Quat::from_rotation_x(-g.knuckle_angle_deg.to_radians()) * Quat::from_rotation_z(std::f32::consts::PI);
        let user = Quat::from_rotation_y(g.yaw_deg.to_radians())
            * Quat::from_rotation_x(g.pitch_deg.to_radians())
            * Quat::from_rotation_z(g.roll_deg.to_radians());
        let rot = (user * base).normalize();
        // Back along the fingers to the wrist, and over towards the back of the hand (the right
        // hand's canonical -X).
        let pos = rot * Vec3::new(-g.palm_depth, -g.wrist_distance, 0.0);
        HandOffsets::mirrored(Pose { pos, rot })
    }

    /// Both sides from the right hand's offset.
    pub fn mirrored(right: Pose) -> Self {
        let left = Pose {
            pos: Vec3::new(-right.pos.x, right.pos.y, right.pos.z),
            rot: Quat::from_xyzw(right.rot.x, -right.rot.y, -right.rot.z, right.rot.w),
        };
        HandOffsets { grip_to_hand: [left, right] }
    }

    /// The hand joint for a grip pose: the wrist position and the absolute orientation of the
    /// hand's canonical frame in tracking space.
    pub fn hand_frame(&self, side: Side, grip: Pose) -> Pose {
        let o = self.grip_to_hand[side as usize];
        Pose { pos: grip.pos + grip.rot * o.pos, rot: (grip.rot * o.rot).normalize() }
    }

    /// The grip pose whose hand joint is `hand`: the inverse of [`HandOffsets::hand_frame`], for
    /// putting the hand exactly somewhere (a game's own pose on a gun).
    pub fn grip_for(&self, side: Side, hand: Pose) -> Pose {
        let o = self.grip_to_hand[side as usize];
        let rot = (hand.rot * o.rot.inverse()).normalize();
        Pose { pos: hand.pos - rot * o.pos, rot }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_grip_is_a_hanging_fist() {
        // Arm hanging, fist holding a stick that points forward: fingers down, thumb forward,
        // wrist above the palm.
        let offsets = HandOffsets::from_geometry(HandGeometry { knuckle_angle_deg: 0.0, ..Default::default() });
        let grip = Pose { pos: Vec3::ZERO, rot: Quat::IDENTITY };
        for side in [Side::Left, Side::Right] {
            let h = offsets.hand_frame(side, grip);
            assert!((h.rot * Vec3::Y - Vec3::NEG_Y).length() < 1e-5, "{side:?} fingers down");
            assert!((h.rot * Vec3::Z - Vec3::Z).length() < 1e-5, "{side:?} thumb forward");
            assert!((h.pos.y - 0.08).abs() < 1e-5);
        }
        // The right palm faces left (+X of the right hand is out of the palm), the left palm
        // faces right (+X of the left hand is out of its back).
        let r = offsets.hand_frame(Side::Right, grip);
        let l = offsets.hand_frame(Side::Left, grip);
        assert!((r.rot * Vec3::X - Vec3::NEG_X).length() < 1e-5);
        assert!((l.rot * Vec3::X - Vec3::NEG_X).length() < 1e-5);
        // Wrists sit towards the back of each hand: right of the right grip, left of the left.
        assert!(r.pos.x > 0.01 && l.pos.x < -0.01);
    }

    #[test]
    fn grip_for_inverts_hand_frame() {
        let offsets = HandOffsets::from_geometry(HandGeometry { yaw_deg: 12.0, roll_deg: -7.0, ..Default::default() });
        let hand = Pose { pos: Vec3::new(0.2, 1.3, 0.4), rot: Quat::from_euler(glam::EulerRot::YXZ, 0.4, -0.9, 0.3) };
        for side in [Side::Left, Side::Right] {
            let back = offsets.hand_frame(side, offsets.grip_for(side, hand));
            assert!(back.pos.distance(hand.pos) < 1e-5 && back.rot.angle_between(hand.rot) < 1e-4, "{side:?}");
        }
    }

    #[test]
    fn wrist_is_seven_to_nine_cm_from_the_palm() {
        let offsets = HandOffsets::default();
        for o in offsets.grip_to_hand {
            let d = o.pos.length();
            assert!((0.07..=0.09).contains(&d), "{d}");
        }
    }

    #[test]
    fn mirroring_is_a_reflection() {
        let g = HandGeometry { yaw_deg: 15.0, pitch_deg: -10.0, roll_deg: 5.0, ..Default::default() };
        let o = HandOffsets::from_geometry(g);
        let [l, r] = o.grip_to_hand;
        let m = |v: Vec3| Vec3::new(-v.x, v.y, v.z);
        // Reflect the right hand's axes; the canonical left frame flips X back (X = Y × Z).
        assert!((l.rot * Vec3::Y - m(r.rot * Vec3::Y)).length() < 1e-5);
        assert!((l.rot * Vec3::Z - m(r.rot * Vec3::Z)).length() < 1e-5);
        assert!((l.rot * Vec3::X + m(r.rot * Vec3::X)).length() < 1e-5);
    }
}
