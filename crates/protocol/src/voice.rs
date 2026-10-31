//! Talking to the game's characters out loud: the voice plugin (`rustvr-voice`, the user's m8,
//! started by the runtime when the player turns voice on) hears the player through the headset's
//! mic, and the character they talk to answers in a voice cloned from the game's own, through the
//! game: its lip sync, its sound.
//!
//! Four regions, after the bows. The plugin is a process of its own: it opens the mapping by
//! name and touches only these.
//!
//! * [`VoiceScene`] (adapter -> plugin, seqlock): who's around, who the player is talking to (the
//!   listener) and who they are, in words from the game's data; where the plugin writes each line
//!   and where each voice's clips are.
//! * [`VoiceEvents`] (adapter -> plugin, ring): the game saved or loaded (each character's memory
//!   goes with the saves), or something happened that the listener saw.
//! * [`VoiceLines`] (plugin -> adapter, ring): the player started talking ([`VL_LISTEN`]: the one
//!   addressed turns to listen), what was heard ([`VL_HEARD`]), a sentence to say ([`VL_SAY`]), the
//!   end of a line ([`VL_END`]), stop talking ([`VL_STOP`]: the player talked over them), a
//!   line streamed as it's made ([`VL_STREAM`]) and its sentences ([`VL_SUBTITLE`]).
//! * [`VoiceStream`] (plugin -> adapter, rings): a streamed line's voice and its mouth's shapes as
//!   they're made, which the adapter takes every frame (the plugin writes no further ahead of what
//!   it has taken than the rings hold); and how much of it the game has played.
//!
//! # A sentence's files
//!
//! For [`VL_SAY`] line `n` the plugin has written, in [`VoiceScene::lines_dir`]:
//!
//! * `<n>.wav`: the voice, mono 16-bit PCM.
//! * `<n>.lips`: the mouth's shapes, FaceGen's 16 phonemes over time (the games built on FaceGen
//!   morph their speaking heads with these: Aah, BigAah, BMP, ChJSh, DST, Eee, Eh, FV, I, K, N, Oh,
//!   OohQ, R, Th, W): `u32` frames a second, `u32` frames, then each frame's 16 `f32` weights (0 to
//!   1), little-endian. Frame 0 is the voice's start.
//!
//! The adapter makes of them what its game needs (Oblivion: its own `.lip` file beside the WAV).
//!
//! # A streamed line
//!
//! [`VL_STREAM`] starts one: its voice and mouth come in [`VoiceStream`]'s rings, from the line's
//! start (sample 0, frame 0: frame `f` is samples `f * rate / VOICE_STREAM_FPS` on), the mouth's
//! frames a little behind the voice, and the voice no further than its frames (they're made from
//! it). Each sentence's words come as a [`VL_SUBTITLE`] at the sample it starts on ([`VoiceLine::at`]),
//! before its voice; [`VL_END`] says the line's whole length ([`VoiceLine::at`]). The game plays the
//! voice as it comes; with nothing more yet it waits (silence), and goes on with the rest when it
//! comes.
//!
//! # A voice's clips
//!
//! A speaker's [`VoiceSpeaker::voice`] names a folder under [`VoiceScene::voices_dir`] holding
//! the game's own lines in that voice: `<k>.mp3` or `<k>.wav` (k from 1) and `<k>.txt`, what's said
//! in each. The plugin joins them into the clip it clones (`reference.wav` and `reference.txt`,
//! beside them). The adapter writes a folder once; one with a `reference.wav` is done.

use bytemuck::{Pod, Zeroable};

/// After the bows ([`crate::archery`]).
pub const OFF_VOICE_SCENE: usize = crate::OFF_BOWS_END.next_multiple_of(64);
pub const OFF_VOICE_EVENTS: usize = (OFF_VOICE_SCENE + size_of::<VoiceScene>()).next_multiple_of(64);
pub const OFF_VOICE_LINES: usize = (OFF_VOICE_EVENTS + size_of::<VoiceEvents>()).next_multiple_of(64);
pub const OFF_VOICE_STREAM: usize = (OFF_VOICE_LINES + size_of::<VoiceLines>()).next_multiple_of(64);
pub const OFF_VOICE_END: usize = OFF_VOICE_STREAM + size_of::<VoiceStream>();

