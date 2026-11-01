//! Objects' drawn shapes ([`rustvr_protocol::COL_OBJECT_MESH`]): a game's mesh of what's drawn of
//! a thing, made the thing's collision as its convex parts (a basket's body and handle, a bowl's
//! wall round its hollow, a sword's blade, guard and grip), whatever shape its description gives
//! it. Working the parts out (an approximate convex decomposition,
//! [`rustvr_guns::hulls::convex_parts_covering`]) takes a while, so it's done on a thread of its
//! own, once for each mesh (one plate's mesh does for every plate); until then an object keeps its
//! own shape.

use glam::Vec3;
use rustvr_protocol::{ColMesh, ColTriangle};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};

/// At most this many convex parts to a thing (and a few more for what they'd leave out), each of at
/// most this many points; worked out on a voxel grid this fine across a round thing (finer across a
/// long one: a sword's grip), each part at most this far from convex (a share of its size). Coarser
/// than a gun's: about a tenth of a second a thing, and a bowl, a bucket or a basket keeps its
/// hollow (finer, its parts run out before its wall does, and fill it).
pub const MAX_PARTS: usize = 16;
pub const MAX_POINTS: usize = 48;
pub const RESOLUTION: u32 = 40;
pub const CONCAVITY: f32 = 0.02;

/// A thing's convex parts, object space (metres).
pub type Parts = Arc<Vec<Vec<Vec3>>>;

/// What a [`rustvr_protocol::COL_OBJECT_MESH`] changed.
pub enum Change {
    /// The object's drawn shape is ready (worked out before, for this mesh).
    Ready(u32, Parts),
    /// Being worked out: ready later ([`Drawn::poll`]).
    Asked(u32),
    /// The object's own shape again.
    Cleared(u32),
}

#[derive(Default)]
pub struct Drawn {
    /// Each object's mesh (its key: the triangles' hash).
    meshes: HashMap<u32, u64>,
    /// Each mesh's parts once worked out, and the ones being worked out.
    done: HashMap<u64, Parts>,
    asked: HashSet<u64>,
    worker: Option<(Sender<(u64, Vec<[Vec3; 3]>)>, Receiver<(u64, Vec<Vec<Vec3>>)>)>,
}

impl Drawn {
    /// A [`rustvr_protocol::COL_OBJECT_MESH`] message's payload.
    pub fn set(&mut self, payload: &[u8]) -> Option<Change> {
        let head: ColMesh = bytemuck::pod_read_unaligned(payload.get(..size_of::<ColMesh>())?);
        let bytes = &payload[size_of::<ColMesh>()..];
        let count = (head.count as usize).min(bytes.len() / size_of::<ColTriangle>());
        if count == 0 {
            self.meshes.remove(&head.id);
            return Some(Change::Cleared(head.id));
        }
        let bytes = &bytes[..count * size_of::<ColTriangle>()];
        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            bytes.hash(&mut h);
            h.finish()
        };
        self.meshes.insert(head.id, key);
        if let Some(parts) = self.done.get(&key) {
            return Some(Change::Ready(head.id, parts.clone()));
        }
        if self.asked.insert(key) {
            let tris: Vec<[Vec3; 3]> = (0..count)
                .map(|i| {
                    let t: ColTriangle = bytemuck::pod_read_unaligned(&bytes[i * size_of::<ColTriangle>()..(i + 1) * size_of::<ColTriangle>()]);
                    [Vec3::new(t.v[0], t.v[1], t.v[2]), Vec3::new(t.v[3], t.v[4], t.v[5]), Vec3::new(t.v[6], t.v[7], t.v[8])]
                })
                .filter(|t| t.iter().all(|v| v.is_finite()))
                .collect();
            let (to, _) = self.worker.get_or_insert_with(spawn);
            let _ = to.send((key, tris));
        }
        Some(Change::Asked(head.id))
    }

    /// Object `id`'s drawn shape, if it has one worked out.
    pub fn parts(&self, id: u32) -> Option<Parts> {
        self.done.get(self.meshes.get(&id)?).cloned()
    }

    /// The parts worked out since last asked: each object they're for, and its parts.
    pub fn poll(&mut self) -> Vec<(u32, Parts)> {
        let Some((_, from)) = &self.worker else { return Vec::new() };
        let mut out = Vec::new();
        while let Ok((key, parts)) = from.try_recv() {
            self.asked.remove(&key);
            let parts = Arc::new(parts);
            self.done.insert(key, parts.clone());
            out.extend(self.meshes.iter().filter(|(_, k)| **k == key).map(|(id, _)| (*id, parts.clone())));
        }
        out
    }
}

/// The thread that works the parts out.
fn spawn() -> (Sender<(u64, Vec<[Vec3; 3]>)>, Receiver<(u64, Vec<Vec<Vec3>>)>) {
    let (to, jobs) = std::sync::mpsc::channel::<(u64, Vec<[Vec3; 3]>)>();
    let (done, from) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new().name("drawn shapes".into()).spawn(move || {
        while let Ok((key, tris)) = jobs.recv() {
            let started = std::time::Instant::now();
            let parts = parts_of(&tris);
            tracing::debug!("rig: a drawn shape of {} triangles as {} convex parts in {:.0} ms", tris.len(), parts.len(), started.elapsed().as_secs_f64() * 1e3);
            if done.send((key, parts)).is_err() {
                break;
            }
        }
    });
    (to, from)
}

/// The convex parts of a mesh: a few triangles (or a decomposition that finds nothing) are one hull.
fn parts_of(tris: &[[Vec3; 3]]) -> Vec<Vec<Vec3>> {
    let whole = || vec![tris.iter().flatten().copied().collect::<Vec<Vec3>>()];
    if tris.len() < 12 {
        return whole();
    }
    let parts = std::panic::catch_unwind(|| rustvr_guns::hulls::convex_parts_covering(tris, MAX_PARTS, MAX_POINTS, RESOLUTION, CONCAVITY)).unwrap_or_default();
    if parts.is_empty() { whole() } else { parts }
}

