//
// PXC1 camera frame header (camera-video.md D73) — header-only, no Arduino
// dependency: included by pnex_camera.cpp (firmware) AND by the host test
// core-cpp-tests (golden vector replayed, same bytes as the Rust
// `header_golden_bytes` test in crates/pnex-core/src/camera.rs).
//
// Layout (16 bytes, little-endian), prefixed to the JPEG inside the
// ChaCha20 ciphertext:
//   0..4   magic "PXC1"
//   4..8   seq        u32
//   8..12  uptime_ms  u32
//   12..14 width      u16
//   14..16 height     u16
//
#ifndef PNEX_CAMERA_FRAME_H
#define PNEX_CAMERA_FRAME_H

#include <cstddef>
#include <cstdint>

namespace pnex_camera_frame {

constexpr size_t HEADER_LEN = 16;

inline void put_u32_le(uint8_t* p, uint32_t v) {
    p[0] = (uint8_t)(v & 0xff);
    p[1] = (uint8_t)((v >> 8) & 0xff);
    p[2] = (uint8_t)((v >> 16) & 0xff);
    p[3] = (uint8_t)((v >> 24) & 0xff);
}

inline void put_u16_le(uint8_t* p, uint16_t v) {
    p[0] = (uint8_t)(v & 0xff);
    p[1] = (uint8_t)((v >> 8) & 0xff);
}

/// Writes the 16-byte clear header into `out`.
inline void encode_header(uint8_t out[HEADER_LEN], uint32_t seq, uint32_t uptime_ms,
                          uint16_t width, uint16_t height) {
    out[0] = 'P';
    out[1] = 'X';
    out[2] = 'C';
    out[3] = '1';
    put_u32_le(out + 4, seq);
    put_u32_le(out + 8, uptime_ms);
    put_u16_le(out + 12, width);
    put_u16_le(out + 14, height);
}

}  // namespace pnex_camera_frame

#endif  // PNEX_CAMERA_FRAME_H
