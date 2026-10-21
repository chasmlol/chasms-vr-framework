//! The game's own sounds: where its sound pack is, and which of the pack's sound sets each object
//! uses.
//!
//! The runtime plays a sound for every interaction (a grab, a magazine seating, a bolt going
//! home, a dry fire, a shot). With no pack these are the runtime's own synthesised sounds; a
//! game that has sounds of its own writes a pack (a folder with a `sounds.toml`, see
//! `rustvr_audio::pack`) and publishes it here, and the runtime then plays the game's sound for
//! every event the pack has. Each object names the set it sounds like (a gun its own, say);
//! objects without one get the pack's defaults.
//!
//! Some sounds the game already makes itself: a game that fires the runtime's shots through its
//! own weapon code usually plays the gunshot too. [`SND_GAME_SHOTS`] (for every object, or per
//! object in [`ObjectSound::flags`]) tells the runtime not to play those, so nothing is heard
//! twice; [`SND_GAME_HANDLING`] does the same for reloading and handling sounds.
//!
//! The region is additive: it lies past every other region, and an adapter that never writes it
//! leaves the runtime on its own sounds.

use bytemuck::{Pod, Zeroable};

use crate::world::{MAX_OBJECTS, OFF_KICKS, KickTable};

/// The [`SoundPack`] region.
pub const OFF_SOUNDS: usize = 0x004A_9000;
/// Longest pack folder path, in UTF-8 bytes (with its NUL).
pub const SOUND_PATH_BYTES: usize = 512;
/// Longest sound set name, in UTF-8 bytes (with its NUL).
pub const SOUND_SET_BYTES: usize = 32;
pub const MAX_SOUND_OBJECTS: usize = MAX_OBJECTS;

/// Adapter -> runtime, seqlocked like the object table: the runtime re-reads it whenever `seq`
/// changes, and (re)loads the pack when the path or the revision changes. Write it after
/// connecting, and again after every reconnect.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct SoundPack {
    pub seq: u32,
    /// [`SND_GAME_SHOTS`], [`SND_GAME_HANDLING`]: for every object.
    pub flags: u32,
    /// Bump to make the runtime load the pack again from the same folder (its files changed).
    pub revision: u32,
    /// Entries in use: `objects[..count]` (with gaps where `object` is 0).
    pub count: u32,
    pub _pad: [u32; 12],
    /// The pack's folder (absolute, holding `sounds.toml`), UTF-8, NUL-terminated. Empty: no
    /// pack (the runtime's own sounds), though the flags still apply.
    pub path: [u8; SOUND_PATH_BYTES],
    pub objects: [ObjectSound; MAX_SOUND_OBJECTS],
}

/// The sound set one object uses.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct ObjectSound {
    /// [`crate::ObjectDesc::id`]; 0 marks an empty entry. Magazines and runtime-spawned copies
    /// need no entry: their sounds are their gun's.
    pub object: u32,
    /// `SND_*` for this object only (on top of [`SoundPack::flags`]).
    pub flags: u32,
    /// A `[sets.<name>]` of the pack's `sounds.toml`, UTF-8, NUL-terminated (case is ignored).
    /// Empty, or a set the pack doesn't have: the pack's defaults.
    pub set: [u8; SOUND_SET_BYTES],
}

/// The game plays the gunshots of the shots it fires (the shot, its tail and the casings): the
/// runtime plays none, from the pack or its own.
pub const SND_GAME_SHOTS: u32 = 1 << 0;
/// The game plays its own reloading and handling sounds (magazine out and in, the bolt, a dry
/// fire): the runtime plays none of those.
pub const SND_GAME_HANDLING: u32 = 1 << 1;

impl SoundPack {
    /// A pack in `dir` (absolute; empty for none), no flags, no objects.
    pub fn new(dir: &str) -> SoundPack {
        let mut p = SoundPack::zeroed();
        p.set_path(dir);
        p
    }

    /// False (and the path left empty) if it doesn't fit.
    pub fn set_path(&mut self, dir: &str) -> bool {
        self.path.fill(0);
        if dir.len() >= SOUND_PATH_BYTES {
            return false;
        }
        crate::set_name(&mut self.path, dir);
        true
    }

    pub fn path(&self) -> &str {
        crate::name_str(&self.path)
    }

    /// Object `id` sounds like the pack's set `set` (plus `flags` for it alone). False when the
    /// table is full or the name doesn't fit.
    pub fn add_object(&mut self, id: u32, set: &str, flags: u32) -> bool {
        let n = self.count as usize;
        if id == 0 || n >= MAX_SOUND_OBJECTS || set.len() >= SOUND_SET_BYTES {
            return false;
        }
        let o = &mut self.objects[n];
        o.object = id;
        o.flags = flags;
        crate::set_name(&mut o.set, set);
        self.count += 1;
        true
    }

    pub fn entries(&self) -> &[ObjectSound] {
        &self.objects[..(self.count as usize).min(MAX_SOUND_OBJECTS)]
    }

    /// The entry for object `id`, if it has one.
    pub fn object(&self, id: u32) -> Option<&ObjectSound> {
        self.entries().iter().find(|o| id != 0 && o.object == id)
    }
}

impl ObjectSound {
    pub fn set_name(&self) -> &str {
        crate::name_str(&self.set)
    }
}

const _: () = {
    assert!(size_of::<ObjectSound>() == 40);
    assert!(size_of::<SoundPack>() == 64 + SOUND_PATH_BYTES + 40 * MAX_SOUND_OBJECTS);
    assert!(OFF_KICKS + size_of::<KickTable>() <= OFF_SOUNDS);
    assert!(OFF_SOUNDS.is_multiple_of(64));
    assert!(OFF_SOUNDS + size_of::<SoundPack>() <= crate::MAPPING_BYTES);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pack_names_its_folder_and_objects() {
        let mut p = SoundPack::new(r"X:\rustvr\local_assets\metro2033\sounds");
        assert_eq!(p.path(), r"X:\rustvr\local_assets\metro2033\sounds");
        assert!(p.add_object(3, "wpn_ak", SND_GAME_SHOTS));
        assert!(p.add_object(4, "wpn_revolver", 0));
        assert!(!p.add_object(0, "nothing", 0), "0 is never an object");
        assert!(!p.add_object(5, &"x".repeat(SOUND_SET_BYTES), 0), "too long a set name");
        assert_eq!(p.object(3).map(|o| (o.set_name(), o.flags)), Some(("wpn_ak", SND_GAME_SHOTS)));
        assert_eq!(p.object(4).map(ObjectSound::set_name), Some("wpn_revolver"));
        assert!(p.object(5).is_none() && p.object(0).is_none());
        assert!(!p.set_path(&"d".repeat(SOUND_PATH_BYTES)) && p.path().is_empty());
    }
}
