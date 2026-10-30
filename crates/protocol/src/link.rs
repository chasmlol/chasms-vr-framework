//! The Windows side of the protocol: the file mapping, seqlocked slots, the slot triple buffer,
//! the event ring and the named wake-up events. Both the runtime and the adapter SDK use this.

use crate::*;
use bytemuck::Pod;
use std::mem::offset_of;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering, fence};
use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HLOCAL, LocalFree, WAIT_OBJECT_0};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, OpenFileMappingW, PAGE_READWRITE,
    UnmapViewOfFile,
};
use windows::Win32::System::SystemInformation::GetTickCount64;
use windows::Win32::System::Threading::{
    CreateEventW, EVENT_MODIFY_STATE, GetCurrentProcess, GetCurrentProcessId, OpenEventW, OpenProcessToken,
    SYNCHRONIZATION_SYNCHRONIZE, SetEvent, WaitForSingleObject,
};
use windows::core::{HSTRING, PWSTR};

pub fn now_ms() -> u64 {
    unsafe { GetTickCount64() }
}

/// Who may open the mapping, events and shared textures: this Windows user (plus the system and
/// administrators), at medium integrity. Said explicitly so an elevated runtime and a non-elevated
/// game (or the other way round) can still talk.
pub struct Security {
    descriptor: PSECURITY_DESCRIPTOR,
}

impl Security {
    pub fn for_this_user() -> Option<Security> {
        unsafe {
            let mut sddl = String::from("D:P(A;;GA;;;SY)(A;;GA;;;BA)");
            let mut token = HANDLE::default();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_ok() {
                let mut size = 0u32;
                let _ = GetTokenInformation(token, TokenUser, None, 0, &mut size);
                let mut buf = vec![0u8; size as usize];
                if size > 0
                    && GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr().cast()), size, &mut size).is_ok()
                {
                    let user = &*(buf.as_ptr() as *const TOKEN_USER);
                    let mut sid = PWSTR::null();
                    if ConvertSidToStringSidW(user.User.Sid, &mut sid).is_ok() {
                        sddl += &format!("(A;;GA;;;{})", sid.to_string().unwrap_or_default());
                        let _ = LocalFree(Some(HLOCAL(sid.0.cast())));
                    }
                }
                let _ = CloseHandle(token);
            }
            sddl += "S:(ML;;NW;;;ME)";
            let mut descriptor = PSECURITY_DESCRIPTOR::default();
            ConvertStringSecurityDescriptorToSecurityDescriptorW(&HSTRING::from(sddl), SDDL_REVISION_1, &mut descriptor, None)
                .ok()?;
            Some(Security { descriptor })
        }
    }

    pub fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.descriptor.0,
            bInheritHandle: false.into(),
        }
    }
}

impl Drop for Security {
    fn drop(&mut self) {
        unsafe {
            let _ = LocalFree(Some(HLOCAL(self.descriptor.0)));
        }
    }
}

/// A view of the shared mapping. The runtime [`Link::create`]s it; adapters [`Link::open`] it.
pub struct Link {
    name: String,
    mapping: HANDLE,
    base: *mut u8,
    frame_event: HANDLE,
    submit_event: HANDLE,
}

// The mapping is process-shared memory accessed only through atomics; handles are thread-safe.
unsafe impl Send for Link {}
unsafe impl Sync for Link {}

impl Link {
    /// Creates (or takes over) the mapping. Runtime side.
    pub fn create(name: &str) -> windows::core::Result<Link> {
        unsafe {
            let sec = Security::for_this_user();
            let sa = sec.as_ref().map(|s| s.attributes());
            let sa_ptr = sa.as_ref().map(|s| s as *const SECURITY_ATTRIBUTES);
            let mapping = CreateFileMappingW(
                windows::Win32::Foundation::INVALID_HANDLE_VALUE,
                sa_ptr,
                PAGE_READWRITE,
                0,
                MAPPING_BYTES as u32,
                &HSTRING::from(name),
            )?;
            let existed = GetLastError() == ERROR_ALREADY_EXISTS;
            let base = Self::map(mapping)?;
            let frame_event = CreateEventW(sa_ptr, false, false, &HSTRING::from(event_name(name, "frame")))?;
            let submit_event = CreateEventW(sa_ptr, false, false, &HSTRING::from(event_name(name, "submit")))?;
            let link = Link { name: name.to_owned(), mapping, base, frame_event, submit_event };
            if !existed {
                std::ptr::write_bytes(base, 0, MAPPING_BYTES);
            }
            link.init_runtime_header();
            Ok(link)
        }
    }

    /// Opens a mapping the runtime created. Adapter side. Fails while no runtime is running.
    pub fn open(name: &str) -> windows::core::Result<Link> {
        unsafe {
            let mapping = OpenFileMappingW(FILE_MAP_ALL_ACCESS.0, false, &HSTRING::from(name))?;
            let base = match Self::map(mapping) {
                Ok(b) => b,
                Err(e) => {
                    let _ = CloseHandle(mapping);
                    return Err(e);
                }
            };
            let access = EVENT_MODIFY_STATE | SYNCHRONIZATION_SYNCHRONIZE;
            let frame_event = OpenEventW(access, false, &HSTRING::from(event_name(name, "frame")));
            let submit_event = OpenEventW(access, false, &HSTRING::from(event_name(name, "submit")));
            let link = Link {
                name: name.to_owned(),
                mapping,
                base,
                frame_event: frame_event.unwrap_or_default(),
                submit_event: submit_event.unwrap_or_default(),
            };
            let magic = link.u32_at(OFF_HEADER + offset_of!(Header, magic)).load(Ordering::Acquire);
            let version = link.u32_at(OFF_HEADER + offset_of!(Header, version)).load(Ordering::Acquire);
            if magic != MAGIC || version != VERSION {
                return Err(windows::core::Error::new(
                    windows::Win32::Foundation::E_FAIL,
                    format!("RustVR mapping has magic {magic:#x} version {version}, want {MAGIC:#x} {VERSION}"),
                ));
            }
            Ok(link)
        }
    }

    unsafe fn map(mapping: HANDLE) -> windows::core::Result<*mut u8> {
        let view = unsafe { MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, MAPPING_BYTES) };
        if view.Value.is_null() {
            return Err(windows::core::Error::from_thread());
        }
        Ok(view.Value as *mut u8)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    fn u32_at(&self, off: usize) -> &AtomicU32 {
        debug_assert!(off.is_multiple_of(4) && off + 4 <= MAPPING_BYTES);
        unsafe { &*(self.base.add(off) as *const AtomicU32) }
    }

