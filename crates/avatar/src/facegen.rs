//! FaceGen faces: a head and the parts made to its shape (ears, eyes, mouth, teeth, hair), moved
//! to a face's place in FaceGen's statistical face space, as Oblivion, Fallout 3 and New Vegas make
//! their characters' heads (the Oblivion adapter's docs/design/mirror.md).
//!
//! * **Geometry:** each coordinate moves every vertex of a part by its morph times the coordinate
//!   ([`morph_mesh`]); the symmetric coordinates and the asymmetric ones each have their own morphs.
//! * **Colour:** the head's texture morphs, each times its coordinate, summed into one texture the
//!   way the game makes its own ([`morph_texture`]): `0.5 + Σ c·scale·delta / 510` (the game's
//!   `FaceGenMap0`). Its shader lays it over the base colour, with a detail map over that:
//!   `4 · detail · (base + 2 · (morphs − 0.5))` (the skin shader's FaceGen blend, `SKIN2001`). That
//!   blend is the avatar shader's ([`crate::gamebryo`]); this makes its textures.

use rustvr_asset::{self as asset, FaceCoords, FaceMorphs, TextureMorphs};

/// A face, as the runtime draws it on the parts that have face morphs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Face {
    pub coords: FaceCoords,
    /// The hair's tint, as the game's hair shader takes it (display 0..1 rgb).
    pub hair: [f32; 3],
    /// The face's detail map: on the meshes whose base texture is the first key, the second laid
    /// over their colour (4× modulate) and the third drawn as their normal map (keys; an empty one
    /// is none).
    pub detail: Option<(String, String, String)>,
    /// The parts that are the eyes (left, right; model keys): each gets a bone of its own at its
    /// centre (`LeftEye`, `RightEye`), so it turns to look ([`crate::Avatar::set_gaze`]).
    pub eyes: [Option<String>; 2],
}

/// A FaceGen part's mesh as the face's geometry coordinates move it, kept so the face can change
/// live (a hand pulling it into shape) without the body being put together again: its vertices'
/// places with no face on them, and each coordinate's move of each, in the body's model space
/// (the part as [`crate::compose`] put it on the skeleton).
#[derive(Debug, Default)]
pub struct FaceShape {
    /// Which of the body's face parts it is (the order [`crate::compose`] met them in).
    pub part: usize,
    pub rest: Vec<[f32; 3]>,
    /// Each symmetric coordinate's move of every vertex, then each asymmetric one's.
    pub symmetric: Vec<Vec<[f32; 3]>>,
    pub asymmetric: Vec<Vec<[f32; 3]>>,
}

impl FaceShape {
    /// Every vertex's place for a face's geometry coordinates (symmetric, asymmetric).
    pub fn positions(&self, symmetric: &[f32], asymmetric: &[f32]) -> Vec<[f32; 3]> {
        let mut out = self.rest.clone();
        for (set, c) in [(&self.symmetric, symmetric), (&self.asymmetric, asymmetric)] {
            for (morph, &k) in set.iter().zip(c) {
                if k == 0.0 {
                    continue;
                }
                for (p, d) in out.iter_mut().zip(morph) {
                    for a in 0..3 {
                        p[a] += d[a] * k;
                    }
                }
            }
        }
        out
    }
}

/// The face as it's drawn live, without the body being put together again
/// ([`crate::Avatar::set_face_shape`], [`crate::HeldModel::set_face`]): its geometry coordinates,
/// and by face part ([`FaceShape::part`]) how strongly each of its vertices glows, 0..255, for what
/// a hand would take hold of (missing or empty: none).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FaceNow {
    pub symmetric: Vec<f32>,
    pub asymmetric: Vec<f32>,
    pub glow: Vec<Vec<u8>>,
}

/// Moves a mesh's vertices to the face: each morph times its coordinate.
pub fn morph_mesh(mesh: &mut asset::Mesh, morphs: &FaceMorphs, coords: &FaceCoords) {
    let sets = [(&morphs.symmetric, &coords.geometry_symmetric), (&morphs.asymmetric, &coords.geometry_asymmetric)];
    for (set, c) in sets {
        for (morph, &k) in set.iter().zip(c.iter()) {
            if k == 0.0 {
                continue;
            }
            for (v, d) in mesh.vertices.iter_mut().zip(morph) {
                for a in 0..3 {
                    v.pos[a] += d[a] * k;
                }
            }
        }
    }
}

/// The texture morphs' texture for symmetric texture coordinates `coords`: RGBA8 (raw values, not
/// sRGB), `0.5 + Σ c·scale·delta / 510` per channel, with its mips.
pub fn morph_texture(t: &TextureMorphs, coords: &[f32]) -> asset::Texture {
    let (w, h) = (t.width as usize, t.height as usize);
    let n = w * h;
    let mut sum = vec![0f32; n * 3];
    for (m, &k) in t.symmetric.iter().zip(coords) {
        let s = k * m.scale;
        if s == 0.0 || m.delta.len() < n * 3 {
            continue;
        }
        for (acc, &d) in sum.iter_mut().zip(&m.delta) {
            *acc += s * d as f32;
        }
    }
    let mut top = vec![255u8; n * 4];
    for i in 0..n {
        for c in 0..3 {
            top[i * 4 + c] = ((0.5 + sum[c * n + i] / 510.0).clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    asset::Texture { version: asset::FORMAT_VERSION, source: "a face's texture morphs".into(), width: t.width, height: t.height, format: asset::TexFormat::Rgba8, mips: mips(top, w, h), faces: 1 }
}

/// A full mip chain from an RGBA8 level, each level the mean of four.
fn mips(top: Vec<u8>, mut w: usize, mut h: usize) -> Vec<Vec<u8>> {
    let mut out = vec![top];
    while w > 1 || h > 1 {
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let prev = out.last().unwrap();
        let mut next = vec![0u8; nw * nh * 4];
        for y in 0..nh {
            for x in 0..nw {
                for c in 0..4 {
                    let at = |xx: usize, yy: usize| prev[((yy.min(h - 1)) * w + xx.min(w - 1)) * 4 + c] as u32;
                    let s = at(2 * x, 2 * y) + at(2 * x + 1, 2 * y) + at(2 * x, 2 * y + 1) + at(2 * x + 1, 2 * y + 1);
                    next[(y * nw + x) * 4 + c] = ((s + 2) / 4) as u8;
                }
            }
        }
        out.push(next);
        (w, h) = (nw, nh);
    }
    out
}
