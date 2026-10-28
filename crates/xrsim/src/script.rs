//! What the emulated headset does: a script of keyframes for the head and both controllers, live
//! overrides from a control file, and the captures to save.
//!
//! Everything here is in OpenXR's stage space (right-handed metres, Y up, -Z forward), but poses
//! are written the way a person would describe them: yaw to the right, pitch up and roll in
//! degrees, and hands relative to the head (`[right, up, forward]` metres from the head, turned
//! with its yaw).
//!
//! ```toml
//! duration = 60            # quit after this many seconds (0: never)
//! profile = "/interaction_profiles/oculus/touch_controller"
//!
//! [[key]]                  # values hold until the next key; poses and analog values ease to it
//! t = 0
//! head = { pos = [0, 1.7, 0], yaw = 0, pitch = 0 }
//! right = { pos = [0.2, -0.4, 0.35], pitch = 0, trigger = 0, a = false }
//!
//! [[capture]]
//! t = 10
//! name = "menu"
//!
//! [[film]]                 # consecutive frames, saved without holding the app up
//! t = 20
//! frames = 120
//! name = "sweep"
//! ```
//!
//! The control file (`<dir>/control.toml`, reread whenever it changes) holds one key without `t`
//! that overrides the script from then on, plus one-shot commands: `capture = "name"`,
//! `focused = false` (the headset's dashboard over the app), `quit = true`. It may also hold a path
//! to play from when it's read: `[[key]]`s whose `t` is seconds from then, eased into from wherever
//! the script is (a test driver's reach for something it has found, say).

use glam::{Quat, Vec2, Vec3};
use serde::Deserialize;

/// One controller in a key.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct HandKey {
    /// `[right, up, forward]` metres from the head, turned with the head's yaw.
    pub pos: Option<[f32; 3]>,
    pub yaw: Option<f32>,
    pub pitch: Option<f32>,
    pub roll: Option<f32>,
    pub trigger: Option<f32>,
    pub squeeze: Option<f32>,
    pub stick: Option<[f32; 2]>,
    pub a: Option<bool>,
    pub b: Option<bool>,
    pub menu: Option<bool>,
    pub stick_click: Option<bool>,
    pub connected: Option<bool>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct HeadKey {
    /// Stage metres.
    pub pos: Option<[f32; 3]>,
    pub yaw: Option<f32>,
    pub pitch: Option<f32>,
    pub roll: Option<f32>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Key {
    pub t: f64,
    pub head: HeadKey,
    pub left: HandKey,
    pub right: HandKey,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    pub t: f64,
    pub name: String,
}

/// Consecutive frames, each as it's submitted and without holding the app up (copied on the GPU,
/// read back a frame or two later, written as they come), for judging motion frame by frame:
/// `films/<name>/NNN.png` (both eyes as submitted, a quarter size unless `mip` says) and `films/<name>/frames.csv` (each frame's script
/// time, the submitted eyes' poses and fields of view, the hands and the head).
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Film {
    pub t: f64,
    pub frames: u32,
    pub name: String,
    /// Instead of the whole of each eye a quarter size: this part of it at full size (x, y, width,
    /// height in the submitted image's pixels), the same for both eyes.
    #[serde(default)]
    pub crop: Option<[u32; 4]>,
    /// Only every this many frames (a film of a longer stretch at a watchable rate: 3 at 90 Hz
    /// gives 30 frames a second); 1 when not given.
    #[serde(default)]
    pub every: Option<u32>,
    /// The size kept of each eye: its mip level (0 the whole, 1 half, 2 a quarter: the default).
    #[serde(default)]
    pub mip: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Script {
    pub duration: f64,
    /// The controllers the headset reports.
    pub profile: String,
    /// Per-eye swapchain size the headset recommends.
    pub eye_size: [u32; 2],
    /// Each eye's field of view in degrees: left, right, up, down (the right eye mirrors it).
    pub fov: [f32; 4],
    pub ipd: f32,
    pub refresh: f32,
    /// Width of each eye in the saved captures (pixels).
    pub capture_width: u32,
    /// How much the aim pose points above the grip pose (degrees; a Touch controller's ray).
    pub aim_pitch: f32,
    #[serde(rename = "key")]
    pub keys: Vec<Key>,
    #[serde(rename = "capture")]
    pub captures: Vec<Capture>,
    #[serde(rename = "film")]
    pub films: Vec<Film>,
}

impl Default for Script {
    fn default() -> Self {
        Script {
            duration: 0.0,
            profile: "/interaction_profiles/oculus/touch_controller".into(),
            eye_size: [2064, 2208],
            // A Quest 3's left eye (through Virtual Desktop), near enough.
            fov: [-54.0, 43.0, 46.0, -52.0],
            ipd: 0.064,
            refresh: 90.0,
            capture_width: 960,
            aim_pitch: 0.0,
            keys: Vec::new(),
            captures: Vec::new(),
            films: Vec::new(),
        }
    }
}

impl Script {
    /// Keys and captures may be written in any order; they're kept in time order.
    pub fn parse(text: &str) -> Result<Script, String> {
        let mut s: Script = toml::from_str(text).map_err(|e| e.to_string())?;
        s.keys.sort_by(|a, b| a.t.total_cmp(&b.t));
        s.captures.sort_by(|a, b| a.t.total_cmp(&b.t));
        s.films.sort_by(|a, b| a.t.total_cmp(&b.t));
        Ok(s)
    }
}

/// The live control file.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Control {
    pub head: HeadKey,
    pub left: HandKey,
    pub right: HandKey,
    /// Bump-style one-shots: a new name saves a capture.
    pub capture: Option<String>,
    pub focused: Option<bool>,
    pub quit: Option<bool>,
    /// A path from when it's read (each key's `t` seconds from then).
    #[serde(rename = "key")]
    pub keys: Vec<Key>,
}

impl Control {
    pub fn parse(text: &str) -> Result<Control, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }
}

