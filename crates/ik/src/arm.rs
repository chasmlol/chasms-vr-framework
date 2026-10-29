//! Arms: clavicle reach (FRIK/VRIK), the elbow swivel, and an analytic two-bone solve with
//! FRIK's flexion clamps.
//!
//! The swivel (the elbow's angle around the shoulder -> wrist axis) starts from VRArmIK's tuned
//! version of Parger's position model with Parger's singularity blend. Parger then corrects the
//! elbow once the wrist bends past its limits, and FRIK keeps the wrist twist inside a window;
//! here those become costs, and the swivel is the cheapest angle in a short 1-D search: stay near
//! the position model, keep the wrist and forearm twist inside their comfortable ranges, keep the
//! elbow out of the ribcage, and don't jump from last frame.
//!
//! The hand is never moved: the wrist lands exactly on the target or, out of reach, the arm
//! stretches a little and then leaves the hand where the controller is.

use glam::{Quat, Vec3};

use crate::math::{frame_yz, signed_angle, smoothing, smoothstep, wrap_angle, UP};
use crate::skeleton::Skeleton;
use crate::{BodyParams, Pose, Side};

/// Parger's fixed elbow direction for hands near the shoulder's vertical axis or behind it, in
/// shoulder space for the right arm (+X outward, +Y up, +Z forward).
const PARGER_FIXED_ELBOW: Vec3 = Vec3::new(0.133, -0.443, -0.886);
/// Parger blends towards it within 0.5 m of the shoulder's vertical axis, and over the first
/// 0.1 m behind the shoulder.
const SINGULAR_RADIUS: f32 = 0.5;
const SINGULAR_BEHIND: f32 = 0.1;
/// Comfortable ranges (degrees) of the wrist and forearm; beyond them the swivel pays a quadratic
/// cost. Typical joint ranges are flexion/extension ~70°, ulnar deviation ~30°, radial ~20° and
/// pronation/supination ~80° each way; the margins are est.
const WRIST_FLEX: f32 = 50.0;
const WRIST_ULNAR: f32 = 30.0;
const WRIST_RADIAL: f32 = 15.0;
const FOREARM_TWIST: f32 = 70.0;
/// Cost weights, per radian² past those ranges, against 1 for swinging the elbow 90° away from the
/// position model (est.).
const W_WRIST: f32 = 8.0;
const W_TWIST: f32 = 4.0;
const W_PREVIOUS: f32 = 0.3;
const W_TORSO: f32 = 20.0;
/// The swivel search runs from just above outward round to a little inward under the arm.
const SWIVEL_RANGE_DEG: [f32; 2] = [30.0, 215.0];
const SWIVEL_STEP_DEG: f32 = 10.0;
/// The forearm joint sits halfway along the forearm, so it shows half of the twist between the
/// elbow hinge and the hand (FRIK spreads forearm roll 0 / 0.5 / 0.5 over its twist bones).
const FOREARM_JOINT_TWIST: f32 = 0.5;
/// The elbow is kept out of a box around the ribcage (upper chest space, metres at reference
/// size): half width, back, front, bottom.
const TORSO_BOX: [f32; 4] = [0.14, -0.12, 0.11, -0.55];

#[derive(Clone, Copy, Debug)]
pub struct ArmPose {
    pub clavicle_root: Vec3,
    pub shoulder: Vec3,
    pub elbow: Vec3,
    /// Canonical frames: clavicle, upper arm, lower arm, hand.
    pub rot: [Quat; 4],
    pub reached: bool,
}

/// Swivel state carried between frames.
#[derive(Clone, Copy, Debug, Default)]
pub struct ArmState {
    /// Smoothed elbow direction, in upper chest space.
    pole: Option<Vec3>,
}