    fn u64_at(&self, off: usize) -> &AtomicU64 {
        debug_assert!(off.is_multiple_of(8) && off + 8 <= MAPPING_BYTES);
        unsafe { &*(self.base.add(off) as *const AtomicU64) }
    }

    // ---- header ---------------------------------------------------------------------------------
    fn init_runtime_header(&self) {
        let h = OFF_HEADER;
        self.u32_at(h + offset_of!(Header, mapping_bytes)).store(MAPPING_BYTES as u32, Ordering::Relaxed);
        self.u32_at(h + offset_of!(Header, runtime_pid)).store(unsafe { GetCurrentProcessId() }, Ordering::Relaxed);
        self.u64_at(h + offset_of!(Header, runtime_heartbeat_ms)).store(now_ms(), Ordering::Relaxed);
        // A runtime (re)start forces the adapter to reconnect: it sees the ack no longer matches.
        self.u32_at(h + offset_of!(Header, runtime_ack_epoch)).store(u32::MAX, Ordering::Relaxed);
        self.reset_submit();
        self.u32_at(h + offset_of!(Header, version)).store(VERSION, Ordering::Relaxed);
        self.u32_at(h + offset_of!(Header, magic)).store(MAGIC, Ordering::Release);
    }

    pub fn heartbeat_runtime(&self) {
        self.u64_at(OFF_HEADER + offset_of!(Header, runtime_heartbeat_ms)).store(now_ms(), Ordering::Release);
    }

    pub fn heartbeat_adapter(&self) {
        self.u64_at(OFF_HEADER + offset_of!(Header, adapter_heartbeat_ms)).store(now_ms(), Ordering::Release);
    }

    pub fn runtime_alive(&self) -> bool {
        let t = self.u64_at(OFF_HEADER + offset_of!(Header, runtime_heartbeat_ms)).load(Ordering::Acquire);
        now_ms().saturating_sub(t) <= HEARTBEAT_TIMEOUT_MS
    }

    pub fn adapter_alive(&self) -> bool {
        let pid = self.u32_at(OFF_HEADER + offset_of!(Header, adapter_pid)).load(Ordering::Acquire);
        let t = self.u64_at(OFF_HEADER + offset_of!(Header, adapter_heartbeat_ms)).load(Ordering::Acquire);
        pid != 0 && now_ms().saturating_sub(t) <= HEARTBEAT_TIMEOUT_MS
    }

    pub fn adapter_pid(&self) -> u32 {
        self.u32_at(OFF_HEADER + offset_of!(Header, adapter_pid)).load(Ordering::Acquire)
    }

    pub fn runtime_pid(&self) -> u32 {
        self.u32_at(OFF_HEADER + offset_of!(Header, runtime_pid)).load(Ordering::Acquire)
    }

    // ---- connect handshake ----------------------------------------------------------------------
    /// Adapter: announce a (re)connect. Returns the epoch to wait for with [`Link::connect_acked`].
    pub fn begin_connect(&self) -> u32 {
        let h = OFF_HEADER;
        self.u32_at(h + offset_of!(Header, adapter_pid)).store(unsafe { GetCurrentProcessId() }, Ordering::Relaxed);
        self.heartbeat_adapter();
        
        self.u32_at(h + offset_of!(Header, adapter_epoch)).fetch_add(1, Ordering::AcqRel).wrapping_add(1)
    }

    /// Adapter: the runtime has reset the triple buffer for this epoch (back slot = 0).
    pub fn connect_acked(&self, epoch: u32) -> bool {
        self.u32_at(OFF_HEADER + offset_of!(Header, runtime_ack_epoch)).load(Ordering::Acquire) == epoch
    }

    /// Runtime: if the adapter announced a new epoch, reset the triple buffer (front = 2), ack it and
    /// return true. A new connection starts with no sound pack (a game that has one publishes it
    /// once acked), so one game's sounds never carry over to the next.
    pub fn poll_connect(&self) -> bool {
        let h = OFF_HEADER;
        let epoch = self.u32_at(h + offset_of!(Header, adapter_epoch)).load(Ordering::Acquire);
        let ack = self.u32_at(h + offset_of!(Header, runtime_ack_epoch));
        if ack.load(Ordering::Acquire) == epoch || self.adapter_pid() == 0 {
            return false;
        }
        self.reset_submit();
        if self.sound_pack_seq() != 0 {
            self.write_sound_pack(&bytemuck::allocation::zeroed_box::<SoundPack>());
        }
        ack.store(epoch, Ordering::Release);
        true
    }

    // ---- seqlocked regions ----------------------------------------------------------------------
    pub fn write_rt_frame(&self, v: &RtFrame) {
        unsafe { seq_write(self.base.add(OFF_RT_FRAME), v) }
    }
    pub fn read_rt_frame(&self) -> Option<RtFrame> {
        unsafe { seq_read(self.base.add(OFF_RT_FRAME)) }
    }
    pub fn write_game_state(&self, v: &GameState) {
        unsafe { seq_write(self.base.add(OFF_GAME_STATE), v) }
    }
    pub fn read_game_state(&self) -> Option<GameState> {
        unsafe { seq_read(self.base.add(OFF_GAME_STATE)) }
    }
    pub fn write_targets(&self, v: &Targets) {
        unsafe { seq_write(self.base.add(OFF_TARGETS), v) }
    }
    pub fn read_targets(&self) -> Option<Targets> {
        unsafe { seq_read(self.base.add(OFF_TARGETS)) }
    }

    // ---- body and objects -----------------------------------------------------------------------
    pub fn write_body(&self, v: &RtBody) {
        unsafe { seq_write(self.base.add(OFF_BODY), v) }
    }
    pub fn read_body(&self) -> Option<RtBody> {
        unsafe { seq_read(self.base.add(OFF_BODY)) }
    }
    /// Adapter: where the game's own simulated objects are now (see [`GamePoses`]).
    pub fn write_game_poses(&self, v: &GamePoses) {
        unsafe { seq_write(self.base.add(OFF_GAME_POSES), v) }
    }
    /// Runtime: where the hands push the game's simulated objects (see [`GamePushes`]).
    pub fn write_game_pushes(&self, v: &GamePushes) {
        unsafe { seq_write(self.base.add(OFF_GAME_PUSHES), v) }
    }
    /// Adapter: read the hands' pushes into `out`.
    pub fn read_game_pushes_into(&self, out: &mut GamePushes) -> bool {
        unsafe { seq_read_into(self.base.add(OFF_GAME_PUSHES), out) }
    }
    /// Adapter: the game objects' convex collision (see [`GameHulls`]).
    pub fn write_game_hulls(&self, v: &GameHulls) {
        unsafe { seq_write(self.base.add(OFF_GAME_HULLS), v) }
    }
    /// Runtime: read the game objects' convex collision into `out`.
    pub fn read_game_hulls_into(&self, out: &mut GameHulls) -> bool {
        unsafe { seq_read_into(self.base.add(OFF_GAME_HULLS), out) }
    }
    /// Runtime: read the game's simulated objects' poses into `out`.
    pub fn read_game_poses_into(&self, out: &mut GamePoses) -> bool {
        unsafe { seq_read_into(self.base.add(OFF_GAME_POSES), out) }
    }

