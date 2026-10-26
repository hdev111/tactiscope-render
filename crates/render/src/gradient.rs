//! CSS background rasterization (solid colors + linear/radial gradients).
//!
//! Matches the gradient parser used by TactiScope closely enough for the native
//! renderer: top-level comma separated layers, CSS gradient geometry, stop
//! normalization and sRGB interpolation. No Canvas/text stack is involved.

use anyhow::{Context, Result, bail};
use tiny_skia::{
    Color, FillRule, GradientStop, LinearGradient, Paint, PathBuilder, Pixmap, Point,
    RadialGradient, Rect, SpreadMode, Transform,
};

use crate::color;

#[derive(Clone, Copy, Debug)]
pub struct Stop {
    pub offset: f32,
    pub color: [f32; 4],
}

#[derive(Clone, Debug)]
pub struct LinearSpec {
    pub angle_deg: f32,
}

#[derive(Clone, Debug)]
pub struct RadialSpec {
    pub cx: f32,
    pub cy: f32,
    pub rx: f32,
    pub ry: f32,
}

#[derive(Clone, Debug)]
pub enum Layer {
    Color([f32; 4]),
    Linear {
        spec: LinearSpec,
        stops: Vec<Stop>,
        repeating: bool,
    },
    Radial {
        spec: RadialSpec,
        stops: Vec<Stop>,
        repeating: bool,
    },
}

pub fn is_gradient(input: &str) -> bool {
    input.to_ascii_lowercase().contains("gradient(")
}

/// Rasterize a CSS `background` string into RGBA (straight alpha).
pub fn rasterize_background(css: &str, width: u32, height: u32) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        bail!("cannot rasterize a background without a canvas");
    }
    let layers = parse_layers(css, width as f32, height as f32)?;
    if layers.is_empty() {
        bail!("background '{css}' has no layers");
    }
    let mut pixmap = Pixmap::new(width, height).context("allocate background pixmap")?;
    let rect = Rect::from_xywh(0.0, 0.0, width as f32, height as f32)
        .context("background rect is invalid")?;
    let mut builder = PathBuilder::new();
    builder.push_rect(rect);
    let path = builder.finish().context("build background path")?;
    // CSS paints the last layer first (first layer is on top).
    for layer in layers.iter().rev() {
        let mut paint = Paint::default();
        // The path covers exact canvas pixels; antialiasing would leave a
        // half-transparent seam around the canvas border.
        paint.anti_alias = false;
        match layer {
            Layer::Color(color) => paint.set_color(skia_color(*color)),
            Layer::Linear {
                spec,
                stops,
                repeating,
            } => {
                let radians = spec.angle_deg.to_radians();
                let dx = radians.sin();
                let dy = -radians.cos();
                let half = ((width as f32 * dx).abs() + (height as f32 * dy).abs()) / 2.0;
                let center_x = width as f32 / 2.0;
                let center_y = height as f32 / 2.0;
                let start = Point::from_xy(center_x - dx * half, center_y - dy * half);
                let end = Point::from_xy(center_x + dx * half, center_y + dy * half);
                let mode = spread_mode(*repeating);
                paint.shader = LinearGradient::new(
                    start,
                    end,
                    gradient_stops(stops),
                    mode,
                    Transform::identity(),
                )
                .context("build linear gradient shader")?;
            }
            Layer::Radial {
                spec,
                stops,
                repeating,
            } => {
                // tiny-skia radial shaders are circular; scale the shader space
                // to reproduce CSS elliptical extents.
                let radius = spec.rx.max(spec.ry).max(1e-6);
                let scale_x = (spec.rx / radius).max(1e-6);
                let scale_y = (spec.ry / radius).max(1e-6);
                let transform =
                    Transform::from_translate(spec.cx, spec.cy).pre_scale(scale_x, scale_y);
                paint.shader = RadialGradient::new(
                    Point::from_xy(0.0, 0.0),
                    Point::from_xy(0.0, 0.0),
                    radius,
                    gradient_stops(stops),
                    spread_mode(*repeating),
                    transform,
                )
                .context("build radial gradient shader")?;
            }
        }
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    Ok(to_straight_rgba(pixmap.data()))
}

