//! CSS color parsing shared by the native renderer.
//!
//! Matches the color subset used by TactiScope (culori-compatible) that the timeline can
//! produce: hex (3/4/6/8 digits), `rgb()`/`rgba()`, `hsl()`/`hsla()`, the
//! common named colors and `transparent`.

use anyhow::{Context, Result, bail};

/// Parse a CSS color into non-premultiplied sRGB + alpha, all in `0..=1`.
pub fn parse_color(input: &str) -> Result<[f32; 4]> {
    let value = input.trim();
    if value.is_empty() {
        bail!("empty color");
    }
    let lower = value.to_ascii_lowercase();
    if lower == "transparent" {
        return Ok([0.0, 0.0, 0.0, 0.0]);
    }
    if let Some(rgba) = parse_function(&lower) {
        return rgba;
    }
    if let Some(rgba) = named_color(&lower) {
        return Ok(rgba);
    }
    parse_hex(&lower).with_context(|| format!("unsupported color '{input}'"))
}

/// Background clear colors keep the historical behavior: alpha is ignored so
/// the canvas clear is always opaque (transparent becomes black).
pub fn parse_background_color(input: &str) -> Result<[f32; 4]> {
    let mut color = if input.trim().eq_ignore_ascii_case("transparent") {
        [0.0, 0.0, 0.0, 1.0]
    } else {
        parse_color(input)?
    };
    color[3] = 1.0;
    Ok(color)
}

pub fn to_linear(color: [f32; 4]) -> [f32; 4] {
    [
        srgb_to_linear(color[0]),
        srgb_to_linear(color[1]),
        srgb_to_linear(color[2]),
        color[3].clamp(0.0, 1.0),
    ]
}

pub fn from_linear(color: [f32; 4]) -> [f32; 4] {
    [
        linear_to_srgb(color[0]),
        linear_to_srgb(color[1]),
        linear_to_srgb(color[2]),
        color[3].clamp(0.0, 1.0),
    ]
}

fn srgb_to_linear(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

fn to_byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Same output as culori's `formatHex` / `formatHex8` used by the web app.
pub fn format_hex(color: [f32; 4]) -> String {
    let r = to_byte(color[0]);
    let g = to_byte(color[1]);
    let b = to_byte(color[2]);
    if color[3] < 1.0 {
        format!("#{r:02x}{g:02x}{b:02x}{:02x}", to_byte(color[3]))
    } else {
        format!("#{r:02x}{g:02x}{b:02x}")
    }
}

fn parse_hex(value: &str) -> Result<[f32; 4]> {
    let hex = value.strip_prefix('#').context("missing '#'")?;
    let expand = |character: char| -> String { character.to_string().repeat(2) };
    let (rgb, alpha) = match hex.len() {
        3 => (
            hex.chars().map(expand).collect::<String>(),
            String::from("ff"),
        ),
        4 => {
            let mut characters = hex.chars();
            let rgb = characters.by_ref().take(3).map(expand).collect::<String>();
            let alpha = characters.next().map(expand).unwrap_or_else(|| "ff".into());
            (rgb, alpha)
        }
        6 => (hex.to_string(), String::from("ff")),
        8 => (hex[0..6].to_string(), hex[6..8].to_string()),
        _ => bail!("unsupported hex color length {}", hex.len()),
    };
    let channel = |range: std::ops::Range<usize>| -> Result<f32> {
        let value = u8::from_str_radix(&rgb[range], 16).context("invalid hex channel")?;
        Ok(value as f32 / 255.0)
    };
    let alpha = u8::from_str_radix(&alpha, 16).context("invalid hex alpha")?;
    Ok([
        channel(0..2)?,
        channel(2..4)?,
        channel(4..6)?,
        alpha as f32 / 255.0,
    ])
}

fn parse_function(lower: &str) -> Option<Result<[f32; 4]>> {
    let open = lower.find('(')?;
    if !lower.ends_with(')') {
        return None;
    }
    let name = lower[..open].trim();
    let body = &lower[open + 1..lower.len() - 1];
    match name {
        "rgb" | "rgba" => Some(parse_rgb_function(body)),
        "hsl" | "hsla" => Some(parse_hsl_function(body)),
        _ => None,
    }
}

fn split_function_args(body: &str) -> Vec<String> {
    let normalized = body.replace('/', " ");
    if normalized.contains(',') {
        normalized
            .split(',')
            .map(|part| part.trim().to_string())
            .collect()
    } else {
        normalized
            .split_whitespace()
            .map(|part| part.trim().to_string())
            .collect()
    }
}

fn parse_channel(value: &str) -> Result<f32> {
    let value = value.trim();
    let number = value.strip_suffix('%').map_or_else(
        || value.parse::<f32>().map(|number| number / 255.0),
        |percent| percent.parse::<f32>().map(|percent| percent / 100.0),
    );
    number.context("invalid color channel")
}

fn parse_alpha(value: &str) -> Result<f32> {
    let value = value.trim();
    if let Some(percent) = value.strip_suffix('%') {
        return Ok(percent.parse::<f32>().context("invalid alpha")? / 100.0);
    }
    value.parse::<f32>().context("invalid alpha")
}

fn parse_rgb_function(body: &str) -> Result<[f32; 4]> {
    let parts = split_function_args(body);
    if parts.len() < 3 {
        bail!("rgb() needs three channels");
    }
    let alpha = if parts.len() > 3 {
        parse_alpha(&parts[3])?
    } else {
        1.0
    };
    Ok([
        parse_channel(&parts[0])?,
        parse_channel(&parts[1])?,
        parse_channel(&parts[2])?,
        alpha.clamp(0.0, 1.0),
    ])
}

fn parse_hsl_function(body: &str) -> Result<[f32; 4]> {
    let parts = split_function_args(body);
    if parts.len() < 3 {
        bail!("hsl() needs three components");
    }
    let hue = parts[0]
        .trim()
        .trim_end_matches("deg")
        .parse::<f32>()
        .context("invalid hue")?;
    let saturation = parts[1]
        .trim()
        .trim_end_matches('%')
        .parse::<f32>()
        .context("invalid saturation")?
        / 100.0;
    let lightness = parts[2]
        .trim()
        .trim_end_matches('%')
        .parse::<f32>()
        .context("invalid lightness")?
        / 100.0;
    let alpha = if parts.len() > 3 {
        parse_alpha(&parts[3])?
    } else {
        1.0
    };
    let (r, g, b) = hsl_to_rgb(hue, saturation, lightness);
    Ok([r, g, b, alpha.clamp(0.0, 1.0)])
}

fn hsl_to_rgb(hue: f32, saturation: f32, lightness: f32) -> (f32, f32, f32) {
    let hue = hue.rem_euclid(360.0) / 360.0;
    let saturation = saturation.clamp(0.0, 1.0);
    let lightness = lightness.clamp(0.0, 1.0);
    if saturation <= f32::EPSILON {
        return (lightness, lightness, lightness);
    }
    let q = if lightness < 0.5 {
        lightness * (1.0 + saturation)
    } else {
        lightness + saturation - lightness * saturation
    };
    let p = 2.0 * lightness - q;
    let channel = |mut t: f32| -> f32 {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            return p + (q - p) * 6.0 * t;
        }
        if t < 0.5 {
            return q;
        }
        if t < 2.0 / 3.0 {
            return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
        }
        p
    };
    (
        channel(hue + 1.0 / 3.0),
        channel(hue),
        channel(hue - 1.0 / 3.0),
    )
}