    // ---- combat ([`crate::combat`]) -------------------------------------------------------------
    /// Adapter: the creatures near the player, every frame.
    pub fn write_actors(&self, v: &Actors) {
        unsafe { seq_write(self.base.add(OFF_ACTORS), v) }
    }
    pub fn read_actors_into(&self, out: &mut Actors) -> bool {
        unsafe { seq_read_into(self.base.add(OFF_ACTORS), out) }
    }
    /// Adapter: what grabbing the creatures needs (see [`ActorBodies`]), every frame with them.
    pub fn write_actor_bodies(&self, v: &ActorBodies) {
        unsafe { seq_write(self.base.add(OFF_ACTOR_BODIES), v) }
    }
    pub fn read_actor_bodies_into(&self, out: &mut ActorBodies) -> bool {
        unsafe { seq_read_into(self.base.add(OFF_ACTOR_BODIES), out) }
    }
    /// Runtime: each hand's grab of a creature (see [`CreatureGrabs`]), every frame.
    pub fn write_creature_grabs(&self, v: &CreatureGrabs) {
        unsafe { seq_write(self.base.add(OFF_CREATURE_GRABS), v) }
    }
    pub fn read_creature_grabs_into(&self, out: &mut CreatureGrabs) -> bool {
        unsafe { seq_read_into(self.base.add(OFF_CREATURE_GRABS), out) }
    }
    /// Adapter: the blades of the melee weapons it offers.
    pub fn write_melee(&self, v: &MeleeTable) {
        unsafe { seq_write(self.base.add(OFF_MELEE), v) }
    }
    pub fn read_melee_into(&self, out: &mut MeleeTable) -> bool {
        unsafe { seq_read_into(self.base.add(OFF_MELEE), out) }
    }
    /// Adapter: the player's spells and magicka.
    pub fn write_spells(&self, v: &Spells) {
        unsafe { seq_write(self.base.add(OFF_SPELLS), v) }
    }
    pub fn read_spells_into(&self, out: &mut Spells) -> bool {
        unsafe { seq_read_into(self.base.add(OFF_SPELLS), out) }
    }
    /// Runtime: each hand's casting, every frame (see [`crate::Casters`]).
    pub fn write_casters(&self, v: &Casters) {
        unsafe { seq_write(self.base.add(OFF_CASTERS), v) }
    }
    pub fn read_casters_into(&self, out: &mut Casters) -> bool {
        unsafe { seq_read_into(self.base.add(OFF_CASTERS), out) }
    }
    /// Adapter: the game's effect shaders on what the runtime draws (see [`crate::Overlays`]).
    pub fn write_overlays(&self, v: &Overlays) {
        unsafe { seq_write(self.base.add(OFF_OVERLAYS), v) }
    }
    pub fn read_overlays_into(&self, out: &mut Overlays) -> bool {
        unsafe { seq_read_into(self.base.add(OFF_OVERLAYS), out) }
    }
    /// Adapter: the player's body as the game makes it up.
    pub fn write_avatar(&self, v: &AvatarDesc) {
        unsafe { seq_write(self.base.add(OFF_AVATAR), v) }
    }
    pub fn read_avatar_into(&self, out: &mut AvatarDesc) -> bool {
        unsafe { seq_read_into(self.base.add(OFF_AVATAR), out) }
    }
    /// Adapter: the player's face ([`FaceDesc`]).
    pub fn write_face(&self, v: &FaceDesc) {
        unsafe { seq_write(self.base.add(OFF_FACE), v) }
    }
    pub fn read_face_into(&self, out: &mut FaceDesc) -> bool {
        unsafe { seq_read_into(self.base.add(OFF_FACE), out) }
    }
    /// Adapter: what the things it offers are ([`ItemInfos`]).
    pub fn write_item_infos(&self, v: &ItemInfos) {
        unsafe { seq_write(self.base.add(OFF_ITEM_INFO), v) }
    }
    pub fn read_item_infos_into(&self, out: &mut ItemInfos) -> bool {
        unsafe { seq_read_into(self.base.add(OFF_ITEM_INFO), out) }
    }
    /// Adapter: how its bows bend and their arrows fly ([`Bows`]).
    pub fn write_bows(&self, v: &Bows) {
        unsafe { seq_write(self.base.add(OFF_BOWS), v) }
    }
    pub fn read_bows_into(&self, out: &mut Bows) -> bool {
        unsafe { seq_read_into(self.base.add(OFF_BOWS), out) }
    }
    /// Adapter: the mount the player rides and the mounts near them ([`Ride`]), every game frame.
    pub fn write_ride(&self, v: &Ride) {
        unsafe { seq_write(self.base.add(OFF_RIDE), v) }
    }
    pub fn read_ride_into(&self, out: &mut Ride) -> bool {
        unsafe { seq_read_into(self.base.add(OFF_RIDE), out) }
    }
    /// Runtime: what the rider does with the mount ([`RtRide`]), every runtime frame.
    pub fn write_rt_ride(&self, v: &RtRide) {
        unsafe { seq_write(self.base.add(OFF_RT_RIDE), v) }
    }
    pub fn read_rt_ride(&self) -> Option<RtRide> {
        unsafe { seq_read(self.base.add(OFF_RT_RIDE)) }
    }
    /// Adapter: the player's things and the game's own slots ([`Inventory`]): when they change.
    pub fn write_inventory(&self, v: &Inventory) {
        unsafe { seq_write(self.base.add(OFF_INVENTORY), v) }
    }
    pub fn read_inventory_into(&self, out: &mut Inventory) -> bool {
        unsafe { seq_read_into(self.base.add(OFF_INVENTORY), out) }
    }
    /// Changes whenever the adapter rewrites the inventory (0: never written).
    pub fn inventory_seq(&self) -> u32 {
        self.u32_at(OFF_INVENTORY).load(Ordering::Acquire)
    }
    /// Runtime: the body's slots as they are ([`BodySlots`]).
    pub fn write_body_slots(&self, v: &BodySlots) {
        unsafe { seq_write(self.base.add(OFF_BODY_SLOTS), v) }
    }
    pub fn read_body_slots_into(&self, out: &mut BodySlots) -> bool {
        unsafe { seq_read_into(self.base.add(OFF_BODY_SLOTS), out) }
    }
    /// Adapter: the slots' editor's changes ([`SlotEdits`]).
    pub fn write_slot_edits(&self, v: &SlotEdits) {
        unsafe { seq_write(self.base.add(OFF_SLOT_EDITS), v) }
    }
    pub fn read_slot_edits_into(&self, out: &mut SlotEdits) -> bool {
        unsafe { seq_read_into(self.base.add(OFF_SLOT_EDITS), out) }
    }
    /// Runtime: a melee hit, block, cast or loosed arrow for the game.
    pub fn push_combat_event(&self, ev: &CombatEvent) {
        let written = self.u64_at(OFF_COMBAT_EVENTS + offset_of!(CombatEvents, written));
        let n = written.load(Ordering::Relaxed);
        let slot = OFF_COMBAT_EVENTS + offset_of!(CombatEvents, events) + (n as usize % COMBAT_EVENT_SLOTS) * size_of::<CombatEvent>();
        unsafe { plain_write(self.base.add(slot), ev) };
        written.store(n + 1, Ordering::Release);
    }
    /// Adapter: the combat events after the `read` first ones (`read` is then how many there have
    /// been). One that fell more than a ring behind skips ahead to the newest.
    pub fn read_combat_events(&self, read: &mut u64) -> Vec<CombatEvent> {
        let written = self.u64_at(OFF_COMBAT_EVENTS + offset_of!(CombatEvents, written)).load(Ordering::Acquire);
        if written < *read {
            *read = written; // the runtime started again
        }
        let from = (*read).max(written.saturating_sub(COMBAT_EVENT_SLOTS as u64));
        let out = (from..written)
            .map(|n| {
                let slot = OFF_COMBAT_EVENTS + offset_of!(CombatEvents, events) + (n as usize % COMBAT_EVENT_SLOTS) * size_of::<CombatEvent>();
                unsafe { plain_read(self.base.add(slot)) }
            })
            .collect();
        *read = written;
        out
    }
    // ---- gore ([`crate::gore`]) -------------------------------------------------------------------
    /// Adapter: the game's kinds of blood, when they change.
    pub fn write_bloods(&self, v: &BloodTable) {
        unsafe { seq_write(self.base.add(OFF_BLOODS), v) }
    }
    pub fn read_bloods_into(&self, out: &mut BloodTable) -> bool {
        unsafe { seq_read_into(self.base.add(OFF_BLOODS), out) }
    }
    /// Adapter: the creatures' blood and stumps, every frame.
    pub fn write_gore_bodies(&self, v: &GoreBodies) {
        unsafe { seq_write(self.base.add(OFF_GORE_BODIES), v) }
    }
    pub fn read_gore_bodies_into(&self, out: &mut GoreBodies) -> bool {
        unsafe { seq_read_into(self.base.add(OFF_GORE_BODIES), out) }
    }
    /// Adapter: a limb cut off.
    pub fn push_gore_event(&self, ev: &GoreEvent) {
        let written = self.u64_at(OFF_GORE_EVENTS + offset_of!(GoreEvents, written));
        let n = written.load(Ordering::Relaxed);
        let slot = OFF_GORE_EVENTS + offset_of!(GoreEvents, events) + (n as usize % GORE_EVENT_SLOTS) * size_of::<GoreEvent>();
        unsafe { plain_write(self.base.add(slot), ev) };
        written.store(n + 1, Ordering::Release);
    }
    /// Runtime: the gore events after the `read` first ones (`read` is then how many there have
    /// been). One that fell more than a ring behind skips ahead to the newest.
    pub fn read_gore_events(&self, read: &mut u64) -> Vec<GoreEvent> {
        let written = self.u64_at(OFF_GORE_EVENTS + offset_of!(GoreEvents, written)).load(Ordering::Acquire);
        if written < *read {
            *read = written; // the game started again
        }
        let from = (*read).max(written.saturating_sub(GORE_EVENT_SLOTS as u64));
        let out = (from..written)
            .map(|n| {
                let slot = OFF_GORE_EVENTS + offset_of!(GoreEvents, events) + (n as usize % GORE_EVENT_SLOTS) * size_of::<GoreEvent>();
                unsafe { plain_read(self.base.add(slot)) }
            })
            .collect();
        *read = written;
        out
    }
    // ---- pockets ([`crate::pockets`]) -----------------------------------------------------------
    /// Adapter: the creatures' pockets and the game's answers, every frame.
    pub fn write_pockets(&self, v: &Pockets) {
        unsafe { seq_write(self.base.add(OFF_POCKETS), v) }
    }
    pub fn read_pockets_into(&self, out: &mut Pockets) -> bool {
        unsafe { seq_read_into(self.base.add(OFF_POCKETS), out) }
    }
    /// Runtime: what a hand did with a pocket, for the game. Returns its number.
    pub fn push_pocket_event(&self, ev: &PocketEvent) -> u64 {
        let written = self.u64_at(OFF_POCKET_EVENTS + offset_of!(PocketEvents, written));
        let n = written.load(Ordering::Relaxed);
        let slot = OFF_POCKET_EVENTS + offset_of!(PocketEvents, events) + (n as usize % POCKET_EVENT_SLOTS) * size_of::<PocketEvent>();
        unsafe { plain_write(self.base.add(slot), ev) };
        written.store(n + 1, Ordering::Release);
        n
    }
    /// The number the next pocket event pushed will have.
    pub fn pocket_events_written(&self) -> u64 {
        self.u64_at(OFF_POCKET_EVENTS + offset_of!(PocketEvents, written)).load(Ordering::Acquire)
    }
    /// Adapter: the pocket events after the `read` first ones, each with its number (`read` is
    /// then how many there have been). One that fell more than a ring behind skips ahead.
    pub fn read_pocket_events(&self, read: &mut u64) -> Vec<(u64, PocketEvent)> {
        let written = self.pocket_events_written();
        if written < *read {
            *read = written; // the runtime started again
        }
        let from = (*read).max(written.saturating_sub(POCKET_EVENT_SLOTS as u64));
        let out = (from..written)
            .map(|n| {
                let slot = OFF_POCKET_EVENTS + offset_of!(PocketEvents, events) + (n as usize % POCKET_EVENT_SLOTS) * size_of::<PocketEvent>();
                (n, unsafe { plain_read(self.base.add(slot)) })
            })
            .collect();
        *read = written;
        out
    }
    /// Runtime: publish the posed player (see [`RtPlayerDraw`]).
    pub fn write_player_draw(&self, v: &RtPlayerDraw) {
        unsafe { seq_write(self.base.add(OFF_PLAYER_DRAW), v) }
    }
    /// Adapter: read the posed player into `out`. False if nothing was ever written or no
    /// consistent copy was obtained.
    pub fn read_player_draw_into(&self, out: &mut RtPlayerDraw) -> bool {
        unsafe { seq_read_into(self.base.add(OFF_PLAYER_DRAW), out) }
    }
    /// Adapter: publish the object table (call when it changes).
    pub fn write_objects(&self, v: &ObjectTable) {
        unsafe { seq_write(self.base.add(OFF_OBJECTS), v) }
    }
    /// Runtime: read the object table into `out` (it's large: keep one boxed). False if nothing
    /// was ever written or no consistent copy was obtained.
    pub fn read_objects_into(&self, out: &mut ObjectTable) -> bool {
        unsafe { seq_read_into(self.base.add(OFF_OBJECTS), out) }
    }
    /// The object table's sequence number: it changes whenever the adapter rewrites the table.
    pub fn objects_seq(&self) -> u32 {
        self.u32_at(OFF_OBJECTS).load(Ordering::Acquire)
    }
    /// Adapter: publish the hand pose table (call when it changes, like the object table).
    pub fn write_hand_poses(&self, v: &HandPoseTable) {
        unsafe { seq_write(self.base.add(OFF_HAND_POSES), v) }
    }
    /// Runtime: read the hand pose table into `out`. False if nothing was ever written or no
    /// consistent copy was obtained.
    pub fn read_hand_poses_into(&self, out: &mut HandPoseTable) -> bool {
        unsafe { seq_read_into(self.base.add(OFF_HAND_POSES), out) }
    }
    /// The hand pose table's sequence number: it changes whenever the adapter rewrites the table.
    pub fn hand_poses_seq(&self) -> u32 {
        self.u32_at(OFF_HAND_POSES).load(Ordering::Acquire)
    }
    /// Adapter: publish the guns' authored recoil (call when it changes, like the object table).
    pub fn write_kicks(&self, v: &KickTable) {
        unsafe { seq_write(self.base.add(OFF_KICKS), v) }
    }
    /// Runtime: read the kick table into `out`. False if nothing was ever written or no consistent
    /// copy was obtained.
    pub fn read_kicks_into(&self, out: &mut KickTable) -> bool {
        unsafe { seq_read_into(self.base.add(OFF_KICKS), out) }
    }
    /// The kick table's sequence number: it changes whenever the adapter rewrites the table.
    pub fn kicks_seq(&self) -> u32 {
        self.u32_at(OFF_KICKS).load(Ordering::Acquire)
    }
    /// Adapter: publish this frame's lighting (see [`GameLighting`]).
    pub fn write_game_lighting(&self, v: &GameLighting) {
        unsafe { seq_write(self.base.add(OFF_GAME_LIGHTING), v) }
    }
    /// Runtime: read the game's lighting into `out`. False if nothing was ever written or no
    /// consistent copy was obtained.
    pub fn read_game_lighting_into(&self, out: &mut GameLighting) -> bool {
        unsafe { seq_read_into(self.base.add(OFF_GAME_LIGHTING), out) }
    }
    /// Adapter: describe the HUD's pieces (see [`HudLayout`]).
    pub fn write_hud_layout(&self, v: &HudLayout) {
        unsafe { seq_write(self.base.add(OFF_HUD_LAYOUT), v) }
    }
    /// Runtime: read the HUD layout into `out`. False if nothing was ever written or no consistent
    /// copy was obtained.
    pub fn read_hud_layout_into(&self, out: &mut HudLayout) -> bool {
        unsafe { seq_read_into(self.base.add(OFF_HUD_LAYOUT), out) }
    }
    /// Changes whenever the adapter rewrites the HUD layout (0: never written).
    pub fn hud_layout_seq(&self) -> u32 {
        self.u32_at(OFF_HUD_LAYOUT).load(Ordering::Acquire)
    }
    /// Adapter: publish where the game's sound pack is and the objects' sound sets (call when it
    /// changes, and after every reconnect, like the object table).
    pub fn write_sound_pack(&self, v: &SoundPack) {
        unsafe { seq_write(self.base.add(OFF_SOUNDS), v) }
    }
    /// Runtime: read the sound pack region into `out`. False if nothing was ever written or no
    /// consistent copy was obtained.
    pub fn read_sound_pack_into(&self, out: &mut SoundPack) -> bool {
        unsafe { seq_read_into(self.base.add(OFF_SOUNDS), out) }
    }
    /// The sound pack region's sequence number: it changes whenever the adapter rewrites it.
    pub fn sound_pack_seq(&self) -> u32 {
        self.u32_at(OFF_SOUNDS).load(Ordering::Acquire)
    }
    pub fn write_object_states(&self, v: &ObjectStates) {
        unsafe { seq_write(self.base.add(OFF_OBJECT_STATES), v) }
    }
    pub fn read_object_states_into(&self, out: &mut ObjectStates) -> bool {
        unsafe { seq_read_into(self.base.add(OFF_OBJECT_STATES), out) }
    }

