//! Native rasterization for graphic elements.
//!
//! Matches the graphics definitions used by TactiScope closely enough for the native
//! renderer: 512×512 source canvas, fill + stroke, center/inside/outside stroke
//! alignment, rectangle/polygon corner radius, star depth and polygon sides.
//! All params can be keyframed before they reach this module.

use anyhow::{Context, Result, bail};
use serde_json::Value;
use tiny_skia::{FillRule, Paint, Path, PathBuilder, Pixmap, Rect, Stroke, Transform};

use crate::color;

pub const GRAPHIC_SOURCE_SIZE: u32 = 512;

fn param_f32(params: &Value, key: &str, fallback: f32) -> f32 {
    params
        .get(key)
        .and_then(Value::as_f64)
        .map(|value| value as f32)
        .unwrap_or(fallback)
}

fn param_str<'a>(params: &'a Value, key: &str, fallback: &'a str) -> &'a str {
    params.get(key).and_then(Value::as_str).unwrap_or(fallback)
}

fn skia_color(rgba: [f32; 4]) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba(
        rgba[0].clamp(0.0, 1.0),
        rgba[1].clamp(0.0, 1.0),
        rgba[2].clamp(0.0, 1.0),
        rgba[3].clamp(0.0, 1.0),
    )
    .unwrap_or(tiny_skia::Color::TRANSPARENT)
}

/// Rasterize a graphic definition into straight-alpha RGBA.
pub fn render_graphic(definition_id: &str, params: &Value) -> Result<(Vec<u8>, u32, u32)> {
    let size = GRAPHIC_SOURCE_SIZE;
    let fill = color::parse_color(param_str(params, "fill", "#ffffff"))?;
    let stroke = color::parse_color(param_str(params, "stroke", "#000000"))?;
    let stroke_width = param_f32(params, "strokeWidth", 0.0).max(0.0);
    let stroke_align = match param_str(params, "strokeAlign", "center") {
        "inside" => "inside",
        "outside" => "outside",
        _ => "center",
    };
    let is_centered = stroke_align == "center";
    let inset = if is_centered { stroke_width / 2.0 } else { 0.0 };

    let path = build_graphic_path(definition_id, params, inset)?;

    let mut pixmap = Pixmap::new(size, size).context("allocate graphic pixmap")?;
    let mut fill_paint = Paint::default();
    fill_paint.set_color(skia_color(fill));
    fill_paint.anti_alias = true;
    pixmap.fill_path(
        &path,
        &fill_paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );

    if stroke_width > 0.0 && stroke[3] > 0.0 {
        // Canvas `applyAlignedStroke` doubles the stroke for inside/outside and
        // clips it against the body path. Center strokes use the real width.
        let render_width = if is_centered {
            stroke_width
        } else {
            stroke_width * 2.0
        };
        let stroke_pixels = render_stroke(&path, size, render_width, stroke)?;
        if !is_centered {
            let coverage = render_coverage(&path, size)?;
            composite_masked_stroke(&mut pixmap, &stroke_pixels, &coverage, stroke_align);
        } else {
            composite_source_over(&mut pixmap, &stroke_pixels);
        }
    }

    Ok((to_straight_rgba(pixmap.data()), size, size))
}

