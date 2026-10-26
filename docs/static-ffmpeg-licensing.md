# Static FFmpeg licensing notes

This is engineering information, not legal advice.

## Pinned build

| Field | Value |
| --- | --- |
| FFmpeg | 6.1.1 |
| Provider | eugeneware/ffmpeg-static |
| Release | b6.1.1 |
| License | GPL-3.0-or-later |
| Provider release | https://github.com/eugeneware/ffmpeg-static/releases/tag/b6.1.1 |
| FFmpeg source | https://ffmpeg.org/releases/ffmpeg-6.1.1.tar.xz |

`ffmpeg.lock.json` pins the compressed asset, unpacked binary, size and SHA-256
for each target. The release script verifies them before packaging.

## Obligations if you redistribute

1. Include the GPL license text (`FFMPEG_LICENSE`).
2. Provide the corresponding source or a valid written offer.
3. Record the exact build/provider and source URL in `NOTICE`.
4. Do not add restrictions that conflict with the GPL for the binaries.
5. Keep the renderer's MIT license separate from the FFmpeg binaries.

The release archives include `NOTICE` and `FFMPEG_LICENSE`. The Rust source is
MIT; the FFmpeg binaries are GPL-3.0-or-later.

## External FFmpeg

To avoid redistributing GPL binaries:

- do not ship `ffmpeg`/`ffprobe`,
- require the user to install FFmpeg or pass `--ffmpeg`,
- keep the renderer's MIT license.

## Verification

- Check archive and unpacked binary checksums.
- Linux: verify the binary is static and executable.
- macOS/Windows: verify on native runners and review dependencies.
- Never commit FFmpeg binaries to this repository.
