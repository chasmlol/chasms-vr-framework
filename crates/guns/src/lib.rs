//! Guns as RustVR holds them, shared by everything that registers or draws one: the test range,
//! the game adapters and the runtime.
//!
//! * [`Weapon`]: a gun in weapon space (the line of sight along +Z, the firing grip at the
//!   origin): its parts and how they move with what the runtime reports (`ObjectState`), where the
//!   hands go (the game's own hands when it has them, [`GunHand`]) and how it fires.
//! * [`metro`]: Metro 2033's guns from the local asset cache, rigged by
//!   `content/rigs/metro2033.toml` (docs/RIGGING.md): [`metro::metro_weapon`] loads one,
//!   [`metro::metro_gun`] makes its registration.
//! * [`desc`]: the protocol's descriptions of a gun (its `ObjectDesc`, magazine template, the
//!   game's hand poses and kicks), the same for every caller.
//!
//! A gun's parts are cut from the game's model by the bones that carry them (magazine, bolt,
//! trigger, barrels, rounds) and remember where from ([`MeshSource`]), so the runtime can draw them
//! with the model's full vertices and the game's materials while moving each as the range does.

#![allow(clippy::too_many_arguments)]

pub mod desc;
pub mod hulls;
mod mesh;
pub mod metro;
mod weapon;

pub use mesh::*;
pub use weapon::*;
