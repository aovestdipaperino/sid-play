//! Where the samples go. The chip does not know and does not care.

use std::io::{self, Seek, SeekFrom, Write};
use std::path::Path;

pub trait Sink {
    type Error;
    /// One block of mono samples at the chip's host sample rate.
    fn write(&mut self, samples: &[f32]) -> Result<(), Self::Error>;
    /// Flush and close. Called once on clean shutdown.
    fn finish(&mut self) -> Result<(), Self::Error>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    Pcm16,
    F32,
}

#[derive(Debug, Clone, Copy)]
pub struct WavSpec {
    pub sample_rate: u32,
    pub channels: u16,
    pub format: SampleFormat,
}

impl WavSpec {
    fn bytes_per_frame(self) -> u64 {
        u64::from(self.channels)
            * match self.format {
                SampleFormat::Pcm16 => 2,
                SampleFormat::F32 => 4,
            }
    }
}

pub struct WavSink {
    file: std::fs::File,
    spec: WavSpec,
    frames: u64,
}

impl WavSink {
    pub fn create(path: &Path, spec: WavSpec) -> io::Result<Self> {
        let mut file = std::fs::File::create(path)?;
        file.write_all(&Self::header(spec, 0, 0))?;
        Ok(Self {
            file,
            spec,
            frames: 0,
        })
    }

    fn header(spec: WavSpec, riff_len: u32, data_len: u32) -> [u8; 44] {
        let (tag, bits) = match spec.format {
            SampleFormat::Pcm16 => (1u16, 16u16),
            SampleFormat::F32 => (3, 32),
        };
        let block_align = spec.channels * bits / 8;
        let byte_rate = spec.sample_rate * u32::from(block_align);
        let mut h = [0u8; 44];
        h[0..4].copy_from_slice(b"RIFF");
        h[4..8].copy_from_slice(&riff_len.to_le_bytes());
        h[8..12].copy_from_slice(b"WAVE");
        h[12..16].copy_from_slice(b"fmt ");
        h[16..20].copy_from_slice(&16u32.to_le_bytes());
        h[20..22].copy_from_slice(&tag.to_le_bytes());
        h[22..24].copy_from_slice(&spec.channels.to_le_bytes());
        h[24..28].copy_from_slice(&spec.sample_rate.to_le_bytes());
        h[28..32].copy_from_slice(&byte_rate.to_le_bytes());
        h[32..34].copy_from_slice(&block_align.to_le_bytes());
        h[34..36].copy_from_slice(&bits.to_le_bytes());
        h[36..40].copy_from_slice(b"data");
        h[40..44].copy_from_slice(&data_len.to_le_bytes());
        h
    }

    pub fn samples_written(&self) -> u64 {
        self.frames
    }
}

impl Sink for WavSink {
    type Error = io::Error;

    fn write(&mut self, samples: &[f32]) -> Result<(), io::Error> {
        let mut buf = Vec::with_capacity(samples.len() * usize::from(self.spec.channels) * 4);
        for &s in samples {
            let s = s.clamp(-1.0, 1.0);
            for _ in 0..self.spec.channels {
                match self.spec.format {
                    SampleFormat::Pcm16 => {
                        // Full negative scale is one step wider than full positive
                        // scale; the float-to-int cast saturates, so +1.0 lands on
                        // 32767 and -1.0 on -32768 without special-casing either end.
                        let v = (s * 32_768.0).round() as i16;
                        buf.extend_from_slice(&v.to_le_bytes());
                    }
                    SampleFormat::F32 => buf.extend_from_slice(&s.to_le_bytes()),
                }
            }
        }
        self.file.write_all(&buf)?;
        self.frames += samples.len() as u64;
        Ok(())
    }

    fn finish(&mut self) -> Result<(), io::Error> {
        let data_len = self.frames * self.spec.bytes_per_frame();
        let file_len = 44 + data_len;
        if data_len > u64::from(u32::MAX) || file_len > u64::from(u32::MAX) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "WAV longer than 4 GiB; split the render",
            ));
        }
        let header = Self::header(self.spec, (file_len - 8) as u32, data_len as u32);
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&header)?;
        self.file.flush()
    }
}