    // ---- collision ring (adapter -> runtime) ---------------------------------------------------
    /// Adapter: queue one collision message. False if the runtime has fallen too far behind.
    pub fn push_collision(&self, kind: u32, payload: &[u8]) -> bool {
        debug_assert!(payload.len() % 4 == 0);
        let head = self.u64_at(OFF_COLLISION);
        let tail = self.u64_at(OFF_COLLISION + RING_TAIL_OFF);
        let cap = COL_RING_DATA_BYTES as u64;
        let size = (8 + payload.len() as u64).div_ceil(8) * 8;
        if size > cap / 2 {
            return false;
        }
        let mut h = head.load(Ordering::Relaxed);
        let used = h.wrapping_sub(tail.load(Ordering::Acquire));
        let pos = h % cap;
        // A message never straddles the end: pad to the start instead.
        let pad = if pos + size > cap { cap - pos } else { 0 };
        if used + pad + size > cap {
            return false;
        }
        let data = unsafe { self.base.add(OFF_COLLISION + COL_RING_DATA_OFF) };
        if pad > 0 {
            unsafe { write_words(data.add(pos as usize), &[COL_PAD, (pad - 8) as u32]) };
            h += pad;
        }
        let pos = (h % cap) as usize;
        unsafe {
            write_words(data.add(pos), &[kind, payload.len() as u32]);
            write_bytes_as_words(data.add(pos + 8), payload);
        }
        head.store(h + size, Ordering::Release);
        true
    }

