//! The player's things as the game keeps them (its inventory), and the body's slots that hand them
//! out: a hand reaching into a slot takes the slot's first thing, the game's own model in the fist,
//! to eat or drink at the mouth, read in front of the eyes, put on a weapon, put back, or drop.
//!
//! **The inventory** ([`Inventory`], adapter -> runtime) is the game's categories (its own names
//! for its kinds of things: food, potions, arrows...) and its things in them ([`InvItem`]), each
//! with the values the slots' orders need (what it restores, its value and weight, how strong it
//! is, when it came) and what using it is (eaten, drunk, read, put on a weapon). The adapter
//! rewrites it whenever it changes. It also gives the game's own idea of a good set of slots
//! (`defaults`), used until the player makes their own.
//!
//! **The slots** ([`SlotDesc`]) are the runtime's: each a place on the body (in the frame of the
//! body anchor the holsters hang from: x right, y up, z forward from the eyes, for a player whose
//! eyes are 1.7 m up; it scales with the player), how big it is and how much further out a hand
//! still takes from it, the categories it draws from in order, the order inside them ([`RULE_*`]),
//! and what reaching into it empty does ([`EMPTY_*`]). The player sets them up in the game's own
//! menus (the adapter's editor, [`SlotEdits`]) and by hand (Move: reach where you want it and
//! squeeze), and the runtime keeps them in its settings, per game. It publishes them, what each
//! gives now and where it is ([`BodySlots`], runtime -> adapter), for the editor.
//!
//! **A thing drawn** from a slot is the runtime's copy of it: an object of its own
//! ([`crate::OS_INVENTORY`], its [`InvItem::id`] in [`crate::ObjectState::parent`]), drawn by the
//! runtime from the model the item names, with the game's lighting. Each use (held at the mouth,
//! read, touched to the weapon) adds one to its state's `shots`, for the game to do its own use of
//! the thing; let go anywhere but a slot, it falls and, at rest, is the game's to drop into the
//! world. When the game has done either it says so ([`Inventory::taken`]) and the runtime lets its
//! copy go. Put back in a slot, it simply goes: it never left the inventory.

use bytemuck::{Pod, Zeroable};

/// After the bows, the last of the chained regions (`MAPPING_BYTES` grew by 64 KB for them): the
/// inventory, the slots and the editor's edits.
pub const OFF_INVENTORY: usize = crate::OFF_BOWS_END.next_multiple_of(64);
pub const MAX_INV_CATEGORIES: usize = 32;
pub const MAX_INV_ITEMS: usize = 256;
pub const MAX_BODY_SLOTS: usize = 12;
/// The categories one slot can draw from.
pub const MAX_SLOT_CATEGORIES: usize = 16;
pub const MAX_TAKEN: usize = 16;
/// [`SlotDesc::categories`]: no category.
pub const SLOT_NO_CATEGORY: u8 = 0xFF;

// ---- what a thing is ([`InvItem::flags`]) --------------------------------------------------------
/// What the player has on (the equipped arrows, the weapon in hand).
pub const INV_EQUIPPED: u32 = 1 << 0;
pub const INV_STOLEN: u32 = 1 << 1;
pub const INV_QUEST: u32 = 1 << 2;
pub const INV_ENCHANTED: u32 = 1 << 3;
/// Eaten: held at the mouth (food, an ingredient).
pub const INV_EAT: u32 = 1 << 4;
/// Drunk: held at the mouth (a potion, a drink).
pub const INV_DRINK: u32 = 1 << 5;
/// Read: held up in front of the eyes (a scroll).
pub const INV_READ: u32 = 1 << 6;
/// Put on a weapon: touched to the one the other hand holds (a poison on a blade).
pub const INV_COAT: u32 = 1 << 7;

// ---- the order a slot gives its things out in ([`SlotDesc::rule`]) ------------------------------
/// What restores health most first, then what restores magicka or fatigue most; what restores none
/// of them isn't given out.
pub const RULE_HEALING: u32 = 0;
/// What restores magicka most first (then health or fatigue); as [`RULE_HEALING`].
pub const RULE_MAGICKA: u32 = 1;
/// What restores fatigue most first (then health or magicka); as [`RULE_HEALING`].
pub const RULE_FATIGUE: u32 = 2;
pub const RULE_VALUE: u32 = 3;
pub const RULE_HEAVIEST: u32 = 4;
pub const RULE_LIGHTEST: u32 = 5;
/// What came last first.
pub const RULE_NEWEST: u32 = 6;
/// By name, A first.
pub const RULE_NAME: u32 = 7;
/// The biggest stack first.
pub const RULE_MOST: u32 = 8;
/// The strongest first ([`InvItem::power`]: a weapon's or an arrow's damage, a soul gem's soul).
pub const RULE_STRONGEST: u32 = 9;
/// The player's own: what they last put back in the slot comes out first, then the newest.
pub const RULE_OWN: u32 = 10;
pub const RULE_COUNT: u32 = 11;

