//! Frame pacing, said every ten seconds: how many frames the headset got, how many it showed late
//! (the next display time skipped a refresh or more: the runtime's frame wasn't ready in time),
//! how long the runtime's own work took on the CPU and on the GPU (between its first and last
//! command each frame, so time the GPU gave the game in between counts), and how many new pictures
//! the game gave. What a player feels as judder shows here as late frames or few pictures.

use anyhow::{Context, Result};
use rustvr_gpu::Gpu;
use windows::Win32::Graphics::Direct3D11::*;
use windows::core::Interface;

/// Timer sets in flight (a frame's are read a few frames later, without waiting).
const TIMERS: usize = 6;
/// Seconds between reports.
const EVERY: f64 = 10.0;

struct Timer {
    disjoint: ID3D11Query,
    start: ID3D11Query,
    end: ID3D11Query,
    pending: bool,
}

pub struct Pacing {
    since: Option<f64>,
    last_display: Option<i64>,
    frames: u32,
    late: u32,
    cpu: Vec<f32>,
    gpu: Vec<f32>,
    pictures: u32,
    /// New pictures with the player the game drew in them (its pose said).
    with_player: u32,
    /// Frames in the framework view, with the game's player moved, warped without one.
    framework: u32,
    in_frame: u32,
    warped: u32,
    /// Frames the game's stereo picture couldn't be moved, by why.
    no_depth: u32,
    no_map: u32,
    timers: Vec<Timer>,
    next: usize,
    open: Option<usize>,
}

fn query(gpu: &Gpu, kind: D3D11_QUERY) -> Result<ID3D11Query> {
    let mut q = None;
    unsafe { gpu.device.CreateQuery(&D3D11_QUERY_DESC { Query: kind, MiscFlags: 0 }, Some(&mut q))? };
    q.context("no query")
}

/// A query's data if it's there yet (never waits; S_FALSE while it isn't).
fn data<T: Default>(gpu: &Gpu, q: &ID3D11Query) -> Option<T> {
    let mut out = T::default();
    let hr = unsafe {
        (Interface::vtable(&gpu.ctx).GetData)(
            Interface::as_raw(&gpu.ctx),
            q.as_raw(),
            (&mut out as *mut T).cast(),
            size_of::<T>() as u32,
            D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32,
        )
    };
    (hr.0 == 0).then_some(out)
}

/// The value at `p` (0..1) of `v` (sorted in place).
fn percentile(v: &mut [f32], p: f32) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f32::total_cmp);
    v[((v.len() - 1) as f32 * p).round() as usize]
}

impl Pacing {
    pub fn new(gpu: &Gpu) -> Result<Pacing> {
        let timers = (0..TIMERS)
            .map(|_| Ok(Timer { disjoint: query(gpu, D3D11_QUERY_TIMESTAMP_DISJOINT)?, start: query(gpu, D3D11_QUERY_TIMESTAMP)?, end: query(gpu, D3D11_QUERY_TIMESTAMP)?, pending: false }))
            .collect::<Result<Vec<_>>>()?;
        Ok(Pacing {
            since: None,
            last_display: None,
            frames: 0,
            late: 0,
            cpu: Vec::new(),
            gpu: Vec::new(),
            pictures: 0,
            with_player: 0,
            framework: 0,
            in_frame: 0,
            warped: 0,
            no_depth: 0,
            no_map: 0,
            timers,
            next: 0,
            open: None,
        })
    }

    /// A headset frame for display at `display_ns`, `period` seconds after the one before should be.
    pub fn frame(&mut self, display_ns: i64, period: f64) {
        if let Some(last) = self.last_display {
            let gap = (display_ns - last) as f64 * 1e-9 / period.max(1e-4);
            if gap > 1.5 {
                self.late += (gap.round() as u32).saturating_sub(1).max(1);
            }
        }
        self.last_display = Some(display_ns);
        self.frames += 1;
    }

    /// The runtime's GPU work for this frame starts.
    pub fn gpu_begin(&mut self, gpu: &Gpu) {
        // (A frame that ended early never closed its timer.)
        self.gpu_end(gpu);
        self.collect(gpu);
        let i = self.next;
        if self.timers[i].pending {
            // Still not read (the GPU is far behind): this frame goes untimed.
            return;
        }
        let t = &mut self.timers[i];
        unsafe {
            gpu.ctx.Begin(&t.disjoint);
            gpu.ctx.End(&t.start);
        }
        self.open = Some(i);
    }