pub const MAX_VOICE_SPEAKERS: usize = 8;
pub const VOICE_NAME_BYTES: usize = 64;
pub const VOICE_KEY_BYTES: usize = 88;
pub const VOICE_PATH_BYTES: usize = 256;
pub const VOICE_PERSONA_BYTES: usize = 8192;
pub const VOICE_SITUATION_BYTES: usize = 1536;
pub const VOICE_EVENT_SLOTS: usize = 16;
pub const VOICE_EVENT_TEXT_BYTES: usize = 240;
pub const VOICE_LINE_SLOTS: usize = 16;
pub const VOICE_HOW_BYTES: usize = 64;
pub const VOICE_TEXT_BYTES: usize = 416;
/// A streamed line's rings: its voice (1.2 s at 24 kHz, mono 16-bit), its mouth's frames.
pub const VOICE_STREAM_SAMPLES: usize = 28_800;
pub const VOICE_STREAM_FRAMES: usize = 64;
/// The mouth's frames a second.
pub const VOICE_STREAM_FPS: u32 = 30;

/// [`VoiceScene::flags`]: the adapter can play lines (its game is in a state to).
pub const VS_READY: u32 = 1 << 0;
/// The game isn't listening now (a menu is open, the game is paused or loading): the plugin hears
/// nothing, and no one is cut off.
pub const VS_DEAF: u32 = 1 << 1;

/// Adapter -> plugin, seqlocked. Write it whenever it changes (the listener, their persona, the
/// situation), and again after every reconnect.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct VoiceScene {
    /// Seqlock word.
    pub seq: u32,
    pub flags: u32, // VS_*
    /// The [`VoiceSpeaker::id`] of the one the player is talking to; 0: no one.
    pub listener: u32,
    /// Entries in use are `speakers[..count]`.
    pub count: u32,
    pub _pad: [u32; 12],
    /// The game's name ("The Elder Scrolls IV: Oblivion") and the player's, UTF-8, NUL-terminated.
    pub game: [u8; VOICE_NAME_BYTES],
    pub player: [u8; VOICE_NAME_BYTES],
    /// Where the plugin writes each sentence's files (absolute; see the module's notes).
    pub lines_dir: [u8; VOICE_PATH_BYTES],
    /// Where each voice's clips are (absolute; see the module's notes).
    pub voices_dir: [u8; VOICE_PATH_BYTES],
    /// Who's around to be talked to, the listener among them.
    pub speakers: [VoiceSpeaker; MAX_VOICE_SPEAKERS],
    /// Who the listener is, in words from the game's data (their name, kind, work, the people
    /// they belong to, how they feel about the player, what they'd talk about): the body of the
    /// LLM's system prompt.
    pub persona: [u8; VOICE_PERSONA_BYTES],
    /// What holds for the listener right now, in words (where they are, what they're doing, the
    /// time, who's about).
    pub situation: [u8; VOICE_SITUATION_BYTES],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct VoiceSpeaker {
    /// The game's id for them (stable across saves: their memory is kept by it); never 0.
    pub id: u32,
    pub _pad: u32,
    /// What they're called, UTF-8, NUL-terminated.
    pub name: [u8; VOICE_NAME_BYTES],
    /// Their voice: its folder under [`VoiceScene::voices_dir`] (`imperial\m`, say), UTF-8,
    /// NUL-terminated.
    pub voice: [u8; VOICE_KEY_BYTES],
}

/// [`VoiceEvent::kind`]: the game saved; `text` is the save's name.
pub const VE_SAVED: u32 = 1;
/// The game loaded a save; `text` is its name. Every character remembers what they did then.
pub const VE_LOADED: u32 = 2;
/// Something happened that `speaker` saw or heard, in words; with [`VEF_REACT`] it may make them
/// speak (only with the plugin's decision model on), else it only goes in their memory.
pub const VE_HAPPENED: u32 = 3;
pub const VEF_REACT: u32 = 1 << 0;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct VoiceEvent {
    pub kind: u32, // VE_*
    /// The [`VoiceSpeaker::id`] it's about (0: the listener).
    pub speaker: u32,
    pub flags: u32, // VEF_*
    pub _pad: u32,
    /// UTF-8, NUL-terminated.
    pub text: [u8; VOICE_EVENT_TEXT_BYTES],
}

