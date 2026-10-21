//! Sound packs: a game's own sounds, played instead of the runtime's synthesised ones.
//!
//! A pack is a folder with a `sounds.toml` and the sample files it names (Ogg Vorbis or WAV,
//! paths relative to the folder). The adapter tells the runtime where it is (the protocol's
//! `SoundPack` region); the runtime loads it on a background thread and from then on plays the
//! pack's sound for every [`Event`] it has, and its own synthesised [`crate::Sound`] for the rest.
//!
//! ```toml
//! format = 1
//! name = "Metro 2033 Redux"
//! gain = 1.0                      # the whole pack
//!
//! [defaults]                      # every object, and every set that doesn't say otherwise
//! grab = ["actor/mvt_short_1.ogg", "actor/mvt_short_2.ogg"]   # one layer, a variation each
//! ui_click = { files = ["ui/click.ogg"], gain = 0.5 }
//!
//! [sets.rifle]                    # a sound set: what one kind of object sounds like
//! mag_out = "reload/mag_out.ogg"
//!
//! [sets.wpn_ak]
//! extends = "rifle"               # events it doesn't define come from there (then [defaults])
//! fire = [                        # layers play together, each picking one of its files
//!   { files = ["ak/close_1.ogg", "ak/close_2.ogg"] },
//!   { files = [{ file = "ak/body_1.ogg", gain = 0.8 }, "ak/body_2.ogg"] },
//! ]
//! fire_tail = { files = ["echo/rifle.ogg"], spatial = false }   # the room: fills the head
//! casing = { files = ["casings/1.ogg", "casings/2.ogg"], delay = 0.5 }
//! ```
//!
//! An event is a file, a list of files (one layer), a layer table, or a list of layer tables.
//! A layer is `files` (each a path, or `{ file, gain }`), `gain` (1), `spatial` (true: placed at
//! the source; false: the unspatialised stereo bed, for tails and room sound) and `delay`
//! (seconds before it starts, e.g. a casing landing). Every time an event plays, each of its
//! layers picks one of its files at random, never the one it played last. Event names are
//! [`Event::name`]; sets are named by the adapter per object (the protocol's `ObjectSound`).
//!
//! Levels: a file plays at its own level times its gain, its layer's, and the pack's. Shots are
//! loud and a pack usually ships them mastered loud: the mixer's limiter keeps the bus under its
//! ceiling either way.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;

use crate::decode;
use crate::sounds::Variation;
use crate::synth::Rng;
use crate::{Event, SetKey};

/// The file in a pack's folder that describes it.
pub const PACK_FILE: &str = "sounds.toml";
/// The `format` this crate reads.
pub const FORMAT: u32 = 1;
/// The longest a pack file may play: anything longer is music or ambience, not an interaction.
const LONGEST: f32 = 20.0;

fn one() -> f32 {
    1.0
}

fn yes() -> bool {
    true
}

/// `sounds.toml`, as written.
#[derive(Clone, Debug, Deserialize)]
pub struct PackSpec {
    pub format: u32,
    #[serde(default)]
    pub name: String,
    /// Where the sounds came from (free text).
    #[serde(default)]
    pub source: String,
    #[serde(default = "one")]
    pub gain: f32,
    #[serde(default)]
    pub defaults: BTreeMap<String, EventSpec>,
    #[serde(default)]
    pub sets: BTreeMap<String, SetSpec>,
}

/// A sound set: events, and the set the rest come from.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SetSpec {
    /// Another set this one builds on (its events, then that set's parents', then `[defaults]`).
    #[serde(default)]
    pub extends: Option<String>,
    /// Free text: what the set is (shown in logs).
    #[serde(default)]
    pub about: String,
    #[serde(flatten)]
    pub events: BTreeMap<String, EventSpec>,
}

/// One event: a file, one layer of files, a layer, or layers (see the module docs).
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum EventSpec {
    File(String),
    Files(Vec<FileSpec>),
    Layer(LayerSpec),
    Layers(Vec<LayerSpec>),
}

