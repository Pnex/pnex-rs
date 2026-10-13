//! Video file → JPEG frames through the confined decoder (Vision Lab
//! import). Needs `ffmpeg` on PATH: generates a 10 s MP4 with `moov` at
//! the end (phone layout), then decodes one frame every 2 s from 3 s.

use std::process::Command;

#[tokio::test]
async fn mp4_with_moov_at_the_end_gives_sampled_frames() {
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        eprintln!("ffmpeg missing: skipped");
        return;
    }
    let dir = std::env::temp_dir().join(format!("pnex-vfile-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("clip.mp4");
    let ok = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i"])
        .arg("testsrc=duration=10:size=320x240:rate=10")
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-f", "mp4"])
        .arg(&path)
        .status()
        .unwrap()
        .success();
    assert!(ok, "ffmpeg could not generate the clip");
    let bytes = std::fs::read(&path).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let format = pnex_media_capture::decoder::video_file_format("video/mp4").unwrap();
    let frames = pnex_media_capture::decode_video_frames(bytes, format, 3.0, 2.0, 50)
        .await
        .expect("frames");
    // 3 s → 10 s every 2 s: 3, 5, 7, 9 (an encoder may add one at the end).
    assert!((4..=5).contains(&frames.len()), "{} frames", frames.len());
    assert_eq!((frames[0].width, frames[0].height), (320, 240));
}
