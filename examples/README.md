# Examples

## Render plans

- `plans/solid-color.json` — minimal plan, no media or fonts.
- `plans/gradient.json` — CSS gradient background.
- `plans/text.json` — text layer using a system font.
- `plans/media.json` — video + image overlay + audio.

Generate the media fixtures used by `media.json`:

```bash
./scripts/make-fixtures.sh
```

Then export:

```bash
cargo run -p tactiscope-render-cli -- export \
  --plan examples/plans/media.json \
  --output media.mp4
```

## Rust examples

```bash
cargo run -p tactiscope-render --example render_frame -- \
  examples/plans/solid-color.json 0 frame.png

cargo run -p tactiscope-render --example export -- \
  examples/plans/solid-color.json out.mp4
```

Set `FFMPEG_PATH` if `ffmpeg` is not on `PATH`.