    /// Runtime: calls `f(kind, payload)` for each queued collision message.
    pub fn drain_collision(&self, mut f: impl FnMut(u32, &[u8])) {
        let head = self.u64_at(OFF_COLLISION);
        let tail = self.u64_at(OFF_COLLISION + RING_TAIL_OFF);
        let cap = COL_RING_DATA_BYTES as u64;
        let data = unsafe { self.base.add(OFF_COLLISION + COL_RING_DATA_OFF) };
        let end = head.load(Ordering::Acquire);
        let mut t = tail.load(Ordering::Relaxed);
        let mut buf = Vec::new();
        while t < end {
            let pos = (t % cap) as usize;
            let hdr = unsafe { words(data.add(pos), 2) };
            let kind = hdr[0].load(Ordering::Relaxed);
            let bytes = hdr[1].load(Ordering::Relaxed) as usize;
            let size = (8 + bytes as u64).div_ceil(8) * 8;
            if kind != COL_PAD {
                buf.clear();
                let w = unsafe { words(data.add(pos + 8), bytes / 4) };
                buf.extend(w.iter().flat_map(|x| x.load(Ordering::Relaxed).to_le_bytes()));
                f(kind, &buf);
            }
            t += size;
        }
        tail.store(t, Ordering::Release);
    }

