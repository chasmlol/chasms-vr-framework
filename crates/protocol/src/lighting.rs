//! The game's lighting of its own first-person model, so the runtime lights the player's hands and
//! body exactly as the game would have lit them.
//!
//! The adapter fills [`GameLighting`] every frame with what the game's renderer used: its lights
//! (as they reached its first-person model), its ambient terms and its colour grading. The runtime
//! evaluates the game's own shading model with them. Positions and the spherical harmonics are in
//! the game camera's view space (x right, y up, z forward, metres), with [`GameLighting::eye`] the
//! tracking-space pose of that camera, so the runtime can bring them into each of its eyes.
//!
//! Lights whose effect also depends on a texture (a projected light map, a shadow map) come with
//! that texture's term sampled by the adapter, through the game's own textures and matrices, over
//! the region where the game evaluates its first-person lighting: the light atlas
//! ([`TexKind::Lights`](crate::TexKind::Lights), see [`LIGHT_TILE`]).
//!
//! The model is Metro 2033 Redux's (docs/research/metro-viewmodel-lighting.md); a game whose
//! renderer differs adds its terms here.

use bytemuck::{Pod, Zeroable};

use crate::Pose;

/// The region (after the HUD layout's place before it grew the mapping).
pub const OFF_GAME_LIGHTING: usize = 0x004B_0000;
pub const GAME_LIGHTS_MAX: usize = 32;
/// The colour-grading table's edge (`LUT_SIZE`³ RGBA8 texels, red fastest).
pub const LUT_SIZE: usize = 16;

/// The light atlas: one tile per light with a texture term, [`LIGHT_TILE`] texels square in view
/// space x and y over [`LIGHT_TILE_EXTENT`] metres around the view axis, [`LIGHT_TILE_SLICES`]
/// slices in view-space depth over 0..[`GameLighting::depth_squash`]·[`LIGHT_TILE_DEPTH`],
/// side by side. Texel (rgb) = the light map's colour, (a) = the shadow term (1 = lit).
pub const LIGHT_TILE: u32 = 64;
pub const LIGHT_TILE_SLICES: u32 = 4;
pub const LIGHT_TILE_EXTENT: f32 = 1.2;
/// The depth the slices cover before squashing (metres of real view depth).
pub const LIGHT_TILE_DEPTH: f32 = 1.0;
pub const LIGHT_ATLAS_TILES: u32 = 16;
pub const LIGHT_ATLAS_W: u32 = LIGHT_TILE * LIGHT_TILE_SLICES;
pub const LIGHT_ATLAS_H: u32 = LIGHT_TILE * LIGHT_ATLAS_TILES;

// ---- light kinds (a deferred renderer's light shaders) ----------------------------------------
/// Omni light.
pub const GL_OMNI: u32 = 0;
/// Light with a projected light map.
pub const GL_PROJECTED: u32 = 1;
/// Light with a projected light map, attenuated by the surface's occlusion (`att *= |w|`).
pub const GL_PROJECTED_OCCLUDED: u32 = 2;
/// Shadowed spot with a light map.
pub const GL_SHADOWED: u32 = 3;
/// Shadowed spot without a light map.
pub const GL_SHADOWED_PLAIN: u32 = 4;
/// "Ambient" omni: no N·L, no specular.
pub const GL_AMBIENT: u32 = 5;

// ---- light flags ------------------------------------------------------------------------------
/// The light reaches the first-person model (its volume was shaded with depth test Always: the
/// game's "mode A"). Others never light it.
pub const GLF_REACHES_VIEWMODEL: u32 = 1 << 0;
/// [`GameLight::tile`] holds this light's texture term in the light atlas.
pub const GLF_TILE: u32 = 1 << 1;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct GameLight {
    /// View-space position (metres); w = 1/R² (R: the light's radius in metres).
    pub pos: [f32; 4],
    /// Linear colour; w = the specular scale.
    pub color: [f32; 4],
    /// x = the specular exponent bias (added to gloss²·128); yzw spare.
    pub params: [f32; 4],
    pub kind: u32,  // GL_*
    pub flags: u32, // GLF_*
    /// Row of this light's tile in the light atlas.
    pub tile: u32,
    pub _pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GameLighting {
    /// Seqlock word.
    pub seq: u32,
    pub count: u32,
    /// The adapter's frame counter.
    pub frame: u64,
    /// The game camera the lighting was captured with, in tracking space.
    pub eye: Pose,
    /// Spherical harmonics, order 0 and 1, per colour channel (r, g, b) in view space, as the
    /// game stores them: (c0, y, z, x). Irradiance = 0.886227·c0 + 1.023328·(N.x·x + N.y·y + N.z·z).
    pub sh: [[f32; 4]; 3],
    /// Flat ambient colour (rgb); w = the reflection term's scale.
    pub ambient: [f32; 4],
    /// The first-person model's per-object ambient occlusion ("hemi"; vertex AO is multiplied in).
    pub hemi: f32,
    /// The view-space depth factor the game squashes its first-person model with before lighting
    /// it (0.04 in Metro: lights see the model a few centimetres in front of the camera).
    pub depth_squash: f32,
    /// The screen-space ambient occlusion the game's first-person model gets (its mean).
    pub ssao: f32,
    /// Nonzero when [`GameLighting::lut`] holds this frame's colour grading.
    pub lut_valid: u32,
    pub lights: [GameLight; GAME_LIGHTS_MAX],
    /// The colour-grading table: `LUT_SIZE`³ RGBA8 texels packed little-endian (r in the low
    /// byte), index `(b·16 + g)·16 + r`, applied as
    /// `out = saturate(tonemapped) + 2·lut(tonemapped.bgr).bgr − 1`.
    pub lut: [u32; LUT_SIZE * LUT_SIZE * LUT_SIZE],
}

impl Default for GameLighting {
    fn default() -> Self {
        Zeroable::zeroed()
    }
}

const _: () = {
    assert!(size_of::<GameLight>() == 64);
    assert!(OFF_GAME_LIGHTING + size_of::<GameLighting>() <= crate::hud::OFF_HUD_LAYOUT);
    assert!(OFF_GAME_LIGHTING.is_multiple_of(64));
};
