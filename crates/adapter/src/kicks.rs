//! A gun's recoil as extracted from its game (`rustvr_asset::Kick`s, in the gun model's space)
//! into the protocol's [`KickDesc`]s, in the weapon space the adapter registers the gun in.

use glam::{Mat4, Vec3};
use rustvr_asset::{KICK_CHANNELS as N, Kick, KickKey, kick_slope, kick_value};
use rustvr_protocol::*;

/// `kicks` for object `object`, carried from model space into weapon space by `fix` (model ->
/// weapon; its rotation turns the channels, the whole of it moves the pivots). Curves longer
/// than [`KICK_KEYS`] keep their first keys.
pub fn kick_descs(object: u32, kicks: &[Kick], fix: Mat4) -> Vec<KickDesc> {
    let (_, rot, _) = fix.to_scale_rotation_translation();
    // A rotation as a rotation vector, so it turns like a translation: (-pitch, yaw, roll).
    let turn = |keys: [&[KickKey]; 3], sign: [f32; 3]| -> [Vec<KickKey>; 3] {
        let mut times: Vec<f32> = keys.iter().flat_map(|k| k.iter().map(|k| k.t)).collect();
        times.sort_by(f32::total_cmp);
        times.dedup();
        let at = |f: &dyn Fn(&[KickKey]) -> f32| Vec3::from_array([0, 1, 2].map(|c| f(keys[c]) * sign[c]));
        let mut out: [Vec<KickKey>; 3] = Default::default();
        for t in times {
            let (v, si, so) = (rot * at(&|k| kick_value(k, t)), rot * at(&|k| kick_slope(k, t, false)), rot * at(&|k| kick_slope(k, t, true)));
            for c in 0..3 {
                out[c].push(KickKey { t, v: v[c] * sign[c], slope_in: si[c] * sign[c], slope_out: so[c] * sign[c] });
            }
        }
        out
    };
    let mut out = Vec::new();
    for k in kicks {
        let ch = &k.channels;
        let [p, y, r] = turn([&ch[0], &ch[1], &ch[2]], [-1.0, 1.0, 1.0]);
        let [x, yy, z] = turn([&ch[3], &ch[4], &ch[5]], [1.0, 1.0, 1.0]);
        let channels: [Vec<KickKey>; N] = [p, y, r, x, yy, z];
        let mut d: KickDesc = bytemuck::Zeroable::zeroed();
        d.object = object;
        d.layer = k.layer as u32;
        d.when = k.when;
        d.speed = k.speed;
        d.length = k.length;
        d.pivot = fix.transform_point3(Vec3::from_array(k.pivot)).to_array();
        for (c, keys) in channels.iter().enumerate() {
            if keys.len() > KICK_KEYS {
                tracing::debug!("kick {}: {} keys, keeping {KICK_KEYS}", k.name, keys.len());
            }
            let curve = &mut d.channels[c];
            curve.count = keys.len().min(KICK_KEYS) as u32;
            for (i, key) in keys.iter().take(KICK_KEYS).enumerate() {
                curve.keys[i] = [key.t, key.v, key.slope_in, key.slope_out];
            }
        }
        out.push(d);
    }
    out
}