    // ---- triple buffer --------------------------------------------------------------------------
    fn submit_state(&self) -> &AtomicU32 {
        self.u32_at(OFF_SUBMIT + offset_of!(SubmitCtl, state))
    }

    /// Runtime: middle = 1, nothing pending.
    pub fn reset_submit(&self) {
        self.submit_state().store(1, Ordering::Release);
        self.u64_at(OFF_SUBMIT + offset_of!(SubmitCtl, published)).store(0, Ordering::Release);
    }

    /// Adapter: describe the back slot. Call before [`Link::publish`].
    pub fn write_slot_info(&self, slot: usize, info: &SlotInfo) {
        assert!(slot < SLOT_COUNT);
        unsafe { plain_write(self.base.add(OFF_SLOTS + slot * SLOT_STRIDE), info) }
    }

    /// Runtime: read the slot it holds.
    pub fn read_slot_info(&self, slot: usize) -> SlotInfo {
        assert!(slot < SLOT_COUNT);
        unsafe { plain_read(self.base.add(OFF_SLOTS + slot * SLOT_STRIDE)) }
    }

    /// Adapter: hand over the finished back slot; returns the new back slot to render into.
    pub fn publish(&self, back: usize) -> usize {
        let prev = self.submit_state().swap(back as u32 | SUBMIT_DIRTY, Ordering::AcqRel);
        self.u64_at(OFF_SUBMIT + offset_of!(SubmitCtl, published)).fetch_add(1, Ordering::AcqRel);
        (prev & 3) as usize
    }

    /// Runtime: if a new slot was published, swap it in. Returns the new front slot.
    pub fn take(&self, front: usize) -> Option<usize> {
        if self.submit_state().load(Ordering::Acquire) & SUBMIT_DIRTY == 0 {
            return None;
        }
        let prev = self.submit_state().swap(front as u32, Ordering::AcqRel);
        Some((prev & 3) as usize)
    }

    pub fn published(&self) -> u64 {
        self.u64_at(OFF_SUBMIT + offset_of!(SubmitCtl, published)).load(Ordering::Acquire)
    }

    // ---- event ring (adapter -> runtime) --------------------------------------------------------
    pub fn push_event(&self, ev: &Event) -> bool {
        let head = self.u64_at(OFF_EVENTS + RING_HEAD_OFF);
        let tail = self.u64_at(OFF_EVENTS + RING_TAIL_OFF);
        let h = head.load(Ordering::Relaxed);
        if h.wrapping_sub(tail.load(Ordering::Acquire)) >= EVENT_RING_ENTRIES as u64 {
            return false; // the runtime fell a full ring behind: drop
        }
        let idx = (h as usize) & (EVENT_RING_ENTRIES - 1);
        unsafe { plain_write(self.base.add(OFF_EVENTS + RING_DATA_OFF + idx * size_of::<Event>()), ev) };
        head.store(h + 1, Ordering::Release);
        true
    }

    pub fn pop_event(&self) -> Option<Event> {
        let head = self.u64_at(OFF_EVENTS + RING_HEAD_OFF);
        let tail = self.u64_at(OFF_EVENTS + RING_TAIL_OFF);
        let t = tail.load(Ordering::Relaxed);
        if t == head.load(Ordering::Acquire) {
            return None;
        }
        let idx = (t as usize) & (EVENT_RING_ENTRIES - 1);
        let ev = unsafe { plain_read(self.base.add(OFF_EVENTS + RING_DATA_OFF + idx * size_of::<Event>())) };
        tail.store(t + 1, Ordering::Release);
        Some(ev)
    }

    // ---- wake-ups -------------------------------------------------------------------------------
    pub fn signal_frame(&self) {
        if !self.frame_event.is_invalid() {
            unsafe {
                let _ = SetEvent(self.frame_event);
            }
        }
    }

    /// Adapter: wait up to `ms` for the runtime's next frame. False on timeout.
    pub fn wait_frame(&self, ms: u32) -> bool {
        wait(self.frame_event, ms)
    }

    pub fn signal_submit(&self) {
        if !self.submit_event.is_invalid() {
            unsafe {
                let _ = SetEvent(self.submit_event);
            }
        }
    }

    /// Runtime: wait up to `ms` for the adapter's next submit. False on timeout.
    pub fn wait_submit(&self, ms: u32) -> bool {
        wait(self.submit_event, ms)
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: self.base.cast() });
            let _ = CloseHandle(self.mapping);
            for h in [self.frame_event, self.submit_event] {
                if !h.is_invalid() {
                    let _ = CloseHandle(h);
                }
            }
        }
    }
}

fn wait(h: HANDLE, ms: u32) -> bool {
    if h.is_invalid() {
        std::thread::sleep(std::time::Duration::from_millis(ms.min(2) as u64));
        return false;
    }
    unsafe { WaitForSingleObject(h, ms) == WAIT_OBJECT_0 }
}

// ---- word-wise copies -----------------------------------------------------------------------------
// Shared memory is only ever touched through 32-bit atomics, so concurrent access is defined
// behaviour; the seqlock (or the triple buffer's swap) supplies consistency and ordering.

