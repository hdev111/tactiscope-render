//! Native audio mixing + mastering for the render service.
//!
//! FFmpeg handles decode/resample/tempo; the clip graph, keyframed volume and
//! the mastering chain are native. No WebAudio is involved.

use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::{AudioElementPlan, RenderPlan, TICKS_PER_SECOND, animation};

const MASTER_LIMITER_THRESHOLD_DB: f32 = -1.0;
const MASTER_LIMITER_RATIO: f32 = 20.0;
const MASTER_LIMITER_ATTACK_SECONDS: f32 = 0.001;
const MASTER_LIMITER_RELEASE_SECONDS: f32 = 0.12;
const MASTER_OUTPUT_HEADROOM: f32 = 0.98;
pub(crate) const MASTER_SAMPLE_RATE: u32 = 48_000;
const VOLUME_DB_MIN: f32 = -60.0;
const VOLUME_DB_MAX: f32 = 20.0;
const MAX_VOLUME_KNOTS: usize = 1024;
const MAX_AUDIO_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;
const AUDIO_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const AUDIO_READ_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy)]
pub struct MasteringReport {
    pub peak_before: f32,
    pub peak_after: f32,
    pub applied: bool,
}

fn clamp_db(value: f32) -> f32 {
    if !value.is_finite() {
        return 0.0;
    }
    value.clamp(VOLUME_DB_MIN, VOLUME_DB_MAX)
}

fn db_to_linear(db: f32) -> f32 {
    10f32.powf(clamp_db(db) / 20.0)
}

fn temporary_path_in(label: &str, directory: Option<&Path>) -> PathBuf {
    let base = directory
        .map(Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir);
    let _ = std::fs::create_dir_all(&base);
    base.join(format!(
        "tactiscope-{label}-{}-{}.wav",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ))
}

/// Build an FFmpeg `volume` expression (evaluated per audio frame) for a
/// keyframed volume channel. Returns `None` when the channel is static.
pub fn volume_expression(
    animations: Option<&Value>,
    duration_ticks: i64,
    base_db: f32,
) -> Option<String> {
    let channel = animation::channel(animations, "volume")?;
    let keys = channel.get("keys")?.as_array()?;
    if keys.is_empty() {
        return None;
    }
    let mut knots = BTreeSet::new();
    knots.insert(0i64);
    knots.insert(duration_ticks.max(0));
    for key in keys {
        if let Some(time) = key.get("time").and_then(Value::as_i64) {
            knots.insert(time.clamp(0, duration_ticks.max(0)));
        }
        let segment = key
            .get("segmentToNext")
            .and_then(Value::as_str)
            .unwrap_or("linear");
        if segment == "step"
            && let Some(time) = key.get("time").and_then(Value::as_i64)
        {
            knots.insert((time - 1).clamp(0, duration_ticks.max(0)));
        }
        if segment == "bezier"
            && let (Some(start), Some(end)) = (
                key.get("time").and_then(Value::as_i64),
                keys.iter()
                    .filter_map(|other| other.get("time").and_then(Value::as_i64))
                    .filter(|time| *time > key.get("time").and_then(Value::as_i64).unwrap_or(0))
                    .min(),
            )
        {
            let span = end - start;
            if span > 0 {
                let steps = 24;
                for step in 1..steps {
                    knots.insert(start + span * step / steps);
                }
            }
        }
    }
    let mut knot_times = knots.into_iter().collect::<Vec<_>>();
    if knot_times.len() > MAX_VOLUME_KNOTS {
        let duration = duration_ticks.max(1);
        let count = MAX_VOLUME_KNOTS;
        knot_times = (0..count)
            .map(|index| duration * index as i64 / (count as i64 - 1))
            .collect();
    }
    let mut points = Vec::with_capacity(knot_times.len());
    for time in knot_times {
        let db = clamp_db(animation::scalar(animations, "volume", time, base_db));
        points.push((time as f64 / TICKS_PER_SECOND, db));
    }
    points.dedup_by(|left, right| (left.0 - right.0).abs() < 1e-9);
    let first = points.first()?.1;
    if points.iter().all(|point| (point.1 - first).abs() < 1e-6) {
        return None;
    }
    let mut expression = format!("{:.6}", db_to_linear(points[points.len() - 1].1));
    for index in (0..points.len() - 1).rev() {
        let (start_time, start_db) = points[index];
        let (end_time, end_db) = points[index + 1];
        let span = end_time - start_time;
        let segment = if span <= 0.0 {
            format!("{:.6}", db_to_linear(end_db))
        } else {
            format!(
                "pow(10,({start_db:.6}+({end_db:.6}-{start_db:.6})*(t-{start_time:.6})/{span:.6})/20)"
            )
        };
        expression = format!("if(lt(t,{end_time:.6}),{segment},{expression})");
    }
    Some(expression)
}

