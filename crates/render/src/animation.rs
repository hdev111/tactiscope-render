//! Keyframe evaluation for the native renderer.
//!
//! The web app stores animations as plain JSON (`ElementAnimations`): scalar
//! channels have `{ keys: [{ time, value, segmentToNext, handles... }],
//! extrapolation? }`, color channels are composite `{ r, g, b, a }` objects of
//! scalar channels, discrete channels are `{ keys: [{ time, value }] }`.
//! This module matches the animation model used by TactiScope.

use serde_json::Value;

use crate::color;

pub fn channel<'a>(animations: Option<&'a Value>, path: &str) -> Option<&'a Value> {
    animations.and_then(|value| value.get(path))
}

fn has_keys(channel: &Value) -> bool {
    channel
        .get("keys")
        .and_then(Value::as_array)
        .map(|keys| !keys.is_empty())
        .unwrap_or(false)
}

fn is_scalar_channel(channel: &Value) -> bool {
    if channel.get("extrapolation").is_some() {
        return true;
    }
    channel
        .get("keys")
        .and_then(Value::as_array)
        .map(|keys| keys.iter().any(|key| key.get("segmentToNext").is_some()))
        .unwrap_or(false)
}

fn is_discrete_channel(channel: &Value) -> bool {
    has_keys(channel) && !is_scalar_channel(channel)
}

fn is_color_channel(channel: &Value) -> bool {
    channel.get("r").is_some()
        && channel.get("g").is_some()
        && channel.get("b").is_some()
        && channel.get("a").is_some()
}

/// Resolve a scalar channel. Identical math to `getScalarChannelValueAtTime`.
pub fn scalar(animations: Option<&Value>, path: &str, local_ticks: i64, fallback: f32) -> f32 {
    scalar_channel(channel(animations, path), local_ticks, fallback)
}

fn scalar_channel(channel: Option<&Value>, local_ticks: i64, fallback: f32) -> f32 {
    let Some(channel) = channel else {
        return fallback;
    };
    if !is_scalar_channel(channel) {
        return fallback;
    }
    let Some(keys) = channel.get("keys").and_then(Value::as_array) else {
        return fallback;
    };
    if keys.is_empty() {
        return fallback;
    }
    let mut points = keys
        .iter()
        .filter_map(|key| {
            Some((
                key.get("time")?.as_i64()?,
                key.get("value")?.as_f64()? as f32,
                key,
            ))
        })
        .collect::<Vec<_>>();
    points.sort_by_key(|point| point.0);
    let time = local_ticks as f32;
    let first = points[0];
    let last = points[points.len() - 1];
    let extrapolation = channel.get("extrapolation");
    let before = extrapolation
        .and_then(|value| value.get("before"))
        .and_then(Value::as_str)
        .unwrap_or("hold");
    let after = extrapolation
        .and_then(|value| value.get("after"))
        .and_then(Value::as_str)
        .unwrap_or("hold");
    if time < first.0 as f32 {
        if before == "linear" && points.len() > 1 {
            let next = points[1];
            let span = (next.0 - first.0) as f32;
            if span > 0.0 {
                return first.1 + ((time - first.0 as f32) / span) * (next.1 - first.1);
            }
        }
        return first.1;
    }
    if time > last.0 as f32 {
        if after == "linear" && points.len() > 1 {
            let previous = points[points.len() - 2];
            let span = (last.0 - previous.0) as f32;
            if span > 0.0 {
                return last.1 + ((time - last.0 as f32) / span) * (last.1 - previous.1);
            }
        }
        return last.1;
    }
    for index in 0..points.len() - 1 {
        let (left_time, left_value, left_key) = points[index];
        let (right_time, right_value, right_key) = points[index + 1];
        if (time - right_time as f32).abs() < 1e-6 {
            return right_value;
        }
        if time < left_time as f32 || time > right_time as f32 {
            continue;
        }
        let segment = left_key
            .get("segmentToNext")
            .and_then(Value::as_str)
            .unwrap_or("linear");
        if segment == "step" {
            return left_value;
        }
        let span = (right_time - left_time) as f32;
        if span <= 0.0 {
            return right_value;
        }
        if segment != "bezier" {
            let progress = ((time - left_time as f32) / span).clamp(0.0, 1.0);
            return left_value + (right_value - left_value) * progress;
        }
        let right_handle = left_key
            .get("rightHandle")
            .map(|handle| {
                (
                    handle
                        .get("dt")
                        .and_then(Value::as_f64)
                        .unwrap_or(span as f64 / 3.0) as f32,
                    handle.get("dv").and_then(Value::as_f64).unwrap_or(0.0) as f32,
                )
            })
            .unwrap_or((span / 3.0, (right_value - left_value) / 3.0));
        let left_handle = right_key
            .get("leftHandle")
            .map(|handle| {
                (
                    handle
                        .get("dt")
                        .and_then(Value::as_f64)
                        .unwrap_or(-span as f64 / 3.0) as f32,
                    handle.get("dv").and_then(Value::as_f64).unwrap_or(0.0) as f32,
                )
            })
            .unwrap_or((-span / 3.0, -(right_value - left_value) / 3.0));
        let progress = solve_bezier_time(
            time,
            left_time as f32,
            left_time as f32 + right_handle.0,
            right_time as f32 + left_handle.0,
            right_time as f32,
        );
        return cubic_bezier(
            progress,
            left_value,
            left_value + right_handle.1,
            right_value + left_handle.1,
            right_value,
        );
    }
    last.1
}