// ---------------------------------------------------------------------------
// Live output. The pure pieces below (interleaving and the blocking ring) carry
// no cpal types, so they are tested in every build; only `AudioSink` itself
// needs the `playback` feature.

/// Write mono samples onto `channels` interleaved device channels, converting
/// each with `convert`. Frames the mono slice cannot fill, and any stray tail
/// that is not a whole frame, get `silence`. Returns how many mono samples were
/// used.
#[cfg_attr(not(feature = "playback"), allow(dead_code))]
fn interleave<T: Copy>(
    mono: &[f32],
    channels: usize,
    out: &mut [T],
    convert: impl Fn(f32) -> T,
    silence: T,
) -> usize {
    let channels = channels.max(1);
    let mut used = 0;
    let mut frames = out.chunks_exact_mut(channels);
    // Mono goes first in the zip: when it runs out, no frame is consumed.
    for (&s, frame) in mono.iter().zip(&mut frames) {
        frame.fill(convert(s));
        used += 1;
    }
    for frame in frames {
        frame.fill(silence);
    }
    let whole = out.len() - out.len() % channels;
    out[whole..].fill(silence);
    used
}

/// A bounded queue of mono samples between the render thread and the device
/// callback. The writer blocks while it is full, so the device sets the pace;
/// the reader never waits.
#[cfg_attr(not(feature = "playback"), allow(dead_code))]
struct Ring {
    queue: std::sync::Mutex<std::collections::VecDeque<f32>>,
    changed: std::sync::Condvar,
    capacity: usize,
    /// Total samples the reader has taken; the stall check watches it move.
    drained: std::sync::atomic::AtomicU64,
    /// How long a blocked waiter tolerates no draining before it gives up.
    stall: std::time::Duration,
}

/// Why a blocked wait on the ring ended early.
#[cfg_attr(not(feature = "playback"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    /// The caller's `give_up` said so (a stored stream failure).
    GaveUp,
    /// The reader made no progress for the stall timeout.
    Stalled,
}

/// The first failure the device reports. Sticky: it is never cleared, so every
/// later check still sees it and a blocked waiter keeps being released.
#[cfg_attr(not(feature = "playback"), allow(dead_code))]
struct Latch<E> {
    slot: std::sync::Mutex<Option<E>>,
}