// ---- what reaching into an empty slot does ([`SlotDesc::empty`]) --------------------------------
/// A dull double tick: there's nothing in it.
pub const EMPTY_TICK: u32 = 0;
/// Nothing at all.
pub const EMPTY_SILENT: u32 = 1;
/// The first of everything the player has, by the slot's order (the healing slot's last potion
/// gone: the best food).
pub const EMPTY_ANY: u32 = 2;
pub const EMPTY_COUNT: u32 = 3;

// ---- [`SlotDesc::flags`] -----------------------------------------------------------------------
/// What the player has on comes out first (the equipped arrows).
pub const SLOTF_EQUIPPED_FIRST: u32 = 1 << 0;
/// It counts only with the hand beside or behind the head (over the shoulder), not in front of the
/// face.
pub const SLOTF_BEHIND: u32 = 1 << 1;

/// One of the game's kinds of things.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct InvCategory {
    /// The game's name for it (UTF-8, NUL-terminated): what the player's slots remember it by.
    pub name: [u8; 24],
    pub _pad: [u32; 2],
}

/// A stack of one thing the player has.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct InvItem {
    /// The adapter's number for the stack, stable while it's there; 0 marks an empty entry.
    pub id: u32,
    /// Its category (`Inventory::categories[category]`).
    pub category: u32,
    pub count: u32,
    pub flags: u32, // INV_*
    /// The game's own id for the thing (a form id): what the player's own order remembers.
    pub key: u32,
    /// When it came: bigger is newer.
    pub newest: u32,
    pub value: f32,
    pub weight: f32,
    /// What using one restores (as the game reckons it, its magnitude over its time): health,
    /// magicka, fatigue.
    pub restores: [f32; 3],
    /// How strong it is (a weapon's or an arrow's damage, a soul gem's soul): [`RULE_STRONGEST`].
    pub power: f32,
    /// Kilograms in the hand.
    pub mass: f32,
    pub _p0: u32,
    /// Its box in its model's space (metres): the collision of the one in the hand.
    pub center: [f32; 3],
    pub _p1: f32,
    pub half_extents: [f32; 3],
    pub _p2: f32,
    /// UTF-8, NUL-terminated.
    pub name: [u8; 48],
    /// The model the runtime draws it with, `<game>/<asset key>` (empty: it can't be drawn, so it
    /// isn't handed out).
    pub model: [u8; 48],
    pub _p3: [u32; 2],
    /// The sound set of a thing of its kind in the game's sound pack (`crate::sound`; UTF-8,
    /// NUL-terminated, empty for the pack's defaults): what the one drawn from a slot sounds like.
    pub sound: [u8; 16],
}

/// A slot on the body as it's set up.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct SlotDesc {
    /// What the player calls it (UTF-8, NUL-terminated).
    pub name: [u8; 24],
    /// Its middle, in the body anchor's frame from the eyes (see the module's notes), metres.
    pub offset: [f32; 3],
    /// How big it is (a sphere's radius, metres): a thing let go in it goes back in.
    pub radius: f32,
    /// How much further out a hand still takes from it (metres).
    pub reach: f32,
    pub rule: u32,  // RULE_*
    pub empty: u32, // EMPTY_*
    pub flags: u32, // SLOTF_*
    /// The categories it draws from, first first (indices into the inventory's categories;
    /// [`SLOT_NO_CATEGORY`] after the last).
    pub categories: [u8; MAX_SLOT_CATEGORIES],
    pub _pad: [u32; 2],
}

impl SlotDesc {
    pub fn name(&self) -> &str {
        crate::name_str(&self.name)
    }

    /// The categories it draws from, in order.
    pub fn categories(&self) -> impl Iterator<Item = u8> + '_ {
        self.categories.iter().copied().take_while(|&c| c != SLOT_NO_CATEGORY)
    }
}

/// The adapter's inventory region.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Inventory {
    /// Seqlock word.
    pub seq: u32,
    /// Things in use are `items[..count]`, categories `categories[..category_count]`, the game's
    /// own slots `defaults[..default_count]`.
    pub count: u32,
    pub category_count: u32,
    pub default_count: u32,
    /// How many objects the game has taken back so far: the last [`MAX_TAKEN`] are in `taken`
    /// (`taken[n % MAX_TAKEN]` is the `n`th).
    pub taken_count: u32,
    pub _pad: [u32; 11],
    /// The objects drawn from the slots ([`crate::ObjectDesc::id`]s of [`crate::OS_INVENTORY`]
    /// ones) the game has taken back: used, or dropped into the world. The runtime lets them go.
    pub taken: [u32; MAX_TAKEN],
    pub categories: [InvCategory; MAX_INV_CATEGORIES],
    pub defaults: [SlotDesc; MAX_BODY_SLOTS],
    pub items: [InvItem; MAX_INV_ITEMS],
}