impl Script {
    /// Plays `path` from script time `t` on (its keys' times from then): eased into from the pose
    /// at `t`, then held at its end. Later keys of the script come after it as they were.
    pub fn play_from(&mut self, t: f64, path: &[Key]) {
        if path.is_empty() {
            return;
        }
        // A key at `t` that changes nothing: the path eases from where things are now, not from
        // the script's last key.
        self.keys.push(Key { t, ..Default::default() });
        self.keys.extend(path.iter().map(|k| Key { t: t + k.t.max(0.0), ..k.clone() }));
    }
}

/// One controller's state at an instant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hand {
    pub pos: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    pub trigger: f32,
    pub squeeze: f32,
    pub stick: Vec2,
    pub a: bool,
    pub b: bool,
    pub menu: bool,
    pub stick_click: bool,
    pub connected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Head {
    pub pos: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
}

/// Everything at an instant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub head: Head,
    pub hands: [Hand; 2],
}

impl Default for Pose {
    fn default() -> Self {
        let hand = |x: f32| Hand {
            pos: [x, -0.55, 0.25],
            yaw: 0.0,
            pitch: 0.0,
            roll: 0.0,
            trigger: 0.0,
            squeeze: 0.0,
            stick: Vec2::ZERO,
            a: false,
            b: false,
            menu: false,
            stick_click: false,
            connected: true,
        };
        Pose { head: Head { pos: Vec3::new(0.0, 1.7, 0.0), yaw: 0.0, pitch: 0.0, roll: 0.0 }, hands: [hand(-0.2), hand(0.2)] }
    }
}

fn lerp(a: f32, b: f32, k: f32) -> f32 {
    a + (b - a) * k
}

impl Hand {
    fn apply(&mut self, k: &HandKey) {
        if let Some(v) = k.pos {
            self.pos = v;
        }
        macro_rules! set {
            ($($f:ident),*) => { $(if let Some(v) = k.$f { self.$f = v; })* };
        }
        set!(yaw, pitch, roll, trigger, squeeze, a, b, menu, stick_click, connected);
        if let Some(v) = k.stick {
            self.stick = Vec2::from_array(v);
        }
    }

    /// Eased towards `b` by `k` (switches flip at the end).
    fn towards(&self, b: &Hand, k: f32) -> Hand {
        let p = |i: usize| lerp(self.pos[i], b.pos[i], k);
        Hand {
            pos: [p(0), p(1), p(2)],
            yaw: lerp(self.yaw, b.yaw, k),
            pitch: lerp(self.pitch, b.pitch, k),
            roll: lerp(self.roll, b.roll, k),
            trigger: lerp(self.trigger, b.trigger, k),
            squeeze: lerp(self.squeeze, b.squeeze, k),
            stick: self.stick.lerp(b.stick, k),
            ..if k >= 1.0 { *b } else { *self }
        }
    }
}

impl Head {
    fn apply(&mut self, k: &HeadKey) {
        if let Some(v) = k.pos {
            self.pos = Vec3::from_array(v);
        }
        if let Some(v) = k.yaw {
            self.yaw = v;
        }
        if let Some(v) = k.pitch {
            self.pitch = v;
        }
        if let Some(v) = k.roll {
            self.roll = v;
        }
    }

    /// Stage-space orientation: yaw to the right, then pitch up, then roll.
    pub fn rot(&self) -> Quat {
        Quat::from_rotation_y(-self.yaw.to_radians()) * Quat::from_rotation_x(self.pitch.to_radians()) * Quat::from_rotation_z(-self.roll.to_radians())
    }
}

impl Pose {
    pub fn apply(&mut self, head: &HeadKey, left: &HandKey, right: &HandKey) {
        self.head.apply(head);
        self.hands[0].apply(left);
        self.hands[1].apply(right);
    }

