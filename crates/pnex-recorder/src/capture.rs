//! Call capture (call-recording.md R1.1): the microphone and what the
//! computer plays, as one stereo WAV on disk (left = this user, right = the
//! other party). The file is the source of truth until the server has it.
//!
//! Linux: two PulseAudio streams (PipeWire through pipewire-pulse) on the
//! same server clock, so the channels stay aligned without resampling.
//! Other systems: not available yet.

use std::path::{Path, PathBuf};
use std::time::Instant;

/// Capture rate: what the transcription expects, so the server does not
/// resample.
pub const RATE: u32 = 16_000;

pub struct Recording {
    pub path: PathBuf,
    /// Call app that triggered it, `None` when started by hand.
    pub app: Option<String>,
    pub started: Instant,
    #[cfg(target_os = "linux")]
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    #[cfg(target_os = "linux")]
    thread: std::thread::JoinHandle<Result<(), String>>,
}

impl Recording {
    /// Stops the capture and closes the file.
    pub fn finish(self) -> Result<PathBuf, String> {
        #[cfg(target_os = "linux")]
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
    cfg!(target_os = "linux")
}

/// Interleaves a block of each channel into stereo samples.
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
            for s in interleave(&mic_buf, &out_buf) {
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

#[cfg(not(target_os = "linux"))]
pub fn start(_path: &Path, _app: Option<String>) -> Result<Recording, String> {
    Err("call capture is not available on this system yet".into())
}

#[cfg(test)]
mod tests {
    #[test]
    fn interleave_puts_the_microphone_left() {
        let mic = [1i16, 2].map(i16::to_le_bytes).concat();
        let out = [-1i16, -2].map(i16::to_le_bytes).concat();
        assert_eq!(super::interleave(&mic, &out), [1, -1, 2, -2]);
    }
}