/// Adapter -> plugin: a ring the adapter writes (`written` counts every event); the plugin keeps
/// how many it has read (and skips ahead if it fell more than a ring behind).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct VoiceEvents {
    pub written: u64,
    pub _pad: [u32; 14],
    pub events: [VoiceEvent; VOICE_EVENT_SLOTS],
}

/// [`VoiceLine::kind`]: the player started talking. The one they talk to now (whom the adapter
/// picks, and says in [`VoiceScene::listener`] by the time the words are known) turns to listen.
pub const VL_LISTEN: u32 = 1;
/// What the player said to `speaker`, as it was heard (`text`).
pub const VL_HEARD: u32 = 2;
/// `speaker` says a sentence: `text`, said as `how` (the line's own words for it, "angrily",
/// "with a laugh"; empty: plainly), its files `<line>.wav` and `<line>.lips`, `duration_ms` long.
pub const VL_SAY: u32 = 3;
/// `speaker`'s whole line has been said (their last [`VL_SAY`] is its end).
pub const VL_END: u32 = 4;
/// Stop talking at once: the player talked over them, or a save was loaded.
pub const VL_STOP: u32 = 5;
/// `speaker` says a line streamed as it's made ([`VoiceStream`]): `line` its number (no file of
/// its own), said as `how`, its voice's sample rate `at`.
pub const VL_STREAM: u32 = 6;
/// A sentence of the streamed line: its words (`text`), said as `how`, from sample `at`.
pub const VL_SUBTITLE: u32 = 7;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct VoiceLine {
    pub kind: u32, // VL_*
    pub speaker: u32,
    /// The sentence's number (its files' names), counting up; no two runs of the plugin share one.
    pub line: u32,
    pub duration_ms: u32,
    /// A streamed line's: its sample rate ([`VL_STREAM`]), where a sentence starts (a sample,
    /// [`VL_SUBTITLE`]), its length (samples, [`VL_END`]).
    pub at: u32,
    pub _pad: [u32; 3],
    pub how: [u8; VOICE_HOW_BYTES],
    pub text: [u8; VOICE_TEXT_BYTES],
}

/// Plugin -> adapter: a ring the plugin writes, as [`VoiceEvents`]. The plugin's heartbeat says
/// it's running (the adapter does no voice work without it).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct VoiceLines {
    pub written: u64,
    /// `GetTickCount64()` at the plugin's latest beat (every 100 ms or so); 0: never started.
    pub heartbeat_ms: u64,
    pub pid: u32,
    pub _pad: [u32; 11],
    pub lines: [VoiceLine; VOICE_LINE_SLOTS],
}

/// Plugin <-> adapter: the streamed line's rings. The plugin writes the voice and the frames
/// (`line`, then the counts written: the rings hold the latest); the adapter, on its own cache
/// line, how many of `line`'s it has taken and how many samples the game has played.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct VoiceStream {
    pub line: u32,
    pub rate: u32,
    pub samples_written: u64,
    pub frames_written: u64,
    pub _pad: [u32; 10],
    pub taken_line: u32,
    pub _pad_taken: u32,
    pub samples_taken: u64,
    pub frames_taken: u64,
    pub played: u64,
    pub _pad_played: [u32; 8],
    pub samples: [i16; VOICE_STREAM_SAMPLES],
    pub frames: [[f32; 16]; VOICE_STREAM_FRAMES],
}

impl VoiceScene {
    pub fn game(&self) -> &str {
        crate::name_str(&self.game)
    }

    pub fn player(&self) -> &str {
        crate::name_str(&self.player)
    }

    pub fn lines_dir(&self) -> &str {
        crate::name_str(&self.lines_dir)
    }

    pub fn voices_dir(&self) -> &str {
        crate::name_str(&self.voices_dir)
    }

    pub fn persona(&self) -> &str {
        crate::name_str(&self.persona)
    }

    pub fn situation(&self) -> &str {
        crate::name_str(&self.situation)
    }

    pub fn entries(&self) -> &[VoiceSpeaker] {
        &self.speakers[..(self.count as usize).min(MAX_VOICE_SPEAKERS)]
    }

