//! What the game's things in the hands are: for a card the runtime shows beside one a hand holds
//! (or pulls), with what the game's own interface says about it: its name and how many, then its
//! numbers (value, weight, damage, armour, condition...) as the game words them, and whether taking
//! it is a crime.
//!
//! The adapter writes it whenever the things it offers change (with the objects, [`crate::ObjectTable`]);
//! an object without an entry has no card.

use bytemuck::{Pod, Zeroable};

pub const OFF_ITEM_INFO: usize = (crate::OFF_AVATAR + size_of::<crate::AvatarDesc>()).next_multiple_of(64);
pub const MAX_ITEM_INFOS: usize = 48;
pub const MAX_ITEM_STATS: usize = 6;

/// Taking it is stealing: someone else owns it (the game's red hand).
pub const ITEM_STOLEN: u32 = 1 << 0;
pub const ITEM_ENCHANTED: u32 = 1 << 1;
/// A quest needs it.
pub const ITEM_QUEST: u32 = 1 << 2;

/// One of a thing's numbers: "Weight", "3.0".
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct ItemStat {
    /// UTF-8, NUL-terminated.
    pub label: [u8; 16],
    pub value: [u8; 16],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct ItemInfo {
    /// The object's [`crate::ObjectDesc::id`].
    pub id: u32,
    pub flags: u32, // ITEM_*
    /// How many it is (a stack: 1 or more).
    pub count: u32,
    /// Stats in use are `stats[..stat_count]`.
    pub stat_count: u32,
    /// UTF-8, NUL-terminated.
    pub name: [u8; 48],
    pub stats: [ItemStat; MAX_ITEM_STATS],
}

impl ItemInfo {
    pub fn new(id: u32, name: &str) -> ItemInfo {
        let mut i: ItemInfo = Zeroable::zeroed();
        i.id = id;
        i.count = 1;
        crate::set_name(&mut i.name, name);
        i
    }

    pub fn name(&self) -> &str {
        crate::name_str(&self.name)
    }

    /// Adds a stat (false: no room).
    pub fn push_stat(&mut self, label: &str, value: &str) -> bool {
        let n = self.stat_count as usize;
        if n >= MAX_ITEM_STATS {
            return false;
        }
        crate::set_name(&mut self.stats[n].label, label);
        crate::set_name(&mut self.stats[n].value, value);
        self.stat_count += 1;
        true
    }

    pub fn stats(&self) -> impl Iterator<Item = (&str, &str)> {
        self.stats[..(self.stat_count as usize).min(MAX_ITEM_STATS)].iter().map(|s| (crate::name_str(&s.label), crate::name_str(&s.value)))
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct ItemInfos {
    /// Seqlock word.
    pub seq: u32,
    /// Entries in use are `infos[..count]`.
    pub count: u32,
    pub _pad: [u32; 14],
    pub infos: [ItemInfo; MAX_ITEM_INFOS],
}

impl ItemInfos {
    pub fn get(&self, id: u32) -> Option<&ItemInfo> {
        self.infos[..(self.count as usize).min(MAX_ITEM_INFOS)].iter().find(|i| i.id == id)
    }
}

const _: () = {
    assert!(size_of::<ItemInfo>() == 16 + 48 + 32 * MAX_ITEM_STATS);
    assert!(OFF_ITEM_INFO.is_multiple_of(64));
    assert!(OFF_ITEM_INFO + size_of::<ItemInfos>() <= crate::OFF_HUD_LAYOUT);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_stats_round_trip_and_are_cut_to_fit() {
        let mut i = ItemInfo::new(7, "Silver Longsword");
        assert!(i.push_stat("Damage", "14"));
        assert!(i.push_stat("Weight", "24"));
        assert_eq!(i.name(), "Silver Longsword");
        assert_eq!(i.stats().collect::<Vec<_>>(), [("Damage", "14"), ("Weight", "24")]);
        let long = "é".repeat(40);
        let j = ItemInfo::new(8, &long);
        assert_eq!(j.name(), "é".repeat(23), "cut at a character, the NUL kept");
        for k in 2..MAX_ITEM_STATS {
            assert!(i.push_stat("x", &k.to_string()));
        }
        assert!(!i.push_stat("one too many", "1"));
    }
}