    /// ... and ends.
    pub fn gpu_end(&mut self, gpu: &Gpu) {
        let Some(i) = self.open.take() else { return };
        let t = &mut self.timers[i];
        unsafe {
            gpu.ctx.End(&t.end);
            gpu.ctx.End(&t.disjoint);
        }
        t.pending = true;
        self.next = (i + 1) % TIMERS;
    }

    fn collect(&mut self, gpu: &Gpu) {
        for t in self.timers.iter_mut().filter(|t| t.pending) {
            let Some(freq) = data::<D3D11_QUERY_DATA_TIMESTAMP_DISJOINT>(gpu, &t.disjoint) else { continue };
            let (Some(a), Some(b)) = (data::<u64>(gpu, &t.start), data::<u64>(gpu, &t.end)) else { continue };
            t.pending = false;
            if !freq.Disjoint.as_bool() && freq.Frequency > 0 && b >= a {
                self.gpu.push(((b - a) as f64 * 1000.0 / freq.Frequency as f64) as f32);
            }
        }
    }

    /// The runtime's CPU time for the frame (ms), and what it showed.
    pub fn cpu(&mut self, ms: f32, framework: bool, in_frame: bool, warped: bool) {
        self.cpu.push(ms);
        self.framework += framework as u32;
        self.in_frame += in_frame as u32;
        self.warped += warped as u32;
    }

    /// The game's stereo picture couldn't be moved to this frame: it came without its depth (the
    /// linear kind), or without where its eyes were in the world.
    pub fn not_moved(&mut self, no_depth: bool, no_map: bool) {
        self.no_depth += no_depth as u32;
        self.no_map += no_map as u32;
    }

    /// The game gave a new picture (`with_player`: with the player it drew in it, its pose said).
    pub fn picture(&mut self, with_player: bool) {
        self.pictures += 1;
        self.with_player += with_player as u32;
    }

    /// Every ten seconds (at `now`, seconds): the report, and a fresh count.
    pub fn report(&mut self, now: f64) -> Option<String> {
        let since = *self.since.get_or_insert(now);
        if now - since < EVERY || self.frames == 0 {
            return None;
        }
        let secs = (now - since).max(1e-3);
        let (mut cpu, mut gpu) = (std::mem::take(&mut self.cpu), std::mem::take(&mut self.gpu));
        let mean = |v: &[f32]| if v.is_empty() { 0.0 } else { v.iter().sum::<f32>() / v.len() as f32 };
        let (cm, gm) = (mean(&cpu), mean(&gpu));
        let mode = if self.framework * 2 > self.frames {
            "the framework view"
        } else if self.in_frame * 2 > self.frames {
            "the game's picture moved, the player it drew moved to where it is"
        } else if self.warped * 2 > self.frames {
            "the game's picture moved"
        } else {
            "the game's picture as it drew it"
        };
        let line = format!(
            "pacing: {} frames in {secs:.1} s ({:.1}/s), {} shown late; the runtime's work {cm:.1} ms on the CPU (95% under {:.1}, most {:.1}), {gm:.1} ms from its first GPU command to its last (95% under {:.1}, most {:.1}); the game's pictures {:.1}/s ({} with its player's pose); {mode} ({} frames moved, {} with the player moved, {} framework; not moved: {} without the game's depth, {} without where its eyes were)",
            self.frames,
            self.frames as f64 / secs,
            self.late,
            percentile(&mut cpu, 0.95),
            percentile(&mut cpu, 1.0),
            percentile(&mut gpu, 0.95),
            percentile(&mut gpu, 1.0),
            self.pictures as f64 / secs,
            self.with_player,
            self.warped,
            self.in_frame,
            self.framework,
            self.no_depth,
            self.no_map
        );
        self.since = Some(now);
        (self.frames, self.late, self.pictures, self.with_player, self.framework, self.in_frame, self.warped) = (0, 0, 0, 0, 0, 0, 0);
        (self.no_depth, self.no_map) = (0, 0);
        Some(line)
    }
}
