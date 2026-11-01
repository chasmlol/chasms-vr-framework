//! A small learned prior for what the head and hands leave open: which way the chest and hips face
//! and which way each elbow points. Motion capture says these follow the head and hands in ways a
//! hand-written rule only roughly captures; MANIKIN (ECCV 2024) found the best split to be a
//! network predicting just these, with the limbs then solved exactly, so the hands stay on the
//! controllers. So the solver solves as before and only takes its priors from here: the chest's
//! yaw where the hands don't say, and each swivel search's centre.
//!
//! The network is a three-layer perceptron (ReLU) over 30 features of the head and hands, in the
//! head's yaw frame and scaled to the player, with their recent motion as running averages
//! (frame-rate independent, so the same at any headset rate). It's trained only on CMU's motion
//! capture, which is free for any use (`docs/design/ik.md`), mirrored left for right; the
//! evaluation's subjects and LaFAN1 are held out. The weights are `prior.bin`, made by the
//! evaluation tools (kept outside the repository, with the motion capture they train on).

use glam::{Quat, Vec3};

use crate::math::{flat, smoothing, wrap_angle, yaw_of, UP};
use crate::Pose;

/// Features per frame and outputs: the chest's yaw from the head (sin, cos), the hips' (sin,
/// cos), each elbow's way out from its shoulder-to-wrist line (x, y, z in the head's yaw frame).
pub const FEATURES: usize = 30;
pub const OUTPUTS: usize = 10;

/// The running averages' time constants (seconds): the wrists', the head's yaw, the head's
/// velocity. (A longer memory of the head's yaw, 2 s, had the chest leading the head round a
/// second after a turn: the people it learned from seldom turned holding still.)
const WRIST_TAU: f32 = 0.25;
const YAW_TAU: f32 = 0.5;
const VELOCITY_TAU: f32 = 0.15;
/// A hand further than this many arm lengths from the eyes is tracking gone wrong, not a pose:
/// no prediction.
const MAX_REACH: f32 = 2.5;
/// The widest layer the network may have (its activations live on the stack).
const WIDEST: usize = 256;
/// The network's inputs are kept within this many of the training set's standard deviations.
const MAX_SIGMA: f32 = 5.0;

/// What the prior sees each frame.
pub struct PriorInput {
    pub dt: f32,
    pub eyes: Vec3,
    pub head_rot: Quat,
    /// The player's standing eye height and arm length (shoulder to wrist).
    pub eye_height: f32,
    pub arm_length: f32,
    /// The hand joints [left, right].
    pub hands: [Option<Pose>; 2],
}

/// The prior's running averages.
#[derive(Clone, Copy, Debug, Default)]
pub struct PriorState {
    wrists: Option<[Vec3; 2]>,
    yaw: Option<f32>,
    eyes: Option<Vec3>,
    velocity: Vec3,
}

/// The head's yaw as the solver takes it: looking straight down, the top of the head still points
/// the way it faces.
pub fn head_yaw(head_rot: Quat) -> f32 {
    let fwd = head_rot * Vec3::Z;
    let facing = flat(fwd - head_rot * UP * fwd.y);
    if facing.length_squared() > 1e-8 { yaw_of(facing) } else { 0.0 }
}

impl PriorState {
    /// This frame's features, or None while a hand isn't tracked (the running averages carry on).
    pub fn features(&mut self, input: &PriorInput) -> Option<[f32; FEATURES]> {
        let dt = input.dt.max(0.0);
        let yaw = head_yaw(input.head_rot);
        let frame = Quat::from_rotation_y(-yaw);
        let fwd = input.head_rot * Vec3::Z;
        let pitch = (-fwd.y).clamp(-1.0, 1.0).asin();
        let roll = (input.head_rot * Vec3::X).y.clamp(-1.0, 1.0).asin();
        let arm = input.arm_length.max(0.1);

        let slow = self.yaw.get_or_insert(yaw);
        *slow = wrap_angle(*slow + wrap_angle(yaw - *slow) * smoothing(dt, YAW_TAU));
        let slow = *slow;
        if let (Some(last), true) = (self.eyes, dt > 0.0) {
            self.velocity = self.velocity.lerp((input.eyes - last) / dt, smoothing(dt, VELOCITY_TAU));
        }
        self.eyes = Some(input.eyes);
        let [Some(l), Some(r)] = input.hands else {
            self.wrists = None;
            return None;
        };
        if l.pos.distance(input.eyes) > MAX_REACH * arm || r.pos.distance(input.eyes) > MAX_REACH * arm {
            self.wrists = None;
            return None;
        }
        let hands = [l, r];
        let wrists = self.wrists.get_or_insert([l.pos, r.pos]);
        for (w, h) in wrists.iter_mut().zip(&hands) {
            *w = w.lerp(h.pos, smoothing(dt, WRIST_TAU));
        }
        let wrists = *wrists;

        let mut f = [0.0; FEATURES];
        f[0] = pitch;
        f[1] = roll;
        f[2] = input.eyes.y / input.eye_height.max(0.5);
        for (i, h) in hands.iter().enumerate() {
            let o = 3 + 12 * i;
            let at = frame * (h.pos - input.eyes) / arm;
            let (y, z) = (frame * (h.rot * Vec3::Y), frame * (h.rot * Vec3::Z));
            let moving = frame * (h.pos - wrists[i]) / arm;
            f[o..o + 12].copy_from_slice(&[at.x, at.y, at.z, y.x, y.y, y.z, z.x, z.y, z.z, moving.x, moving.y, moving.z]);
        }
        f[27] = wrap_angle(yaw - slow);
        let v = frame * self.velocity;
        f[28] = v.x;
        f[29] = v.z;
        Some(f)
    }
}

