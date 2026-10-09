# syntax=docker/dockerfile:1.7
#
# PNeX images (D71) — built from the repo root:
#   docker build -f deploy/docker/Dockerfile --target server  -t pnex-server  .
#   docker build -f deploy/docker/Dockerfile --target builder -t pnex-builder .
# (or `task docker:build`; multi-arch amd64+arm64 push: `task docker:publish`).
#
#   server   Chainguard glibc-dynamic: no shell, no package manager, non-root.
#            pnex-server (HTTP + websockets, `start` = server only) + the
#            flow runtime it supervises + the web front.
#   builder  Chainguard Wolfi (glibc): the same binary as a queue worker
#            (`start --worker`) — firmware builds need PlatformIO/esptool
#            (Python + toolchains, pre-installed), panorama stitching needs
#            the ONNX models.
#
# Build stages use Debian trixie (not shipped): the prebuilt dx needs glibc
# >= 2.39. Binaries link against trixie's glibc 2.41; the Chainguard images
# ship 2.44 (forward compatible).

ARG RUST_VERSION=1.96
ARG BUN_VERSION=1.4
ARG DIOXUS_CLI_VERSION=0.7.10
ARG PLATFORMIO_VERSION=6.2.0
ARG ESPTOOL_VERSION=5.4.0

# ── Front assets: Tailwind CSS + esbuild bundles ──────────────────────────
# Arch-independent stages run on the build host ($BUILDPLATFORM): a
# multi-arch build (linux/amd64 + linux/arm64) shares them instead of
# emulating bun/rustc under QEMU.
FROM --platform=$BUILDPLATFORM oven/bun:${BUN_VERSION}-slim AS assets
WORKDIR /src/crates/pnex-frontend
COPY crates/pnex-frontend/package.json crates/pnex-frontend/bun.lock ./
RUN bun install --frozen-lockfile
COPY crates/pnex-frontend/ ./
RUN bun run css:build && bun run js:build && bun run js:viewers && bun run js:map

