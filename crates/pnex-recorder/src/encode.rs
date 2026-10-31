//! Recording → Ogg/Opus parts for the upload: speech at 32 kbit/s is about
//! 14 MB per hour instead of 230 MB of WAV, and a long call is cut into
//! parts of at most [`PART_SECS`] so each request stays well under the
//! 100 MB body limit of the proxies in front of the server (Cloudflare).
//! The model runs on the server: the recorder only changes the transport.

use std::path::Path;

use ogg::writing::{PacketWriteEndInfo, PacketWriter};

/// Longest part: 2 h at 32 kbit/s ≈ 29 MB.
pub const PART_SECS: u32 = 2 * 3600;
const RATE: u32 = 16_000;
const BITRATE: i32 = 32_000;
/// 20 ms frames, per channel.
const FRAME: usize = (RATE / 50) as usize;
/// Ogg granule positions count 48 kHz samples whatever the input rate.
const GRANULE_PER_SAMPLE: u64 = (48_000 / RATE) as u64;

/// Encodes a 16 kHz stereo 16-bit WAV (left = me, right = the other side)
/// into Ogg/Opus parts of at most `part_secs` each.
pub fn ogg_parts(wav: &Path, part_secs: u32) -> Result<Vec<Vec<u8>>, String> {
    let mut reader = hound::WavReader::open(wav).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    if spec.sample_rate != RATE || spec.channels != 2 || spec.bits_per_sample != 16 {
        return Err(format!(
            "unexpected recording format: {} Hz, {} channels, {} bits",
            spec.sample_rate, spec.channels, spec.bits_per_sample
        ));
    }
    let per_part = part_secs as usize * RATE as usize * 2;
    let mut parts = Vec::new();
    let mut buf = Vec::with_capacity(per_part.min(1 << 24));
    for sample in reader.samples::<i16>() {
        // A recording cut by a crash ends mid-frame: keep what was read.
        let Ok(sample) = sample else { break };
        buf.push(sample);
        if buf.len() == per_part {
            parts.push(encode(&buf)?);
            buf.clear();
        }
    }
    if !buf.is_empty() || parts.is_empty() {
        parts.push(encode(&buf)?);
    }
    Ok(parts)
}

/// One Ogg/Opus stream of interleaved stereo samples.
fn encode(samples: &[i16]) -> Result<Vec<u8>, String> {
    let err = |e: opus::Error| e.to_string();
    let mut enc =
        opus::Encoder::new(RATE, opus::Channels::Stereo, opus::Application::Voip).map_err(err)?;
    enc.set_bitrate(opus::Bitrate::Bits(BITRATE)).map_err(err)?;
    let pre_skip = enc.get_lookahead().map_err(err)? as u64 * GRANULE_PER_SAMPLE;

    let serial = 1;
    let mut w = PacketWriter::new(Vec::new());
    let mut head = b"OpusHead".to_vec();
    head.push(1); // version
    head.push(2); // channels
    head.extend((pre_skip as u16).to_le_bytes());
    head.extend(RATE.to_le_bytes());
    head.extend(0i16.to_le_bytes()); // output gain
    head.push(0); // mapping family: mono/stereo
    let write = |w: &mut PacketWriter<Vec<u8>>, data: Vec<u8>, end, gp| {
        w.write_packet(data, serial, end, gp)
            .map_err(|e| e.to_string())
    };
    write(&mut w, head, PacketWriteEndInfo::EndPage, 0)?;
    let vendor = b"pnex-recorder";
    let mut tags = b"OpusTags".to_vec();
    tags.extend((vendor.len() as u32).to_le_bytes());
    tags.extend(vendor);
    tags.extend(0u32.to_le_bytes()); // no comments
    write(&mut w, tags, PacketWriteEndInfo::EndPage, 0)?;

    let frames: Vec<&[i16]> = samples.chunks(FRAME * 2).collect();
    let mut out = vec![0u8; 4000];
    let mut done: u64 = 0;
    let mut last = [0i16; FRAME * 2];
    for (i, frame) in frames.iter().enumerate() {
        // The last frame is padded with silence: Opus takes whole frames.
        let input: &[i16] = if frame.len() == FRAME * 2 {
            frame
        } else {
            last[..frame.len()].copy_from_slice(frame);
            &last
        };
        let n = enc.encode(input, &mut out).map_err(err)?;
        done += (frame.len() / 2) as u64;
        let end = if i + 1 == frames.len() {
            PacketWriteEndInfo::EndStream
        } else {
            PacketWriteEndInfo::NormalPacket
        };
        write(
            &mut w,
            out[..n].to_vec(),
            end,
            pre_skip + done * GRANULE_PER_SAMPLE,
        )?;
    }
    if frames.is_empty() {
        // An empty recording still makes a valid (silent) stream.
        let n = enc.encode(&last, &mut out).map_err(err)?;
        write(
            &mut w,
            out[..n].to_vec(),
            PacketWriteEndInfo::EndStream,
            pre_skip,
        )?;
    }
    Ok(w.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(path: &Path, secs: u32) {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for i in 0..secs * RATE {
            let t = i as f32 / RATE as f32;
            let s = ((t * 440.0 * std::f32::consts::TAU).sin() * 8000.0) as i16;
            w.write_sample(s).unwrap();
            w.write_sample(s / 2).unwrap();
        }
        w.finalize().unwrap();
    }

    #[test]
    fn a_recording_becomes_small_ogg_opus_parts() {
        let dir = std::env::temp_dir().join(format!("pnex-enc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("call.wav");
        wav(&path, 5);
        let wav_len = std::fs::metadata(&path).unwrap().len() as usize;

        let one = ogg_parts(&path, PART_SECS).unwrap();
        assert_eq!(one.len(), 1);
        assert!(one[0].starts_with(b"OggS"));
        assert!(one[0].len() * 10 < wav_len, "{} vs {wav_len}", one[0].len());

        // 5 s cut every 2 s: three parts, each a stream of its own.
        let parts = ogg_parts(&path, 2).unwrap();
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|p| p.starts_with(b"OggS")));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