unsafe fn words<'a>(p: *const u8, n: usize) -> &'a [AtomicU32] {
    debug_assert!((p as usize).is_multiple_of(4));
    unsafe { std::slice::from_raw_parts(p as *const AtomicU32, n) }
}

/// Seqlock write: `seq` (the first `u32` of `T`) is odd while the body is written.
unsafe fn seq_write<T: Pod>(p: *mut u8, v: &T) {
    let n = size_of::<T>() / 4;
    let w = unsafe { words(p, n) };
    let src: &[u32] = bytemuck::cast_slice(bytemuck::bytes_of(v));
    let s = w[0].load(Ordering::Relaxed) & !1;
    w[0].store(s.wrapping_add(1), Ordering::Relaxed);
    fence(Ordering::Release);
    for i in 1..n {
        w[i].store(src[i], Ordering::Relaxed);
    }
    w[0].store(s.wrapping_add(2), Ordering::Release);
}

/// Seqlock read. None if no consistent copy was obtained (the writer is mid-write repeatedly) or
/// nothing was ever written.
unsafe fn seq_read<T: Pod>(p: *const u8) -> Option<T> {
    let n = size_of::<T>() / 4;
    let w = unsafe { words(p, n) };
    let mut out = T::zeroed();
    let dst: &mut [u32] = bytemuck::cast_slice_mut(bytemuck::bytes_of_mut(&mut out));
    for _ in 0..256 {
        let s1 = w[0].load(Ordering::Acquire);
        if s1 == 0 {
            return None;
        }
        if s1 & 1 != 0 {
            std::hint::spin_loop();
            continue;
        }
        for i in 1..n {
            dst[i] = w[i].load(Ordering::Relaxed);
        }
        fence(Ordering::Acquire);
        if w[0].load(Ordering::Relaxed) == s1 {
            dst[0] = s1;
            return Some(out);
        }
    }
    None
}

/// Seqlock read into a caller-owned value (for tables too big for the stack).
unsafe fn seq_read_into<T: Pod>(p: *const u8, out: &mut T) -> bool {
    let n = size_of::<T>() / 4;
    let w = unsafe { words(p, n) };
    let dst: &mut [u32] = bytemuck::cast_slice_mut(bytemuck::bytes_of_mut(out));
    for _ in 0..64 {
        let s1 = w[0].load(Ordering::Acquire);
        if s1 == 0 {
            return false;
        }
        if s1 & 1 != 0 {
            std::hint::spin_loop();
            continue;
        }
        for i in 1..n {
            dst[i] = w[i].load(Ordering::Relaxed);
        }
        fence(Ordering::Acquire);
        if w[0].load(Ordering::Relaxed) == s1 {
            dst[0] = s1;
            return true;
        }
    }
    false
}

unsafe fn write_words(p: *mut u8, v: &[u32]) {
    let w = unsafe { words(p, v.len()) };
    for (dst, src) in w.iter().zip(v) {
        dst.store(*src, Ordering::Relaxed);
    }
}

unsafe fn write_bytes_as_words(p: *mut u8, bytes: &[u8]) {
    let w = unsafe { words(p, bytes.len() / 4) };
    for (dst, chunk) in w.iter().zip(bytes.chunks_exact(4)) {
        dst.store(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]), Ordering::Relaxed);
    }
}

unsafe fn plain_write<T: Pod>(p: *mut u8, v: &T) {
    let n = size_of::<T>() / 4;
    let w = unsafe { words(p, n) };
    let src: &[u32] = bytemuck::cast_slice(bytemuck::bytes_of(v));
    for i in 0..n {
        w[i].store(src[i], Ordering::Relaxed);
    }
    fence(Ordering::Release);
}

