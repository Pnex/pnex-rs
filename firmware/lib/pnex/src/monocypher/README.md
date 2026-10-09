# Monocypher 4.0.2 (vendored)

Upstream: https://github.com/LoupVaillant/Monocypher, tag `4.0.2`
(archive sha256 `bc1ca30b1b2654e4e7daf2492c0d204200e55137f23fda6b7142fd7d523bd6b4`).
Licence: BSD-2-Clause or CC0-1.0 (`LICENCE.md`), unmodified sources.

Used by the Noise link (`pnex_noise.cpp`, D156) for X25519 and
ChaCha20-Poly1305 (RFC 8439). Audited by Cure53 (2017). Pure C, no
platform dependency: the same code runs on every chip and in the host tests
(`firmware/core-cpp-tests`).

`monocypher-ed25519.{c,h}` come from `src/optional/` of the same tag
(unmodified): Ed25519 with SHA-512 (RFC 8032), used to verify the
signature of OTA images (SEC-18, `pnex_ota.cpp`) against the server's
`ed25519-dalek` signatures.
