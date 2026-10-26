# Render plan reference

The render plan is the stable integration contract between an editor and the
renderer. It is camelCase JSON. The machine-readable schema is
[`schema/render-plan.schema.json`](../schema/render-plan.schema.json).

## Time

All times are integer **media ticks**, with **120,000 ticks per second**:

```text
1 second  = 120000
1 frame   = 120000 / fps
```

Frame rate is rational:

```json
"fps": { "numerator": 30, "denominator": 1 }
```

## Top level

```json
{
  "version": 1,
  "project": { ... },
  "durationTicks": 120000,
  "media": [ ... ],
  "tracks": [ ... ],
  "audio": { "elements": [ ... ] },
  "quality": "high"
}
```

- `version`: currently `1`. Breaking changes bump this.
- `durationTicks`: timeline duration.
- `quality`: `low`, `medium`, `high` (default) or `very_high`; controls H.264 CRF.
- `tracks`: visual layers, painted in ascending `order`; later tracks are above.
- `media`: source files referenced by video/image elements.
- `audio`: timeline audio elements mixed before muxing.

## Project and background

```json
"project": {
  "width": 1920,
  "height": 1080,
  "fps": { "numerator": 30, "denominator": 1 },
  "background": { "type": "color", "color": "#112233" }
}
```

Background variants:

```json
{ "type": "color", "color": "#112233" }
{ "type": "gradient", "css": "linear-gradient(90deg, #000, #fff)" }
{ "type": "blur", "blurIntensity": 25.0 }
```

A `color` value that is a CSS gradient is treated as a gradient.

## Media

```json
{
  "id": "media-1",
  "type": "video",
  "sourcePath": "/absolute/path/clip.mp4",
  "width": 1920,
  "height": 1080
}
```

`type` is `video` or `image`. `width`/`height` are required for video and image
decoding. Images can be PNG, JPEG, WebP, GIF or SVG.

## Tracks and elements

```json
{
  "order": 1,
  "hidden": false,
  "elements": [ ... ]
}
```

Elements have a `type` discriminator:

- `video`
- `image`
- `text`
- `graphic`
- `sticker`
- `scene-effect`

All visual elements use:

```json
"transform": {
  "positionX": 0,
  "positionY": 0,
  "scaleX": 1,
  "scaleY": 1,
  "rotate": 0
},
"opacity": 1.0,
"blendMode": "normal",
"startTicks": 0,
"durationTicks": 120000,
"effects": [],
"masks": [],
"hidden": false
```

Positions are canvas pixels; scale is a multiplier; rotation is degrees.
`blendMode` supports the standard 17 modes (normal, darken, multiply, colorBurn,
lighten, screen, plusLighter, colorDodge, overlay, softLight, hardLight,
difference, exclusion, hue, saturation, color, luminosity).

### Video

```json
{
  "type": "video",
  "id": "clip-1",
  "mediaId": "media-1",
  "startTicks": 0,
  "durationTicks": 240000,
  "trimStartTicks": 0,
  "transform": { "positionX": 0, "positionY": 0, "scaleX": 1, "scaleY": 1, "rotate": 0 },
  "opacity": 1.0,
  "blendMode": "normal",
  "retime": { "rate": 1.0, "maintainPitch": false }
}
```

### Image

Same as video without `trimStartTicks` or `retime`; `mediaId` points at an image.

### Text

```json
{
  "type": "text",
  "id": "title",
  "startTicks": 0,
  "durationTicks": 120000,
  "transform": { "positionX": 0, "positionY": 0, "scaleX": 1, "scaleY": 1, "rotate": 0 },
  "opacity": 1.0,
  "blendMode": "normal",
  "params": {
    "content": "Hello",
    "fontSize": 15,
    "fontFamily": "Arial",
    "fontWeight": "normal",
    "fontStyle": "normal",
    "textAlign": "center",
    "textDecoration": "none",
    "letterSpacing": 0,
    "lineHeight": 1.2,
    "color": "#ffffff"
  }
}
```

`fontSize` is relative: rendered pixels are approximately
`fontSize * canvasHeight / 90`. The default design size is `15`.

Optional text background fields: `backgroundEnabled`, `backgroundColor`,
`backgroundCornerRadius`, `backgroundPaddingX/Y`, `backgroundOffsetX/Y`.
`fontPath` can point at a specific TTF/OTF file. See
[fonts and assets](fonts-assets.md).

### Graphic

```json
{
  "type": "graphic",
  "id": "star",
  "definitionId": "star",
  "params": { "fill": "#ffd166", "points": 5, "depth": 0.5 },
  "startTicks": 0,
  "durationTicks": 120000,
  "transform": { "positionX": 0, "positionY": 0, "scaleX": 1, "scaleY": 1, "rotate": 0 },
  "opacity": 1.0,
  "blendMode": "normal"
}
```

Definitions: `rectangle`, `ellipse`, `polygon`, `star`. Param keys depend on the
definition.

### Sticker / SVG

```json
{
  "type": "sticker",
  "id": "flag",
  "assetPath": "flags/fr.svg",
  "intrinsicWidth": 640,
  "intrinsicHeight": 480,
  "startTicks": 0,
  "durationTicks": 120000,
  "transform": { "positionX": 0, "positionY": 0, "scaleX": 1, "scaleY": 1, "rotate": 0 },
  "opacity": 1.0,
  "blendMode": "normal"
}
```

`assetPath` is resolved under the assets root.

### Scene effect

```json
{
  "type": "scene-effect",
  "id": "blur",
  "effectType": "blur",
  "params": { "intensity": 20.0 },
  "startTicks": 0,
  "durationTicks": 120000
}
```

## Effects

```json
"effects": [
  { "id": "blur-1", "type": "blur", "intensity": 20.0 }
]
```

Currently supported: `blur`.

## Masks

```json
"masks": [
  {
    "type": "rectangle",
    "feather": 0.0,
    "inverted": false,
    "strokeColor": "#ffffff",
    "strokeWidth": 0.0,
    "strokeAlign": "center",
    "params": { "width": 0.6, "height": 0.6, "centerX": 0.5, "centerY": 0.5 }
  }
]
```

Types: `split`, `cinematic-bars`, `rectangle`, `ellipse`, `heart`, `diamond`,
`star`, `text`, `freeform`. Params are mask-specific; see the schema/examples.

## Keyframes

`animations` is an object keyed by property path. Each channel has `keys`:

```json
"animations": {
  "opacity": {
    "keys": [
      { "time": 0, "value": 0, "segmentToNext": "linear" },
      { "time": 120000, "value": 1, "segmentToNext": "linear" }
    ]
  }
}
```

Segments: `hold`, `linear`, `bezier` (with bezier control fields). Color
channels use `{ "r": ..., "g": ..., "b": ..., "a": ... }` sub-channels.

## Audio

```json
"audio": {
  "elements": [
    {
      "sourcePath": "/path/music.wav",
      "startTicks": 0,
      "durationTicks": 600000,
      "trimStartTicks": 0,
      "volumeDb": 0,
      "muted": false,
      "retime": { "rate": 1.0, "maintainPitch": true }
    }
  ]
}
```

`sourceUrl` is also supported for remote audio. Audio is mixed and mastered to
48 kHz stereo, then muxed as AAC when present.

## Versioning

- `version` is required.
- Additive fields are backward compatible.
- Breaking field/shape changes bump `version`.
- The schema is generated from the Rust types and checked in CI.
