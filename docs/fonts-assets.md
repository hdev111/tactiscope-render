# Fonts and assets

## Fonts

Text elements use `fontFamily`, `fontWeight`, `fontStyle` and optional
`fontPath`.

Resolution order:

1. `fontPath` from the text element (relative to the assets/font roots).
2. `RENDER_FONT_MAP` JSON override.
3. System fonts (fontconfig on Linux, known Windows/macOS font files).
4. `<assetsRoot>/manifest.json` bundled family manifest.
5. Bundled fallback family.

Environment variables:

- `RENDER_FONTS_PATH`: extra font root.
- `RENDER_FONT_MAP`: JSON map of `family` or `family:style` to a font file.

The assets root is:

- `--assets` on the CLI,
- `RENDER_ASSETS_PATH`, or
- `assets` by default.

A font manifest looks like:

```json
{
  "fallback": { "regular": "NotoSans-Regular.ttf" },
  "families": {
    "arial": { "regular": "NotoSans-Regular.ttf" },
    "noto sans": {
      "regular": "NotoSans-Regular.ttf",
      "bold": "NotoSans-Bold.ttf"
    }
  }
}
```

Only single-face TTF/OTF files are supported; `.ttc` collections are skipped.

## Assets

Sticker elements use `assetPath` relative to the assets root. SVG is rasterized
with resvg. PNG/JPEG/WebP/GIF images are decoded by FFmpeg.

```json
{
  "type": "sticker",
  "assetPath": "flags/fr.svg",
  "intrinsicWidth": 640,
  "intrinsicHeight": 480
}
```

The public renderer does not bundle TactiScope's flag/shape packs. Point the
assets root at your own directory or copy the examples' assets.

## Example assets

The examples use only colors, gradients and system fonts so they run without an
asset pack. For sticker examples, create an `assets/` directory and add SVGs,
then set `--assets assets`.

## Font licensing

You are responsible for the fonts you ship or resolve. The renderer does not
redistribute system fonts. If you bundle fonts, include their licenses.
