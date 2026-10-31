//! Call capture (call-recording.md R1.1): the microphone and what the
//! computer plays, as one stereo WAV on disk (left = this user, right = the
//! other party). The file is the source of truth until the server has it.
//!
//! Linux: two PulseAudio streams (PipeWire through pipewire-pulse) on the
//! same server clock, so the channels stay aligned without resampling.
//! Windows: WASAPI through cpal, the output captured in loopback, both
//! streams downmixed and resampled to 16 kHz.
//! macOS: not available yet.

use std::path::{Path, PathBuf};
use std::time::Instant;

/// Capture rate: what the transcription expects, so the server does not
/// resample.
pub const RATE: u32 = 16_000;

/// Peak of the last 100 ms per channel (0 = microphone, 1 = what the
/// computer plays), as absolute 16-bit sample values: the window's meters.
pub type Levels = std::sync::Arc<[std::sync::atomic::AtomicU16; 2]>;

/// Stores the peak of each channel of interleaved stereo `frames`.
pub fn store_peaks(levels: &[std::sync::atomic::AtomicU16; 2], frames: &[i16]) {
    let mut peak = [0u16; 2];
    for (i, s) in frames.iter().enumerate() {
        peak[i % 2] = peak[i % 2].max(s.unsigned_abs());
    }
    for (level, p) in levels.iter().zip(peak) {
        level.store(p, std::sync::atomic::Ordering::Relaxed);
    }
}

pub struct Recording {
    pub path: PathBuf,
    pub levels: Levels,
    /// Call app that triggered it, `None` when started by hand.
    pub app: Option<String>,
    pub started: Instant,
    #[cfg(any(target_os = "linux", windows))]
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    #[cfg(any(target_os = "linux", windows))]
    thread: std::thread::JoinHandle<Result<(), String>>,
}

impl Recording {
    /// Stops the capture and closes the file.
    pub fn finish(self) -> Result<PathBuf, String> {
        #[cfg(any(target_os = "linux", windows))]
        {
            self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
            self.thread
                .join()
                .map_err(|_| "capture thread panicked".to_string())??;
        }
        Ok(self.path)
    }

    /// Stops the capture and deletes the file: nothing leaves the computer.
    pub fn discard(self) {
        let path = self.path.clone();
        let _ = self.finish();
        let _ = std::fs::remove_file(path);
    }
}

pub fn supported() -> bool {
    cfg!(any(target_os = "linux", windows))
}

/// Linear resampler to [`RATE`], with a one-pole low-pass before a
/// downsampling (speech band kept, aliasing damped).
// shortcut: linear interpolation, enough for speech recognition; use a
// polyphase resampler (rubato) if music or wideband audio matters.
#[cfg(any(windows, test))]
pub struct Resampler {
    step: f64,
    t: f64,
    prev: f32,
    alpha: f32,
    lp: f32,
}

#[cfg(any(windows, test))]
impl Resampler {
    pub fn new(rate: u32) -> Self {
        let step = f64::from(rate) / f64::from(RATE);
        // 7 kHz cutoff when downsampling, none otherwise.
        let alpha = if rate > RATE {
            1.0 - (-2.0 * std::f32::consts::PI * 7_000.0 / rate as f32).exp()
        } else {
            1.0
        };
        Self {
            step,
            t: 1.0,
            prev: 0.0,
            alpha,
            lp: 0.0,
        }
    }

    /// Appends the 16 kHz samples of `input` (mono, -1..1) to `out`.
    pub fn push(&mut self, input: &[f32], out: &mut Vec<i16>) {
        for &x in input {
            self.lp += self.alpha * (x - self.lp);
            let x = self.lp;
            while self.t <= 1.0 {
                let y = self.prev + (x - self.prev) * self.t as f32;
                out.push((y.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16);
                self.t += self.step;
            }
            self.t -= 1.0;
            self.prev = x;
        }
    }
}

/// Interleaves a block of each channel into stereo samples.
#[cfg(any(target_os = "linux", test))]
pub fn interleave(mic: &[u8], out: &[u8]) -> Vec<i16> {
    mic.as_chunks::<2>()
        .0
        .iter()
        .zip(out.as_chunks::<2>().0)
        .flat_map(|(m, o)| [i16::from_le_bytes(*m), i16::from_le_bytes(*o)])
        .collect()
}

/// Starts recording into `path`; both devices are open when it returns.
/// `PNEX_RECORDER_MIC` / `PNEX_RECORDER_OUTPUT` pick other devices than the
/// defaults (a PulseAudio source name; the output is a monitor source).
#[cfg(target_os = "linux")]
pub fn start(path: &Path, app: Option<String>) -> Result<Recording, String> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use libpulse_binding::sample::{Format, Spec};
    use libpulse_binding::stream::Direction;
    use libpulse_simple_binding::Simple;