pub struct ArmInput {
    pub side: Side,
    pub chest_pos: Vec3,
    pub chest_rot: Quat,
    /// Hand joint target: the wrist position and the hand's canonical frame.
    pub target: Pose,
    /// Where the headset tracks the elbow, when it does: the elbow points the way the player's
    /// does (the arm's own lengths kept).
    pub elbow: Option<Vec3>,
}

/// Swivel frame around the shoulder -> wrist axis: elbow direction e(α) = u cos α + o sin α, so
/// α = 0 points up, 90° outward and 180° down.
struct SwivelFrame {
    u: Vec3,
    o: Vec3,
}

impl SwivelFrame {
    fn new(axis: Vec3, up: Vec3, forward: Vec3, outward: Vec3) -> Self {
        // When the arm hangs (or points straight up) "up" around it is undefined; leaning it
        // forward then makes "down" point back for a hanging arm, where elbows go.
        let lean = -axis.dot(up) * 0.2;
        let u = (up.reject_from(axis) + forward.reject_from(axis) * lean)
            .try_normalize()
            .unwrap_or_else(|| axis.any_orthonormal_vector());
        // Outward stays outward even with the hand behind the body; with the arm itself pointing
        // outward, backward takes over.
        let o = (outward.reject_from(axis) - forward.reject_from(axis) * 0.3).reject_from(u);
        let o = o.try_normalize().unwrap_or_else(|| {
            let c = u.cross(axis);
            if c.dot(outward) >= 0.0 { c } else { -c }
        });
        SwivelFrame { u, o }
    }

    fn dir(&self, alpha: f32) -> Vec3 {
        self.u * alpha.cos() + self.o * alpha.sin()
    }

    fn angle(&self, e: Vec3) -> f32 {
        e.dot(self.o).atan2(e.dot(self.u))
    }
}

pub struct TwoBone {
    pub elbow: Vec3,
    /// Unit direction of the forearm (towards the wrist end of the bone, which is the target
    /// unless the hand was left floating).
    pub forearm: Vec3,
    /// The way the forearm folds, square to the upper arm: from the pole, so a straight arm has
    /// one too (its roll doesn't spin).
    pub fold: Vec3,
    pub reached: bool,
}

/// Law-of-cosines two-bone solve. Flexion stays inside `[min_flex, max_flex]`: at full reach the
/// bones stretch (up to `max_stretch`, then the hand floats), near the shoulder they shorten.
pub fn two_bone(
    shoulder: Vec3,
    target: Vec3,
    lengths: [f32; 2],
    pole: Vec3,
    flex: [f32; 2],
    max_stretch: f32,
) -> TwoBone {
    let [a, b] = lengths;
    let to = target - shoulder;
    let d = to.length();
    let axis = to.try_normalize().unwrap_or(-UP);
    let reach = |f: f32| (a * a + b * b + 2.0 * a * b * f.cos()).sqrt();
    let (d_max, d_min) = (reach(flex[0]), reach(flex[1]));
    let (k, d_eff) = if d > d_max {
        let k = (d / d_max).min(max_stretch);
        (k, d_max * k)
    } else if d < d_min {
        ((d / d_min).max(0.05), d.max(d_min * 0.05))
    } else {
        (1.0, d)
    };
    let (a, b) = (a * k, b * k);
    let cos_shoulder = ((a * a + d_eff * d_eff - b * b) / (2.0 * a * d_eff)).clamp(-1.0, 1.0);
    let side = pole.reject_from(axis).try_normalize().unwrap_or_else(|| axis.any_orthonormal_vector());
    let sin_shoulder = (1.0 - cos_shoulder * cos_shoulder).sqrt();
    let elbow = shoulder + (axis * cos_shoulder + side * sin_shoulder) * a;
    let wrist = shoulder + axis * d_eff;
    TwoBone {
        elbow,
        forearm: (wrist - elbow).normalize_or(axis),
        fold: (axis * sin_shoulder - side * cos_shoulder).normalize_or(-side),
        reached: d <= (lengths[0] + lengths[1]) * 1.001,
    }
}