impl Inventory {
    pub fn items(&self) -> &[InvItem] {
        &self.items[..(self.count as usize).min(MAX_INV_ITEMS)]
    }

    pub fn item(&self, id: u32) -> Option<&InvItem> {
        (id != 0).then(|| self.items().iter().find(|i| i.id == id)).flatten()
    }

    pub fn categories(&self) -> &[InvCategory] {
        &self.categories[..(self.category_count as usize).min(MAX_INV_CATEGORIES)]
    }

    /// A category's number by its name.
    pub fn category(&self, name: &str) -> Option<u8> {
        self.categories().iter().position(|c| crate::name_str(&c.name).eq_ignore_ascii_case(name)).map(|i| i as u8)
    }

    pub fn defaults(&self) -> &[SlotDesc] {
        &self.defaults[..(self.default_count as usize).min(MAX_BODY_SLOTS)]
    }

    /// Adds an object the game has taken back.
    pub fn push_taken(&mut self, id: u32) {
        self.taken[self.taken_count as usize % MAX_TAKEN] = id;
        self.taken_count = self.taken_count.wrapping_add(1);
    }

    /// The objects taken back after the first `seen` (`seen` is then how many there have been).
    pub fn taken_since(&self, seen: &mut u32) -> Vec<u32> {
        let n = self.taken_count;
        let from = if n.wrapping_sub(*seen) > MAX_TAKEN as u32 { n.wrapping_sub(MAX_TAKEN as u32) } else { *seen };
        let out = (0..n.wrapping_sub(from)).map(|k| self.taken[from.wrapping_add(k) as usize % MAX_TAKEN]).collect();
        *seen = n;
        out
    }
}

impl InvItem {
    pub fn name(&self) -> &str {
        crate::name_str(&self.name)
    }

    pub fn model(&self) -> &str {
        crate::name_str(&self.model)
    }

    pub fn sound(&self) -> &str {
        crate::name_str(&self.sound)
    }
}

/// What a slot gives now, and where it is.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct SlotNow {
    /// The [`InvItem::id`] a hand reaching in now takes (0: nothing), and how many it has to give.
    pub item: u32,
    pub count: u32,
    /// Its middle now (tracking space).
    pub at: [f32; 3],
    pub _pad: [u32; 3],
}

/// The runtime's slots region.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct BodySlots {
    /// Seqlock word.
    pub seq: u32,
    /// Slots in use are `slots[..count]` (with `now[..count]`).
    pub count: u32,
    /// Goes up by one each time the player asks for the slots' editor (a grip held in a slot, or
    /// the runtime's menu): open it on slot `edit_slot`.
    pub edit_seq: u32,
    pub edit_slot: u32,
    /// The slot being placed by hand, plus one (0: none): the next squeeze puts it there.
    pub placing: u32,
    /// Goes up by one each time a slot has been placed by hand.
    pub placed_seq: u32,
    /// Goes up by one each time the slots change (from the editor, by hand, or the player's own
    /// order).
    pub revision: u32,
    pub _pad: [u32; 9],
    pub slots: [SlotDesc; MAX_BODY_SLOTS],
    pub now: [SlotNow; MAX_BODY_SLOTS],
}

impl BodySlots {
    pub fn slots(&self) -> &[SlotDesc] {
        &self.slots[..(self.count as usize).min(MAX_BODY_SLOTS)]
    }
}

/// The editor's changes to the slots (adapter -> runtime).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct SlotEdits {
    /// Seqlock word.
    pub seq: u32,
    /// Goes up by one with each change: the slots are then `slots[..count]`.
    pub revision: u32,
    pub count: u32,
    /// Goes up by one each time the player asks to place a slot by hand: slot `place_slot` (its
    /// index; past the last: stop placing).
    pub place_seq: u32,
    pub place_slot: u32,
    /// While the editor is open, the slot picked in it, plus one (0: it's closed): the runtime shows
    /// that slot's first thing at its place on the body (the game's model, still), so the player
    /// sees where it is.
    pub editing: u32,
    pub _pad: [u32; 10],
    pub slots: [SlotDesc; MAX_BODY_SLOTS],
}

pub const OFF_BODY_SLOTS: usize = (OFF_INVENTORY + size_of::<Inventory>()).next_multiple_of(64);
pub const OFF_SLOT_EDITS: usize = (OFF_BODY_SLOTS + size_of::<BodySlots>()).next_multiple_of(64);

const _: () = {
    assert!(size_of::<InvCategory>() == 32);
    assert!(size_of::<InvItem>() == 208);
    assert!(size_of::<SlotDesc>() == 80);
    assert!(size_of::<SlotNow>() == 32);
    assert!(OFF_INVENTORY.is_multiple_of(64));
    assert!(OFF_SLOT_EDITS + size_of::<SlotEdits>() <= crate::hud::OFF_HUD_LAYOUT);
};

