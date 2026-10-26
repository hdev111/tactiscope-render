# Quick start

## 1. Build the CLI

```bash
cargo build --release -p tactiscope-render-cli
```

The binary is `target/release/tactiscope-render` (`.exe` on Windows).

## 2. Provide FFmpeg

Either:

```bash
export FFMPEG_PATH=/path/to/ffmpeg
```

or pass `--ffmpeg /path/to/ffmpeg` to the command, or put `ffmpeg` next to the
CLI binary. Release archives include `ffmpeg` and `ffprobe`.

## 3. Render a frame

```bash
./target/release/tactiscope-render render-frame \
  --plan examples/plans/solid-color.json \
  --time-ticks 0 \
  --output frame.png
```

Output is a PNG at the plan's project dimensions.

## 4. Export a video

```bash
./target/release/tactiscope-render export \
  --plan examples/plans/solid-color.json \
  --output out.mp4
```

Progress is printed as JSON lines:

```json
{"type":"progress","frame":10,"total":30,"percent":33.33}
{"type":"complete","path":"out.mp4","frames":30,"durationSeconds":1.0,"hasAudio":false}
```

## 5. Use a media plan

Generate the sample media with FFmpeg:

```bash
./scripts/make-fixtures.sh
```

Then render/export:

```bash
./target/release/tactiscope-render export \
  --plan examples/plans/media.json \
  --output media.mp4
```

Media paths in a plan are resolved relative to the current working directory
unless they are absolute.

## 6. Use the Rust library

```rust
use std::path::PathBuf;
use tactiscope_render::{Renderer, RendererOptions, RenderPlan};

let plan = RenderPlan::from_file("examples/plans/solid-color.json".as_ref())?;
let mut renderer = Renderer::new(
    plan,
    RendererOptions {
        ffmpeg_path: PathBuf::from("ffmpeg"),
        assets_root: None,
        temp_dir: None,
    },
)?;

let rgba = renderer.render_rgba(0)?;
std::fs::write("frame.rgba", rgba)?;
```

See [Rust API](rust-api.md) and [render plan](plan-schema.md) for details.

## 7. Validate and inspect the schema

```bash
./target/release/tactiscope-render validate --plan my-plan.json
./target/release/tactiscope-render schema > schema/render-plan.schema.json
```