#[cfg_attr(not(feature = "playback"), allow(dead_code))]
impl<E: Clone> Latch<E> {
    fn new() -> Self {
        Self {
            slot: std::sync::Mutex::new(None),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<E>> {
        self.slot.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Record `err` unless one is already held; later ones are usually echoes.
    fn set_first(&self, err: E) {
        let mut slot = self.lock();
        if slot.is_none() {
            *slot = Some(err);
        }
    }

    fn is_set(&self) -> bool {
        self.lock().is_some()
    }

    /// `Err` with a copy of the stored failure, every time it is asked.
    fn check(&self) -> Result<(), E> {
        match &*self.lock() {
            Some(e) => Err(e.clone()),
            None => Ok(()),
        }
    }
}

#[cfg_attr(not(feature = "playback"), allow(dead_code))]
impl Ring {
    /// How long a blocked writer sleeps before it asks again whether to give up.
    const POLL: std::time::Duration = std::time::Duration::from_millis(20);

    /// No draining for this long while a waiter is blocked means the device
    /// has silently stopped calling back.
    const STALL: std::time::Duration = std::time::Duration::from_secs(2);

    fn new(capacity: usize) -> Self {
        Self::with_stall(capacity, Self::STALL)
    }

    fn with_stall(capacity: usize, stall: std::time::Duration) -> Self {
        Self {
            queue: std::sync::Mutex::new(std::collections::VecDeque::with_capacity(capacity)),
            changed: std::sync::Condvar::new(),
            capacity: capacity.max(1),
            drained: std::sync::atomic::AtomicU64::new(0),
            stall,
        }
    }

    /// Tracks reader progress for one wait: true once the reader has taken
    /// nothing for `stall`. Call it each time the waiter wakes.
    fn stalled(&self, mark: &mut (u64, std::time::Instant)) -> bool {
        let now = self.drained.load(std::sync::atomic::Ordering::Relaxed);
        if now != mark.0 {
            *mark = (now, std::time::Instant::now());
            false
        } else {
            mark.1.elapsed() >= self.stall
        }
    }

    /// A panicking holder cannot leave a queue of floats inconsistent, so a
    /// poisoned lock is simply taken over.
    fn lock(&self) -> std::sync::MutexGuard<'_, std::collections::VecDeque<f32>> {
        self.queue.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.lock().len()
    }

    /// Queue every sample, waiting for room as needed. Stops early with
    /// `GaveUp` if `give_up` says so while the ring is full, or with `Stalled`
    /// if the reader takes nothing for the stall timeout.
    fn push_blocking(&self, samples: &[f32], give_up: impl Fn() -> bool) -> Result<(), Stop> {
        let mut pushed = 0;
        let mut q = self.lock();
        let mut mark = (
            self.drained.load(std::sync::atomic::Ordering::Relaxed),
            std::time::Instant::now(),
        );
        while pushed < samples.len() {
            let room = self.capacity - q.len();
            if room == 0 {
                if give_up() {
                    return Err(Stop::GaveUp);
                }
                if self.stalled(&mut mark) {
                    return Err(Stop::Stalled);
                }
                q = self
                    .changed
                    .wait_timeout(q, Self::POLL)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
                continue;
            }
            let take = room.min(samples.len() - pushed);
            q.extend(&samples[pushed..pushed + take]);
            pushed += take;
        }
        Ok(())
    }

    /// Move as many queued samples as fit into `out`, without waiting for more.
    fn drain_into(&self, out: &mut [f32]) -> usize {
        let n = {
            let mut q = self.lock();
            let n = out.len().min(q.len());
            for (slot, s) in out.iter_mut().zip(q.drain(..n)) {
                *slot = s;
            }
            n
        };
        if n > 0 {
            self.drained
                .fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
            self.changed.notify_all();
        }
        n
    }

    /// Wait until the reader has taken everything, or stop early as
    /// `push_blocking` does.
    fn wait_drained(&self, give_up: impl Fn() -> bool) -> Result<(), Stop> {
        let mut q = self.lock();
        let mut mark = (
            self.drained.load(std::sync::atomic::Ordering::Relaxed),
            std::time::Instant::now(),
        );
        loop {
            if q.is_empty() {
                return Ok(());
            }
            if give_up() {
                return Err(Stop::GaveUp);
            }
            if self.stalled(&mut mark) {
                return Err(Stop::Stalled);
            }
            q = self
                .changed
                .wait_timeout(q, Self::POLL)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

#[cfg(feature = "playback")]
pub use audio::{AudioError, AudioSink};

#[cfg(feature = "playback")]
mod audio {
    use super::{Latch, Ring, Sink, Stop, interleave};
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, Sample, SampleFormat, SizedSample};
    use std::fmt;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// How much audio the ring holds before `write` blocks: long enough to ride
    /// out a scheduling hiccup, short enough that stopping feels immediate.
    const BUFFER_SECONDS: f64 = 0.35;

    /// Formats the callback converts to, best first.
    const FORMATS: [SampleFormat; 8] = [
        SampleFormat::F32,
        SampleFormat::I16,
        SampleFormat::U16,
        SampleFormat::I32,
        SampleFormat::F64,
        SampleFormat::U32,
        SampleFormat::I8,
        SampleFormat::U8,
    ];

    #[derive(Debug, Clone)]
    pub enum AudioError {
        /// The host reports no default output device.
        NoDevice,
        /// The device would not list its output configurations.
        Configs(cpal::Error),
        /// No output configuration runs at the requested rate. `available`
        /// lists the device's rate ranges, inclusive.
        UnsupportedRate {
            requested: u32,
            available: Vec<(u32, u32)>,
        },
        BuildStream(cpal::Error),
        PlayStream(cpal::Error),
        /// Reported by the device while the stream was running.
        Stream(cpal::Error),
        /// The device stopped calling back while samples were waiting.
        Stalled,
    }

    impl fmt::Display for AudioError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::NoDevice => f.write_str("no default audio output device"),
                Self::Configs(e) => write!(f, "could not list the output device's formats: {e}"),
                Self::UnsupportedRate {
                    requested,
                    available,
                } => {
                    write!(
                        f,
                        "the output device does not run at {requested} Hz; it offers"
                    )?;
                    if available.is_empty() {
                        return f.write_str(" no output formats at all");
                    }
                    for (i, (lo, hi)) in available.iter().enumerate() {
                        let sep = if i == 0 { " " } else { ", " };
                        if lo == hi {
                            write!(f, "{sep}{lo} Hz")?;
                        } else {
                            write!(f, "{sep}{lo}-{hi} Hz")?;
                        }
                    }
                    Ok(())
                }
                Self::BuildStream(e) => write!(f, "could not open the output stream: {e}"),
                Self::PlayStream(e) => write!(f, "could not start the output stream: {e}"),
                Self::Stream(e) => write!(f, "the output stream failed: {e}"),
                Self::Stalled => f.write_str("the audio device stopped consuming samples"),
            }
        }
    }

    impl std::error::Error for AudioError {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            match self {
                Self::Configs(e) | Self::BuildStream(e) | Self::PlayStream(e) | Self::Stream(e) => {
                    Some(e)
                }
                Self::NoDevice | Self::UnsupportedRate { .. } | Self::Stalled => None,
            }
        }
    }

