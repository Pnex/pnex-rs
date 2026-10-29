//! Microphone capture: a fixed-length mono clip encoded as a WAV file.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

#[derive(Debug)]
pub enum CaptureError {
    NoMicrophone,
    Device(String),
}

/// Records `secs` seconds from the default input device; blocking, so call it
/// from `spawn_blocking`. The server resamples, the clip keeps the device rate.
pub fn record_wav(secs: u64) -> Result<Vec<u8>, CaptureError> {
    let device = cpal::default_host()
        .default_input_device()
        .ok_or(CaptureError::NoMicrophone)?;
    let supported = device
        .default_input_config()
        .map_err(|e| CaptureError::Device(e.to_string()))?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let channels = config.channels as usize;
    let rate = config.sample_rate.0;
    let samples = Arc::new(Mutex::new(Vec::<i16>::with_capacity(
        rate as usize * secs as usize,
    )));

    let stream = match format {
        SampleFormat::F32 => build::<f32>(&device, &config, channels, samples.clone()),
        SampleFormat::I16 => build::<i16>(&device, &config, channels, samples.clone()),
        SampleFormat::U16 => build::<u16>(&device, &config, channels, samples.clone()),
        SampleFormat::I32 => build::<i32>(&device, &config, channels, samples.clone()),
        other => {
            return Err(CaptureError::Device(format!(
                "unsupported sample format {other}"
            )))
        }
    }?;
    stream
        .play()
        .map_err(|e| CaptureError::Device(e.to_string()))?;
    std::thread::sleep(Duration::from_secs(secs));
    drop(stream);

    let samples = std::mem::take(&mut *samples.lock().expect("capture buffer"));
    Ok(encode_wav(&samples, rate))
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    out: Arc<Mutex<Vec<i16>>>,
) -> Result<cpal::Stream, CaptureError>
where
    T: SizedSample,
    i16: FromSample<T>,
{
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                let mut out = out.lock().expect("capture buffer");
                // Keep the first channel: speech-to-text is mono.
                out.extend(data.iter().step_by(channels).map(|s| s.to_sample::<i16>()));
            },
            |e| eprintln!("microphone stream error: {e}"),
            None,
        )
        .map_err(|e| CaptureError::Device(e.to_string()))
}

/// 16-bit PCM mono WAV.
pub fn encode_wav(samples: &[i16], rate: u32) -> Vec<u8> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut buf = std::io::Cursor::new(Vec::new());
    let mut writer = hound::WavWriter::new(&mut buf, spec).expect("wav header");
    for s in samples {
        writer.write_sample(*s).expect("in-memory write");
    }
    writer.finalize().expect("in-memory write");
    buf.into_inner()
}

#[cfg(test)]
mod tests {
    #[test]
    fn wav_round_trips() {
        let wav = super::encode_wav(&[0, 1000, -1000], 48_000);
        let mut reader = hound::WavReader::new(std::io::Cursor::new(wav)).unwrap();
        assert_eq!(reader.spec().sample_rate, 48_000);
        let back: Vec<i16> = reader.samples::<i16>().map(Result::unwrap).collect();
        assert_eq!(back, [0, 1000, -1000]);
    }
}