    let mic = std::env::var("PNEX_RECORDER_MIC").unwrap_or_else(|_| "@DEFAULT_SOURCE@".into());
    let out = std::env::var("PNEX_RECORDER_OUTPUT").unwrap_or_else(|_| "@DEFAULT_MONITOR@".into());
    let spec = Spec {
        format: Format::S16le,
        channels: 1,
        rate: RATE,
    };
    let stop = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let file = path.to_path_buf();
    let flag = stop.clone();
    let levels = Levels::default();
    let meters = levels.clone();
    let thread = std::thread::spawn(move || {
        let open = |dev: &str, what: &str| {
            Simple::new(
                None,
                "pnex-recorder",
                Direction::Record,
                Some(dev),
                what,
                &spec,
                None,
                None,
            )
            .map_err(|e| format!("{dev}: {e}"))
        };
        let streams = open(&mic, "microphone").and_then(|m| Ok((m, open(&out, "output")?)));
        let wav = hound::WavSpec {
            channels: 2,
            sample_rate: RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let opened = streams.and_then(|s| {
            hound::WavWriter::create(&file, wav)
                .map(|w| (s, w))
                .map_err(|e| e.to_string())
        });
        let ((mic, out), mut writer) = match opened {
            Ok(v) => {
                let _ = ready_tx.send(Ok(()));
                v
            }
            Err(e) => {
                let _ = ready_tx.send(Err(e.clone()));
                return Err(e);
            }
        };
        // 100 ms of 16-bit mono per read.
        let mut mic_buf = vec![0u8; RATE as usize / 10 * 2];
        let mut out_buf = mic_buf.clone();
        let mut blocks = 0u64;
        while !flag.load(Ordering::Relaxed) {
            mic.read(&mut mic_buf).map_err(|e| format!("{e}"))?;
            out.read(&mut out_buf).map_err(|e| format!("{e}"))?;
            let frames = interleave(&mic_buf, &out_buf);
            store_peaks(&meters, &frames);
            for s in frames {
                writer.write_sample(s).map_err(|e| e.to_string())?;
            }
            blocks += 1;
            // A header rewritten every second keeps the file readable after a crash.
            if blocks.is_multiple_of(10) {
                writer.flush().map_err(|e| e.to_string())?;
            }
        }
        writer.finalize().map_err(|e| e.to_string())
    });
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(Recording {
            path: path.to_path_buf(),
            levels,
            app,
            started: Instant::now(),
            stop,
            thread,
        }),
        Ok(Err(e)) => {
            let _ = thread.join();
            let _ = std::fs::remove_file(path);
            Err(e)
        }
        Err(_) => Err("capture thread ended before opening the devices".into()),
    }
}

