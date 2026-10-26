use std::path::PathBuf;
use tactiscope_render::{RenderPlan, RendererOptions, export_mp4};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let plan_path = args
        .next()
        .unwrap_or_else(|| "examples/plans/solid-color.json".into());
    let output = args.next().unwrap_or_else(|| "out.mp4".into());

    let plan = RenderPlan::from_file(plan_path.as_ref())?;
    let options = RendererOptions {
        ffmpeg_path: PathBuf::from(
            std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".into()),
        ),
        ..Default::default()
    };
    let report = export_mp4(&plan, output.as_ref(), &options)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