    fn towards(&self, b: &Pose, k: f32) -> Pose {
        Pose {
            head: Head {
                pos: self.head.pos.lerp(b.head.pos, k),
                yaw: lerp(self.head.yaw, b.head.yaw, k),
                pitch: lerp(self.head.pitch, b.head.pitch, k),
                roll: lerp(self.head.roll, b.head.roll, k),
            },
            hands: [self.hands[0].towards(&b.hands[0], k), self.hands[1].towards(&b.hands[1], k)],
        }
    }

    /// A hand's stage pose (position, orientation) for its grip.
    pub fn hand_pose(&self, h: usize) -> (Vec3, Quat) {
        let hd = &self.hands[h];
        let yaw = Quat::from_rotation_y(-self.head.yaw.to_radians());
        let p = self.head.pos + yaw * Vec3::new(hd.pos[0], hd.pos[1], -hd.pos[2]);
        let r = yaw * Quat::from_rotation_y(-hd.yaw.to_radians()) * Quat::from_rotation_x(hd.pitch.to_radians()) * Quat::from_rotation_z(-hd.roll.to_radians());
        (p, r)
    }
}

/// The script played out: the pose at `t` seconds (keys ease into each other, then hold).
pub fn sample(script: &Script, t: f64) -> Pose {
    let mut keys: Vec<&Key> = script.keys.iter().collect();
    keys.sort_by(|a, b| a.t.total_cmp(&b.t));
    // Accumulate keys into absolute poses (a key only names what changes).
    let mut poses = Vec::with_capacity(keys.len());
    let mut cur = Pose::default();
    for k in &keys {
        cur.apply(&k.head, &k.left, &k.right);
        poses.push((k.t, cur));
    }
    let Some(first) = poses.first() else { return Pose::default() };
    if t <= first.0 {
        return first.1;
    }
    for w in poses.windows(2) {
        let ((t0, a), (t1, b)) = (w[0], w[1]);
        if t < t1 {
            let k = if t1 > t0 { ((t - t0) / (t1 - t0)) as f32 } else { 1.0 };
            return a.towards(&b, k.clamp(0.0, 1.0));
        }
    }
    poses.last().unwrap().1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_ease_and_hold() {
        let s = Script::parse(
            "[[key]]\nt = 0\nhead = { yaw = 0 }\nright = { trigger = 0, a = false }\n\
             [[key]]\nt = 2\nhead = { yaw = 90 }\nright = { trigger = 1, a = true }\n",
        )
        .unwrap();
        let mid = sample(&s, 1.0);
        assert!((mid.head.yaw - 45.0).abs() < 1e-4);
        assert!((mid.hands[1].trigger - 0.5).abs() < 1e-4);
        assert!(!mid.hands[1].a, "switches flip on the key");
        let end = sample(&s, 5.0);
        assert_eq!(end.head.yaw, 90.0);
        assert!(end.hands[1].a);
        // Yaw to the right: forward (-Z) turns towards +X.
        let fwd = end.head.rot() * Vec3::NEG_Z;
        assert!(fwd.abs_diff_eq(Vec3::X, 1e-5), "{fwd:?}");
    }

    #[test]
    fn hands_follow_the_head_yaw() {
        let mut p = Pose::default();
        p.head.yaw = 90.0;
        p.hands[1].pos = [0.0, 0.0, 0.5];
        let (pos, _) = p.hand_pose(1);
        assert!(pos.abs_diff_eq(Vec3::new(0.5, 1.7, 0.0), 1e-5), "half a metre in front of a head turned right: {pos:?}");
    }

    #[test]
    fn a_path_from_the_control_file_eases_from_where_things_are() {
        let mut s = Script::parse("[[key]]
t = 0
right = { pos = [0.2, -0.5, 0.3] }
").unwrap();
        let c = Control::parse("[[key]]
t = 1
right = { pos = [0.4, -0.5, 0.3], squeeze = 1 }
").unwrap();
        s.play_from(10.0, &c.keys);
        // Until it's read, as it was; then from there to the path's key over its second.
        assert_eq!(sample(&s, 10.0).hands[1].pos, [0.2, -0.5, 0.3]);
        assert!((sample(&s, 10.5).hands[1].pos[0] - 0.3).abs() < 1e-5);
        let end = sample(&s, 12.0).hands[1];
        assert_eq!((end.pos, end.squeeze), ([0.4, -0.5, 0.3], 1.0));
    }

    #[test]
    fn control_files_parse() {
        let c = Control::parse("capture = \"menu\"\nfocused = false\nright = { menu = true }\n").unwrap();
        assert_eq!(c.capture.as_deref(), Some("menu"));
        assert_eq!(c.right.menu, Some(true));
        assert!(Control::parse("nonsense = 1").is_err());
    }
}