fn build_graphic_path(definition_id: &str, params: &Value, inset: f32) -> Result<Path> {
    let size = GRAPHIC_SOURCE_SIZE as f32;
    let center = size / 2.0;
    let mut builder = PathBuilder::new();
    match definition_id {
        "rectangle" => {
            let left = inset;
            let top = inset;
            let draw_width = (size - inset * 2.0).max(1.0);
            let draw_height = (size - inset * 2.0).max(1.0);
            let radius_percent = param_f32(params, "cornerRadius", 0.0).clamp(0.0, 50.0);
            let radius = (draw_width.min(draw_height) / 2.0) * radius_percent / 50.0;
            push_rounded_rect(&mut builder, left, top, draw_width, draw_height, radius);
        }
        "ellipse" => {
            let radius = (center - inset).max(1.0);
            let oval = Rect::from_ltrb(
                center - radius,
                center - radius,
                center + radius,
                center + radius,
            )
            .context("ellipse rect")?;
            builder.push_oval(oval);
        }
        "polygon" => {
            let sides = param_f32(params, "sides", 5.0).clamp(3.0, 12.0).round() as usize;
            let radius = (center - inset).max(1.0);
            let max_corner = radius * (std::f32::consts::PI / sides as f32).sin();
            let corner_percent = param_f32(params, "cornerRadius", 0.0).clamp(0.0, 50.0);
            let corner_radius = max_corner * corner_percent / 50.0;
            let vertices = regular_polygon(center, radius, sides);
            push_polygon(&mut builder, &vertices, corner_radius);
        }
        "star" => {
            let points = param_f32(params, "points", 5.0).clamp(3.0, 12.0).round() as usize;
            let depth = param_f32(params, "depth", 45.0).clamp(1.0, 99.0) / 100.0;
            let outer = (center - inset).max(1.0);
            let inner = outer * depth;
            let mut vertices = Vec::with_capacity(points * 2);
            for index in 0..points * 2 {
                let radius = if index % 2 == 0 { outer } else { inner };
                let angle = -std::f32::consts::FRAC_PI_2
                    + index as f32 * std::f32::consts::PI / points as f32;
                vertices.push((center + angle.cos() * radius, center + angle.sin() * radius));
            }
            push_polygon(&mut builder, &vertices, 0.0);
        }
        other => bail!("unsupported graphic '{other}'"),
    }
    builder.finish().context("finish graphic path")
}

fn regular_polygon(center: f32, radius: f32, sides: usize) -> Vec<(f32, f32)> {
    (0..sides)
        .map(|index| {
            let angle =
                -std::f32::consts::FRAC_PI_2 + index as f32 * std::f32::consts::TAU / sides as f32;
            (center + angle.cos() * radius, center + angle.sin() * radius)
        })
        .collect()
}

fn push_polygon(builder: &mut PathBuilder, vertices: &[(f32, f32)], corner_radius: f32) {
    if vertices.len() < 3 {
        return;
    }
    if corner_radius <= 0.0 {
        builder.move_to(vertices[0].0, vertices[0].1);
        for vertex in &vertices[1..] {
            builder.line_to(vertex.0, vertex.1);
        }
        builder.close();
        return;
    }
    for index in 0..vertices.len() {
        let previous = vertices[(index + vertices.len() - 1) % vertices.len()];
        let current = vertices[index];
        let next = vertices[(index + 1) % vertices.len()];
        let to_previous = normalize(previous.0 - current.0, previous.1 - current.1);
        let to_next = normalize(next.0 - current.0, next.1 - current.1);
        let cosine = (to_previous.0 * to_next.0 + to_previous.1 * to_next.1).clamp(-1.0, 1.0);
        let angle = cosine.acos();
        let max_offset = distance(previous, current).min(distance(current, next)) / 2.0;
        let tangent_offset = if angle <= 1e-4 {
            max_offset
        } else {
            (corner_radius / (angle / 2.0).tan()).min(max_offset)
        };
        let start = (
            current.0 + to_previous.0 * tangent_offset,
            current.1 + to_previous.1 * tangent_offset,
        );
        let end = (
            current.0 + to_next.0 * tangent_offset,
            current.1 + to_next.1 * tangent_offset,
        );
        if index == 0 {
            builder.move_to(start.0, start.1);
        } else {
            builder.line_to(start.0, start.1);
        }
        builder.quad_to(current.0, current.1, end.0, end.1);
    }
    builder.close();
}

fn normalize(x: f32, y: f32) -> (f32, f32) {
    let length = x.hypot(y);
    if length <= f32::EPSILON {
        (0.0, 0.0)
    } else {
        (x / length, y / length)
    }
}

