# tactiscope-render

Native GPU video renderer for building video editors.

`tactiscope-render` takes a versioned JSON render plan, composites video, images,
text, SVG, graphics, effects and masks on the GPU, mixes audio, and exports an
H.264 + AAC MP4. It is the rendering engine extracted from
[TactiScope](https://github.com/hdev111/TactiScope), packaged as a standalone
library and CLI so other editor projects can use it.

- **Language-agnostic integration:** CLI + JSON render plan.
- **Rust integration:** library API for embedding the renderer.
- **Native only:** wgpu compositor, FFmpeg decode/encode, resvg/fontdue/tiny-skia.
- **Deterministic:** integer media ticks and rational frame rates.
- **No editor UI, timeline model or project storage.**

## Features

- Video and image layers with cover/contain transforms, position, scale,
  rotation, opacity and 17 blend modes.
- Text with font resolution, weights/styles, alignment, letter spacing, line
  height, backgrounds, padding and corner radius.
- SVG images and stickers, shapes/graphics, gradients and blur backgrounds.
- Masks: split, cinematic bars, rectangle, ellipse, heart, diamond, star,
  freeform and text, with feathering, inversion and stroke layers.
- Keyframed transform, opacity, text color/background, blur intensity, audio
  volume and graphic params (hold, linear and cubic-bezier segments).
- Audio mixing, retime, keyframed volume and mastering to 48 kHz stereo.
- H.264 + AAC MP4 export with quality presets, progress and cancellation at the
  process level.
- Single-frame PNG rendering for thumbnails and previews.

## Requirements

- Rust 1.93+ (edition 2024).
- A GPU supported by wgpu (Vulkan, Metal, DX12 or a software adapter).
- FFmpeg/FFprobe:
  - release archives bundle a pinned static build, or
  - pass `--ffmpeg /path/to/ffmpeg`, set `FFMPEG_PATH`, put it next to the
    binary, or install it on `PATH`.
- Fonts are resolved from system fonts or an explicit font directory/assets
  root. See [docs/fonts-assets.md](docs/fonts-assets.md).

## Quick start

### CLI from source

```bash
cargo run -p tactiscope-render-cli -- render-frame \
  --plan examples/plans/solid-color.json \
  --time-ticks 0 \
  --output frame.png

cargo run -p tactiscope-render-cli -- export \
  --plan examples/plans/solid-color.json \
  --output out.mp4 \
  --ffmpeg /path/to/ffmpeg
```

Print the render-plan JSON Schema:

```bash
cargo run -p tactiscope-render-cli -- schema > schema/render-plan.schema.json
```

Validate a plan:

```bash
cargo run -p tactiscope-render-cli -- validate --plan examples/plans/solid-color.json
```

### Release archive

A release archive contains:

```text
tactiscope-render
ffmpeg
ffprobe
LICENSE
NOTICE
FFMPEG_LICENSE
README.md
schema/render-plan.schema.json
docs/
```

Use it directly:

```bash
./tactiscope-render export --plan my-plan.json --output out.mp4
```

The CLI automatically finds `ffmpeg` next to the executable.

## Rust API

```rust
use std::path::PathBuf;
use tactiscope_render::{Renderer, RendererOptions, RenderPlan};

fn main() -> anyhow::Result<()> {
    let plan = RenderPlan::from_file("examples/plans/solid-color.json".as_ref())?;
    let options = RendererOptions {
        ffmpeg_path: PathBuf::from("ffmpeg"),
        assets_root: None,
        temp_dir: None,
    };

    let mut renderer = Renderer::new(plan, options)?;
    let rgba = renderer.render_rgba(0)?;
    std::fs::write("frame.rgba", rgba)?;
    Ok(())
}
```

Other entry points:

- `render_frame_rgba(plan, ticks, options)`
- `render_frame_png(plan, ticks, output, options)`
- `export_mp4(plan, output, options)`
- `export_mp4_with_progress(plan, output, options, callback)`

The full API is documented in [docs/rust-api.md](docs/rust-api.md).

## Render plan

The render plan is the integration contract. It is versioned, camelCase JSON and
uses integer media ticks (120,000 ticks per second) with rational frame rates.

Minimal solid-color plan:

```json
{
  "version": 1,
  "project": {
    "width": 1280,
    "height": 720,
    "fps": { "numerator": 30, "denominator": 1 },
    "background": { "type": "color", "color": "#112233" }
  },
  "durationTicks": 120000,
  "media": [],
  "tracks": [],
  "audio": { "elements": [] },
  "quality": "high"
}
```

- [JSON Schema](schema/render-plan.schema.json)
- [Plan reference](docs/plan-schema.md)
- [Examples](examples/plans)

## FFmpeg and licensing

The renderer source is **MIT** (see [LICENSE](LICENSE)).

Release archives bundle a pinned **FFmpeg 6.1.1** static build licensed under
**GPL-3.0-or-later**. The archive includes the FFmpeg license and a
[NOTICE](NOTICE) with the exact build/source information. If you redistribute a
release archive, you must comply with the GPL for those FFmpeg binaries.

If you prefer not to redistribute GPL binaries, use an external FFmpeg build
(`--ffmpeg`, `FFMPEG_PATH`, next to the binary or on `PATH`). The renderer code
itself remains MIT.

See [docs/ffmpeg.md](docs/ffmpeg.md) for the full explanation.

## Platforms

- Linux x64: built and tested locally; release workflow builds it.
- macOS x64/arm64 and Windows x64: built in CI and release workflow; native GPU
  and FFmpeg packaging are validated on those runners.
- Other targets require a wgpu-capable platform and a matching FFmpeg build.

## Limitations

- The renderer needs a GPU adapter; there is no CPU-only compositor.
- `tactiscope-render` is a renderer, not an editor. It has no timeline, media
  import, project persistence or UI.
- Media paths are resolved by the caller; the renderer does not manage media
  libraries.
- FFmpeg is a separate process, not linked into the library.
- Very high resolution/frame-rate plans need a suitably fast GPU and decoder.

## Documentation

- [Quick start](docs/quick-start.md)
- [Render plan reference](docs/plan-schema.md)
- [Rust API](docs/rust-api.md)
- [FFmpeg and licensing](docs/ffmpeg.md)
- [Fonts and assets](docs/fonts-assets.md)
- [Architecture](docs/architecture.md)
- [Release packaging](docs/release.md)
- [Contributing](docs/contributing.md)

## Contributing

Run the checks before opening a PR:

```bash
cargo fmt --check
cargo test --workspace
cargo run -p tactiscope-render-cli -- schema > /tmp/schema.json
```

Keep app-specific behavior out of this crate. New element/effect types need a
render-plan field, a schema update and a test.

## Lineage

The renderer was extracted from the TactiScope project, which started from the
OpenCut codebase. The MIT copyright notice retains both attributions.