fn named_color(value: &str) -> Option<[f32; 4]> {
    let rgb = match value {
        "black" => 0x000000,
        "white" => 0xffffff,
        "red" => 0xff0000,
        "green" => 0x008000,
        "lime" => 0x00ff00,
        "blue" => 0x0000ff,
        "yellow" => 0xffff00,
        "cyan" | "aqua" => 0x00ffff,
        "magenta" | "fuchsia" => 0xff00ff,
        "gray" | "grey" => 0x808080,
        "silver" => 0xc0c0c0,
        "maroon" => 0x800000,
        "olive" => 0x808000,
        "teal" => 0x008080,
        "navy" => 0x000080,
        "purple" => 0x800080,
        "orange" => 0xffa500,
        "pink" => 0xffc0cb,
        "brown" => 0xa52a2a,
        "gold" => 0xffd700,
        "indigo" => 0x4b0082,
        "violet" => 0xee82ee,
        "beige" => 0xf5f5dc,
        "ivory" => 0xfffff0,
        "khaki" => 0xf0e68c,
        "salmon" => 0xfa8072,
        "tomato" => 0xff6347,
        "turquoise" => 0x40e0d0,
        "crimson" => 0xdc143c,
        "darkred" => 0x8b0000,
        "darkgreen" => 0x006400,
        "darkblue" => 0x00008b,
        "lightgray" | "lightgrey" => 0xd3d3d3,
        "darkgray" | "darkgrey" => 0xa9a9a9,
        "rebeccapurple" => 0x663399,
        _ => return None,
    };
    Some([
        ((rgb >> 16) & 0xff) as f32 / 255.0,
        ((rgb >> 8) & 0xff) as f32 / 255.0,
        (rgb & 0xff) as f32 / 255.0,
        1.0,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(left: [f32; 4], right: [f32; 4]) {
        for index in 0..4 {
            assert!(
                (left[index] - right[index]).abs() < 1e-4,
                "channel {index}: {} vs {}",
                left[index],
                right[index]
            );
        }
    }

    #[test]
    fn parses_hex_variants() {
        approx(
            parse_color("#ff8000").unwrap(),
            [1.0, 128.0 / 255.0, 0.0, 1.0],
        );
        approx(parse_color("#f80").unwrap(), [1.0, 136.0 / 255.0, 0.0, 1.0]);
        approx(
            parse_color("#ff800080").unwrap(),
            [1.0, 128.0 / 255.0, 0.0, 128.0 / 255.0],
        );
        approx(
            parse_color("#f808").unwrap(),
            [1.0, 136.0 / 255.0, 0.0, 136.0 / 255.0],
        );
    }

    #[test]
    fn parses_functions_and_names() {
        approx(
            parse_color("rgb(255, 128, 0)").unwrap(),
            [1.0, 128.0 / 255.0, 0.0, 1.0],
        );
        approx(
            parse_color("rgba(255,128,0,0.5)").unwrap(),
            [1.0, 128.0 / 255.0, 0.0, 0.5],
        );
        approx(parse_color("white").unwrap(), [1.0, 1.0, 1.0, 1.0]);
        approx(parse_color("transparent").unwrap(), [0.0, 0.0, 0.0, 0.0]);
        let hsl = parse_color("hsl(0, 100%, 50%)").unwrap();
        approx(hsl, [1.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn linear_round_trip_formats_hex() {
        let color = parse_color("#3366cc").unwrap();
        let linear = to_linear(color);
        let round_trip = from_linear(linear);
        approx(round_trip, color);
        assert_eq!(format_hex(round_trip), "#3366cc");
        assert_eq!(format_hex([1.0, 1.0, 1.0, 0.5]), "#ffffff80");
    }

    #[test]
    fn background_clear_is_opaque() {
        approx(
            parse_background_color("transparent").unwrap(),
            [0.0, 0.0, 0.0, 1.0],
        );
        approx(
            parse_background_color("#ffffff80").unwrap(),
            [1.0, 1.0, 1.0, 1.0],
        );
    }
}