/// Starts recording into `path`; both devices are open when it returns.
#[cfg(windows)]
pub fn start(path: &Path, app: Option<String>) -> Result<Recording, String> {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    type Queue = Arc<Mutex<VecDeque<i16>>>;

    /// An input stream on `device` (an output device = WASAPI loopback),
    /// downmixed and resampled into `queue`.
    fn open(
        device: &cpal::Device,
        config: cpal::SupportedStreamConfig,
        queue: Queue,
    ) -> Result<cpal::Stream, String> {
        let channels = usize::from(config.channels()).max(1);
        let format = config.sample_format();
        let mut resampler = Resampler::new(config.sample_rate().0);
        let mut mono = Vec::new();
        let mut out = Vec::new();
        let config: cpal::StreamConfig = config.into();
        let mut feed = move |frames: &mut dyn Iterator<Item = f32>| {
            mono.clear();
            let mut acc = 0.0;
            for (i, s) in frames.enumerate() {
                acc += s;
                if i % channels == channels - 1 {
                    mono.push(acc / channels as f32);
                    acc = 0.0;
                }
            }
            out.clear();
            resampler.push(&mono, &mut out);
            queue.lock().expect("capture queue").extend(out.iter());
        };
        let err = |e| eprintln!("capture stream error: {e}");
        let stream = match format {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &config,
                move |d: &[f32], _: &_| feed(&mut d.iter().copied()),
                err,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                &config,
                move |d: &[i16], _: &_| {
                    feed(&mut d.iter().map(|s| f32::from(*s) / f32::from(i16::MAX)))
                },
                err,
                None,
            ),
            other => return Err(format!("unsupported sample format {other}")),
        };
        stream.map_err(|e| e.to_string())
    }

    let stop = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let file = path.to_path_buf();
    let flag = stop.clone();
    let levels = Levels::default();
    let meters = levels.clone();
    // WASAPI streams are not Send: they live and die in this thread.
    let thread = std::thread::spawn(move || {
        let mic_q: Queue = Arc::default();
        let out_q: Queue = Arc::default();
        let opened = (|| {
            let host = cpal::default_host();
            let mic = host.default_input_device().ok_or("no microphone")?;
            let out = host.default_output_device().ok_or("no audio output")?;
            let mic_cfg = mic.default_input_config().map_err(|e| e.to_string())?;
            let out_cfg = out.default_output_config().map_err(|e| e.to_string())?;
            let a = open(&mic, mic_cfg, mic_q.clone())?;
            let b = open(&out, out_cfg, out_q.clone())?;
            a.play().map_err(|e| e.to_string())?;
            b.play().map_err(|e| e.to_string())?;
            let spec = hound::WavSpec {
                channels: 2,
                sample_rate: RATE,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let writer = hound::WavWriter::create(&file, spec).map_err(|e| e.to_string())?;
            Ok::<_, String>((a, b, writer))
        })();
        let (_mic, _out, mut writer) = match opened {
            Ok(v) => {
                let _ = ready_tx.send(Ok(()));
                v
            }
            Err(e) => {
                let _ = ready_tx.send(Err(e.clone()));
                return Err(e);
            }
        };
        let mut ticks = 0u64;
        while !flag.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let mic: Vec<i16> = mic_q.lock().expect("capture queue").drain(..).collect();
            let other = {
                let mut q = out_q.lock().expect("capture queue");
                // Loopback runs ahead of the mic (drift): keep at most 1 s.
                let excess = q.len().saturating_sub(RATE as usize);
                q.drain(..excess);
                let n = mic.len().min(q.len());
                q.drain(..n).collect::<Vec<i16>>()
            };
            // Loopback delivers nothing while nothing plays: silence.
            let frames: Vec<i16> = mic
                .iter()
                .enumerate()
                .flat_map(|(i, m)| [*m, other.get(i).copied().unwrap_or(0)])
                .collect();
            store_peaks(&meters, &frames);
            for s in frames {
                writer.write_sample(s).map_err(|e| e.to_string())?;
            }
            ticks += 1;
            if ticks.is_multiple_of(10) {
                writer.flush().map_err(|e| e.to_string())?;
            }
        }
        writer.finalize().map_err(|e| e.to_string())
    });
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(Recording {
            path: path.to_path_buf(),
            levels,
            app,
            started: Instant::now(),
            stop,
            thread,
        }),
        Ok(Err(e)) => {
            let _ = thread.join();
            let _ = std::fs::remove_file(path);
            Err(e)
        }
        Err(_) => Err("capture thread ended before opening the devices".into()),
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
pub fn start(_path: &Path, _app: Option<String>) -> Result<Recording, String> {
    Err("call capture is not available on this system yet".into())
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_resampler_keeps_16k_and_divides_48k_by_three() {
        let mut out = Vec::new();
        super::Resampler::new(16_000).push(&[0.5; 1600], &mut out);
        assert_eq!(out.len(), 1600);
        assert!(
            out.iter().all(|s| (*s - 16_383).abs() <= 1),
            "{:?}",
            &out[..4]
        );
        let mut out = Vec::new();
        let mut r = super::Resampler::new(48_000);
        r.push(&[0.5; 4800], &mut out);
        r.push(&[0.5; 4800], &mut out);
        assert!((3199..=3201).contains(&out.len()), "{}", out.len());
        // The low-pass settles on the level after a few samples.
        assert!((out[3000] - 16_383).abs() <= 2, "{}", out[3000]);
        let mut out = Vec::new();
        super::Resampler::new(44_100).push(&[0.0; 44_100], &mut out);
        assert!((15_999..=16_001).contains(&out.len()), "{}", out.len());
    }

    #[test]
    fn peaks_are_per_channel() {
        let levels = super::Levels::default();
        super::store_peaks(&levels, &[100, -3000, -200, 50]);
        let get = |i: usize| levels[i].load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!((get(0), get(1)), (200, 3000));
    }

    #[test]
    fn interleave_puts_the_microphone_left() {
        let mic = [1i16, 2].map(i16::to_le_bytes).concat();
        let out = [-1i16, -2].map(i16::to_le_bytes).concat();
        assert_eq!(super::interleave(&mic, &out), [1, -1, 2, -2]);
    }
}
