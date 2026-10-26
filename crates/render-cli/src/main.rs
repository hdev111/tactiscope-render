use anyhow::{Context, Result, bail};
use std::path::PathBuf;
use tactiscope_render::{
    ExportProgress, RenderPlan, RendererOptions, export_mp4_with_progress, plan_schema,
    render_frame_png,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(String::as_str).unwrap_or("");
    if mode == "--version" || mode == "version" {
        println!("tactiscope-render {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    if mode == "schema" {
        println!(
            "{}",
            serde_json::to_string_pretty(&plan_schema()).context("serialize schema")?
        );
        return Ok(());
    }

    let plan_path = arg_value(&args, "--plan")
        .map(PathBuf::from)
        .context("--plan is required")?;
    let plan = RenderPlan::from_file(&plan_path)?;

    if mode == "validate" {
        println!(
            "{}",
            serde_json::json!({
                "valid": true,
                "width": plan.width(),
                "height": plan.height(),
                "fps": plan.fps(),
                "durationTicks": plan.duration_ticks(),
                "totalFrames": plan.total_frames(),
            })
        );
        return Ok(());
    }

    let options = RendererOptions {
        ffmpeg_path: resolve_ffmpeg(&args),
        assets_root: arg_value(&args, "--assets").map(PathBuf::from),
        temp_dir: arg_value(&args, "--temp-dir").map(PathBuf::from),
    };

    match mode {
        "render-frame" => {
            let time_ticks = arg_value(&args, "--time-ticks")
                .context("--time-ticks is required")?
                .parse::<i64>()
                .context("invalid --time-ticks")?;
            let output = arg_value(&args, "--output")
                .map(PathBuf::from)
                .context("--output is required")?;
            render_frame_png(&plan, time_ticks, &output, &options)?;
            println!(
                "{}",
                serde_json::json!({
                    "type": "frame",
                    "path": output.display().to_string(),
                    "width": plan.width(),
                    "height": plan.height(),
                })
            );
            Ok(())
        }
        "export" => {
            let output = arg_value(&args, "--output")
                .map(PathBuf::from)
                .context("--output is required")?;
            let report = export_mp4_with_progress(&plan, &output, &options, |progress| {
                log_progress(progress);
            })?;
            println!(
                "{}",
                serde_json::json!({
                    "type": "complete",
                    "path": report.output,
                    "width": report.width,
                    "height": report.height,
                    "frames": report.frames,
                    "durationSeconds": report.duration_seconds,
                    "hasAudio": report.has_audio,
                })
            );
            Ok(())
        }
        _ => bail!(
            "usage: tactiscope-render <render-frame|export|validate|schema|version> --plan <path> ..."
        ),
    }
}

fn log_progress(progress: ExportProgress) {
    if progress.frame == 1
        || progress.frame.is_multiple_of(10)
        || progress.frame == progress.total_frames
    {
        println!(
            "{}",
            serde_json::json!({
                "type": "progress",
                "frame": progress.frame,
                "total": progress.total_frames,
                "percent": progress.percent,
            })
        );
    }
}

fn resolve_ffmpeg(args: &[String]) -> PathBuf {
    if let Some(value) = arg_value(args, "--ffmpeg") {
        return PathBuf::from(value);
    }
    if let Ok(value) = std::env::var("FFMPEG_PATH")
        && !value.trim().is_empty()
    {
        return PathBuf::from(value);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let name = if cfg!(windows) {
            "ffmpeg.exe"
        } else {
            "ffmpeg"
        };
        let candidate = dir.join(name);
        if candidate.is_file() {
            return candidate;
        }
    }
    PathBuf::from(if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    })
}

fn arg_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}