/// How far from the shoulder an arm `length` long bends as if the hand were, reaching `d` from
/// where the shoulder rests: `d`, until `ease` short of full reach; from there it eases (with the
/// same slope at its start and none at its end) onto full reach, `ease` past it, and stays there.
/// The shoulder goes the rest of the way.
fn eased_reach(d: f32, length: f32, ease: f32) -> f32 {
    if ease > 0.0 && d > length - ease {
        let u = (d - (length - ease)).min(2.0 * ease);
        length - ease + u - u * u / (4.0 * ease)
    } else {
        d.min(length)
    }
}

pub fn solve(p: &BodyParams, skel: &Skeleton, dt: f32, state: &mut ArmState, input: &ArmInput) -> ArmPose {
    let sign = if input.side == Side::Left { -1.0 } else { 1.0 };
    let q = input.chest_rot;
    let (up, forward, outward) = (q * UP, q * Vec3::Z, q * Vec3::X * sign);
    let wrist = input.target.pos;
    let arm = skel.arm_length();

    // The shoulder: where it rests while the hand is well within the arm's reach. Near full reach
    // and past it, it goes the hand's way as a shoulder blade does (forward or across the chest
    // most, up in a shrug, back and down less, out least; `BodyParams::shoulder_reach`). Over the
    // last `arm_ease` of reach either side of full it takes a growing share of the reaching, so the
    // elbow straightens at a steady rate (a chain's bend changes ever faster as it straightens: it
    // would snap straight), then all of it; the bones keep their length. Only past the shoulder's
    // own reach does the arm stretch. The clavicle points at it.
    let clavicle_root = input.chest_pos + q * Vec3::new(sign * skel.clavicle_root, skel.shoulder_up, 0.0);
    let rest_shoulder = input.chest_pos + q * Vec3::new(sign * skel.shoulder_half, skel.shoulder_up, 0.0);
    let to_hand = wrist - rest_shoulder;
    let reaching = to_hand.length();
    let bend_at = eased_reach(reaching, arm, (p.arm_ease * arm).clamp(0.0, 0.25 * arm));
    let slide = if reaching > bend_at {
        // (Chest space, outward +X on either side.)
        let l = q.inverse() * to_hand.normalize_or_zero();
        let (x, y, z) = (l.x * sign, l.y, l.z);
        let [fwd, back, rise, drop, out] = p.shoulder_reach;
        let sq = |v: f32| v * v;
        let reach = skel.scale
            * (sq(z.max(0.0)) * fwd + sq(x.min(0.0)) * fwd + sq(z.min(0.0)) * back + sq(y.max(0.0)) * rise + sq(y.min(0.0)) * drop + sq(x.max(0.0)) * out);
        (reaching - bend_at).min(reach)
    } else {
        0.0
    };
    let shoulder = rest_shoulder + to_hand.normalize_or_zero() * slide;
    let clavicle_dir = (shoulder - clavicle_root).normalize_or(outward);
    let clavicle_rot = frame_yz(clavicle_dir, forward, up);
    let hand_rot = input.target.rot;
    let lengths = [skel.upper_arm, skel.lower_arm];

    // FRIK treats a hand this far away as lost tracking: the arm hangs and the hand stays put.
    if shoulder.distance(wrist) > p.arm_tracking_loss * arm {
        state.pole = None;
        let elbow = shoulder - UP * skel.upper_arm;
        let upper = frame_yz(-UP, forward, -outward);
        let lower = frame_yz(-UP, forward, -outward);
        return ArmPose { clavicle_root, shoulder, elbow, rot: [clavicle_rot, upper, lower, hand_rot], reached: false };
    }

    let flex = [p.elbow_min_flexion_deg.to_radians(), p.elbow_max_flexion_deg.to_radians()];
    let axis = (wrist - shoulder).normalize_or(-UP);
    let frame = SwivelFrame::new(axis, up, forward, outward);
    let previous = state.pole.map(|local| q * local);
    // How straight the arm is: 1 with the elbow all but straight, 0 once it's bent past 30°. Only
    // there does the hand's twist turn the elbow: bent more, where the elbow points shows, and the
    // position model has it.
    let (a, b) = (lengths[0], lengths[1]);
    let d = shoulder.distance(wrist).min(a + b);
    let flexion = ((d * d - a * a - b * b) / (2.0 * a * b)).clamp(-1.0, 1.0).acos();
    let straight = smoothstep(30f32.to_radians(), 8f32.to_radians(), flexion);
    let target_dir = swivel_target(skel, &frame, sign, rest_shoulder, previous, straight, input, |e| {
        two_bone(shoulder, wrist, lengths, e, flex, p.arm_stretch)
    });
    // The headset's own tracking of the elbow: its way out from the shoulder-to-wrist line, the
    // model's where the arm is nearly straight (the way is lost in the tracking's noise there).
    let target_dir = match input.elbow {
        Some(e) => {
            let off = (e - shoulder).reject_from(axis);
            let k = smoothstep(0.01, 0.04, off.length());
            target_dir.lerp(off.normalize_or(target_dir), k).normalize_or(target_dir)
        }
        None => target_dir,
    };

    // Smooth the swivel, never the hand: the elbow direction is filtered in chest space.
    let local = q.inverse() * target_dir;
    let pole_local = match state.pole {
        Some(prev) => prev.lerp(local, smoothing(dt, p.swivel_tau)).normalize_or(local),
        None => local,
    };
    state.pole = Some(pole_local);
    let pole = q * pole_local;
    let pole = if pole.reject_from(axis).length_squared() > 1e-6 { pole } else { target_dir };

    let solved = two_bone(shoulder, wrist, lengths, pole, flex, p.arm_stretch);
    let upper_dir = (solved.elbow - shoulder).normalize_or(-UP);
    // The upper arm's +Z faces the way the forearm folds (forward in the T-pose), straight or not.
    let upper = frame_yz(upper_dir, solved.fold, forward);
    // The forearm joint without twist is the upper arm frame turned through the elbow hinge;
    // it then takes part of the hand's twist about the forearm.
    let hinge = Quat::from_rotation_arc(upper_dir, solved.forearm);
    let untwisted = hinge * (upper * Vec3::Z);
    let twist = signed_angle(untwisted, hand_rot * Vec3::Z, solved.forearm) * FOREARM_JOINT_TWIST;
    let lower_z = Quat::from_axis_angle(solved.forearm, twist) * untwisted;
    let lower = frame_yz(solved.forearm, lower_z, forward);
    ArmPose { clavicle_root, shoulder, elbow: solved.elbow, rot: [clavicle_rot, upper, lower, hand_rot], reached: solved.reached }
}

