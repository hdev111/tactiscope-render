# Release packaging

Releases bundle the CLI and a pinned static FFmpeg build.

## Build inputs

- `crates/render-cli` release binary for the target.
- `ffmpeg.lock.json` pinned FFmpeg assets (URLs, SHA-256, sizes).
- `LICENSE`, `NOTICE`, `README.md`, `schema/`, `docs/`.

## Local packaging

```bash
cargo build --release -p tactiscope-render-cli
bun scripts/package-release.ts \
  --target linux-x64 \
  --binary target/release/tactiscope-render \
  --output dist
```

If you already have FFmpeg binaries:

```bash
bun scripts/package-release.ts \
  --target linux-x64 \
  --binary target/release/tactiscope-render \
  --ffmpeg-dir /path/to/ffmpeg-bin \
  --output dist
```

The script downloads and verifies the pinned FFmpeg assets when
`--ffmpeg-dir` is omitted. It creates a target directory and a `.tar.gz` (or
`.zip` on Windows) plus a `SHA256SUMS` file.

## Release workflow

`.github/workflows/release.yml` runs on `v*` tags and builds:

- Linux x64
- macOS x64
- macOS arm64
- Windows x64

Each job builds the CLI, packages FFmpeg, uploads the archive and publishes a
GitHub release. The archives are unsigned; signing is a downstream concern.

## GPL obligations

The release archives include GPL-3.0-or-later FFmpeg binaries. The archive
contains `FFMPEG_LICENSE` and `NOTICE` with the exact provider/source. If you
redistribute an archive, ensure you meet the GPL source/notice obligations for
the FFmpeg build. The Rust renderer is MIT.

If you do not want to distribute GPL binaries, publish a source-only release or
require an external FFmpeg; do not include `ffmpeg`/`ffprobe` in the archive.