# ── Rust toolchain on the build host, with aarch64 cross compilers ────────
FROM --platform=$BUILDPLATFORM rust:${RUST_VERSION}-trixie AS toolchain-base
ARG DIOXUS_CLI_VERSION
RUN apt-get update \
 && apt-get install -y --no-install-recommends cmake clang libclang-dev \
      gcc-aarch64-linux-gnu g++-aarch64-linux-gnu \
      libc6-dev-arm64-cross linux-libc-dev-arm64-cross \
 && rm -rf /var/lib/apt/lists/*
RUN rustup target add wasm32-unknown-unknown aarch64-unknown-linux-gnu
RUN curl -L --proto '=https' --tlsv1.2 -sSf \
      https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh | bash \
 && cargo binstall -y --locked "dioxus-cli@${DIOXUS_CLI_VERSION}"
# aarch64 cross settings (same as the CI aarch64 job). The build host is
# assumed amd64: the amd64 target builds natively, arm64 is cross-compiled
# (build scripts and proc macros stay host builds).
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++ \
    AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar
WORKDIR /src

# Sources on top of the tool layers: everything above stays cached across
# source changes (shared with agent-dist).
FROM toolchain-base AS toolchain
COPY . .
COPY --from=assets /src/crates/pnex-frontend/assets/ crates/pnex-frontend/assets/
# Content hash of firmware/: pnex-firmware-builder's build.rs reruns on it
# (rerun-if-env-changed). Its mtime-based rerun-if-changed alone let the
# target/ cache mount keep a stale firmware embed across images.
RUN cd firmware && find . -type f -print0 | LC_ALL=C sort -z | xargs -0 sha256sum \
      | sha256sum | cut -c1-16 > /firmware-tree-hash

# ── Wasm front (dx): built once, shared by every target platform ──────────
FROM toolchain AS web
# Short commit sha shown next to the semver ("0.1.0 (abc1234)"), read at
# compile time by option_env!("BUILD_SHA"). Declared here, not in the
# toolchain stage: a new value only invalidates the build RUN, never the
# toolchain layers.
ARG BUILD_SHA=dev
ENV BUILD_SHA=${BUILD_SHA}
# One cargo registry/git cache per stage: web and every TARGETARCH build run
# in parallel, and a shared mount races on crate unpacking ("File exists").
# dx downloads its tools (wasm-opt…) from GitHub into its data dir on first
# use: cached, so a flaky download cannot fail every image build.
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=pnex-cargo-registry-web \
    --mount=type=cache,target=/usr/local/cargo/git,id=pnex-cargo-git-web \
    --mount=type=cache,target=/src/target,id=pnex-target-web \
    --mount=type=cache,target=/root/.local/share/dioxus,id=pnex-dx-tools \
    set -eux; \
    rm -rf target/dx/pnex-frontend/release/web/public; \
    deploy/docker/touch-changed.sh target/.src-manifest; \
    (cd crates/pnex-frontend && dx build --platform web --release); \
    mv target/.src-manifest.new target/.src-manifest; \
    cp -r target/dx/pnex-frontend/release/web/public /dist

# ── Native binaries, cross-compiled for $TARGETPLATFORM ───────────────────
FROM toolchain AS build
ARG TARGETARCH
ARG BUILD_SHA=dev
ENV BUILD_SHA=${BUILD_SHA}
# target/ lives in a cache mount (incremental rebuilds), so artefacts are
# copied out to /out within the same RUN. touch-changed.sh makes freshness
# content-based: cargo's mtime check alone shipped stale binaries (O33).
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=pnex-cargo-registry-${TARGETARCH} \
    --mount=type=cache,target=/usr/local/cargo/git,id=pnex-cargo-git-${TARGETARCH} \
    --mount=type=cache,target=/src/target,id=pnex-target-${TARGETARCH} \
    set -eux; \
    case "$TARGETARCH" in \
      amd64) triple=x86_64-unknown-linux-gnu ;; \
      arm64) triple=aarch64-unknown-linux-gnu; \
             export BINDGEN_EXTRA_CLANG_ARGS="--sysroot=/usr/aarch64-linux-gnu -I/usr/aarch64-linux-gnu/include" ;; \
      *) echo "unsupported TARGETARCH=$TARGETARCH" >&2; exit 1 ;; \
    esac; \
    export PNEX_FIRMWARE_TREE_HASH="$(cat /firmware-tree-hash)"; \
    deploy/docker/touch-changed.sh target/.src-manifest; \
    cargo build --release --locked --target "$triple" \
      -p pnex-backend -p pnex-flow-runtime --bin pnex-server --bin pnex-flow-runtime; \
    mv target/.src-manifest.new target/.src-manifest; \
    mkdir -p /out/bin /out/data/flow-state /out/data/media; \
    cp "target/$triple/release/pnex-server" "target/$triple/release/pnex-flow-runtime" /out/bin/
COPY --from=web /dist /out/dist

# ── Edge agent distribution (D95): built once on the build host ──────────
# Static musl Linux (x86_64/aarch64/armv7, linked through zig) + Windows
# x86_64 (mingw) + SHA256SUMS, served by pnex-server at
# /api/v1/agent/download/{target}.
# Its tools are installed before the sources are copied, so a source change
# no longer re-runs the mingw apt-get (≈ 10 GB of build cache per rebuild).
FROM toolchain-base AS agent-dist
RUN apt-get update \
 && apt-get install -y --no-install-recommends gcc-mingw-w64-x86-64 xz-utils \
 && rm -rf /var/lib/apt/lists/*
RUN cargo binstall -y --locked cargo-zigbuild
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=pnex-cargo-registry-agent \
    --mount=type=cache,target=/usr/local/cargo/git,id=pnex-cargo-git-agent \
    --mount=type=cache,target=/src/target,id=pnex-target-agent \
    deploy/docker/touch-changed.sh target/.src-manifest \
 && deploy/agent/build-dist.sh /agent-dist \
 && mv target/.src-manifest.new target/.src-manifest

# ── server ────────────────────────────────────────────────────────────────
FROM cgr.dev/chainguard/glibc-dynamic:latest AS server
COPY --from=build /out/bin/ /usr/local/bin/
COPY --from=build /out/dist/ /app/pnex-frontend/dist/
COPY --from=agent-dist /agent-dist/ /opt/pnex/agent-dist/
COPY crates/pnex-backend/config/ /app/pnex-backend/config/
# Catalog fixtures: seeded idempotently at API boot (PNEX_AUTO_SEED).
COPY crates/pnex-backend/fixtures/ /app/pnex-backend/fixtures/
# Named volumes inherit this ownership (nonroot = 65532).
COPY --from=build --chown=65532:65532 /out/data/ /data/
# Loco resolves config/ and the static folder (../pnex-frontend/dist)
# relative to the working directory — same layout as the repo.
WORKDIR /app/pnex-backend
ENV PATH=/usr/local/bin:/usr/bin:/bin \
    LOCO_ENV=production \
    PNEX_FLOW_RUNTIME_CMD=pnex-flow-runtime \
    PNEX_AGENT_DIST_DIR=/opt/pnex/agent-dist \
    PNEX_FLOW_STATE_DIR=/data/flow-state \
    PNEX_MEDIA_DIR=/data/media
USER 65532:65532
EXPOSE 5150
VOLUME ["/data"]
ENTRYPOINT ["pnex-server"]
CMD ["start"]

# ── builder (queue worker) ────────────────────────────────────────────────
FROM cgr.dev/chainguard/wolfi-base:latest AS builder
ARG PLATFORMIO_VERSION
ARG ESPTOOL_VERSION
RUN apk add --no-cache python-3.14 py3.14-pip git libstdc++
RUN python3 -m venv /opt/pio \
 && /opt/pio/bin/pip install --no-cache-dir --upgrade pip \
 && /opt/pio/bin/pip install --no-cache-dir \
      "platformio==${PLATFORMIO_VERSION}" "esptool==${ESPTOOL_VERSION}"
# Two PlatformIO cores: the pioarduino platform (Arduino core 3.x, ESP32-C6)
# and the official espressif32 install `framework-arduinoespressif32` and
# `tool-esptoolpy` under the same names and overwrite each other in a shared
# core — every switch between them downloaded them again. The worker picks
# the core from the project's platform (PNEX_PIO_CORE_DIR_PIOARDUINO).
ENV PATH=/opt/pio/bin:/usr/local/bin:/usr/bin:/bin \
    PLATFORMIO_CORE_DIR=/opt/platformio \
    PNEX_PIO_CORE_DIR_PIOARDUINO=/opt/platformio-pioarduino \
    PLATFORMIO_NO_ANSI=true \
    HOME=/home/pnex
# Bake platforms + toolchains + frameworks for every supported SoC family,
# then every library a worker build may use (seed_libdeps.sh): firmware
# builds run without network (air-gapped sites).
# Toolchains first, in their own layer: a firmware change does not redo them.
COPY deploy/docker/prewarm/platformio.ini /tmp/prewarm/
COPY deploy/docker/prewarm/src/ /tmp/prewarm/src/
# Owned by the worker uid in this same layer: a later `chown -R` would copy
# every toolchain file into a new layer (image size doubled).
RUN pio run -d /tmp/prewarm -e esp8266 -e esp32 -e esp32c3 -e esp32s3 \
 && PLATFORMIO_CORE_DIR=$PNEX_PIO_CORE_DIR_PIOARDUINO pio run -d /tmp/prewarm -e esp32c6 \
 && rm -rf /tmp/prewarm /opt/platformio/.cache $PNEX_PIO_CORE_DIR_PIOARDUINO/.cache \
 && find /opt/platformio/packages $PNEX_PIO_CORE_DIR_PIOARDUINO/packages -type f \
      \( -name '*.pem' -o -name '*.key' \) \
      -exec grep -l 'PRIVATE KEY-----' {} + | xargs -r rm -f \
 && chown -R 65532:65532 /opt/platformio $PNEX_PIO_CORE_DIR_PIOARDUINO
COPY deploy/docker/prewarm/seed_libdeps.sh deploy/docker/prewarm/lib_catalog.txt /tmp/prewarm/
COPY firmware/ /tmp/fw/
# SEC-15: once the prewarm has created pioarduino's penv, drop what only
# installs packages and ships vulnerable code (pip vendors old urllib3,
# msgpack, setuptools; penv resolves with uv) and lift penv's starlette past
# pioarduino's `<1` pin (PIO Home only, never started in the worker).
RUN sh /tmp/prewarm/seed_libdeps.sh /tmp/fw \
      generic_esp8266:nodemcuv2 generic_esp32:esp32dev \
      generic_esp32c3:esp32-c3-devkitm-1 generic_esp32s3:esp32-s3-devkitc-1 \
      generic_esp32c6:esp32-c6-devkitc-1 generic_esp32cam:esp32cam \
 && penv=$PNEX_PIO_CORE_DIR_PIOARDUINO/penv \
 && $penv/bin/uv pip install --no-cache --python $penv/bin/python --no-deps \
      "starlette>=1.3.1" \
 && $penv/bin/python -m pip uninstall -y pip \
 && /opt/pio/bin/python -m pip uninstall -y pip \
 && apk del --no-cache py3.14-pip \
 && rm -rf /tmp/prewarm /tmp/fw /opt/platformio/.cache \
      $PNEX_PIO_CORE_DIR_PIOARDUINO/.cache \
 && mkdir -p /home/pnex \
 && chown 65532:65532 /home/pnex \
 && find /opt/platformio $PNEX_PIO_CORE_DIR_PIOARDUINO ! -user 65532 \
      -exec chown -h 65532:65532 {} +
COPY --from=build /out/bin/pnex-server /usr/local/bin/
COPY crates/pnex-backend/config/ /app/pnex-backend/config/
COPY deploy/models/ /app/models/
COPY --from=build --chown=65532:65532 /out/data/ /data/
WORKDIR /app/pnex-backend
ENV LOCO_ENV=production \
    PNEX_MEDIA_DIR=/data/media \
    PNEX_STITCH_MODELS_DIR=/app/models
USER 65532:65532
VOLUME ["/data"]
ENTRYPOINT ["pnex-server"]
CMD ["start", "--worker"]