/// The elbow direction this frame, before smoothing.
#[allow(clippy::too_many_arguments)]
fn swivel_target(
    skel: &Skeleton,
    frame: &SwivelFrame,
    sign: f32,
    rest_shoulder: Vec3,
    previous: Option<Vec3>,
    straight: f32,
    input: &ArmInput,
    solve: impl Fn(Vec3) -> TwoBone,
) -> Vec3 {
    let wrist = input.target.pos;
    let chest_rot = input.chest_rot;
    let swivel_axis = frame.u.cross(frame.o);
    // 1. VRArmIK's tuned version of Parger's model, from the hand position in shoulder space
    //    normalized by arm length.
    let rel = chest_rot.inverse() * (wrist - rest_shoulder);
    let h = rel / skel.arm_length();
    let mut alpha = 135.0 - 60.0 * h.y;
    alpha += if h.y > 0.0 {
        260.0 * (0.6 - h.z).max(0.0) * h.y
    } else {
        -100.0 * (0.6 - h.z).max(0.0) * -h.y
    };
    alpha -= 50.0 * (-h.x * sign + 0.1).max(0.0);
    let mut prior = frame.dir(soft_clamp(alpha, 13.0, 175.0, 10.0).to_radians());

    // 2. Parger's singularity correction: near the shoulder's vertical axis and behind the
    //    shoulder the position model is unreliable, so blend towards a fixed direction.
    let radius = Vec3::new(rel.x, 0.0, rel.z).length() / skel.scale;
    let blend = (1.0 - radius / SINGULAR_RADIUS).max((-rel.z / skel.scale / SINGULAR_BEHIND).clamp(0.0, 1.0));
    if blend > 0.0 {
        let fixed = chest_rot * Vec3::new(PARGER_FIXED_ELBOW.x * sign, PARGER_FIXED_ELBOW.y, PARGER_FIXED_ELBOW.z);
        let fixed = fixed.reject_from(swivel_axis).normalize_or(prior);
        prior = prior.lerp(fixed, blend.clamp(0.0, 1.0)).normalize_or(prior);
    }
    let (hand_x, hand_y, hand_z) = (input.target.rot * Vec3::X, input.target.rot * UP, input.target.rot * Vec3::Z);
    let mut prior = frame.angle(prior);
    // A nearly straight arm's elbow circle shrinks to a point and the position model says
    // nothing, so there the swivel follows the hand's twist instead: the inside of the elbow
    // towards the thumb, as in a handshake grip (an arm held out palm down points its elbow back).
    let thumb = Quat::from_rotation_arc(hand_y, solve(frame.dir(prior)).forearm) * hand_z;
    let untwisted = frame.angle((-thumb).reject_from(swivel_axis));
    prior += wrap_angle(untwisted - prior) * straight;
    let previous = previous.filter(|e| e.reject_from(swivel_axis).length_squared() > 1e-4).map(|e| frame.angle(e));

    // 3. The cheapest swivel near the prior that keeps the wrist comfortable.
    let excess = |angle: f32, limit: f32| (angle - limit.to_radians()).max(0.0);
    let cost = |alpha: f32| {
        let e = frame.dir(alpha);
        let arm = solve(e);
        let f = arm.forearm;
        // Wrist bend: where the forearm points, seen from the hand. Towards the thumb side means
        // the hand is bent towards the pinky (ulnar deviation).
        let along = f.dot(hand_y);
        let flex = f.dot(hand_x).atan2(along).abs();
        let deviation = f.dot(hand_z).atan2(along);
        let deviation = if deviation > 0.0 { excess(deviation, WRIST_ULNAR) } else { excess(-deviation, WRIST_RADIAL) };
        // Forearm twist: the thumb, swung onto the forearm, against the inside of the elbow
        // (they line up in a handshake grip).
        let thumb = Quat::from_rotation_arc(hand_y, f) * hand_z;
        let twist = signed_angle(-e, thumb, f).abs();
        let mut c = 1.0 - (alpha - prior).cos()
            + W_WRIST * (excess(flex, WRIST_FLEX).powi(2) + deviation.powi(2))
            + W_TWIST * excess(twist, FOREARM_TWIST).powi(2);
        if let Some(prev) = previous {
            c += W_PREVIOUS * (1.0 - (alpha - prev).cos());
        }
        if inside_torso(skel, chest_rot.inverse() * (arm.elbow - input.chest_pos)) {
            c += W_TORSO;
        }
        c
    };
    let [lo, hi] = SWIVEL_RANGE_DEG.map(f32::to_radians);
    let step = SWIVEL_STEP_DEG.to_radians();
    let count = ((hi - lo) / step).round() as usize;
    let (mut best, mut best_cost) = (lo, f32::MAX);
    for k in 0..=count {
        let a = lo + step * k as f32;
        let c = cost(a);
        if c < best_cost {
            (best, best_cost) = (a, c);
        }
    }
    // Refine with a parabola through the best sample and its neighbours.
    let (before, after) = (cost(best - step), cost(best + step));
    let curve = before - 2.0 * best_cost + after;
    if curve > 1e-6 {
        let refined = (best + 0.5 * step * (before - after) / curve).clamp(lo, hi);
        if cost(refined) < best_cost {
            best = refined;
        }
    }
    frame.dir(best)
}

