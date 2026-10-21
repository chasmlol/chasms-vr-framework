//! The output device: a background thread opens the default output with cpal (WASAPI on
//! Windows), runs the mixer in the stream's callback, and reopens the output if it goes away
//! (a headset unplugged, the default device changed).
//!
//! Nothing here may take the runtime down: every failure is logged and the sounds go silent,
//! with a retry every few seconds in case a device turns up.

use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError, TryLockError};
use std::thread;
use std::time::Duration;

use anyhow::{Context, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

use crate::mixer::Mixer;
use crate::{Command, Shared, sounds};

/// Wait before retrying when no device could be opened.
const RETRY: Duration = Duration::from_secs(3);
/// Wait before reopening after a stream died, so a replacement default device can settle.
const REOPEN: Duration = Duration::from_millis(500);
/// How long a driver may take to start a stream.
const OPEN_TIMEOUT: Duration = Duration::from_secs(2);

/// Finds the device to play on.
pub(crate) type Open = Box<dyn Fn() -> anyhow::Result<cpal::Device> + Send>;

pub(crate) fn default_output() -> anyhow::Result<cpal::Device> {
    cpal::default_host().default_output_device().context("no default audio output device")
}

pub(crate) enum Event {
    /// The `Audio` was dropped.
    Stop,
    /// A stream died. Streams are numbered so a late report about an old one is ignored.
    Lost { stream: u64, error: cpal::Error },
}

/// Starts the output thread. Send the returned channel [`Event::Stop`] to shut it down.
pub(crate) fn spawn(shared: Arc<Shared>, commands: Receiver<Command>, open: Open) -> Sender<Event> {
    let (events, inbox) = channel();
    let thread_events = events.clone();
    let started = thread::Builder::new()
        .name("rustvr-audio".into())
        .spawn(move || run(&shared, commands, &open, &thread_events, &inbox));
    if let Err(e) = started {
        tracing::error!("couldn't start the audio thread; interaction sounds are off: {e}");
    }
    events
}

fn run(
    shared: &Arc<Shared>,
    commands: Receiver<Command>,
    open: &Open,
    events: &Sender<Event>,
    inbox: &Receiver<Event>,
) {
    let mut commands = Some(commands);
    let mut mixer = None;
    let mut failures = 0u32;
    for stream in 1u64.. {
        let wait = match start(open, shared, &mut commands, &mut mixer, events, stream) {
            Ok(output) => {
                failures = 0;
                shared.live.store(true, Ordering::Release);
                let stopped = wait_for_loss(inbox, stream);
                shared.live.store(false, Ordering::Release);
                drop(output);
                if stopped {
                    return;
                }
                REOPEN
            }
            Err(e) => {
                // Said once at warn; after that at debug, or a machine without audio fills the log.
                if failures == 0 {
                    tracing::warn!("no audio output; interaction sounds are silent: {e:#}");
                } else {
                    tracing::debug!("audio output still unavailable: {e:#}");
                }
                failures += 1;
                RETRY
            }
        };
        match inbox.recv_timeout(wait) {
            Ok(Event::Stop) | Err(RecvTimeoutError::Disconnected) => return,
            Ok(Event::Lost { .. }) | Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

/// Blocks until the stream dies (false) or the `Audio` is dropped (true).
fn wait_for_loss(inbox: &Receiver<Event>, current: u64) -> bool {
    loop {
        match inbox.recv() {
            Ok(Event::Lost { stream, error }) if stream == current => {
                tracing::warn!("audio output lost ({error}); reopening");
                return false;
            }
            Ok(Event::Lost { .. }) => {}
            Ok(Event::Stop) | Err(_) => return true,
        }
    }
}

/// Opens the device and starts a stream on it, making the mixer the first time (once the
/// sample rate is known) and regenerating its sounds if a new device runs at another rate.
fn start(
    open: &Open,
    shared: &Arc<Shared>,
    commands: &mut Option<Receiver<Command>>,
    slot: &mut Option<Arc<Mutex<Mixer>>>,
    events: &Sender<Event>,
    stream: u64,
) -> anyhow::Result<cpal::Stream> {
    let device = open()?;
    let name = device.description().map_or_else(|_| "unnamed device".to_owned(), |d| d.name().to_owned());
    let config = device.default_output_config().with_context(|| format!("no output format on {name}"))?;
    let rate = config.sample_rate();
    let format = config.sample_format();
    if config.channels() == 0 {
        bail!("{name} has no output channels");
    }

    let mixer = match slot.as_ref() {
        Some(mixer) => {
            // No stream is running, so this lock is free.
            let mut m = mixer.lock().unwrap_or_else(PoisonError::into_inner);
            if m.rate() != rate {
                m.retune(sounds::bank(rate));
            }
            mixer.clone()
        }
        None => {
            let Some(commands) = commands.take() else { bail!("the mixer's command queue is gone") };
            let mixer = Arc::new(Mutex::new(Mixer::new(sounds::bank(rate), commands, shared.clone())));
            *slot = Some(mixer.clone());
            mixer
        }
    };

    let config = cpal::StreamConfig {
        channels: config.channels(),
        sample_rate: rate,
        buffer_size: cpal::BufferSize::Default,
    };
    let channels = config.channels;
    let events = events.clone();
    let on_error = move |error: cpal::Error| match error.kind() {
        // Glitches, not failures: the stream carries on.
        cpal::ErrorKind::Xrun | cpal::ErrorKind::RealtimeDenied | cpal::ErrorKind::DeviceChanged => {
            tracing::debug!("audio stream: {error}");
        }
        _ => {
            let _ = events.send(Event::Lost { stream, error });
        }
    };
    let output = match format {
        SampleFormat::F32 => build::<f32>(&device, config, mixer, on_error),
        SampleFormat::I16 => build::<i16>(&device, config, mixer, on_error),
        SampleFormat::I32 => build::<i32>(&device, config, mixer, on_error),
        SampleFormat::U16 => build::<u16>(&device, config, mixer, on_error),
        other => bail!("{name} wants {other} samples, which the mixer doesn't write"),
    }
    .with_context(|| format!("couldn't open a stream on {name}"))?;
    output.play().with_context(|| format!("couldn't start the stream on {name}"))?;
    tracing::info!(device = %name, rate, channels, %format, "audio output open");
    Ok(output)
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mixer: Arc<Mutex<Mixer>>,
    on_error: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let callback = move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
        // The device thread only takes this lock while no stream runs, so it is always free
        // here; if it somehow weren't, a block of silence beats waiting on the audio thread.
        let mut mixer = match mixer.try_lock() {
            Ok(mixer) => mixer,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => return,
        };
        mixer.mix(data.len() / channels, |i, l, r| {
            let frame = &mut data[i * channels..(i + 1) * channels];
            if let [mono] = frame {
                *mono = T::from_sample(0.5 * (l + r));
            } else {
                // Any further channels keep the silence cpal filled them with.
                frame[0] = T::from_sample(l);
                frame[1] = T::from_sample(r);
            }
        });
    };
    device.build_output_stream(config, callback, on_error, Some(OPEN_TIMEOUT))
}
