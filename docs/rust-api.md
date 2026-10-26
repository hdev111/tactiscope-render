# Rust API

```rust
use std::path::PathBuf;
use tactiscope_render::{
    export_mp4, export_mp4_with_progress, plan_schema, render_frame_png,
    render_frame_rgba, ExportProgress, RenderPlan, Renderer, RendererOptions,
};
```

## Load a plan

```rust
let plan = RenderPlan::from_file("plan.json".as_ref())?;
let plan = RenderPlan::from_json_str(json)?;
let plan = RenderPlan::from_json(bytes)?;
```

`from_json`/`from_file` parse and structurally validate the plan. Use
`plan.to_json()` to serialize it.

Inspect a plan:

```rust
plan.width();
plan.height();
plan.fps();
plan.duration_ticks();
plan.total_frames();
plan.frame_time_ticks(frame);
```

## Options

```rust
let options = RendererOptions {
    ffmpeg_path: PathBuf::from("ffmpeg"),
    assets_root: Some(PathBuf::from("assets")),
    temp_dir: None,
};
```

- `ffmpeg_path`: FFmpeg executable.
- `assets_root`: root for sticker/graphic assets and the optional font
  manifest. Defaults to `RENDER_ASSETS_PATH` or `assets`.
- `temp_dir`: where mixed audio is written. Defaults to the system temp dir.

## Repeated frames

Use `Renderer` when rendering many frames from the same plan (preview, scrubbing,
thumbnails):

```rust
let mut renderer = Renderer::new(plan, options)?;
let rgba = renderer.render_rgba(120_000)?; // 1 second
renderer.render_png(240_000, "frame.png".as_ref())?;
```

`Renderer` keeps GPU and decoder state alive. `render_rgba` returns tightly
packed RGBA8 bytes (`width * height * 4`).

## One-shot helpers

```rust
let rgba = render_frame_rgba(&plan, 0, &options)?;
render_frame_png(&plan, 0, "frame.png".as_ref(), &options)?;
let report = export_mp4(&plan, "out.mp4".as_ref(), &options)?;
```

These create a renderer, render/export, and release it.

## Export progress

```rust
let report = export_mp4_with_progress(
    &plan,
    "out.mp4".as_ref(),
    &options,
    |progress: ExportProgress| {
        println!("{}%", progress.percent);
    },
)?;
```

`ExportReport` includes `output`, `width`, `height`, `frames`,
`duration_seconds` and `has_audio`.

## Schema

```rust
let schema = plan_schema(); // serde_json::Value
```

The schema is generated from the Rust plan types. CI checks that the committed
`schema/render-plan.schema.json` matches this function.

## Errors

All public functions return `anyhow::Result`. Typical errors:

- invalid plan version/dimensions/fps/duration,
- missing media file or missing width/height,
- unsupported element/mask/effect type,
- FFmpeg spawn/decode/encode failure,
- GPU adapter initialization failure.

There is no fallback renderer: if the GPU or FFmpeg is unavailable, the call
fails with an actionable error.

## Threading

- A `Renderer` owns GPU state and is not `Send`/`Sync` in a way that allows
  concurrent frame rendering.
- Use one renderer per worker/thread.
- Export uses FFmpeg as a child process and writes frames sequentially.