fn inside_torso(skel: &Skeleton, local: Vec3) -> bool {
    let [half, back, front, bottom] = TORSO_BOX.map(|v| v * skel.scale);
    local.x.abs() < half && local.z > back && local.z < front && local.y > bottom && local.y < skel.shoulder_up
}

/// Clamp with a smooth knee of width `knee` inside each limit (VRArmIK soft-clamps the swivel).
fn soft_clamp(x: f32, lo: f32, hi: f32, knee: f32) -> f32 {
    let soft = |over: f32| knee * (1.0 - (-over / knee).exp());
    if x > hi - knee {
        hi - knee + soft(x - (hi - knee))
    } else if x < lo + knee {
        lo + knee - soft(lo + knee - x)
    } else {
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flexion(shoulder: Vec3, elbow: Vec3, wrist: Vec3) -> f32 {
        (elbow - shoulder).angle_between(wrist - elbow).to_degrees()
    }

    #[test]
    fn two_bone_lands_on_the_target() {
        let s = Vec3::ZERO;
        let flex = [10f32.to_radians(), 145f32.to_radians()];
        let w = Vec3::new(0.3, -0.2, 0.25);
        let r = two_bone(s, w, [0.3, 0.3], Vec3::NEG_Y, flex, 1.05);
        assert!(r.reached);
        assert!((r.elbow.distance(s) - 0.3).abs() < 1e-5);
        assert!((r.elbow.distance(w) - 0.3).abs() < 1e-5);
        assert!((r.forearm - (w - r.elbow).normalize()).length() < 1e-5);
        // The elbow bends towards the pole.
        assert!(r.elbow.y < w.y * 0.5);
    }

    #[test]
    fn flexion_clamps_hold() {
        let s = Vec3::ZERO;
        let flex = [10f32.to_radians(), 145f32.to_radians()];
        // Fully extended: still 10° of bend, bones stretched slightly to reach.
        let w = Vec3::new(0.6, 0.0, 0.0);
        let r = two_bone(s, w, [0.3, 0.3], Vec3::NEG_Y, flex, 1.05);
        assert!((flexion(s, r.elbow, w) - 10.0).abs() < 0.1);
        assert!(r.reached);
        // Right at the shoulder: at most 145°.
        let w = Vec3::new(0.05, 0.0, 0.0);
        let r = two_bone(s, w, [0.3, 0.3], Vec3::NEG_Y, flex, 1.05);
        assert!(flexion(s, r.elbow, w) < 145.1);
        // Out of reach: stretched, then floating; never NaN.
        let w = Vec3::new(0.62, 0.0, 0.0);
        assert!(!two_bone(s, w, [0.3, 0.3], Vec3::NEG_Y, flex, 1.05).reached);
        let r = two_bone(s, Vec3::new(2.0, 0.0, 0.0), [0.3, 0.3], Vec3::X, flex, 1.05);
        assert!(r.elbow.is_finite() && !r.reached);
        let r = two_bone(s, s, [0.3, 0.3], Vec3::NEG_Y, flex, 1.05);
        assert!(r.elbow.is_finite() && r.forearm.is_finite());
    }

    #[test]
    fn soft_clamp_is_monotonic_and_bounded() {
        let mut last = f32::MIN;
        for i in -100..400 {
            let v = soft_clamp(i as f32, 13.0, 175.0, 10.0);
            assert!(v >= last && (13.0..=175.0).contains(&v));
            last = v;
        }
        assert_eq!(soft_clamp(90.0, 13.0, 175.0, 10.0), 90.0);
    }
}
