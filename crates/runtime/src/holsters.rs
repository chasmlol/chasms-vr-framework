//! The body's slots between the rig, the runtime's settings and the game (`rustvr_protocol::
//! inventory`): the game's inventory into the rig (and the models of what its slots hand out
//! loaded to draw); the slots in use, the player's own for this game from the settings or else the
//! game's own; what the player does with them (a slot placed by hand, the player's own order, the
//! editor asked for) kept in the settings; the slots and what each gives now published for the
//! game's editor, and its edits taken back.

use rustvr_gpu::Gpu;
use rustvr_protocol::link::Link;
use rustvr_protocol::*;

use crate::held_objects::HeldObjects;
use crate::rig::{PocketEvent, Rig, pockets};
use crate::settings::{BodySlot, SettingsStore, SlotOrder};

/// How many of what each slot gives first are loaded to draw ahead of being drawn.
const PRELOAD: usize = 3;
/// How long the player's own order of a slot is (the things last put back in it).
const OWN_LEN: usize = 24;

pub struct HolsterLink {
    inventory: Box<Inventory>,
    inventory_seq: u32,
    out: Box<BodySlots>,
    edits: Box<SlotEdits>,
    edits_revision: u32,
    place_seq: u32,
    /// The slots last published, to number their revisions.
    published: Vec<BodySlot>,
}

impl Default for HolsterLink {
    fn default() -> Self {
        HolsterLink { inventory: bytemuck::zeroed_box(), inventory_seq: 0, out: bytemuck::zeroed_box(), edits: bytemuck::zeroed_box(), edits_revision: 0, place_seq: 0, published: Vec::new() }
    }
}

/// The game's key in the settings (its adapter's name, as its profile is named).
fn key(game: &str) -> String {
    game.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c.to_ascii_lowercase() } else { '_' }).collect()
}

impl HolsterLink {
    /// A new connection: what the last game said is forgotten (the editor's revisions too).
    pub fn reset(&mut self) {
        let (edit, placed) = (self.out.edit_seq, self.out.placed_seq);
        *self = HolsterLink::default();
        // (The counters go on, so a game that saw them before doesn't take an old one for new.)
        self.out.edit_seq = edit;
        self.out.placed_seq = placed;
    }

    /// The slots in use for `game`: the player's own, or the game's (mirrored for a left hand).
    pub fn slots(&self, settings: &SettingsStore, game: &str) -> Vec<BodySlot> {
        match settings.current.holsters.games.get(&key(game)) {
            Some(s) if !s.is_empty() => s.clone(),
            _ => pockets::defaults(&self.inventory, settings.current.dominant()),
        }
    }

    /// The player's own slots for `game` to change (the game's own copied in, the first time).
    fn own<'a>(&self, settings: &'a mut SettingsStore, game: &str) -> &'a mut Vec<BodySlot> {
        let slots = self.slots(settings, game);
        let own = settings.current.holsters.games.entry(key(game)).or_default();
        if own.is_empty() {
            *own = slots;
        }
        own
    }

    /// Once a frame before the rig's step, while a game is connected: its inventory (when it
    /// changes) into the rig, and the models of what the slots give first loaded; the editor's
    /// edits into the settings (saved); the slots in use into the rig.
    pub fn before(&mut self, link: &Link, rig: &mut Rig, settings: &mut SettingsStore, game: &str, gpu: &Gpu, held: &mut HeldObjects) {
        let seq = link.inventory_seq();
        if seq != self.inventory_seq && link.read_inventory_into(&mut self.inventory) {
            self.inventory_seq = seq;
            rig.set_inventory(&self.inventory);
            let out = rig.drawn_out();
            let mut descs = Vec::new();
            for slot in self.slots(settings, game) {
                for item in pockets::contents(&slot, &self.inventory, &out).into_iter().take(PRELOAD) {
                    descs.push(pockets::object_desc(item, item.id));
                }
            }
            held.sync_descs(gpu, &descs);
            tracing::info!(
                "holsters: the game's inventory: {} things in {} categories, {} slots of its own",
                self.inventory.count,
                self.inventory.category_count,
                self.inventory.default_count
            );
        }
        if link.read_slot_edits_into(&mut self.edits) {
            if self.edits.revision != 0 && self.edits.revision != self.edits_revision {
                self.edits_revision = self.edits.revision;
                let n = (self.edits.count as usize).min(MAX_BODY_SLOTS);
                let was = self.slots(settings, game);
                let slots: Vec<BodySlot> = self.edits.slots[..n].iter().enumerate().map(|(k, d)| pockets::from_desc(d, &self.inventory, was.get(k))).collect();
                tracing::info!("holsters: the editor's slots: {:?}", slots.iter().map(|s| format!("{} {:?} {:?}", s.name, s.categories, s.order)).collect::<Vec<_>>());
                settings.current.holsters.games.insert(key(game), slots);
                save(settings);
            }
            if self.edits.place_seq != self.place_seq {
                self.place_seq = self.edits.place_seq;
                let slot = self.edits.place_slot as usize;
                rig.place_pocket((slot < MAX_BODY_SLOTS).then_some(slot));
                tracing::info!("holsters: slot {slot} to be placed by hand");
            }
        }
        rig.set_pockets(&self.slots(settings, game));
    }

    /// After the rig's step: what the player did with the slots, kept; and the slots for the game.
    pub fn after(&mut self, link: &Link, rig: &mut Rig, settings: &mut SettingsStore, game: &str) {
        for ev in std::mem::take(&mut rig.pocket_events) {
            match ev {
                PocketEvent::Edit(k) => self.request_editor(k),
                PocketEvent::Placed(k, at) => {
                    if let Some(s) = self.own(settings, game).get_mut(k) {
                        s.at = at.to_array();
                    }
                    save(settings);
                    self.out.placed_seq = self.out.placed_seq.wrapping_add(1);
                }
                PocketEvent::PutBack(k, key) => {
                    if self.slots(settings, game).get(k).is_some_and(|s| s.order == SlotOrder::Own) {
                        if let Some(s) = self.own(settings, game).get_mut(k) {
                            s.own.retain(|&x| x != key);
                            s.own.insert(0, key);
                            s.own.truncate(OWN_LEN);
                        }
                        save(settings);
                    }
                }
            }
        }
        let slots = self.slots(settings, game);
        if slots != self.published {
            self.out.revision = self.out.revision.wrapping_add(1);
            self.published = slots.clone();
        }
        let now = rig.pockets_now();
        let n = slots.len().min(MAX_BODY_SLOTS);
        self.out.count = n as u32;
        for k in 0..MAX_BODY_SLOTS {
            self.out.slots[k] = if k < n { pockets::to_desc(&slots[k], &self.inventory) } else { bytemuck::Zeroable::zeroed() };
            self.out.now[k] = now.get(k).copied().unwrap_or_default();
        }
        self.out.placing = rig.placing().map_or(0, |k| k as u32 + 1);
        link.write_body_slots(&self.out);
    }

    /// Asks the game for the slots' editor, on slot `slot` (the runtime's menu, or a grip held in a
    /// slot).
    pub fn request_editor(&mut self, slot: usize) {
        self.out.edit_seq = self.out.edit_seq.wrapping_add(1);
        self.out.edit_slot = slot as u32;
        tracing::info!("holsters: the slots' editor asked for, on slot {slot}");
    }
}

fn save(settings: &mut SettingsStore) {
    if let Err(e) = settings.save() {
        tracing::warn!("holsters: saving the settings: {e:#}");
    }
}