fn spread_mode(repeating: bool) -> SpreadMode {
    if repeating {
        SpreadMode::Repeat
    } else {
        SpreadMode::Pad
    }
}

fn skia_color(rgba: [f32; 4]) -> Color {
    Color::from_rgba(
        rgba[0].clamp(0.0, 1.0),
        rgba[1].clamp(0.0, 1.0),
        rgba[2].clamp(0.0, 1.0),
        rgba[3].clamp(0.0, 1.0),
    )
    .unwrap_or(Color::TRANSPARENT)
}

fn gradient_stops(stops: &[Stop]) -> Vec<GradientStop> {
    stops
        .iter()
        .map(|stop| GradientStop::new(stop.offset, skia_color(stop.color)))
        .collect()
}

fn to_straight_rgba(premultiplied: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(premultiplied.len());
    for pixel in premultiplied.chunks_exact(4) {
        let alpha = pixel[3] as f32 / 255.0;
        if alpha <= 0.0 {
            output.extend_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        output.push((pixel[0] as f32 / alpha).round().clamp(0.0, 255.0) as u8);
        output.push((pixel[1] as f32 / alpha).round().clamp(0.0, 255.0) as u8);
        output.push((pixel[2] as f32 / alpha).round().clamp(0.0, 255.0) as u8);
        output.push(pixel[3]);
    }
    output
}

fn parse_layers(css: &str, width: f32, height: f32) -> Result<Vec<Layer>> {
    let mut layers = Vec::new();
    for segment in split_top_level(css, ',')? {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        layers.push(parse_layer(segment, width, height)?);
    }
    Ok(layers)
}

fn parse_layer(segment: &str, width: f32, height: f32) -> Result<Layer> {
    let lower = segment.to_ascii_lowercase();
    let (kind, repeating) = if lower.contains("repeating-linear-gradient(") {
        ("linear", true)
    } else if lower.contains("linear-gradient(") {
        ("linear", false)
    } else if lower.contains("repeating-radial-gradient(") {
        ("radial", true)
    } else if lower.contains("radial-gradient(") {
        ("radial", false)
    } else {
        let color = color::parse_color(segment)
            .with_context(|| format!("unsupported background layer '{segment}'"))?;
        return Ok(Layer::Color(color));
    };
    let open = segment
        .find('(')
        .context("gradient without an opening parenthesis")?;
    let body = segment[open + 1..]
        .strip_suffix(')')
        .context("gradient without a closing parenthesis")?;
    match kind {
        "linear" => parse_linear(body, width, height, repeating),
        _ => parse_radial(body, width, height, repeating),
    }
}

fn parse_linear(body: &str, width: f32, height: f32, repeating: bool) -> Result<Layer> {
    let mut args = split_top_level(body, ',')?;
    if args.is_empty() {
        bail!("linear-gradient without color stops");
    }
    let angle_deg = if is_linear_orientation(args[0].trim()) {
        let angle = parse_angle(args[0].trim())?;
        args.remove(0);
        angle
    } else {
        180.0
    };
    let gradient_length = {
        let radians = angle_deg.to_radians();
        let dx = radians.sin().abs();
        let dy = radians.cos().abs();
        width * dx + height * dy
    };
    let stops = parse_stops(&args, gradient_length)?;
    Ok(Layer::Linear {
        spec: LinearSpec { angle_deg },
        stops,
        repeating,
    })
}

fn parse_radial(body: &str, width: f32, height: f32, repeating: bool) -> Result<Layer> {
    let mut args = split_top_level(body, ',')?;
    if args.is_empty() {
        bail!("radial-gradient without color stops");
    }
    let (spec, gradient_length) = if is_radial_orientation(args[0].trim()) {
        let spec = parse_radial_spec(args[0].trim(), width, height)?;
        args.remove(0);
        let length = spec.rx.max(spec.ry);
        (spec, length)
    } else {
        let spec = resolve_radial_extents(
            width,
            height,
            width / 2.0,
            height / 2.0,
            Shape::Ellipse,
            "farthest-corner",
        );
        let gradient_length = spec.rx.max(spec.ry);
        (spec, gradient_length)
    };
    let stops = parse_stops(&args, gradient_length.max(1.0))?;
    Ok(Layer::Radial {
        spec,
        stops,
        repeating,
    })
}

fn is_linear_orientation(argument: &str) -> bool {
    let lower = argument.trim().to_ascii_lowercase();
    if lower.starts_with("to ") {
        return true;
    }
    parse_angle(&lower).is_ok()
}

fn is_radial_orientation(argument: &str) -> bool {
    let lower = argument.trim().to_ascii_lowercase();
    if lower.contains(" at ") || lower == "at" {
        return true;
    }
    for keyword in [
        "circle",
        "ellipse",
        "closest-side",
        "closest-corner",
        "farthest-side",
        "farthest-corner",
        "contain",
        "cover",
    ] {
        if lower.starts_with(keyword) {
            return true;
        }
    }
    // Bare lengths: `radial-gradient(40% 20%, ...)`.
    lower
        .split_whitespace()
        .all(|token| parse_distance(token, 1.0).is_some())
}

fn parse_angle(argument: &str) -> Result<f32> {
    let lower = argument.trim().to_ascii_lowercase();
    if lower.starts_with("to ") {
        return Ok(angle_from_direction(&lower));
    }
    if let Some(value) = lower.strip_suffix("deg") {
        return Ok(value.trim().parse::<f32>().context("invalid angle")?);
    }
    if let Some(value) = lower.strip_suffix("rad") {
        return Ok(
            value.trim().parse::<f32>().context("invalid angle")? * 180.0 / std::f32::consts::PI,
        );
    }
    if let Some(value) = lower.strip_suffix("grad") {
        return Ok(value.trim().parse::<f32>().context("invalid angle")? * 0.9);
    }
    if let Some(value) = lower.strip_suffix("turn") {
        return Ok(value.trim().parse::<f32>().context("invalid angle")? * 360.0);
    }
    bail!("unsupported gradient orientation '{argument}'")
}

fn angle_from_direction(value: &str) -> f32 {
    let normalized = value.replace("to ", "");
    let mut dx = 0.0f32;
    let mut dy = 0.0f32;
    for part in normalized.split_whitespace() {
        match part {
            "left" => dx = -1.0,
            "right" => dx = 1.0,
            "top" => dy = -1.0,
            "bottom" => dy = 1.0,
            _ => {}
        }
    }
    if dx == 0.0 && dy == 0.0 {
        return 180.0;
    }
    let angle = dx.atan2(-dy).to_degrees();
    (angle + 360.0) % 360.0
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Circle,
    Ellipse,
}

fn parse_radial_spec(argument: &str, width: f32, height: f32) -> Result<RadialSpec> {
    let lower = argument.trim().to_ascii_lowercase();
    let (descriptor, position) = match lower.split_once(" at ") {
        Some((descriptor, position)) => (descriptor.trim(), Some(position.trim())),
        None => (lower.as_str(), None),
    };
    let (cx, cy) = parse_position(position, width, height);
    let mut shape = Shape::Ellipse;
    let mut extent = "farthest-corner";
    let mut sizes = Vec::new();
    for token in descriptor.split_whitespace() {
        match token {
            "circle" => shape = Shape::Circle,
            "ellipse" => shape = Shape::Ellipse,
            "closest-side" | "closest-corner" | "farthest-side" | "farthest-corner" | "contain"
            | "cover" => extent = token,
            other => {
                if let Some(distance) = parse_distance_value(other) {
                    sizes.push(distance);
                }
            }
        }
    }
    let extent = match extent {
        "contain" => "closest-side",
        "cover" => "farthest-corner",
        other => other,
    };
    if sizes.is_empty() {
        return Ok(resolve_radial_extents(width, height, cx, cy, shape, extent));
    }
    let (rx, ry) = if sizes.len() > 1 {
        (
            resolve_distance(sizes[0], width),
            resolve_distance(sizes[1], height),
        )
    } else {
        let radius = resolve_distance(sizes[0], width.max(height));
        (radius, radius)
    };
    Ok(RadialSpec { cx, cy, rx, ry })
}

fn parse_position(position: Option<&str>, width: f32, height: f32) -> (f32, f32) {
    let Some(position) = position else {
        return (width / 2.0, height / 2.0);
    };
    let tokens = position.split_whitespace().collect::<Vec<_>>();
    let mut x = None;
    let mut y = None;
    for token in tokens {
        match token {
            "left" => x = Some(0.0),
            "right" => x = Some(width),
            "top" => y = Some(0.0),
            "bottom" => y = Some(height),
            "center" => match (x, y) {
                (None, _) => x = Some(width / 2.0),
                (_, None) => y = Some(height / 2.0),
                _ => {}
            },
            other => {
                if let Some(distance) = parse_distance_value(other) {
                    if x.is_none() {
                        x = Some(resolve_distance(distance, width));
                    } else {
                        y = Some(resolve_distance(distance, height));
                    }
                }
            }
        }
    }
    (x.unwrap_or(width / 2.0), y.unwrap_or(height / 2.0))
}

fn resolve_radial_extents(
    width: f32,
    height: f32,
    cx: f32,
    cy: f32,
    shape: Shape,
    extent: &str,
) -> RadialSpec {
    let left = cx.max(0.0);
    let right = (width - cx).max(0.0);
    let top = cy.max(0.0);
    let bottom = (height - cy).max(0.0);
    if shape == Shape::Circle {
        let corners = [
            (left * left + top * top).sqrt(),
            (right * right + top * top).sqrt(),
            (left * left + bottom * bottom).sqrt(),
            (right * right + bottom * bottom).sqrt(),
        ];
        let radius = match extent {
            "closest-side" => left.min(right).min(top).min(bottom),
            "farthest-side" => left.max(right).max(top).max(bottom),
            "closest-corner" => corners.iter().copied().fold(f32::INFINITY, f32::min),
            _ => corners.iter().copied().fold(0.0, f32::max),
        };
        return RadialSpec {
            cx,
            cy,
            rx: radius.max(0.0),
            ry: radius.max(0.0),
        };
    }
    if extent == "closest-side" {
        return RadialSpec {
            cx,
            cy,
            rx: left.min(right),
            ry: top.min(bottom),
        };
    }
    if extent == "farthest-side" {
        return RadialSpec {
            cx,
            cy,
            rx: left.max(right),
            ry: top.max(bottom),
        };
    }
    let corners = [(left, top), (right, top), (left, bottom), (right, bottom)];
    let mut sorted = corners.to_vec();
    sorted.sort_by(|left, right| {
        (left.0 * left.0 + left.1 * left.1)
            .partial_cmp(&(right.0 * right.0 + right.1 * right.1))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let chosen = if extent == "closest-corner" {
        sorted[0]
    } else {
        sorted[sorted.len() - 1]
    };
    RadialSpec {
        cx,
        cy,
        rx: chosen.0.abs().max(1.0),
        ry: chosen.1.abs().max(1.0),
    }
}

#[derive(Clone, Copy)]
enum Distance {
    Percent(f32),
    Px(f32),
    Em(f32),
}

fn parse_distance_value(token: &str) -> Option<Distance> {
    let token = token.trim();
    if let Some(value) = token.strip_suffix('%') {
        return value.trim().parse::<f32>().ok().map(Distance::Percent);
    }
    if let Some(value) = token.strip_suffix("px") {
        return value.trim().parse::<f32>().ok().map(Distance::Px);
    }
    if let Some(value) = token.strip_suffix("em") {
        return value.trim().parse::<f32>().ok().map(Distance::Em);
    }
    if let Some(value) = token.strip_suffix("rem") {
        return value.trim().parse::<f32>().ok().map(Distance::Em);
    }
    None
}

fn parse_distance(token: &str, axis: f32) -> Option<f32> {
    parse_distance_value(token).map(|distance| resolve_distance(distance, axis))
}

fn resolve_distance(distance: Distance, axis: f32) -> f32 {
    match distance {
        Distance::Percent(value) => value / 100.0 * axis,
        Distance::Px(value) => value,
        Distance::Em(value) => value * 16.0,
    }
}

fn parse_stops(args: &[String], gradient_length: f32) -> Result<Vec<Stop>> {
    let gradient_length = gradient_length.max(1e-6);
    let mut raw: Vec<([f32; 4], Option<f32>)> = Vec::new();
    for argument in args {
        let tokens = split_top_level_whitespace(argument);
        if tokens.is_empty() {
            continue;
        }
        let (color_token, offset) = match tokens.last() {
            Some(last) => match parse_distance_value(last) {
                Some(distance) => (
                    tokens[..tokens.len() - 1].join(" "),
                    Some(resolve_distance(distance, gradient_length) / gradient_length),
                ),
                None => (tokens.join(" "), None),
            },
            None => continue,
        };
        let color = color::parse_color(&color_token)
            .with_context(|| format!("invalid gradient stop color '{color_token}'"))?;
        raw.push((color, offset));
    }
    if raw.is_empty() {
        bail!("gradient has no valid color stops");
    }
    let mut stops = normalize_stops(raw);
    stops.sort_by(|left, right| left.offset.partial_cmp(&right.offset).unwrap());
    Ok(stops)
}

fn normalize_stops(raw: Vec<([f32; 4], Option<f32>)>) -> Vec<Stop> {
    let mut stops = raw
        .into_iter()
        .map(|(color, offset)| Stop {
            offset: offset.unwrap_or(f32::NAN),
            color,
        })
        .collect::<Vec<_>>();
    let known = stops
        .iter()
        .enumerate()
        .filter(|(_, stop)| !stop.offset.is_nan())
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if known.is_empty() {
        let step = if stops.len() > 1 {
            1.0 / (stops.len() - 1) as f32
        } else {
            1.0
        };
        for (index, stop) in stops.iter_mut().enumerate() {
            stop.offset = step * index as f32;
        }
        return clamp_stops(stops);
    }
    let first_known = known[0];
    if first_known > 0 {
        let first_offset = stops[first_known].offset;
        for index in 0..first_known {
            stops[index].offset = first_offset * index as f32 / first_known as f32;
        }
    }
    for pair in known.windows(2) {
        let (start_index, end_index) = (pair[0], pair[1]);
        let start_offset = stops[start_index].offset;
        let end_offset = stops[end_index].offset;
        let gap = end_index - start_index;
        if gap <= 1 {
            continue;
        }
        let step = (end_offset - start_offset) / gap as f32;
        for index in 1..gap {
            stops[start_index + index].offset = start_offset + step * index as f32;
        }
    }
    let last_known = *known.last().unwrap();
    let last_offset = stops[last_known].offset;
    if last_known < stops.len() - 1 {
        let gap = stops.len() - 1 - last_known;
        let step = (1.0 - last_offset) / gap as f32;
        for index in 1..=gap {
            stops[last_known + index].offset = last_offset + step * index as f32;
        }
    }
    clamp_stops(stops)
}

fn clamp_stops(stops: Vec<Stop>) -> Vec<Stop> {
    stops
        .into_iter()
        .map(|mut stop| {
            stop.offset = stop.offset.clamp(0.0, 1.0);
            stop
        })
        .collect()
}

/// Split a string on a separator at parenthesis depth zero.
fn split_top_level(input: &str, separator: char) -> Result<Vec<String>> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    for character in input.chars() {
        match character {
            '(' => {
                depth += 1;
                current.push(character);
            }
            ')' => {
                depth -= 1;
                if depth < 0 {
                    bail!("unbalanced parentheses in '{input}'");
                }
                current.push(character);
            }
            character if character == separator && depth == 0 => {
                parts.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(character),
        }
    }
    if depth != 0 {
        bail!("unbalanced parentheses in '{input}'");
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }
    Ok(parts)
}

fn split_top_level_whitespace(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    for character in input.chars() {
        match character {
            '(' => {
                depth += 1;
                current.push(character);
            }
            ')' => {
                depth = (depth - 1).max(0);
                current.push(character);
            }
            character if character.is_whitespace() && depth == 0 => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(character),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let index = ((y * width + x) as usize) * 4;
        [
            pixels[index],
            pixels[index + 1],
            pixels[index + 2],
            pixels[index + 3],
        ]
    }

    #[test]
    fn linear_gradient_to_right() {
        let pixels =
            rasterize_background("linear-gradient(to right, #ff0000, #0000ff)", 64, 16).unwrap();
        let left = pixel(&pixels, 64, 1, 8);
        let right = pixel(&pixels, 64, 62, 8);
        assert!(left[0] > 240 && left[2] < 20, "{left:?}");
        assert!(right[2] > 240 && right[0] < 20, "{right:?}");
        let middle = pixel(&pixels, 64, 32, 8);
        assert!(middle[0] > 100 && middle[2] > 100, "{middle:?}");
    }

    #[test]
    fn radial_gradient_defaults_to_farthest_corner() {
        let pixels =
            rasterize_background("radial-gradient(circle at 50% 50%, white, black)", 64, 64)
                .unwrap();
        let center = pixel(&pixels, 64, 32, 32);
        let corner = pixel(&pixels, 64, 0, 0);
        assert!(center[0] > 240, "{center:?}");
        assert!(corner[0] < 20, "{corner:?}");
    }

    #[test]
    fn radial_gradient_supports_elliptical_extents() {
        let pixels = rasterize_background(
            "radial-gradient(ellipse 20% 40% at 50% 50%, white, black)",
            100,
            100,
        )
        .unwrap();
        let center = pixel(&pixels, 100, 50, 50);
        let horizontal = pixel(&pixels, 100, 60, 50);
        let vertical = pixel(&pixels, 100, 50, 60);
        assert!(center[0] > 240, "{center:?}");
        // 20% radius on x vs 40% on y: the same pixel offset fades faster
        // horizontally than vertically.
        assert!(horizontal[0] < 200, "{horizontal:?}");
        assert!(
            vertical[0] > horizontal[0],
            "{vertical:?} vs {horizontal:?}"
        );
    }

    #[test]
    fn stopNormalization_fills_implicit_offsets() {
        let pixels =
            rasterize_background("linear-gradient(90deg, red 20%, blue, lime 80%)", 100, 10)
                .unwrap();
        // 20% -> red, 80% -> lime, blue implicitly at 50%.
        let red = pixel(&pixels, 100, 20, 5);
        let blue = pixel(&pixels, 100, 50, 5);
        let lime = pixel(&pixels, 100, 80, 5);
        assert!(red[0] > 200 && red[1] < 30, "{red:?}");
        assert!(blue[2] > 200 && blue[0] < 30, "{blue:?}");
        assert!(lime[1] > 200 && lime[0] < 30, "{lime:?}");
    }

    #[test]
    fn multiple_layers_paint_top_first() {
        let pixels = rasterize_background(
            "linear-gradient(to right, rgba(255,0,0,0.5), rgba(255,0,0,0.5)), #0000ff",
            8,
            8,
        )
        .unwrap();
        let center = pixel(&pixels, 8, 4, 4);
        assert!(center[0] > 100 && center[2] > 100, "{center:?}");
    }

    #[test]
    fn solid_color_is_not_a_gradient() {
        assert!(!is_gradient("#ff0000"));
        assert!(is_gradient("linear-gradient(#000, #fff)"));
    }

    #[test]
    fn rgb_stops_and_angles() {
        let pixels = rasterize_background(
            "linear-gradient(180deg, rgb(255, 0, 0), rgb(0, 0, 255))",
            16,
            32,
        )
        .unwrap();
        let top = pixel(&pixels, 16, 8, 1);
        let bottom = pixel(&pixels, 16, 8, 30);
        assert!(top[0] > 240 && top[2] < 20, "{top:?}");
        assert!(bottom[2] > 240 && bottom[0] < 20, "{bottom:?}");
    }
}