    pub fn speaker(&self, id: u32) -> Option<&VoiceSpeaker> {
        self.entries().iter().find(|s| id != 0 && s.id == id)
    }

    /// Adds a speaker; false when the table is full or `id` is 0.
    pub fn add(&mut self, id: u32, name: &str, voice: &str) -> bool {
        let n = self.count as usize;
        if id == 0 || n >= MAX_VOICE_SPEAKERS {
            return false;
        }
        let s = &mut self.speakers[n];
        s.id = id;
        crate::set_name(&mut s.name, name);
        crate::set_name(&mut s.voice, voice);
        self.count += 1;
        true
    }
}

impl VoiceSpeaker {
    pub fn name(&self) -> &str {
        crate::name_str(&self.name)
    }

    pub fn voice(&self) -> &str {
        crate::name_str(&self.voice)
    }
}

impl VoiceEvent {
    pub fn new(kind: u32, speaker: u32, flags: u32, text: &str) -> VoiceEvent {
        let mut e = VoiceEvent::zeroed();
        e.kind = kind;
        e.speaker = speaker;
        e.flags = flags;
        crate::set_name(&mut e.text, text);
        e
    }

    pub fn text(&self) -> &str {
        crate::name_str(&self.text)
    }
}

impl VoiceLine {
    pub fn new(kind: u32, speaker: u32, line: u32, duration_ms: u32, how: &str, text: &str) -> VoiceLine {
        let mut l = VoiceLine::zeroed();
        l.kind = kind;
        l.speaker = speaker;
        l.line = line;
        l.duration_ms = duration_ms;
        crate::set_name(&mut l.how, how);
        crate::set_name(&mut l.text, text);
        l
    }

    pub fn how(&self) -> &str {
        crate::name_str(&self.how)
    }

    pub fn text(&self) -> &str {
        crate::name_str(&self.text)
    }
}

/// A `.lips` file (see the module's notes): `fps`, then the frames.
pub fn lips_bytes(fps: u32, frames: &[[f32; 16]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + frames.len() * 64);
    out.extend(fps.to_le_bytes());
    out.extend((frames.len() as u32).to_le_bytes());
    for frame in frames {
        for w in frame {
            out.extend(w.to_le_bytes());
        }
    }
    out
}

/// A `.lips` file's frames a second and frames (None: not one).
pub fn read_lips(bytes: &[u8]) -> Option<(u32, Vec<[f32; 16]>)> {
    let word = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let (fps, count) = (word(0)?, word(4)? as usize);
    if fps == 0 || bytes.len() != 8 + count * 64 {
        return None;
    }
    let frames = bytes[8..]
        .chunks_exact(64)
        .map(|f| std::array::from_fn(|p| f32::from_le_bytes(f[p * 4..p * 4 + 4].try_into().unwrap())))
        .collect();
    Some((fps, frames))
}

const _: () = {
    assert!(size_of::<VoiceSpeaker>() == 160);
    assert!(size_of::<VoiceEvent>() == 256);
    assert!(size_of::<VoiceLine>() == 512);
    assert!(std::mem::offset_of!(VoiceStream, taken_line) == 64 && std::mem::offset_of!(VoiceStream, samples) == 128);
    assert!(size_of::<VoiceScene>().is_multiple_of(4));
    assert!(OFF_VOICE_SCENE.is_multiple_of(64));
    assert!(OFF_VOICE_END <= crate::OFF_HUD_LAYOUT);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scene_names_its_speakers() {
        let mut s = VoiceScene::zeroed();
        assert!(s.add(0x0001_A2B3, "Hieronymus Lex", r"imperial\m"));
        assert!(!s.add(0, "no one", ""), "0 is never a speaker");
        s.listener = 0x0001_A2B3;
        assert_eq!(s.speaker(s.listener).map(|l| (l.name(), l.voice())), Some(("Hieronymus Lex", r"imperial\m")));
        assert!(s.speaker(0).is_none());
    }

    #[test]
    fn lips_files_read_back() {
        let mut frame = [0.0; 16];
        frame[2] = 1.0;
        let bytes = lips_bytes(30, &[frame, [0.25; 16]]);
        assert_eq!(read_lips(&bytes), Some((30, vec![frame, [0.25; 16]])));
        assert_eq!(read_lips(&bytes[..bytes.len() - 1]), None);
    }
}