    /// Glitches the stream recovers from on its own; they are not worth
    /// stopping a tune for.
    fn is_transient(kind: cpal::ErrorKind) -> bool {
        matches!(
            kind,
            cpal::ErrorKind::Xrun
                | cpal::ErrorKind::DeviceChanged
                | cpal::ErrorKind::RealtimeDenied
        )
    }

    /// Convert one sample to the device's format, clamped to full scale.
    pub(super) fn to_device<T: Sample + FromSample<f32>>(s: f32) -> T {
        T::from_sample(s.clamp(-1.0, 1.0))
    }

    /// Plays through the default output device. The mono stream is copied onto
    /// every device channel.
    pub struct AudioSink {
        stream: Option<cpal::Stream>,
        ring: Arc<Ring>,
        failed: Arc<Latch<cpal::Error>>,
        /// The largest buffer, in frames, the device has asked for so far.
        period: Arc<AtomicUsize>,
        sample_rate: u32,
    }

    impl AudioSink {
        pub fn open(sample_rate: u32) -> Result<Self, AudioError> {
            let host = cpal::default_host();
            let device = host.default_output_device().ok_or(AudioError::NoDevice)?;
            let ranges: Vec<_> = device
                .supported_output_configs()
                .map_err(AudioError::Configs)?
                .collect();

            let at_rate = ranges.iter().filter(|r| {
                r.min_sample_rate() <= sample_rate && sample_rate <= r.max_sample_rate()
            });
            let chosen = at_rate
                .clone()
                .filter_map(|r| {
                    let rank = FORMATS.iter().position(|f| *f == r.sample_format())?;
                    // Stereo first, then the fewest channels.
                    Some(((rank, r.channels() != 2, r.channels()), *r))
                })
                .min_by_key(|(key, _)| *key)
                .map(|(_, r)| r);
            let Some(range) = chosen else {
                if at_rate.count() > 0 {
                    return Err(AudioError::BuildStream(cpal::Error::with_message(
                        cpal::ErrorKind::UnsupportedConfig,
                        format!(
                            "no output format at {sample_rate} Hz that this player converts to"
                        ),
                    )));
                }
                let mut available: Vec<(u32, u32)> = ranges
                    .iter()
                    .map(|r| (r.min_sample_rate(), r.max_sample_rate()))
                    .collect();
                available.sort_unstable();
                available.dedup();
                return Err(AudioError::UnsupportedRate {
                    requested: sample_rate,
                    available,
                });
            };
            let format = range.sample_format();
            let config = range.with_sample_rate(sample_rate).config();

            let capacity = (f64::from(sample_rate) * BUFFER_SECONDS) as usize;
            let ring = Arc::new(Ring::new(capacity));
            let failed = Arc::new(Latch::new());
            let period = Arc::new(AtomicUsize::new(0));

            let stream = match format {
                SampleFormat::F32 => build::<f32>(&device, config, &ring, &failed, &period),
                SampleFormat::I16 => build::<i16>(&device, config, &ring, &failed, &period),
                SampleFormat::U16 => build::<u16>(&device, config, &ring, &failed, &period),
                SampleFormat::I32 => build::<i32>(&device, config, &ring, &failed, &period),
                SampleFormat::F64 => build::<f64>(&device, config, &ring, &failed, &period),
                SampleFormat::U32 => build::<u32>(&device, config, &ring, &failed, &period),
                SampleFormat::I8 => build::<i8>(&device, config, &ring, &failed, &period),
                SampleFormat::U8 => build::<u8>(&device, config, &ring, &failed, &period),
                // `FORMATS` admits nothing else.
                other => Err(cpal::Error::with_message(
                    cpal::ErrorKind::UnsupportedConfig,
                    format!("unsupported sample format {other}"),
                )),
            }
            .map_err(AudioError::BuildStream)?;
            stream.play().map_err(AudioError::PlayStream)?;

            Ok(Self {
                stream: Some(stream),
                ring,
                failed,
                period,
                sample_rate,
            })
        }

