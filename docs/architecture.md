# Architecture

```text
JSON render plan
      │
      ▼
tactiscope-render (library)
  ├── plan validation
  ├── media refs / frame timing
  ├── GPU compositor (wgpu)
  │   ├── video/image layers
  │   ├── text (fontdue)
  │   ├── SVG/stickers (resvg)
  │   ├── masks/strokes (tiny-skia)
  │   └── effects/gradients
  ├── audio mixing/mastering
  └── FFmpeg encode/mux
      │
      ▼
MP4 or PNG
```

## Workspace

```text
crates/
  render/        public library: plan, renderer, audio, export
  render-cli/    CLI binary
  compositor/    GPU layer composition
  gpu/           wgpu context/texture helpers
  effects/       GPU effect passes
  masks/         GPU mask helpers
```

## Render flow

1. Parse and validate the render plan.
2. Initialize a wgpu context and compositor.
3. For each frame:
   - decode the required video frame (FFmpeg child process) or image/SVG,
   - rasterize text/graphics/masks,
   - composite tracks in ascending order,
   - apply effects and background,
   - read back RGBA.
4. For export, pipe RGBA frames to FFmpeg and mux the mixed audio.

## Determinism

- Time is integer ticks (120,000/s).
- Frame rate is rational.
- Track order is explicit.
- Frame times are derived from the plan, not wall-clock time.

## Native-only

There is no Canvas, WebCodecs or browser fallback. The renderer requires a wgpu
adapter and FFmpeg. Failures are returned as errors rather than silently falling
back to another renderer.

## Caching

The renderer caches decoded images, sequential video decoders, text textures and
the blur background. Export uses sequential decoding; random frame requests use
seek-based decoding.

## Limits

- One renderer owns GPU state; do not share it across threads.
- Media paths must be readable by the process.
- FFmpeg is a child process; process-level cancellation is supported by the
  caller (kill the export process in your integration).