fn distance(left: (f32, f32), right: (f32, f32)) -> f32 {
    (left.0 - right.0).hypot(left.1 - right.1)
}

fn push_rounded_rect(
    builder: &mut PathBuilder,
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    radius: f32,
) {
    let radius = radius.clamp(0.0, width.min(height) / 2.0);
    if radius <= 0.0 {
        if let Some(rect) = Rect::from_xywh(left, top, width, height) {
            builder.push_rect(rect);
        }
        return;
    }
    let right = left + width;
    let bottom = top + height;
    let kappa = 0.552_284_8;
    let handle = radius * kappa;
    builder.move_to(left + radius, top);
    builder.line_to(right - radius, top);
    builder.cubic_to(
        right - radius + handle,
        top,
        right,
        top + radius - handle,
        right,
        top + radius,
    );
    builder.line_to(right, bottom - radius);
    builder.cubic_to(
        right,
        bottom - radius + handle,
        right - radius + handle,
        bottom,
        right - radius,
        bottom,
    );
    builder.line_to(left + radius, bottom);
    builder.cubic_to(
        left + radius - handle,
        bottom,
        left,
        bottom - radius + handle,
        left,
        bottom - radius,
    );
    builder.line_to(left, top + radius);
    builder.cubic_to(
        left,
        top + radius - handle,
        left + radius - handle,
        top,
        left + radius,
        top,
    );
    builder.close();
}

fn render_stroke(path: &Path, size: u32, width: f32, color: [f32; 4]) -> Result<Vec<u8>> {
    let mut pixmap = Pixmap::new(size, size).context("allocate graphic stroke pixmap")?;
    let mut paint = Paint::default();
    paint.set_color(skia_color(color));
    paint.anti_alias = true;
    let stroke = Stroke {
        width: width.max(0.01),
        ..Default::default()
    };
    pixmap.stroke_path(path, &paint, &stroke, Transform::identity(), None);
    Ok(pixmap.data().to_vec())
}

fn render_coverage(path: &Path, size: u32) -> Result<Vec<u8>> {
    let mut pixmap = Pixmap::new(size, size).context("allocate graphic coverage pixmap")?;
    let mut paint = Paint::default();
    paint.set_color_rgba8(255, 255, 255, 255);
    paint.anti_alias = true;
    pixmap.fill_path(path, &paint, FillRule::Winding, Transform::identity(), None);
    Ok(pixmap.data().to_vec())
}

/// Apply the body coverage to a doubled stroke so only the requested half
/// survives, then merge it over the fill.
fn composite_masked_stroke(fill: &mut Pixmap, stroke: &[u8], coverage: &[u8], stroke_align: &str) {
    let inside = stroke_align == "inside";
    let mut masked = stroke.to_vec();
    for (pixel, body) in masked.chunks_exact_mut(4).zip(coverage.chunks_exact(4)) {
        let body_alpha = body[3] as f32 / 255.0;
        let factor = if inside { body_alpha } else { 1.0 - body_alpha };
        for channel in 0..4 {
            pixel[channel] = (pixel[channel] as f32 * factor).round() as u8;
        }
    }
    composite_source_over(fill, &masked);
}