unsafe fn plain_read<T: Pod>(p: *const u8) -> T {
    let n = size_of::<T>() / 4;
    let w = unsafe { words(p, n) };
    fence(Ordering::Acquire);
    let mut out = T::zeroed();
    let dst: &mut [u32] = bytemuck::cast_slice_mut(bytemuck::bytes_of_mut(&mut out));
    for i in 0..n {
        dst[i] = w[i].load(Ordering::Relaxed);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique(name: &str) -> String {
        format!("Local\\RustVR_test_{name}_{}", unsafe { GetCurrentProcessId() })
    }

    #[test]
    fn seqlock_round_trip() {
        let name = unique("seq");
        let rt = Link::create(&name).unwrap();
        let ad = Link::open(&name).unwrap();
        assert!(ad.read_rt_frame().is_none(), "nothing written yet");
        let mut f: RtFrame = bytemuck::Zeroable::zeroed();
        f.frame_index = 42;
        f.intents.presses[3] = 7;
        f.hmd.pos = [1.0, 1.6, -2.0];
        rt.write_rt_frame(&f);
        let g = ad.read_rt_frame().unwrap();
        assert_eq!(g.frame_index, 42);
        assert_eq!(g.intents.presses[3], 7);
        assert_eq!(g.hmd.pos, [1.0, 1.6, -2.0]);
        assert_eq!(g.seq % 2, 0);
    }

    #[test]
    fn triple_buffer_hands_over_newest() {
        let name = unique("tb");
        let rt = Link::create(&name).unwrap();
        let ad = Link::open(&name).unwrap();
        let epoch = ad.begin_connect();
        assert!(!ad.connect_acked(epoch));
        assert!(rt.poll_connect());
        assert!(ad.connect_acked(epoch));
        let mut front = 2;
        let mut back = 0;
        assert_eq!(rt.take(front), None);
        for frame in 1..=3u64 {
            let mut info: SlotInfo = bytemuck::Zeroable::zeroed();
            info.frame_index = frame;
            ad.write_slot_info(back, &info);
            back = ad.publish(back);
        }
        front = rt.take(front).expect("published");
        assert_eq!(rt.read_slot_info(front).frame_index, 3, "the newest frame wins");
        let mut seen = vec![front, back];
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 2, "reader and writer never share a slot");
        assert_eq!(rt.take(front), None);
    }

    #[test]
    fn event_ring_drops_when_full() {
        let name = unique("ring");
        let rt = Link::create(&name).unwrap();
        let ad = Link::open(&name).unwrap();
        for i in 0..EVENT_RING_ENTRIES as u32 {
            assert!(ad.push_event(&Event { kind: EV_HAPTIC, a: i, f: [0.0; 6] }));
        }
        assert!(!ad.push_event(&Event::default()));
        for i in 0..EVENT_RING_ENTRIES as u32 {
            assert_eq!(rt.pop_event().unwrap().a, i);
        }
        assert!(rt.pop_event().is_none());
    }

    #[test]
    fn collision_ring_wraps_with_padding() {
        let name = unique("col");
        let rt = Link::create(&name).unwrap();
        let ad = Link::open(&name).unwrap();
        // Big messages so the ring wraps several times; each is drained before the next batch.
        let payload: Vec<u8> = (0..(COL_RING_DATA_BYTES / 3) as u32 & !3).map(|i| i as u8).collect();
        let payload = &payload[..payload.len() / 4 * 4];
        for round in 0..5u32 {
            assert!(ad.push_collision(COL_MESH, payload), "round {round}");
            assert!(ad.push_collision(COL_CLEAR, &[]));
            let mut got = Vec::new();
            rt.drain_collision(|kind, bytes| got.push((kind, bytes.len(), bytes.first().copied())));
            assert_eq!(got, vec![(COL_MESH, payload.len(), Some(0)), (COL_CLEAR, 0, None)]);
        }
        // Without draining, the producer refuses once the ring is full.
        assert!(ad.push_collision(COL_MESH, payload));
        assert!(ad.push_collision(COL_MESH, payload));
        assert!(!ad.push_collision(COL_MESH, payload));
    }

    #[test]
    fn object_table_round_trip() {
        let name = unique("obj");
        let rt = Link::create(&name).unwrap();
        let ad = Link::open(&name).unwrap();
        let mut table: Box<ObjectTable> = bytemuck::allocation::zeroed_box();
        table.count = 2;
        table.descs[1].id = 77;
        table.descs[1].mass = 3.5;
        ad.write_objects(&table);
        let mut back: Box<ObjectTable> = bytemuck::allocation::zeroed_box();
        assert!(rt.read_objects_into(&mut back));
        assert_eq!((back.count, back.descs[1].id, back.descs[1].mass), (2, 77, 3.5));
        let seq = rt.objects_seq();
        ad.write_objects(&table);
        assert_ne!(rt.objects_seq(), seq, "rewrites are visible");
    }

    #[test]
    fn hand_pose_table_round_trip() {
        let name = unique("poses");
        let rt = Link::create(&name).unwrap();
        let ad = Link::open(&name).unwrap();
        let mut back: Box<HandPoseTable> = bytemuck::allocation::zeroed_box();
        assert!(!rt.read_hand_poses_into(&mut back), "nothing written yet");
        let mut table: Box<HandPoseTable> = bytemuck::allocation::zeroed_box();
        table.count = 2;
        table.poses[1].id = 9;
        table.poses[1].side = HAND_RIGHT as u32;
        table.poses[1].fingers[14] = [0.1, 0.2, 0.3, 0.9];
        table.poses[1].index_pulled[2] = [0.0, 0.0, 0.5, 0.5];
        ad.write_hand_poses(&table);
        assert!(rt.read_hand_poses_into(&mut back));
        let p = back.find(9).unwrap();
        assert_eq!((p.side, p.fingers[14], p.index_pulled[2]), (1, [0.1, 0.2, 0.3, 0.9], [0.0, 0.0, 0.5, 0.5]));
        assert!(back.find(0).is_none() && back.find(7).is_none());
        let seq = rt.hand_poses_seq();
        ad.write_hand_poses(&table);
        assert_ne!(rt.hand_poses_seq(), seq, "rewrites are visible");
    }

    #[test]
    fn kick_table_round_trip() {
        let name = unique("kicks");
        let rt = Link::create(&name).unwrap();
        let ad = Link::open(&name).unwrap();
        let mut back: Box<KickTable> = bytemuck::allocation::zeroed_box();
        assert!(!rt.read_kicks_into(&mut back), "nothing written yet");
        let mut table: Box<KickTable> = bytemuck::allocation::zeroed_box();
        table.count = 1;
        let k = &mut table.kicks[0];
        k.object = 5;
        k.when = KICK_HIP | KICK_STANDING | KICK_FIRST;
        k.channels[KICK_PITCH].count = 2;
        k.channels[KICK_PITCH].keys[0] = [0.0, 0.02, 0.0, -0.1];
        k.channels[KICK_PITCH].keys[1] = [0.2, 0.0, -0.1, 0.0];
        ad.write_kicks(&table);
        assert!(rt.read_kicks_into(&mut back));
        let k = &back.entries()[0];
        assert_eq!((k.object, k.when), (5, KICK_HIP | KICK_STANDING | KICK_FIRST));
        assert!((k.channels[KICK_PITCH].value(0.1) - 0.01).abs() < 1e-4, "{}", k.channels[KICK_PITCH].value(0.1));
        assert_eq!(k.channels[KICK_YAW].value(0.1), 0.0);
        let seq = rt.kicks_seq();
        ad.write_kicks(&table);
        assert_ne!(rt.kicks_seq(), seq, "rewrites are visible");
    }

    #[test]
    fn sound_pack_round_trip() {
        let name = unique("sounds");
        let rt = Link::create(&name).unwrap();
        let ad = Link::open(&name).unwrap();
        let mut back: Box<SoundPack> = bytemuck::allocation::zeroed_box();
        assert!(!rt.read_sound_pack_into(&mut back), "nothing written yet: the runtime's own sounds");
        let mut pack = SoundPack::new(r"C:\packs\game");
        pack.flags = SND_GAME_SHOTS;
        assert!(pack.add_object(7, "wpn_ak", SND_GAME_HANDLING));
        ad.write_sound_pack(&pack);
        assert!(rt.read_sound_pack_into(&mut back));
        assert_eq!((back.path(), back.flags), (r"C:\packs\game", SND_GAME_SHOTS));
        assert_eq!(back.object(7).map(|o| (o.set_name(), o.flags)), Some(("wpn_ak", SND_GAME_HANDLING)));
        let seq = rt.sound_pack_seq();
        ad.write_sound_pack(&pack);
        assert_ne!(rt.sound_pack_seq(), seq, "rewrites are visible");
        // Another game connecting starts without it.
        let epoch = ad.begin_connect();
        assert!(rt.poll_connect() && ad.connect_acked(epoch));
        assert!(rt.read_sound_pack_into(&mut back));
        assert!(back.path().is_empty() && back.count == 0 && back.flags == 0);
    }

    #[test]
    fn open_fails_without_runtime() {
        assert!(Link::open(&unique("missing")).is_err());
    }

    #[test]
    fn events_wake_the_other_side() {
        let name = unique("ev");
        let rt = Link::create(&name).unwrap();
        let ad = Link::open(&name).unwrap();
        assert!(!ad.wait_frame(0));
        rt.signal_frame();
        assert!(ad.wait_frame(100));
        ad.signal_submit();
        assert!(rt.wait_submit(100));
    }
}
