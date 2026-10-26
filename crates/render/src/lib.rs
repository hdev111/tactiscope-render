mod animation;
mod audio;
mod color;
mod fonts;
mod gradient;
mod graphic;

use anyhow::{Context, Result, bail};
use compositor::{
    BlendMode, CanvasClearDescriptor, Compositor, EffectPassDescriptor,
    EffectUniformValueDescriptor, FrameDescriptor, FrameItemDescriptor, LayerDescriptor,
    QuadTransformDescriptor,
};
use fontdue::{Font, FontSettings};
use gpu::GpuContext;
use gpu::wgpu;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};

pub const TICKS_PER_SECOND: f64 = 120_000.0;
pub const PLAN_VERSION: u32 = 1;
const VIDEO_CACHE_LIMIT: usize = 60;
const TEXT_CACHE_LIMIT: usize = 48;

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RenderPlan {
    version: u32,
    project: ProjectPlan,
    duration_ticks: i64,
    media: Vec<MediaRef>,
    tracks: Vec<TrackPlan>,
    #[serde(default)]
    audio: AudioPlan,
    #[serde(default = "default_quality")]
    quality: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct AudioPlan {
    #[serde(default)]
    elements: Vec<AudioElementPlan>,
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct AudioElementPlan {
    #[serde(default)]
    source_path: Option<String>,
    #[serde(default)]
    source_url: Option<String>,
    start_ticks: i64,
    duration_ticks: i64,
    trim_start_ticks: i64,
    #[serde(default)]
    retime: Option<RetimePlan>,
    #[serde(default)]
    volume_db: f32,
    #[serde(default)]
    muted: bool,
    #[serde(default)]
    animations: Option<Value>,
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct RetimePlan {
    rate: f32,
    #[serde(default)]
    maintain_pitch: bool,
}

fn default_quality() -> String {
    "high".to_string()
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ProjectPlan {
    width: u32,
    height: u32,
    fps: FpsPlan,
    background: BackgroundPlan,
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct FpsPlan {
    numerator: u32,
    denominator: u32,
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
enum BackgroundPlan {
    Color {
        color: String,
    },
    Gradient {
        css: String,
    },
    Blur {
        #[serde(rename = "blurIntensity")]
        blur_intensity: f32,
    },
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct MediaRef {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    source_path: String,
    width: Option<u32>,
    height: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct TrackPlan {
    order: i32,
    #[serde(default)]
    hidden: bool,
    elements: Vec<ElementPlan>,
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
enum ElementPlan {
    Video(VideoElementPlan),
    Image(ImageElementPlan),
    Text(TextElementPlan),
    Graphic(GraphicElementPlan),
    Sticker(StickerElementPlan),
    #[serde(rename = "scene-effect", alias = "sceneEffect")]
    SceneEffect(SceneEffectElementPlan),
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct SceneEffectElementPlan {
    id: String,
    effect_type: String,
    params: Value,
    start_ticks: i64,
    duration_ticks: i64,
    #[serde(default)]
    animations: Option<Value>,
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct VideoElementPlan {
    id: String,
    media_id: String,
    start_ticks: i64,
    duration_ticks: i64,
    trim_start_ticks: i64,
    transform: TransformPlan,
    opacity: f32,
    blend_mode: String,
    #[serde(default)]
    effects: Vec<EffectPlan>,
    #[serde(default)]
    masks: Vec<MaskPlan>,
    #[serde(default)]
    animations: Option<Value>,
    #[serde(default)]
    retime: Option<RetimePlan>,
    #[serde(default)]
    hidden: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct MaskPlan {
    #[serde(rename = "type")]
    kind: String,
    feather: f32,
    inverted: bool,
    stroke_color: String,
    stroke_width: f32,
    stroke_align: String,
    params: Value,
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct EffectPlan {
    #[serde(default)]
    id: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    intensity: f32,
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct GraphicElementPlan {
    id: String,
    definition_id: String,
    params: Value,
    start_ticks: i64,
    duration_ticks: i64,
    transform: TransformPlan,
    opacity: f32,
    blend_mode: String,
    #[serde(default)]
    effects: Vec<EffectPlan>,
    #[serde(default)]
    masks: Vec<MaskPlan>,
    #[serde(default)]
    animations: Option<Value>,
    #[serde(default)]
    hidden: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct StickerElementPlan {
    id: String,
    asset_path: String,
    intrinsic_width: Option<u32>,
    intrinsic_height: Option<u32>,
    start_ticks: i64,
    duration_ticks: i64,
    transform: TransformPlan,
    opacity: f32,
    blend_mode: String,
    #[serde(default)]
    effects: Vec<EffectPlan>,
    #[serde(default)]
    animations: Option<Value>,
    #[serde(default)]
    hidden: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct TextElementPlan {
    id: String,
    start_ticks: i64,
    duration_ticks: i64,
    transform: TransformPlan,
    opacity: f32,
    blend_mode: String,
    #[serde(default)]
    effects: Vec<EffectPlan>,
    params: TextParams,
    #[serde(default)]
    animations: Option<Value>,
    #[serde(default)]
    hidden: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct TextParams {
    #[serde(default)]
    font_path: Option<String>,
    content: String,
    font_size: f32,
    font_family: String,
    font_weight: String,
    font_style: String,
    text_align: String,
    #[serde(default)]
    #[allow(dead_code)]
    text_decoration: String,
    #[serde(default)]
    letter_spacing: f32,
    #[serde(default)]
    line_height: f32,
    #[serde(default = "default_text_color")]
    color: String,
    #[serde(default)]
    background_enabled: bool,
    #[serde(default = "default_text_color")]
    background_color: String,
    #[serde(default)]
    background_corner_radius: f32,
    #[serde(default)]
    background_padding_x: f32,
    #[serde(default)]
    background_padding_y: f32,
    #[serde(default)]
    background_offset_x: f32,
    #[serde(default)]
    background_offset_y: f32,
}

fn default_text_color() -> String {
    "#ffffff".to_string()
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ImageElementPlan {
    id: String,
    media_id: String,
    start_ticks: i64,
    duration_ticks: i64,
    transform: TransformPlan,
    opacity: f32,
    blend_mode: String,
    #[serde(default)]
    effects: Vec<EffectPlan>,
    #[serde(default)]
    masks: Vec<MaskPlan>,
    #[serde(default)]
    animations: Option<Value>,
    #[serde(default)]
    hidden: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct TransformPlan {
    position_x: f32,
    position_y: f32,
    scale_x: f32,
    scale_y: f32,
    rotate: f32,
}

impl RenderPlan {
    pub fn from_json(data: &[u8]) -> Result<Self> {
        let plan: Self = serde_json::from_slice(data).context("parse render plan")?;
        plan.validate()?;
        Ok(plan)
    }

    pub fn from_json_str(data: &str) -> Result<Self> {
        Self::from_json(data.as_bytes())
    }

    pub fn from_file(path: &Path) -> Result<Self> {
        let data = std::fs::read(path).with_context(|| format!("read plan {}", path.display()))?;
        Self::from_json(&data)
    }

    pub fn to_json(&self) -> Result<Value> {
        serde_json::to_value(self).context("serialize render plan")
    }

    pub fn validate(&self) -> Result<()> {
        if self.version != PLAN_VERSION {
            bail!(
                "unsupported render plan version {}; expected {PLAN_VERSION}",
                self.version
            );
        }
        if self.project.width == 0 || self.project.height == 0 {
            bail!("render plan has invalid canvas size");
        }
        if self.project.fps.numerator == 0 || self.project.fps.denominator == 0 {
            bail!("render plan has invalid fps");
        }
        if self.duration_ticks <= 0 {
            bail!("render plan has invalid duration");
        }
        Ok(())
    }

    pub fn width(&self) -> u32 {
        self.project.width
    }

    pub fn height(&self) -> u32 {
        self.project.height
    }

    pub fn duration_ticks(&self) -> i64 {
        self.duration_ticks
    }

    pub fn fps(&self) -> f64 {
        f64::from(self.project.fps.numerator) / f64::from(self.project.fps.denominator.max(1))
    }

    pub fn total_frames(&self) -> u64 {
        let seconds = self.duration_ticks as f64 / TICKS_PER_SECOND;
        ((seconds * self.fps()).floor() as u64).max(1)
    }

    pub fn frame_time_ticks(&self, frame: u64) -> i64 {
        let ticks = f64::from(self.project.fps.denominator) * TICKS_PER_SECOND
            / f64::from(self.project.fps.numerator.max(1));
        (frame as f64 * ticks).round() as i64
    }
}

pub fn plan_schema() -> Value {
    serde_json::to_value(schemars::schema_for!(RenderPlan)).expect("render plan schema serializes")
}

struct NativeRenderer {
    plan: RenderPlan,
    context: GpuContext,
    compositor: Compositor,
    media: HashMap<String, MediaRef>,
    image_cache: HashMap<String, Vec<u8>>,
    video_cache: HashMap<(String, i64), Vec<u8>>,
    video_cache_order: VecDeque<(String, i64)>,
    sequential_decoders: HashMap<String, SequentialVideoDecoder>,
    sequential_frame_cache: HashMap<String, (i64, Vec<u8>)>,
    fonts: HashMap<String, Font>,
    text_cache: HashMap<String, Arc<CachedTextTexture>>,
    text_cache_order: VecDeque<String>,
    background_cache: Option<Vec<u8>>,
    assets_root: PathBuf,
    ffmpeg: String,
}

struct CachedTextTexture {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    origin_x: f32,
    origin_y: f32,
}

struct LayerPayload {
    id: String,
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    descriptor: LayerDescriptor,
}

struct StrokePayload {
    id: String,
    rgba: Vec<u8>,
}

struct RenderedLayer {
    layer: LayerPayload,
    stroke: Option<StrokePayload>,
}

struct MaskArtifacts {
    texture_id: String,
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    descriptor: compositor::LayerMaskDescriptor,
    stroke: Option<StrokePayload>,
}

struct SequentialVideoDecoder {
    child: Child,
    stdout: BufReader<ChildStdout>,
    width: u32,
    height: u32,
}

impl Drop for SequentialVideoDecoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenderMode {
    Random,
    Sequential,
}

impl NativeRenderer {
    fn new(plan: RenderPlan, ffmpeg: String, assets_root: PathBuf) -> Result<Self> {
        plan.validate()?;
        let media = plan
            .media
            .iter()
            .map(|item| (item.id.clone(), item.clone()))
            .collect::<HashMap<_, _>>();
        let context = pollster::block_on(GpuContext::new())
            .map_err(|error| anyhow::anyhow!("failed to initialize GPU: {error}"))?;
        let compositor = Compositor::new(&context);
        Ok(Self {
            plan,
            context,
            compositor,
            media,
            image_cache: HashMap::new(),
            video_cache: HashMap::new(),
            video_cache_order: VecDeque::new(),
            sequential_decoders: HashMap::new(),
            sequential_frame_cache: HashMap::new(),
            fonts: HashMap::new(),
            text_cache: HashMap::new(),
            text_cache_order: VecDeque::new(),
            background_cache: None,
            assets_root,
            ffmpeg,
        })
    }

    fn render_frame(&mut self, time_ticks: i64) -> Result<Vec<u8>> {
        self.render_frame_internal(time_ticks, RenderMode::Random)
    }

    fn render_frame_sequential(&mut self, time_ticks: i64) -> Result<Vec<u8>> {
        self.render_frame_internal(time_ticks, RenderMode::Sequential)
    }

    fn render_frame_internal(&mut self, time_ticks: i64, mode: RenderMode) -> Result<Vec<u8>> {
        let width = self.plan.project.width;
        let height = self.plan.project.height;
        self.sequential_frame_cache.clear();
        let mut items = Vec::new();
        let mut texture_ids = Vec::new();

        enum BackgroundKind {
            Solid([f32; 4]),
            Gradient(String),
            Blur(f32),
        }
        let background = match &self.plan.project.background {
            BackgroundPlan::Color { color } if gradient::is_gradient(color) => {
                BackgroundKind::Gradient(color.clone())
            }
            BackgroundPlan::Color { color } => {
                BackgroundKind::Solid(color::parse_background_color(color)?)
            }
            BackgroundPlan::Gradient { css } => BackgroundKind::Gradient(css.clone()),
            BackgroundPlan::Blur { blur_intensity } => BackgroundKind::Blur(*blur_intensity),
        };
        let clear = match &background {
            BackgroundKind::Solid(color) => *color,
            BackgroundKind::Gradient(_) | BackgroundKind::Blur(_) => [0.0, 0.0, 0.0, 1.0],
        };
        if let BackgroundKind::Gradient(css) = &background {
            self.push_gradient_background(css, width, height, &mut items, &mut texture_ids)?;
        }
        if let BackgroundKind::Blur(blur_intensity) = &background {
            self.push_blur_background(
                time_ticks,
                mode == RenderMode::Sequential,
                *blur_intensity,
                &mut items,
                &mut texture_ids,
            )?;
        }

        let mut tracks = self.plan.tracks.clone();
        tracks.sort_by_key(|track| track.order);

        for track in &tracks {
            if track.hidden {
                continue;
            }
            for element in &track.elements {
                let rendered = match element {
                    ElementPlan::Video(element) => {
                        if element.hidden
                            || time_ticks < element.start_ticks
                            || time_ticks >= element.start_ticks + element.duration_ticks
                        {
                            continue;
                        }
                        let blend_mode = parse_blend_mode(&element.blend_mode)?;
                        let media = self
                            .media
                            .get(&element.media_id)
                            .cloned()
                            .context("missing media")?;
                        let local_ticks = time_ticks - element.start_ticks;
                        let rgba = self.decode_video_element_frame(
                            element,
                            &media,
                            time_ticks,
                            mode == RenderMode::Sequential,
                        )?;
                        let transform = resolved_transform(
                            &element.transform,
                            element.animations.as_ref(),
                            local_ticks,
                        );
                        let opacity = animation::scalar(
                            element.animations.as_ref(),
                            "opacity",
                            local_ticks,
                            element.opacity,
                        );
                        let source_width = media.width.context("media width is required")?;
                        let source_height = media.height.context("media height is required")?;
                        let (texture_id, mut descriptor) = build_layer(
                            &element.id,
                            source_width,
                            source_height,
                            width,
                            height,
                            &transform,
                            opacity,
                            blend_mode,
                            &element.effects,
                            element.animations.as_ref(),
                            local_ticks,
                        )?;
                        let stroke = self.attach_mask(
                            &element.masks,
                            source_width,
                            source_height,
                            &transform,
                            width,
                            height,
                            &element.id,
                            &mut descriptor,
                            &mut texture_ids,
                        )?;
                        Some(RenderedLayer {
                            layer: LayerPayload {
                                id: texture_id,
                                rgba,
                                width: source_width,
                                height: source_height,
                                descriptor,
                            },
                            stroke,
                        })
                    }
                    ElementPlan::Image(element) => {
                        if element.hidden
                            || time_ticks < element.start_ticks
                            || time_ticks >= element.start_ticks + element.duration_ticks
                        {
                            continue;
                        }
                        let blend_mode = parse_blend_mode(&element.blend_mode)?;
                        let media = self
                            .media
                            .get(&element.media_id)
                            .cloned()
                            .context("missing media")?;
                        let rgba = self.decode_image_frame(&element.media_id)?;
                        let local_ticks = time_ticks - element.start_ticks;
                        let transform = resolved_transform(
                            &element.transform,
                            element.animations.as_ref(),
                            local_ticks,
                        );
                        let opacity = animation::scalar(
                            element.animations.as_ref(),
                            "opacity",
                            local_ticks,
                            element.opacity,
                        );
                        let source_width = media.width.context("media width is required")?;
                        let source_height = media.height.context("media height is required")?;
                        let (texture_id, mut descriptor) = build_layer(
                            &element.id,
                            source_width,
                            source_height,
                            width,
                            height,
                            &transform,
                            opacity,
                            blend_mode,
                            &element.effects,
                            element.animations.as_ref(),
                            local_ticks,
                        )?;
                        let stroke = self.attach_mask(
                            &element.masks,
                            source_width,
                            source_height,
                            &transform,
                            width,
                            height,
                            &element.id,
                            &mut descriptor,
                            &mut texture_ids,
                        )?;
                        Some(RenderedLayer {
                            layer: LayerPayload {
                                id: texture_id,
                                rgba,
                                width: source_width,
                                height: source_height,
                                descriptor,
                            },
                            stroke,
                        })
                    }
                    ElementPlan::Graphic(element) => {
                        if element.hidden
                            || time_ticks < element.start_ticks
                            || time_ticks >= element.start_ticks + element.duration_ticks
                        {
                            continue;
                        }
                        let blend_mode = parse_blend_mode(&element.blend_mode)?;
                        let local_ticks = time_ticks - element.start_ticks;
                        let resolved_params = animation::resolve_params(
                            element.animations.as_ref(),
                            local_ticks,
                            &element.params,
                        );
                        let (pixels, source_width, source_height) =
                            self.render_graphic_raster(&element.definition_id, &resolved_params)?;
                        let transform = resolved_transform(
                            &element.transform,
                            element.animations.as_ref(),
                            local_ticks,
                        );
                        let opacity = animation::scalar(
                            element.animations.as_ref(),
                            "opacity",
                            local_ticks,
                            element.opacity,
                        );
                        let (texture_id, mut descriptor) = build_layer(
                            &element.id,
                            source_width,
                            source_height,
                            width,
                            height,
                            &transform,
                            opacity,
                            blend_mode,
                            &element.effects,
                            element.animations.as_ref(),
                            local_ticks,
                        )?;
                        let stroke = self.attach_mask(
                            &element.masks,
                            source_width,
                            source_height,
                            &transform,
                            width,
                            height,
                            &element.id,
                            &mut descriptor,
                            &mut texture_ids,
                        )?;
                        Some(RenderedLayer {
                            layer: LayerPayload {
                                id: texture_id,
                                rgba: pixels,
                                width: source_width,
                                height: source_height,
                                descriptor,
                            },
                            stroke,
                        })
                    }
                    ElementPlan::Sticker(element) => {
                        if element.hidden
                            || time_ticks < element.start_ticks
                            || time_ticks >= element.start_ticks + element.duration_ticks
                        {
                            continue;
                        }
                        let blend_mode = parse_blend_mode(&element.blend_mode)?;
                        let (pixels, source_width, source_height) = self.render_sticker_raster(
                            &element.asset_path,
                            element.intrinsic_width,
                            element.intrinsic_height,
                        )?;
                        let local_ticks = time_ticks - element.start_ticks;
                        let transform = resolved_transform(
                            &element.transform,
                            element.animations.as_ref(),
                            local_ticks,
                        );
                        let opacity = animation::scalar(
                            element.animations.as_ref(),
                            "opacity",
                            local_ticks,
                            element.opacity,
                        );
                        let (texture_id, descriptor) = build_layer(
                            &element.id,
                            source_width,
                            source_height,
                            width,
                            height,
                            &transform,
                            opacity,
                            blend_mode,
                            &element.effects,
                            element.animations.as_ref(),
                            local_ticks,
                        )?;
                        Some(RenderedLayer {
                            layer: LayerPayload {
                                id: texture_id,
                                rgba: pixels,
                                width: source_width,
                                height: source_height,
                                descriptor,
                            },
                            stroke: None,
                        })
                    }
                    ElementPlan::SceneEffect(element) => {
                        if time_ticks < element.start_ticks
                            || time_ticks >= element.start_ticks + element.duration_ticks
                        {
                            continue;
                        }
                        if element.effect_type != "blur" {
                            bail!(
                                "scene effect '{}' is not supported by the native engine yet",
                                element.effect_type
                            );
                        }
                        let intensity = element
                            .params
                            .get("intensity")
                            .and_then(Value::as_f64)
                            .unwrap_or(15.0) as f32;
                        let local_ticks = time_ticks - element.start_ticks;
                        let intensity = animation::scalar(
                            element.animations.as_ref(),
                            "params.intensity",
                            local_ticks,
                            intensity,
                        );
                        let groups = build_effects(
                            &[EffectPlan {
                                id: element.id.clone(),
                                kind: "blur".to_string(),
                                intensity,
                            }],
                            width as f32,
                            height as f32,
                            None,
                            0,
                        )?;
                        if !groups.is_empty() {
                            items.push(FrameItemDescriptor::SceneEffect {
                                effect_pass_groups: groups,
                            });
                        }
                        None
                    }
                    ElementPlan::Text(element) => {
                        if element.hidden
                            || time_ticks < element.start_ticks
                            || time_ticks >= element.start_ticks + element.duration_ticks
                        {
                            continue;
                        }
                        let blend_mode = parse_blend_mode(&element.blend_mode)?;
                        let local_ticks = time_ticks - element.start_ticks;
                        let transform = resolved_transform(
                            &element.transform,
                            element.animations.as_ref(),
                            local_ticks,
                        );
                        let opacity = animation::scalar(
                            element.animations.as_ref(),
                            "opacity",
                            local_ticks,
                            element.opacity,
                        );
                        let params = resolve_text_params(
                            element.animations.as_ref(),
                            local_ticks,
                            &element.params,
                        );
                        let rgba = self.render_text_layer(
                            &element.id,
                            &params,
                            &transform,
                            width,
                            height,
                        )?;
                        Some(RenderedLayer {
                            layer: LayerPayload {
                                id: format!("native-text-{}", element.id),
                                rgba,
                                width,
                                height,
                                descriptor: LayerDescriptor {
                                    texture_id: format!("native-text-{}", element.id),
                                    transform: QuadTransformDescriptor {
                                        center_x: width as f32 / 2.0,
                                        center_y: height as f32 / 2.0,
                                        width: width as f32,
                                        height: height as f32,
                                        rotation_degrees: 0.0,
                                        flip_x: false,
                                        flip_y: false,
                                    },
                                    opacity: opacity.clamp(0.0, 1.0),
                                    blend_mode,
                                    effect_pass_groups: build_effects(
                                        &element.effects,
                                        width as f32,
                                        height as f32,
                                        element.animations.as_ref(),
                                        local_ticks,
                                    )?,
                                    mask: None,
                                },
                            },
                            stroke: None,
                        })
                    }
                };

                if let Some(rendered) = rendered {
                    self.push_rendered_layer(rendered, &mut items, &mut texture_ids);
                }
            }
        }

        let frame = FrameDescriptor {
            width,
            height,
            clear: CanvasClearDescriptor { color: clear },
            items,
        };
        let output = self
            .compositor
            .render_frame_to_texture(&self.context, &frame)
            .map_err(|error| anyhow::anyhow!("compositor failed: {error}"))?;
        let rgba = read_texture_rgba(&self.context, &output, width, height)?;
        for texture_id in texture_ids {
            self.compositor.release_texture(&texture_id);
        }
        Ok(rgba)
    }

    fn push_rendered_layer(
        &mut self,
        rendered: RenderedLayer,
        items: &mut Vec<FrameItemDescriptor>,
        texture_ids: &mut Vec<String>,
    ) {
        let payload = rendered.layer;
        let texture =
            self.upload_texture(&payload.rgba, payload.width, payload.height, &payload.id);
        self.compositor.upsert_texture(payload.id.clone(), texture);
        texture_ids.push(payload.id);
        items.push(FrameItemDescriptor::Layer(payload.descriptor));
        if let Some(stroke) = rendered.stroke {
            let texture = self.upload_texture(
                &stroke.rgba,
                self.plan.project.width,
                self.plan.project.height,
                &stroke.id,
            );
            self.compositor.upsert_texture(stroke.id.clone(), texture);
            texture_ids.push(stroke.id.clone());
            items.push(FrameItemDescriptor::Layer(full_canvas_layer(
                stroke.id,
                self.plan.project.width,
                self.plan.project.height,
            )));
        }
    }

    fn push_gradient_background(
        &mut self,
        css: &str,
        width: u32,
        height: u32,
        items: &mut Vec<FrameItemDescriptor>,
        texture_ids: &mut Vec<String>,
    ) -> Result<()> {
        if self.background_cache.is_none() {
            self.background_cache = Some(gradient::rasterize_background(css, width, height)?);
        }
        let pixels = self
            .background_cache
            .as_ref()
            .context("background cache missing")?
            .clone();
        let texture_id = "native-background-gradient".to_string();
        let texture = self.upload_texture(&pixels, width, height, &texture_id);
        self.compositor.upsert_texture(texture_id.clone(), texture);
        texture_ids.push(texture_id.clone());
        items.push(FrameItemDescriptor::Layer(full_canvas_layer(
            texture_id, width, height,
        )));
        Ok(())
    }

    fn push_blur_background(
        &mut self,
        time_ticks: i64,
        sequential: bool,
        blur_intensity: f32,
        items: &mut Vec<FrameItemDescriptor>,
        texture_ids: &mut Vec<String>,
    ) -> Result<()> {
        let width = self.plan.project.width;
        let height = self.plan.project.height;
        let Some(track) = self
            .plan
            .tracks
            .iter()
            .filter(|track| !track.hidden)
            .min_by_key(|track| track.order)
        else {
            return Ok(());
        };
        let sigma_x = intensity_to_sigma(blur_intensity, width as f32, 1920.0);
        let sigma_y = intensity_to_sigma(blur_intensity, height as f32, 1080.0);
        let passes = build_blur_passes(sigma_x, sigma_y);
        let candidates = track
            .elements
            .iter()
            .filter(|element| match element {
                ElementPlan::Video(element) => {
                    !element.hidden
                        && time_ticks >= element.start_ticks
                        && time_ticks < element.start_ticks + element.duration_ticks
                }
                ElementPlan::Image(element) => {
                    !element.hidden
                        && time_ticks >= element.start_ticks
                        && time_ticks < element.start_ticks + element.duration_ticks
                }
                _ => false,
            })
            .cloned()
            .collect::<Vec<_>>();
        for element in candidates {
            let (id, media_id, rgba) = match &element {
                ElementPlan::Video(video) => {
                    let media = self
                        .media
                        .get(&video.media_id)
                        .cloned()
                        .context("missing media")?;
                    let rgba =
                        self.decode_video_element_frame(video, &media, time_ticks, sequential)?;
                    (video.id.clone(), video.media_id.clone(), rgba)
                }
                ElementPlan::Image(image) => {
                    let rgba = self.decode_image_frame(&image.media_id)?;
                    (image.id.clone(), image.media_id.clone(), rgba)
                }
                _ => continue,
            };
            let media = self
                .media
                .get(&media_id)
                .cloned()
                .context("missing media")?;
            let source_width = media.width.context("media width is required")?;
            let source_height = media.height.context("media height is required")?;
            let scaled = cover_scale_rgba(&rgba, source_width, source_height, width, height);
            let texture_id = format!("native-backdrop-{id}");
            let texture = self.upload_texture(&scaled, width, height, &texture_id);
            self.compositor.upsert_texture(texture_id.clone(), texture);
            texture_ids.push(texture_id.clone());
            let mut descriptor = full_canvas_layer(texture_id, width, height);
            if !passes.is_empty() {
                descriptor.effect_pass_groups = vec![passes.clone()];
            }
            items.push(FrameItemDescriptor::Layer(descriptor));
        }
        Ok(())
    }

    fn attach_mask(
        &mut self,
        masks: &[MaskPlan],
        source_width: u32,
        source_height: u32,
        transform: &TransformPlan,
        canvas_width: u32,
        canvas_height: u32,
        element_id: &str,
        descriptor: &mut LayerDescriptor,
        texture_ids: &mut Vec<String>,
    ) -> Result<Option<StrokePayload>> {
        let Some(mask) = masks.first() else {
            return Ok(None);
        };
        if !mask_is_active(mask) {
            return Ok(None);
        }
        let artifacts = self.render_mask(
            mask,
            source_width,
            source_height,
            transform,
            canvas_width,
            canvas_height,
            element_id,
        )?;
        let texture = self.upload_texture(
            &artifacts.rgba,
            artifacts.width,
            artifacts.height,
            &artifacts.texture_id,
        );
        self.compositor
            .upsert_texture(artifacts.texture_id.clone(), texture);
        texture_ids.push(artifacts.texture_id.clone());
        descriptor.mask = Some(artifacts.descriptor);
        Ok(artifacts.stroke)
    }

    fn decode_video_element_frame(
        &mut self,
        element: &VideoElementPlan,
        media: &MediaRef,
        time_ticks: i64,
        sequential: bool,
    ) -> Result<Vec<u8>> {
        let rate = element
            .retime
            .as_ref()
            .map(|retime| retime.rate.clamp(0.01, 5.0))
            .unwrap_or(1.0);
        let source_ticks = element.trim_start_ticks
            + ((time_ticks - element.start_ticks) as f64 * f64::from(rate)).round() as i64;
        if !sequential {
            return self.decode_video_frame(&element.media_id, source_ticks);
        }
        if let Some((cached_ticks, bytes)) = self.sequential_frame_cache.get(&element.id) {
            if *cached_ticks == time_ticks {
                return Ok(bytes.clone());
            }
        }
        let bytes =
            self.decode_video_frame_sequential(&element.id, media, element.trim_start_ticks, rate)?;
        self.sequential_frame_cache
            .insert(element.id.clone(), (time_ticks, bytes.clone()));
        Ok(bytes)
    }

    fn decode_video_frame_sequential(
        &mut self,
        element_id: &str,
        media: &MediaRef,
        trim_start_ticks: i64,
        rate: f32,
    ) -> Result<Vec<u8>> {
        if !self.sequential_decoders.contains_key(element_id) {
            let width = media.width.context("media width is required")?;
            let height = media.height.context("media height is required")?;
            let fps = self.plan.fps();
            let start_seconds = trim_start_ticks as f64 / TICKS_PER_SECOND;
            let mut child = Command::new(&self.ffmpeg)
                .args([
                    "-v",
                    "error",
                    "-an",
                    "-sn",
                    "-ss",
                    &format!("{start_seconds:.6}"),
                    "-i",
                    &media.source_path,
                    "-vf",
                    &format!("setpts=(PTS-STARTPTS)/{rate:.6},fps={fps:.6}"),
                    "-f",
                    "rawvideo",
                    "-pix_fmt",
                    "rgba",
                    "-",
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .with_context(|| format!("spawn video decoder for {}", media.source_path))?;
            let stdout = child
                .stdout
                .take()
                .context("video decoder stdout unavailable")?;
            self.sequential_decoders.insert(
                element_id.to_string(),
                SequentialVideoDecoder {
                    child,
                    stdout: BufReader::new(stdout),
                    width,
                    height,
                },
            );
        }
        let decoder = self
            .sequential_decoders
            .get_mut(element_id)
            .context("video decoder missing")?;
        let expected = decoder.width as usize * decoder.height as usize * 4;
        let mut bytes = vec![0u8; expected];
        decoder
            .stdout
            .read_exact(&mut bytes)
            .with_context(|| format!("video decoder ended for {element_id}"))?;
        Ok(bytes)
    }

    fn decode_video_frame(&mut self, media_id: &str, source_ticks: i64) -> Result<Vec<u8>> {
        let key = (media_id.to_string(), source_ticks);
        if let Some(bytes) = self.video_cache.get(&key) {
            return Ok(bytes.clone());
        }
        let media = self.media.get(media_id).context("missing video media")?;
        let seconds = source_ticks as f64 / TICKS_PER_SECOND;
        let bytes = self.decode_raw(media, Some(seconds))?;
        self.video_cache.insert(key.clone(), bytes.clone());
        self.video_cache_order.push_back(key);
        while self.video_cache_order.len() > VIDEO_CACHE_LIMIT {
            if let Some(old) = self.video_cache_order.pop_front() {
                self.video_cache.remove(&old);
            }
        }
        Ok(bytes)
    }

    fn decode_image_frame(&mut self, media_id: &str) -> Result<Vec<u8>> {
        if let Some(bytes) = self.image_cache.get(media_id) {
            return Ok(bytes.clone());
        }
        let media = self.media.get(media_id).context("missing image media")?;
        let bytes = self.decode_raw(media, None)?;
        self.image_cache.insert(media_id.to_string(), bytes.clone());
        Ok(bytes)
    }

    fn decode_raw(&self, media: &MediaRef, source_time: Option<f64>) -> Result<Vec<u8>> {
        if media.kind != "video" && media.kind != "image" {
            bail!("media '{}' has unsupported type '{}'", media.id, media.kind);
        }
        let width = media.width.context("media width is required")?;
        let height = media.height.context("media height is required")?;
        let expected = width as usize * height as usize * 4;
        if media.source_path.to_ascii_lowercase().ends_with(".svg") {
            let data = std::fs::read(&media.source_path)
                .with_context(|| format!("read SVG image {}", media.source_path))?;
            return render_svg_rgba(&data, width, height, &media.source_path);
        }
        let mut command = Command::new(&self.ffmpeg);
        command.args(["-v", "error", "-an", "-sn"]);
        if let Some(seconds) = source_time {
            command.args(["-ss", &format!("{seconds:.6}")]);
        }
        command.args([
            "-i",
            &media.source_path,
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "-",
        ]);
        let output = command
            .output()
            .with_context(|| format!("failed to run ffmpeg for {}", media.source_path))?;
        if !output.status.success() {
            bail!(
                "ffmpeg decode failed for {}: {}",
                media.source_path,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        if output.stdout.len() != expected {
            bail!(
                "ffmpeg decode for {} returned {} bytes, expected {} ({}x{})",
                media.source_path,
                output.stdout.len(),
                expected,
                width,
                height
            );
        }
        Ok(output.stdout)
    }
}

fn full_canvas_layer(texture_id: String, width: u32, height: u32) -> LayerDescriptor {
    LayerDescriptor {
        texture_id,
        transform: QuadTransformDescriptor {
            center_x: width as f32 / 2.0,
            center_y: height as f32 / 2.0,
            width: width as f32,
            height: height as f32,
            rotation_degrees: 0.0,
            flip_x: false,
            flip_y: false,
        },
        opacity: 1.0,
        blend_mode: BlendMode::Normal,
        effect_pass_groups: Vec::new(),
        mask: None,
    }
}

/// Resize and center-crop a frame to the canvas using bilinear sampling.
fn cover_scale_rgba(
    source: &[u8],
    source_width: u32,
    source_height: u32,
    width: u32,
    height: u32,
) -> Vec<u8> {
    let mut output = vec![0u8; width as usize * height as usize * 4];
    if source_width == 0 || source_height == 0 || width == 0 || height == 0 {
        return output;
    }
    let scale = (width as f32 / source_width as f32).max(height as f32 / source_height as f32);
    let scaled_width = source_width as f32 * scale;
    let scaled_height = source_height as f32 * scale;
    let offset_x = (width as f32 - scaled_width) / 2.0;
    let offset_y = (height as f32 - scaled_height) / 2.0;
    for y in 0..height {
        for x in 0..width {
            let source_x =
                ((x as f32 + 0.5 - offset_x) / scale - 0.5).clamp(0.0, source_width as f32 - 1.0);
            let source_y =
                ((y as f32 + 0.5 - offset_y) / scale - 0.5).clamp(0.0, source_height as f32 - 1.0);
            let x0 = source_x.floor() as u32;
            let y0 = source_y.floor() as u32;
            let x1 = (x0 + 1).min(source_width - 1);
            let y1 = (y0 + 1).min(source_height - 1);
            let fx = source_x - x0 as f32;
            let fy = source_y - y0 as f32;
            let index = |px: u32, py: u32| ((py * source_width + px) as usize) * 4;
            let destination = ((y * width + x) as usize) * 4;
            for channel in 0..4 {
                let top = source[index(x0, y0) + channel] as f32 * (1.0 - fx)
                    + source[index(x1, y0) + channel] as f32 * fx;
                let bottom = source[index(x0, y1) + channel] as f32 * (1.0 - fx)
                    + source[index(x1, y1) + channel] as f32 * fx;
                output[destination + channel] =
                    (top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    output
}

fn resolve_text_params(
    animations: Option<&Value>,
    local_ticks: i64,
    base: &TextParams,
) -> TextParams {
    let mut resolved = base.clone();
    resolved.color = color::format_hex(animation::color_rgba(
        animations,
        "color",
        local_ticks,
        &base.color,
    ));
    resolved.background_color = color::format_hex(animation::color_rgba(
        animations,
        "background.color",
        local_ticks,
        &base.background_color,
    ));
    resolved.background_padding_x = animation::scalar(
        animations,
        "background.paddingX",
        local_ticks,
        base.background_padding_x,
    );
    resolved.background_padding_y = animation::scalar(
        animations,
        "background.paddingY",
        local_ticks,
        base.background_padding_y,
    );
    resolved.background_offset_x = animation::scalar(
        animations,
        "background.offsetX",
        local_ticks,
        base.background_offset_x,
    );
    resolved.background_offset_y = animation::scalar(
        animations,
        "background.offsetY",
        local_ticks,
        base.background_offset_y,
    );
    resolved.background_corner_radius = animation::scalar(
        animations,
        "background.cornerRadius",
        local_ticks,
        base.background_corner_radius,
    );
    resolved
}

fn build_layer(
    element_id: &str,
    source_width: u32,
    source_height: u32,
    canvas_width: u32,
    canvas_height: u32,
    transform: &TransformPlan,
    opacity: f32,
    blend_mode: BlendMode,
    effects: &[EffectPlan],
    animations: Option<&Value>,
    local_ticks: i64,
) -> Result<(String, LayerDescriptor)> {
    let source_width = source_width as f32;
    let source_height = source_height as f32;
    let contain = (canvas_width as f32 / source_width).min(canvas_height as f32 / source_height);
    let width = (source_width * contain * transform.scale_x).abs();
    let height = (source_height * contain * transform.scale_y).abs();
    let texture_id = format!("native-layer-{element_id}");
    let descriptor = LayerDescriptor {
        texture_id: texture_id.clone(),
        transform: QuadTransformDescriptor {
            center_x: canvas_width as f32 / 2.0 + transform.position_x,
            center_y: canvas_height as f32 / 2.0 + transform.position_y,
            width,
            height,
            rotation_degrees: transform.rotate,
            flip_x: transform.scale_x < 0.0,
            flip_y: transform.scale_y < 0.0,
        },
        opacity: opacity.clamp(0.0, 1.0),
        blend_mode,
        effect_pass_groups: build_effects(
            effects,
            canvas_width as f32,
            canvas_height as f32,
            animations,
            local_ticks,
        )?,
        mask: None,
    };
    Ok((texture_id, descriptor))
}

fn read_texture_rgba(
    context: &GpuContext,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Result<Vec<u8>> {
    let unpadded_bytes_per_row = width * 4;
    let padded_bytes_per_row = unpadded_bytes_per_row.div_ceil(256) * 256;
    let buffer_size = u64::from(padded_bytes_per_row) * u64::from(height);
    let buffer = context.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("native-readback"),
        size: buffer_size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = context
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("native-readback-encoder"),
        });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    context.queue().submit([encoder.finish()]);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    context
        .device()
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|error| anyhow::anyhow!("GPU readback poll failed: {error}"))?;
    let mapped = slice.get_mapped_range();
    let mut output = Vec::with_capacity(unpadded_bytes_per_row as usize * height as usize);
    for row in 0..height {
        let start = (row * padded_bytes_per_row) as usize;
        let end = start + unpadded_bytes_per_row as usize;
        output.extend_from_slice(&mapped[start..end]);
    }
    drop(mapped);
    buffer.unmap();
    if context.texture_format() == wgpu::TextureFormat::Bgra8Unorm {
        for pixel in output.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
    }
    Ok(output)
}

fn fill_rounded_rect(
    buffer: &mut [u8],
    width: u32,
    height: u32,
    x: f32,
    y: f32,
    rect_width: f32,
    rect_height: f32,
    radius: f32,
    color: [f32; 4],
) {
    let left = x.max(0.0).floor() as u32;
    let top = y.max(0.0).floor() as u32;
    let right = (x + rect_width).min(width as f32).ceil().max(0.0) as u32;
    let bottom = (y + rect_height).min(height as f32).ceil().max(0.0) as u32;
    let radius = radius.max(0.0).min(rect_width.min(rect_height) / 2.0);
    for py in top..bottom {
        for px in left..right {
            let fx = px as f32 + 0.5;
            let fy = py as f32 + 0.5;
            let cx = fx.clamp(x + radius, x + rect_width - radius);
            let cy = fy.clamp(y + radius, y + rect_height - radius);
            if (fx - cx).hypot(fy - cy) > radius.max(0.5) {
                continue;
            }
            let index = (py * width + px) as usize * 4;
            let alpha = color[3];
            for channel in 0..3 {
                buffer[index + channel] = (((color[channel] * alpha)
                    + (buffer[index + channel] as f32 / 255.0) * (1.0 - alpha))
                    .clamp(0.0, 1.0)
                    * 255.0)
                    .round() as u8;
            }
            buffer[index + 3] = ((alpha + (buffer[index + 3] as f32 / 255.0) * (1.0 - alpha))
                * 255.0)
                .round() as u8;
        }
    }
}

fn blit_alpha_bitmap(
    buffer: &mut [u8],
    width: u32,
    height: u32,
    bitmap: &[u8],
    glyph_width: usize,
    glyph_height: usize,
    x: f32,
    y: f32,
    color: [f32; 4],
) {
    let origin_x = x.round() as i32;
    let origin_y = y.round() as i32;
    for gy in 0..glyph_height {
        for gx in 0..glyph_width {
            let coverage = bitmap[gy * glyph_width + gx] as f32 / 255.0;
            if coverage <= 0.0 {
                continue;
            }
            let px = origin_x + gx as i32;
            let py = origin_y + gy as i32;
            if px < 0 || py < 0 || px >= width as i32 || py >= height as i32 {
                continue;
            }
            let index = (py as u32 * width + px as u32) as usize * 4;
            let alpha = coverage * color[3];
            for channel in 0..3 {
                buffer[index + channel] = (((color[channel] * alpha)
                    + (buffer[index + channel] as f32 / 255.0) * (1.0 - alpha))
                    .clamp(0.0, 1.0)
                    * 255.0)
                    .round() as u8;
            }
            buffer[index + 3] = ((alpha + (buffer[index + 3] as f32 / 255.0) * (1.0 - alpha))
                * 255.0)
                .round() as u8;
        }
    }
}

fn resolved_transform(
    base: &TransformPlan,
    animations: Option<&Value>,
    local_ticks: i64,
) -> TransformPlan {
    TransformPlan {
        position_x: animation::scalar(
            animations,
            "transform.positionX",
            local_ticks,
            base.position_x,
        ),
        position_y: animation::scalar(
            animations,
            "transform.positionY",
            local_ticks,
            base.position_y,
        ),
        scale_x: animation::scalar(animations, "transform.scaleX", local_ticks, base.scale_x),
        scale_y: animation::scalar(animations, "transform.scaleY", local_ticks, base.scale_y),
        rotate: animation::scalar(animations, "transform.rotate", local_ticks, base.rotate),
    }
}

fn intensity_to_sigma(intensity: f32, resolution: f32, reference: f32) -> f32 {
    (intensity / 5.0) * (resolution / reference)
}

fn build_effects(
    effects: &[EffectPlan],
    width: f32,
    height: f32,
    animations: Option<&Value>,
    local_ticks: i64,
) -> Result<Vec<Vec<EffectPassDescriptor>>> {
    let mut groups = Vec::new();
    for effect in effects {
        match effect.kind.as_str() {
            "blur" => {
                let intensity = animation::scalar(
                    animations,
                    &format!("effects.{}.params.intensity", effect.id),
                    local_ticks,
                    effect.intensity,
                );
                let sigma_x = intensity_to_sigma(intensity, width, 1920.0);
                let sigma_y = intensity_to_sigma(intensity, height, 1080.0);
                let passes = build_blur_passes(sigma_x, sigma_y);
                if !passes.is_empty() {
                    groups.push(passes);
                }
            }
            other => bail!("effect '{other}' is not supported by the native engine yet"),
        }
    }
    Ok(groups)
}

fn build_blur_passes(sigma_x: f32, sigma_y: f32) -> Vec<EffectPassDescriptor> {
    let max_sigma = sigma_x.max(sigma_y);
    if max_sigma < 0.001 {
        return Vec::new();
    }
    let iterations = ((max_sigma * max_sigma) / (10.0 * 4.0 * 10.0 * 4.0))
        .ceil()
        .max(1.0)
        .min(8.0) as usize;
    let per_sigma_x = sigma_x / (iterations as f32).sqrt();
    let per_sigma_y = sigma_y / (iterations as f32).sqrt();
    let step_x = (per_sigma_x / 10.0).max(1.0);
    let step_y = (per_sigma_y / 10.0).max(1.0);
    let mut passes = Vec::with_capacity(iterations * 2);
    for _ in 0..iterations {
        passes.push(EffectPassDescriptor {
            shader: "gaussian-blur".to_string(),
            uniforms: HashMap::from([
                (
                    "u_sigma".to_string(),
                    EffectUniformValueDescriptor::Number(per_sigma_x),
                ),
                (
                    "u_step".to_string(),
                    EffectUniformValueDescriptor::Number(step_x),
                ),
                (
                    "u_direction".to_string(),
                    EffectUniformValueDescriptor::Vector(vec![1.0, 0.0]),
                ),
            ]),
        });
        passes.push(EffectPassDescriptor {
            shader: "gaussian-blur".to_string(),
            uniforms: HashMap::from([
                (
                    "u_sigma".to_string(),
                    EffectUniformValueDescriptor::Number(per_sigma_y),
                ),
                (
                    "u_step".to_string(),
                    EffectUniformValueDescriptor::Number(step_y),
                ),
                (
                    "u_direction".to_string(),
                    EffectUniformValueDescriptor::Vector(vec![0.0, 1.0]),
                ),
            ]),
        });
    }
    passes
}

fn mask_param_f32(params: &Value, key: &str, fallback: f32) -> f32 {
    params
        .get(key)
        .and_then(Value::as_f64)
        .map(|value| value as f32)
        .unwrap_or(fallback)
}

/// Mirrors `MaskDefinition.isActive`: an open freeform path is only being
/// drawn and does not mask or stroke the element yet.
fn mask_is_active(mask: &MaskPlan) -> bool {
    if mask.kind != "freeform" {
        return true;
    }
    mask.params
        .get("closed")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn stroke_offset(stroke_align: &str, stroke_width: f32) -> f32 {
    match stroke_align {
        "inside" => -stroke_width / 2.0,
        "outside" => stroke_width / 2.0,
        _ => 0.0,
    }
}

fn rotate_point(x: f32, y: f32, cx: f32, cy: f32, angle: f32) -> (f32, f32) {
    let cos = angle.cos();
    let sin = angle.sin();
    let dx = x - cx;
    let dy = y - cy;
    (cx + dx * cos - dy * sin, cy + dx * sin + dy * cos)
}

fn add_rotated_polygon(
    builder: &mut PathBuilder,
    center_x: f32,
    center_y: f32,
    half_width: f32,
    half_height: f32,
    rotation: f32,
    points: &[(f32, f32)],
) {
    for (index, (x, y)) in points.iter().enumerate() {
        let point = rotate_point(
            center_x + x * half_width,
            center_y + y * half_height,
            center_x,
            center_y,
            rotation,
        );
        if index == 0 {
            builder.move_to(point.0, point.1);
        } else {
            builder.line_to(point.0, point.1);
        }
    }
    builder.close();
}

fn add_rotated_ellipse(
    builder: &mut PathBuilder,
    center_x: f32,
    center_y: f32,
    radius_x: f32,
    radius_y: f32,
    rotation: f32,
) {
    let segments = 64;
    for index in 0..segments {
        let angle = index as f32 * std::f32::consts::TAU / segments as f32;
        let point = rotate_point(
            center_x + radius_x * angle.cos(),
            center_y + radius_y * angle.sin(),
            center_x,
            center_y,
            rotation,
        );
        if index == 0 {
            builder.move_to(point.0, point.1);
        } else {
            builder.line_to(point.0, point.1);
        }
    }
    builder.close();
}

fn add_rotated_heart(
    builder: &mut PathBuilder,
    center_x: f32,
    center_y: f32,
    half_width: f32,
    half_height: f32,
    rotation: f32,
) {
    let point = |x: f32, y: f32| rotate_point(x, y, center_x, center_y, rotation);
    let start = point(center_x, center_y - half_height * 0.475);
    let right1 = point(center_x + half_width, center_y - half_height * 1.225);
    let right2 = point(center_x + half_width, center_y - half_height * 0.125);
    let bottom = point(center_x, center_y + half_height * 0.725);
    let left1 = point(center_x - half_width, center_y - half_height * 0.125);
    let left2 = point(center_x - half_width, center_y - half_height * 1.225);
    builder.move_to(start.0, start.1);
    builder.cubic_to(right1.0, right1.1, right2.0, right2.1, bottom.0, bottom.1);
    builder.cubic_to(left1.0, left1.1, left2.0, left2.1, start.0, start.1);
    builder.close();
}

fn parse_blend_mode(value: &str) -> Result<BlendMode> {
    Ok(match value {
        "normal" => BlendMode::Normal,
        "darken" => BlendMode::Darken,
        "multiply" => BlendMode::Multiply,
        "color-burn" => BlendMode::ColorBurn,
        "lighten" => BlendMode::Lighten,
        "screen" => BlendMode::Screen,
        "plus-lighter" => BlendMode::PlusLighter,
        "color-dodge" => BlendMode::ColorDodge,
        "overlay" => BlendMode::Overlay,
        "soft-light" => BlendMode::SoftLight,
        "hard-light" => BlendMode::HardLight,
        "difference" => BlendMode::Difference,
        "exclusion" => BlendMode::Exclusion,
        "hue" => BlendMode::Hue,
        "saturation" => BlendMode::Saturation,
        "color" => BlendMode::Color,
        "luminosity" => BlendMode::Luminosity,
        _ => bail!("unsupported blend mode '{value}'"),
    })
}

fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    let file = File::create(path).with_context(|| format!("create {}", path.display()))?;
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().context("write PNG header")?;
    writer.write_image_data(rgba).context("write PNG data")?;
    Ok(())
}

fn quality_crf(quality: &str) -> u8 {
    match quality {
        "low" => 28,
        "medium" => 23,
        "very_high" => 18,
        _ => 20,
    }
}

#[derive(Debug, Clone)]
pub struct RendererOptions {
    pub ffmpeg_path: PathBuf,
    pub assets_root: Option<PathBuf>,
    pub temp_dir: Option<PathBuf>,
}

impl Default for RendererOptions {
    fn default() -> Self {
        Self {
            ffmpeg_path: PathBuf::from("ffmpeg"),
            assets_root: None,
            temp_dir: None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ExportProgress {
    pub frame: u64,
    pub total_frames: u64,
    pub percent: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExportReport {
    pub output: String,
    pub width: u32,
    pub height: u32,
    pub frames: u64,
    pub duration_seconds: f64,
    pub has_audio: bool,
}

pub struct Renderer {
    inner: NativeRenderer,
    options: RendererOptions,
}

impl Renderer {
    pub fn new(plan: RenderPlan, options: RendererOptions) -> Result<Self> {
        let assets_root = resolve_assets_root(options.assets_root.as_deref());
        let inner = NativeRenderer::new(
            plan,
            options.ffmpeg_path.to_string_lossy().into_owned(),
            assets_root,
        )?;
        Ok(Self { inner, options })
    }

    pub fn width(&self) -> u32 {
        self.inner.plan.project.width
    }

    pub fn height(&self) -> u32 {
        self.inner.plan.project.height
    }

    pub fn duration_ticks(&self) -> i64 {
        self.inner.plan.duration_ticks
    }

    pub fn total_frames(&self) -> u64 {
        self.inner.plan.total_frames()
    }

    pub fn render_rgba(&mut self, time_ticks: i64) -> Result<Vec<u8>> {
        self.inner.render_frame(time_ticks)
    }

    pub fn render_png(&mut self, time_ticks: i64, output: &Path) -> Result<()> {
        let width = self.width();
        let height = self.height();
        let rgba = self.render_rgba(time_ticks)?;
        write_png(output, width, height, &rgba)
    }

    pub fn export_mp4(&mut self, output: &Path) -> Result<ExportReport> {
        export_plan(&self.inner.plan, output, &self.options, |_| {})
    }
}

pub fn render_frame_rgba(
    plan: &RenderPlan,
    time_ticks: i64,
    options: &RendererOptions,
) -> Result<Vec<u8>> {
    let mut renderer = Renderer::new(plan.clone(), options.clone())?;
    renderer.render_rgba(time_ticks)
}

pub fn render_frame_png(
    plan: &RenderPlan,
    time_ticks: i64,
    output: &Path,
    options: &RendererOptions,
) -> Result<()> {
    let mut renderer = Renderer::new(plan.clone(), options.clone())?;
    renderer.render_png(time_ticks, output)
}

pub fn export_mp4(
    plan: &RenderPlan,
    output: &Path,
    options: &RendererOptions,
) -> Result<ExportReport> {
    export_plan(plan, output, options, |_| {})
}

pub fn export_mp4_with_progress<F>(
    plan: &RenderPlan,
    output: &Path,
    options: &RendererOptions,
    progress: F,
) -> Result<ExportReport>
where
    F: FnMut(ExportProgress),
{
    export_plan(plan, output, options, progress)
}

fn resolve_assets_root(explicit: Option<&Path>) -> PathBuf {
    explicit
        .map(Path::to_path_buf)
        .or_else(|| std::env::var("RENDER_ASSETS_PATH").ok().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("assets"))
}

struct TemporaryAudioFile(Option<PathBuf>);

impl Drop for TemporaryAudioFile {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn export_plan<F>(
    plan: &RenderPlan,
    output_path: &Path,
    options: &RendererOptions,
    mut progress: F,
) -> Result<ExportReport>
where
    F: FnMut(ExportProgress),
{
    plan.validate()?;
    let ffmpeg = options.ffmpeg_path.to_string_lossy().into_owned();
    let assets_root = resolve_assets_root(options.assets_root.as_deref());
    let width = plan.project.width;
    let height = plan.project.height;
    let fps = plan.fps();
    let total_frames = plan.total_frames();
    let duration_seconds = plan.duration_ticks as f64 / TICKS_PER_SECOND;
    let mixed_audio = TemporaryAudioFile(audio::mix_audio_in(
        plan,
        &ffmpeg,
        options.temp_dir.as_deref(),
    )?);
    let effective_audio = mixed_audio.0.as_deref();
    let mut renderer = NativeRenderer::new(plan.clone(), ffmpeg.clone(), assets_root)?;

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    if output_path.exists() {
        std::fs::remove_file(output_path).ok();
    }

    let mut args: Vec<String> = vec![
        "-y".into(),
        "-v".into(),
        "error".into(),
        "-f".into(),
        "rawvideo".into(),
        "-pix_fmt".into(),
        "rgba".into(),
        "-s".into(),
        format!("{width}x{height}"),
        "-r".into(),
        format!("{fps:.6}"),
        "-i".into(),
        "pipe:0".into(),
    ];
    if let Some(audio) = effective_audio {
        args.push("-i".into());
        args.push(audio.display().to_string());
    }
    args.extend([
        "-t".into(),
        format!("{duration_seconds:.6}"),
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "medium".into(),
        "-crf".into(),
        quality_crf(&renderer.plan.quality).to_string(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        "-max_muxing_queue_size".into(),
        "4096".into(),
    ]);
    if effective_audio.is_some() {
        args.extend([
            "-c:a".into(),
            "aac".into(),
            "-b:a".into(),
            "192k".into(),
            "-ar".into(),
            "48000".into(),
            "-ac".into(),
            "2".into(),
        ]);
    } else {
        args.push("-an".into());
    }
    args.extend([
        "-movflags".into(),
        "+faststart".into(),
        output_path.display().to_string(),
    ]);

    let mut child = Command::new(&ffmpeg)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawn ffmpeg: {ffmpeg}"))?;
    let stderr = child.stderr.take().context("ffmpeg stderr unavailable")?;
    let stderr_text = Arc::new(Mutex::new(String::new()));
    let stderr_thread = {
        let stderr_text = Arc::clone(&stderr_text);
        thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 {
                let mut guard = stderr_text.lock().unwrap();
                guard.push_str(&line);
                if guard.len() > 16_000 {
                    let drain = guard.len() - 16_000;
                    guard.drain(0..drain);
                }
                line.clear();
            }
        })
    };

    let mut stdin = child.stdin.take().context("ffmpeg stdin unavailable")?;
    for frame in 0..total_frames {
        let time_ticks = renderer.plan.frame_time_ticks(frame);
        let rgba = match renderer.render_frame_sequential(time_ticks) {
            Ok(rgba) => rgba,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_file(output_path);
                bail!("frame {frame} failed: {error}");
            }
        };
        if let Err(error) = stdin.write_all(&rgba) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(output_path);
            bail!("writing frame {frame} to ffmpeg failed: {error}");
        }
        progress(ExportProgress {
            frame: frame + 1,
            total_frames,
            percent: ((frame + 1) as f64 / total_frames as f64) * 100.0,
        });
    }
    drop(stdin);
    let status = child.wait().context("wait for ffmpeg")?;
    let _ = stderr_thread.join();
    if !status.success() {
        let _ = std::fs::remove_file(output_path);
        let stderr = stderr_text.lock().unwrap().clone();
        bail!("ffmpeg failed with {status}: {stderr}");
    }
    Ok(ExportReport {
        output: output_path.display().to_string(),
        width,
        height,
        frames: total_frames,
        duration_seconds,
        has_audio: effective_audio.is_some(),
    })
}

impl NativeRenderer {
    fn render_mask(
        &mut self,
        mask: &MaskPlan,
        source_width: u32,
        source_height: u32,
        transform: &TransformPlan,
        canvas_width: u32,
        canvas_height: u32,
        element_id: &str,
    ) -> Result<MaskArtifacts> {
        let source_width = source_width as f32;
        let source_height = source_height as f32;
        let contain =
            (canvas_width as f32 / source_width).min(canvas_height as f32 / source_height);
        let geometry = MaskGeometry {
            element_width: (source_width * contain * transform.scale_x).abs(),
            element_height: (source_height * contain * transform.scale_y).abs(),
            center_x: canvas_width as f32 / 2.0 + transform.position_x,
            center_y: canvas_height as f32 / 2.0 + transform.position_y,
            rotation: (transform.rotate + mask_param_f32(&mask.params, "rotation", 0.0))
                .to_radians(),
        };

        let mut rgba = vec![0u8; canvas_width as usize * canvas_height as usize * 4];
        let body_path = build_mask_path(mask, &geometry, false)?;
        let text_pixels = if mask.kind == "text" {
            Some(self.render_text_mask(
                mask,
                &geometry,
                transform,
                canvas_width,
                canvas_height,
                element_id,
            )?)
        } else {
            None
        };
        if let Some(text_pixels) = text_pixels.as_ref() {
            for (source, target) in text_pixels.chunks_exact(4).zip(rgba.chunks_exact_mut(4)) {
                let alpha = source[3];
                target[0] = alpha;
                target[1] = alpha;
                target[2] = alpha;
                target[3] = alpha;
            }
        } else {
            let Some(builder) = body_path else {
                bail!("unsupported native mask type '{}'", mask.kind);
            };
            let path = builder.finish().context("build mask path")?;
            let mut pixmap =
                Pixmap::new(canvas_width, canvas_height).context("allocate mask pixmap")?;
            let mut paint = Paint::default();
            paint.set_color_rgba8(255, 255, 255, 255);
            paint.anti_alias = true;
            pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
            for (source, target) in pixmap.data().chunks_exact(4).zip(rgba.chunks_exact_mut(4)) {
                let alpha = source[3];
                target[0] = alpha;
                target[1] = alpha;
                target[2] = alpha;
                target[3] = alpha;
            }
        }

        let stroke = if mask.stroke_width > 0.0 {
            let color = color::parse_color(&mask.stroke_color).unwrap_or([1.0, 1.0, 1.0, 1.0]);
            let freeform_open = mask.kind == "freeform"
                && !mask
                    .params
                    .get("closed")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
            if color[3] > 0.0 && !freeform_open {
                let pixels = if mask.kind == "text" {
                    let source = text_pixels
                        .as_ref()
                        .context("text mask pixels missing for stroke")?;
                    text_mask_stroke(
                        source,
                        canvas_width,
                        canvas_height,
                        mask.stroke_width,
                        &mask.stroke_align,
                        color,
                    )
                } else {
                    let stroke_path = build_mask_path(mask, &geometry, true)?
                        .context("mask has no stroke geometry")?;
                    let path = stroke_path.finish().context("build stroke path")?;
                    stroke_path_pixels(StrokePixelsArgs {
                        path: &path,
                        width: canvas_width,
                        height: canvas_height,
                        stroke_width: mask.stroke_width,
                        stroke_align: &mask.stroke_align,
                        color,
                        round_joins: mask.kind == "freeform",
                        mask_by_fill: mask.kind == "freeform",
                        fill_alpha: &rgba,
                    })
                };
                Some(StrokePayload {
                    id: format!("native-mask-stroke-{element_id}"),
                    rgba: pixels,
                })
            } else {
                None
            }
        } else {
            None
        };

        let texture_id = format!("native-mask-{element_id}");
        Ok(MaskArtifacts {
            descriptor: compositor::LayerMaskDescriptor {
                texture_id: texture_id.clone(),
                feather: mask.feather,
                inverted: mask.inverted,
            },
            texture_id,
            rgba,
            width: canvas_width,
            height: canvas_height,
            stroke,
        })
    }

    fn render_text_mask(
        &mut self,
        mask: &MaskPlan,
        geometry: &MaskGeometry,
        transform: &TransformPlan,
        canvas_width: u32,
        canvas_height: u32,
        element_id: &str,
    ) -> Result<Vec<u8>> {
        let params = TextParams {
            font_path: None,
            content: mask
                .params
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("Text")
                .to_string(),
            font_size: mask_param_f32(&mask.params, "fontSize", 15.0),
            font_family: mask
                .params
                .get("fontFamily")
                .and_then(Value::as_str)
                .unwrap_or("Arial")
                .to_string(),
            font_weight: mask
                .params
                .get("fontWeight")
                .and_then(Value::as_str)
                .unwrap_or("normal")
                .to_string(),
            font_style: mask
                .params
                .get("fontStyle")
                .and_then(Value::as_str)
                .unwrap_or("normal")
                .to_string(),
            text_align: "center".to_string(),
            text_decoration: "none".to_string(),
            letter_spacing: mask_param_f32(&mask.params, "letterSpacing", 0.0),
            line_height: mask_param_f32(&mask.params, "lineHeight", 1.2),
            color: "#ffffff".to_string(),
            background_enabled: false,
            background_color: "#000000".to_string(),
            background_corner_radius: 0.0,
            background_padding_x: 0.0,
            background_padding_y: 0.0,
            background_offset_x: 0.0,
            background_offset_y: 0.0,
        };
        let scale = mask_param_f32(&mask.params, "scale", 1.0);
        // The Canvas renderer draws the text mask into an element-sized
        // surface, so font size follows the element height, not the canvas
        // height. `render_text_layer` builds at canvas resolution, so fold the
        // element/canvas ratio into the transform scale.
        let element_scale = if canvas_height == 0 {
            1.0
        } else {
            geometry.element_height / canvas_height as f32
        };
        let mask_scale = (element_scale * scale).max(1e-5);
        let text_transform = TransformPlan {
            position_x: transform.position_x
                + mask_param_f32(&mask.params, "centerX", 0.0) * geometry.element_width,
            position_y: transform.position_y
                + mask_param_f32(&mask.params, "centerY", 0.0) * geometry.element_height,
            scale_x: mask_scale,
            scale_y: mask_scale,
            rotate: transform.rotate + mask_param_f32(&mask.params, "rotation", 0.0),
        };
        self.render_text_layer(
            &format!("mask-text-{element_id}"),
            &params,
            &text_transform,
            canvas_width,
            canvas_height,
        )
    }

    fn render_graphic_raster(
        &self,
        definition_id: &str,
        params: &Value,
    ) -> Result<(Vec<u8>, u32, u32)> {
        graphic::render_graphic(definition_id, params)
    }

    fn render_sticker_raster(
        &self,
        asset_path: &str,
        intrinsic_width: Option<u32>,
        intrinsic_height: Option<u32>,
    ) -> Result<(Vec<u8>, u32, u32)> {
        let relative = asset_path.trim_start_matches('/');
        let path = self.assets_root.join(relative);
        let data =
            std::fs::read(&path).with_context(|| format!("read sticker {}", path.display()))?;
        let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default())
            .map_err(|error| anyhow::anyhow!("parse SVG {}: {error}", path.display()))?;
        let tree_size = tree.size();
        let width = intrinsic_width
            .unwrap_or(tree_size.width().round().max(1.0) as u32)
            .max(1);
        let height = intrinsic_height
            .unwrap_or(tree_size.height().round().max(1.0) as u32)
            .max(1);
        let rgba = render_svg_rgba(&data, width, height, &path.display().to_string())?;
        Ok((rgba, width, height))
    }

    fn render_text_layer(
        &mut self,
        element_id: &str,
        params: &TextParams,
        transform: &TransformPlan,
        canvas_width: u32,
        canvas_height: u32,
    ) -> Result<Vec<u8>> {
        let key = format!(
            "{canvas_height}:{}",
            serde_json::to_string(params).context("serialize text params")?
        );
        let cached = if let Some(cached) = self.text_cache.get(&key) {
            Some(Arc::clone(cached))
        } else {
            None
        };
        let texture = match cached {
            Some(texture) => texture,
            None => {
                let texture = Arc::new(self.build_text_texture(params, canvas_height)?);
                self.text_cache.insert(key.clone(), Arc::clone(&texture));
                self.text_cache_order.push_back(key);
                while self.text_cache_order.len() > TEXT_CACHE_LIMIT {
                    if let Some(old) = self.text_cache_order.pop_front() {
                        self.text_cache.remove(&old);
                    }
                }
                texture
            }
        };
        let scale_x = transform.scale_x;
        let scale_y = transform.scale_y;
        let mut output = vec![0u8; canvas_width as usize * canvas_height as usize * 4];
        if scale_x.abs() < 1e-5 || scale_y.abs() < 1e-5 {
            return Ok(output);
        }
        let center_x = canvas_width as f32 / 2.0 + transform.position_x;
        let center_y = canvas_height as f32 / 2.0 + transform.position_y;
        let rotation = transform.rotate.to_radians();
        let cos = rotation.cos();
        let sin = rotation.sin();
        let local_width = texture.width as f32;
        let local_height = texture.height as f32;
        for y in 0..canvas_height {
            for x in 0..canvas_width {
                let dx = x as f32 + 0.5 - center_x;
                let dy = y as f32 + 0.5 - center_y;
                let rotated_x = cos * dx + sin * dy;
                let rotated_y = -sin * dx + cos * dy;
                let local_x = rotated_x / scale_x - texture.origin_x;
                let local_y = rotated_y / scale_y - texture.origin_y;
                if local_x < 0.0
                    || local_y < 0.0
                    || local_x >= local_width
                    || local_y >= local_height
                {
                    continue;
                }
                let sx = local_x.floor() as u32;
                let sy = local_y.floor() as u32;
                let source = (sy * texture.width + sx) as usize * 4;
                let dest = (y * canvas_width + x) as usize * 4;
                let alpha = texture.rgba[source + 3] as f32 / 255.0;
                if alpha <= 0.0 {
                    continue;
                }
                for channel in 0..3 {
                    output[dest + channel] = ((texture.rgba[source + channel] as f32 * alpha)
                        + (output[dest + channel] as f32 * (1.0 - alpha)))
                        .round() as u8;
                }
                output[dest + 3] = ((alpha + (output[dest + 3] as f32 / 255.0) * (1.0 - alpha))
                    * 255.0)
                    .round() as u8;
            }
        }
        let _ = element_id;
        Ok(output)
    }

    fn build_text_texture(
        &mut self,
        params: &TextParams,
        canvas_height: u32,
    ) -> Result<CachedTextTexture> {
        let font_path = fonts::resolve_font_path(
            &params.font_family,
            &params.font_weight,
            &params.font_style,
            params.font_path.as_deref(),
            &self.assets_root,
        )?;
        let font_key = font_path.to_string_lossy().to_string();
        if !self.fonts.contains_key(&font_key) {
            let bytes = std::fs::read(&font_path)
                .with_context(|| format!("read font {}", font_path.display()))?;
            let font = Font::from_bytes(bytes, FontSettings::default())
                .map_err(|error| anyhow::anyhow!("parse font {}: {error}", font_path.display()))?;
            self.fonts.insert(font_key.clone(), font);
        }
        let font = self.fonts.get(&font_key).context("font missing")?;
        let scaled_size = (params.font_size * canvas_height as f32 / 90.0).max(1.0);
        let line_height = (if params.line_height > 0.0 {
            params.line_height
        } else {
            1.2
        }) * scaled_size;
        let lines = if params.content.is_empty() {
            vec!["".to_string()]
        } else {
            params.content.split('\n').map(str::to_string).collect()
        };
        let text_color = color::parse_color(&params.color)?;
        let background_color = color::parse_color(&params.background_color)?;
        let padding_scale = (params.font_size / 15.0).max(1.0);
        let padding_x = if params.background_enabled {
            params.background_padding_x.max(0.0) * padding_scale
        } else {
            0.0
        };
        let padding_y = if params.background_enabled {
            params.background_padding_y.max(0.0) * padding_scale
        } else {
            0.0
        };
        let offset_x = if params.background_enabled {
            params.background_offset_x
        } else {
            0.0
        };
        let offset_y = if params.background_enabled {
            params.background_offset_y
        } else {
            0.0
        };

        let mut line_widths = Vec::with_capacity(lines.len());
        for line in &lines {
            let mut width = 0.0;
            for character in line.chars() {
                let metrics = font.metrics(character, scaled_size);
                width += metrics.advance_width + params.letter_spacing.max(0.0);
            }
            line_widths.push(width);
        }
        let block_width = line_widths.iter().copied().fold(0.0f32, f32::max);
        let block_height = line_height * lines.len() as f32;
        let text_left = match params.text_align.as_str() {
            "left" => 0.0,
            "right" => -block_width,
            _ => -block_width / 2.0,
        };
        let text_top = -block_height / 2.0;
        let background_width = block_width + padding_x * 2.0;
        let background_height = block_height + padding_y * 2.0;
        let background_left = text_left - padding_x + offset_x;
        let background_top = text_top - padding_y + offset_y;
        let margin = scaled_size * 0.1 + 1.0;
        let min_x = if params.background_enabled {
            text_left.min(background_left)
        } else {
            text_left
        } - margin;
        let min_y = if params.background_enabled {
            text_top.min(background_top)
        } else {
            text_top
        } - margin;
        let max_x = if params.background_enabled {
            (text_left + block_width).max(background_left + background_width)
        } else {
            text_left + block_width
        } + margin;
        let max_y = if params.background_enabled {
            (text_top + block_height).max(background_top + background_height)
        } else {
            text_top + block_height
        } + margin;
        let local_width = (max_x - min_x).ceil().max(1.0) as u32;
        let local_height = (max_y - min_y).ceil().max(1.0) as u32;
        let mut local = vec![0u8; local_width as usize * local_height as usize * 4];

        if params.background_enabled && background_color[3] > 0.0 {
            let radius = params.background_corner_radius.clamp(0.0, 100.0) / 100.0
                * background_width.min(background_height)
                / 2.0;
            fill_rounded_rect(
                &mut local,
                local_width,
                local_height,
                background_left - min_x,
                background_top - min_y,
                background_width,
                background_height,
                radius,
                background_color,
            );
        }

        let ascent = font
            .horizontal_line_metrics(scaled_size)
            .map(|metrics| metrics.ascent)
            .unwrap_or(scaled_size * 0.8);
        for (line_index, line) in lines.iter().enumerate() {
            let line_width = line_widths[line_index];
            let start_x = text_left - min_x
                + match params.text_align.as_str() {
                    "left" => 0.0,
                    "right" => block_width - line_width,
                    _ => (block_width - line_width) / 2.0,
                };
            let baseline = text_top - min_y + ascent + line_index as f32 * line_height;
            let mut pen_x = start_x;
            for character in line.chars() {
                let (metrics, bitmap) = font.rasterize(character, scaled_size);
                let glyph_x = pen_x + metrics.xmin as f32;
                let glyph_y = baseline - metrics.ymin as f32 - metrics.height as f32;
                blit_alpha_bitmap(
                    &mut local,
                    local_width,
                    local_height,
                    &bitmap,
                    metrics.width,
                    metrics.height,
                    glyph_x,
                    glyph_y,
                    text_color,
                );
                pen_x += metrics.advance_width + params.letter_spacing.max(0.0);
            }
        }

        Ok(CachedTextTexture {
            rgba: local,
            width: local_width,
            height: local_height,
            origin_x: min_x,
            origin_y: min_y,
        })
    }

    fn upload_texture(&self, rgba: &[u8], width: u32, height: u32, label: &str) -> wgpu::Texture {
        let texture = self
            .context
            .device()
            .create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
        self.context.queue().write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        texture
    }
}

struct MaskGeometry {
    element_width: f32,
    element_height: f32,
    center_x: f32,
    center_y: f32,
    rotation: f32,
}

/// Build the mask body path (or the stroke path when `for_stroke` is set).
fn build_mask_path(
    mask: &MaskPlan,
    geometry: &MaskGeometry,
    for_stroke: bool,
) -> Result<Option<PathBuilder>> {
    let params = &mask.params;
    let element_width = geometry.element_width;
    let element_height = geometry.element_height;
    let center_x = geometry.center_x;
    let center_y = geometry.center_y;
    let rotation = geometry.rotation;
    let offset = if for_stroke {
        stroke_offset(&mask.stroke_align, mask.stroke_width)
    } else {
        0.0
    };
    let mut builder = PathBuilder::new();
    match mask.kind.as_str() {
        "split" => {
            let line_center_x = center_x + mask_param_f32(params, "centerX", 0.0) * element_width;
            let line_center_y = center_y + mask_param_f32(params, "centerY", 0.0) * element_height;
            let normal = (rotation.cos(), rotation.sin());
            let tangent = (-rotation.sin(), rotation.cos());
            if for_stroke {
                let half_width = element_width / 2.0;
                let half_height = element_height / 2.0;
                let left = center_x - half_width;
                let right = center_x + half_width;
                let top = center_y - half_height;
                let bottom = center_y + half_height;
                let mut intersections = Vec::new();
                for (x1, y1, x2, y2) in [
                    (left, top, right, top),
                    (right, top, right, bottom),
                    (right, bottom, left, bottom),
                    (left, bottom, left, top),
                ] {
                    if let Some(point) = line_segment_intersection(
                        line_center_x,
                        line_center_y,
                        tangent.0,
                        tangent.1,
                        x1,
                        y1,
                        x2,
                        y2,
                    ) {
                        if !intersections.iter().any(|existing: &(f32, f32)| {
                            (existing.0 - point.0).abs() < 1e-3
                                && (existing.1 - point.1).abs() < 1e-3
                        }) {
                            intersections.push(point);
                        }
                    }
                }
                if intersections.len() != 2 {
                    return Ok(None);
                }
                builder.move_to(intersections[0].0, intersections[0].1);
                builder.line_to(intersections[1].0, intersections[1].1);
            } else {
                let large = 100_000.0;
                builder.move_to(
                    line_center_x + tangent.0 * large,
                    line_center_y + tangent.1 * large,
                );
                builder.line_to(
                    line_center_x + tangent.0 * large + normal.0 * large,
                    line_center_y + tangent.1 * large + normal.1 * large,
                );
                builder.line_to(
                    line_center_x - tangent.0 * large + normal.0 * large,
                    line_center_y - tangent.1 * large + normal.1 * large,
                );
                builder.line_to(
                    line_center_x - tangent.0 * large,
                    line_center_y - tangent.1 * large,
                );
                builder.close();
            }
        }
        "rectangle" | "cinematic-bars" | "ellipse" | "heart" | "diamond" | "star" => {
            let center_offset_x = mask_param_f32(params, "centerX", 0.0) * element_width;
            let center_offset_y = mask_param_f32(params, "centerY", 0.0) * element_height;
            let shape_center_x = center_x + center_offset_x;
            let shape_center_y = center_y + center_offset_y;
            let mut shape_width = mask_param_f32(params, "width", 0.6).max(0.01) * element_width;
            let shape_height = mask_param_f32(params, "height", 0.6).max(0.01) * element_height;
            if mask.kind == "cinematic-bars" {
                shape_width = shape_width.max(element_width);
            }
            let half_width = (shape_width / 2.0 + offset).max(1.0);
            let half_height = (shape_height / 2.0 + offset).max(1.0);
            match mask.kind.as_str() {
                "rectangle" | "cinematic-bars" => {
                    add_rotated_polygon(
                        &mut builder,
                        shape_center_x,
                        shape_center_y,
                        half_width,
                        half_height,
                        rotation,
                        &[(1.0, 1.0), (1.0, -1.0), (-1.0, -1.0), (-1.0, 1.0)],
                    );
                }
                "ellipse" => {
                    add_rotated_ellipse(
                        &mut builder,
                        shape_center_x,
                        shape_center_y,
                        half_width,
                        half_height,
                        rotation,
                    );
                }
                "diamond" => {
                    add_rotated_polygon(
                        &mut builder,
                        shape_center_x,
                        shape_center_y,
                        half_width,
                        half_height,
                        rotation,
                        &[(0.0, -1.0), (1.0, 0.0), (0.0, 1.0), (-1.0, 0.0)],
                    );
                }
                "star" => {
                    let mut points = Vec::with_capacity(10);
                    for index in 0..10 {
                        let inner = index % 2 == 1;
                        let radius = if inner { 0.45 } else { 1.0 };
                        let angle =
                            index as f32 * std::f32::consts::PI / 5.0 - std::f32::consts::FRAC_PI_2;
                        points.push((radius * angle.cos(), radius * angle.sin()));
                    }
                    add_rotated_polygon(
                        &mut builder,
                        shape_center_x,
                        shape_center_y,
                        half_width,
                        half_height,
                        rotation,
                        &points,
                    );
                }
                "heart" => {
                    add_rotated_heart(
                        &mut builder,
                        shape_center_x,
                        shape_center_y,
                        half_width,
                        half_height,
                        rotation,
                    );
                }
                _ => unreachable!(),
            }
        }
        "freeform" => {
            let Some(points) = params.get("path").and_then(Value::as_array) else {
                bail!("freeform mask has no path");
            };
            let scale = mask_param_f32(params, "scale", 1.0);
            let offset_x = center_x + mask_param_f32(params, "centerX", 0.0) * element_width;
            let offset_y = center_y + mask_param_f32(params, "centerY", 0.0) * element_height;
            let cos = rotation.cos();
            let sin = rotation.sin();
            let mut first = true;
            for point in points {
                let x = point.get("x").and_then(Value::as_f64).unwrap_or(0.0) as f32
                    * element_width
                    * scale;
                let y = point.get("y").and_then(Value::as_f64).unwrap_or(0.0) as f32
                    * element_height
                    * scale;
                let px = offset_x + x * cos - y * sin;
                let py = offset_y + x * sin + y * cos;
                if first {
                    builder.move_to(px, py);
                    first = false;
                } else {
                    builder.line_to(px, py);
                }
            }
            if params
                .get("closed")
                .and_then(Value::as_bool)
                .unwrap_or(true)
            {
                builder.close();
            }
        }
        "text" => return Ok(None),
        other => bail!("unsupported native mask type '{other}'"),
    }
    Ok(Some(builder))
}

fn line_segment_intersection(
    line_x: f32,
    line_y: f32,
    direction_x: f32,
    direction_y: f32,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
) -> Option<(f32, f32)> {
    let edge_x = x2 - x1;
    let edge_y = y2 - y1;
    let denominator = direction_x * edge_y - direction_y * edge_x;
    if denominator.abs() < 1e-9 {
        return None;
    }
    let t = ((x1 - line_x) * edge_y - (y1 - line_y) * edge_x) / denominator;
    let u = ((x1 - line_x) * direction_y - (y1 - line_y) * direction_x) / denominator;
    if !(-1e-6..=1.0 + 1e-6).contains(&u) {
        return None;
    }
    Some((line_x + direction_x * t, line_y + direction_y * t))
}

struct StrokePixelsArgs<'a> {
    path: &'a tiny_skia::Path,
    width: u32,
    height: u32,
    stroke_width: f32,
    stroke_align: &'a str,
    color: [f32; 4],
    round_joins: bool,
    mask_by_fill: bool,
    fill_alpha: &'a [u8],
}

fn stroke_path_pixels(args: StrokePixelsArgs<'_>) -> Vec<u8> {
    let mut pixmap = match Pixmap::new(args.width, args.height) {
        Some(pixmap) => pixmap,
        None => return vec![0u8; args.width as usize * args.height as usize * 4],
    };
    let mut paint = Paint::default();
    paint.set_color(
        tiny_skia::Color::from_rgba(args.color[0], args.color[1], args.color[2], args.color[3])
            .unwrap_or(tiny_skia::Color::TRANSPARENT),
    );
    paint.anti_alias = true;
    let stroke = Stroke {
        width: args.stroke_width.max(0.01),
        line_cap: if args.round_joins {
            LineCap::Round
        } else {
            LineCap::Butt
        },
        line_join: if args.round_joins {
            LineJoin::Round
        } else {
            LineJoin::Miter
        },
        ..Default::default()
    };
    pixmap.stroke_path(args.path, &paint, &stroke, Transform::identity(), None);
    let mut straight = pixmap_to_straight_rgba(pixmap.data(), args.width, args.height);
    if args.mask_by_fill && args.stroke_align != "center" {
        apply_fill_alpha_mask(&mut straight, args.fill_alpha, args.stroke_align);
    }
    straight
}

fn render_svg_rgba(data: &[u8], width: u32, height: u32, label: &str) -> Result<Vec<u8>> {
    let tree = resvg::usvg::Tree::from_data(data, &resvg::usvg::Options::default())
        .map_err(|error| anyhow::anyhow!("parse SVG {label}: {error}"))?;
    let tree_size = tree.size();
    let width = width.max(1);
    let height = height.max(1);
    let mut pixmap = Pixmap::new(width, height).context("allocate SVG pixmap")?;
    let scale = (width as f32 / tree_size.width()).min(height as f32 / tree_size.height());
    let transform = Transform::from_scale(scale, scale).post_translate(
        (width as f32 - tree_size.width() * scale) / 2.0,
        (height as f32 - tree_size.height() * scale) / 2.0,
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    Ok(pixmap_to_straight_rgba(pixmap.data(), width, height))
}

fn pixmap_to_straight_rgba(premultiplied: &[u8], width: u32, height: u32) -> Vec<u8> {
    let _ = (width, height);
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

fn apply_fill_alpha_mask(stroke: &mut [u8], body: &[u8], stroke_align: &str) {
    let inside = stroke_align == "inside";
    for (stroke_pixel, body_pixel) in stroke.chunks_exact_mut(4).zip(body.chunks_exact(4)) {
        let coverage = body_pixel[3] as f32 / 255.0;
        let factor = if inside { coverage } else { 1.0 - coverage };
        // The stroke buffer is straight-alpha; only alpha is clipped by the
        // body, the stroke color stays constant like Canvas destination-in/out.
        stroke_pixel[3] = (stroke_pixel[3] as f32 * factor).round() as u8;
    }
}

/// Approximate the Canvas text stroke (a stroke of the glyph outlines) with a
/// distance-transform ring around the rasterized glyph coverage.
fn text_mask_stroke(
    mask: &[u8],
    width: u32,
    height: u32,
    stroke_width: f32,
    stroke_align: &str,
    color: [f32; 4],
) -> Vec<u8> {
    let pixel_count = width as usize * height as usize;
    let mut inside = vec![false; pixel_count];
    for (index, pixel) in mask.chunks_exact(4).enumerate() {
        inside[index] = pixel[3] >= 128;
    }
    let mut distance_to_inside = vec![f32::MAX; pixel_count];
    let mut distance_to_outside = vec![f32::MAX; pixel_count];
    for index in 0..pixel_count {
        if inside[index] {
            distance_to_inside[index] = 0.0;
        } else {
            distance_to_outside[index] = 0.0;
        }
    }
    chamfer_distance(&mut distance_to_inside, width, height);
    chamfer_distance(&mut distance_to_outside, width, height);
    let half = (stroke_width / 2.0).max(0.5);
    let mut output = vec![0u8; pixel_count * 4];
    for index in 0..pixel_count {
        let coverage = if inside[index] {
            match stroke_align {
                "outside" => 0.0,
                _ => (half - distance_to_outside[index] + 0.5).clamp(0.0, 1.0),
            }
        } else {
            match stroke_align {
                "inside" => 0.0,
                _ => (half - distance_to_inside[index] + 0.5).clamp(0.0, 1.0),
            }
        };
        if coverage <= 0.0 {
            continue;
        }
        let alpha = coverage * color[3];
        let base = index * 4;
        output[base] = (color[0] * 255.0).round() as u8;
        output[base + 1] = (color[1] * 255.0).round() as u8;
        output[base + 2] = (color[2] * 255.0).round() as u8;
        output[base + 3] = (alpha * 255.0).round() as u8;
    }
    output
}

fn chamfer_distance(distance: &mut [f32], width: u32, height: u32) {
    let width = width as usize;
    let height = height as usize;
    let diagonal = std::f32::consts::SQRT_2;
    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            let mut value = distance[index];
            if y > 0 {
                value = value.min(distance[index - width] + 1.0);
                if x > 0 {
                    value = value.min(distance[index - width - 1] + diagonal);
                }
                if x + 1 < width {
                    value = value.min(distance[index - width + 1] + diagonal);
                }
            }
            if x > 0 {
                value = value.min(distance[index - 1] + 1.0);
            }
            distance[index] = value;
        }
    }
    for y in (0..height).rev() {
        for x in (0..width).rev() {
            let index = y * width + x;
            let mut value = distance[index];
            if y + 1 < height {
                value = value.min(distance[index + width] + 1.0);
                if x > 0 {
                    value = value.min(distance[index + width - 1] + diagonal);
                }
                if x + 1 < width {
                    value = value.min(distance[index + width + 1] + diagonal);
                }
            }
            if x + 1 < width {
                value = value.min(distance[index + 1] + 1.0);
            }
            distance[index] = value;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_plan(background: BackgroundPlan) -> RenderPlan {
        RenderPlan {
            version: 1,
            project: ProjectPlan {
                width: 1280,
                height: 720,
                fps: FpsPlan {
                    numerator: 30,
                    denominator: 1,
                },
                background,
            },
            duration_ticks: 1_800_000,
            media: vec![],
            tracks: vec![],
            audio: AudioPlan::default(),
            quality: "high".into(),
        }
    }

    #[test]
    fn deserializes_camel_case_backgrounds() {
        let base = json!({
            "version": 1,
            "project": {
                "width": 640,
                "height": 360,
                "fps": { "numerator": 30, "denominator": 1 },
                "background": { "type": "blur", "blurIntensity": 25.0 }
            },
            "durationTicks": 120000,
            "media": [],
            "tracks": [],
            "audio": { "elements": [] },
            "quality": "high"
        });
        let blur_plan: RenderPlan = serde_json::from_value(base.clone()).unwrap();
        match blur_plan.project.background {
            BackgroundPlan::Blur { blur_intensity } => {
                assert!((blur_intensity - 25.0).abs() < 1e-6);
            }
            other => panic!("unexpected background: {other:?}"),
        }
        let mut gradient = base;
        gradient["project"]["background"] = json!({
            "type": "gradient",
            "css": "linear-gradient(90deg, #000000, #ffffff)"
        });
        let gradient_plan: RenderPlan = serde_json::from_value(gradient).unwrap();
        match gradient_plan.project.background {
            BackgroundPlan::Gradient { css } => assert!(css.contains("linear-gradient")),
            other => panic!("unexpected background: {other:?}"),
        }
    }

    #[test]
    fn frame_time_uses_rational_fps() {
        let plan = test_plan(BackgroundPlan::Color {
            color: "#000000".into(),
        });
        assert_eq!(plan.total_frames(), 450);
        assert_eq!(plan.frame_time_ticks(450), 1_800_000);
    }

    #[test]
    fn cover_scale_fills_canvas_and_centers() {
        let source = vec![255u8; 4 * 2];
        let scaled = cover_scale_rgba(&source, 2, 1, 4, 4);
        assert_eq!(scaled.len(), 4 * 4 * 4);
        assert!(scaled.iter().all(|value| *value == 255));
    }

    #[test]
    fn stroke_offsets_match_alignment() {
        assert_eq!(stroke_offset("inside", 8.0), -4.0);
        assert_eq!(stroke_offset("outside", 8.0), 4.0);
        assert_eq!(stroke_offset("center", 8.0), 0.0);
    }

    #[test]
    fn text_mask_stroke_produces_a_ring() {
        let mut mask = vec![0u8; 9 * 9 * 4];
        for y in 3..6 {
            for x in 3..6 {
                let index = (y * 9 + x) * 4;
                mask[index + 3] = 255;
            }
        }
        // A 3px stroke keeps the 3x3 block hollow: its centre is exactly two
        // pixels away from the outline, outside the 1.5px inner half.
        let stroke = text_mask_stroke(&mask, 9, 9, 3.0, "center", [1.0, 1.0, 1.0, 1.0]);
        let alpha = |x: usize, y: usize| stroke[(y * 9 + x) * 4 + 3];
        assert!(alpha(4, 4) < 40, "center should stay hollow");
        assert!(alpha(3, 4) > 100, "inner ring should be painted");
        assert!(alpha(2, 4) > 100, "outer ring should be painted");

        let inside = text_mask_stroke(&mask, 9, 9, 3.0, "inside", [1.0, 1.0, 1.0, 1.0]);
        let inside_alpha = |x: usize, y: usize| inside[(y * 9 + x) * 4 + 3];
        assert!(inside_alpha(3, 4) > 100);
        assert_eq!(
            inside_alpha(2, 4),
            0,
            "inside strokes stay inside the glyph"
        );

        let outside = text_mask_stroke(&mask, 9, 9, 3.0, "outside", [1.0, 1.0, 1.0, 1.0]);
        let outside_alpha = |x: usize, y: usize| outside[(y * 9 + x) * 4 + 3];
        assert_eq!(
            outside_alpha(3, 4),
            0,
            "outside strokes stay outside the glyph"
        );
        assert!(outside_alpha(2, 4) > 100);
    }

    fn color_channel(from: [f32; 4], to: [f32; 4]) -> Value {
        let component = |left: f32, right: f32| {
            json!({
                "keys": [
                    { "time": 0, "value": left, "segmentToNext": "linear" },
                    { "time": 1000, "value": right, "segmentToNext": "linear" }
                ]
            })
        };
        json!({
            "r": component(from[0], to[0]),
            "g": component(from[1], to[1]),
            "b": component(from[2], to[2]),
            "a": component(from[3], to[3])
        })
    }

    #[test]
    fn text_color_and_background_keyframes_resolve() {
        let animations = json!({
            "color": color_channel([0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0, 1.0]),
            "background.color": color_channel([1.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0]),
            "background.paddingX": {
                "keys": [
                    { "time": 0, "value": 4, "segmentToNext": "linear" },
                    { "time": 1000, "value": 24, "segmentToNext": "linear" }
                ]
            },
            "background.cornerRadius": {
                "keys": [
                    { "time": 0, "value": 0, "segmentToNext": "linear" },
                    { "time": 1000, "value": 50, "segmentToNext": "linear" }
                ]
            }
        });
        let base = TextParams {
            font_path: None,
            content: "keyframes".into(),
            font_size: 15.0,
            font_family: "Arial".into(),
            font_weight: "normal".into(),
            font_style: "normal".into(),
            text_align: "center".into(),
            text_decoration: "none".into(),
            letter_spacing: 0.0,
            line_height: 1.2,
            color: "#000000".into(),
            background_enabled: true,
            background_color: "#ff0000".into(),
            background_corner_radius: 0.0,
            background_padding_x: 4.0,
            background_padding_y: 4.0,
            background_offset_x: 0.0,
            background_offset_y: 0.0,
        };
        let start = resolve_text_params(Some(&animations), 0, &base);
        let middle = resolve_text_params(Some(&animations), 500, &base);
        assert_eq!(start.color, "#000000");
        assert_eq!(middle.color, "#bcbcbc");
        assert_eq!(middle.background_color, "#bc00bc");
        assert!((middle.background_padding_x - 14.0).abs() < 1e-4);
        assert!((middle.background_corner_radius - 25.0).abs() < 1e-4);
    }

    #[test]
    fn open_freeform_masks_are_inactive() {
        let open = MaskPlan {
            kind: "freeform".into(),
            feather: 0.0,
            inverted: false,
            stroke_color: "#ffffff".into(),
            stroke_width: 4.0,
            stroke_align: "center".into(),
            params: json!({ "path": [], "closed": false }),
        };
        let mut closed = open.clone();
        closed.params = json!({ "path": [], "closed": true });
        assert!(!mask_is_active(&open));
        assert!(mask_is_active(&closed));
    }

    #[test]
    fn freeform_points_scale_with_the_element_bounds() {
        let mask = MaskPlan {
            kind: "freeform".into(),
            feather: 0.0,
            inverted: false,
            stroke_color: "#ffffff".into(),
            stroke_width: 4.0,
            stroke_align: "center".into(),
            params: json!({
                "path": [
                    { "x": -0.5, "y": -0.5 },
                    { "x": 0.5, "y": -0.5 },
                    { "x": 0.5, "y": 0.5 },
                    { "x": -0.5, "y": 0.5 }
                ],
                "closed": true,
                "scale": 1.0,
                "centerX": 0.0,
                "centerY": 0.0
            }),
        };
        let geometry = MaskGeometry {
            element_width: 200.0,
            element_height: 100.0,
            center_x: 100.0,
            center_y: 50.0,
            rotation: 0.0,
        };
        let path = build_mask_path(&mask, &geometry, false)
            .unwrap()
            .unwrap()
            .finish()
            .unwrap();
        let bounds = path.bounds();
        assert!((bounds.left() - 0.0).abs() < 1e-3, "{bounds:?}");
        assert!((bounds.right() - 200.0).abs() < 1e-3, "{bounds:?}");
        assert!((bounds.top() - 0.0).abs() < 1e-3, "{bounds:?}");
        assert!((bounds.bottom() - 100.0).abs() < 1e-3, "{bounds:?}");
    }
    #[test]
    fn committed_schema_matches_the_plan_types() {
        let committed = include_str!("../../../schema/render-plan.schema.json");
        let generated = serde_json::to_string_pretty(&plan_schema()).unwrap();
        assert_eq!(committed.trim(), generated.trim());
    }

    #[test]
    fn rejects_unsupported_plan_versions() {
        let plan = RenderPlan::from_json_str(
            r##"{"version":99,"project":{"width":10,"height":10,"fps":{"numerator":30,"denominator":1},"background":{"type":"color","color":"#000000"}},"durationTicks":120000,"media":[],"tracks":[]}"##,
        );
        assert!(plan.is_err());
    }

    #[test]
    fn svg_image_rasterizes_to_requested_rgba() {
        let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"64\" height=\"64\"><rect width=\"64\" height=\"64\" fill=\"#ff0000\"/></svg>";
        let rgba = render_svg_rgba(svg, 64, 64, "test.svg").unwrap();
        assert_eq!(rgba.len(), 64 * 64 * 4);
        let center = ((32 * 64) + 32) * 4;
        assert_eq!(&rgba[center..center + 4], &[255, 0, 0, 255]);
    }
}