fn discrete_channel(channel: Option<&Value>, local_ticks: i64, fallback: &str) -> String {
    let Some(channel) = channel else {
        return fallback.to_string();
    };
    let Some(keys) = channel.get("keys").and_then(Value::as_array) else {
        return fallback.to_string();
    };
    let mut points = keys
        .iter()
        .filter_map(|key| {
            Some((
                key.get("time")?.as_i64()?,
                key.get("value")?.as_str()?.to_string(),
            ))
        })
        .collect::<Vec<_>>();
    if points.is_empty() {
        return fallback.to_string();
    }
    points.sort_by_key(|point| point.0);
    let mut current = points[0].1.clone();
    for (time, value) in points {
        if (time as f32) <= local_ticks as f32 {
            current = value;
        }
    }
    current
}

/// Resolve a color channel (composite linear-RGBA components) to sRGB + alpha.
pub fn color_rgba(
    animations: Option<&Value>,
    path: &str,
    local_ticks: i64,
    fallback: &str,
) -> [f32; 4] {
    color_channel(channel(animations, path), local_ticks, fallback)
}

fn color_channel(channel: Option<&Value>, local_ticks: i64, fallback: &str) -> [f32; 4] {
    let fallback_rgba = color::parse_color(fallback).unwrap_or([1.0, 1.0, 1.0, 1.0]);
    let Some(channel) = channel else {
        return fallback_rgba;
    };
    if is_discrete_channel(channel) {
        let next = discrete_channel(Some(channel), local_ticks, fallback);
        return color::parse_color(&next).unwrap_or(fallback_rgba);
    }
    if !is_color_channel(channel) {
        return fallback_rgba;
    }
    let fallback_linear = color::to_linear(fallback_rgba);
    let component =
        |key: &str, value: f32| -> f32 { scalar_channel(channel.get(key), local_ticks, value) };
    color::from_linear([
        component("r", fallback_linear[0]),
        component("g", fallback_linear[1]),
        component("b", fallback_linear[2]),
        component("a", fallback_linear[3]),
    ])
}

/// Resolve graphic params at a local time. Returns a new JSON object; keys
/// without an animation channel keep their original value.
pub fn resolve_params(animations: Option<&Value>, local_ticks: i64, params: &Value) -> Value {
    let Some(object) = params.as_object() else {
        return params.clone();
    };
    let mut resolved = object.clone();
    for (key, fallback) in object {
        let Some(channel) = channel(animations, &format!("params.{key}")) else {
            continue;
        };
        if is_color_channel(channel) {
            let fallback = fallback.as_str().unwrap_or("#ffffff");
            let rgba = color_channel(Some(channel), local_ticks, fallback);
            resolved.insert(key.clone(), Value::String(color::format_hex(rgba)));
            continue;
        }
        if is_discrete_channel(channel) {
            if let Some(fallback) = fallback.as_str() {
                let value = discrete_channel(Some(channel), local_ticks, fallback);
                resolved.insert(key.clone(), Value::String(value));
            }
            continue;
        }
        if let Some(fallback) = fallback.as_f64() {
            let value = scalar_channel(Some(channel), local_ticks, fallback as f32);
            if let Some(number) = serde_json::Number::from_f64(value as f64) {
                resolved.insert(key.clone(), Value::Number(number));
            }
        }
    }
    Value::Object(resolved)
}

fn cubic_bezier(progress: f32, p0: f32, p1: f32, p2: f32, p3: f32) -> f32 {
    let mt = 1.0 - progress;
    mt * mt * mt * p0
        + 3.0 * mt * mt * progress * p1
        + 3.0 * mt * progress * progress * p2
        + progress * progress * progress * p3
}