        /// The error the device reported, if any. Sticky: the failure stays
        /// stored, so every later call returns it again and nothing can block
        /// on a dead stream after the first report.
        fn check_failure(&self) -> Result<(), AudioError> {
            self.failed.check().map_err(AudioError::Stream)
        }
    }

    fn build<T>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        ring: &Arc<Ring>,
        failed: &Arc<Latch<cpal::Error>>,
        period: &Arc<AtomicUsize>,
    ) -> Result<cpal::Stream, cpal::Error>
    where
        T: SizedSample + Sample + FromSample<f32>,
    {
        let channels = usize::from(config.channels).max(1);
        let ring = Arc::clone(ring);
        let period_cb = Arc::clone(period);
        let failed = Arc::clone(failed);
        let mut scratch: Vec<f32> = Vec::new();
        device.build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                let frames = data.len() / channels;
                period_cb.fetch_max(frames, Ordering::Relaxed);
                if scratch.len() < frames {
                    scratch.resize(frames, 0.0);
                }
                let n = ring.drain_into(&mut scratch[..frames]);
                interleave(
                    &scratch[..n],
                    channels,
                    data,
                    to_device::<T>,
                    T::EQUILIBRIUM,
                );
            },
            move |err: cpal::Error| {
                if is_transient(err.kind()) {
                    return;
                }
                // Keep the first failure; later ones are usually its echoes.
                failed.set_first(err);
            },
            None,
        )
    }

    impl Sink for AudioSink {
        type Error = AudioError;

        /// Fails with `Stream` on every call once the device has reported a
        /// failure, and with `Stalled` if the device stops draining for about
        /// two seconds.
        fn write(&mut self, samples: &[f32]) -> Result<(), AudioError> {
            self.check_failure()?;
            let failed = Arc::clone(&self.failed);
            match self.ring.push_blocking(samples, || failed.is_set()) {
                Ok(()) | Err(Stop::GaveUp) => {}
                Err(Stop::Stalled) => return Err(AudioError::Stalled),
            }
            self.check_failure()
        }

        fn finish(&mut self) -> Result<(), AudioError> {
            let failed = Arc::clone(&self.failed);
            let drained = self.ring.wait_drained(|| failed.is_set());
            if drained.is_ok() {
                // The last samples left the ring but may still sit in the
                // device's own buffer; give it one buffer's worth to play out.
                let frames = self.period.load(Ordering::Relaxed).max(1024);
                std::thread::sleep(Duration::from_secs_f64(
                    frames as f64 / f64::from(self.sample_rate),
                ));
            }
            if let Some(stream) = self.stream.take() {
                // Dropping the stream stops it; pausing first is a courtesy the
                // drop does not need, so its failure is not an error here.
                let _ = stream.pause();
            }
            self.check_failure()?;
            match drained {
                Err(Stop::Stalled) => Err(AudioError::Stalled),
                _ => Ok(()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn temp(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "rust-sid-{}-{}-{}.wav",
            name,
            std::process::id(),
            // Two tests in the same process must still not share a file.
            {
                use std::sync::atomic::{AtomicU64, Ordering};
                static SEQ: AtomicU64 = AtomicU64::new(0);
                SEQ.fetch_add(1, Ordering::Relaxed)
            }
        ));
        p
    }

    fn header(path: &std::path::Path) -> Vec<u8> {
        let mut f = std::fs::File::open(path).unwrap();
        let mut b = vec![0u8; 44];
        f.read_exact(&mut b).unwrap();
        b
    }

    #[test]
    fn writes_a_canonical_riff_header() {
        let path = temp("hdr");
        let spec = WavSpec {
            sample_rate: 44_100,
            channels: 1,
            format: SampleFormat::Pcm16,
        };
        let mut sink = WavSink::create(&path, spec).unwrap();
        sink.write(&[0.0; 128]).unwrap();
        sink.finish().unwrap();

        let h = header(&path);
        assert_eq!(&h[0..4], b"RIFF");
        assert_eq!(&h[8..12], b"WAVE");
        assert_eq!(&h[12..16], b"fmt ");
        assert_eq!(u32::from_le_bytes(h[16..20].try_into().unwrap()), 16);
        assert_eq!(
            u16::from_le_bytes(h[20..22].try_into().unwrap()),
            1,
            "PCM tag"
        );
        assert_eq!(u16::from_le_bytes(h[22..24].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(h[24..28].try_into().unwrap()), 44_100);
        assert_eq!(
            u16::from_le_bytes(h[34..36].try_into().unwrap()),
            16,
            "bits"
        );
        assert_eq!(&h[36..40], b"data");
        assert_eq!(
            u32::from_le_bytes(h[40..44].try_into().unwrap()),
            256,
            "byte count"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn patches_both_length_fields_on_finish() {
        let path = temp("patch");
        let spec = WavSpec {
            sample_rate: 22_050,
            channels: 2,
            format: SampleFormat::Pcm16,
        };
        let mut sink = WavSink::create(&path, spec).unwrap();
        sink.write(&[0.25; 100]).unwrap();
        sink.finish().unwrap();

        let file_len = std::fs::metadata(&path).unwrap().len();
        let h = header(&path);
        let riff = u32::from_le_bytes(h[4..8].try_into().unwrap()) as u64;
        let data = u32::from_le_bytes(h[40..44].try_into().unwrap()) as u64;
        assert_eq!(riff, file_len - 8, "RIFF size not patched");
        // 100 mono frames duplicated onto two channels, 2 bytes each.
        assert_eq!(data, 400, "data size not patched");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn float_format_is_tagged_as_ieee() {
        let path = temp("f32");
        let spec = WavSpec {
            sample_rate: 44_100,
            channels: 1,
            format: SampleFormat::F32,
        };
        let mut sink = WavSink::create(&path, spec).unwrap();
        sink.write(&[0.0; 4]).unwrap();
        sink.finish().unwrap();
        let h = header(&path);
        assert_eq!(
            u16::from_le_bytes(h[20..22].try_into().unwrap()),
            3,
            "IEEE float tag"
        );
        assert_eq!(u16::from_le_bytes(h[34..36].try_into().unwrap()), 32);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn samples_are_clamped_to_full_scale() {
        let path = temp("clamp");
        let spec = WavSpec {
            sample_rate: 44_100,
            channels: 1,
            format: SampleFormat::Pcm16,
        };
        let mut sink = WavSink::create(&path, spec).unwrap();
        sink.write(&[9.0, -9.0]).unwrap();
        sink.finish().unwrap();

        let body = std::fs::read(&path).unwrap();
        let samples: Vec<i16> = body[44..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(samples, vec![32_767, -32_768]);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_sink_can_be_swapped_without_changing_the_caller() {
        // The point of the trait: this function knows nothing about WAV.
        fn pump<S: Sink<Error = std::io::Error>>(sink: &mut S) -> std::io::Result<()> {
            for _ in 0..4 {
                sink.write(&[0.1, 0.2, 0.3, 0.4])?;
            }
            sink.finish()
        }
        let path = temp("swap");
        let mut sink = WavSink::create(
            &path,
            WavSpec {
                sample_rate: 8_000,
                channels: 1,
                format: SampleFormat::Pcm16,
            },
        )
        .unwrap();
        pump(&mut sink).unwrap();
        assert_eq!(sink.samples_written(), 16);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_sink_works_behind_a_trait_object() {
        // The playback loop in a later task holds `Box<dyn Sink>`; if the trait ever
        // stops being object safe that loop stops compiling, and this fails first.
        let path = temp("dyn");
        let sink = WavSink::create(
            &path,
            WavSpec {
                sample_rate: 44_100,
                channels: 1,
                format: SampleFormat::Pcm16,
            },
        )
        .unwrap();
        let mut boxed: Box<dyn Sink<Error = std::io::Error>> = Box::new(sink);
        boxed.write(&[0.5, -0.5]).unwrap();
        boxed.finish().unwrap();
        drop(boxed);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 44 + 4);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn interleave_duplicates_mono_onto_every_channel() {
        let mut out = [0i32; 6];
        let used = interleave(&[1.0, 2.0, 3.0], 2, &mut out, |s| s as i32, -1);
        assert_eq!(used, 3);
        assert_eq!(out, [1, 1, 2, 2, 3, 3]);
    }

    #[test]
    fn interleave_pads_an_underrun_with_silence() {
        let mut out = [0i32; 9];
        let used = interleave(&[5.0], 3, &mut out, |s| s as i32, 7);
        assert_eq!(used, 1, "only one frame was available");
        assert_eq!(out, [5, 5, 5, 7, 7, 7, 7, 7, 7]);
    }

    #[test]
    fn interleave_takes_no_more_frames_than_fit() {
        let mut out = [0i32; 4];
        let used = interleave(&[1.0, 2.0, 3.0, 4.0], 2, &mut out, |s| s as i32, 0);
        assert_eq!(used, 2);
        assert_eq!(out, [1, 1, 2, 2]);
    }

    #[test]
    fn interleave_survives_a_ragged_device_buffer() {
        // A buffer that is not a whole number of frames must not panic; the
        // stray tail is filled with silence.
        let mut out = [9i32; 5];
        let used = interleave(&[1.0, 2.0, 3.0], 2, &mut out, |s| s as i32, 0);
        assert_eq!(used, 2);
        assert_eq!(out, [1, 1, 2, 2, 0]);
    }

    #[test]
    fn ring_drains_in_order_and_reports_how_much_it_gave() {
        let ring = Ring::new(8);
        ring.push_blocking(&[0.1, 0.2, 0.3], || false).unwrap();
        let mut out = [0.0f32; 5];
        assert_eq!(ring.drain_into(&mut out), 3);
        assert_eq!(&out[..3], &[0.1, 0.2, 0.3]);
        assert_eq!(ring.len(), 0);
        assert_eq!(ring.drain_into(&mut out), 0, "an empty ring gives nothing");
    }

    #[test]
    fn ring_blocks_the_writer_while_full() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        let ring = Arc::new(Ring::new(4));
        let done = Arc::new(AtomicBool::new(false));
        let writer = {
            let ring = Arc::clone(&ring);
            let done = Arc::clone(&done);
            std::thread::spawn(move || {
                let samples: Vec<f32> = (0..10).map(|i| i as f32).collect();
                ring.push_blocking(&samples, || false).unwrap();
                done.store(true, Ordering::SeqCst);
            })
        };
        // Wait (bounded) until the ring is full, then check the writer is
        // still stuck behind it rather than guessing with a fixed sleep.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while ring.len() < 4 {
            assert!(std::time::Instant::now() < deadline, "ring never filled");
            std::thread::yield_now();
        }
        assert!(
            !done.load(Ordering::SeqCst),
            "the writer ran past a full ring"
        );
        assert_eq!(ring.len(), 4, "the ring holds more than its capacity");

        let mut got = Vec::new();
        let mut out = [0.0f32; 3];
        while got.len() < 10 {
            let n = ring.drain_into(&mut out);
            got.extend_from_slice(&out[..n]);
            std::thread::yield_now();
        }
        writer.join().unwrap();
        assert!(done.load(Ordering::SeqCst));
        assert_eq!(got, (0..10).map(|i| i as f32).collect::<Vec<_>>());
    }

    #[test]
    fn ring_writer_gives_up_when_told_to() {
        // A dead stream never drains; the writer must not hang on it.
        let ring = Ring::new(2);
        let r = ring.push_blocking(&[1.0, 2.0, 3.0, 4.0], || true);
        assert_eq!(r, Err(Stop::GaveUp));
        assert_eq!(ring.len(), 2);
    }

    #[test]
    fn ring_wait_drained_returns_once_empty() {
        use std::sync::Arc;
        let ring = Arc::new(Ring::new(4));
        ring.push_blocking(&[1.0, 2.0], || false).unwrap();
        let reader = {
            let ring = Arc::clone(&ring);
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(50));
                let mut out = [0.0f32; 4];
                ring.drain_into(&mut out);
            })
        };
        assert_eq!(ring.wait_drained(|| false), Ok(()));
        assert_eq!(ring.len(), 0);
        reader.join().unwrap();
        // Told to give up with samples still queued, it reports that.
        ring.push_blocking(&[1.0], || false).unwrap();
        assert_eq!(ring.wait_drained(|| true), Err(Stop::GaveUp));
    }

    #[test]
    fn a_stored_failure_releases_a_blocked_writer() {
        use std::sync::Arc;
        let ring = Arc::new(Ring::new(2));
        let latch = Arc::new(Latch::<String>::new());
        ring.push_blocking(&[1.0, 2.0], || false).unwrap();
        let writer = {
            let (ring, latch) = (Arc::clone(&ring), Arc::clone(&latch));
            std::thread::spawn(move || ring.push_blocking(&[3.0], || latch.is_set()))
        };
        std::thread::sleep(std::time::Duration::from_millis(30));
        latch.set_first("boom".into());
        assert_eq!(writer.join().unwrap(), Err(Stop::GaveUp));
    }

    #[test]
    fn a_failure_is_sticky_across_later_writes_and_finish() {
        let ring = Ring::new(2);
        let latch = Latch::<String>::new();
        assert_eq!(latch.check(), Ok(()));
        ring.push_blocking(&[1.0, 2.0], || false).unwrap();
        latch.set_first("first".into());
        latch.set_first("echo".into());
        // Nothing drains, yet every later wait returns at once with the same error.
        for _ in 0..2 {
            assert_eq!(latch.check(), Err("first".to_string()));
            assert_eq!(
                ring.push_blocking(&[3.0], || latch.is_set()),
                Err(Stop::GaveUp)
            );
        }
        assert_eq!(latch.check(), Err("first".to_string()));
        assert_eq!(ring.wait_drained(|| latch.is_set()), Err(Stop::GaveUp));
        assert_eq!(latch.check(), Err("first".to_string()));
    }

    #[test]
    fn a_silent_device_stalls_the_writer_and_the_drain_wait() {
        let stall = std::time::Duration::from_millis(60);
        let ring = Ring::with_stall(2, stall);
        let start = std::time::Instant::now();
        assert_eq!(
            ring.push_blocking(&[1.0, 2.0, 3.0], || false),
            Err(Stop::Stalled)
        );
        assert!(start.elapsed() >= stall);
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
        assert_eq!(ring.wait_drained(|| false), Err(Stop::Stalled));
    }

    #[test]
    fn a_slow_but_moving_reader_does_not_stall() {
        use std::sync::Arc;
        let ring = Arc::new(Ring::with_stall(1, std::time::Duration::from_millis(100)));
        let reader = {
            let ring = Arc::clone(&ring);
            std::thread::spawn(move || {
                let mut out = [0.0f32; 1];
                let mut got = 0;
                while got < 8 {
                    std::thread::sleep(std::time::Duration::from_millis(30));
                    got += ring.drain_into(&mut out);
                }
            })
        };
        let samples = [0.5f32; 8];
        assert_eq!(ring.push_blocking(&samples, || false), Ok(()));
        reader.join().unwrap();
    }

    #[cfg(feature = "playback")]
    #[test]
    fn device_conversion_clamps_and_maps_full_scale() {
        use super::audio::to_device;
        assert_eq!(to_device::<f32>(0.5), 0.5);
        assert_eq!(to_device::<f32>(3.0), 1.0);
        assert_eq!(to_device::<f32>(-3.0), -1.0);
        assert_eq!(to_device::<i16>(-1.0), i16::MIN);
        assert_eq!(to_device::<i16>(0.0), 0);
        assert!(to_device::<i16>(1.0) >= i16::MAX - 1);
        assert!(
            to_device::<i16>(9.0) >= i16::MAX - 1,
            "over full scale must clamp"
        );
        assert_eq!(to_device::<u16>(-1.0), 0);
        assert_eq!(to_device::<u16>(0.0), 32_768, "u16 silence is mid-scale");
        assert!(to_device::<u16>(1.0) >= u16::MAX - 1);
        assert_eq!(to_device::<u16>(-9.0), 0, "under full scale must clamp");
    }
}
