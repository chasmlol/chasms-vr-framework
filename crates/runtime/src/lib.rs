//! The RustVR runtime: one VR player experience (hands, aiming, gestures, comfort, menus) that
//! connects into any game through an adapter. See `docs/DESIGN.md` at the repository root.

#![allow(clippy::too_many_arguments, clippy::needless_range_loop)]

pub mod app;
pub mod backend;
pub mod blockout;
pub mod combat;
pub mod powers;
pub mod body;
pub mod compositor;
pub mod crosshair;
pub mod gore;
pub mod mechanics;
pub mod menu;
pub mod mirror;
pub mod obs_view;
pub mod held_cards;
pub mod held_menus;
pub mod held_objects;
pub mod holsters;
pub mod hud;
pub mod item_card;
pub mod game_cards;
pub mod panels;
pub mod physics_view;
pub mod rig;
pub mod reins;
pub mod riding;
pub mod scenario;
pub mod scenario_physical;
pub mod session;
pub mod settings;
pub mod spectator;
pub mod swim;
pub mod targets;
pub mod warp;
pub mod locomotion;
pub mod pacing;

/// Wraps an angle to (-pi, pi].
pub fn wrap_angle(a: f32) -> f32 {
    let t = std::f32::consts::TAU;
    let r = a.rem_euclid(t);
    if r > std::f32::consts::PI { r - t } else { r }
}
