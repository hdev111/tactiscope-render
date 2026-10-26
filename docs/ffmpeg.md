# FFmpeg and licensing

`tactiscope-render` uses FFmpeg as a separate process for video/image decode and
H.264/AAC encode. FFmpeg is not linked into the Rust library.

## Resolution order

The CLI resolves FFmpeg in this order:

1. `--ffmpeg /path/to/ffmpeg`
2. `FFMPEG_PATH`
3. `ffmpeg` next to the CLI executable (release archives)
4. `ffmpeg` on `PATH`

The Rust API takes `RendererOptions::ffmpeg_path` directly.

## Bundled release archives

Release archives bundle a pinned static build:

| Field | Value |
| --- | --- |
| FFmpeg | 6.1.1 |
| Provider | eugeneware/ffmpeg-static |
| Release | b6.1.1 |
| License | GPL-3.0-or-later |

The archive contains `ffmpeg`, `ffprobe`, `FFMPEG_LICENSE` and a `NOTICE` with
the exact provider/source information. The renderer source remains MIT.

If you redistribute a release archive, you are responsible for GPL compliance
for the FFmpeg binaries, including source availability and notices. See
[`NOTICE`](../NOTICE) and [docs/static-ffmpeg-licensing.md](static-ffmpeg-licensing.md).

## External FFmpeg

If you do not want to redistribute GPL binaries:

- build/install FFmpeg yourself,
- pass `--ffmpeg` or set `FFMPEG_PATH`,
- do not ship the bundled `ffmpeg`/`ffprobe` files.

The renderer code is MIT either way.

## Build requirements

Use an FFmpeg build with `libx264` and AAC support for MP4 export. The pinned
release assets are static builds with the required codecs.

## Audio

Timeline audio is mixed/mastered to 48 kHz stereo WAV, then encoded as AAC. If
there are no audio elements, export passes `-an` and produces a video-only MP4.

## Static linking and platform notes

- Linux release assets are static; `scripts/package-release.ts` verifies
  checksums and executable bits.
- macOS/Windows release assets are validated by their checksums and are built by
  the release workflow on native runners.
- FFmpeg binary redistribution is separate from the MIT renderer license.