impl EventSpec {
    pub fn layers(&self) -> Vec<LayerSpec> {
        let layer = |files: Vec<FileSpec>| LayerSpec { files, gain: 1.0, spatial: true, delay: 0.0 };
        match self {
            EventSpec::File(f) => vec![layer(vec![FileSpec::Path(f.clone())])],
            EventSpec::Files(f) => vec![layer(f.clone())],
            EventSpec::Layer(l) => vec![l.clone()],
            EventSpec::Layers(l) => l.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayerSpec {
    pub files: Vec<FileSpec>,
    #[serde(default = "one")]
    pub gain: f32,
    /// Placed at the source (true) or in the unspatialised stereo bed (false).
    #[serde(default = "yes")]
    pub spatial: bool,
    /// Seconds before the layer starts.
    #[serde(default)]
    pub delay: f32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum FileSpec {
    Path(String),
    Entry(FileEntry),
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileEntry {
    pub file: String,
    #[serde(default = "one")]
    pub gain: f32,
}

impl FileSpec {
    pub fn path(&self) -> &str {
        match self {
            FileSpec::Path(p) => p,
            FileSpec::Entry(e) => &e.file,
        }
    }

    pub fn gain(&self) -> f32 {
        match self {
            FileSpec::Path(_) => 1.0,
            FileSpec::Entry(e) => e.gain,
        }
    }
}

impl PackSpec {
    pub fn parse(text: &str) -> Result<PackSpec> {
        let spec: PackSpec = toml::from_str(text).context("reading the sound pack")?;
        ensure!(spec.format == FORMAT, "sound pack format {} (this runtime reads {FORMAT})", spec.format);
        Ok(spec)
    }

    pub fn read(dir: &Path) -> Result<PackSpec> {
        let path = dir.join(PACK_FILE);
        let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        PackSpec::parse(&text).with_context(|| path.display().to_string())
    }

    /// Every event of a set as it resolves: its own, then its parents', then `[defaults]`. The
    /// value is the name of the set that defines it (`None`: `[defaults]`) and the event.
    pub fn resolve(&self, set: Option<&str>) -> Result<BTreeMap<String, (Option<String>, EventSpec)>> {
        let mut chain = Vec::new();
        let mut at = set.map(str::to_owned);
        while let Some(name) = at {
            ensure!(!chain.contains(&name), "sound set {name} extends itself");
            let s = self.sets.get(&name).with_context(|| format!("no sound set {name}"))?;
            at = s.extends.clone();
            chain.push(name);
        }
        let mut out: BTreeMap<String, (Option<String>, EventSpec)> =
            self.defaults.iter().map(|(e, spec)| (e.clone(), (None, spec.clone()))).collect();
        for name in chain.iter().rev() {
            for (e, spec) in &self.sets[name].events {
                out.insert(e.clone(), (Some(name.clone()), spec.clone()));
            }
        }
        Ok(out)
    }
}

/// One layer of a loaded event.
pub(crate) struct Layer {
    /// Indices into [`Pack::clips`].
    pub clips: Vec<usize>,
    pub gain: f32,
    /// Frames before it starts.
    pub delay: usize,
    /// The clip it played last (`usize::MAX`: none yet), so it never plays the same one twice in
    /// a row. Atomic so the mixer can update it in a shared pack without locking.
    last: AtomicUsize,
}

impl Layer {
    /// A random clip, never the one this layer played last.
    pub fn pick(&self, rng: &mut Rng) -> usize {
        let n = self.clips.len();
        if n <= 1 {
            return 0;
        }
        let last = self.last.load(Ordering::Relaxed);
        let k = rng.below(n - 1);
        let pick = if last < n && k >= last { k + 1 } else { k };
        self.last.store(pick, Ordering::Relaxed);
        pick
    }
}

pub(crate) struct PackEvent {
    pub layers: Vec<Layer>,
}

/// A pack decoded for one sample rate, ready to mix. Built on the loader thread; the mixer only
/// reads it (and the layers' `last`).
pub(crate) struct Pack {
    /// Unique per load, so a voice knows which pack its clip belongs to.
    pub generation: u64,
    pub rate: u32,
    pub gain: f32,
    /// Mono clips for spatial layers (`main`), stereo ones for the bed (`bed`).
    pub clips: Vec<Variation>,
    pub events: Vec<PackEvent>,
    /// Per set, the event index for each [`Event`].
    sets: HashMap<SetKey, Box<[Option<usize>]>>,
    defaults: Box<[Option<usize>]>,
}

impl Pack {
    /// The event a set plays for `event`: the set's own (or inherited), else `[defaults]`'s.
    pub fn find(&self, set: SetKey, event: Event) -> Option<&PackEvent> {
        let table = self.sets.get(&set).unwrap_or(&self.defaults);
        table[event as usize].map(|i| &self.events[i])
    }
}

/// What a loaded pack holds (for logs, the menu and tests).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PackInfo {
    pub dir: PathBuf,
    pub name: String,
    /// The sample rate it was decoded for.
    pub rate: u32,
    pub sets: usize,
    pub events: usize,
    pub clips: usize,
    /// Seconds of audio decoded.
    pub seconds: f32,
    /// Files it names that are missing or unreadable (skipped).
    pub missing: Vec<String>,
    /// Event names it uses that this runtime doesn't know (ignored).
    pub unknown: Vec<String>,
    pub millis: u64,
}

/// Reads, decodes and resamples a whole pack for `rate`.
pub(crate) fn load(dir: &Path, rate: u32, generation: u64) -> Result<(Pack, PackInfo)> {
    let started = std::time::Instant::now();
    let spec = PackSpec::read(dir)?;
    ensure!(spec.gain.is_finite() && spec.gain >= 0.0, "pack gain {}", spec.gain);
    let mut info = PackInfo { dir: dir.to_path_buf(), name: spec.name.clone(), rate, ..Default::default() };
    let mut clips = Vec::new();
    // (file, spatial) -> clip, so a file several sets share is decoded once.
    let mut decoded: HashMap<(String, bool), Option<usize>> = HashMap::new();
    let mut events = Vec::new();
    // (defining set, event) -> event index, so inherited events are shared.
    let mut built: HashMap<(Option<String>, Event), Option<usize>> = HashMap::new();

    let mut table = |set: Option<&str>, clips: &mut Vec<Variation>, info: &mut PackInfo| -> Result<Box<[Option<usize>]>> {
        let mut out = vec![None; Event::COUNT].into_boxed_slice();
        for (name, (definer, spec)) in spec.resolve(set)? {
            let Some(event) = Event::from_name(&name) else {
                if !info.unknown.contains(&name) {
                    info.unknown.push(name);
                }
                continue;
            };
            let key = (definer, event);
            let index = match built.get(&key) {
                Some(i) => *i,
                None => {
                    let layers: Vec<Layer> = spec
                        .layers()
                        .iter()
                        .filter_map(|l| {
                            let ids: Vec<usize> = l
                                .files
                                .iter()
                                .filter_map(|f| {
                                    *decoded.entry((f.path().to_owned(), l.spatial)).or_insert_with(|| {
                                        match clip(dir, f, l.spatial, rate) {
                                            Ok(v) => {
                                                info.seconds += v.len as f32 / rate as f32;
                                                clips.push(v);
                                                Some(clips.len() - 1)
                                            }
                                            Err(e) => {
                                                tracing::warn!("sound pack {}: {}: {e:#}", dir.display(), f.path());
                                                info.missing.push(f.path().to_owned());
                                                None
                                            }
                                        }
                                    })
                                })
                                .collect();
                            let delay = (l.delay.clamp(0.0, 5.0) * rate as f32) as usize;
                            let gain = if l.gain.is_finite() { l.gain.max(0.0) } else { 1.0 };
                            (!ids.is_empty()).then(|| Layer { clips: ids, gain, delay, last: AtomicUsize::new(usize::MAX) })
                        })
                        .collect();
                    let index = (!layers.is_empty()).then(|| {
                        events.push(PackEvent { layers });
                        events.len() - 1
                    });
                    built.insert(key, index);
                    index
                }
            };
            out[event as usize] = index;
        }
        Ok(out)
    };

    let defaults = table(None, &mut clips, &mut info)?;
    let mut sets = HashMap::new();
    for name in spec.sets.keys() {
        let t = table(Some(name), &mut clips, &mut info)?;
        if sets.insert(SetKey::of(name), t).is_some() {
            bail!("two sound sets named {name} (names ignore case)");
        }
    }
    info.sets = sets.len();
    info.events = events.len();
    info.clips = clips.len();
    info.millis = started.elapsed().as_millis() as u64;
    Ok((Pack { generation, rate, gain: spec.gain, clips, events, sets, defaults }, info))
}

/// One file, decoded and resampled: mono for a spatial layer, stereo for the bed.
fn clip(dir: &Path, f: &FileSpec, spatial: bool, rate: u32) -> Result<Variation> {
    let rel = f.path();
    ensure!(!rel.is_empty() && !Path::new(rel).is_absolute() && !rel.split(['/', '\\']).any(|p| p == ".."), "not a path inside the pack");
    let bytes = std::fs::read(dir.join(rel)).context("reading")?;
    let pcm = decode::decode(&bytes)?;
    ensure!(pcm.seconds() <= LONGEST, "{:.1} s long (the most is {LONGEST} s)", pcm.seconds());
    let gain = if f.gain().is_finite() { f.gain().max(0.0) } else { 1.0 };
    if spatial {
        let mut main = decode::resample(&pcm.mono(), pcm.rate, rate);
        main.iter_mut().for_each(|s| *s = finite(*s * gain));
        Ok(Variation::from_layers(main, Vec::new(), Vec::new(), false))
    } else {
        let stereo = pcm.stereo();
        let left: Vec<f32> = stereo.iter().step_by(2).copied().collect();
        let right: Vec<f32> = stereo.iter().skip(1).step_by(2).copied().collect();
        let (l, r) = (decode::resample(&left, pcm.rate, rate), decode::resample(&right, pcm.rate, rate));
        let bed = l.iter().zip(&r).flat_map(|(a, b)| [finite(a * gain), finite(b * gain)]).collect();
        Ok(Variation::from_layers(Vec::new(), Vec::new(), bed, false))
    }
}

fn finite(x: f32) -> f32 {
    if x.is_finite() { x.clamp(-4.0, 4.0) } else { 0.0 }
}
