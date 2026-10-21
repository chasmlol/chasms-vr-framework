//! Where a sound is relative to the head, turned into what each ear hears.
//!
//! Cheap, but enough for a sound at the hand to sit at the hand:
//! * interaural time difference from Woodworth's spherical-head formula (up to about 0.66 ms);
//! * head shadow at each ear, Brown and Duda's one-pole, one-zero shelf. This makes the level
//!   difference depend on frequency the way a real head does: lows wrap round, highs don't;
//! * a gentle low-pass blended in as the source moves behind, the main front/back cue that a
//!   spherical head lacks;
//! * inverse-distance level from each ear, so a hand beside the head is clearly louder in that
//!   ear (the near-field level difference);
//! * a low-shelf lift inside a metre, the bass boost of very close sources.

use std::f32::consts::PI;

use glam::Vec3;

use crate::Listener;

const HEAD_RADIUS: f32 = 0.0875;
const SPEED_OF_SOUND: f32 = 343.0;
/// Distance from an ear at which a sound plays at its authored level: a hand held in front of
/// the chest is about this far from either ear.
const REFERENCE: f32 = 0.45;
/// Closer than this to an ear a sound gets no louder (+3.5 dB over the reference). The shoulder
/// ammo and backpack zones are about 0.2 m from an ear, and a full inverse-distance rise there
/// makes every zone tick and magazine grab jump out.
const NEAREST: f32 = 0.3;
/// The bass lift starts inside this distance from the head...
const NEAR_FIELD: f32 = 1.0;
/// ...and reaches this much extra low-shelf gain (about +4 dB) at `NEAREST` and closer.
const NEAR_BASS: f32 = 0.6;
/// Corner of the near-field low shelf, Hz.
pub(crate) const BASS_CORNER: f32 = 250.0;
/// Corner of the rear low-pass, Hz.
pub(crate) const REAR_CORNER: f32 = 3500.0;

/// What a source sounds like from where the listener is. Pairs are `[left, right]`.
pub(crate) struct Placement {
    /// Interaural delay in samples (the near ear is 0).
    pub delay: [f32; 2],
    /// Distance gain.
    pub gain: [f32; 2],
    /// Head-shadow gain at high frequencies.
    pub shadow: [f32; 2],
    /// How much of the rear low-pass to blend in: 0 in front or beside, 1 straight behind.
    pub behind: f32,
    /// Extra low-shelf gain, 0 beyond a metre.
    pub bass: f32,
    /// Gain for the layers that aren't spatialised.
    pub bed: f32,
}

pub(crate) fn place(listener: &Listener, source: Vec3, rate: f32) -> Placement {
    let local = listener.rot.inverse() * (source - listener.pos);
    let distance = local.length();
    let dir = if distance > 1e-4 { local / distance } else { Vec3::Z };
    let lateral = dir.x.clamp(-1.0, 1.0).asin().abs();
    let itd = HEAD_RADIUS / SPEED_OF_SOUND * (lateral + lateral.sin()) * rate;
    let ear = |side: f32| attenuation((local - Vec3::new(side * HEAD_RADIUS, 0.0, 0.0)).length());
    let near = ((NEAR_FIELD - distance) / (NEAR_FIELD - NEAREST)).clamp(0.0, 1.0);
    Placement {
        delay: if dir.x > 0.0 { [itd, 0.0] } else { [0.0, itd] },
        gain: [ear(-1.0), ear(1.0)],
        shadow: [shadow(-dir.x), shadow(dir.x)],
        behind: (-dir.z).max(0.0),
        bass: NEAR_BASS * near,
        bed: attenuation(distance),
    }
}

/// Inverse distance, clamped so a sound right at the ear doesn't blow up.
pub(crate) fn attenuation(distance: f32) -> f32 {
    REFERENCE / distance.max(NEAREST)
}

/// Brown and Duda's high-frequency gain for an ear, from the cosine of the angle between the
/// source and that ear's axis: 2 (+6 dB) facing the ear, 0.1 (−20 dB) at 150° round the head.
fn shadow(cos: f32) -> f32 {
    const DEEPEST: f32 = 0.1;
    const DEEPEST_ANGLE: f32 = 150.0 * PI / 180.0;
    let angle = cos.clamp(-1.0, 1.0).acos();
    (1.0 + DEEPEST / 2.0) + (1.0 - DEEPEST / 2.0) * (angle / DEEPEST_ANGLE * PI).cos()
}

/// The head-shadow filter H(s) = (α s + β) / (s + β), β = 2c/a, by the bilinear transform. α is
/// the gain at high frequencies; low frequencies pass unchanged.
pub(crate) struct Shelf {
    k: f32,
    beta: f32,
    norm: f32,
    /// Feedback coefficient (the same for every α).
    pub a1: f32,
}

impl Shelf {
    pub fn new(rate: f32) -> Shelf {
        let k = 2.0 * rate;
        let beta = 2.0 * SPEED_OF_SOUND / HEAD_RADIUS;
        Shelf { k, beta, norm: 1.0 / (k + beta), a1: (beta - k) / (k + beta) }
    }

    /// Feed-forward coefficients for high-frequency gain `alpha`.
    pub fn b(&self, alpha: f32) -> (f32, f32) {
        ((alpha * self.k + self.beta) * self.norm, (self.beta - alpha * self.k) * self.norm)
    }
}