fn solve_bezier_time(time: f32, p0: f32, p1: f32, p2: f32, p3: f32) -> f32 {
    let mut lower = 0.0;
    let mut upper = 1.0;
    for _ in 0..20 {
        let mid = (lower + upper) / 2.0;
        let estimate = cubic_bezier(mid, p0, p1, p2, p3);
        if estimate < time {
            lower = mid;
        } else {
            upper = mid;
        }
    }
    (lower + upper) / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn scalar_hold_linear_and_extrapolation() {
        let animations = json!({
            "opacity": {
                "keys": [
                    { "time": 0, "value": 0.0, "segmentToNext": "linear" },
                    { "time": 1000, "value": 1.0, "segmentToNext": "linear" }
                ],
                "extrapolation": { "before": "linear", "after": "hold" }
            }
        });
        assert!((scalar(Some(&animations), "opacity", 500, 0.0) - 0.5).abs() < 1e-5);
        assert!((scalar(Some(&animations), "opacity", 2000, 0.0) - 1.0).abs() < 1e-5);
        assert!((scalar(Some(&animations), "opacity", -500, 0.0) + 0.5).abs() < 1e-5);
        assert_eq!(scalar(Some(&animations), "missing", 500, 7.0), 7.0);
    }

    #[test]
    fn scalar_step_channel() {
        let animations = json!({
            "opacity": {
                "keys": [
                    { "time": 0, "value": 0.25, "segmentToNext": "step" },
                    { "time": 1000, "value": 1.0, "segmentToNext": "linear" }
                ]
            }
        });
        assert!((scalar(Some(&animations), "opacity", 900, 0.0) - 0.25).abs() < 1e-5);
        assert!((scalar(Some(&animations), "opacity", 1000, 0.0) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn color_channel_interpolates_in_linear_space() {
        let animations = json!({
            "color": {
                "r": { "keys": [
                    { "time": 0, "value": 0.0, "segmentToNext": "linear" },
                    { "time": 1000, "value": 1.0, "segmentToNext": "linear" }
                ] },
                "g": { "keys": [
                    { "time": 0, "value": 0.0, "segmentToNext": "linear" },
                    { "time": 1000, "value": 1.0, "segmentToNext": "linear" }
                ] },
                "b": { "keys": [
                    { "time": 0, "value": 0.0, "segmentToNext": "linear" },
                    { "time": 1000, "value": 1.0, "segmentToNext": "linear" }
                ] },
                "a": { "keys": [
                    { "time": 0, "value": 1.0, "segmentToNext": "linear" },
                    { "time": 1000, "value": 1.0, "segmentToNext": "linear" }
                ] }
            }
        });
        let black = color_rgba(Some(&animations), "color", 0, "#00ff00");
        assert!(black[0] < 1e-4 && black[1] < 1e-4 && black[2] < 1e-4);
        let mid = color_rgba(Some(&animations), "color", 500, "#000000");
        assert!((mid[0] - 0.7354).abs() < 1e-3, "{mid:?}");
        assert!((mid[1] - 0.7354).abs() < 1e-3, "{mid:?}");
    }

    #[test]
    fn resolves_graphic_param_animations() {
        let animations = json!({
            "params.sides": {
                "keys": [
                    { "time": 0, "value": 3, "segmentToNext": "linear" },
                    { "time": 1000, "value": 8, "segmentToNext": "linear" }
                ]
            },
            "params.fill": {
                "r": { "keys": [{ "time": 0, "value": 1, "segmentToNext": "linear" }] },
                "g": { "keys": [{ "time": 0, "value": 0, "segmentToNext": "linear" }] },
                "b": { "keys": [{ "time": 0, "value": 0, "segmentToNext": "linear" }] },
                "a": { "keys": [{ "time": 0, "value": 1, "segmentToNext": "linear" }] }
            }
        });
        let params = json!({ "sides": 5, "fill": "#00ff00", "stroke": "#000000" });
        let resolved = resolve_params(Some(&animations), 500, &params);
        assert!((resolved["sides"].as_f64().unwrap() - 5.5).abs() < 1e-5);
        assert_eq!(resolved["fill"].as_str().unwrap(), "#ff0000");
        assert_eq!(resolved["stroke"].as_str().unwrap(), "#000000");
    }

    #[test]
    fn discrete_numeric_channels_hold_the_static_value() {
        let animations = json!({
            "params.strokeWidth": {
                "keys": [
                    { "time": 0, "value": 1 },
                    { "time": 1000, "value": 9 }
                ]
            }
        });
        let params = json!({ "strokeWidth": 4 });
        let resolved = resolve_params(Some(&animations), 500, &params);
        assert_eq!(resolved["strokeWidth"].as_f64().unwrap(), 4.0);
    }

    #[test]
    fn discrete_string_channels_step() {
        let animations = json!({
            "params.strokeAlign": {
                "keys": [
                    { "time": 0, "value": "inside" },
                    { "time": 1000, "value": "outside" }
                ]
            }
        });
        let params = json!({ "strokeAlign": "center" });
        let inside = resolve_params(Some(&animations), 500, &params);
        let outside = resolve_params(Some(&animations), 1500, &params);
        assert_eq!(inside["strokeAlign"].as_str().unwrap(), "inside");
        assert_eq!(outside["strokeAlign"].as_str().unwrap(), "outside");
    }
}