/// What the prior predicts.
#[derive(Clone, Copy, Debug)]
pub struct Prediction {
    /// The chest's and the hips' yaw (radians, tracking space).
    pub chest_yaw: f32,
    pub hips_yaw: f32,
    /// Each elbow's way out from its shoulder-to-wrist line (tracking space, unit).
    pub elbows: [Vec3; 2],
}

struct Layer {
    inputs: usize,
    weights: Vec<f32>,
    biases: Vec<f32>,
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn word(&mut self) -> Option<u32> {
        let w = self.bytes.get(self.at..self.at + 4)?;
        self.at += 4;
        Some(u32::from_le_bytes(w.try_into().ok()?))
    }

    fn floats(&mut self, n: usize) -> Option<Vec<f32>> {
        (0..n).map(|_| self.word().map(f32::from_bits)).collect()
    }
}

/// The trained network.
pub struct Prior {
    mean: [f32; FEATURES],
    scale: [f32; FEATURES],
    layers: Vec<Layer>,
}

/// The weights, as the evaluation tools' trainer writes them: "IKP1", the feature count,
/// the features' means and scales, then each layer's input and output counts, its weights (output
/// by input) and biases, all little-endian.
const WEIGHTS: &[u8] = include_bytes!("prior.bin");

impl Prior {
    /// The prior compiled in, if its weights are there and fit.
    pub fn load() -> Option<Prior> {
        Prior::parse(WEIGHTS)
    }

    pub fn parse(bytes: &[u8]) -> Option<Prior> {
        let mut r = Reader { bytes, at: 0 };
        if r.word()? != u32::from_le_bytes(*b"IKP1") || r.word()? as usize != FEATURES {
            return None;
        }
        let mean: [f32; FEATURES] = r.floats(FEATURES)?.try_into().ok()?;
        let scale: [f32; FEATURES] = r.floats(FEATURES)?.try_into().ok()?;
        let count = r.word()? as usize;
        let mut layers = Vec::with_capacity(count);
        let mut inputs = FEATURES;
        for _ in 0..count {
            let (i, o) = (r.word()? as usize, r.word()? as usize);
            if i != inputs || o == 0 || o > WIDEST {
                return None;
            }
            let weights = r.floats(i * o)?;
            let biases = r.floats(o)?;
            layers.push(Layer { inputs: i, weights, biases });
            inputs = o;
        }
        (inputs == OUTPUTS && r.at == bytes.len()).then_some(Prior { mean, scale, layers })
    }

    /// The network's raw outputs for some features.
    pub fn run(&self, features: &[f32; FEATURES]) -> [f32; OUTPUTS] {
        let (mut x, mut y) = ([0.0f32; WIDEST], [0.0f32; WIDEST]);
        for k in 0..FEATURES {
            x[k] = ((features[k] - self.mean[k]) * self.scale[k]).clamp(-MAX_SIGMA, MAX_SIGMA);
        }
        for (n, layer) in self.layers.iter().enumerate() {
            let last = n + 1 == self.layers.len();
            for (o, out) in y[..layer.biases.len()].iter_mut().enumerate() {
                let row = &layer.weights[o * layer.inputs..(o + 1) * layer.inputs];
                let v = layer.biases[o] + row.iter().zip(&x[..layer.inputs]).map(|(w, v)| w * v).sum::<f32>();
                *out = if last { v } else { v.max(0.0) };
            }
            std::mem::swap(&mut x, &mut y);
        }
        let mut out = [0.0; OUTPUTS];
        out.copy_from_slice(&x[..OUTPUTS]);
        out
    }

    /// The prediction for some features, the head's yaw being `head_yaw`.
    pub fn predict(&self, features: &[f32; FEATURES], head_yaw: f32) -> Prediction {
        let y = self.run(features);
        let back = Quat::from_rotation_y(head_yaw);
        let elbow = |k: usize| (back * Vec3::new(y[k], y[k + 1], y[k + 2])).normalize_or(-UP);
        Prediction {
            chest_yaw: wrap_angle(head_yaw + y[0].atan2(y[1])),
            hips_yaw: wrap_angle(head_yaw + y[2].atan2(y[3])),
            elbows: [elbow(4), elbow(7)],
        }
    }
}
