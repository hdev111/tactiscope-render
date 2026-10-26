use std::path::PathBuf;
use tactiscope_render::{RenderPlan, Renderer, RendererOptions};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let plan_path = args
        .next()
        .unwrap_or_else(|| "examples/plans/solid-color.json".into());
    let time_ticks = args
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let output = args.next().unwrap_or_else(|| "frame.png".into());

    let plan = RenderPlan::from_file(plan_path.as_ref())?;
    let options = RendererOptions {
        ffmpeg_path: PathBuf::from(
            std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".into()),
        ),
        ..Default::default()
    };
    let mut renderer = Renderer::new(plan, options)?;
    renderer.render_png(time_ticks, output.as_ref())?;
    println!("wrote {output}");
    Ok(())
}