/// Source-over composite of premultiplied RGBA over `fill`.
fn composite_source_over(fill: &mut Pixmap, source: &[u8]) {
    for (destination, pixel) in fill
        .data_mut()
        .chunks_exact_mut(4)
        .zip(source.chunks_exact(4))
    {
        let source_alpha = pixel[3] as f32 / 255.0;
        if source_alpha <= 0.0 {
            continue;
        }
        for channel in 0..3 {
            destination[channel] = (pixel[channel] as f32
                + destination[channel] as f32 * (1.0 - source_alpha))
                .round()
                .clamp(0.0, 255.0) as u8;
        }
        destination[3] = (pixel[3] as f32 + destination[3] as f32 * (1.0 - source_alpha))
            .round()
            .clamp(0.0, 255.0) as u8;
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha_at(pixels: &[u8], size: u32, x: u32, y: u32) -> u8 {
        pixels[((y * size + x) as usize) * 4 + 3]
    }

    fn rgb_at(pixels: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let index = ((y * size + x) as usize) * 4;
        [
            pixels[index],
            pixels[index + 1],
            pixels[index + 2],
            pixels[index + 3],
        ]
    }

    #[test]
    fn rectangle_corner_radius_is_applied() {
        let sharp = serde_json::json!({
            "fill": "#ff0000",
            "stroke": "#000000",
            "strokeWidth": 0,
            "cornerRadius": 0
        });
        let rounded = serde_json::json!({
            "fill": "#ff0000",
            "stroke": "#000000",
            "strokeWidth": 0,
            "cornerRadius": 50
        });
        let (sharp_pixels, size, _) = render_graphic("rectangle", &sharp).unwrap();
        let (rounded_pixels, _, _) = render_graphic("rectangle", &rounded).unwrap();
        assert_eq!(alpha_at(&sharp_pixels, size, 0, 0), 255);
        assert_eq!(alpha_at(&rounded_pixels, size, 0, 0), 0);
        assert_eq!(alpha_at(&rounded_pixels, size, size / 2, size / 2), 255);
    }

    #[test]
    fn polygon_sides_and_corner_radius_change_geometry() {
        let triangle = serde_json::json!({
            "fill": "#ffffff",
            "strokeWidth": 0,
            "sides": 3,
            "cornerRadius": 0
        });
        let dodecagon = serde_json::json!({
            "fill": "#ffffff",
            "strokeWidth": 0,
            "sides": 12,
            "cornerRadius": 0
        });
        let rounded_square = serde_json::json!({
            "fill": "#ffffff",
            "strokeWidth": 0,
            "sides": 4,
            "cornerRadius": 50
        });
        let square = serde_json::json!({
            "fill": "#ffffff",
            "strokeWidth": 0,
            "sides": 4,
            "cornerRadius": 0
        });
        let count = |pixels: &[u8]| {
            pixels
                .chunks_exact(4)
                .filter(|pixel| pixel[3] > 128)
                .count()
        };
        let (triangle_pixels, _, _) = render_graphic("polygon", &triangle).unwrap();
        let (dodecagon_pixels, _, _) = render_graphic("polygon", &dodecagon).unwrap();
        let (square_pixels, _, _) = render_graphic("polygon", &square).unwrap();
        let (rounded_pixels, _, _) = render_graphic("polygon", &rounded_square).unwrap();
        assert!(count(&triangle_pixels) < count(&dodecagon_pixels));
        assert!(count(&rounded_pixels) < count(&square_pixels));
    }

    #[test]
    fn stroke_alignment_controls_the_outer_half() {
        let base = serde_json::json!({
            "fill": "#ffffff",
            "stroke": "#ff0000",
            "strokeWidth": 40,
            "cornerRadius": 20
        });
        let mut inside = base.clone();
        inside["strokeAlign"] = serde_json::json!("inside");
        let mut outside = base;
        outside["strokeAlign"] = serde_json::json!("outside");
        let (inside_pixels, size, _) = render_graphic("rectangle", &inside).unwrap();
        let (outside_pixels, _, _) = render_graphic("rectangle", &outside).unwrap();
        // With a 102px corner radius the corner pocket around (10, 10) is
        // outside the body; the outside stroke fills it.
        assert_eq!(alpha_at(&inside_pixels, size, 10, 10), 0);
        assert!(alpha_at(&outside_pixels, size, 10, 10) > 0);
        assert!(rgb_at(&outside_pixels, size, 10, 10)[0] > 200);
        // Inside strokes stay inside the body.
        assert!(rgb_at(&inside_pixels, size, 128, 128)[0] > 200);
    }
}