fn element_filter(element: &AudioElementPlan, index: usize) -> String {
    let rate = element
        .retime
        .as_ref()
        .map(|retime| retime.rate.clamp(0.01, 5.0))
        .unwrap_or(1.0);
    let source_span = element.duration_ticks as f64 / TICKS_PER_SECOND * f64::from(rate);
    let start = element.trim_start_ticks as f64 / TICKS_PER_SECOND;
    let end = start + source_span;
    let start_ms = (element.start_ticks as f64 / TICKS_PER_SECOND * 1000.0).max(0.0) as u64;
    let mut chain = format!("[{index}:a]atrim=start={start:.6}:end={end:.6},asetpts=PTS-STARTPTS");
    if (rate - 1.0).abs() > 1e-4 {
        if element
            .retime
            .as_ref()
            .map(|retime| retime.maintain_pitch)
            .unwrap_or(false)
        {
            let mut remaining = rate;
            while remaining > 2.0 {
                chain.push_str(",atempo=2.0");
                remaining /= 2.0;
            }
            while remaining < 0.5 {
                chain.push_str(",atempo=0.5");
                remaining /= 0.5;
            }
            chain.push_str(&format!(",atempo={remaining:.6}"));
        } else {
            chain.push_str(&format!(
                ",asetrate={:.0},aresample={MASTER_SAMPLE_RATE}",
                MASTER_SAMPLE_RATE as f32 * rate
            ));
        }
    }
    if element.muted {
        chain.push_str(",volume=0");
    } else if let Some(expression) = volume_expression(
        element.animations.as_ref(),
        element.duration_ticks,
        element.volume_db,
    ) {
        chain.push_str(&format!(",volume=volume='{expression}':eval=frame"));
    } else {
        chain.push_str(&format!(",volume={:.6}", db_to_linear(element.volume_db)));
    }
    chain.push_str(&format!(
        ",adelay={start_ms}|{start_ms},aformat=sample_fmts=fltp:sample_rates={MASTER_SAMPLE_RATE}:channel_layouts=stereo[a{index}]"
    ));
    chain
}

/// Removes downloaded audio sources when the mix finishes, fails or panics.
#[derive(Default)]
struct DownloadedAudio(Vec<PathBuf>);

impl Drop for DownloadedAudio {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn download_audio_source(url: &str) -> Result<PathBuf> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        bail!("unsupported audio source URL (expected http or https): {url}");
    }
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(AUDIO_CONNECT_TIMEOUT)
        .timeout_read(AUDIO_READ_TIMEOUT)
        .build();
    let response = agent
        .get(url)
        .call()
        .with_context(|| format!("download audio source {url}"))?;
    let path = std::env::temp_dir().join(format!(
        "tactiscope-audio-source-{}-{}.audio",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    let mut file = std::fs::File::create(&path)
        .with_context(|| format!("create audio source {}", path.display()))?;
    let copied = std::io::copy(
        &mut response
            .into_reader()
            .take(MAX_AUDIO_DOWNLOAD_BYTES.saturating_add(1)),
        &mut file,
    )
    .with_context(|| format!("write audio source {}", path.display()));
    if let Err(error) = copied {
        let _ = std::fs::remove_file(&path);
        return Err(error);
    }
    if file.metadata().map(|meta| meta.len()).unwrap_or(0) > MAX_AUDIO_DOWNLOAD_BYTES {
        let _ = std::fs::remove_file(&path);
        bail!(
            "audio source exceeds the {} MiB download limit: {url}",
            MAX_AUDIO_DOWNLOAD_BYTES / (1024 * 1024)
        );
    }
    Ok(path)
}

/// Mix every audio plan element into a stereo 48 kHz s16 WAV, then apply the
/// mastering chain unless the mix already sits below the headroom target.
/// Mix and master the timeline audio into `directory` (or the system temp
/// directory when omitted). Preview sessions pass their own temp directory so
/// The caller can remove the WAV with the rest of its session.
pub fn mix_audio_in(
    plan: &RenderPlan,
    ffmpeg: &str,
    directory: Option<&Path>,
) -> Result<Option<PathBuf>> {
    if plan.audio.elements.is_empty() {
        return Ok(None);
    }
    let mut args: Vec<String> = vec!["-y".into(), "-v".into(), "error".into()];
    let mut downloaded = DownloadedAudio::default();
    for element in &plan.audio.elements {
        let source = if let Some(path) = element.source_path.as_ref() {
            PathBuf::from(path)
        } else if let Some(url) = element.source_url.as_ref() {
            let path = download_audio_source(url)?;
            downloaded.0.push(path.clone());
            path
        } else {
            bail!("audio element has neither a source path nor a source URL");
        };
        args.push("-i".into());
        args.push(source.display().to_string());
    }
    let mut filters = Vec::new();
    let mut labels = Vec::new();
    for (index, element) in plan.audio.elements.iter().enumerate() {
        filters.push(element_filter(element, index));
        labels.push(format!("[a{index}]"));
    }
    filters.push(format!(
        "{}amix=inputs={}:duration=longest:normalize=0[aout]",
        labels.join(""),
        plan.audio.elements.len()
    ));
    let output = temporary_path_in("audio", directory);
    args.extend([
        "-filter_complex".into(),
        filters.join(";"),
        "-map".into(),
        "[aout]".into(),
        "-t".into(),
        format!("{:.6}", plan.duration_ticks as f64 / TICKS_PER_SECOND),
        "-c:a".into(),
        "pcm_s16le".into(),
        "-ar".into(),
        MASTER_SAMPLE_RATE.to_string(),
        "-ac".into(),
        "2".into(),
        output.display().to_string(),
    ]);
    let result = Command::new(ffmpeg)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("mix audio with {ffmpeg}"))?;
    if !result.status.success() {
        let _ = std::fs::remove_file(&output);
        bail!(
            "audio mix failed: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        );
    }
    let report = match master_wav(&output) {
        Ok(report) => report,
        Err(error) => {
            let _ = std::fs::remove_file(&output);
            return Err(error);
        }
    };
    if report.applied {
        eprintln!(
            "[native-audio] mastering applied: peak {:.4} -> {:.4}",
            report.peak_before, report.peak_after
        );
    }
    Ok(Some(output))
}

/// Apply the native mastering chain in place when the mix peaks above the
/// output headroom. Matches the mastering curve used by TactiScope:
/// peak gate, -1 dB / 20:1 compressor, 0.98 output gain, clamp to +/-0.98.
pub fn master_wav(path: &Path) -> Result<MasteringReport> {
    let mut file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut data = Vec::new();
    file.read_to_end(&mut data)
        .with_context(|| format!("read {}", path.display()))?;
    let (channels, data_start, data_len) = parse_wav_layout(&data)?;
    if channels == 0 {
        bail!("audio mix has no channels");
    }
    let peak_before = peak_of(&data[data_start..data_start + data_len]);
    if peak_before <= MASTER_OUTPUT_HEADROOM {
        return Ok(MasteringReport {
            peak_before,
            peak_after: peak_before,
            applied: false,
        });
    }
    let threshold = db_to_linear(MASTER_LIMITER_THRESHOLD_DB);
    let attack_coefficient = envelope_coefficient(MASTER_LIMITER_ATTACK_SECONDS);
    let release_coefficient = envelope_coefficient(MASTER_LIMITER_RELEASE_SECONDS);
    let step = MASTER_LIMITER_RATIO.recip();
    let mut reduction_db = 0.0f32;
    for frame in data[data_start..data_start + data_len].chunks_exact_mut(channels as usize * 2) {
        let mut level = 0.0f32;
        for channel in 0..channels as usize {
            let sample = i16::from_le_bytes([frame[channel * 2], frame[channel * 2 + 1]]);
            level = level.max((sample as f32 / 32768.0).abs());
        }
        let target_reduction = if level > threshold {
            let level_db = 20.0 * level.log10();
            let threshold_db = 20.0 * threshold.log10();
            -(level_db - threshold_db) * (1.0 - step)
        } else {
            0.0
        };
        let coefficient = if target_reduction < reduction_db {
            attack_coefficient
        } else {
            release_coefficient
        };
        reduction_db = target_reduction + (reduction_db - target_reduction) * coefficient;
        let gain = 10f32.powf(reduction_db / 20.0) * MASTER_OUTPUT_HEADROOM;
        for channel in 0..channels as usize {
            let sample = i16::from_le_bytes([frame[channel * 2], frame[channel * 2 + 1]]);
            let processed = ((sample as f32 / 32768.0) * gain)
                .clamp(-MASTER_OUTPUT_HEADROOM, MASTER_OUTPUT_HEADROOM);
            let encoded = (processed * 32767.0).round() as i16;
            frame[channel * 2..channel * 2 + 2].copy_from_slice(&encoded.to_le_bytes());
        }
    }
    std::fs::write(path, &data).with_context(|| format!("write {}", path.display()))?;
    Ok(MasteringReport {
        peak_before,
        peak_after: peak_of(&data[data_start..data_start + data_len]),
        applied: true,
    })
}

fn envelope_coefficient(seconds: f32) -> f32 {
    if seconds <= 0.0 {
        return 0.0;
    }
    (-1.0 / (seconds * MASTER_SAMPLE_RATE as f32)).exp()
}

fn peak_of(samples: &[u8]) -> f32 {
    samples
        .chunks_exact(2)
        .map(|sample| i16::from_le_bytes([sample[0], sample[1]]))
        .map(|sample| (sample as f32 / 32768.0).abs())
        .fold(0.0f32, f32::max)
}

pub(crate) fn parse_wav_layout(data: &[u8]) -> Result<(u16, usize, usize)> {
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        bail!("not a RIFF/WAVE file");
    }
    let mut offset = 12;
    let mut channels = 0u16;
    let mut data_start = None;
    while offset + 8 <= data.len() {
        let id = &data[offset..offset + 4];
        let size = u32::from_le_bytes([
            data[offset + 4],
            data[offset + 5],
            data[offset + 6],
            data[offset + 7],
        ]) as usize;
        let body = offset + 8;
        if body + size > data.len() {
            break;
        }
        if id == b"fmt " && size >= 16 {
            let audio_format = u16::from_le_bytes([data[body], data[body + 1]]);
            channels = u16::from_le_bytes([data[body + 2], data[body + 3]]);
            let bits = u16::from_le_bytes([data[body + 14], data[body + 15]]);
            if audio_format != 1 || bits != 16 {
                bail!("unsupported WAV format (need 16-bit PCM)");
            }
        }
        if id == b"data" {
            data_start = Some((body, size));
        }
        offset = body + size + (size % 2);
    }
    let (start, len) = data_start.context("WAV file has no data chunk")?;
    Ok((channels, start, len))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write_wav(path: &Path, samples: &[i16]) {
        let mut data = Vec::new();
        data.extend_from_slice(b"RIFF");
        data.extend_from_slice(&(36u32 + (samples.len() * 2) as u32).to_le_bytes());
        data.extend_from_slice(b"WAVE");
        data.extend_from_slice(b"fmt ");
        data.extend_from_slice(&16u32.to_le_bytes());
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend_from_slice(&2u16.to_le_bytes());
        data.extend_from_slice(&48000u32.to_le_bytes());
        data.extend_from_slice(&(48000u32 * 4).to_le_bytes());
        data.extend_from_slice(&4u16.to_le_bytes());
        data.extend_from_slice(&16u16.to_le_bytes());
        data.extend_from_slice(b"data");
        data.extend_from_slice(&((samples.len() * 2) as u32).to_le_bytes());
        for sample in samples {
            data.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(path, data).unwrap();
    }

    #[test]
    fn volume_expression_interpolates_db_between_keys() {
        let animations = json!({
            "volume": { "keys": [
                { "time": 0, "value": -60.0, "segmentToNext": "linear" },
                { "time": 240000, "value": 0.0, "segmentToNext": "linear" }
            ] }
        });
        let expression = volume_expression(Some(&animations), 240_000, 0.0).unwrap();
        assert!(
            expression.starts_with("if(lt(t,2.000000),pow(10,"),
            "{expression}"
        );
        assert!(expression.contains("t-0.000000"));
    }

    #[test]
    fn static_volume_returns_no_expression() {
        let animations = json!({
            "volume": { "keys": [{ "time": 0, "value": -6.0, "segmentToNext": "linear" }] }
        });
        assert!(volume_expression(Some(&animations), 240_000, 0.0).is_none());
        assert!(volume_expression(None, 240_000, 0.0).is_none());
    }

    #[test]
    fn download_rejects_non_http_sources() {
        let error = download_audio_source("file:///etc/passwd").unwrap_err();
        assert!(error.to_string().contains("unsupported audio source URL"));
    }

    #[test]
    fn mastering_clamps_hot_mixes() {
        let path =
            std::env::temp_dir().join(format!("tactiscope-master-test-{}.wav", std::process::id()));
        let mut samples = Vec::new();
        for index in 0..48_000 {
            let value = ((index as f32 / 48_000.0) * 1000.0 * std::f32::consts::TAU).sin();
            let sample = (value * 1.2 * 32767.0) as i16;
            samples.push(sample);
            samples.push(sample);
        }
        write_wav(&path, &samples);
        let report = master_wav(&path).unwrap();
        assert!(report.applied);
        assert!(report.peak_before > 0.98);
        assert!(report.peak_after <= 0.981, "{}", report.peak_after);
        assert!(report.peak_after <= 0.98);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn mastering_keeps_quiet_mixes_untouched() {
        let path = std::env::temp_dir().join(format!(
            "tactiscope-master-quiet-test-{}.wav",
            std::process::id()
        ));
        let mut samples = Vec::new();
        for index in 0..4800 {
            let value = ((index as f32 / 48_000.0) * 1000.0 * std::f32::consts::TAU).sin();
            let sample = (value * 0.5 * 32767.0) as i16;
            samples.push(sample);
            samples.push(sample);
        }
        write_wav(&path, &samples);
        let report = master_wav(&path).unwrap();
        assert!(!report.applied);
        assert!((report.peak_after - report.peak_before).abs() < 1e-5);
        let _ = std::fs::remove_file(&path);
    }
}
