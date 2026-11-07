use base64::{engine::general_purpose, Engine as _};
use image::{
    codecs::gif::{GifEncoder, Repeat},
    codecs::png::PngEncoder,
    codecs::webp::WebPEncoder,
    imageops::{overlay, resize, FilterType},
    AnimationDecoder, ColorType, Delay, DynamicImage, GenericImageView, ImageDecoder, ImageEncoder,
    Rgba, RgbaImage,
};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
#[cfg(feature = "desktop")]
use std::thread::JoinHandle;
use std::{
    cmp::Ordering,
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, ErrorKind, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering as AtomicOrdering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
#[cfg(feature = "desktop")]
use tauri::Manager;
#[cfg(feature = "desktop")]
use tauri_plugin_opener::OpenerExt;
use thiserror::Error;
use walkdir::WalkDir;

mod pixel_snap;
use pixel_snap::{PixelGrid, PixelSnapEngine, PixelSnapOptions};

const APP_NAME: &str = "sprite-atlas-packer";
const VERSION: &str = "26.11.8";
const DEFAULT_DURATION_MS: u32 = 120;
const DEFAULT_MAX_SIZE: u32 = 16384;
const RASTER_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif"];
const ASEPRITE_EXTENSIONS: &[&str] = &["ase", "aseprite"];
const SUPPORTED_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "ase", "aseprite"];
const ASEPRITE_BIN_ENV: &str = "SPRITE_ATLAS_PACKER_ASEPRITE_BIN";
const AGENT_API_REGISTRY_FILE: &str = "agent-api-registry.json";
const SAP_DEFAULT_PORT: u16 = 9877;
const SAP_BIND_ADDRESS: &str = "127.0.0.1";
#[cfg(feature = "desktop")]
const DEFAULT_WINDOW_WIDTH: u32 = 1920;
#[cfg(feature = "desktop")]
const DEFAULT_WINDOW_HEIGHT: u32 = 1080;
#[cfg(feature = "desktop")]
const MIN_WINDOW_WIDTH: u32 = 860;
#[cfg(feature = "desktop")]
const MIN_WINDOW_HEIGHT: u32 = 540;
#[cfg(feature = "desktop")]
const SMALL_MONITOR_WIDTH_RATIO: f64 = 0.94;
#[cfg(feature = "desktop")]
const SMALL_MONITOR_HEIGHT_RATIO: f64 = 0.9;
const GENERATED_GRID_POLICY: &str =
    "auto_detect_grid_from_sprite_distances_then_fallback_proportional";
const GENERATED_PROPORTION_POLICY: &str = "global_uniform_scale_preserve_sprite_proportions";
const GENERATED_ROOT_ANCHOR_POLICY: &str = "fixed_upper_body_centroid_anchor_position_stable";

#[derive(Debug, Error)]
pub enum AtlasError {
    #[error("{0}")]
    Message(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Image error: {0}")]
    Image(#[from] image::ImageError),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Walkdir error: {0}")]
    Walkdir(#[from] walkdir::Error),
}

pub type Result<T> = std::result::Result<T, AtlasError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Webp,
    Png,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliceMode {
    Grid,
    Alpha,
}

impl SliceMode {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "grid" => Ok(Self::Grid),
            "alpha" => Ok(Self::Alpha),
            _ => Err(AtlasError::Message(format!(
                "--slice must be grid or alpha; got {value}"
            ))),
        }
    }
}

impl OutputFormat {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "webp" => Ok(Self::Webp),
            "png" => Ok(Self::Png),
            "both" => Ok(Self::Both),
            _ => Err(AtlasError::Message(format!(
                "--format must be webp, png, or both; got {value}"
            ))),
        }
    }

    fn atlas_image_name(self) -> &'static str {
        match self {
            Self::Png => "sheet.png",
            Self::Webp | Self::Both => "sheet.webp",
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Webp => "webp",
            Self::Png => "png",
            Self::Both => "both",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackingStrategy {
    MaxRects,
    Shelf,
    Guillotine,
    Quadtree,
}

impl PackingStrategy {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "maxrects" => Ok(Self::MaxRects),
            "shelf" => Ok(Self::Shelf),
            "guillotine" => Ok(Self::Guillotine),
            "quadtree" => Ok(Self::Quadtree),
            _ => Err(AtlasError::Message(format!(
                "--algo must be maxrects, shelf, guillotine, or quadtree; got {value}"
            ))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::MaxRects => "maxrects",
            Self::Shelf => "shelf",
            Self::Guillotine => "guillotine",
            Self::Quadtree => "quadtree",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PackOptions {
    pub input: PathBuf,
    pub output: PathBuf,
    pub format: OutputFormat,
    pub packing_strategy: PackingStrategy,
    pub pot: bool,
    pub padding: u32,
    pub max_size: u32,
    pub rotate: bool,
    pub trim: bool,
    pub dry_run: bool,
    pub watch: bool,
    pub slice_mode: Option<SliceMode>,
    pub cell: Option<(u32, u32)>,
    pub prefix: String,
    pub aseprite_json: bool,
    pub godot_spriteframes: bool,
    pub unity_importer: bool,
    pub animation_regex: Option<String>,
    pub animations: Vec<AnimationDefinition>,
    pub stable_pivot: bool,
    pub dedup_threshold: Option<u8>,
    pub linked_channels: Vec<String>,
    pub normalize_generated: bool,
    pub frame_size: Option<(u32, u32)>,
}

#[derive(Debug, Clone)]
pub struct GeneratedLoopOptions {
    pub input: PathBuf,
    pub output: PathBuf,
    pub format: OutputFormat,
    pub dry_run: bool,
    pub columns: Option<u32>,
    pub prefix: String,
    pub aseprite_json: bool,
    pub frame_size: Option<(u32, u32)>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectFile {
    #[serde(skip_serializing_if = "Option::is_none")]
    source_folder: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pot: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    padding: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_size: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rotate: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    trim: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    aseprite_json: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    godot_spriteframes: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unity_importer: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    animation_regex: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stable_pivot: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dedup_threshold: Option<u8>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    linked_channels: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pixel_snap: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pixel_snap_size: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pixel_snap_colors: Option<usize>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pixel_snap_palette: String,
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pixel_snap_lock_grid: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pixel_snap_engine: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    animations: Vec<AnimationDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    packing_strategy: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationDefinition {
    pub name: String,
    #[serde(default)]
    pub frames: Vec<String>,
    #[serde(default = "default_looped", rename = "loop")]
    pub looped: bool,
    #[serde(default = "default_fps_hint", alias = "fps_hint")]
    pub fps_hint: f32,
    #[serde(default)]
    pub pivot: Option<Pivot>,
    #[serde(default, alias = "duration_ms")]
    pub duration_ms: Option<u32>,
    #[serde(default, alias = "frame_overrides")]
    pub frame_overrides: BTreeMap<String, FrameOverride>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameOverride {
    #[serde(default)]
    pub pivot: Option<Pivot>,
    #[serde(default, alias = "duration_ms")]
    pub duration_ms: Option<u32>,
}

fn default_looped() -> bool {
    true
}

fn default_fps_hint() -> f32 {
    1000.0 / DEFAULT_DURATION_MS as f32
}

fn default_true() -> bool {
    true
}

fn default_padding() -> u32 {
    2
}

fn default_max_size() -> u32 {
    DEFAULT_MAX_SIZE
}

fn default_webp_format() -> String {
    "webp".to_string()
}

fn default_png_format() -> String {
    "png".to_string()
}

fn default_packing_strategy() -> String {
    "maxrects".to_string()
}

fn default_grid_mode() -> String {
    "grid".to_string()
}

fn default_frame_prefix() -> String {
    "frame".to_string()
}

fn default_generated_frame_size() -> u32 {
    256
}

fn default_pixel_snap_colors() -> usize {
    16
}

fn default_pixel_snap_engine() -> String {
    "fixer".to_string()
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn is_true(value: &bool) -> bool {
    *value
}

fn default_walk_frame_count() -> u32 {
    16
}

fn default_cat_walk_action() -> String {
    "cat_walk".to_string()
}

fn default_cat_walk_prefix() -> String {
    "cat_walk".to_string()
}

#[derive(Debug, Default)]
struct CliOverrides {
    input: bool,
    output: bool,
    format: bool,
    packing_strategy: bool,
    pot: bool,
    padding: bool,
    max_size: bool,
    rotate: bool,
    trim: bool,
    aseprite_json: bool,
    godot_spriteframes: bool,
    unity_importer: bool,
    animation_regex: bool,
    stable_pivot: bool,
    dedup_threshold: bool,
    linked_channels: bool,
}

impl Default for PackOptions {
    fn default() -> Self {
        Self {
            input: PathBuf::new(),
            output: PathBuf::new(),
            format: OutputFormat::Webp,
            packing_strategy: PackingStrategy::MaxRects,
            pot: true,
            padding: 2,
            max_size: DEFAULT_MAX_SIZE,
            rotate: false,
            trim: true,
            dry_run: false,
            watch: false,
            slice_mode: None,
            cell: None,
            prefix: "frame".to_string(),
            aseprite_json: false,
            godot_spriteframes: false,
            unity_importer: false,
            animation_regex: None,
            animations: Vec::new(),
            stable_pivot: false,
            dedup_threshold: None,
            linked_channels: Vec::new(),
            normalize_generated: false,
            frame_size: None,
        }
    }
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PackReport {
    pub input_count: usize,
    pub atlas_width: u32,
    pub atlas_height: u32,
    pub packing_strategy: String,
    pub max_size: u32,
    pub used_pixels: u64,
    pub waste_percent: f64,
    pub frames: Vec<String>,
    pub animations: Vec<String>,
    pub wrote: Vec<String>,
    pub aliases: Vec<AliasReport>,
    pub atlas: Atlas,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SliceReport {
    pub frame_count: usize,
    pub sheet_width: u32,
    pub sheet_height: u32,
    pub frames: Vec<String>,
    pub wrote: Vec<String>,
    pub atlas: Atlas,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normalization: Option<SliceNormalizationReport>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SliceNormalizationReport {
    pub source_sheet_size: Size,
    pub output_sheet_size: Size,
    pub output_frame_size: Size,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_columns: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_rows: Option<u32>,
    pub source_cells: Vec<JsonRect>,
    pub frame_bboxes: Vec<Option<JsonRect>>,
    pub root_anchors: Vec<Option<Pivot>>,
    pub scale: f32,
    pub grid_policy: String,
    pub grid_detection: String,
    pub proportion_policy: String,
    pub root_anchor_policy: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuity: Option<SliceContinuityReport>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SliceContinuityReport {
    pub frame_count: usize,
    pub protocol: String,
    pub median_neighbor_delta: f32,
    pub loop_delta: f32,
    pub max_neighbor_delta: f32,
    pub warnings: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuiPackOptions {
    pub input: String,
    pub output: String,
    #[serde(default = "default_webp_format")]
    pub format: String,
    #[serde(default = "default_packing_strategy")]
    pub packing_strategy: String,
    #[serde(default = "default_true")]
    pub pot: bool,
    #[serde(default = "default_padding")]
    pub padding: u32,
    #[serde(default = "default_max_size")]
    pub max_size: u32,
    #[serde(default)]
    pub rotate: bool,
    #[serde(default = "default_true")]
    pub trim: bool,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub aseprite_json: bool,
    #[serde(default)]
    pub godot_spriteframes: bool,
    #[serde(default)]
    pub unity_importer: bool,
    #[serde(default)]
    pub animation_regex: Option<String>,
    #[serde(default)]
    pub stable_pivot: bool,
    #[serde(default)]
    pub dedup_threshold: Option<u8>,
    #[serde(default)]
    pub linked_channels: Vec<String>,
    #[serde(default)]
    pub pixel_snap: bool,
    #[serde(default)]
    pub pixel_snap_size: Option<f64>,
    #[serde(default = "default_pixel_snap_colors")]
    pub pixel_snap_colors: usize,
    #[serde(default)]
    pub pixel_snap_palette: String,
    #[serde(default = "default_true")]
    pub pixel_snap_lock_grid: bool,
    #[serde(default = "default_pixel_snap_engine")]
    pub pixel_snap_engine: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuiProjectOptions {
    pub input: String,
    pub output: String,
    pub format: String,
    #[serde(default = "default_packing_strategy")]
    pub packing_strategy: String,
    pub pot: bool,
    pub padding: u32,
    #[serde(default = "default_max_size")]
    pub max_size: u32,
    #[serde(default)]
    pub rotate: bool,
    pub trim: bool,
    pub aseprite_json: bool,
    #[serde(default)]
    pub godot_spriteframes: bool,
    #[serde(default)]
    pub unity_importer: bool,
    #[serde(default)]
    pub animation_regex: Option<String>,
    #[serde(default)]
    pub stable_pivot: bool,
    #[serde(default)]
    pub dedup_threshold: Option<u8>,
    #[serde(default)]
    pub linked_channels: Vec<String>,
    #[serde(default)]
    pub pixel_snap: bool,
    #[serde(default)]
    pub pixel_snap_size: Option<f64>,
    #[serde(default = "default_pixel_snap_colors")]
    pub pixel_snap_colors: usize,
    #[serde(default)]
    pub pixel_snap_palette: String,
    #[serde(default = "default_true")]
    pub pixel_snap_lock_grid: bool,
    #[serde(default = "default_pixel_snap_engine")]
    pub pixel_snap_engine: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuiSliceOptions {
    pub input: String,
    pub output: String,
    #[serde(default = "default_png_format")]
    pub format: String,
    #[serde(default = "default_grid_mode")]
    pub mode: String,
    #[serde(default)]
    pub cell_width: u32,
    #[serde(default)]
    pub cell_height: u32,
    #[serde(default = "default_frame_prefix")]
    pub prefix: String,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub aseprite_json: bool,
    #[serde(default)]
    pub normalize_generated: bool,
    #[serde(default = "default_generated_frame_size")]
    pub frame_width: u32,
    #[serde(default = "default_generated_frame_size")]
    pub frame_height: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuiGeneratedLoopOptions {
    pub input: String,
    pub output: String,
    #[serde(default = "default_png_format")]
    pub format: String,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub columns: u32,
    #[serde(default = "default_frame_prefix")]
    pub prefix: String,
    #[serde(default)]
    pub aseprite_json: bool,
    #[serde(default = "default_generated_frame_size")]
    pub frame_width: u32,
    #[serde(default = "default_generated_frame_size")]
    pub frame_height: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationJobStartOptions {
    pub output: String,
    #[serde(default = "default_cat_walk_action")]
    pub action: String,
    #[serde(default = "default_walk_frame_count")]
    pub frame_count: u32,
    #[serde(default)]
    pub columns: u32,
    #[serde(default = "default_cat_walk_prefix")]
    pub prefix: String,
    #[serde(default = "default_generated_frame_size")]
    pub frame_width: u32,
    #[serde(default = "default_generated_frame_size")]
    pub frame_height: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationJobSubmitFrameOptions {
    pub job_dir: String,
    pub frame_index: usize,
    pub source_path: String,
    #[serde(default)]
    pub force_accept: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationJobAssembleOptions {
    pub job_dir: String,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default = "default_png_format")]
    pub format: String,
    #[serde(default = "default_true")]
    pub aseprite_json: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuiGifExportOptions {
    pub atlas_path: String,
    pub sheet_path: String,
    pub output_path: String,
    pub animation: String,
    pub fps_override: f32,
    pub frame_start: Option<usize>,
    pub frame_end: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GifExportReport {
    pub output_path: String,
    pub animation: String,
    pub frame_count: usize,
    pub encoder: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AnimationJob {
    pub job_id: String,
    pub action: String,
    pub prefix: String,
    pub frame_count: usize,
    pub columns: u32,
    pub frame_size: Size,
    pub job_dir: String,
    pub frames_dir: String,
    pub candidates_dir: String,
    pub validation_dir: String,
    pub pose_guides_dir: String,
    pub assembled_dir: String,
    pub continuity_protocol: String,
    pub prompt_contract: Vec<String>,
    pub frames: Vec<AnimationFramePlan>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AnimationFramePlan {
    pub index: usize,
    pub filename: String,
    pub pose: String,
    pub phase: f32,
    pub foot_contacts: Vec<String>,
    pub body_height: String,
    pub pose_guide_path: String,
    pub prompt: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AnimationJobStartReport {
    pub job: AnimationJob,
    pub plan_path: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AnimationJobFrameAnalysis {
    pub source_size: Size,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bbox: Option<JsonRect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bbox_center: Option<Pivot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bbox_size: Option<Size>,
    pub visible_pixels: u64,
    pub green_edge_pixels: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<AnimationJobStyleStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gait: Option<AnimationJobGaitStats>,
}

#[derive(Debug, Serialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub struct AnimationJobStyleStats {
    pub mean_r: f32,
    pub mean_g: f32,
    pub mean_b: f32,
    pub mean_saturation: f32,
    pub orange_ratio: f32,
    pub cream_ratio: f32,
    pub dark_ratio: f32,
    pub silhouette_density: f32,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AnimationJobGaitStats {
    pub root_anchor: Pivot,
    pub ground_y: f32,
    pub contact_count: usize,
    pub stance_width: f32,
    pub contact_center_x: f32,
    pub rear_contact_ratio: f32,
    pub front_contact_ratio: f32,
    pub lower_body_ratio: f32,
    pub contact_points: Vec<Pivot>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AnimationJobFrameSubmitReport {
    pub job_id: String,
    pub frame_index: usize,
    pub expected_pose: String,
    pub accepted: bool,
    pub forced: bool,
    pub candidate_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved_frame_path: Option<String>,
    pub validation_path: String,
    pub analysis: AnimationJobFrameAnalysis,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub prompt: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AtlasPreviewLoad {
    pub atlas_path: String,
    pub sheet_path: String,
    pub sheet_data_url: String,
    pub atlas: Atlas,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AliasReport {
    pub frame: String,
    pub target: String,
}

#[derive(Debug, Clone)]
struct ServeConfig {
    host: String,
    port: u16,
    token: String,
}

#[derive(Debug, Default)]
struct ControlState {
    last_pack: Option<PackReport>,
    last_slice: Option<SliceReport>,
    last_atlas: Option<Atlas>,
    last_sheet_path: Option<String>,
    last_output: Option<String>,
}

type EventClients = Arc<Mutex<Vec<TcpStream>>>;

#[cfg(feature = "desktop")]
#[derive(Default)]
struct GuiControlServerState {
    server: Mutex<Option<GuiControlServer>>,
}

#[cfg(feature = "desktop")]
struct GuiControlServer {
    config: ServeConfig,
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

#[cfg(feature = "desktop")]
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GuiControlServerInfo {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub token: String,
    pub base_url: String,
    pub discovery_file: String,
}

#[derive(Debug, Clone)]
struct SourceSprite {
    name: String,
    path: PathBuf,
    image: RgbaImage,
    original_width: u32,
    original_height: u32,
    trim_x: u32,
    trim_y: u32,
    trimmed: bool,
}

#[derive(Debug, Clone)]
struct PackedSprite {
    source: SourceSprite,
    x: u32,
    y: u32,
    rotated: bool,
}

#[derive(Debug, Clone)]
struct SourceAlias {
    source: SourceSprite,
    target: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

#[derive(Debug, Clone)]
struct SliceDetection {
    rects: Vec<Rect>,
    columns: Option<u32>,
    rows: Option<u32>,
    grid_detection: String,
}

#[derive(Debug, Clone)]
struct NormalizedSlice {
    sheet: RgbaImage,
    frame_rects: Vec<Rect>,
    report: SliceNormalizationReport,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Atlas {
    pub meta: Meta,
    pub frames: Vec<Frame>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub animations: Vec<Animation>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Meta {
    pub app: String,
    pub version: String,
    pub image: String,
    pub size: Size,
    pub format: String,
    pub scale: f32,
    pub trimmed: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Frame {
    pub filename: String,
    pub frame: JsonRect,
    pub rotated: bool,
    pub trimmed: bool,
    #[serde(rename = "spriteSourceSize")]
    pub sprite_source_size: JsonRect,
    #[serde(rename = "sourceSize")]
    pub source_size: Size,
    pub pivot: Pivot,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Animation {
    pub name: String,
    pub frames: Vec<String>,
    #[serde(rename = "loop")]
    pub looped: bool,
    pub fps_hint: f32,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
pub struct Pivot {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub struct JsonRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub w: u32,
    pub h: u32,
}

pub fn run_cli(args: Vec<String>) -> Result<()> {
    if args.iter().any(|arg| arg == "--serve") {
        return serve_control_api(parse_serve_args(&args)?);
    }
    let options = parse_args(args)?;
    if options.slice_mode.is_some() {
        let report = slice_sheet(options)?;
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if options.watch {
        watch(options)?;
    } else {
        let report = pack(options)?;
        println!("{}", serde_json::to_string_pretty(&report)?);
    }
    Ok(())
}

#[cfg(feature = "desktop")]
fn clamp_window_dimension(value: u32, min: u32, monitor_max: u32) -> u32 {
    let max = monitor_max.max(1);
    value.max(min.min(max)).min(max)
}

#[cfg(feature = "desktop")]
fn initial_window_size(monitor_width: u32, monitor_height: u32) -> tauri::PhysicalSize<u32> {
    let aspect = DEFAULT_WINDOW_HEIGHT as f64 / DEFAULT_WINDOW_WIDTH as f64;
    let max_width = if monitor_width >= DEFAULT_WINDOW_WIDTH {
        DEFAULT_WINDOW_WIDTH as f64
    } else {
        (monitor_width as f64 * SMALL_MONITOR_WIDTH_RATIO)
            .round()
            .max(1.0)
    };
    let max_height = if monitor_height >= DEFAULT_WINDOW_HEIGHT {
        DEFAULT_WINDOW_HEIGHT as f64
    } else {
        (monitor_height as f64 * SMALL_MONITOR_HEIGHT_RATIO)
            .round()
            .max(1.0)
    };
    let scale = (max_width / DEFAULT_WINDOW_WIDTH as f64)
        .min(max_height / DEFAULT_WINDOW_HEIGHT as f64)
        .min(1.0);
    let width = (DEFAULT_WINDOW_WIDTH as f64 * scale).round();
    let height = (width * aspect).round();

    tauri::PhysicalSize::new(width.max(1.0) as u32, height.max(1.0) as u32)
}

#[cfg(feature = "desktop")]
fn apply_initial_window_size(app: &tauri::App) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let monitor_size = window
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| window.primary_monitor().ok().flatten())
        .map(|monitor| *monitor.size());
    let monitor_width = monitor_size
        .as_ref()
        .map(|size| size.width)
        .unwrap_or(DEFAULT_WINDOW_WIDTH);
    let monitor_height = monitor_size
        .as_ref()
        .map(|size| size.height)
        .unwrap_or(DEFAULT_WINDOW_HEIGHT);
    let size = initial_window_size(monitor_width, monitor_height);
    let size = tauri::PhysicalSize::new(
        clamp_window_dimension(size.width, MIN_WINDOW_WIDTH, monitor_width),
        clamp_window_dimension(size.height, MIN_WINDOW_HEIGHT, monitor_height),
    );
    let min_size = tauri::PhysicalSize::new(
        MIN_WINDOW_WIDTH.min(monitor_width.max(1)),
        MIN_WINDOW_HEIGHT.min(monitor_height.max(1)),
    );

    if let Err(error) = window.set_min_size(Some(tauri::Size::Physical(min_size))) {
        eprintln!("Failed to initialize minimum window size: {error}");
    }
    if let Err(error) = window.set_size(tauri::Size::Physical(size)) {
        eprintln!("Failed to initialize window size: {error}");
    }
    let _ = window.center();
}

#[cfg(feature = "desktop")]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|_app, _args, _cwd| {}))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(GuiControlServerState::default())
        .setup(|app| {
            apply_initial_window_size(app);
            let state = app.state::<GuiControlServerState>();
            if let Err(error) = start_agent_control_server(&state) {
                eprintln!(
                    "{}",
                    serde_json::json!({
                        "status": "control_error",
                        "error": error,
                    })
                );
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            pack_atlas_gui,
            slice_sheet_gui,
            assemble_generated_loop_gui,
            load_project_gui,
            save_project_gui,
            source_snapshot_gui,
            preview_image_data_url,
            inspect_gif_gui,
            load_atlas_preview_gui,
            export_gif_gui,
            read_binary_file_gui,
            write_binary_file_gui,
            agent_control_status_gui,
            start_agent_control_gui,
            stop_agent_control_gui,
            open_path
        ])
        .run(tauri::generate_context!())
        .expect("error while running Sprite Atlas Packer");
}

#[cfg(not(feature = "desktop"))]
pub fn run() {
    eprintln!(
        "{}",
        serde_json::json!({
            "status": "error",
            "error": "Desktop app support was not compiled. Use `npm run tauri:dev` or build with `--features desktop`."
        })
    );
    std::process::exit(1);
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn pack_atlas_gui(options: GuiPackOptions) -> std::result::Result<PackReport, String> {
    let pixel_snap_options = pixel_snap_options_from_gui(&options);
    let format = OutputFormat::parse(&options.format).map_err(|error| error.to_string())?;
    let packing_strategy =
        PackingStrategy::parse(&options.packing_strategy).map_err(|error| error.to_string())?;
    pack_with_pixel_snap(
        PackOptions {
            input: PathBuf::from(options.input),
            output: PathBuf::from(options.output),
            format,
            packing_strategy,
            pot: options.pot,
            padding: options.padding,
            max_size: options.max_size,
            rotate: options.rotate,
            trim: options.trim,
            dry_run: options.dry_run,
            watch: false,
            slice_mode: None,
            cell: None,
            prefix: "frame".to_string(),
            aseprite_json: options.aseprite_json,
            godot_spriteframes: options.godot_spriteframes,
            unity_importer: options.unity_importer,
            animation_regex: clean_optional_string(options.animation_regex),
            animations: Vec::new(),
            stable_pivot: options.stable_pivot,
            dedup_threshold: options.dedup_threshold,
            linked_channels: normalize_channel_suffixes(options.linked_channels),
            normalize_generated: false,
            frame_size: None,
        },
        pixel_snap_options,
    )
    .map_err(|error| error.to_string())
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn slice_sheet_gui(options: GuiSliceOptions) -> std::result::Result<SliceReport, String> {
    let mode = SliceMode::parse(&options.mode).map_err(|error| error.to_string())?;
    let cell = if mode == SliceMode::Grid && options.cell_width > 0 && options.cell_height > 0 {
        Some((options.cell_width, options.cell_height))
    } else {
        None
    };
    slice_sheet(PackOptions {
        input: PathBuf::from(options.input),
        output: PathBuf::from(options.output),
        format: OutputFormat::parse(&options.format).map_err(|error| error.to_string())?,
        packing_strategy: PackingStrategy::MaxRects,
        pot: true,
        padding: 0,
        max_size: DEFAULT_MAX_SIZE,
        rotate: false,
        trim: false,
        dry_run: options.dry_run,
        watch: false,
        slice_mode: Some(mode),
        cell,
        prefix: if options.prefix.trim().is_empty() {
            "frame".to_string()
        } else {
            options.prefix
        },
        aseprite_json: options.aseprite_json,
        godot_spriteframes: false,
        unity_importer: false,
        animation_regex: None,
        animations: Vec::new(),
        stable_pivot: false,
        dedup_threshold: None,
        linked_channels: Vec::new(),
        normalize_generated: options.normalize_generated,
        frame_size: normalized_frame_size_option(options.frame_width, options.frame_height),
    })
    .map_err(|error| error.to_string())
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn assemble_generated_loop_gui(
    options: GuiGeneratedLoopOptions,
) -> std::result::Result<SliceReport, String> {
    let options = generated_loop_options_from_gui(options).map_err(|error| error.to_string())?;
    assemble_generated_loop(options).map_err(|error| error.to_string())
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn load_project_gui(path: String) -> std::result::Result<GuiProjectOptions, String> {
    let path = PathBuf::from(path);
    let project = read_project_file(&path).map_err(|error| error.to_string())?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    Ok(project_to_gui_options(project, base))
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn save_project_gui(path: String, options: GuiProjectOptions) -> std::result::Result<(), String> {
    let pixel_snap = options.pixel_snap;
    let pixel_snap_size = options.pixel_snap_size;
    let pixel_snap_colors = options.pixel_snap_colors;
    let pixel_snap_palette = options.pixel_snap_palette.clone();
    let pixel_snap_lock_grid = options.pixel_snap_lock_grid;
    let pixel_snap_engine = clean_optional_string(Some(options.pixel_snap_engine.clone()));
    let pack_options = PackOptions {
        input: PathBuf::from(options.input),
        output: PathBuf::from(options.output),
        format: OutputFormat::parse(&options.format).map_err(|error| error.to_string())?,
        packing_strategy: PackingStrategy::parse(&options.packing_strategy)
            .map_err(|error| error.to_string())?,
        pot: options.pot,
        padding: options.padding,
        max_size: options.max_size,
        rotate: options.rotate,
        trim: options.trim,
        dry_run: false,
        watch: false,
        slice_mode: None,
        cell: None,
        prefix: "frame".to_string(),
        aseprite_json: options.aseprite_json,
        godot_spriteframes: options.godot_spriteframes,
        unity_importer: options.unity_importer,
        animation_regex: clean_optional_string(options.animation_regex),
        animations: Vec::new(),
        stable_pivot: options.stable_pivot,
        dedup_threshold: options.dedup_threshold,
        linked_channels: normalize_channel_suffixes(options.linked_channels),
        normalize_generated: false,
        frame_size: None,
    };
    let path = Path::new(&path);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
    }
    let mut project = project_from_options(&pack_options);
    project.pixel_snap = pixel_snap;
    project.pixel_snap_size = pixel_snap_size;
    project.pixel_snap_colors = Some(pixel_snap_colors);
    project.pixel_snap_palette = pixel_snap_palette;
    project.pixel_snap_lock_grid = pixel_snap_lock_grid;
    project.pixel_snap_engine = pixel_snap_engine;
    let json = serde_json::to_string_pretty(&project).map_err(|error| error.to_string())?;
    write_text_if_changed(path, &format!("{json}\n")).map_err(|error| error.to_string())
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn source_snapshot_gui(input: String) -> std::result::Result<String, String> {
    source_snapshot(Path::new(&input)).map_err(|error| error.to_string())
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn preview_image_data_url(path: String) -> std::result::Result<String, String> {
    image_data_url(Path::new(&path)).map_err(|error| error.to_string())
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn inspect_gif_gui(path: String) -> std::result::Result<AtlasPreviewLoad, String> {
    inspect_gif_preview(Path::new(&path)).map_err(|error| error.to_string())
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn load_atlas_preview_gui(path: String) -> std::result::Result<AtlasPreviewLoad, String> {
    let atlas_path = PathBuf::from(path);
    let atlas: Atlas =
        serde_json::from_slice(&fs::read(&atlas_path).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    let sheet_path = atlas_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(&atlas.meta.image);
    let sheet_data_url = image_data_url(&sheet_path).map_err(|error| error.to_string())?;
    Ok(AtlasPreviewLoad {
        atlas_path: atlas_path.to_string_lossy().to_string(),
        sheet_path: sheet_path.to_string_lossy().to_string(),
        sheet_data_url,
        atlas,
    })
}

#[cfg(feature = "desktop")]
fn image_data_url(path: &Path) -> Result<String> {
    let bytes = fs::read(path)?;
    let mime = match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "webp" => "image/webp",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        _ => "application/octet-stream",
    };
    Ok(format!(
        "data:{mime};base64,{}",
        general_purpose::STANDARD.encode(bytes)
    ))
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn export_gif_gui(options: GuiGifExportOptions) -> std::result::Result<GifExportReport, String> {
    export_gif(
        Path::new(&options.atlas_path),
        Path::new(&options.sheet_path),
        Path::new(&options.output_path),
        &options.animation,
        options.fps_override,
        options.frame_start,
        options.frame_end,
    )
    .map_err(|error| error.to_string())
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn read_binary_file_gui(path: String) -> std::result::Result<Vec<u8>, String> {
    fs::read(Path::new(&path)).map_err(|error| error.to_string())
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn write_binary_file_gui(path: String, bytes: Vec<u8>) -> std::result::Result<(), String> {
    let path = Path::new(&path);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
    }
    fs::write(path, bytes).map_err(|error| error.to_string())
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn open_path(app: tauri::AppHandle, path: String) -> std::result::Result<(), String> {
    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|error| error.to_string())
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn agent_control_status_gui(
    state: tauri::State<'_, GuiControlServerState>,
) -> std::result::Result<GuiControlServerInfo, String> {
    let server = state.server.lock().map_err(|error| error.to_string())?;
    Ok(gui_control_server_info(server.as_ref()))
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn start_agent_control_gui(
    state: tauri::State<'_, GuiControlServerState>,
) -> std::result::Result<GuiControlServerInfo, String> {
    start_agent_control_server(&state)
}

#[cfg(feature = "desktop")]
fn start_agent_control_server(
    state: &GuiControlServerState,
) -> std::result::Result<GuiControlServerInfo, String> {
    let mut server = state.server.lock().map_err(|error| error.to_string())?;
    if server.is_some() {
        return Ok(gui_control_server_info(server.as_ref()));
    }

    let config = ServeConfig {
        host: "127.0.0.1".to_string(),
        port: 9877,
        token: generate_control_token(),
    };
    let listener = bind_control_listener(&config).map_err(|error| error.to_string())?;
    write_control_discovery(&config).map_err(|error| error.to_string())?;
    let shutdown = Arc::new(AtomicBool::new(false));
    let thread_shutdown = shutdown.clone();
    let thread_config = config.clone();
    let handle = thread::spawn(move || {
        if let Err(error) = run_control_listener(thread_config, listener, Some(thread_shutdown)) {
            eprintln!(
                "{}",
                serde_json::json!({
                    "status": "control_error",
                    "error": error.to_string(),
                })
            );
        }
    });
    *server = Some(GuiControlServer {
        config,
        shutdown,
        handle: Some(handle),
    });
    Ok(gui_control_server_info(server.as_ref()))
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn stop_agent_control_gui(
    state: tauri::State<'_, GuiControlServerState>,
) -> std::result::Result<GuiControlServerInfo, String> {
    let mut server = state.server.lock().map_err(|error| error.to_string())?;
    if let Some(mut active) = server.take() {
        active.shutdown.store(true, AtomicOrdering::SeqCst);
        let _ = TcpStream::connect(format!("{}:{}", active.config.host, active.config.port));
        if let Some(handle) = active.handle.take() {
            handle
                .join()
                .map_err(|_| "Remote control server thread panicked while stopping".to_string())?;
        }
    }
    Ok(gui_control_server_info(None))
}

pub fn pack(options: PackOptions) -> Result<PackReport> {
    pack_with_pixel_snap(options, PixelSnapOptions::default())
}

fn pack_with_pixel_snap(
    options: PackOptions,
    pixel_snap_options: PixelSnapOptions,
) -> Result<PackReport> {
    validate_options(&options)?;
    let (sources, pixel_grids) = if pixel_snap_options.enabled {
        let sources = load_sources_with_channels(&options.input, false, &options.linked_channels)?;
        pixel_snap_sources(sources, options.trim, &pixel_snap_options)?
    } else {
        (
            load_sources_with_channels(&options.input, options.trim, &options.linked_channels)?,
            BTreeMap::new(),
        )
    };
    validate_animation_options(&options, &sources)?;
    if sources.is_empty() {
        return Err(AtlasError::Message(format!(
            "No supported images found under {}",
            options.input.display()
        )));
    }

    let (packed_sources, aliases) = deduplicate_sources(
        &sources,
        options.dedup_threshold,
        options.trim,
        &options.linked_channels,
        &pixel_grids,
    )?;
    let alias_reports = aliases
        .iter()
        .map(|alias| AliasReport {
            frame: alias.source.name.clone(),
            target: alias.target.clone(),
        })
        .collect::<Vec<_>>();
    let layout = pack_sprites(
        &packed_sources,
        options.padding,
        options.pot,
        options.packing_strategy,
        options.max_size,
        options.rotate,
    )?;
    let atlas = build_atlas(
        &layout.sprites,
        &aliases,
        &options,
        layout.width,
        layout.height,
    );
    let used_pixels = layout
        .sprites
        .iter()
        .map(|sprite| sprite.source.image.width() as u64 * sprite.source.image.height() as u64)
        .sum::<u64>();
    let total_pixels = layout.width as u64 * layout.height as u64;
    let waste_percent = if total_pixels == 0 {
        0.0
    } else {
        100.0 - (used_pixels as f64 / total_pixels as f64 * 100.0)
    };

    let mut wrote = Vec::new();
    if !options.dry_run {
        fs::create_dir_all(&options.output)?;
        let sheet = render_sheet(&layout.sprites, layout.width, layout.height);
        match options.format {
            OutputFormat::Webp => {
                write_webp(&options.output.join("sheet.webp"), &sheet)?;
                wrote.push("sheet.webp".to_string());
            }
            OutputFormat::Png => {
                write_png(&options.output.join("sheet.png"), &sheet)?;
                wrote.push("sheet.png".to_string());
            }
            OutputFormat::Both => {
                write_webp(&options.output.join("sheet.webp"), &sheet)?;
                write_png(&options.output.join("sheet.png"), &sheet)?;
                wrote.push("sheet.webp".to_string());
                wrote.push("sheet.png".to_string());
            }
        }
        let atlas_json = serde_json::to_string_pretty(&atlas)?;
        write_text_if_changed(
            &options.output.join("atlas.json"),
            &format!("{atlas_json}\n"),
        )?;
        wrote.push("atlas.json".to_string());
        if options.aseprite_json {
            write_aseprite_json(&options.output.join("aseprite.json"), &atlas)?;
            wrote.push("aseprite.json".to_string());
        }
        if options.godot_spriteframes {
            write_godot_spriteframes(&options.output.join("godot_spriteframes.tres"), &atlas)?;
            wrote.push("godot_spriteframes.tres".to_string());
        }
        if options.unity_importer {
            write_unity_importer_script(
                &options.output.join("SpriteAtlasPackerImporter.cs"),
                &atlas,
            )?;
            wrote.push("SpriteAtlasPackerImporter.cs".to_string());
        }
        for suffix in &options.linked_channels {
            let channel_sheet = render_linked_channel_sheet(
                &layout.sprites,
                layout.width,
                layout.height,
                suffix,
                options.trim,
                &pixel_grids,
            )?;
            for name in
                write_channel_sheet(&options.output, options.format, suffix, &channel_sheet)?
            {
                wrote.push(name);
            }
        }
    }

    let frames = atlas
        .frames
        .iter()
        .map(|frame| frame.filename.clone())
        .collect();
    let animations = atlas
        .animations
        .iter()
        .map(|animation| animation.name.clone())
        .collect();

    Ok(PackReport {
        input_count: sources.len(),
        atlas_width: layout.width,
        atlas_height: layout.height,
        packing_strategy: options.packing_strategy.as_str().to_string(),
        max_size: options.max_size,
        used_pixels,
        waste_percent,
        frames,
        animations,
        wrote,
        aliases: alias_reports,
        atlas,
    })
}

pub fn slice_sheet(options: PackOptions) -> Result<SliceReport> {
    let mode = options
        .slice_mode
        .ok_or_else(|| AtlasError::Message("--slice mode is required".to_string()))?;
    if options.input.as_os_str().is_empty() {
        return Err(AtlasError::Message("--input sheet is required".to_string()));
    }
    if options.output.as_os_str().is_empty() && !options.dry_run {
        return Err(AtlasError::Message("--output is required".to_string()));
    }
    let source_image = decode_image(&options.input)?;
    let detection_image = if options.normalize_generated {
        remove_generation_chroma_key(&source_image)
    } else {
        source_image.clone()
    };
    let detection = detect_slice_rects(
        &detection_image,
        mode,
        options.cell,
        options.normalize_generated,
    )?;
    if detection.rects.is_empty() {
        return Err(AtlasError::Message(
            "No frames detected in source sheet".to_string(),
        ));
    }

    let sheet_name = options.format.atlas_image_name().to_string();
    let (sheet_image, atlas, normalization) = if options.normalize_generated {
        let normalized = normalize_generated_slice(&detection_image, &detection, &options)?;
        let atlas = slice_atlas(
            &sheet_name,
            &options.prefix,
            normalized.sheet.width(),
            normalized.sheet.height(),
            &normalized.frame_rects,
            Pivot { x: 0.5, y: 0.305 },
        );
        (normalized.sheet, atlas, Some(normalized.report))
    } else {
        let atlas = slice_atlas(
            &sheet_name,
            &options.prefix,
            source_image.width(),
            source_image.height(),
            &detection.rects,
            Pivot { x: 0.5, y: 0.5 },
        );
        (source_image, atlas, None)
    };

    let mut wrote = Vec::new();
    if !options.dry_run {
        fs::create_dir_all(options.output.join("frames"))?;
        match options.format {
            OutputFormat::Webp => {
                write_webp(&options.output.join("sheet.webp"), &sheet_image)?;
                wrote.push("sheet.webp".to_string());
            }
            OutputFormat::Png => {
                write_png(&options.output.join("sheet.png"), &sheet_image)?;
                wrote.push("sheet.png".to_string());
            }
            OutputFormat::Both => {
                write_webp(&options.output.join("sheet.webp"), &sheet_image)?;
                write_png(&options.output.join("sheet.png"), &sheet_image)?;
                wrote.push("sheet.webp".to_string());
                wrote.push("sheet.png".to_string());
            }
        }
        for frame in &atlas.frames {
            let cropped = sheet_image
                .view(frame.frame.x, frame.frame.y, frame.frame.w, frame.frame.h)
                .to_image();
            write_png(
                &options
                    .output
                    .join("frames")
                    .join(format!("{}.png", frame.filename)),
                &cropped,
            )?;
        }
        wrote.push("frames/*.png".to_string());
        let atlas_json = serde_json::to_string_pretty(&atlas)?;
        write_text_if_changed(
            &options.output.join("atlas.json"),
            &format!("{atlas_json}\n"),
        )?;
        wrote.push("atlas.json".to_string());
        if options.aseprite_json {
            write_aseprite_json(&options.output.join("aseprite.json"), &atlas)?;
            wrote.push("aseprite.json".to_string());
        }
        if let Some(normalization) = &normalization {
            let json = serde_json::to_string_pretty(normalization)?;
            write_text_if_changed(
                &options.output.join("spritesheet_normalization.json"),
                &format!("{json}\n"),
            )?;
            wrote.push("spritesheet_normalization.json".to_string());
        }
    }

    Ok(SliceReport {
        frame_count: atlas.frames.len(),
        sheet_width: sheet_image.width(),
        sheet_height: sheet_image.height(),
        frames: atlas
            .frames
            .iter()
            .map(|frame| frame.filename.clone())
            .collect(),
        wrote,
        atlas,
        normalization,
    })
}

pub fn assemble_generated_loop(options: GeneratedLoopOptions) -> Result<SliceReport> {
    if options.input.as_os_str().is_empty() {
        return Err(AtlasError::Message(
            "Generated loop input folder is required".to_string(),
        ));
    }
    if options.output.as_os_str().is_empty() && !options.dry_run {
        return Err(AtlasError::Message("--output is required".to_string()));
    }
    let sources = load_sources(&options.input, false)?;
    if sources.is_empty() {
        return Err(AtlasError::Message(
            "No generated frame images found in input folder".to_string(),
        ));
    }

    let columns = generated_loop_columns(sources.len(), options.columns)?;
    let rows = ceil_div_u32(sources.len() as u32, columns).max(1);
    let cell_w = sources
        .iter()
        .map(|source| source.image.width())
        .max()
        .unwrap_or(1)
        .max(1);
    let cell_h = sources
        .iter()
        .map(|source| source.image.height())
        .max()
        .unwrap_or(1)
        .max(1);
    let sheet_w = cell_w.checked_mul(columns).ok_or_else(|| {
        AtlasError::Message("Generated loop source sheet width would overflow".to_string())
    })?;
    let sheet_h = cell_h.checked_mul(rows).ok_or_else(|| {
        AtlasError::Message("Generated loop source sheet height would overflow".to_string())
    })?;
    let mut source_sheet = RgbaImage::from_pixel(sheet_w, sheet_h, Rgba([0, 0, 0, 0]));
    let mut rects = Vec::new();
    for (index, source) in sources.iter().enumerate() {
        let column = index as u32 % columns;
        let row = index as u32 / columns;
        let x = column * cell_w;
        let y = row * cell_h;
        let cleaned = remove_generation_chroma_key(&source.image);
        let offset_x = x + (cell_w - cleaned.width()) / 2;
        let offset_y = y + (cell_h - cleaned.height()) / 2;
        overlay(
            &mut source_sheet,
            &cleaned,
            offset_x as i64,
            offset_y as i64,
        );
        rects.push(Rect {
            x,
            y,
            w: cell_w,
            h: cell_h,
        });
    }

    let detection = SliceDetection {
        rects,
        columns: Some(columns),
        rows: Some(rows),
        grid_detection: format!(
            "individual_generated_frames_sorted:{}_frames:{}x{}",
            sources.len(),
            columns,
            rows
        ),
    };
    let normalizer_options = PackOptions {
        input: options.input.clone(),
        output: options.output.clone(),
        format: options.format,
        packing_strategy: PackingStrategy::MaxRects,
        pot: true,
        padding: 0,
        max_size: DEFAULT_MAX_SIZE,
        rotate: false,
        trim: false,
        dry_run: options.dry_run,
        watch: false,
        slice_mode: Some(SliceMode::Grid),
        cell: Some((cell_w, cell_h)),
        prefix: options.prefix.clone(),
        aseprite_json: options.aseprite_json,
        godot_spriteframes: false,
        unity_importer: false,
        animation_regex: None,
        animations: Vec::new(),
        stable_pivot: false,
        dedup_threshold: None,
        linked_channels: Vec::new(),
        normalize_generated: true,
        frame_size: options.frame_size,
    };
    let normalized = normalize_generated_slice(&source_sheet, &detection, &normalizer_options)?;
    let sheet_name = options.format.atlas_image_name().to_string();
    let atlas = slice_atlas(
        &sheet_name,
        &options.prefix,
        normalized.sheet.width(),
        normalized.sheet.height(),
        &normalized.frame_rects,
        Pivot { x: 0.5, y: 0.305 },
    );
    let sheet_image = normalized.sheet;
    let normalization = Some(normalized.report);

    let mut wrote = Vec::new();
    if !options.dry_run {
        fs::create_dir_all(options.output.join("frames"))?;
        match options.format {
            OutputFormat::Webp => {
                write_webp(&options.output.join("sheet.webp"), &sheet_image)?;
                wrote.push("sheet.webp".to_string());
            }
            OutputFormat::Png => {
                write_png(&options.output.join("sheet.png"), &sheet_image)?;
                wrote.push("sheet.png".to_string());
            }
            OutputFormat::Both => {
                write_webp(&options.output.join("sheet.webp"), &sheet_image)?;
                write_png(&options.output.join("sheet.png"), &sheet_image)?;
                wrote.push("sheet.webp".to_string());
                wrote.push("sheet.png".to_string());
            }
        }
        for frame in &atlas.frames {
            let cropped = sheet_image
                .view(frame.frame.x, frame.frame.y, frame.frame.w, frame.frame.h)
                .to_image();
            write_png(
                &options
                    .output
                    .join("frames")
                    .join(format!("{}.png", frame.filename)),
                &cropped,
            )?;
        }
        wrote.push("frames/*.png".to_string());
        let atlas_json = serde_json::to_string_pretty(&atlas)?;
        write_text_if_changed(
            &options.output.join("atlas.json"),
            &format!("{atlas_json}\n"),
        )?;
        wrote.push("atlas.json".to_string());
        if options.aseprite_json {
            write_aseprite_json(&options.output.join("aseprite.json"), &atlas)?;
            wrote.push("aseprite.json".to_string());
        }
        if let Some(normalization) = &normalization {
            let json = serde_json::to_string_pretty(normalization)?;
            write_text_if_changed(
                &options.output.join("spritesheet_normalization.json"),
                &format!("{json}\n"),
            )?;
            wrote.push("spritesheet_normalization.json".to_string());
        }
    }

    Ok(SliceReport {
        frame_count: atlas.frames.len(),
        sheet_width: sheet_image.width(),
        sheet_height: sheet_image.height(),
        frames: atlas
            .frames
            .iter()
            .map(|frame| frame.filename.clone())
            .collect(),
        wrote,
        atlas,
        normalization,
    })
}

pub fn start_animation_job(options: AnimationJobStartOptions) -> Result<AnimationJobStartReport> {
    let action = if options.action.trim().is_empty() {
        default_cat_walk_action()
    } else {
        options.action.trim().to_ascii_lowercase()
    };
    if action != "cat_walk" {
        return Err(AtlasError::Message(format!(
            "Unsupported animation job action: {action}. Supported action: cat_walk"
        )));
    }
    let frame_count = options.frame_count as usize;
    if !matches!(frame_count, 8 | 16) {
        return Err(AtlasError::Message(
            "Cat walk jobs currently support 8 or 16 frames".to_string(),
        ));
    }
    let frame_size = Size {
        w: options.frame_width.max(16),
        h: options.frame_height.max(16),
    };
    let columns = generated_loop_columns(
        frame_count,
        (options.columns > 0).then_some(options.columns),
    )?;
    let job_dir = PathBuf::from(options.output);
    if job_dir.as_os_str().is_empty() {
        return Err(AtlasError::Message(
            "Animation job output folder is required".to_string(),
        ));
    }
    let frames_dir = job_dir.join("accepted_frames");
    let candidates_dir = job_dir.join("candidates");
    let validation_dir = job_dir.join("validation");
    let pose_guides_dir = job_dir.join("pose_guides");
    let assembled_dir = job_dir.join("assembled");
    fs::create_dir_all(&frames_dir)?;
    fs::create_dir_all(&candidates_dir)?;
    fs::create_dir_all(&validation_dir)?;
    fs::create_dir_all(&pose_guides_dir)?;
    fs::create_dir_all(&assembled_dir)?;

    let job_id = generate_animation_job_id();
    let prefix = if options.prefix.trim().is_empty() {
        default_cat_walk_prefix()
    } else {
        options.prefix
    };
    let frames = cat_walk_frame_plan(frame_count, frame_size, &pose_guides_dir)?;
    let job = AnimationJob {
        job_id,
        action,
        prefix,
        frame_count,
        columns,
        frame_size,
        job_dir: job_dir.to_string_lossy().to_string(),
        frames_dir: frames_dir.to_string_lossy().to_string(),
        candidates_dir: candidates_dir.to_string_lossy().to_string(),
        validation_dir: validation_dir.to_string_lossy().to_string(),
        pose_guides_dir: pose_guides_dir.to_string_lossy().to_string(),
        assembled_dir: assembled_dir.to_string_lossy().to_string(),
        continuity_protocol:
            "cat_walk_frame_at_a_time_pose_guides_validate_regenerate_then_assemble".to_string(),
        prompt_contract: vec![
            "Use the locked cat identity reference for every frame".to_string(),
            "Follow the exact pose guide for the requested frame".to_string(),
            "Keep same side-view camera, scale, body length, head size, tail style, palette, and full-body framing".to_string(),
            "Use transparent background, or a thin dark neutral gray matte/separator only".to_string(),
            "Make planted front/hind paws visibly contact one stable invisible ground line".to_string(),
            "Keep the upper-body/root anchor locked; only legs, paws, tail, and subtle body height should change".to_string(),
            "Do not duplicate frame 1 as the final frame; frame 16 must be the in-between before frame 1".to_string(),
        ],
        frames,
    };
    let plan_path = animation_job_plan_path(&job_dir);
    write_animation_job(&plan_path, &job)?;
    Ok(AnimationJobStartReport {
        job,
        plan_path: plan_path.to_string_lossy().to_string(),
    })
}

pub fn submit_animation_job_frame(
    options: AnimationJobSubmitFrameOptions,
) -> Result<AnimationJobFrameSubmitReport> {
    let job_dir = PathBuf::from(options.job_dir);
    let plan_path = animation_job_plan_path(&job_dir);
    let job = read_animation_job(&plan_path)?;
    if options.frame_index >= job.frame_count {
        return Err(AtlasError::Message(format!(
            "Frame index {} is outside this {} frame job",
            options.frame_index, job.frame_count
        )));
    }
    let source_path = PathBuf::from(&options.source_path);
    if !source_path.exists() {
        return Err(AtlasError::Message(format!(
            "Generated frame does not exist: {}",
            source_path.display()
        )));
    }
    let frame_plan = job.frames[options.frame_index].clone();
    let candidates_dir = PathBuf::from(&job.candidates_dir);
    let validation_dir = PathBuf::from(&job.validation_dir);
    let frames_dir = PathBuf::from(&job.frames_dir);
    fs::create_dir_all(&candidates_dir)?;
    fs::create_dir_all(&validation_dir)?;
    fs::create_dir_all(&frames_dir)?;

    let extension = source_path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("png");
    let candidate_path = candidates_dir.join(format!(
        "{:03}_candidate.{}",
        options.frame_index,
        extension.to_ascii_lowercase()
    ));
    fs::copy(&source_path, &candidate_path)?;
    let analysis = analyze_animation_frame(&candidate_path)?;
    let accepted_analyses = accepted_frame_analyses(&job, Some(options.frame_index))?;
    let (mut errors, warnings) =
        validate_animation_frame(&job, &frame_plan, &analysis, &accepted_analyses);
    let forced = options.force_accept && !errors.is_empty();
    let accepted = errors.is_empty() || options.force_accept;
    let saved_frame_path = if accepted {
        let destination = frames_dir.join(&frame_plan.filename);
        fs::copy(&candidate_path, &destination)?;
        Some(destination.to_string_lossy().to_string())
    } else {
        None
    };
    if forced {
        errors.push("Frame was force-accepted despite validation errors".to_string());
    }

    let validation_path = validation_dir.join(format!("{:03}.json", options.frame_index));
    let report = AnimationJobFrameSubmitReport {
        job_id: job.job_id,
        frame_index: options.frame_index,
        expected_pose: frame_plan.pose,
        accepted,
        forced,
        candidate_path: candidate_path.to_string_lossy().to_string(),
        saved_frame_path,
        validation_path: validation_path.to_string_lossy().to_string(),
        analysis,
        errors,
        warnings,
        prompt: frame_plan.prompt,
    };
    write_text_if_changed(
        &validation_path,
        &format!("{}\n", serde_json::to_string_pretty(&report)?),
    )?;
    Ok(report)
}

pub fn assemble_animation_job(options: AnimationJobAssembleOptions) -> Result<SliceReport> {
    let job_dir = PathBuf::from(options.job_dir);
    let job = read_animation_job(&animation_job_plan_path(&job_dir))?;
    let frames_dir = PathBuf::from(&job.frames_dir);
    let missing = job
        .frames
        .iter()
        .filter(|frame| !frames_dir.join(&frame.filename).exists())
        .map(|frame| format!("{}:{}", frame.index, frame.filename))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(AtlasError::Message(format!(
            "Cannot assemble yet; missing accepted frames: {}",
            missing.join(", ")
        )));
    }
    let output = options
        .output
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&job.assembled_dir));
    assemble_generated_loop(GeneratedLoopOptions {
        input: frames_dir,
        output,
        format: OutputFormat::parse(&options.format)?,
        dry_run: false,
        columns: Some(job.columns),
        prefix: job.prefix,
        aseprite_json: options.aseprite_json,
        frame_size: Some((job.frame_size.w, job.frame_size.h)),
    })
}

pub fn export_gif(
    atlas_path: &Path,
    sheet_path: &Path,
    output_path: &Path,
    animation_name: &str,
    fps_override: f32,
    frame_start: Option<usize>,
    frame_end: Option<usize>,
) -> Result<GifExportReport> {
    let atlas: Atlas = serde_json::from_slice(&fs::read(atlas_path)?)?;
    let sheet = decode_image(sheet_path)?;
    let frame_names = animation_frame_names(&atlas, animation_name)?;
    let frames = selected_animation_frame_names(&frame_names, frame_start, frame_end)
        .iter()
        .map(|name| {
            atlas
                .frames
                .iter()
                .find(|frame| &frame.filename == name)
                .ok_or_else(|| {
                    AtlasError::Message(format!("Animation references missing frame: {name}"))
                })
        })
        .collect::<Result<Vec<_>>>()?;
    if frames.is_empty() {
        return Err(AtlasError::Message(
            "Animation has no exportable frames".to_string(),
        ));
    }

    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let rendered = render_animation_frames(&sheet, &frames);
    let delay_ms = if fps_override > 0.0 {
        (1000.0 / fps_override).round().max(1.0) as u32
    } else {
        0
    };

    if gifski_available() {
        if let Ok(report) = export_gif_with_gifski(
            output_path,
            animation_name,
            &rendered,
            delay_ms,
            frames[0].duration_ms.unwrap_or(DEFAULT_DURATION_MS),
        ) {
            return Ok(report);
        }
    }

    export_gif_with_image_encoder(output_path, animation_name, &rendered, delay_ms, &frames)
}

fn selected_animation_frame_names(
    frame_names: &[String],
    frame_start: Option<usize>,
    frame_end: Option<usize>,
) -> Vec<String> {
    if frame_names.is_empty() {
        return Vec::new();
    }
    let max_index = frame_names.len() - 1;
    let start = frame_start.unwrap_or(0).min(max_index);
    let end = frame_end.unwrap_or(max_index).min(max_index);
    let (start, end) = if start <= end {
        (start, end)
    } else {
        (end, start)
    };
    frame_names[start..=end].to_vec()
}

fn animation_frame_names(atlas: &Atlas, animation_name: &str) -> Result<Vec<String>> {
    if !animation_name.trim().is_empty() {
        if let Some(animation) = atlas
            .animations
            .iter()
            .find(|animation| animation.name == animation_name)
        {
            return Ok(animation.frames.clone());
        }
        if atlas
            .frames
            .iter()
            .any(|frame| frame.filename == animation_name)
        {
            return Ok(vec![animation_name.to_string()]);
        }
        return Err(AtlasError::Message(format!(
            "Animation not found: {animation_name}"
        )));
    }
    if let Some(animation) = atlas.animations.first() {
        return Ok(animation.frames.clone());
    }
    Ok(atlas
        .frames
        .iter()
        .map(|frame| frame.filename.clone())
        .collect())
}

fn render_animation_frames(sheet: &RgbaImage, frames: &[&Frame]) -> Vec<RgbaImage> {
    let canvas_width = frames
        .iter()
        .map(|frame| frame.source_size.w)
        .max()
        .unwrap_or(1);
    let canvas_height = frames
        .iter()
        .map(|frame| frame.source_size.h)
        .max()
        .unwrap_or(1);
    frames
        .iter()
        .map(|frame| {
            let mut image = RgbaImage::from_pixel(canvas_width, canvas_height, Rgba([0, 0, 0, 0]));
            let cropped = extract_frame_image(sheet, frame);
            image::imageops::overlay(
                &mut image,
                &cropped,
                frame.sprite_source_size.x.into(),
                frame.sprite_source_size.y.into(),
            );
            image
        })
        .collect()
}

fn extract_frame_image(sheet: &RgbaImage, frame: &Frame) -> RgbaImage {
    let cropped = sheet
        .view(frame.frame.x, frame.frame.y, frame.frame.w, frame.frame.h)
        .to_image();
    if frame.rotated {
        image::imageops::rotate270(&cropped)
    } else {
        cropped
    }
}

fn gifski_available() -> bool {
    std::process::Command::new("gifski")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn export_gif_with_gifski(
    output_path: &Path,
    animation_name: &str,
    frames: &[RgbaImage],
    delay_ms: u32,
    fallback_delay_ms: u32,
) -> Result<GifExportReport> {
    let temp = std::env::temp_dir().join(format!(
        "sprite-atlas-packer-gif-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&temp)?;
    let mut frame_paths = Vec::new();
    for (index, frame) in frames.iter().enumerate() {
        let path = temp.join(format!("frame_{index:04}.png"));
        write_png(&path, frame)?;
        frame_paths.push(path);
    }
    let selected_delay = if delay_ms > 0 {
        delay_ms
    } else {
        fallback_delay_ms
    };
    let fps = 1000.0 / selected_delay.max(1) as f32;
    let mut command = std::process::Command::new("gifski");
    command
        .arg("--quiet")
        .arg("--fps")
        .arg(format!("{fps:.3}"))
        .arg("--output")
        .arg(output_path);
    for path in &frame_paths {
        command.arg(path);
    }
    let output = command.output()?;
    let _ = fs::remove_dir_all(&temp);
    if !output.status.success() {
        return Err(AtlasError::Message(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }
    Ok(GifExportReport {
        output_path: output_path.to_string_lossy().to_string(),
        animation: animation_name.to_string(),
        frame_count: frames.len(),
        encoder: "gifski".to_string(),
    })
}

fn export_gif_with_image_encoder(
    output_path: &Path,
    animation_name: &str,
    rendered: &[RgbaImage],
    delay_ms: u32,
    atlas_frames: &[&Frame],
) -> Result<GifExportReport> {
    let file = fs::File::create(output_path)?;
    let mut encoder = GifEncoder::new(file);
    encoder.set_repeat(Repeat::Infinite)?;
    let frames = rendered
        .iter()
        .zip(atlas_frames.iter())
        .map(|(image, frame)| {
            let duration = if delay_ms > 0 {
                delay_ms
            } else {
                frame.duration_ms.unwrap_or(DEFAULT_DURATION_MS)
            }
            .max(1);
            image::Frame::from_parts(image.clone(), 0, 0, Delay::from_numer_denom_ms(duration, 1))
        })
        .collect::<Vec<_>>();
    encoder.encode_frames(frames.into_iter())?;
    Ok(GifExportReport {
        output_path: output_path.to_string_lossy().to_string(),
        animation: animation_name.to_string(),
        frame_count: rendered.len(),
        encoder: "image-gif".to_string(),
    })
}

fn write_aseprite_json(path: &Path, atlas: &Atlas) -> Result<()> {
    let frame_index = atlas
        .frames
        .iter()
        .enumerate()
        .map(|(index, frame)| (frame.filename.clone(), index))
        .collect::<BTreeMap<_, _>>();
    let frame_tags = atlas
        .animations
        .iter()
        .filter_map(|animation| {
            let indexes = animation
                .frames
                .iter()
                .filter_map(|name| frame_index.get(name).copied())
                .collect::<Vec<_>>();
            if indexes.is_empty() {
                return None;
            }
            let from = indexes.iter().min().copied().unwrap_or(0);
            let to = indexes.iter().max().copied().unwrap_or(from);
            Some(serde_json::json!({
                "name": animation.name,
                "from": from,
                "to": to,
                "direction": "forward"
            }))
        })
        .collect::<Vec<_>>();
    let frames = atlas
        .frames
        .iter()
        .map(|frame| {
            serde_json::json!({
                "filename": frame.filename,
                "frame": frame.frame,
                "rotated": frame.rotated,
                "trimmed": frame.trimmed,
                "spriteSourceSize": frame.sprite_source_size,
                "sourceSize": frame.source_size,
                "duration": frame.duration_ms.unwrap_or(DEFAULT_DURATION_MS)
            })
        })
        .collect::<Vec<_>>();
    let value = serde_json::json!({
        "frames": frames,
        "meta": {
            "app": APP_NAME,
            "version": VERSION,
            "image": atlas.meta.image,
            "format": atlas.meta.format,
            "size": atlas.meta.size,
            "scale": atlas.meta.scale,
            "frameTags": frame_tags
        }
    });
    let json = serde_json::to_string_pretty(&value)?;
    write_text_if_changed(path, &format!("{json}\n"))?;
    Ok(())
}

fn write_godot_spriteframes(path: &Path, atlas: &Atlas) -> Result<()> {
    reject_rotated_native_export(atlas, "Godot SpriteFrames")?;
    let animations = atlas_animation_sequences(atlas);
    let mut text = String::new();
    text.push_str(&format!(
        "[gd_resource type=\"SpriteFrames\" load_steps={} format=3]\n\n",
        atlas.frames.len() + 2
    ));
    text.push_str(&format!(
        "[ext_resource type=\"Texture2D\" path=\"res://{}\" id=\"1_sheet\"]\n\n",
        escape_godot_string(&atlas.meta.image)
    ));
    for frame in &atlas.frames {
        text.push_str(&format!(
            "[sub_resource type=\"AtlasTexture\" id=\"AtlasTexture_{}\"]\natlas = ExtResource(\"1_sheet\")\nregion = Rect2({}, {}, {}, {})\n\n",
            sanitize_resource_id(&frame.filename),
            frame.frame.x,
            frame.frame.y,
            frame.frame.w,
            frame.frame.h
        ));
    }
    text.push_str("[resource]\nanimations = [");
    for (animation_index, animation) in animations.iter().enumerate() {
        if animation_index > 0 {
            text.push_str(", ");
        }
        text.push_str("{\n\"frames\": [");
        for (frame_index, frame_name) in animation.frames.iter().enumerate() {
            if frame_index > 0 {
                text.push_str(", ");
            }
            text.push_str(&format!(
                "{{\n\"duration\": 1.0,\n\"texture\": SubResource(\"AtlasTexture_{}\")\n}}",
                sanitize_resource_id(frame_name)
            ));
        }
        text.push_str(&format!(
            "],\n\"loop\": {},\n\"name\": &\"{}\",\n\"speed\": {}\n}}",
            animation.looped,
            escape_godot_string(&animation.name),
            format_float(animation.fps_hint)
        ));
    }
    text.push_str("]\n");
    write_text_if_changed(path, &text)?;
    Ok(())
}

fn write_unity_importer_script(path: &Path, atlas: &Atlas) -> Result<()> {
    reject_rotated_native_export(atlas, "Unity importer")?;
    let text = format!(
        r#"#if UNITY_EDITOR
// Generated by Sprite Atlas Packer for {image} ({frame_count} frames).
// Put this file in an Editor folder, select the atlas texture, then run
// Tools > Sprite Atlas Packer > Import Selected TexturePacker JSON.
using System.IO;
using UnityEditor;
using UnityEngine;

public static class SpriteAtlasPackerImporter
{{
    [System.Serializable]
    private struct Atlas
    {{
        public Frame[] frames;
    }}

    [System.Serializable]
    private struct Frame
    {{
        public string filename;
        public RectData frame;
        public RectData spriteSourceSize;
        public SizeData sourceSize;
        public PivotData pivot;
    }}

    [System.Serializable]
    private struct RectData {{ public int x, y, w, h; }}
    [System.Serializable]
    private struct SizeData {{ public int w, h; }}
    [System.Serializable]
    private struct PivotData {{ public float x, y; }}

    [MenuItem("Tools/Sprite Atlas Packer/Import Selected TexturePacker JSON")]
    public static void ImportSelectedAtlas()
    {{
        var texture = Selection.activeObject as Texture2D;
        if (texture == null)
        {{
            Debug.LogError("Select the atlas texture before importing.");
            return;
        }}

        var texturePath = AssetDatabase.GetAssetPath(texture);
        var atlasPath = Path.Combine(Path.GetDirectoryName(texturePath) ?? string.Empty, "atlas.json");
        if (!File.Exists(atlasPath))
        {{
            Debug.LogError("Could not find atlas.json next to " + texturePath);
            return;
        }}

        var importer = AssetImporter.GetAtPath(texturePath) as TextureImporter;
        if (importer == null)
        {{
            Debug.LogError("Selected asset is not a texture importer.");
            return;
        }}

        var atlas = JsonUtility.FromJson<Atlas>(File.ReadAllText(atlasPath));
        var metadata = new SpriteMetaData[atlas.frames.Length];
        for (var i = 0; i < atlas.frames.Length; i++)
        {{
            var frame = atlas.frames[i];
            metadata[i] = new SpriteMetaData
            {{
                name = frame.filename,
                rect = new Rect(frame.frame.x, texture.height - frame.frame.y - frame.frame.h, frame.frame.w, frame.frame.h),
                alignment = (int)SpriteAlignment.Custom,
                pivot = new Vector2(frame.pivot.x, frame.pivot.y)
            }};
        }}

        importer.textureType = TextureImporterType.Sprite;
        importer.spriteImportMode = SpriteImportMode.Multiple;
        importer.alphaIsTransparency = true;
        importer.spritesheet = metadata;
        EditorUtility.SetDirty(importer);
        importer.SaveAndReimport();
        Debug.Log("Imported " + metadata.Length + " sprites from Sprite Atlas Packer atlas.json.");
    }}
}}
#endif
"#,
        image = escape_csharp_string(&atlas.meta.image),
        frame_count = atlas.frames.len()
    );
    write_text_if_changed(path, &text)?;
    Ok(())
}

fn reject_rotated_native_export(atlas: &Atlas, name: &str) -> Result<()> {
    if atlas.frames.iter().any(|frame| frame.rotated) {
        return Err(AtlasError::Message(format!(
            "{name} export does not support rotated frames; rerun with --no-rotate"
        )));
    }
    Ok(())
}

fn atlas_animation_sequences(atlas: &Atlas) -> Vec<Animation> {
    if !atlas.animations.is_empty() {
        return atlas.animations.clone();
    }
    vec![Animation {
        name: "default".to_string(),
        frames: atlas
            .frames
            .iter()
            .map(|frame| frame.filename.clone())
            .collect(),
        looped: true,
        fps_hint: 1000.0 / DEFAULT_DURATION_MS as f32,
    }]
}

fn sanitize_resource_id(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn escape_godot_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn escape_csharp_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn format_float(value: f32) -> String {
    let mut formatted = format!("{value:.3}");
    while formatted.contains('.') && formatted.ends_with('0') {
        formatted.pop();
    }
    if formatted.ends_with('.') {
        formatted.push('0');
    }
    formatted
}

fn parse_args(args: Vec<String>) -> Result<PackOptions> {
    let mut options = PackOptions::default();
    let mut overrides = CliOverrides::default();
    let mut project_path: Option<PathBuf> = None;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--headless" => {}
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            "--input" => {
                index += 1;
                options.input = PathBuf::from(require_value(&args, index, "--input")?);
                overrides.input = true;
            }
            "--output" => {
                index += 1;
                options.output = PathBuf::from(require_value(&args, index, "--output")?);
                overrides.output = true;
            }
            "--format" => {
                index += 1;
                options.format = OutputFormat::parse(require_value(&args, index, "--format")?)?;
                overrides.format = true;
            }
            "--algo" => {
                index += 1;
                options.packing_strategy =
                    PackingStrategy::parse(require_value(&args, index, "--algo")?)?;
                overrides.packing_strategy = true;
            }
            "--pot" => {
                options.pot = true;
                overrides.pot = true;
            }
            "--npot" => {
                options.pot = false;
                overrides.pot = true;
            }
            "--padding" => {
                index += 1;
                options.padding = require_value(&args, index, "--padding")?
                    .parse()
                    .map_err(|error| AtlasError::Message(format!("Invalid --padding: {error}")))?;
                overrides.padding = true;
            }
            "--max-size" => {
                index += 1;
                options.max_size = require_value(&args, index, "--max-size")?
                    .parse()
                    .map_err(|error| AtlasError::Message(format!("Invalid --max-size: {error}")))?;
                overrides.max_size = true;
            }
            "--rotate" => {
                options.rotate = true;
                overrides.rotate = true;
            }
            "--no-rotate" => {
                options.rotate = false;
                overrides.rotate = true;
            }
            "--trim" => {
                options.trim = true;
                overrides.trim = true;
            }
            "--no-trim" => {
                options.trim = false;
                overrides.trim = true;
            }
            "--dry-run" => options.dry_run = true,
            "--aseprite-json" => {
                options.aseprite_json = true;
                overrides.aseprite_json = true;
            }
            "--godot-spriteframes" => {
                options.godot_spriteframes = true;
                overrides.godot_spriteframes = true;
            }
            "--no-godot-spriteframes" => {
                options.godot_spriteframes = false;
                overrides.godot_spriteframes = true;
            }
            "--unity-importer" => {
                options.unity_importer = true;
                overrides.unity_importer = true;
            }
            "--no-unity-importer" => {
                options.unity_importer = false;
                overrides.unity_importer = true;
            }
            "--animation-regex" => {
                index += 1;
                options.animation_regex =
                    Some(require_value(&args, index, "--animation-regex")?.to_string());
                overrides.animation_regex = true;
            }
            "--stable-pivot" => {
                options.stable_pivot = true;
                overrides.stable_pivot = true;
            }
            "--no-stable-pivot" => {
                options.stable_pivot = false;
                overrides.stable_pivot = true;
            }
            "--dedup" => {
                options.dedup_threshold = Some(0);
                overrides.dedup_threshold = true;
            }
            "--dedup-threshold" => {
                index += 1;
                options.dedup_threshold = Some(
                    require_value(&args, index, "--dedup-threshold")?
                        .parse()
                        .map_err(|error| {
                            AtlasError::Message(format!("Invalid --dedup-threshold: {error}"))
                        })?,
                );
                overrides.dedup_threshold = true;
            }
            "--no-dedup" => {
                options.dedup_threshold = None;
                overrides.dedup_threshold = true;
            }
            "--linked-channels" => {
                index += 1;
                options.linked_channels =
                    parse_channel_suffixes(require_value(&args, index, "--linked-channels")?);
                overrides.linked_channels = true;
            }
            "--project" => {
                index += 1;
                project_path = Some(PathBuf::from(require_value(&args, index, "--project")?));
            }
            "--watch" => options.watch = true,
            "--slice" => {
                index += 1;
                options.slice_mode =
                    Some(SliceMode::parse(require_value(&args, index, "--slice")?)?);
            }
            "--cell" => {
                index += 1;
                options.cell = Some(parse_cell(require_value(&args, index, "--cell")?)?);
            }
            "--prefix" => {
                index += 1;
                options.prefix = require_value(&args, index, "--prefix")?.to_string();
            }
            "--normalize-generated" | "--normalize" => {
                options.normalize_generated = true;
            }
            "--no-normalize-generated" => {
                options.normalize_generated = false;
            }
            "--frame-size" => {
                index += 1;
                options.frame_size =
                    Some(parse_cell(require_value(&args, index, "--frame-size")?)?);
            }
            value if value.starts_with('-') => {
                return Err(AtlasError::Message(format!("Unknown option: {value}")));
            }
            value => {
                if options.input.as_os_str().is_empty() {
                    options.input = PathBuf::from(value);
                    overrides.input = true;
                } else if options.output.as_os_str().is_empty() {
                    options.output = PathBuf::from(value);
                    overrides.output = true;
                } else {
                    return Err(AtlasError::Message(format!("Unexpected argument: {value}")));
                }
            }
        }
        index += 1;
    }
    if let Some(path) = project_path {
        apply_project_file(&mut options, &overrides, &path)?;
    }
    Ok(options)
}

fn parse_serve_args(args: &[String]) -> Result<ServeConfig> {
    let mut host = "127.0.0.1".to_string();
    let mut port = 9877;
    let mut token: Option<String> = None;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--headless" | "--serve" => {}
            "--host" => {
                index += 1;
                host = require_value(args, index, "--host")?.to_string();
            }
            "--port" => {
                index += 1;
                port = require_value(args, index, "--port")?
                    .parse()
                    .map_err(|error| AtlasError::Message(format!("Invalid --port: {error}")))?;
            }
            "--token" => {
                index += 1;
                token = Some(require_value(args, index, "--token")?.to_string());
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            value => {
                return Err(AtlasError::Message(format!(
                    "Unknown --serve option: {value}"
                )));
            }
        }
        index += 1;
    }
    Ok(ServeConfig {
        host,
        port,
        token: token.unwrap_or_else(generate_control_token),
    })
}

fn generate_control_token() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("sap-{}-{nanos:x}", std::process::id())
}

fn serve_control_api(config: ServeConfig) -> Result<()> {
    let listener = bind_control_listener(&config)?;
    write_control_discovery(&config)?;
    eprintln!(
        "{}",
        serde_json::json!({
            "status": "serving",
            "host": config.host,
            "port": config.port,
            "discoveryFile": control_discovery_path(),
        })
    );
    run_control_listener(config, listener, None)
}

fn bind_control_listener(config: &ServeConfig) -> Result<TcpListener> {
    let address = format!("{}:{}", config.host, config.port);
    Ok(TcpListener::bind(&address)?)
}

fn run_control_listener(
    config: ServeConfig,
    listener: TcpListener,
    shutdown: Option<Arc<AtomicBool>>,
) -> Result<()> {
    let state = Arc::new(Mutex::new(ControlState::default()));
    let event_clients: EventClients = Arc::new(Mutex::new(Vec::new()));
    loop {
        if shutdown
            .as_ref()
            .map(|shutdown| shutdown.load(AtomicOrdering::SeqCst))
            .unwrap_or(false)
        {
            return Ok(());
        }
        match listener.accept() {
            Ok((stream, _address)) => {
                let connection_config = config.clone();
                let connection_state = Arc::clone(&state);
                let connection_clients = Arc::clone(&event_clients);
                thread::spawn(move || {
                    if let Err(error) = handle_control_connection(
                        stream,
                        &connection_config,
                        &connection_state,
                        &connection_clients,
                    ) {
                        eprintln!(
                            "{}",
                            serde_json::json!({
                                "status": "control_error",
                                "error": error.to_string(),
                            })
                        );
                    }
                });
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => {
                eprintln!(
                    "{}",
                    serde_json::json!({
                        "status": "control_accept_error",
                        "error": error.to_string(),
                    })
                );
                if shutdown.is_none() {
                    return Err(error.into());
                }
            }
        }
    }
}

#[cfg(feature = "desktop")]
fn gui_control_server_info(server: Option<&GuiControlServer>) -> GuiControlServerInfo {
    if let Some(server) = server {
        GuiControlServerInfo {
            enabled: true,
            host: server.config.host.clone(),
            port: server.config.port,
            token: server.config.token.clone(),
            base_url: format!(
                "http://{}:{}/api/v1",
                server.config.host, server.config.port
            ),
            discovery_file: control_discovery_display_path()
                .to_string_lossy()
                .to_string(),
        }
    } else {
        GuiControlServerInfo {
            enabled: false,
            host: "127.0.0.1".to_string(),
            port: 9877,
            token: String::new(),
            base_url: "http://127.0.0.1:9877/api/v1".to_string(),
            discovery_file: control_discovery_display_path()
                .to_string_lossy()
                .to_string(),
        }
    }
}

fn write_control_discovery(config: &ServeConfig) -> Result<()> {
    let value = serde_json::json!({
        "app": APP_NAME,
        "host": config.host,
        "port": config.port,
        "token": config.token,
        "baseUrl": format!("http://{}:{}/api/v1", config.host, config.port),
        "eventsUrl": format!("ws://{}:{}/api/v1/events?token={}", config.host, config.port, config.token),
    });
    let payload = format!("{}\n", serde_json::to_string_pretty(&value)?);
    let mut last_error = None;
    let mut wrote_any = false;
    for path in control_discovery_paths() {
        let write_result = (|| -> Result<()> {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            write_text_if_changed(&path, &payload)
        })();
        match write_result {
            Ok(()) => {
                wrote_any = true;
            }
            Err(error) => last_error = Some(error),
        }
    }
    if wrote_any {
        register_agent_api(config, false, None);
        return Ok(());
    }
    Err(last_error.unwrap_or_else(|| {
        AtlasError::Message("Unable to write control discovery file".to_string())
    }))
}

fn control_discovery_path() -> PathBuf {
    #[cfg(windows)]
    {
        return dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(APP_NAME)
            .join("control-api.json");
    }
    #[cfg(not(windows))]
    {
        if let Ok(config_home) = std::env::var("XDG_CONFIG_HOME") {
            return PathBuf::from(config_home)
                .join(APP_NAME)
                .join("control-api.json");
        }
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home)
                .join(".config")
                .join(APP_NAME)
                .join("control-api.json");
        }
        PathBuf::from("control-api.json")
    }
}

fn control_discovery_paths() -> Vec<PathBuf> {
    let mut paths = vec![control_discovery_path()];
    if let Ok(current_dir) = std::env::current_dir() {
        let fallback = current_dir.join("control-api.json");
        if !paths.iter().any(|path| path == &fallback) {
            paths.push(fallback);
        }
    }
    paths
}

fn control_discovery_display_path() -> PathBuf {
    control_discovery_paths()
        .into_iter()
        .find(|path| path.exists())
        .unwrap_or_else(control_discovery_path)
}

fn agent_api_registry_path() -> Option<PathBuf> {
    // Same shared folder Neko Legends Control Center reads on each platform.
    #[cfg(windows)]
    let root = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let root = std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Library").join("Application Support"));
    #[cfg(all(not(windows), not(target_os = "macos")))]
    let root = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    root.map(|root| root.join("NekoLegends").join(AGENT_API_REGISTRY_FILE))
}

fn registry_timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

fn read_registry_value(path: &Path) -> serde_json::Value {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .filter(|value| value.is_object())
        .unwrap_or_else(|| serde_json::json!({ "updatedAt": registry_timestamp(), "apps": [] }))
}

fn with_registry_lock<F>(path: &Path, mut write: F) -> Result<()>
where
    F: FnMut() -> Result<()>,
{
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let lock_path = path.with_extension("json.lock");
    for _ in 0..3 {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(_) => {
                let result = write();
                let _ = fs::remove_file(&lock_path);
                return result;
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                let stale = fs::metadata(&lock_path)
                    .and_then(|metadata| metadata.modified())
                    .ok()
                    .and_then(|modified| modified.elapsed().ok())
                    .map(|elapsed| elapsed > Duration::from_secs(2))
                    .unwrap_or(false);
                if stale {
                    let _ = fs::remove_file(&lock_path);
                } else {
                    thread::sleep(Duration::from_millis(100));
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
    Err(AtlasError::Message(
        "Agent API registry is busy.".to_string(),
    ))
}

fn register_agent_api(config: &ServeConfig, busy: bool, active_job_id: Option<String>) {
    let Some(path) = agent_api_registry_path() else {
        return;
    };
    let _ = with_registry_lock(&path, || {
        let mut registry = read_registry_value(&path);
        registry["updatedAt"] = serde_json::json!(registry_timestamp());
        if !registry
            .get("apps")
            .map(serde_json::Value::is_array)
            .unwrap_or(false)
        {
            registry["apps"] = serde_json::json!([]);
        }
        let url = format!("http://{}:{}", config.host, config.port);
        let entry = serde_json::json!({
            "appId": "sprite-atlas-packer",
            "appName": "Sprite Atlas Packer",
            "defaultPort": SAP_DEFAULT_PORT,
            "bindAddress": SAP_BIND_ADDRESS,
            "port": config.port,
            "enabled": true,
            "url": url,
            "openapiUrl": format!("{url}/openapi.json"),
            "busy": busy,
            "activeJobId": active_job_id,
            "lastSeen": registry_timestamp(),
            "note": "Pack/slice control API."
        });
        if let Some(apps) = registry
            .get_mut("apps")
            .and_then(serde_json::Value::as_array_mut)
        {
            if let Some(existing) = apps.iter_mut().find(|candidate| {
                candidate
                    .get("appId")
                    .and_then(serde_json::Value::as_str)
                    .map(|app_id| app_id == "sprite-atlas-packer")
                    .unwrap_or(false)
            }) {
                *existing = entry;
            } else {
                apps.push(entry);
            }
        }
        write_text_if_changed(
            &path,
            &format!("{}\n", serde_json::to_string_pretty(&registry)?),
        )?;
        Ok(())
    });
}

fn handle_control_connection(
    mut stream: TcpStream,
    config: &ServeConfig,
    state: &Arc<Mutex<ControlState>>,
    event_clients: &EventClients,
) -> Result<()> {
    let request = read_http_request(&mut stream)?;
    if request.method == "OPTIONS" {
        return write_http_json(&mut stream, 204, &serde_json::json!({}));
    }
    if request.method == "GET" && request.path == "/openapi.json" {
        return write_http_json(&mut stream, 200, &control_openapi_json());
    }
    if !is_authorized(&request, &config.token) {
        return write_http_json(
            &mut stream,
            401,
            &serde_json::json!({ "error": "unauthorized" }),
        );
    }

    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/api/v1/events") => handle_websocket_events(stream, &request, event_clients),
        ("GET", "/api/v1/state") => {
            let state = state
                .lock()
                .map_err(|error| AtlasError::Message(error.to_string()))?;
            write_http_json(&mut stream, 200, &control_state_json(&state))
        }
        ("GET", "/api/v1/atlas") => {
            let state = state
                .lock()
                .map_err(|error| AtlasError::Message(error.to_string()))?;
            if let Some(atlas) = &state.last_atlas {
                write_http_json(
                    &mut stream,
                    200,
                    &serde_json::json!({
                        "atlas": atlas,
                        "sheetPath": state.last_sheet_path,
                        "outputPath": state.last_output,
                    }),
                )
            } else {
                write_http_json(
                    &mut stream,
                    404,
                    &serde_json::json!({ "error": "No atlas has been produced yet" }),
                )
            }
        }
        ("POST", "/api/v1/pack") => {
            let gui: GuiPackOptions = serde_json::from_slice(&request.body)?;
            let pixel_snap_options = pixel_snap_options_from_gui(&gui);
            let options = pack_options_from_gui(gui)?;
            let sheet_path = options.output.join(options.format.atlas_image_name());
            let output = options.output.clone();
            let report = pack_with_pixel_snap(options, pixel_snap_options)?;
            let sheet_path = sheet_path.to_string_lossy().to_string();
            let output = output.to_string_lossy().to_string();
            {
                let mut state = state
                    .lock()
                    .map_err(|error| AtlasError::Message(error.to_string()))?;
                state.last_atlas = Some(report.atlas.clone());
                state.last_sheet_path = Some(sheet_path.clone());
                state.last_output = Some(output.clone());
                state.last_pack = Some(report.clone());
            }
            broadcast_control_event(
                event_clients,
                &serde_json::json!({
                    "type": "pack",
                    "atlas": report.atlas.clone(),
                    "sheetPath": sheet_path,
                    "outputPath": output,
                    "wrote": report.wrote.clone(),
                }),
            );
            write_http_json(&mut stream, 200, &report)
        }
        ("POST", "/api/v1/slice") => {
            let gui: GuiSliceOptions = serde_json::from_slice(&request.body)?;
            let options = slice_options_from_gui(gui)?;
            let sheet_path = options.output.join(options.format.atlas_image_name());
            let output = options.output.clone();
            let report = slice_sheet(options)?;
            let sheet_path = sheet_path.to_string_lossy().to_string();
            let output = output.to_string_lossy().to_string();
            {
                let mut state = state
                    .lock()
                    .map_err(|error| AtlasError::Message(error.to_string()))?;
                state.last_atlas = Some(report.atlas.clone());
                state.last_sheet_path = Some(sheet_path.clone());
                state.last_output = Some(output.clone());
                state.last_slice = Some(report.clone());
            }
            broadcast_control_event(
                event_clients,
                &serde_json::json!({
                    "type": "slice",
                    "atlas": report.atlas.clone(),
                    "sheetPath": sheet_path,
                    "outputPath": output,
                    "wrote": report.wrote.clone(),
                }),
            );
            write_http_json(&mut stream, 200, &report)
        }
        ("POST", "/api/v1/animation-job/start") => {
            let options: AnimationJobStartOptions = serde_json::from_slice(&request.body)?;
            let report = start_animation_job(options)?;
            write_http_json(&mut stream, 200, &report)
        }
        ("POST", "/api/v1/animation-job/submit-frame") => {
            let options: AnimationJobSubmitFrameOptions = serde_json::from_slice(&request.body)?;
            let report = submit_animation_job_frame(options)?;
            write_http_json(&mut stream, 200, &report)
        }
        ("POST", "/api/v1/animation-job/assemble") => {
            let gui: AnimationJobAssembleOptions = serde_json::from_slice(&request.body)?;
            let output = gui
                .output
                .as_ref()
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(&gui.job_dir).join("assembled"));
            let format = OutputFormat::parse(&gui.format)?;
            let sheet_path = output.join(format.atlas_image_name());
            let report = assemble_animation_job(gui)?;
            let sheet_path = sheet_path.to_string_lossy().to_string();
            let output = output.to_string_lossy().to_string();
            {
                let mut state = state
                    .lock()
                    .map_err(|error| AtlasError::Message(error.to_string()))?;
                state.last_atlas = Some(report.atlas.clone());
                state.last_sheet_path = Some(sheet_path.clone());
                state.last_output = Some(output.clone());
                state.last_slice = Some(report.clone());
            }
            broadcast_control_event(
                event_clients,
                &serde_json::json!({
                    "type": "animationJobAssemble",
                    "atlas": report.atlas.clone(),
                    "sheetPath": sheet_path,
                    "outputPath": output,
                    "wrote": report.wrote.clone(),
                }),
            );
            write_http_json(&mut stream, 200, &report)
        }
        ("POST", "/api/v1/assemble-generated-loop") => {
            let gui: GuiGeneratedLoopOptions = serde_json::from_slice(&request.body)?;
            let options = generated_loop_options_from_gui(gui)?;
            let sheet_path = options.output.join(options.format.atlas_image_name());
            let output = options.output.clone();
            let report = assemble_generated_loop(options)?;
            let sheet_path = sheet_path.to_string_lossy().to_string();
            let output = output.to_string_lossy().to_string();
            {
                let mut state = state
                    .lock()
                    .map_err(|error| AtlasError::Message(error.to_string()))?;
                state.last_atlas = Some(report.atlas.clone());
                state.last_sheet_path = Some(sheet_path.clone());
                state.last_output = Some(output.clone());
                state.last_slice = Some(report.clone());
            }
            broadcast_control_event(
                event_clients,
                &serde_json::json!({
                    "type": "assembleGeneratedLoop",
                    "atlas": report.atlas.clone(),
                    "sheetPath": sheet_path,
                    "outputPath": output,
                    "wrote": report.wrote.clone(),
                }),
            );
            write_http_json(&mut stream, 200, &report)
        }
        ("POST", "/api/v1/export-gif") => {
            let gui: GuiGifExportOptions = serde_json::from_slice(&request.body)?;
            let report = export_gif(
                Path::new(&gui.atlas_path),
                Path::new(&gui.sheet_path),
                Path::new(&gui.output_path),
                &gui.animation,
                gui.fps_override,
                gui.frame_start,
                gui.frame_end,
            )?;
            broadcast_control_event(
                event_clients,
                &serde_json::json!({
                    "type": "exportGif",
                    "outputPath": report.output_path.clone(),
                    "animation": report.animation.clone(),
                    "frameCount": report.frame_count,
                    "encoder": report.encoder.clone(),
                }),
            );
            write_http_json(&mut stream, 200, &report)
        }
        _ => write_http_json(
            &mut stream,
            404,
            &serde_json::json!({ "error": "Unknown endpoint" }),
        ),
    }
}

fn control_openapi_json() -> serde_json::Value {
    serde_json::json!({
        "openapi": "3.0.3",
        "info": {
            "title": "Sprite Atlas Packer Control API",
            "version": VERSION
        },
        "paths": {
            "/api/v1/state": { "get": { "summary": "Control server state" } },
            "/api/v1/atlas": { "get": { "summary": "Last atlas result" } },
            "/api/v1/pack": { "post": { "summary": "Pack sprites into an atlas" } },
            "/api/v1/slice": { "post": { "summary": "Slice a sprite sheet into frames" } },
            "/api/v1/animation-job/start": { "post": { "summary": "Start an agent-directed frame-by-frame animation job" } },
            "/api/v1/animation-job/submit-frame": { "post": { "summary": "Submit and validate one generated animation frame" } },
            "/api/v1/animation-job/assemble": { "post": { "summary": "Assemble accepted animation job frames" } },
            "/api/v1/assemble-generated-loop": { "post": { "summary": "Normalize separate generated frames into one loop spritesheet" } },
            "/api/v1/export-gif": { "post": { "summary": "Export an animation preview GIF" } },
            "/api/v1/events": { "get": { "summary": "Event stream" } }
        }
    })
}

fn control_state_json(state: &ControlState) -> serde_json::Value {
    serde_json::json!({
        "app": APP_NAME,
        "version": VERSION,
        "status": "ready",
        "lastPackResult": state.last_pack,
        "lastSliceResult": state.last_slice,
        "lastAtlas": state.last_atlas,
        "lastSheetPath": state.last_sheet_path,
        "lastOutputPath": state.last_output,
    })
}

fn pack_options_from_gui(options: GuiPackOptions) -> Result<PackOptions> {
    Ok(PackOptions {
        input: PathBuf::from(options.input),
        output: PathBuf::from(options.output),
        format: OutputFormat::parse(&options.format)?,
        packing_strategy: PackingStrategy::parse(&options.packing_strategy)?,
        pot: options.pot,
        padding: options.padding,
        max_size: options.max_size,
        rotate: options.rotate,
        trim: options.trim,
        dry_run: options.dry_run,
        watch: false,
        slice_mode: None,
        cell: None,
        prefix: "frame".to_string(),
        aseprite_json: options.aseprite_json,
        godot_spriteframes: options.godot_spriteframes,
        unity_importer: options.unity_importer,
        animation_regex: clean_optional_string(options.animation_regex),
        animations: Vec::new(),
        stable_pivot: options.stable_pivot,
        dedup_threshold: options.dedup_threshold,
        linked_channels: normalize_channel_suffixes(options.linked_channels),
        normalize_generated: false,
        frame_size: None,
    })
}

fn pixel_snap_options_from_gui(options: &GuiPackOptions) -> PixelSnapOptions {
    PixelSnapOptions {
        enabled: options.pixel_snap,
        color_count: options.pixel_snap_colors,
        pixel_size: options.pixel_snap_size,
        palette: options.pixel_snap_palette.clone(),
        lock_grid: options.pixel_snap_lock_grid,
        engine: PixelSnapEngine::parse(&options.pixel_snap_engine).unwrap_or_default(),
    }
}

fn slice_options_from_gui(options: GuiSliceOptions) -> Result<PackOptions> {
    let mode = SliceMode::parse(&options.mode)?;
    let cell = if mode == SliceMode::Grid && options.cell_width > 0 && options.cell_height > 0 {
        Some((options.cell_width, options.cell_height))
    } else {
        None
    };
    Ok(PackOptions {
        input: PathBuf::from(options.input),
        output: PathBuf::from(options.output),
        format: OutputFormat::parse(&options.format)?,
        packing_strategy: PackingStrategy::MaxRects,
        pot: true,
        padding: 0,
        max_size: DEFAULT_MAX_SIZE,
        rotate: false,
        trim: false,
        dry_run: options.dry_run,
        watch: false,
        slice_mode: Some(mode),
        cell,
        prefix: if options.prefix.trim().is_empty() {
            "frame".to_string()
        } else {
            options.prefix
        },
        aseprite_json: options.aseprite_json,
        godot_spriteframes: false,
        unity_importer: false,
        animation_regex: None,
        animations: Vec::new(),
        stable_pivot: false,
        dedup_threshold: None,
        linked_channels: Vec::new(),
        normalize_generated: options.normalize_generated,
        frame_size: normalized_frame_size_option(options.frame_width, options.frame_height),
    })
}

fn generated_loop_options_from_gui(
    options: GuiGeneratedLoopOptions,
) -> Result<GeneratedLoopOptions> {
    Ok(GeneratedLoopOptions {
        input: PathBuf::from(options.input),
        output: PathBuf::from(options.output),
        format: OutputFormat::parse(&options.format)?,
        dry_run: options.dry_run,
        columns: (options.columns > 0).then_some(options.columns),
        prefix: if options.prefix.trim().is_empty() {
            "frame".to_string()
        } else {
            options.prefix
        },
        aseprite_json: options.aseprite_json,
        frame_size: normalized_frame_size_option(options.frame_width, options.frame_height),
    })
}

#[derive(Debug)]
struct HttpRequest {
    method: String,
    path: String,
    query: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

fn read_http_request(stream: &mut TcpStream) -> Result<HttpRequest> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default();
    let (path, query) = target
        .split_once('?')
        .map(|(path, query)| (path.to_string(), query.to_string()))
        .unwrap_or_else(|| (target.to_string(), String::new()));
    if method.is_empty() || path.is_empty() {
        return Err(AtlasError::Message("Invalid HTTP request".to_string()));
    }

    let mut headers = BTreeMap::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body)?;
    }
    Ok(HttpRequest {
        method,
        path,
        query,
        headers,
        body,
    })
}

fn is_authorized(request: &HttpRequest, token: &str) -> bool {
    request
        .headers
        .get("authorization")
        .map(|value| value == &format!("Bearer {token}"))
        .unwrap_or(false)
        || request
            .query
            .split('&')
            .filter_map(|part| part.split_once('='))
            .any(|(key, value)| key == "token" && value == token)
}

fn handle_websocket_events(
    mut stream: TcpStream,
    request: &HttpRequest,
    event_clients: &EventClients,
) -> Result<()> {
    if !request
        .headers
        .get("upgrade")
        .map(|value| value.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false)
    {
        return write_http_json(
            &mut stream,
            400,
            &serde_json::json!({ "error": "Expected WebSocket upgrade" }),
        );
    }
    let key = request
        .headers
        .get("sec-websocket-key")
        .ok_or_else(|| AtlasError::Message("Missing Sec-WebSocket-Key".to_string()))?;
    let accept = websocket_accept_key(key);
    write!(
        stream,
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {accept}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         \r\n"
    )?;
    stream.flush()?;
    event_clients
        .lock()
        .map_err(|error| AtlasError::Message(error.to_string()))?
        .push(stream);
    Ok(())
}

fn websocket_accept_key(key: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    general_purpose::STANDARD.encode(hasher.finalize())
}

fn broadcast_control_event(event_clients: &EventClients, value: &serde_json::Value) {
    let Ok(text) = serde_json::to_string(value) else {
        return;
    };
    let Ok(mut clients) = event_clients.lock() else {
        return;
    };
    clients.retain_mut(|stream| write_websocket_text_frame(stream, &text).is_ok());
}

fn write_websocket_text_frame(stream: &mut TcpStream, text: &str) -> Result<()> {
    let payload = text.as_bytes();
    let mut frame = vec![0x81];
    if payload.len() < 126 {
        frame.push(payload.len() as u8);
    } else if payload.len() <= u16::MAX as usize {
        frame.push(126);
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    } else {
        frame.push(127);
        frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    }
    stream.write_all(&frame)?;
    stream.write_all(payload)?;
    stream.flush()?;
    Ok(())
}

fn write_http_json<T: Serialize>(stream: &mut TcpStream, status: u16, value: &T) -> Result<()> {
    let body = if status == 204 {
        Vec::new()
    } else {
        serde_json::to_vec_pretty(value)?
    };
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Headers: Authorization, Content-Type\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    if !body.is_empty() {
        stream.write_all(&body)?;
    }
    stream.flush()?;
    Ok(())
}

fn apply_project_file(
    options: &mut PackOptions,
    overrides: &CliOverrides,
    path: &Path,
) -> Result<()> {
    let project = read_project_file(path)?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));

    if !overrides.packing_strategy {
        if let Some(strategy) = project.packing_strategy.as_deref() {
            options.packing_strategy = PackingStrategy::parse(strategy)?;
        }
    }
    if !overrides.input {
        if let Some(input) = project.source_folder {
            options.input = absolutize_project_path(base, input);
        }
    }
    if !overrides.output {
        if let Some(output) = project.output_path {
            options.output = absolutize_project_path(base, output);
        }
    }
    if !overrides.format {
        if let Some(format) = project.format {
            options.format = OutputFormat::parse(&format)?;
        }
    }
    if !overrides.pot {
        if let Some(pot) = project.pot {
            options.pot = pot;
        }
    }
    if !overrides.padding {
        if let Some(padding) = project.padding {
            options.padding = padding;
        }
    }
    if !overrides.max_size {
        if let Some(max_size) = project.max_size {
            options.max_size = max_size;
        }
    }
    if !overrides.rotate {
        if let Some(rotate) = project.rotate {
            options.rotate = rotate;
        }
    }
    if !overrides.trim {
        if let Some(trim) = project.trim {
            options.trim = trim;
        }
    }
    if !overrides.aseprite_json {
        if let Some(aseprite_json) = project.aseprite_json {
            options.aseprite_json = aseprite_json;
        }
    }
    if !overrides.godot_spriteframes {
        if let Some(godot_spriteframes) = project.godot_spriteframes {
            options.godot_spriteframes = godot_spriteframes;
        }
    }
    if !overrides.unity_importer {
        if let Some(unity_importer) = project.unity_importer {
            options.unity_importer = unity_importer;
        }
    }
    if !overrides.animation_regex {
        options.animation_regex = project.animation_regex;
    }
    if !overrides.stable_pivot {
        if let Some(stable_pivot) = project.stable_pivot {
            options.stable_pivot = stable_pivot;
        }
    }
    if !overrides.dedup_threshold {
        options.dedup_threshold = project.dedup_threshold;
    }
    if !overrides.linked_channels {
        options.linked_channels = normalize_channel_suffixes(project.linked_channels);
    }
    options.animations = project.animations;
    Ok(())
}

fn read_project_file(path: &Path) -> Result<ProjectFile> {
    let bytes = fs::read(path)?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(feature = "desktop")]
fn project_to_gui_options(project: ProjectFile, base: &Path) -> GuiProjectOptions {
    GuiProjectOptions {
        input: project
            .source_folder
            .map(|path| absolutize_project_path(base, path))
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
        output: project
            .output_path
            .map(|path| absolutize_project_path(base, path))
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
        format: project.format.unwrap_or_else(|| "webp".to_string()),
        packing_strategy: project
            .packing_strategy
            .unwrap_or_else(default_packing_strategy),
        pot: project.pot.unwrap_or(true),
        padding: project.padding.unwrap_or(2),
        max_size: project.max_size.unwrap_or(DEFAULT_MAX_SIZE),
        rotate: project.rotate.unwrap_or(false),
        trim: project.trim.unwrap_or(true),
        aseprite_json: project.aseprite_json.unwrap_or(false),
        godot_spriteframes: project.godot_spriteframes.unwrap_or(false),
        unity_importer: project.unity_importer.unwrap_or(false),
        animation_regex: clean_optional_string(project.animation_regex),
        stable_pivot: project.stable_pivot.unwrap_or(false),
        dedup_threshold: project.dedup_threshold,
        linked_channels: normalize_channel_suffixes(project.linked_channels),
        pixel_snap: project.pixel_snap,
        pixel_snap_size: project.pixel_snap_size,
        pixel_snap_colors: project
            .pixel_snap_colors
            .unwrap_or_else(default_pixel_snap_colors),
        pixel_snap_palette: project.pixel_snap_palette,
        pixel_snap_lock_grid: project.pixel_snap_lock_grid,
        pixel_snap_engine: project
            .pixel_snap_engine
            .unwrap_or_else(default_pixel_snap_engine),
    }
}

fn project_from_options(options: &PackOptions) -> ProjectFile {
    ProjectFile {
        source_folder: Some(options.input.clone()),
        output_path: Some(options.output.clone()),
        format: Some(options.format.as_str().to_string()),
        pot: Some(options.pot),
        padding: Some(options.padding),
        max_size: Some(options.max_size),
        rotate: Some(options.rotate),
        trim: Some(options.trim),
        aseprite_json: Some(options.aseprite_json),
        godot_spriteframes: Some(options.godot_spriteframes),
        unity_importer: Some(options.unity_importer),
        animation_regex: clean_optional_string(options.animation_regex.clone()),
        stable_pivot: Some(options.stable_pivot),
        dedup_threshold: options.dedup_threshold,
        linked_channels: normalize_channel_suffixes(options.linked_channels.clone()),
        pixel_snap: false,
        pixel_snap_size: None,
        pixel_snap_colors: None,
        pixel_snap_palette: String::new(),
        pixel_snap_lock_grid: true,
        pixel_snap_engine: None,
        animations: options.animations.clone(),
        packing_strategy: Some(options.packing_strategy.as_str().to_string()),
    }
}

fn clean_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn parse_channel_suffixes(value: &str) -> Vec<String> {
    normalize_channel_suffixes(value.split(',').map(str::to_string).collect())
}

fn normalize_channel_suffixes(values: Vec<String>) -> Vec<String> {
    let mut suffixes = values
        .into_iter()
        .filter_map(|value| {
            let value = value.trim();
            if value.is_empty() {
                None
            } else if value.starts_with('_') {
                Some(value.to_string())
            } else {
                Some(format!("_{value}"))
            }
        })
        .collect::<Vec<_>>();
    suffixes.sort();
    suffixes.dedup();
    suffixes
}

pub fn write_project_file(path: &Path, options: &PackOptions) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let json = serde_json::to_string_pretty(&project_from_options(options))?;
    write_text_if_changed(path, &format!("{json}\n"))?;
    Ok(())
}

fn watch(options: PackOptions) -> Result<()> {
    if options.dry_run {
        return Err(AtlasError::Message(
            "--watch cannot be combined with --dry-run".to_string(),
        ));
    }
    validate_options(&options)?;
    let (tx, rx) = mpsc::channel();
    let mut watcher = RecommendedWatcher::new(
        move |result| {
            let _ = tx.send(result);
        },
        Config::default().with_poll_interval(Duration::from_millis(500)),
    )
    .map_err(|error| AtlasError::Message(format!("Unable to start watcher: {error}")))?;
    watcher
        .watch(&options.input, RecursiveMode::Recursive)
        .map_err(|error| AtlasError::Message(format!("Unable to watch input: {error}")))?;

    let mut last_snapshot = String::new();
    repack_if_changed(&options, &mut last_snapshot)?;
    eprintln!(
        "{}",
        serde_json::json!({
            "status": "watching",
            "input": options.input,
            "output": options.output
        })
    );

    let mut last_repack = Instant::now();
    loop {
        match rx.recv() {
            Ok(Ok(event)) => {
                if event_touches_output(&event.paths, &options.output) {
                    continue;
                }
                if last_repack.elapsed() < Duration::from_millis(120) {
                    continue;
                }
                last_repack = Instant::now();
                if let Err(error) = repack_if_changed(&options, &mut last_snapshot) {
                    eprintln!(
                        "{}",
                        serde_json::json!({
                            "status": "error",
                            "error": error.to_string()
                        })
                    );
                }
            }
            Ok(Err(error)) => {
                eprintln!(
                    "{}",
                    serde_json::json!({
                        "status": "watch_error",
                        "error": error.to_string()
                    })
                );
            }
            Err(error) => {
                return Err(AtlasError::Message(format!(
                    "Watcher channel closed: {error}"
                )));
            }
        }
    }
}

fn repack_if_changed(options: &PackOptions, last_snapshot: &mut String) -> Result<()> {
    let snapshot = source_snapshot(&options.input)?;
    if snapshot == *last_snapshot {
        return Ok(());
    }
    let report = pack(PackOptions {
        watch: false,
        ..options.clone()
    })?;
    *last_snapshot = snapshot;
    eprintln!("{}", serde_json::to_string(&report)?);
    Ok(())
}

fn event_touches_output(paths: &[PathBuf], output: &Path) -> bool {
    paths.iter().any(|path| path.starts_with(output))
}

fn source_snapshot(input: &Path) -> Result<String> {
    let mut lines = Vec::new();
    if input.is_file() {
        if is_supported_image(input) {
            lines.push(file_snapshot_line(input)?);
        }
    } else {
        for entry in WalkDir::new(input).sort_by_file_name() {
            let entry = entry?;
            if entry.file_type().is_file() && is_supported_image(entry.path()) {
                lines.push(file_snapshot_line(entry.path())?);
            }
        }
    }
    lines.sort();
    Ok(lines.join("\n"))
}

fn file_snapshot_line(path: &Path) -> Result<String> {
    let bytes = fs::read(path)?;
    Ok(format!(
        "{}:{}:{:016x}",
        normalize_path(path),
        bytes.len(),
        fnv1a64(&bytes)
    ))
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn absolutize_project_path(base: &Path, value: PathBuf) -> PathBuf {
    if value.is_absolute() {
        value
    } else {
        base.join(value)
    }
}

fn require_value<'a>(args: &'a [String], index: usize, flag: &str) -> Result<&'a str> {
    args.get(index)
        .map(String::as_str)
        .filter(|value| !value.starts_with('-'))
        .ok_or_else(|| AtlasError::Message(format!("{flag} requires a value")))
}

fn parse_cell(value: &str) -> Result<(u32, u32)> {
    let Some((width, height)) = value.split_once('x').or_else(|| value.split_once('X')) else {
        return Err(AtlasError::Message(
            "--cell requires a value like 32x32".to_string(),
        ));
    };
    let width = width
        .parse::<u32>()
        .map_err(|error| AtlasError::Message(format!("Invalid --cell width: {error}")))?;
    let height = height
        .parse::<u32>()
        .map_err(|error| AtlasError::Message(format!("Invalid --cell height: {error}")))?;
    if width == 0 || height == 0 {
        return Err(AtlasError::Message(
            "--cell dimensions must be greater than zero".to_string(),
        ));
    }
    Ok((width, height))
}

fn print_help() {
    println!(
        "sprite-atlas-packer --headless --input <folder> --output <folder> [options]\n\n\
         Options:\n\
           --algo maxrects|shelf|guillotine|quadtree\n\
                                    Packing algorithm, default maxrects\n\
           Inputs                  PNG/JPG/WebP/GIF, plus .ase/.aseprite via Aseprite CLI\n\
           --format webp|png|both   Output image format, default webp\n\
           --pot | --npot           Power-of-two atlas sizing, default --pot\n\
           --padding <px>           Transparent border between sprites, default 2\n\
           --max-size <px>          Maximum atlas side, default 16384\n\
           --rotate | --no-rotate   Allow 90-degree packed sprites, default --no-rotate\n\
           --trim | --no-trim       Trim transparent borders, default --trim\n\
           --dry-run                Print proposed layout without writing files\n\
           --aseprite-json          Also write aseprite.json sidecar metadata\n\
           --godot-spriteframes     Also write godot_spriteframes.tres\n\
           --unity-importer         Also write SpriteAtlasPackerImporter.cs\n\
           --animation-regex <rx>   Custom grouping regex with name/index captures\n\
           --stable-pivot           Keep trimmed frames anchored to original canvas center\n\
           --dedup                  Alias exact duplicate frames to one packed rect\n\
           --dedup-threshold <n>    Alias near-duplicates within a per-channel 0-255 delta\n\
           --linked-channels <list> Write sibling sheets for suffixes like _n,_e,_m\n\
           --watch                  Repack when source images change\n\
           --serve                  Run local HTTP control API instead of packing\n\
           --host <host>            Control API host, default 127.0.0.1\n\
           --port <port>            Control API port, default 9877\n\
           --token <token>          Control API bearer token, generated by default\n\
           --slice grid|alpha       Reverse mode: slice an existing sheet\n\
           --cell <w>x<h>           Grid cell size for --slice grid\n\
           --prefix <name>          Frame filename prefix for reverse mode\n\
           --normalize-generated    Reverse mode: clean and stabilize generated sheets\n\
           --frame-size <w>x<h>     Normalized output frame size, default 256x256"
    );
}

fn validate_options(options: &PackOptions) -> Result<()> {
    if options.input.as_os_str().is_empty() {
        return Err(AtlasError::Message("--input is required".to_string()));
    }
    if options.output.as_os_str().is_empty() && !options.dry_run {
        return Err(AtlasError::Message("--output is required".to_string()));
    }
    if options.max_size == 0 {
        return Err(AtlasError::Message(
            "--max-size must be greater than zero".to_string(),
        ));
    }
    if !options.input.exists() {
        return Err(AtlasError::Message(format!(
            "Input path does not exist: {}",
            options.input.display()
        )));
    }
    if let Some(pattern) = options.animation_regex.as_deref() {
        let regex = Regex::new(pattern)
            .map_err(|error| AtlasError::Message(format!("Invalid animation regex: {error}")))?;
        let has_name =
            regex.capture_names().flatten().any(|name| name == "name") || regex.captures_len() > 1;
        if !has_name {
            return Err(AtlasError::Message(
                "Animation regex must capture a name group".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_animation_options(options: &PackOptions, sources: &[SourceSprite]) -> Result<()> {
    if options.animations.is_empty() {
        return Ok(());
    }
    let frame_names = sources
        .iter()
        .map(|source| source.name.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    for animation in &options.animations {
        if animation.name.trim().is_empty() {
            return Err(AtlasError::Message(
                "Project animation names must not be empty".to_string(),
            ));
        }
        if animation.frames.is_empty() {
            return Err(AtlasError::Message(format!(
                "Project animation '{}' has no frames",
                animation.name
            )));
        }
        if animation.fps_hint <= 0.0 {
            return Err(AtlasError::Message(format!(
                "Project animation '{}' fpsHint must be greater than zero",
                animation.name
            )));
        }
        if let Some(duration_ms) = animation.duration_ms {
            if duration_ms == 0 {
                return Err(AtlasError::Message(format!(
                    "Project animation '{}' durationMs must be greater than zero",
                    animation.name
                )));
            }
        }
        if let Some(pivot) = animation.pivot {
            if !valid_pivot(pivot) {
                return Err(AtlasError::Message(format!(
                    "Project animation '{}' pivot must be between 0 and 1",
                    animation.name
                )));
            }
        }
        for frame in &animation.frames {
            if !frame_names.contains(frame.as_str()) {
                return Err(AtlasError::Message(format!(
                    "Project animation '{}' references missing frame '{}'",
                    animation.name, frame
                )));
            }
        }
        for (frame, override_) in &animation.frame_overrides {
            if !animation.frames.iter().any(|name| name == frame) {
                return Err(AtlasError::Message(format!(
                    "Project animation '{}' override references frame '{}' outside the animation",
                    animation.name, frame
                )));
            }
            if !frame_names.contains(frame.as_str()) {
                return Err(AtlasError::Message(format!(
                    "Project animation '{}' override references missing frame '{}'",
                    animation.name, frame
                )));
            }
            if let Some(pivot) = override_.pivot {
                if !valid_pivot(pivot) {
                    return Err(AtlasError::Message(format!(
                        "Project animation '{}' override for '{}' pivot must be between 0 and 1",
                        animation.name, frame
                    )));
                }
            }
            if let Some(duration_ms) = override_.duration_ms {
                if duration_ms == 0 {
                    return Err(AtlasError::Message(format!(
                        "Project animation '{}' override for '{}' durationMs must be greater than zero",
                        animation.name, frame
                    )));
                }
            }
        }
    }
    Ok(())
}

fn valid_pivot(pivot: Pivot) -> bool {
    (0.0..=1.0).contains(&pivot.x) && (0.0..=1.0).contains(&pivot.y)
}

fn slice_atlas(
    sheet_name: &str,
    prefix: &str,
    sheet_width: u32,
    sheet_height: u32,
    rects: &[Rect],
    pivot: Pivot,
) -> Atlas {
    let frames = rects
        .iter()
        .enumerate()
        .map(|(index, rect)| {
            let filename = format!("{prefix}_{index:04}");
            Frame {
                filename,
                frame: json_rect(*rect),
                rotated: false,
                trimmed: false,
                sprite_source_size: JsonRect {
                    x: 0,
                    y: 0,
                    w: rect.w,
                    h: rect.h,
                },
                source_size: Size {
                    w: rect.w,
                    h: rect.h,
                },
                pivot,
                duration_ms: Some(DEFAULT_DURATION_MS),
            }
        })
        .collect::<Vec<_>>();
    Atlas {
        meta: Meta {
            app: APP_NAME.to_string(),
            version: VERSION.to_string(),
            image: sheet_name.to_string(),
            size: Size {
                w: sheet_width,
                h: sheet_height,
            },
            format: "RGBA8888".to_string(),
            scale: 1.0,
            trimmed: false,
        },
        animations: vec![Animation {
            name: prefix.to_string(),
            frames: frames.iter().map(|frame| frame.filename.clone()).collect(),
            looped: true,
            fps_hint: 1000.0 / DEFAULT_DURATION_MS as f32,
        }],
        frames,
    }
}

fn detect_slice_rects(
    image: &RgbaImage,
    mode: SliceMode,
    cell: Option<(u32, u32)>,
    normalize_generated: bool,
) -> Result<SliceDetection> {
    match mode {
        SliceMode::Grid if normalize_generated => detect_generated_grid_rects(image, cell),
        SliceMode::Grid => {
            let (cell_w, cell_h, grid_detection) = resolve_grid_cell(image, cell)?;
            Ok(SliceDetection {
                rects: grid_rects_from_cell(image, cell_w, cell_h),
                columns: Some(image.width() / cell_w),
                rows: Some(image.height() / cell_h),
                grid_detection,
            })
        }
        SliceMode::Alpha => Ok(SliceDetection {
            rects: detect_alpha_components(image),
            columns: None,
            rows: None,
            grid_detection: "alpha_components".to_string(),
        }),
    }
}

fn resolve_grid_cell(image: &RgbaImage, cell: Option<(u32, u32)>) -> Result<(u32, u32, String)> {
    let (cell_w, cell_h, detection) = match cell {
        Some((cell_w, cell_h)) => (cell_w, cell_h, "explicit_cell".to_string()),
        None => {
            let (cell_w, cell_h) = autodetect_grid_cell(image).ok_or_else(|| {
                AtlasError::Message(
                    "Unable to auto-detect grid; provide --cell <w>x<h>".to_string(),
                )
            })?;
            (cell_w, cell_h, "transparent_gutter_auto_cell".to_string())
        }
    };
    if cell_w == 0 || cell_h == 0 || cell_w > image.width() || cell_h > image.height() {
        return Err(AtlasError::Message(
            "Grid cell dimensions must fit inside the sheet".to_string(),
        ));
    }
    if image.width() / cell_w == 0 || image.height() / cell_h == 0 {
        return Err(AtlasError::Message(
            "Grid cell dimensions do not produce any source cells".to_string(),
        ));
    }
    Ok((cell_w, cell_h, detection))
}

fn grid_rects_from_cell(image: &RgbaImage, cell_w: u32, cell_h: u32) -> Vec<Rect> {
    let mut rects = Vec::new();
    let columns = image.width() / cell_w;
    let rows = image.height() / cell_h;
    for row in 0..rows {
        for column in 0..columns {
            let rect = Rect {
                x: column * cell_w,
                y: row * cell_h,
                w: cell_w,
                h: cell_h,
            };
            if rect_has_alpha(image, rect) {
                rects.push(rect);
            }
        }
    }
    rects
}

fn detect_generated_grid_rects(
    image: &RgbaImage,
    cell: Option<(u32, u32)>,
) -> Result<SliceDetection> {
    let (columns, rows, source_detection) = resolve_generated_grid(image, cell)?;
    let (x_bounds, y_bounds, projection_detection) = projected_grid_bounds(image, columns, rows);
    let rects = grid_rects_from_bounds(image, &x_bounds, &y_bounds);
    Ok(SliceDetection {
        rects,
        columns: Some(columns),
        rows: Some(rows),
        grid_detection: format!("{source_detection}:{projection_detection}"),
    })
}

fn resolve_generated_grid(
    image: &RgbaImage,
    cell: Option<(u32, u32)>,
) -> Result<(u32, u32, String)> {
    if let Some((cell_w, cell_h)) = cell {
        if cell_w == 0 || cell_h == 0 || cell_w > image.width() || cell_h > image.height() {
            return Err(AtlasError::Message(
                "Grid cell dimensions must fit inside the sheet".to_string(),
            ));
        }
        let columns = rounded_grid_count(image.width(), cell_w);
        let rows = rounded_grid_count(image.height(), cell_h);
        if columns == 0 || rows == 0 {
            return Err(AtlasError::Message(
                "Grid cell dimensions do not produce any source cells".to_string(),
            ));
        }
        let exact = image.width() % cell_w == 0 && image.height() % cell_h == 0;
        let detection = if exact {
            "explicit_cell_exact_grid"
        } else {
            "explicit_cell_approx_grid"
        };
        return Ok((columns, rows, detection.to_string()));
    }

    if let Some((columns, rows, detection)) = infer_generated_grid_size(image) {
        return Ok((columns, rows, detection));
    }

    if let Some((cell_w, cell_h)) = autodetect_grid_cell(image) {
        let columns = image.width() / cell_w;
        let rows = image.height() / cell_h;
        if columns > 0 && rows > 0 {
            return Ok((columns, rows, "transparent_gutter_auto_cell".to_string()));
        }
    }

    Err(AtlasError::Message(
        "Unable to infer generated grid; provide --cell <w>x<h> or use a common generated layout such as 4x4 or 4x3".to_string(),
    ))
}

fn rounded_grid_count(size: u32, cell: u32) -> u32 {
    if cell == 0 {
        return 0;
    }
    ((size as u64 + cell as u64 / 2) / cell as u64)
        .max(1)
        .min(u32::MAX as u64) as u32
}

fn infer_generated_grid_size(image: &RgbaImage) -> Option<(u32, u32, String)> {
    let width = image.width();
    let height = image.height();
    if width < 128 || height < 128 {
        return None;
    }
    let image_ratio = width as f32 / height as f32;
    let candidates = [
        (4, 4),
        (4, 3),
        (3, 4),
        (5, 4),
        (4, 5),
        (6, 4),
        (4, 6),
        (8, 4),
        (4, 8),
        (8, 2),
        (2, 8),
    ];
    candidates
        .iter()
        .map(|(columns, rows)| {
            let candidate_ratio = *columns as f32 / *rows as f32;
            let score = (image_ratio - candidate_ratio).abs() / candidate_ratio;
            (score, *columns, *rows)
        })
        .filter(|(score, _, _)| *score <= 0.18)
        .min_by(|left, right| left.0.partial_cmp(&right.0).unwrap_or(Ordering::Equal))
        .map(|(_, columns, rows)| {
            (
                columns,
                rows,
                format!("inferred_generated_{columns}x{rows}"),
            )
        })
}

fn grid_rects_from_bounds(image: &RgbaImage, x_bounds: &[u32], y_bounds: &[u32]) -> Vec<Rect> {
    let mut rects = Vec::new();
    if x_bounds.len() < 2 || y_bounds.len() < 2 {
        return rects;
    }
    for row in 0..y_bounds.len() - 1 {
        for column in 0..x_bounds.len() - 1 {
            let x0 = x_bounds[column].min(image.width());
            let x1 = x_bounds[column + 1].min(image.width());
            let y0 = y_bounds[row].min(image.height());
            let y1 = y_bounds[row + 1].min(image.height());
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            let rect = Rect {
                x: x0,
                y: y0,
                w: x1 - x0,
                h: y1 - y0,
            };
            if rect_has_visible_alpha(image, rect) {
                rects.push(rect);
            }
        }
    }
    rects
}

fn autodetect_grid_cell(image: &RgbaImage) -> Option<(u32, u32)> {
    let columns = transparent_columns(image);
    let rows = transparent_rows(image);
    let width = first_repeating_span(&columns, image.width())?;
    let height = first_repeating_span(&rows, image.height())?;
    Some((width, height))
}

fn transparent_columns(image: &RgbaImage) -> Vec<u32> {
    (0..image.width())
        .filter(|&x| (0..image.height()).all(|y| image.get_pixel(x, y)[3] == 0))
        .collect()
}

fn transparent_rows(image: &RgbaImage) -> Vec<u32> {
    (0..image.height())
        .filter(|&y| (0..image.width()).all(|x| image.get_pixel(x, y)[3] == 0))
        .collect()
}

fn first_repeating_span(gutters: &[u32], limit: u32) -> Option<u32> {
    let mut boundaries = vec![0];
    boundaries.extend(gutters.iter().copied());
    boundaries.push(limit);
    let spans = boundaries
        .windows(2)
        .filter_map(|window| {
            let span = window[1].saturating_sub(window[0]).saturating_sub(1);
            (span > 0).then_some(span)
        })
        .collect::<Vec<_>>();
    spans.into_iter().find(|span| *span > 1)
}

fn projected_grid_bounds(
    image: &RgbaImage,
    columns: u32,
    rows: u32,
) -> (Vec<u32>, Vec<u32>, String) {
    let x_counts = (0..image.width())
        .map(|x| {
            (0..image.height())
                .filter(|&y| image.get_pixel(x, y)[3] > 24)
                .count() as u32
        })
        .collect::<Vec<_>>();
    let y_counts = (0..image.height())
        .map(|y| {
            (0..image.width())
                .filter(|&x| image.get_pixel(x, y)[3] > 24)
                .count() as u32
        })
        .collect::<Vec<_>>();
    let x_runs = projection_runs(
        &x_counts,
        (image.height() * 6 / 1000).max(3),
        (image.width() / 160).max(6),
        (image.width() / 80).max(12),
    );
    let y_runs = projection_runs(
        &y_counts,
        (image.width() * 6 / 1000).max(3),
        (image.height() / 160).max(6),
        (image.height() / 80).max(12),
    );
    let x_bounds = boundaries_from_runs(&x_runs, columns, image.width());
    let y_bounds = boundaries_from_runs(&y_runs, rows, image.height());
    if let (Some(x_bounds), Some(y_bounds)) = (x_bounds, y_bounds) {
        return (x_bounds, y_bounds, "projection_auto_detected".to_string());
    }
    (
        proportional_bounds(image.width(), columns),
        proportional_bounds(image.height(), rows),
        "projection_proportional_fallback".to_string(),
    )
}

fn projection_runs(
    counts: &[u32],
    threshold: u32,
    merge_gap: u32,
    min_size: u32,
) -> Vec<(u32, u32)> {
    let mut runs = Vec::new();
    let mut start: Option<u32> = None;
    for (index, value) in counts.iter().enumerate() {
        let index = index as u32;
        if *value > threshold && start.is_none() {
            start = Some(index);
        } else if *value <= threshold {
            if let Some(run_start) = start.take() {
                runs.push((run_start, index));
            }
        }
    }
    if let Some(run_start) = start {
        runs.push((run_start, counts.len() as u32));
    }

    let mut merged: Vec<(u32, u32)> = Vec::new();
    for run in runs {
        if let Some(previous) = merged.last_mut() {
            if run.0.saturating_sub(previous.1) <= merge_gap {
                previous.1 = run.1;
                continue;
            }
        }
        merged.push(run);
    }
    merged
        .into_iter()
        .filter(|(start, end)| end.saturating_sub(*start) >= min_size)
        .collect()
}

fn boundaries_from_runs(runs: &[(u32, u32)], expected: u32, size: u32) -> Option<Vec<u32>> {
    if runs.len() != expected as usize || expected == 0 {
        return None;
    }
    if expected == 1 {
        return Some(vec![0, size]);
    }
    let centers = runs
        .iter()
        .map(|(start, end)| (*start as f32 + *end as f32) / 2.0)
        .collect::<Vec<_>>();
    let distances = centers
        .windows(2)
        .map(|window| window[1] - window[0])
        .collect::<Vec<_>>();
    let median_distance = median_f32(&distances)?;
    if median_distance <= 0.0 {
        return None;
    }
    let mut bounds = vec![(centers[0] - median_distance / 2.0).round() as i64];
    for window in centers.windows(2) {
        bounds.push(((window[0] + window[1]) / 2.0).round() as i64);
    }
    bounds.push((centers[centers.len() - 1] + median_distance / 2.0).round() as i64);
    if let Some(first) = bounds.first_mut() {
        *first = (*first).clamp(0, size as i64);
    }
    if let Some(last) = bounds.last_mut() {
        *last = (*last).clamp(0, size as i64);
    }
    if bounds.windows(2).any(|window| window[1] <= window[0]) {
        return None;
    }
    Some(bounds.into_iter().map(|value| value as u32).collect())
}

fn median_f32(values: &[f32]) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    let mut values = values.to_vec();
    values.sort_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
    let middle = values.len() / 2;
    if values.len() % 2 == 0 {
        Some((values[middle - 1] + values[middle]) / 2.0)
    } else {
        Some(values[middle])
    }
}

fn proportional_bounds(size: u32, count: u32) -> Vec<u32> {
    (0..=count)
        .map(|index| ((index as u64 * size as u64 + count as u64 / 2) / count as u64) as u32)
        .collect()
}

fn detect_alpha_components(image: &RgbaImage) -> Vec<Rect> {
    let width = image.width();
    let height = image.height();
    let mut visited = vec![false; width as usize * height as usize];
    let mut rects = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let index = pixel_index(width, x, y);
            if visited[index] || image.get_pixel(x, y)[3] == 0 {
                visited[index] = true;
                continue;
            }
            let rect = flood_component(image, &mut visited, x, y);
            rects.push(rect);
        }
    }
    rects.sort_by(|left, right| left.y.cmp(&right.y).then_with(|| left.x.cmp(&right.x)));
    rects
}

fn flood_component(image: &RgbaImage, visited: &mut [bool], start_x: u32, start_y: u32) -> Rect {
    let width = image.width();
    let height = image.height();
    let mut stack = vec![(start_x, start_y)];
    let mut min_x = start_x;
    let mut min_y = start_y;
    let mut max_x = start_x;
    let mut max_y = start_y;
    while let Some((x, y)) = stack.pop() {
        let index = pixel_index(width, x, y);
        if visited[index] {
            continue;
        }
        visited[index] = true;
        if image.get_pixel(x, y)[3] == 0 {
            continue;
        }
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
        if x > 0 {
            stack.push((x - 1, y));
        }
        if y > 0 {
            stack.push((x, y - 1));
        }
        if x + 1 < width {
            stack.push((x + 1, y));
        }
        if y + 1 < height {
            stack.push((x, y + 1));
        }
    }
    Rect {
        x: min_x,
        y: min_y,
        w: max_x - min_x + 1,
        h: max_y - min_y + 1,
    }
}

fn pixel_index(width: u32, x: u32, y: u32) -> usize {
    (y as usize * width as usize) + x as usize
}

fn rect_has_alpha(image: &RgbaImage, rect: Rect) -> bool {
    for y in rect.y..rect.y + rect.h {
        for x in rect.x..rect.x + rect.w {
            if image.get_pixel(x, y)[3] != 0 {
                return true;
            }
        }
    }
    false
}

fn rect_has_visible_alpha(image: &RgbaImage, rect: Rect) -> bool {
    for y in rect.y..rect.y + rect.h {
        for x in rect.x..rect.x + rect.w {
            if image.get_pixel(x, y)[3] > 24 {
                return true;
            }
        }
    }
    false
}

fn normalize_generated_slice(
    image: &RgbaImage,
    detection: &SliceDetection,
    options: &PackOptions,
) -> Result<NormalizedSlice> {
    let (frame_w, frame_h) = normalized_frame_size(options.frame_size, &detection.rects)?;
    let columns = detection
        .columns
        .filter(|columns| *columns > 0)
        .unwrap_or_else(|| ceil_sqrt_u32(detection.rects.len() as u32).max(1));
    let rows = ceil_div_u32(detection.rects.len() as u32, columns).max(1);
    let sheet_w = frame_w
        .checked_mul(columns)
        .ok_or_else(|| AtlasError::Message("Normalized sheet width would overflow".to_string()))?;
    let sheet_h = frame_h
        .checked_mul(rows)
        .ok_or_else(|| AtlasError::Message("Normalized sheet height would overflow".to_string()))?;

    let mut cells = Vec::new();
    let mut boxes = Vec::new();
    let mut anchors = Vec::new();
    let mut source_cells = Vec::new();
    for rect in &detection.rects {
        let source_rect = inset_rect(*rect);
        let mut cell = image
            .view(source_rect.x, source_rect.y, source_rect.w, source_rect.h)
            .to_image();
        remove_border_connected_generated_matte(&mut cell);
        decontaminate_transparent_edges(&mut cell);
        let bbox = alpha_bbox(&cell);
        let anchor = bbox.map(|bbox| upper_body_anchor(&cell, bbox));
        cells.push(cell);
        boxes.push(bbox);
        anchors.push(anchor);
        source_cells.push(json_rect(source_rect));
    }

    let valid_boxes = boxes.iter().flatten().copied().collect::<Vec<_>>();
    if valid_boxes.is_empty() {
        return Err(AtlasError::Message(
            "No visible sprite pixels found after generated-sheet cleanup".to_string(),
        ));
    }
    let max_w = valid_boxes.iter().map(|rect| rect.w).max().unwrap_or(1);
    let max_h = valid_boxes.iter().map(|rect| rect.h).max().unwrap_or(1);
    let usable_w = frame_w.saturating_sub(12).max(1);
    let usable_h = frame_h.saturating_sub(12).max(1);
    let scale = (usable_w as f32 / max_w.max(1) as f32)
        .min(usable_h as f32 / max_h.max(1) as f32)
        .max(0.001);
    let target_anchor = (frame_w as f32 / 2.0, frame_h as f32 * 0.305);
    let mut sheet = RgbaImage::from_pixel(sheet_w, sheet_h, Rgba([0, 0, 0, 0]));
    let mut frame_rects = Vec::new();
    let mut frame_bboxes = Vec::new();
    let mut root_anchors = Vec::new();

    for (index, ((cell, bbox), anchor)) in cells
        .iter()
        .zip(boxes.iter())
        .zip(anchors.iter())
        .enumerate()
    {
        let column = index as u32 % columns;
        let row = index as u32 / columns;
        let frame_rect = Rect {
            x: column * frame_w,
            y: row * frame_h,
            w: frame_w,
            h: frame_h,
        };
        frame_rects.push(frame_rect);
        let Some(bbox) = bbox else {
            frame_bboxes.push(None);
            root_anchors.push(None);
            continue;
        };
        let Some(anchor) = anchor else {
            frame_bboxes.push(None);
            root_anchors.push(None);
            continue;
        };

        let sprite = cell.view(bbox.x, bbox.y, bbox.w, bbox.h).to_image();
        let scaled_w = ((bbox.w as f32 * scale).round() as u32).max(1);
        let scaled_h = ((bbox.h as f32 * scale).round() as u32).max(1);
        let sprite = resize(&sprite, scaled_w, scaled_h, FilterType::Lanczos3);
        let anchor_x = (anchor.0 - bbox.x as f32) * scale;
        let anchor_y = (anchor.1 - bbox.y as f32) * scale;
        let mut local_x = (target_anchor.0 - anchor_x).round() as i64;
        let mut local_y = (target_anchor.1 - anchor_y).round() as i64;
        if scaled_w <= frame_w.saturating_sub(8) {
            local_x = local_x.clamp(4, frame_w.saturating_sub(scaled_w + 4) as i64);
        } else {
            local_x = (frame_w as i64 - scaled_w as i64) / 2;
        }
        if scaled_h <= frame_h.saturating_sub(8) {
            local_y = local_y.clamp(4, frame_h.saturating_sub(scaled_h + 4) as i64);
        } else {
            local_y = (frame_h as i64 - scaled_h as i64) / 2;
        }
        let paste_x = frame_rect.x as i64 + local_x;
        let paste_y = frame_rect.y as i64 + local_y;
        overlay(&mut sheet, &sprite, paste_x, paste_y);
        frame_bboxes.push(Some(JsonRect {
            x: local_x.max(0) as u32,
            y: local_y.max(0) as u32,
            w: scaled_w,
            h: scaled_h,
        }));
        root_anchors.push(Some(Pivot {
            x: target_anchor.0 / frame_w as f32,
            y: target_anchor.1 / frame_h as f32,
        }));
    }

    decontaminate_transparent_edges(&mut sheet);
    decontaminate_transparent_edges(&mut sheet);
    let continuity = generated_loop_continuity_report(&sheet, &frame_rects);

    Ok(NormalizedSlice {
        sheet,
        frame_rects,
        report: SliceNormalizationReport {
            source_sheet_size: Size {
                w: image.width(),
                h: image.height(),
            },
            output_sheet_size: Size {
                w: sheet_w,
                h: sheet_h,
            },
            output_frame_size: Size {
                w: frame_w,
                h: frame_h,
            },
            source_columns: detection.columns,
            source_rows: detection.rows,
            source_cells,
            frame_bboxes,
            root_anchors,
            scale,
            grid_policy: GENERATED_GRID_POLICY.to_string(),
            grid_detection: detection.grid_detection.clone(),
            proportion_policy: GENERATED_PROPORTION_POLICY.to_string(),
            root_anchor_policy: GENERATED_ROOT_ANCHOR_POLICY.to_string(),
            continuity,
        },
    })
}

fn normalized_frame_size(frame_size: Option<(u32, u32)>, rects: &[Rect]) -> Result<(u32, u32)> {
    let (width, height) = frame_size.unwrap_or_else(|| {
        let width = rects.iter().map(|rect| rect.w).max().unwrap_or(256);
        let height = rects.iter().map(|rect| rect.h).max().unwrap_or(256);
        (width.max(1), height.max(1))
    });
    if width < 16 || height < 16 {
        return Err(AtlasError::Message(
            "Normalized frame size must be at least 16x16".to_string(),
        ));
    }
    Ok((width, height))
}

fn normalized_frame_size_option(width: u32, height: u32) -> Option<(u32, u32)> {
    (width > 0 && height > 0).then_some((width, height))
}

fn inset_rect(rect: Rect) -> Rect {
    let inset = ((rect.w.min(rect.h) as f32 * 0.01).round() as u32)
        .max(1)
        .min(rect.w.saturating_sub(1) / 2)
        .min(rect.h.saturating_sub(1) / 2);
    Rect {
        x: rect.x + inset,
        y: rect.y + inset,
        w: rect.w.saturating_sub(inset * 2).max(1),
        h: rect.h.saturating_sub(inset * 2).max(1),
    }
}

fn alpha_bbox(image: &RgbaImage) -> Option<Rect> {
    let mut min_x = image.width();
    let mut min_y = image.height();
    let mut max_x = 0;
    let mut max_y = 0;
    let mut found = false;
    for (x, y, pixel) in image.enumerate_pixels() {
        if pixel[3] > 24 {
            found = true;
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
    }
    found.then_some(Rect {
        x: min_x,
        y: min_y,
        w: max_x - min_x + 1,
        h: max_y - min_y + 1,
    })
}

fn upper_body_anchor(image: &RgbaImage, bbox: Rect) -> (f32, f32) {
    let upper_y1 = bbox.y + ((bbox.h as f32 * 0.58).round() as u32).max(1);
    let mut xs = 0.0;
    let mut ys = 0.0;
    let mut total = 0.0;
    for y in bbox.y..upper_y1.min(bbox.y + bbox.h) {
        for x in bbox.x..bbox.x + bbox.w {
            let alpha = image.get_pixel(x, y)[3] as f32;
            if alpha > 24.0 {
                xs += x as f32 * alpha;
                ys += y as f32 * alpha;
                total += alpha;
            }
        }
    }
    if total > 0.0 {
        (xs / total, ys / total)
    } else {
        (
            bbox.x as f32 + bbox.w as f32 / 2.0,
            bbox.y as f32 + bbox.h as f32 * 0.32,
        )
    }
}

fn ceil_div_u32(value: u32, divisor: u32) -> u32 {
    if divisor == 0 {
        0
    } else {
        value.div_ceil(divisor)
    }
}

fn ceil_sqrt_u32(value: u32) -> u32 {
    if value <= 1 {
        return value;
    }
    let mut side = 1;
    while side * side < value {
        side += 1;
    }
    side
}

fn generated_loop_columns(frame_count: usize, requested: Option<u32>) -> Result<u32> {
    if frame_count == 0 {
        return Err(AtlasError::Message(
            "Generated loop must contain at least one frame".to_string(),
        ));
    }
    if let Some(columns) = requested.filter(|columns| *columns > 0) {
        if columns as usize > frame_count {
            return Err(AtlasError::Message(
                "Generated loop column count cannot exceed frame count".to_string(),
            ));
        }
        return Ok(columns);
    }
    Ok(if frame_count <= 4 {
        frame_count as u32
    } else {
        4
    })
}

fn generate_animation_job_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("anim-{}-{nanos:x}", std::process::id())
}

fn animation_job_plan_path(job_dir: &Path) -> PathBuf {
    job_dir.join("animation_job.json")
}

fn read_animation_job(path: &Path) -> Result<AnimationJob> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

fn write_animation_job(path: &Path, job: &AnimationJob) -> Result<()> {
    write_text_if_changed(path, &format!("{}\n", serde_json::to_string_pretty(job)?))
}

#[derive(Debug, Clone)]
struct AnimationPoseSpec {
    pose: &'static str,
    foot_contacts: &'static [&'static str],
    body_height: &'static str,
}

fn cat_walk_frame_plan(
    frame_count: usize,
    frame_size: Size,
    pose_guides_dir: &Path,
) -> Result<Vec<AnimationFramePlan>> {
    let specs = cat_walk_specs(frame_count)?;
    let mut frames = Vec::new();
    for (index, spec) in specs.iter().enumerate() {
        let filename = format!("{index:03}_{}.png", slugify(spec.pose));
        let guide_path = pose_guides_dir.join(format!("{index:03}_{}.svg", slugify(spec.pose)));
        write_cat_pose_guide(&guide_path, frame_size, index, frame_count, spec)?;
        frames.push(AnimationFramePlan {
            index,
            filename,
            pose: spec.pose.to_string(),
            phase: index as f32 / frame_count as f32,
            foot_contacts: spec
                .foot_contacts
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
            body_height: spec.body_height.to_string(),
            pose_guide_path: guide_path.to_string_lossy().to_string(),
            prompt: cat_walk_frame_prompt(index, frame_count, spec, &guide_path),
        });
    }
    Ok(frames)
}

fn cat_walk_specs(frame_count: usize) -> Result<Vec<AnimationPoseSpec>> {
    let specs = match frame_count {
        8 => vec![
            AnimationPoseSpec {
                pose: "contact_left_front_right_hind",
                foot_contacts: &["left_front", "right_hind"],
                body_height: "neutral",
            },
            AnimationPoseSpec {
                pose: "down_weight_transfer",
                foot_contacts: &["left_front", "right_hind", "left_hind"],
                body_height: "low",
            },
            AnimationPoseSpec {
                pose: "passing_left_front_lift",
                foot_contacts: &["right_front", "left_hind"],
                body_height: "neutral",
            },
            AnimationPoseSpec {
                pose: "up_reach_before_opposite_contact",
                foot_contacts: &["right_front", "left_hind"],
                body_height: "high",
            },
            AnimationPoseSpec {
                pose: "opposite_contact_right_front_left_hind",
                foot_contacts: &["right_front", "left_hind"],
                body_height: "neutral",
            },
            AnimationPoseSpec {
                pose: "opposite_down_weight_transfer",
                foot_contacts: &["right_front", "left_hind", "right_hind"],
                body_height: "low",
            },
            AnimationPoseSpec {
                pose: "opposite_passing_right_front_lift",
                foot_contacts: &["left_front", "right_hind"],
                body_height: "neutral",
            },
            AnimationPoseSpec {
                pose: "final_inbetween_before_contact",
                foot_contacts: &["left_front", "right_hind"],
                body_height: "high",
            },
        ],
        16 => vec![
            AnimationPoseSpec {
                pose: "contact_left_front_right_hind",
                foot_contacts: &["left_front", "right_hind"],
                body_height: "neutral",
            },
            AnimationPoseSpec {
                pose: "inbetween_settle_into_contact",
                foot_contacts: &["left_front", "right_hind"],
                body_height: "slightly_low",
            },
            AnimationPoseSpec {
                pose: "down_weight_transfer",
                foot_contacts: &["left_front", "right_hind", "left_hind"],
                body_height: "low",
            },
            AnimationPoseSpec {
                pose: "inbetween_left_front_lift",
                foot_contacts: &["right_front", "left_hind", "right_hind"],
                body_height: "slightly_low",
            },
            AnimationPoseSpec {
                pose: "passing_left_front_lift",
                foot_contacts: &["right_front", "left_hind"],
                body_height: "neutral",
            },
            AnimationPoseSpec {
                pose: "inbetween_reach_forward",
                foot_contacts: &["right_front", "left_hind"],
                body_height: "slightly_high",
            },
            AnimationPoseSpec {
                pose: "up_reach_before_opposite_contact",
                foot_contacts: &["right_front", "left_hind"],
                body_height: "high",
            },
            AnimationPoseSpec {
                pose: "inbetween_pre_opposite_contact",
                foot_contacts: &["right_front", "left_hind"],
                body_height: "slightly_high",
            },
            AnimationPoseSpec {
                pose: "opposite_contact_right_front_left_hind",
                foot_contacts: &["right_front", "left_hind"],
                body_height: "neutral",
            },
            AnimationPoseSpec {
                pose: "opposite_inbetween_settle",
                foot_contacts: &["right_front", "left_hind"],
                body_height: "slightly_low",
            },
            AnimationPoseSpec {
                pose: "opposite_down_weight_transfer",
                foot_contacts: &["right_front", "left_hind", "right_hind"],
                body_height: "low",
            },
            AnimationPoseSpec {
                pose: "opposite_inbetween_right_front_lift",
                foot_contacts: &["left_front", "right_hind", "left_hind"],
                body_height: "slightly_low",
            },
            AnimationPoseSpec {
                pose: "opposite_passing_right_front_lift",
                foot_contacts: &["left_front", "right_hind"],
                body_height: "neutral",
            },
            AnimationPoseSpec {
                pose: "opposite_inbetween_reach_forward",
                foot_contacts: &["left_front", "right_hind"],
                body_height: "slightly_high",
            },
            AnimationPoseSpec {
                pose: "opposite_up_reach_before_contact",
                foot_contacts: &["left_front", "right_hind"],
                body_height: "high",
            },
            AnimationPoseSpec {
                pose: "final_inbetween_before_contact",
                foot_contacts: &["left_front", "right_hind"],
                body_height: "slightly_high",
            },
        ],
        _ => {
            return Err(AtlasError::Message(
                "Cat walk pose guides support 8 or 16 frames".to_string(),
            ))
        }
    };
    Ok(specs)
}

fn cat_walk_frame_prompt(
    index: usize,
    frame_count: usize,
    spec: &AnimationPoseSpec,
    guide_path: &Path,
) -> String {
    format!(
        "Generate frame {}/{} for a side-view cat walk cycle. Pose: {}. Paw contacts: {}. Body height: {}. Use the locked cat reference plus pose guide {}. Keep identical character identity, scale, body length, head size, tail style, palette, side-view camera, and full-body framing. Plant the listed front/hind paws on one stable invisible ground line with readable paw spacing; keep the upper-body/root anchor locked while only legs, paws, tail, and subtle body height change. Transparent background preferred; if a separator/matte is needed use thin dark neutral gray #2b2f36. This is one frame of a loop; do not duplicate the first frame unless explicitly requested.",
        index + 1,
        frame_count,
        spec.pose,
        spec.foot_contacts.join(", "),
        spec.body_height,
        guide_path.display()
    )
}

fn write_cat_pose_guide(
    path: &Path,
    size: Size,
    index: usize,
    frame_count: usize,
    spec: &AnimationPoseSpec,
) -> Result<()> {
    let w = size.w.max(128) as f32;
    let h = size.h.max(128) as f32;
    let phase = index as f32 / frame_count as f32 * std::f32::consts::TAU;
    let body_y_offset = match spec.body_height {
        "low" => h * 0.035,
        "slightly_low" => h * 0.018,
        "high" => -h * 0.028,
        "slightly_high" => -h * 0.014,
        _ => 0.0,
    };
    let ground_y = h * 0.78;
    let body_cx = w * 0.50;
    let body_cy = h * 0.49 + body_y_offset;
    let body_w = w * 0.46;
    let body_h = h * 0.18;
    let head_cx = body_cx + body_w * 0.42;
    let head_cy = body_cy - body_h * 0.22;
    let stride = w * 0.075;
    let lift = h * 0.09;
    let legs = [
        (
            "left_hind",
            body_cx - body_w * 0.30,
            phase + std::f32::consts::FRAC_PI_2,
        ),
        (
            "right_hind",
            body_cx - body_w * 0.12,
            phase + std::f32::consts::PI * 1.5,
        ),
        ("left_front", body_cx + body_w * 0.18, phase),
        (
            "right_front",
            body_cx + body_w * 0.34,
            phase + std::f32::consts::PI,
        ),
    ];
    let mut leg_svg = String::new();
    for (name, hip_x, leg_phase) in legs {
        let swing = leg_phase.sin();
        let contact = spec.foot_contacts.iter().any(|contact| *contact == name);
        let paw_x = hip_x + swing * stride;
        let paw_y = if contact {
            ground_y
        } else {
            ground_y - swing.max(0.0).abs() * lift - h * 0.025
        };
        let color = if contact { "#0f766e" } else { "#6b7280" };
        leg_svg.push_str(&format!(
            r#"<line x1="{hip_x:.1}" y1="{:.1}" x2="{paw_x:.1}" y2="{paw_y:.1}" stroke="{color}" stroke-width="3" stroke-linecap="round"/><circle cx="{paw_x:.1}" cy="{paw_y:.1}" r="4" fill="{color}"><title>{name}</title></circle>"#,
            body_cy + body_h * 0.42
        ));
    }
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.0}" height="{h:.0}" viewBox="0 0 {w:.0} {h:.0}">
<rect width="100%" height="100%" fill="#f8fafc"/>
<line x1="{:.1}" y1="{ground_y:.1}" x2="{:.1}" y2="{ground_y:.1}" stroke="#94a3b8" stroke-width="2" stroke-dasharray="6 6"/>
<ellipse cx="{body_cx:.1}" cy="{body_cy:.1}" rx="{:.1}" ry="{:.1}" fill="none" stroke="#111827" stroke-width="4"/>
<circle cx="{head_cx:.1}" cy="{head_cy:.1}" r="{:.1}" fill="none" stroke="#111827" stroke-width="4"/>
<path d="M {:.1} {:.1} C {:.1} {:.1}, {:.1} {:.1}, {:.1} {:.1}" fill="none" stroke="#111827" stroke-width="4" stroke-linecap="round"/>
{leg_svg}
<text x="12" y="22" font-family="Arial" font-size="14" fill="#111827">{}</text>
<text x="12" y="42" font-family="Arial" font-size="12" fill="#475569">contacts: {}</text>
</svg>
"##,
        w * 0.08,
        w * 0.92,
        body_w / 2.0,
        body_h / 2.0,
        body_h * 0.62,
        body_cx - body_w * 0.48,
        body_cy - body_h * 0.1,
        body_cx - body_w * 0.65,
        body_cy - body_h * 0.45,
        body_cx - body_w * 0.72,
        body_cy + body_h * 0.15,
        body_cx - body_w * 0.56,
        body_cy + body_h * 0.05,
        spec.pose,
        spec.foot_contacts.join(", ")
    );
    write_text_if_changed(path, &svg)
}

fn analyze_animation_frame(path: &Path) -> Result<AnimationJobFrameAnalysis> {
    let source = decode_image(path)?;
    let cleaned = remove_generation_chroma_key(&source);
    let bbox = alpha_bbox(&cleaned);
    let visible_pixels = cleaned.pixels().filter(|pixel| pixel[3] > 24).count() as u64;
    let green_edge_pixels = count_green_edge_pixels(&cleaned);
    let style = bbox.and_then(|bbox| foreground_style_stats(&cleaned, bbox, visible_pixels));
    let gait = bbox.and_then(|bbox| foreground_gait_stats(&cleaned, bbox, visible_pixels));
    let bbox_center = bbox.map(|bbox| Pivot {
        x: (bbox.x as f32 + bbox.w as f32 / 2.0) / cleaned.width().max(1) as f32,
        y: (bbox.y as f32 + bbox.h as f32 / 2.0) / cleaned.height().max(1) as f32,
    });
    let bbox_size = bbox.map(|bbox| Size {
        w: bbox.w,
        h: bbox.h,
    });
    Ok(AnimationJobFrameAnalysis {
        source_size: Size {
            w: source.width(),
            h: source.height(),
        },
        bbox: bbox.map(json_rect),
        bbox_center,
        bbox_size,
        visible_pixels,
        green_edge_pixels,
        style,
        gait,
    })
}

fn foreground_style_stats(
    image: &RgbaImage,
    bbox: Rect,
    visible_pixels: u64,
) -> Option<AnimationJobStyleStats> {
    if visible_pixels == 0 {
        return None;
    }
    let mut total = 0.0_f32;
    let mut r_sum = 0.0_f32;
    let mut g_sum = 0.0_f32;
    let mut b_sum = 0.0_f32;
    let mut saturation_sum = 0.0_f32;
    let mut orange = 0.0_f32;
    let mut cream = 0.0_f32;
    let mut dark = 0.0_f32;
    let x_end = bbox.x.saturating_add(bbox.w).min(image.width());
    let y_end = bbox.y.saturating_add(bbox.h).min(image.height());
    for y in bbox.y..y_end {
        for x in bbox.x..x_end {
            let pixel = image.get_pixel(x, y);
            let [r, g, b, a] = pixel.0;
            if a <= 24 {
                continue;
            }
            let r_f = r as f32;
            let g_f = g as f32;
            let b_f = b as f32;
            let max_channel = r.max(g).max(b);
            let min_channel = r.min(g).min(b);
            r_sum += r_f;
            g_sum += g_f;
            b_sum += b_f;
            if max_channel > 0 {
                saturation_sum +=
                    (max_channel.saturating_sub(min_channel) as f32) / max_channel as f32;
            }
            if r_f > g_f * 1.08 && g_f > b_f * 1.08 && r > 110 {
                orange += 1.0;
            }
            if r > 180 && g > 145 && b > 100 && r.abs_diff(g) < 80 {
                cream += 1.0;
            }
            if max_channel < 85 {
                dark += 1.0;
            }
            total += 1.0;
        }
    }
    if total <= 0.0 {
        return None;
    }
    let bbox_area = (bbox.w as f32 * bbox.h as f32).max(1.0);
    Some(AnimationJobStyleStats {
        mean_r: r_sum / total,
        mean_g: g_sum / total,
        mean_b: b_sum / total,
        mean_saturation: saturation_sum / total,
        orange_ratio: orange / total,
        cream_ratio: cream / total,
        dark_ratio: dark / total,
        silhouette_density: total / bbox_area,
    })
}

fn foreground_gait_stats(
    image: &RgbaImage,
    bbox: Rect,
    visible_pixels: u64,
) -> Option<AnimationJobGaitStats> {
    if visible_pixels == 0 || bbox.w == 0 || bbox.h == 0 {
        return None;
    }
    let (root_x, root_y) = upper_body_anchor(image, bbox);
    let width = image.width().max(1) as f32;
    let height = image.height().max(1) as f32;
    let ground_y = bbox.y.saturating_add(bbox.h) as f32 / height;

    let contact_start_y = bbox.y + ((bbox.h as f32 * 0.86).round() as u32).min(bbox.h);
    let contact_end_y = bbox.y.saturating_add(bbox.h).min(image.height());
    if contact_start_y >= contact_end_y {
        return Some(AnimationJobGaitStats {
            root_anchor: Pivot {
                x: root_x / width,
                y: root_y / height,
            },
            ground_y,
            contact_count: 0,
            stance_width: 0.0,
            contact_center_x: 0.5,
            rear_contact_ratio: 0.0,
            front_contact_ratio: 0.0,
            lower_body_ratio: 0.0,
            contact_points: Vec::new(),
        });
    }

    let mut counts = vec![0_u32; bbox.w as usize];
    let mut lower_pixels = 0.0_f32;
    let mut rear_pixels = 0.0_f32;
    let mut front_pixels = 0.0_f32;
    let mut weighted_x = 0.0_f32;
    let mid_x = bbox.x as f32 + bbox.w as f32 * 0.5;
    let x_end = bbox.x.saturating_add(bbox.w).min(image.width());
    for y in contact_start_y..contact_end_y {
        for x in bbox.x..x_end {
            if image.get_pixel(x, y)[3] <= 24 {
                continue;
            }
            let local_x = x.saturating_sub(bbox.x) as usize;
            if let Some(count) = counts.get_mut(local_x) {
                *count += 1;
            }
            lower_pixels += 1.0;
            weighted_x += (x as f32 - bbox.x as f32) / bbox.w as f32;
            if x as f32 <= mid_x {
                rear_pixels += 1.0;
            } else {
                front_pixels += 1.0;
            }
        }
    }

    let band_h = contact_end_y.saturating_sub(contact_start_y).max(1);
    let runs = projection_runs(
        &counts,
        (band_h / 22).max(1),
        (bbox.w / 70).max(2),
        (bbox.w / 80).max(3),
    );
    let mut contact_points = Vec::new();
    for (start, end) in runs {
        let mut px = 0.0_f32;
        let mut py = 0.0_f32;
        let mut total = 0.0_f32;
        let run_x0 = bbox.x.saturating_add(start);
        let run_x1 = bbox.x.saturating_add(end).min(x_end);
        for y in contact_start_y..contact_end_y {
            for x in run_x0..run_x1 {
                let alpha = image.get_pixel(x, y)[3] as f32;
                if alpha <= 24.0 {
                    continue;
                }
                px += ((x as f32 - bbox.x as f32) / bbox.w as f32) * alpha;
                py += ((y as f32 - bbox.y as f32) / bbox.h as f32) * alpha;
                total += alpha;
            }
        }
        if total > 0.0 {
            contact_points.push(Pivot {
                x: px / total,
                y: py / total,
            });
        }
    }
    contact_points.sort_by(|left, right| {
        left.x
            .partial_cmp(&right.x)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let stance_width = match (contact_points.first(), contact_points.last()) {
        (Some(first), Some(last)) if contact_points.len() >= 2 => (last.x - first.x).max(0.0),
        _ => 0.0,
    };
    let lower_body_ratio = lower_pixels / visible_pixels.max(1) as f32;
    let contact_center_x = if lower_pixels > 0.0 {
        weighted_x / lower_pixels
    } else {
        0.5
    };
    Some(AnimationJobGaitStats {
        root_anchor: Pivot {
            x: root_x / width,
            y: root_y / height,
        },
        ground_y,
        contact_count: contact_points.len(),
        stance_width,
        contact_center_x,
        rear_contact_ratio: rear_pixels / lower_pixels.max(1.0),
        front_contact_ratio: front_pixels / lower_pixels.max(1.0),
        lower_body_ratio,
        contact_points,
    })
}

fn accepted_frame_analyses(
    job: &AnimationJob,
    skip_index: Option<usize>,
) -> Result<Vec<AnimationJobFrameAnalysis>> {
    let frames_dir = PathBuf::from(&job.frames_dir);
    let mut analyses = Vec::new();
    for frame in &job.frames {
        if Some(frame.index) == skip_index {
            continue;
        }
        let path = frames_dir.join(&frame.filename);
        if path.exists() {
            analyses.push(analyze_animation_frame(&path)?);
        }
    }
    Ok(analyses)
}

fn validate_animation_frame(
    job: &AnimationJob,
    frame_plan: &AnimationFramePlan,
    analysis: &AnimationJobFrameAnalysis,
    accepted: &[AnimationJobFrameAnalysis],
) -> (Vec<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    if analysis.bbox.is_none() || analysis.visible_pixels == 0 {
        errors.push("No visible sprite pixels after matte cleanup".to_string());
        return (errors, warnings);
    }
    if analysis.source_size.w < 64 || analysis.source_size.h < 64 {
        warnings
            .push("Frame is very small; image generation should use a larger canvas".to_string());
    }
    if analysis.green_edge_pixels > 64 {
        warnings.push(format!(
            "{} green-dominant edge pixels remain after cleanup; prefer transparent or dark-gray matte",
            analysis.green_edge_pixels
        ));
    }
    if let Some(bbox) = analysis.bbox {
        if bbox.x <= 1
            || bbox.y <= 1
            || bbox.x + bbox.w + 1 >= analysis.source_size.w
            || bbox.y + bbox.h + 1 >= analysis.source_size.h
        {
            warnings.push(
                "Visible sprite touches the canvas edge; regenerate with more transparent padding"
                    .to_string(),
            );
        }
    }
    if let Some(size) = analysis.bbox_size {
        let frame_area = (analysis.source_size.w as u64 * analysis.source_size.h as u64).max(1);
        let bbox_area = size.w as u64 * size.h as u64;
        if bbox_area * 100 < frame_area * 8 {
            warnings.push("Sprite is tiny inside the frame; preserve full-body scale".to_string());
        }
        if bbox_area * 100 > frame_area * 92 {
            warnings.push(
                "Sprite nearly fills the frame; leave room for paws and tail motion".to_string(),
            );
        }
    }
    if let Some((median_w, median_h, median_pixels, median_center)) =
        median_accepted_frame_stats(accepted)
    {
        if let Some(size) = analysis.bbox_size {
            if relative_delta(size.w as f32, median_w) > 0.35 {
                errors.push(format!(
                    "Body width changed too much for {}: expected near {:.1}px, got {}px",
                    frame_plan.pose, median_w, size.w
                ));
            }
            if relative_delta(size.h as f32, median_h) > 0.28 {
                errors.push(format!(
                    "Body height changed too much for {}: expected near {:.1}px, got {}px",
                    frame_plan.pose, median_h, size.h
                ));
            }
        }
        if median_pixels > 0.0 {
            let pixel_delta = relative_delta(analysis.visible_pixels as f32, median_pixels);
            if pixel_delta > 0.55 {
                errors.push(format!(
                    "Visible sprite area changed too much for {}",
                    frame_plan.pose
                ));
            }
        }
        if let (Some(center), Some(median_center)) = (analysis.bbox_center, median_center) {
            if (center.x - median_center.x).abs() > 0.22
                || (center.y - median_center.y).abs() > 0.22
            {
                errors.push(format!(
                    "Sprite center drifted too far for {}; keep root/camera locked",
                    frame_plan.pose
                ));
            }
        }
    }
    if let (Some(style), Some(median_style)) =
        (analysis.style, median_accepted_style_stats(accepted))
    {
        if relative_delta(style.mean_saturation, median_style.mean_saturation) > 0.14 {
            errors.push(format!(
                "Foreground saturation drifted too far for {}; regenerate with the same palette and rendering style",
                frame_plan.pose
            ));
        }
        if relative_delta(style.orange_ratio, median_style.orange_ratio) > 0.16 {
            errors.push(format!(
                "Orange body-color coverage changed too much for {}; keep the same cat identity and markings",
                frame_plan.pose
            ));
        }
        if relative_delta(style.mean_b, median_style.mean_b) > 0.22 {
            errors.push(format!(
                "Foreground color balance shifted too far for {}; keep the same orange-and-cream palette",
                frame_plan.pose
            ));
        }
        if relative_delta(style.silhouette_density, median_style.silhouette_density) > 0.16 {
            warnings.push(format!(
                "Silhouette density changed for {}; check for missing body detail or extra matte pixels",
                frame_plan.pose
            ));
        }
    }
    if job.action == "cat_walk" {
        if frame_plan.foot_contacts.is_empty() {
            warnings.push("Cat walk frame has no planned paw contact metadata".to_string());
        }
        match &analysis.gait {
            Some(gait) => {
                if gait.contact_count < 2 {
                    errors.push(format!(
                        "{} does not show at least two readable paw-contact clusters; regenerate with planted front and hind paws",
                        frame_plan.pose
                    ));
                }
                if gait.stance_width < 0.22 {
                    errors.push(format!(
                        "{} paw contacts are too bunched to read as a walk stance; use the pose guide's front/hind spacing",
                        frame_plan.pose
                    ));
                }
                if gait.rear_contact_ratio < 0.12 || gait.front_contact_ratio < 0.12 {
                    errors.push(format!(
                        "{} lacks balanced rear/front lower-body support; keep one hind-side and one front-side paw contact",
                        frame_plan.pose
                    ));
                }
                if gait.lower_body_ratio < 0.035 {
                    warnings.push(format!(
                        "{} has little lower-body foreground detail; paws may be too hidden for validation",
                        frame_plan.pose
                    ));
                }
                if let Some((median_root, median_ground_y)) = median_accepted_gait_stats(accepted) {
                    if (gait.root_anchor.x - median_root.x).abs() > 0.075 {
                        errors.push(format!(
                            "{} root anchor shifted horizontally; keep the torso/head locked while only paws move",
                            frame_plan.pose
                        ));
                    }
                    if (gait.ground_y - median_ground_y).abs() > 0.07 {
                        errors.push(format!(
                            "{} ground anchor moved too far; keep paws on the same invisible ground line",
                            frame_plan.pose
                        ));
                    }
                }
            }
            None => errors.push(format!(
                "{} has no measurable lower-body gait; regenerate with visible paws and legs",
                frame_plan.pose
            )),
        }
    }
    (errors, warnings)
}

fn median_accepted_frame_stats(
    analyses: &[AnimationJobFrameAnalysis],
) -> Option<(f32, f32, f32, Option<Pivot>)> {
    let widths = analyses
        .iter()
        .filter_map(|analysis| analysis.bbox_size.map(|size| size.w as f32))
        .collect::<Vec<_>>();
    let heights = analyses
        .iter()
        .filter_map(|analysis| analysis.bbox_size.map(|size| size.h as f32))
        .collect::<Vec<_>>();
    let pixels = analyses
        .iter()
        .map(|analysis| analysis.visible_pixels as f32)
        .filter(|value| *value > 0.0)
        .collect::<Vec<_>>();
    let centers_x = analyses
        .iter()
        .filter_map(|analysis| analysis.bbox_center.map(|center| center.x))
        .collect::<Vec<_>>();
    let centers_y = analyses
        .iter()
        .filter_map(|analysis| analysis.bbox_center.map(|center| center.y))
        .collect::<Vec<_>>();
    let median_w = median_f32(&widths)?;
    let median_h = median_f32(&heights)?;
    let median_pixels = median_f32(&pixels)?;
    let median_center = match (median_f32(&centers_x), median_f32(&centers_y)) {
        (Some(x), Some(y)) => Some(Pivot { x, y }),
        _ => None,
    };
    Some((median_w, median_h, median_pixels, median_center))
}

fn median_accepted_style_stats(
    analyses: &[AnimationJobFrameAnalysis],
) -> Option<AnimationJobStyleStats> {
    if analyses.len() < 3 {
        return None;
    }
    let styles = analyses
        .iter()
        .filter_map(|analysis| analysis.style)
        .collect::<Vec<_>>();
    if styles.len() < 3 {
        return None;
    }
    Some(AnimationJobStyleStats {
        mean_r: median_f32(&styles.iter().map(|style| style.mean_r).collect::<Vec<_>>())?,
        mean_g: median_f32(&styles.iter().map(|style| style.mean_g).collect::<Vec<_>>())?,
        mean_b: median_f32(&styles.iter().map(|style| style.mean_b).collect::<Vec<_>>())?,
        mean_saturation: median_f32(
            &styles
                .iter()
                .map(|style| style.mean_saturation)
                .collect::<Vec<_>>(),
        )?,
        orange_ratio: median_f32(
            &styles
                .iter()
                .map(|style| style.orange_ratio)
                .collect::<Vec<_>>(),
        )?,
        cream_ratio: median_f32(
            &styles
                .iter()
                .map(|style| style.cream_ratio)
                .collect::<Vec<_>>(),
        )?,
        dark_ratio: median_f32(
            &styles
                .iter()
                .map(|style| style.dark_ratio)
                .collect::<Vec<_>>(),
        )?,
        silhouette_density: median_f32(
            &styles
                .iter()
                .map(|style| style.silhouette_density)
                .collect::<Vec<_>>(),
        )?,
    })
}

fn median_accepted_gait_stats(analyses: &[AnimationJobFrameAnalysis]) -> Option<(Pivot, f32)> {
    if analyses.len() < 3 {
        return None;
    }
    let gaits = analyses
        .iter()
        .filter_map(|analysis| analysis.gait.as_ref())
        .collect::<Vec<_>>();
    if gaits.len() < 3 {
        return None;
    }
    let root_x = median_f32(
        &gaits
            .iter()
            .map(|gait| gait.root_anchor.x)
            .collect::<Vec<_>>(),
    )?;
    let root_y = median_f32(
        &gaits
            .iter()
            .map(|gait| gait.root_anchor.y)
            .collect::<Vec<_>>(),
    )?;
    let ground_y = median_f32(&gaits.iter().map(|gait| gait.ground_y).collect::<Vec<_>>())?;
    Some((
        Pivot {
            x: root_x,
            y: root_y,
        },
        ground_y,
    ))
}

fn relative_delta(value: f32, target: f32) -> f32 {
    if target.abs() <= f32::EPSILON {
        0.0
    } else {
        (value - target).abs() / target.abs()
    }
}

fn count_green_edge_pixels(image: &RgbaImage) -> u64 {
    let mut count = 0;
    for y in 0..image.height() {
        for x in 0..image.width() {
            let pixel = image.get_pixel(x, y);
            if pixel[3] > 24
                && pixel[1] > pixel[0].max(pixel[2]).saturating_add(18)
                && touches_transparent_neighbor(image, x, y)
            {
                count += 1;
            }
        }
    }
    count
}

fn slugify(value: &str) -> String {
    let mut slug = String::new();
    let mut previous_underscore = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            previous_underscore = false;
        } else if !previous_underscore {
            slug.push('_');
            previous_underscore = true;
        }
    }
    slug.trim_matches('_').to_string()
}

fn json_rect(rect: Rect) -> JsonRect {
    JsonRect {
        x: rect.x,
        y: rect.y,
        w: rect.w,
        h: rect.h,
    }
}

fn remove_generation_chroma_key(image: &RgbaImage) -> RgbaImage {
    let mut output = image.clone();
    for pixel in output.pixels_mut() {
        let [r, g, b, a] = pixel.0;
        let r_i = r as i16;
        let g_i = g as i16;
        let b_i = b as i16;
        let green_score = g_i - r_i.max(b_i);
        let magenta_score = r_i.min(b_i) - g_i;
        if g > 130 && green_score > 35 {
            if green_score > 82 {
                *pixel = Rgba([r, g, b, 0]);
            } else {
                let alpha_cut = (((green_score - 35) as f32 / 47.0) * 255.0).round() as u8;
                *pixel = Rgba([r, g.min(r.max(b).saturating_add(8)), b, 255 - alpha_cut]);
            }
        } else if r > 120 && b > 120 && g < 170 && magenta_score > 18 {
            if magenta_score > 34 {
                *pixel = Rgba([r, g, b, 0]);
            } else {
                let alpha_cut = (((magenta_score - 18) as f32 / 16.0) * 255.0).round() as u8;
                let clean = g.saturating_add(4).max(r.min(b));
                *pixel = Rgba([clean, g, clean, 255 - alpha_cut]);
            }
        } else if a > 0 && r > 55 && b > 55 && g < 115 && magenta_score > 12 && r.abs_diff(b) <= 96
        {
            if r.max(b) > 150 && magenta_score > 28 {
                *pixel = Rgba([r, g, b, 0]);
            } else {
                let clean = g.saturating_add(10).min(r.min(b).saturating_sub(36));
                *pixel = Rgba([clean, g, clean, a]);
            }
        } else if g > 135 && b > 135 && r < 130 && g.min(b) as i16 - r_i > 38 {
            let cyan_score = g_i.min(b_i) - r_i;
            if cyan_score > 72 {
                *pixel = Rgba([r, g, b, 0]);
            } else {
                let alpha_cut = (((cyan_score - 38) as f32 / 34.0) * 255.0).round() as u8;
                let clean = r.saturating_add(8).min(g.min(b).saturating_sub(24));
                *pixel = Rgba([r, clean, clean, 255 - alpha_cut]);
            }
        }
    }
    remove_border_connected_generated_matte(&mut output);
    decontaminate_transparent_edges(&mut output);
    output
}

fn remove_border_connected_generated_matte(image: &mut RgbaImage) {
    let width = image.width();
    let height = image.height();
    if width == 0 || height == 0 {
        return;
    }
    let mut visited = vec![false; width as usize * height as usize];
    let mut stack = Vec::new();
    for x in 0..width {
        stack.push((x, 0));
        stack.push((x, height - 1));
    }
    for y in 0..height {
        stack.push((0, y));
        stack.push((width - 1, y));
    }

    while let Some((x, y)) = stack.pop() {
        let index = pixel_index(width, x, y);
        if visited[index] {
            continue;
        }
        visited[index] = true;
        if !is_generated_matte_or_transparent(image.get_pixel(x, y)) {
            continue;
        }
        if image.get_pixel(x, y)[3] > 0 {
            let mut pixel = *image.get_pixel(x, y);
            pixel[3] = 0;
            image.put_pixel(x, y, pixel);
        }
        if x > 0 {
            stack.push((x - 1, y));
        }
        if y > 0 {
            stack.push((x, y - 1));
        }
        if x + 1 < width {
            stack.push((x + 1, y));
        }
        if y + 1 < height {
            stack.push((x, y + 1));
        }
    }
}

fn decontaminate_transparent_edges(image: &mut RgbaImage) {
    let width = image.width();
    let height = image.height();
    if width == 0 || height == 0 {
        return;
    }
    let source = image.clone();
    for y in 0..height {
        for x in 0..width {
            let mut pixel = *source.get_pixel(x, y);
            if pixel[3] <= 24 || !touches_transparent_neighbor(&source, x, y) {
                continue;
            }
            let [r, g, b, a] = pixel.0;
            let r_i = r as i16;
            let g_i = g as i16;
            let b_i = b as i16;
            let green_score = g_i - r_i.max(b_i);
            if g > 90 && green_score > 8 {
                let spill = green_score.min(80) as u8;
                pixel[1] = r.max(b).saturating_add(4).min(g);
                pixel[3] = a.saturating_sub(spill.saturating_mul(2));
            } else if is_dark_gray_generated_matte(pixel) && a < 224 {
                pixel[3] = 0;
            }
            image.put_pixel(x, y, pixel);
        }
    }
}

fn touches_transparent_neighbor(image: &RgbaImage, x: u32, y: u32) -> bool {
    let width = image.width();
    let height = image.height();
    (x > 0 && image.get_pixel(x - 1, y)[3] <= 24)
        || (y > 0 && image.get_pixel(x, y - 1)[3] <= 24)
        || (x + 1 < width && image.get_pixel(x + 1, y)[3] <= 24)
        || (y + 1 < height && image.get_pixel(x, y + 1)[3] <= 24)
}

fn is_generated_matte_or_transparent(pixel: &Rgba<u8>) -> bool {
    pixel[3] <= 24 || is_generated_matte_pixel(*pixel)
}

fn is_generated_matte_pixel(pixel: Rgba<u8>) -> bool {
    let [r, g, b, a] = pixel.0;
    if a <= 24 {
        return true;
    }
    let r_i = r as i16;
    let g_i = g as i16;
    let b_i = b as i16;
    let green_score = g_i - r_i.max(b_i);
    let magenta_score = r_i.min(b_i) - g_i;
    let cyan_score = g_i.min(b_i) - r_i;
    (g > 110 && green_score > 18)
        || (r > 100 && b > 100 && g < 175 && magenta_score > 14)
        || (g > 115 && b > 115 && r < 145 && cyan_score > 24)
        || is_dark_gray_generated_matte(pixel)
}

fn is_dark_gray_generated_matte(pixel: Rgba<u8>) -> bool {
    let [r, g, b, a] = pixel.0;
    if a <= 24 {
        return true;
    }
    let max_channel = r.max(g).max(b);
    let min_channel = r.min(g).min(b);
    max_channel <= 105 && max_channel >= 12 && max_channel - min_channel <= 24
}

fn generated_loop_continuity_report(
    sheet: &RgbaImage,
    frame_rects: &[Rect],
) -> Option<SliceContinuityReport> {
    if frame_rects.len() < 2 {
        return None;
    }
    let mut deltas = Vec::new();
    for pair in frame_rects.windows(2) {
        deltas.push(frame_delta(sheet, pair[0], pair[1]));
    }
    let loop_delta = frame_delta(sheet, *frame_rects.last()?, frame_rects[0]);
    let median_neighbor_delta = median_f32(&deltas).unwrap_or(0.0);
    let max_neighbor_delta = deltas.iter().copied().fold(0.0, f32::max);
    let mut warnings = Vec::new();
    if !matches!(frame_rects.len(), 8 | 12 | 16) {
        warnings
            .push("Generated walk loops usually export one 8, 12, or 16 frame cycle".to_string());
    }
    if loop_delta < 0.008 {
        warnings.push(
            "Last frame appears to duplicate the first frame; export should end on the in-between before frame 1".to_string(),
        );
    } else if median_neighbor_delta > 0.0 && loop_delta > (median_neighbor_delta * 1.25).max(0.14) {
        warnings.push(
            "Last-to-first frame jump is larger than neighboring motion; regenerate with a loopable tail frame".to_string(),
        );
    }
    if median_neighbor_delta > 0.0 && max_neighbor_delta > (median_neighbor_delta * 2.4).max(0.18) {
        warnings.push(
            "One or more adjacent frame jumps are much larger than the median; this looks like random poses instead of a gait cycle".to_string(),
        );
    }
    Some(SliceContinuityReport {
        frame_count: frame_rects.len(),
        protocol:
            "single_walk_cycle_contact_down_passing_up_opposite_contact_opposite_down_opposite_passing_opposite_up_no_duplicate_tail"
                .to_string(),
        median_neighbor_delta,
        loop_delta,
        max_neighbor_delta,
        warnings,
    })
}

fn frame_delta(sheet: &RgbaImage, left: Rect, right: Rect) -> f32 {
    let width = left.w.min(right.w);
    let height = left.h.min(right.h);
    if width == 0 || height == 0 {
        return 0.0;
    }
    let step = (width.max(height) / 64).max(1);
    let mut total = 0.0;
    let mut count = 0.0;
    let mut y = 0;
    while y < height {
        let mut x = 0;
        while x < width {
            let a = sheet.get_pixel(left.x + x, left.y + y).0;
            let b = sheet.get_pixel(right.x + x, right.y + y).0;
            let alpha_weight = (a[3].max(b[3]) as f32 / 255.0).max(0.1);
            let dr = (a[0] as f32 - b[0] as f32).abs();
            let dg = (a[1] as f32 - b[1] as f32).abs();
            let db = (a[2] as f32 - b[2] as f32).abs();
            let da = (a[3] as f32 - b[3] as f32).abs();
            total += ((dr + dg + db + da) / (4.0 * 255.0)) * alpha_weight;
            count += alpha_weight;
            x += step;
        }
        y += step;
    }
    if count > 0.0 {
        total / count
    } else {
        0.0
    }
}

fn load_sources(input: &Path, trim: bool) -> Result<Vec<SourceSprite>> {
    let mut paths = Vec::new();
    if input.is_file() {
        if is_supported_image(input) {
            paths.push(input.to_path_buf());
        }
    } else {
        for entry in WalkDir::new(input).sort_by_file_name() {
            let entry = entry?;
            if entry.file_type().is_file() && is_supported_image(entry.path()) {
                paths.push(entry.path().to_path_buf());
            }
        }
    }
    paths.sort_by(|left, right| normalize_path(left).cmp(&normalize_path(right)));
    let mut sources = Vec::new();
    for path in paths {
        sources.extend(load_sources_from_path(&path, trim)?);
    }
    validate_unique_frame_names(sources)
}

fn pixel_snap_sources(
    sources: Vec<SourceSprite>,
    trim: bool,
    options: &PixelSnapOptions,
) -> Result<(Vec<SourceSprite>, BTreeMap<PathBuf, PixelGrid>)> {
    let images = sources
        .iter()
        .map(|source| source.image.clone())
        .collect::<Vec<_>>();
    let snapped =
        pixel_snap::snap_images_with_grids(&images, options).map_err(AtlasError::Message)?;
    let mut grids = BTreeMap::new();
    let sources = sources
        .into_iter()
        .zip(snapped)
        .map(|(mut source, snapped)| {
            grids.insert(source.path.clone(), snapped.grid);
            let image = snapped.image;
            let original_width = image.width();
            let original_height = image.height();
            let (image, trim_x, trim_y, trimmed) = if trim {
                trim_image(&image)
            } else {
                (image, 0, 0, false)
            };
            source.image = image;
            source.original_width = original_width;
            source.original_height = original_height;
            source.trim_x = trim_x;
            source.trim_y = trim_y;
            source.trimmed = trimmed;
            source
        })
        .collect();
    Ok((sources, grids))
}

fn load_sources_with_channels(
    input: &Path,
    trim: bool,
    linked_channels: &[String],
) -> Result<Vec<SourceSprite>> {
    let sources = load_sources(input, trim)?;
    if linked_channels.is_empty() {
        return Ok(sources);
    }
    Ok(sources
        .into_iter()
        .filter(|source| {
            !linked_channels
                .iter()
                .any(|suffix| source.name.ends_with(suffix))
        })
        .collect())
}

fn linked_channel_path(source: &SourceSprite, suffix: &str) -> Option<PathBuf> {
    let parent = source.path.parent().unwrap_or_else(|| Path::new(""));
    let stem = source.path.file_stem()?.to_str()?;
    let current_extension = source
        .path
        .extension()
        .and_then(|extension| extension.to_str());
    let channel_stem = format!("{stem}{suffix}");

    if let Some(extension) = current_extension {
        if RASTER_EXTENSIONS
            .iter()
            .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        {
            let candidate = parent.join(format!("{channel_stem}.{extension}"));
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }

    RASTER_EXTENSIONS.iter().find_map(|extension| {
        let candidate = parent.join(format!("{channel_stem}.{extension}"));
        candidate.exists().then_some(candidate)
    })
}

fn validate_unique_frame_names(sources: Vec<SourceSprite>) -> Result<Vec<SourceSprite>> {
    let mut seen: BTreeMap<String, PathBuf> = BTreeMap::new();
    for source in &sources {
        if let Some(previous) = seen.insert(source.name.clone(), source.path.clone()) {
            return Err(AtlasError::Message(format!(
                "Duplicate frame name '{}' from {} and {}; rename one file or split the batch",
                source.name,
                previous.display(),
                source.path.display()
            )));
        }
    }
    Ok(sources)
}

fn is_supported_image(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            SUPPORTED_EXTENSIONS
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
        .unwrap_or(false)
}

fn is_aseprite_source(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            ASEPRITE_EXTENSIONS
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
        .unwrap_or(false)
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}

fn load_source(path: &Path, trim: bool) -> Result<SourceSprite> {
    let image = decode_image(path)?;
    let original_width = image.width();
    let original_height = image.height();
    let (image, trim_x, trim_y, trimmed) = if trim {
        trim_image(&image)
    } else {
        (image, 0, 0, false)
    };
    Ok(SourceSprite {
        name: frame_name(path),
        path: path.to_path_buf(),
        image,
        original_width,
        original_height,
        trim_x,
        trim_y,
        trimmed,
    })
}

fn load_sources_from_path(path: &Path, trim: bool) -> Result<Vec<SourceSprite>> {
    if is_aseprite_source(path) {
        return load_aseprite_sources(path, trim);
    }
    Ok(vec![load_source(path, trim)?])
}

#[cfg(any(test, feature = "desktop"))]
fn gif_delay_ms(delay: Delay) -> u32 {
    let (numerator, denominator) = delay.numer_denom_ms();
    if denominator == 0 {
        return DEFAULT_DURATION_MS;
    }
    let rounded = ((numerator as u64 + (denominator as u64 / 2)) / denominator as u64)
        .min(u32::MAX as u64) as u32;
    if rounded == 0 {
        DEFAULT_DURATION_MS
    } else {
        rounded
    }
}

#[cfg(any(test, feature = "desktop"))]
fn inspect_gif_preview(path: &Path) -> Result<AtlasPreviewLoad> {
    if !path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.eq_ignore_ascii_case("gif"))
        .unwrap_or(false)
    {
        return Err(AtlasError::Message(format!(
            "GIF inspection requires a .gif file: {}",
            path.display()
        )));
    }

    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);
    let decoder = image::codecs::gif::GifDecoder::new(reader)?;
    let frames = decoder.into_frames().collect_frames()?;
    if frames.is_empty() {
        return Err(AtlasError::Message(format!(
            "GIF has no frames: {}",
            path.display()
        )));
    }

    let cell_width = frames
        .iter()
        .map(|frame| frame.buffer().width())
        .max()
        .unwrap_or(1)
        .max(1);
    let cell_height = frames
        .iter()
        .map(|frame| frame.buffer().height())
        .max()
        .unwrap_or(1)
        .max(1);
    let frame_count = frames.len() as u32;
    let columns = (frame_count as f64).sqrt().ceil().max(1.0) as u32;
    let rows = frame_count.div_ceil(columns).max(1);
    let mut sheet = RgbaImage::from_pixel(
        cell_width.saturating_mul(columns),
        cell_height.saturating_mul(rows),
        Rgba([0, 0, 0, 0]),
    );
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("gif");
    let mut atlas_frames = Vec::with_capacity(frames.len());
    let mut animation_frames = Vec::with_capacity(frames.len());
    let mut total_duration_ms = 0u64;

    for (index, frame) in frames.iter().enumerate() {
        let index = index as u32;
        let column = index % columns;
        let row = index / columns;
        let x = column.saturating_mul(cell_width);
        let y = row.saturating_mul(cell_height);
        overlay(&mut sheet, frame.buffer(), x.into(), y.into());

        let duration_ms = gif_delay_ms(frame.delay());
        total_duration_ms += duration_ms as u64;
        let filename = format!("{stem}_{index:04}");
        animation_frames.push(filename.clone());
        atlas_frames.push(Frame {
            filename,
            frame: JsonRect {
                x,
                y,
                w: cell_width,
                h: cell_height,
            },
            rotated: false,
            trimmed: false,
            sprite_source_size: JsonRect {
                x: 0,
                y: 0,
                w: cell_width,
                h: cell_height,
            },
            source_size: Size {
                w: cell_width,
                h: cell_height,
            },
            pivot: Pivot { x: 0.5, y: 0.5 },
            duration_ms: Some(duration_ms),
        });
    }

    let average_duration = (total_duration_ms as f32 / frames.len().max(1) as f32).max(1.0);
    let sheet_name = format!("{stem}_inspect_sheet.png");
    let sheet_data_url = format!(
        "data:image/png;base64,{}",
        general_purpose::STANDARD.encode(encode_png(&sheet)?)
    );
    let atlas = Atlas {
        meta: Meta {
            app: APP_NAME.to_string(),
            version: VERSION.to_string(),
            image: sheet_name.clone(),
            size: Size {
                w: sheet.width(),
                h: sheet.height(),
            },
            format: "RGBA8888".to_string(),
            scale: 1.0,
            trimmed: false,
        },
        frames: atlas_frames,
        animations: vec![Animation {
            name: stem.to_string(),
            frames: animation_frames,
            looped: true,
            fps_hint: (1000.0 / average_duration).clamp(1.0, 60.0),
        }],
    };

    Ok(AtlasPreviewLoad {
        atlas_path: path.to_string_lossy().to_string(),
        sheet_path: format!("{}#{sheet_name}", path.display()),
        sheet_data_url,
        atlas,
    })
}

struct TemporaryDirectory {
    path: PathBuf,
}

impl TemporaryDirectory {
    fn new(prefix: &str) -> Result<Self> {
        let mut path = std::env::temp_dir();
        let unique = format!(
            "{prefix}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        );
        path.push(unique);
        fs::create_dir_all(&path)?;
        Ok(Self { path })
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn load_aseprite_sources(path: &Path, trim: bool) -> Result<Vec<SourceSprite>> {
    let export_dir = TemporaryDirectory::new("sprite-atlas-packer-aseprite")?;
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("aseprite");
    let output_pattern = export_dir.path.join(format!("{stem}_{{frame}}.png"));
    let binary = std::env::var(ASEPRITE_BIN_ENV).unwrap_or_else(|_| "aseprite".to_string());
    let output = std::process::Command::new(&binary)
        .arg("--batch")
        .arg(path)
        .arg("--save-as")
        .arg(&output_pattern)
        .output()
        .map_err(|error| {
            AtlasError::Message(format!(
                "Unable to import {}: failed to run Aseprite CLI '{binary}' ({error}). Install Aseprite or set {ASEPRITE_BIN_ENV}.",
                path.display()
            ))
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AtlasError::Message(format!(
            "Unable to import {} with Aseprite CLI: {}",
            path.display(),
            stderr.trim()
        )));
    }

    let mut exported = fs::read_dir(&export_dir.path)?
        .map(|entry| entry.map(|entry| entry.path()).map_err(AtlasError::from))
        .collect::<Result<Vec<_>>>()?;
    exported.retain(|path| {
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.eq_ignore_ascii_case("png"))
            .unwrap_or(false)
    });
    exported.sort_by(|left, right| normalize_path(left).cmp(&normalize_path(right)));
    if exported.is_empty() {
        return Err(AtlasError::Message(format!(
            "Aseprite CLI did not export any PNG frames for {}",
            path.display()
        )));
    }

    exported
        .into_iter()
        .map(|frame| load_source(&frame, trim))
        .collect()
}

fn decode_image(path: &Path) -> Result<RgbaImage> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    if extension.eq_ignore_ascii_case("gif") {
        let file = fs::File::open(path)?;
        let reader = BufReader::new(file);
        let decoder = image::codecs::gif::GifDecoder::new(reader)?;
        let frame = decoder.into_frames().next().ok_or_else(|| {
            AtlasError::Message(format!("GIF has no frames: {}", path.display()))
        })??;
        return Ok(DynamicImage::ImageRgba8(frame.into_buffer()).to_rgba8());
    }
    let mut decoder = image::ImageReader::open(path)?
        .with_guessed_format()?
        .into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    Ok(image.to_rgba8())
}

fn frame_name(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("sprite")
        .replace('\\', "/")
}

fn trim_image(image: &RgbaImage) -> (RgbaImage, u32, u32, bool) {
    let mut min_x = image.width();
    let mut min_y = image.height();
    let mut max_x = 0;
    let mut max_y = 0;
    let mut found = false;

    for (x, y, pixel) in image.enumerate_pixels() {
        if pixel[3] != 0 {
            found = true;
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
    }

    if !found {
        return (RgbaImage::from_pixel(1, 1, Rgba([0, 0, 0, 0])), 0, 0, true);
    }

    let width = max_x - min_x + 1;
    let height = max_y - min_y + 1;
    let trimmed = min_x != 0 || min_y != 0 || width != image.width() || height != image.height();
    if !trimmed {
        return (image.clone(), 0, 0, false);
    }
    let cropped = image.view(min_x, min_y, width, height).to_image();
    (cropped, min_x, min_y, true)
}

#[derive(Debug)]
struct Layout {
    width: u32,
    height: u32,
    sprites: Vec<PackedSprite>,
}

fn pack_sprites(
    sources: &[SourceSprite],
    padding: u32,
    pot: bool,
    strategy: PackingStrategy,
    max_size: u32,
    rotate: bool,
) -> Result<Layout> {
    let mut sorted = sources.to_vec();
    sorted.sort_by(|left, right| {
        let left_area = left.image.width() as u64 * left.image.height() as u64;
        let right_area = right.image.width() as u64 * right.image.height() as u64;
        right_area
            .cmp(&left_area)
            .then_with(|| right.image.height().cmp(&left.image.height()))
            .then_with(|| right.image.width().cmp(&left.image.width()))
            .then_with(|| left.name.cmp(&right.name))
    });

    let min_width = sorted
        .iter()
        .map(|sprite| sprite.image.width() + padding * 2)
        .max()
        .unwrap_or(1);
    let min_height = sorted
        .iter()
        .map(|sprite| sprite.image.height() + padding * 2)
        .max()
        .unwrap_or(1);
    if min_width > max_size || min_height > max_size {
        return Err(AtlasError::Message(format!(
            "At least one sprite including padding requires {}x{}, which exceeds --max-size {max_size}",
            min_width, min_height
        )));
    }
    let area = sorted
        .iter()
        .map(|sprite| {
            (sprite.image.width() as u64 + padding as u64 * 2)
                * (sprite.image.height() as u64 + padding as u64 * 2)
        })
        .sum::<u64>();

    let mut side = ceil_sqrt(area).max(min_width).max(min_height).max(1);
    if pot {
        side = side.next_power_of_two();
    }

    let mut width = side.min(max_size);
    let mut height = side.min(max_size);
    for _ in 0..24 {
        if let Some(mut sprites) = try_pack(&sorted, width, height, padding, strategy, rotate) {
            sprites.sort_by(|left, right| left.source.name.cmp(&right.source.name));
            return Ok(Layout {
                width,
                height,
                sprites,
            });
        }
        if width <= height {
            width = grow_dimension(width, pot, max_size)?;
        } else {
            height = grow_dimension(height, pot, max_size)?;
        }
    }

    Err(AtlasError::Message(
        "Unable to fit sprites after expanding atlas".to_string(),
    ))
}

fn ceil_sqrt(value: u64) -> u32 {
    let mut side = (value as f64).sqrt().ceil() as u32;
    while (side as u64) * (side as u64) < value {
        side += 1;
    }
    side
}

fn grow_dimension(value: u32, pot: bool, max_size: u32) -> Result<u32> {
    if value >= max_size {
        return Err(AtlasError::Message(format!(
            "Atlas grew beyond --max-size {max_size}px; multi-atlas output is a post-M1 feature"
        )));
    }
    let next = if pot {
        (value * 2).next_power_of_two()
    } else {
        value + value.max(64) / 2
    };
    Ok(next.min(max_size))
}

fn try_pack(
    sources: &[SourceSprite],
    atlas_width: u32,
    atlas_height: u32,
    padding: u32,
    strategy: PackingStrategy,
    rotate: bool,
) -> Option<Vec<PackedSprite>> {
    match strategy {
        PackingStrategy::MaxRects => {
            try_pack_maxrects(sources, atlas_width, atlas_height, padding, rotate)
        }
        PackingStrategy::Shelf => {
            try_pack_shelf(sources, atlas_width, atlas_height, padding, rotate)
        }
        PackingStrategy::Guillotine => {
            try_pack_guillotine(sources, atlas_width, atlas_height, padding, rotate)
        }
        PackingStrategy::Quadtree => {
            try_pack_quadtree(sources, atlas_width, atlas_height, padding, rotate)
        }
    }
}

fn try_pack_maxrects(
    sources: &[SourceSprite],
    atlas_width: u32,
    atlas_height: u32,
    padding: u32,
    rotate: bool,
) -> Option<Vec<PackedSprite>> {
    let mut free = vec![Rect {
        x: 0,
        y: 0,
        w: atlas_width,
        h: atlas_height,
    }];
    let mut packed = Vec::with_capacity(sources.len());

    for source in sources {
        let (free_index, placement, rotated) =
            choose_oriented_rect(&free, source, padding, rotate)?;
        let used = Rect {
            x: placement.x,
            y: placement.y,
            w: placement.w,
            h: placement.h,
        };
        split_free_rects(&mut free, used);
        prune_free_rects(&mut free);
        packed.push(PackedSprite {
            source: source.clone(),
            x: placement.x + padding,
            y: placement.y + padding,
            rotated,
        });
        if free_index < free.len() {
            sort_free_rects(&mut free);
        }
    }
    Some(packed)
}

fn try_pack_shelf(
    sources: &[SourceSprite],
    atlas_width: u32,
    atlas_height: u32,
    padding: u32,
    rotate: bool,
) -> Option<Vec<PackedSprite>> {
    let mut packed = Vec::with_capacity(sources.len());
    let mut cursor_x = 0;
    let mut cursor_y = 0;
    let mut shelf_height = 0;

    for source in sources {
        let (mut padded_w, mut padded_h, mut rotated) = oriented_dimensions(source, padding, false);
        if rotate && source.image.width() != source.image.height() {
            let (rotated_w, rotated_h, _) = oriented_dimensions(source, padding, true);
            if (cursor_x > 0
                && cursor_x + padded_w > atlas_width
                && cursor_x + rotated_w <= atlas_width)
                || (padded_w > atlas_width && rotated_w <= atlas_width)
            {
                padded_w = rotated_w;
                padded_h = rotated_h;
                rotated = true;
            }
        }
        if padded_w > atlas_width || padded_h > atlas_height {
            return None;
        }
        if cursor_x > 0 && cursor_x + padded_w > atlas_width {
            cursor_x = 0;
            cursor_y += shelf_height;
            shelf_height = 0;
        }
        if cursor_y + padded_h > atlas_height {
            return None;
        }
        packed.push(PackedSprite {
            source: source.clone(),
            x: cursor_x + padding,
            y: cursor_y + padding,
            rotated,
        });
        cursor_x += padded_w;
        shelf_height = shelf_height.max(padded_h);
    }
    Some(packed)
}

fn try_pack_guillotine(
    sources: &[SourceSprite],
    atlas_width: u32,
    atlas_height: u32,
    padding: u32,
    rotate: bool,
) -> Option<Vec<PackedSprite>> {
    let mut free = vec![Rect {
        x: 0,
        y: 0,
        w: atlas_width,
        h: atlas_height,
    }];
    let mut packed = Vec::with_capacity(sources.len());

    for source in sources {
        let (free_index, placement, rotated) =
            choose_oriented_rect(&free, source, padding, rotate)?;
        let free_rect = free.remove(free_index);
        split_guillotine_rect(&mut free, free_rect, placement);
        sort_free_rects(&mut free);
        packed.push(PackedSprite {
            source: source.clone(),
            x: placement.x + padding,
            y: placement.y + padding,
            rotated,
        });
    }
    Some(packed)
}

fn split_guillotine_rect(free: &mut Vec<Rect>, free_rect: Rect, used: Rect) {
    let leftover_w = free_rect.w - used.w;
    let leftover_h = free_rect.h - used.h;
    if leftover_w == 0 && leftover_h == 0 {
        return;
    }

    if leftover_w > leftover_h {
        push_free_rect(
            free,
            Rect {
                x: used.x + used.w,
                y: free_rect.y,
                w: leftover_w,
                h: free_rect.h,
            },
        );
        push_free_rect(
            free,
            Rect {
                x: free_rect.x,
                y: used.y + used.h,
                w: used.w,
                h: leftover_h,
            },
        );
    } else {
        push_free_rect(
            free,
            Rect {
                x: free_rect.x,
                y: used.y + used.h,
                w: free_rect.w,
                h: leftover_h,
            },
        );
        push_free_rect(
            free,
            Rect {
                x: used.x + used.w,
                y: free_rect.y,
                w: leftover_w,
                h: used.h,
            },
        );
    }
}

fn push_free_rect(free: &mut Vec<Rect>, rect: Rect) {
    if rect.w > 0 && rect.h > 0 {
        free.push(rect);
    }
}

#[derive(Debug)]
struct PackTreeNode {
    rect: Rect,
    used: bool,
    right: Option<Box<PackTreeNode>>,
    down: Option<Box<PackTreeNode>>,
}

impl PackTreeNode {
    fn new(rect: Rect) -> Self {
        Self {
            rect,
            used: false,
            right: None,
            down: None,
        }
    }

    fn insert(&mut self, width: u32, height: u32) -> Option<Rect> {
        if self.used {
            if let Some(rect) = self
                .right
                .as_mut()
                .and_then(|node| node.insert(width, height))
            {
                return Some(rect);
            }
            return self
                .down
                .as_mut()
                .and_then(|node| node.insert(width, height));
        }
        if width > self.rect.w || height > self.rect.h {
            return None;
        }

        let placement = Rect {
            x: self.rect.x,
            y: self.rect.y,
            w: width,
            h: height,
        };
        self.used = true;
        let remaining_w = self.rect.w - width;
        let remaining_h = self.rect.h - height;
        if remaining_w > remaining_h {
            self.right = (remaining_w > 0).then(|| {
                Box::new(Self::new(Rect {
                    x: self.rect.x + width,
                    y: self.rect.y,
                    w: remaining_w,
                    h: self.rect.h,
                }))
            });
            self.down = (remaining_h > 0).then(|| {
                Box::new(Self::new(Rect {
                    x: self.rect.x,
                    y: self.rect.y + height,
                    w: width,
                    h: remaining_h,
                }))
            });
        } else {
            self.down = (remaining_h > 0).then(|| {
                Box::new(Self::new(Rect {
                    x: self.rect.x,
                    y: self.rect.y + height,
                    w: self.rect.w,
                    h: remaining_h,
                }))
            });
            self.right = (remaining_w > 0).then(|| {
                Box::new(Self::new(Rect {
                    x: self.rect.x + width,
                    y: self.rect.y,
                    w: remaining_w,
                    h: height,
                }))
            });
        }
        Some(placement)
    }
}

fn try_pack_quadtree(
    sources: &[SourceSprite],
    atlas_width: u32,
    atlas_height: u32,
    padding: u32,
    rotate: bool,
) -> Option<Vec<PackedSprite>> {
    let mut root = PackTreeNode::new(Rect {
        x: 0,
        y: 0,
        w: atlas_width,
        h: atlas_height,
    });
    let mut packed = Vec::with_capacity(sources.len());
    for source in sources {
        let (padded_w, padded_h, rotated) =
            if rotate && source.image.width() != source.image.height() {
                let (normal_w, normal_h, _) = oriented_dimensions(source, padding, false);
                if let Some(placement) = root.insert(normal_w, normal_h) {
                    packed.push(PackedSprite {
                        source: source.clone(),
                        x: placement.x + padding,
                        y: placement.y + padding,
                        rotated: false,
                    });
                    continue;
                }
                oriented_dimensions(source, padding, true)
            } else {
                oriented_dimensions(source, padding, false)
            };
        let placement = root.insert(padded_w, padded_h)?;
        packed.push(PackedSprite {
            source: source.clone(),
            x: placement.x + padding,
            y: placement.y + padding,
            rotated,
        });
    }
    Some(packed)
}

fn oriented_dimensions(source: &SourceSprite, padding: u32, rotated: bool) -> (u32, u32, bool) {
    let width = if rotated {
        source.image.height()
    } else {
        source.image.width()
    };
    let height = if rotated {
        source.image.width()
    } else {
        source.image.height()
    };
    (width + padding * 2, height + padding * 2, rotated)
}

fn choose_oriented_rect(
    free: &[Rect],
    source: &SourceSprite,
    padding: u32,
    rotate: bool,
) -> Option<(usize, Rect, bool)> {
    let (width, height, rotated) = oriented_dimensions(source, padding, false);
    let normal = choose_rect(free, width, height).map(|(index, rect)| (index, rect, rotated));
    if !rotate || source.image.width() == source.image.height() {
        return normal;
    }
    let (width, height, rotated) = oriented_dimensions(source, padding, true);
    let rotated_rect = choose_rect(free, width, height).map(|(index, rect)| (index, rect, rotated));
    match (normal, rotated_rect) {
        (Some(normal), Some(rotated)) => {
            Some(best_oriented_placement(normal, rotated, width, height))
        }
        (Some(normal), None) => Some(normal),
        (None, Some(rotated)) => Some(rotated),
        (None, None) => None,
    }
}

fn best_oriented_placement(
    normal: (usize, Rect, bool),
    rotated: (usize, Rect, bool),
    rotated_width: u32,
    rotated_height: u32,
) -> (usize, Rect, bool) {
    let normal_score = normal.1.w * normal.1.h;
    let rotated_score = rotated_width * rotated_height;
    if rotated_score < normal_score {
        rotated
    } else {
        normal
    }
}

fn choose_rect(free: &[Rect], width: u32, height: u32) -> Option<(usize, Rect)> {
    free.iter()
        .copied()
        .enumerate()
        .filter(|(_, rect)| width <= rect.w && height <= rect.h)
        .min_by(|(_, left), (_, right)| {
            let left_short = (left.w - width).min(left.h - height);
            let right_short = (right.w - width).min(right.h - height);
            let left_long = (left.w - width).max(left.h - height);
            let right_long = (right.w - width).max(right.h - height);
            left.y
                .cmp(&right.y)
                .then_with(|| left.x.cmp(&right.x))
                .then_with(|| left_short.cmp(&right_short))
                .then_with(|| left_long.cmp(&right_long))
        })
        .map(|(index, rect)| {
            (
                index,
                Rect {
                    x: rect.x,
                    y: rect.y,
                    w: width,
                    h: height,
                },
            )
        })
}

fn split_free_rects(free: &mut Vec<Rect>, used: Rect) {
    let mut next = Vec::new();
    for rect in free.drain(..) {
        if !intersects(rect, used) {
            next.push(rect);
            continue;
        }
        if used.x > rect.x {
            next.push(Rect {
                x: rect.x,
                y: rect.y,
                w: used.x - rect.x,
                h: rect.h,
            });
        }
        if used.x + used.w < rect.x + rect.w {
            next.push(Rect {
                x: used.x + used.w,
                y: rect.y,
                w: rect.x + rect.w - (used.x + used.w),
                h: rect.h,
            });
        }
        if used.y > rect.y {
            next.push(Rect {
                x: rect.x,
                y: rect.y,
                w: rect.w,
                h: used.y - rect.y,
            });
        }
        if used.y + used.h < rect.y + rect.h {
            next.push(Rect {
                x: rect.x,
                y: used.y + used.h,
                w: rect.w,
                h: rect.y + rect.h - (used.y + used.h),
            });
        }
    }
    *free = next
        .into_iter()
        .filter(|rect| rect.w > 0 && rect.h > 0)
        .collect();
    sort_free_rects(free);
}

fn intersects(left: Rect, right: Rect) -> bool {
    left.x < right.x + right.w
        && left.x + left.w > right.x
        && left.y < right.y + right.h
        && left.y + left.h > right.y
}

fn prune_free_rects(free: &mut Vec<Rect>) {
    let mut keep = vec![true; free.len()];
    for i in 0..free.len() {
        for j in 0..free.len() {
            if i != j && keep[i] && contains(free[j], free[i]) {
                keep[i] = false;
            }
        }
    }
    let mut index = 0;
    free.retain(|_| {
        let should_keep = keep[index];
        index += 1;
        should_keep
    });
    sort_free_rects(free);
}

fn contains(outer: Rect, inner: Rect) -> bool {
    inner.x >= outer.x
        && inner.y >= outer.y
        && inner.x + inner.w <= outer.x + outer.w
        && inner.y + inner.h <= outer.y + outer.h
}

fn sort_free_rects(free: &mut [Rect]) {
    free.sort_by(|left, right| {
        left.y
            .cmp(&right.y)
            .then_with(|| left.x.cmp(&right.x))
            .then_with(|| left.h.cmp(&right.h))
            .then_with(|| left.w.cmp(&right.w))
    });
}

fn render_sheet(sprites: &[PackedSprite], width: u32, height: u32) -> RgbaImage {
    let mut sheet = RgbaImage::from_pixel(width, height, Rgba([0, 0, 0, 0]));
    for sprite in sprites {
        let image = packed_sprite_image(&sprite.source.image, sprite.rotated);
        image::imageops::overlay(&mut sheet, &image, sprite.x.into(), sprite.y.into());
    }
    sheet
}

fn packed_sprite_image(image: &RgbaImage, rotated: bool) -> RgbaImage {
    if rotated {
        image::imageops::rotate90(image)
    } else {
        image.clone()
    }
}

fn render_linked_channel_sheet(
    sprites: &[PackedSprite],
    width: u32,
    height: u32,
    suffix: &str,
    trim: bool,
    pixel_grids: &BTreeMap<PathBuf, PixelGrid>,
) -> Result<RgbaImage> {
    let mut sheet = RgbaImage::from_pixel(width, height, Rgba([0, 0, 0, 0]));
    for sprite in sprites {
        let channel = load_linked_channel_image(&sprite.source, suffix, trim, pixel_grids)?;
        let channel = packed_sprite_image(&channel, sprite.rotated);
        image::imageops::overlay(&mut sheet, &channel, sprite.x.into(), sprite.y.into());
    }
    Ok(sheet)
}

fn load_linked_channel_image(
    source: &SourceSprite,
    suffix: &str,
    trim: bool,
    pixel_grids: &BTreeMap<PathBuf, PixelGrid>,
) -> Result<RgbaImage> {
    let Some(path) = linked_channel_path(source, suffix) else {
        return Ok(RgbaImage::from_pixel(
            source.image.width(),
            source.image.height(),
            Rgba([0, 0, 0, 0]),
        ));
    };
    let input = decode_image(&path)?;
    let image = if let Some(grid) = pixel_grids.get(&source.path) {
        pixel_snap::resample_linked_channel(&input, grid).map_err(|error| {
            AtlasError::Message(format!(
                "Unable to apply the color sprite's Pixel Snap grid to {}: {error}",
                path.display()
            ))
        })?
    } else {
        input
    };
    if image.width() != source.original_width || image.height() != source.original_height {
        return Err(AtlasError::Message(format!(
            "Linked channel {} must match {} dimensions ({}x{}), got {}x{}",
            path.display(),
            source.path.display(),
            source.original_width,
            source.original_height,
            image.width(),
            image.height()
        )));
    }
    if trim {
        Ok(image
            .view(
                source.trim_x,
                source.trim_y,
                source.image.width(),
                source.image.height(),
            )
            .to_image())
    } else {
        Ok(image)
    }
}

fn deduplicate_sources(
    sources: &[SourceSprite],
    threshold: Option<u8>,
    trim: bool,
    linked_channels: &[String],
    pixel_grids: &BTreeMap<PathBuf, PixelGrid>,
) -> Result<(Vec<SourceSprite>, Vec<SourceAlias>)> {
    let Some(threshold) = threshold else {
        return Ok((sources.to_vec(), Vec::new()));
    };
    let mut unique = Vec::<SourceSprite>::new();
    let mut aliases = Vec::<SourceAlias>::new();
    for source in sources {
        let mut target = None;
        for candidate in &unique {
            if can_alias_source_with_channels(
                source,
                candidate,
                threshold,
                trim,
                linked_channels,
                pixel_grids,
            )? {
                target = Some(candidate.name.clone());
                break;
            }
        }
        if let Some(target) = target {
            aliases.push(SourceAlias {
                source: source.clone(),
                target,
            });
        } else {
            unique.push(source.clone());
        }
    }
    Ok((unique, aliases))
}

fn can_alias_source_with_channels(
    source: &SourceSprite,
    candidate: &SourceSprite,
    threshold: u8,
    trim: bool,
    linked_channels: &[String],
    pixel_grids: &BTreeMap<PathBuf, PixelGrid>,
) -> Result<bool> {
    if !can_alias_source(source, candidate, threshold) {
        return Ok(false);
    }
    for suffix in linked_channels {
        let source_channel = load_linked_channel_image(source, suffix, trim, pixel_grids)?;
        let candidate_channel = load_linked_channel_image(candidate, suffix, trim, pixel_grids)?;
        if !image_delta_within_threshold(&source_channel, &candidate_channel, threshold) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn can_alias_source(source: &SourceSprite, candidate: &SourceSprite, threshold: u8) -> bool {
    source.original_width == candidate.original_width
        && source.original_height == candidate.original_height
        && source.trim_x == candidate.trim_x
        && source.trim_y == candidate.trim_y
        && source.image.dimensions() == candidate.image.dimensions()
        && image_delta_within_threshold(&source.image, &candidate.image, threshold)
}

fn image_delta_within_threshold(left: &RgbaImage, right: &RgbaImage, threshold: u8) -> bool {
    if left.dimensions() != right.dimensions() {
        return false;
    }
    left.as_raw()
        .iter()
        .zip(right.as_raw().iter())
        .all(|(left, right)| left.abs_diff(*right) <= threshold)
}

fn write_png(path: &Path, image: &RgbaImage) -> Result<()> {
    write_bytes_if_changed(path, &encode_png(image)?)
}

fn encode_png(image: &RgbaImage) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes).write_image(
        image.as_raw(),
        image.width(),
        image.height(),
        ColorType::Rgba8.into(),
    )?;
    Ok(bytes)
}

fn write_channel_sheet(
    output: &Path,
    format: OutputFormat,
    suffix: &str,
    image: &RgbaImage,
) -> Result<Vec<String>> {
    let suffix = normalize_sheet_suffix(suffix);
    let mut wrote = Vec::new();
    match format {
        OutputFormat::Webp => {
            let name = format!("sheet{suffix}.webp");
            write_webp(&output.join(&name), image)?;
            wrote.push(name);
        }
        OutputFormat::Png => {
            let name = format!("sheet{suffix}.png");
            write_png(&output.join(&name), image)?;
            wrote.push(name);
        }
        OutputFormat::Both => {
            let webp = format!("sheet{suffix}.webp");
            let png = format!("sheet{suffix}.png");
            write_webp(&output.join(&webp), image)?;
            write_png(&output.join(&png), image)?;
            wrote.push(webp);
            wrote.push(png);
        }
    }
    Ok(wrote)
}

fn normalize_sheet_suffix(suffix: &str) -> String {
    if suffix.starts_with('_') {
        suffix.to_string()
    } else {
        format!("_{suffix}")
    }
}

fn write_webp(path: &Path, image: &RgbaImage) -> Result<()> {
    write_bytes_if_changed(path, &encode_webp(image)?)
}

fn encode_webp(image: &RgbaImage) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    WebPEncoder::new_lossless(&mut bytes).encode(
        image.as_raw(),
        image.width(),
        image.height(),
        ColorType::Rgba8.into(),
    )?;
    Ok(bytes)
}

fn write_text_if_changed(path: &Path, text: &str) -> Result<()> {
    write_bytes_if_changed(path, text.as_bytes())
}

fn write_bytes_if_changed(path: &Path, bytes: &[u8]) -> Result<()> {
    if fs::read(path)
        .map(|existing| existing == bytes)
        .unwrap_or(false)
    {
        return Ok(());
    }
    fs::write(path, bytes)?;
    Ok(())
}

fn build_atlas(
    sprites: &[PackedSprite],
    aliases: &[SourceAlias],
    options: &PackOptions,
    width: u32,
    height: u32,
) -> Atlas {
    let mut frames = sprites
        .iter()
        .map(|sprite| {
            frame_from_source(&sprite.source, sprite.x, sprite.y, sprite.rotated, options)
        })
        .collect::<Vec<_>>();
    let packed_by_name = sprites
        .iter()
        .map(|sprite| (sprite.source.name.as_str(), sprite))
        .collect::<BTreeMap<_, _>>();
    for alias in aliases {
        if let Some(target) = packed_by_name.get(alias.target.as_str()) {
            frames.push(frame_from_source(
                &alias.source,
                target.x,
                target.y,
                target.rotated,
                options,
            ));
        }
    }
    apply_animation_frame_overrides(&mut frames, options);

    Atlas {
        meta: Meta {
            app: APP_NAME.to_string(),
            version: VERSION.to_string(),
            image: options.format.atlas_image_name().to_string(),
            size: Size {
                w: width,
                h: height,
            },
            format: "RGBA8888".to_string(),
            scale: 1.0,
            trimmed: options.trim,
        },
        animations: atlas_animations(&frames, options),
        frames,
    }
}

fn frame_from_source(
    source: &SourceSprite,
    x: u32,
    y: u32,
    rotated: bool,
    options: &PackOptions,
) -> Frame {
    let frame_width = if rotated {
        source.image.height()
    } else {
        source.image.width()
    };
    let frame_height = if rotated {
        source.image.width()
    } else {
        source.image.height()
    };
    Frame {
        filename: source.name.clone(),
        frame: JsonRect {
            x,
            y,
            w: frame_width,
            h: frame_height,
        },
        rotated,
        trimmed: source.trimmed,
        sprite_source_size: JsonRect {
            x: source.trim_x,
            y: source.trim_y,
            w: source.image.width(),
            h: source.image.height(),
        },
        source_size: Size {
            w: source.original_width,
            h: source.original_height,
        },
        pivot: frame_pivot(source, options),
        duration_ms: Some(DEFAULT_DURATION_MS),
    }
}

fn atlas_animations(frames: &[Frame], options: &PackOptions) -> Vec<Animation> {
    if !options.animations.is_empty() {
        return explicit_animations(frames, &options.animations);
    }
    infer_animations(frames, options.animation_regex.as_deref())
}

fn explicit_animations(frames: &[Frame], definitions: &[AnimationDefinition]) -> Vec<Animation> {
    let frame_names = frames
        .iter()
        .map(|frame| frame.filename.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    definitions
        .iter()
        .filter_map(|definition| {
            let frames = definition
                .frames
                .iter()
                .filter(|name| frame_names.contains(name.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            if definition.name.trim().is_empty() || frames.is_empty() {
                return None;
            }
            Some(Animation {
                name: definition.name.clone(),
                frames,
                looped: definition.looped,
                fps_hint: definition.fps_hint.max(0.01),
            })
        })
        .collect()
}

fn apply_animation_frame_overrides(frames: &mut [Frame], options: &PackOptions) {
    if options.animations.is_empty() {
        return;
    }
    let mut overrides = BTreeMap::new();
    for definition in &options.animations {
        for frame in &definition.frames {
            overrides.insert(frame.as_str(), (definition.pivot, definition.duration_ms));
        }
        for (frame, override_) in &definition.frame_overrides {
            let base = overrides.entry(frame.as_str()).or_insert((None, None));
            if override_.pivot.is_some() {
                base.0 = override_.pivot;
            }
            if override_.duration_ms.is_some() {
                base.1 = override_.duration_ms;
            }
        }
    }
    for frame in frames {
        if let Some((pivot, duration_ms)) = overrides.get(frame.filename.as_str()) {
            if let Some(pivot) = pivot {
                frame.pivot = *pivot;
            }
            if let Some(duration_ms) = duration_ms {
                frame.duration_ms = Some((*duration_ms).max(1));
            }
        }
    }
}

fn frame_pivot(source: &SourceSprite, options: &PackOptions) -> Pivot {
    if let Some(pivot) = options
        .animations
        .iter()
        .find(|definition| definition.frames.iter().any(|frame| frame == &source.name))
        .and_then(|definition| definition.pivot)
    {
        return pivot;
    }
    if options.stable_pivot {
        return stable_trimmed_pivot(source);
    }
    Pivot { x: 0.5, y: 0.5 }
}

fn stable_trimmed_pivot(source: &SourceSprite) -> Pivot {
    let center_x = source.original_width as f32 / 2.0;
    let center_y = source.original_height as f32 / 2.0;
    Pivot {
        x: ((center_x - source.trim_x as f32) / source.image.width().max(1) as f32).clamp(0.0, 1.0),
        y: ((center_y - source.trim_y as f32) / source.image.height().max(1) as f32)
            .clamp(0.0, 1.0),
    }
}

fn infer_animations(frames: &[Frame], animation_regex: Option<&str>) -> Vec<Animation> {
    let regex = animation_regex.and_then(|pattern| Regex::new(pattern).ok());
    let mut groups: BTreeMap<String, Vec<(u32, String)>> = BTreeMap::new();
    for frame in frames {
        let (name, index) = split_animation_name(&frame.filename, regex.as_ref());
        groups
            .entry(name)
            .or_default()
            .push((index, frame.filename.clone()));
    }
    groups
        .into_iter()
        .map(|(name, mut frames)| {
            frames.sort_by(|left, right| {
                left.0
                    .cmp(&right.0)
                    .then_with(|| naturalish_cmp(&left.1, &right.1))
            });
            Animation {
                name,
                frames: frames.into_iter().map(|(_, frame)| frame).collect(),
                looped: true,
                fps_hint: 1000.0 / DEFAULT_DURATION_MS as f32,
            }
        })
        .collect()
}

fn split_animation_name(filename: &str, animation_regex: Option<&Regex>) -> (String, u32) {
    if let Some(regex) = animation_regex {
        if let Some(captures) = regex.captures(filename) {
            let name = captures
                .name("name")
                .map(|match_| match_.as_str())
                .or_else(|| captures.get(1).map(|match_| match_.as_str()))
                .filter(|name| !name.is_empty());
            let index = captures
                .name("index")
                .or_else(|| captures.get(2))
                .and_then(|match_| match_.as_str().parse::<u32>().ok())
                .unwrap_or(0);
            if let Some(name) = name {
                return (name.to_string(), index);
            }
        }
    }
    let Some((prefix, suffix)) = filename.rsplit_once('_') else {
        return (filename.to_string(), 0);
    };
    match suffix.parse::<u32>() {
        Ok(index) => (prefix.to_string(), index),
        Err(_) => (filename.to_string(), 0),
    }
}

fn naturalish_cmp(left: &str, right: &str) -> Ordering {
    left.cmp(right)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;
    use tempfile::TempDir;

    #[test]
    fn pixel_art_finish_runs_before_atlas_packing() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input");
        let output = temp.path().join("output");
        fs::create_dir_all(&input).unwrap();
        write_precise_sprite(
            &input.join("hero_idle_0.png"),
            8,
            8,
            Rect {
                x: 0,
                y: 0,
                w: 8,
                h: 8,
            },
            [240, 20, 30, 255],
        );

        let report = pack_with_pixel_snap(
            PackOptions {
                input,
                output: output.clone(),
                format: OutputFormat::Png,
                pot: false,
                padding: 0,
                trim: false,
                ..PackOptions::default()
            },
            PixelSnapOptions {
                enabled: true,
                color_count: 4,
                pixel_size: Some(4.0),
                palette: "000000,ffffff".to_string(),
                lock_grid: true,
                engine: PixelSnapEngine::Snapper,
            },
        )
        .unwrap();

        assert_eq!(report.atlas.frames[0].source_size, Size { w: 2, h: 2 });
        assert_eq!(report.atlas_width, 2);
        assert_eq!(report.atlas_height, 2);
        let sheet = image::open(output.join("sheet.png")).unwrap().to_rgba8();
        assert_eq!(sheet.get_pixel(0, 0).0, [0, 0, 0, 255]);
    }

    #[test]
    fn pixel_art_finish_reuses_color_grid_for_linked_channels() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input");
        let output = temp.path().join("output");
        fs::create_dir_all(&input).unwrap();
        let full_frame = Rect {
            x: 0,
            y: 0,
            w: 8,
            h: 8,
        };
        write_precise_sprite(
            &input.join("hero_idle_0.png"),
            8,
            8,
            full_frame,
            [240, 20, 30, 255],
        );
        write_precise_sprite(
            &input.join("hero_idle_0_n.png"),
            8,
            8,
            full_frame,
            [128, 128, 255, 255],
        );

        let report = pack_with_pixel_snap(
            PackOptions {
                input,
                output: output.clone(),
                format: OutputFormat::Png,
                pot: false,
                padding: 0,
                trim: false,
                linked_channels: vec!["_n".to_string()],
                ..PackOptions::default()
            },
            PixelSnapOptions {
                enabled: true,
                color_count: 4,
                pixel_size: Some(4.0),
                palette: "000000,ffffff".to_string(),
                lock_grid: true,
                engine: PixelSnapEngine::Snapper,
            },
        )
        .unwrap();

        assert_eq!(report.atlas.frames[0].source_size, Size { w: 2, h: 2 });
        assert!(report.wrote.contains(&"sheet_n.png".to_string()));
        let color_sheet = image::open(output.join("sheet.png")).unwrap().to_rgba8();
        let normal_sheet = image::open(output.join("sheet_n.png")).unwrap().to_rgba8();
        assert_eq!(color_sheet.get_pixel(0, 0).0, [0, 0, 0, 255]);
        assert_eq!(normal_sheet.get_pixel(0, 0).0, [128, 128, 255, 255]);
        assert_eq!(normal_sheet.dimensions(), color_sheet.dimensions());
    }

    #[test]
    fn pixel_art_finish_fixer_engine_recovers_native_grid() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input");
        let output = temp.path().join("output");
        fs::create_dir_all(&input).unwrap();
        // 16x16 logical pixels stored at 4x block scale (64x64), with a
        // non-repeating per-cell pattern so detection cannot lock onto a
        // content harmonic instead of the true 4 px cell.
        let mut image = RgbaImage::new(64, 64);
        for y in 0..64u32 {
            for x in 0..64u32 {
                let cell_x = x / 4;
                let cell_y = y / 4;
                let color = [
                    ((cell_x * 37 + cell_y * 11) % 256) as u8,
                    ((cell_x * 7 + cell_y * 53) % 256) as u8,
                    ((cell_x * 23 + cell_y * 29) % 256) as u8,
                    255,
                ];
                image.put_pixel(x, y, Rgba(color));
            }
        }
        image.save(input.join("hero_idle_0.png")).unwrap();

        let report = pack_with_pixel_snap(
            PackOptions {
                input,
                output,
                format: OutputFormat::Png,
                pot: false,
                padding: 0,
                trim: false,
                ..PackOptions::default()
            },
            PixelSnapOptions {
                enabled: true,
                color_count: 16,
                pixel_size: None,
                palette: String::new(),
                lock_grid: true,
                engine: PixelSnapEngine::Fixer,
            },
        )
        .unwrap();

        assert_eq!(report.atlas.frames[0].source_size, Size { w: 16, h: 16 });
    }

    #[test]
    fn trims_transparent_borders() {
        let mut image = RgbaImage::from_pixel(6, 5, Rgba([0, 0, 0, 0]));
        for y in 2..4 {
            for x in 1..5 {
                image.put_pixel(x, y, Rgba([255, 0, 0, 255]));
            }
        }
        let (trimmed, x, y, changed) = trim_image(&image);
        assert!(changed);
        assert_eq!((x, y), (1, 2));
        assert_eq!((trimmed.width(), trimmed.height()), (4, 2));
    }

    #[test]
    fn decode_image_applies_exif_orientation() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("oriented.jpg");
        write_oriented_jpeg(&path, 2, 3, 6);

        let decoded = decode_image(&path).unwrap();
        assert_eq!(decoded.dimensions(), (3, 2));
    }

    #[test]
    fn packed_outputs_strip_source_metadata() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input");
        let output = temp.path().join("out");
        fs::create_dir_all(&input).unwrap();
        write_oriented_jpeg(&input.join("hero_idle_0.jpg"), 2, 3, 6);

        pack(PackOptions {
            input,
            output: output.clone(),
            format: OutputFormat::Both,
            packing_strategy: PackingStrategy::MaxRects,
            pot: true,
            padding: 2,
            max_size: DEFAULT_MAX_SIZE,
            rotate: false,
            trim: true,
            dry_run: false,
            watch: false,
            slice_mode: None,
            cell: None,
            prefix: "frame".to_string(),
            aseprite_json: false,
            godot_spriteframes: false,
            unity_importer: false,
            animation_regex: None,
            animations: Vec::new(),
            stable_pivot: false,
            dedup_threshold: None,
            linked_channels: Vec::new(),
            normalize_generated: false,
            frame_size: None,
        })
        .unwrap();

        assert_image_has_no_exif_or_icc(&output.join("sheet.png"));
        assert_image_has_no_exif_or_icc(&output.join("sheet.webp"));
    }

    #[test]
    fn aseprite_inputs_export_frames_through_cli() {
        static ASEPRITE_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ASEPRITE_ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input");
        let output = temp.path().join("out");
        #[cfg(windows)]
        let fake_bin = temp.path().join("fake-aseprite.cmd");
        #[cfg(not(windows))]
        let fake_bin = temp.path().join("fake-aseprite");
        let frames = temp.path().join("frames");
        fs::create_dir_all(&input).unwrap();
        fs::create_dir_all(&frames).unwrap();
        fs::write(input.join("hero_run.aseprite"), b"fake aseprite document").unwrap();
        write_test_sprite(&frames.join("frame0.png"), [255, 0, 0, 255], 3, 3);
        write_test_sprite(&frames.join("frame1.png"), [0, 255, 0, 255], 3, 3);
        #[cfg(windows)]
        fs::write(
            &fake_bin,
            format!(
                "@echo off\r\nsetlocal\r\nset \"out=\"\r\nset \"prev=\"\r\n:loop\r\nif \"%~1\"==\"\" goto done\r\nif \"%prev%\"==\"--save-as\" set \"out=%~1\"\r\nset \"prev=%~1\"\r\nshift\r\ngoto loop\r\n:done\r\nset \"out0=%out:{{frame}}=0%\"\r\nset \"out1=%out:{{frame}}=1%\"\r\ncopy /Y \"{}\" \"%out0%\" >NUL\r\ncopy /Y \"{}\" \"%out1%\" >NUL\r\n",
                frames.join("frame0.png").display(),
                frames.join("frame1.png").display()
            ),
        )
        .unwrap();
        #[cfg(not(windows))]
        fs::write(
            &fake_bin,
            format!(
                "#!/usr/bin/env bash\nset -euo pipefail\nout=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"--save-as\" ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\ncp \"{}\" \"${{out/\\{{frame\\}}/0}}\"\ncp \"{}\" \"${{out/\\{{frame\\}}/1}}\"\n",
                frames.join("frame0.png").display(),
                frames.join("frame1.png").display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&fake_bin).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&fake_bin, permissions).unwrap();
        }

        std::env::set_var(ASEPRITE_BIN_ENV, &fake_bin);
        let report = pack(PackOptions {
            input,
            output,
            format: OutputFormat::Png,
            packing_strategy: PackingStrategy::MaxRects,
            pot: true,
            padding: 2,
            max_size: DEFAULT_MAX_SIZE,
            rotate: false,
            trim: true,
            dry_run: false,
            watch: false,
            slice_mode: None,
            cell: None,
            prefix: "frame".to_string(),
            aseprite_json: false,
            godot_spriteframes: false,
            unity_importer: false,
            animation_regex: None,
            animations: Vec::new(),
            stable_pivot: false,
            dedup_threshold: None,
            linked_channels: Vec::new(),
            normalize_generated: false,
            frame_size: None,
        })
        .unwrap();
        std::env::remove_var(ASEPRITE_BIN_ENV);

        assert_eq!(report.input_count, 2);
        assert_eq!(report.frames, vec!["hero_run_0", "hero_run_1"]);
        assert_eq!(report.animations, vec!["hero_run"]);
    }

    #[test]
    fn gif_inputs_decode_first_frame_only() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("animated.gif");
        write_two_frame_gif(&path);

        let decoded = decode_image(&path).unwrap();
        assert_eq!(decoded.dimensions(), (3, 2));
        assert_eq!(decoded.get_pixel(0, 0), &Rgba([255, 0, 0, 255]));
    }

    #[test]
    fn gif_inspection_preview_loads_all_frames() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("walk.gif");
        write_two_frame_gif(&path);

        let preview = inspect_gif_preview(&path).unwrap();

        assert_eq!(preview.atlas.frames.len(), 2);
        assert_eq!(preview.atlas.animations.len(), 1);
        assert_eq!(
            preview.atlas.animations[0].frames,
            vec!["walk_0000".to_string(), "walk_0001".to_string()]
        );
        assert_eq!(preview.atlas.frames[0].duration_ms, Some(80));
        assert_eq!(preview.atlas.frames[1].duration_ms, Some(80));
        assert!(preview.sheet_data_url.starts_with("data:image/png;base64,"));
    }

    #[test]
    fn infers_animation_from_last_numeric_token() {
        let frames = vec![
            frame("hero_run_10"),
            frame("hero_run_2"),
            frame("hero_idle_0"),
        ];
        let animations = infer_animations(&frames, None);
        assert_eq!(animations[0].name, "hero_idle");
        assert_eq!(animations[1].name, "hero_run");
        assert_eq!(animations[1].frames, vec!["hero_run_2", "hero_run_10"]);
    }

    #[test]
    fn custom_animation_regex_groups_frames() {
        let frames = vec![
            frame("hero-idle-10"),
            frame("hero-idle-2"),
            frame("hero-run-0"),
        ];
        let animations = infer_animations(&frames, Some(r"^(?P<name>.+)-(?P<index>\d+)$"));
        assert_eq!(animations[0].name, "hero-idle");
        assert_eq!(animations[0].frames, vec!["hero-idle-2", "hero-idle-10"]);
        assert_eq!(animations[1].name, "hero-run");
    }

    #[test]
    fn dry_run_plans_without_output_path_or_writes() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input");
        let output = temp.path().join("should_not_exist");
        fs::create_dir_all(&input).unwrap();
        write_test_sprite(&input.join("hero_idle_0.png"), [255, 0, 0, 255], 4, 4);

        let report = pack(PackOptions {
            input,
            output: PathBuf::new(),
            format: OutputFormat::Png,
            packing_strategy: PackingStrategy::MaxRects,
            pot: true,
            padding: 2,
            max_size: DEFAULT_MAX_SIZE,
            rotate: false,
            trim: true,
            dry_run: true,
            watch: false,
            slice_mode: None,
            cell: None,
            prefix: "frame".to_string(),
            aseprite_json: true,
            godot_spriteframes: false,
            unity_importer: false,
            animation_regex: None,
            animations: Vec::new(),
            stable_pivot: false,
            dedup_threshold: None,
            linked_channels: vec!["_n".to_string()],
            normalize_generated: false,
            frame_size: None,
        })
        .unwrap();

        assert_eq!(report.input_count, 1);
        assert!(report.wrote.is_empty());
        assert!(!output.exists());
    }

    #[test]
    fn source_snapshot_tracks_content_not_mtime() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input");
        fs::create_dir_all(&input).unwrap();
        let sprite = input.join("hero_idle_0.png");
        write_test_sprite(&sprite, [255, 0, 0, 255], 4, 4);

        let first = source_snapshot(&input).unwrap();
        write_test_sprite(&sprite, [255, 0, 0, 255], 4, 4);
        let identical_rewrite = source_snapshot(&input).unwrap();
        write_test_sprite(&sprite, [0, 255, 0, 255], 4, 4);
        let content_change = source_snapshot(&input).unwrap();

        assert_eq!(first, identical_rewrite);
        assert_ne!(first, content_change);
    }

    #[test]
    fn pot_toggle_controls_atlas_dimensions() {
        let source = SourceSprite {
            name: "wide".to_string(),
            path: PathBuf::from("wide.png"),
            image: RgbaImage::from_pixel(7, 3, Rgba([255, 0, 0, 255])),
            original_width: 7,
            original_height: 3,
            trim_x: 0,
            trim_y: 0,
            trimmed: false,
        };

        let pot_layout = pack_sprites(
            std::slice::from_ref(&source),
            0,
            true,
            PackingStrategy::MaxRects,
            DEFAULT_MAX_SIZE,
            false,
        )
        .unwrap();
        let npot_layout = pack_sprites(
            &[source],
            0,
            false,
            PackingStrategy::MaxRects,
            DEFAULT_MAX_SIZE,
            false,
        )
        .unwrap();

        assert!(pot_layout.width.is_power_of_two());
        assert!(pot_layout.height.is_power_of_two());
        assert_eq!((npot_layout.width, npot_layout.height), (7, 7));
        assert!(!npot_layout.width.is_power_of_two());
    }

    #[test]
    fn max_size_rejects_sprites_that_cannot_fit() {
        let source = SourceSprite {
            name: "wide".to_string(),
            path: PathBuf::from("wide.png"),
            image: RgbaImage::from_pixel(9, 4, Rgba([255, 0, 0, 255])),
            original_width: 9,
            original_height: 4,
            trim_x: 0,
            trim_y: 0,
            trimmed: false,
        };

        let error = pack_sprites(&[source], 0, true, PackingStrategy::MaxRects, 8, false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("--max-size 8"));
    }

    #[test]
    fn shelf_strategy_wraps_rows_deterministically() {
        let sources = ["a", "b"]
            .into_iter()
            .map(|name| SourceSprite {
                name: name.to_string(),
                path: PathBuf::from(format!("{name}.png")),
                image: RgbaImage::from_pixel(4, 4, Rgba([255, 0, 0, 255])),
                original_width: 4,
                original_height: 4,
                trim_x: 0,
                trim_y: 0,
                trimmed: false,
            })
            .collect::<Vec<_>>();

        let packed = try_pack(&sources, 5, 8, 0, PackingStrategy::Shelf, false).unwrap();
        assert_eq!(
            packed
                .iter()
                .map(|sprite| (sprite.source.name.as_str(), sprite.x, sprite.y))
                .collect::<Vec<_>>(),
            vec![("a", 0, 0), ("b", 0, 4)]
        );
    }

    #[test]
    fn guillotine_strategy_splits_free_space_deterministically() {
        let sources = [("wide", 6, 2), ("tall", 2, 6), ("small", 2, 2)]
            .into_iter()
            .map(|(name, width, height)| SourceSprite {
                name: name.to_string(),
                path: PathBuf::from(format!("{name}.png")),
                image: RgbaImage::from_pixel(width, height, Rgba([255, 0, 0, 255])),
                original_width: width,
                original_height: height,
                trim_x: 0,
                trim_y: 0,
                trimmed: false,
            })
            .collect::<Vec<_>>();

        let packed = try_pack(&sources, 8, 8, 0, PackingStrategy::Guillotine, false).unwrap();
        assert_eq!(
            packed
                .iter()
                .map(|sprite| (sprite.source.name.as_str(), sprite.x, sprite.y))
                .collect::<Vec<_>>(),
            vec![("wide", 0, 0), ("tall", 0, 2), ("small", 6, 0)]
        );
    }

    #[test]
    fn quadtree_strategy_splits_leaf_nodes_deterministically() {
        let sources = [("big", 4, 4), ("right", 2, 2), ("down", 2, 2)]
            .into_iter()
            .map(|(name, width, height)| SourceSprite {
                name: name.to_string(),
                path: PathBuf::from(format!("{name}.png")),
                image: RgbaImage::from_pixel(width, height, Rgba([255, 0, 0, 255])),
                original_width: width,
                original_height: height,
                trim_x: 0,
                trim_y: 0,
                trimmed: false,
            })
            .collect::<Vec<_>>();

        let packed = try_pack(&sources, 6, 6, 0, PackingStrategy::Quadtree, false).unwrap();
        assert_eq!(
            packed
                .iter()
                .map(|sprite| (sprite.source.name.as_str(), sprite.x, sprite.y))
                .collect::<Vec<_>>(),
            vec![("big", 0, 0), ("right", 4, 0), ("down", 4, 2)]
        );
    }

    #[test]
    fn rotated_packed_sprite_round_trips_upright_pixels() {
        let mut image = RgbaImage::from_pixel(2, 5, Rgba([0, 0, 0, 0]));
        image.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
        image.put_pixel(1, 4, Rgba([0, 255, 0, 255]));
        let source = SourceSprite {
            name: "pillar".to_string(),
            path: PathBuf::from("pillar.png"),
            image: image.clone(),
            original_width: 2,
            original_height: 5,
            trim_x: 0,
            trim_y: 0,
            trimmed: false,
        };

        let packed = try_pack(
            std::slice::from_ref(&source),
            5,
            2,
            0,
            PackingStrategy::MaxRects,
            true,
        )
        .unwrap();
        assert!(packed[0].rotated);
        let sheet = render_sheet(&packed, 5, 2);
        let options = PackOptions {
            rotate: true,
            ..PackOptions::default()
        };
        let frame = frame_from_source(
            &source,
            packed[0].x,
            packed[0].y,
            packed[0].rotated,
            &options,
        );
        assert!(frame.rotated);
        assert_eq!((frame.frame.w, frame.frame.h), (5, 2));
        assert_eq!(
            (frame.sprite_source_size.w, frame.sprite_source_size.h),
            (2, 5)
        );
        assert_eq!(extract_frame_image(&sheet, &frame), image);
    }

    #[test]
    fn pack_is_byte_deterministic() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input");
        let out_a = temp.path().join("a");
        let out_b = temp.path().join("b");
        fs::create_dir_all(&input).unwrap();
        write_test_sprite(&input.join("hero_idle_1.png"), [0, 255, 0, 255], 8, 6);
        write_test_sprite(&input.join("hero_idle_0.png"), [255, 0, 0, 255], 6, 8);
        write_test_sprite(&input.join("hero_run_0.png"), [0, 0, 255, 255], 5, 5);

        let options_a = PackOptions {
            input: input.clone(),
            output: out_a.clone(),
            format: OutputFormat::Png,
            packing_strategy: PackingStrategy::MaxRects,
            pot: true,
            padding: 2,
            max_size: DEFAULT_MAX_SIZE,
            rotate: false,
            trim: true,
            dry_run: false,
            watch: false,
            slice_mode: None,
            cell: None,
            prefix: "frame".to_string(),
            aseprite_json: false,
            godot_spriteframes: false,
            unity_importer: false,
            animation_regex: None,
            animations: Vec::new(),
            stable_pivot: false,
            dedup_threshold: None,
            linked_channels: Vec::new(),
            normalize_generated: false,
            frame_size: None,
        };
        let options_b = PackOptions {
            output: out_b.clone(),
            ..options_a.clone()
        };
        pack(options_a).unwrap();
        pack(options_b).unwrap();

        assert_eq!(
            fs::read(out_a.join("atlas.json")).unwrap(),
            fs::read(out_b.join("atlas.json")).unwrap()
        );
        assert_eq!(
            fs::read(out_a.join("sheet.png")).unwrap(),
            fs::read(out_b.join("sheet.png")).unwrap()
        );
    }

    #[test]
    fn pack_skips_rewriting_identical_outputs() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input");
        let output = temp.path().join("out");
        fs::create_dir_all(&input).unwrap();
        write_test_sprite(&input.join("hero_idle_0.png"), [255, 0, 0, 255], 4, 4);

        let options = PackOptions {
            input,
            output: output.clone(),
            format: OutputFormat::Png,
            packing_strategy: PackingStrategy::MaxRects,
            pot: true,
            padding: 2,
            max_size: DEFAULT_MAX_SIZE,
            rotate: false,
            trim: true,
            dry_run: false,
            watch: false,
            slice_mode: None,
            cell: None,
            prefix: "frame".to_string(),
            aseprite_json: false,
            godot_spriteframes: false,
            unity_importer: false,
            animation_regex: None,
            animations: Vec::new(),
            stable_pivot: false,
            dedup_threshold: None,
            linked_channels: Vec::new(),
            normalize_generated: false,
            frame_size: None,
        };
        pack(options.clone()).unwrap();
        let sheet_mtime = fs::metadata(output.join("sheet.png"))
            .unwrap()
            .modified()
            .unwrap();
        let atlas_mtime = fs::metadata(output.join("atlas.json"))
            .unwrap()
            .modified()
            .unwrap();
        std::thread::sleep(Duration::from_millis(20));
        pack(options).unwrap();

        assert_eq!(
            fs::metadata(output.join("sheet.png"))
                .unwrap()
                .modified()
                .unwrap(),
            sheet_mtime
        );
        assert_eq!(
            fs::metadata(output.join("atlas.json"))
                .unwrap()
                .modified()
                .unwrap(),
            atlas_mtime
        );
    }

    #[test]
    fn golden_basic_fixture_matches_committed_outputs() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input");
        let output = temp.path().join("out");
        fs::create_dir_all(&input).unwrap();
        write_precise_sprite(
            &input.join("hero_idle_0.png"),
            10,
            10,
            Rect {
                x: 2,
                y: 3,
                w: 5,
                h: 4,
            },
            [255, 0, 0, 255],
        );
        write_precise_sprite(
            &input.join("hero_idle_1.png"),
            10,
            10,
            Rect {
                x: 1,
                y: 2,
                w: 6,
                h: 5,
            },
            [0, 255, 0, 255],
        );
        write_precise_sprite(
            &input.join("hero_run_0.png"),
            8,
            8,
            Rect {
                x: 2,
                y: 2,
                w: 4,
                h: 4,
            },
            [0, 0, 255, 255],
        );

        pack(PackOptions {
            input,
            output: output.clone(),
            format: OutputFormat::Png,
            packing_strategy: PackingStrategy::MaxRects,
            pot: true,
            padding: 2,
            max_size: DEFAULT_MAX_SIZE,
            rotate: false,
            trim: true,
            dry_run: false,
            watch: false,
            slice_mode: None,
            cell: None,
            prefix: "frame".to_string(),
            aseprite_json: false,
            godot_spriteframes: false,
            unity_importer: false,
            animation_regex: None,
            animations: Vec::new(),
            stable_pivot: false,
            dedup_threshold: None,
            linked_channels: Vec::new(),
            normalize_generated: false,
            frame_size: None,
        })
        .unwrap();

        let expected_atlas = include_str!("../../tests/golden/basic/atlas.json");
        let actual_atlas = fs::read_to_string(output.join("atlas.json")).unwrap();
        let expected_atlas = expected_atlas.replace("\r\n", "\n");
        let actual_atlas = actual_atlas.replace("\r\n", "\n");
        assert_eq!(
            actual_atlas.trim_end_matches('\n'),
            expected_atlas.trim_end_matches('\n')
        );

        let expected_hash = include_str!("../../tests/golden/basic/sheet.sha256").trim();
        let actual_hash = sha256_hex(&fs::read(output.join("sheet.png")).unwrap());
        assert_eq!(actual_hash, expected_hash);
    }

    #[test]
    fn export_gif_writes_animation_frames_from_atlas() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input");
        let output = temp.path().join("out");
        let gif_path = temp.path().join("hero_idle.gif");
        fs::create_dir_all(&input).unwrap();
        write_test_sprite(&input.join("hero_idle_0.png"), [255, 0, 0, 255], 4, 4);
        write_test_sprite(&input.join("hero_idle_1.png"), [0, 255, 0, 255], 4, 4);

        pack(PackOptions {
            input,
            output: output.clone(),
            format: OutputFormat::Png,
            packing_strategy: PackingStrategy::MaxRects,
            pot: true,
            padding: 2,
            max_size: DEFAULT_MAX_SIZE,
            rotate: false,
            trim: true,
            dry_run: false,
            watch: false,
            slice_mode: None,
            cell: None,
            prefix: "frame".to_string(),
            aseprite_json: false,
            godot_spriteframes: false,
            unity_importer: false,
            animation_regex: None,
            animations: Vec::new(),
            stable_pivot: false,
            dedup_threshold: None,
            linked_channels: Vec::new(),
            normalize_generated: false,
            frame_size: None,
        })
        .unwrap();

        let report = export_gif(
            &output.join("atlas.json"),
            &output.join("sheet.png"),
            &gif_path,
            "hero_idle",
            12.0,
            None,
            None,
        )
        .unwrap();
        assert_eq!(report.frame_count, 2);
        assert!(gif_path.exists());

        let file = fs::File::open(gif_path).unwrap();
        let decoder = image::codecs::gif::GifDecoder::new(BufReader::new(file)).unwrap();
        let frames = decoder.into_frames().collect_frames().unwrap();
        assert_eq!(frames.len(), 2);
    }

    #[test]
    fn export_gif_can_limit_animation_frame_range() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input");
        let output = temp.path().join("out");
        let gif_path = temp.path().join("hero_idle_range.gif");
        fs::create_dir_all(&input).unwrap();
        write_test_sprite(&input.join("hero_idle_0.png"), [255, 0, 0, 255], 4, 4);
        write_test_sprite(&input.join("hero_idle_1.png"), [0, 255, 0, 255], 4, 4);
        write_test_sprite(&input.join("hero_idle_2.png"), [0, 0, 255, 255], 4, 4);

        pack(PackOptions {
            input,
            output: output.clone(),
            format: OutputFormat::Png,
            packing_strategy: PackingStrategy::MaxRects,
            pot: true,
            padding: 2,
            max_size: DEFAULT_MAX_SIZE,
            rotate: false,
            trim: true,
            dry_run: false,
            watch: false,
            slice_mode: None,
            cell: None,
            prefix: "frame".to_string(),
            aseprite_json: false,
            godot_spriteframes: false,
            unity_importer: false,
            animation_regex: None,
            animations: Vec::new(),
            stable_pivot: false,
            dedup_threshold: None,
            linked_channels: Vec::new(),
            normalize_generated: false,
            frame_size: None,
        })
        .unwrap();

        let report = export_gif(
            &output.join("atlas.json"),
            &output.join("sheet.png"),
            &gif_path,
            "hero_idle",
            0.0,
            Some(1),
            Some(1),
        )
        .unwrap();
        assert_eq!(report.frame_count, 1);
    }

    #[test]
    fn trim_metadata_recomposes_original() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input");
        let output = temp.path().join("out");
        fs::create_dir_all(&input).unwrap();
        let mut original = RgbaImage::from_pixel(8, 8, Rgba([0, 0, 0, 0]));
        for y in 3..6 {
            for x in 2..7 {
                original.put_pixel(x, y, Rgba([17, 34, 51, 255]));
            }
        }
        original.save(input.join("hero_idle_0.png")).unwrap();
        pack(PackOptions {
            input,
            output: output.clone(),
            format: OutputFormat::Png,
            packing_strategy: PackingStrategy::MaxRects,
            pot: true,
            padding: 2,
            max_size: DEFAULT_MAX_SIZE,
            rotate: false,
            trim: true,
            dry_run: false,
            watch: false,
            slice_mode: None,
            cell: None,
            prefix: "frame".to_string(),
            aseprite_json: false,
            godot_spriteframes: false,
            unity_importer: false,
            animation_regex: None,
            animations: Vec::new(),
            stable_pivot: false,
            dedup_threshold: None,
            linked_channels: Vec::new(),
            normalize_generated: false,
            frame_size: None,
        })
        .unwrap();

        let atlas: Atlas =
            serde_json::from_slice(&fs::read(output.join("atlas.json")).unwrap()).unwrap();
        let sheet = image::open(output.join("sheet.png")).unwrap().to_rgba8();
        let frame = &atlas.frames[0];
        let mut recomposed =
            RgbaImage::from_pixel(frame.source_size.w, frame.source_size.h, Rgba([0, 0, 0, 0]));
        let cropped = sheet
            .view(frame.frame.x, frame.frame.y, frame.frame.w, frame.frame.h)
            .to_image();
        image::imageops::overlay(
            &mut recomposed,
            &cropped,
            frame.sprite_source_size.x.into(),
            frame.sprite_source_size.y.into(),
        );
        assert_eq!(original.as_raw(), recomposed.as_raw());
    }

    #[test]
    fn padding_pixels_stay_transparent() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input");
        let output = temp.path().join("out");
        fs::create_dir_all(&input).unwrap();
        write_test_sprite(&input.join("a_0.png"), [255, 0, 0, 255], 8, 8);
        write_test_sprite(&input.join("b_0.png"), [0, 0, 255, 255], 8, 8);
        pack(PackOptions {
            input,
            output: output.clone(),
            format: OutputFormat::Png,
            packing_strategy: PackingStrategy::MaxRects,
            pot: true,
            padding: 2,
            max_size: DEFAULT_MAX_SIZE,
            rotate: false,
            trim: true,
            dry_run: false,
            watch: false,
            slice_mode: None,
            cell: None,
            prefix: "frame".to_string(),
            aseprite_json: false,
            godot_spriteframes: false,
            unity_importer: false,
            animation_regex: None,
            animations: Vec::new(),
            stable_pivot: false,
            dedup_threshold: None,
            linked_channels: Vec::new(),
            normalize_generated: false,
            frame_size: None,
        })
        .unwrap();

        let atlas: Atlas =
            serde_json::from_slice(&fs::read(output.join("atlas.json")).unwrap()).unwrap();
        let sheet = image::open(output.join("sheet.png")).unwrap().to_rgba8();
        for frame in atlas.frames {
            let left = frame.frame.x - 1;
            let top = frame.frame.y - 1;
            assert_eq!(sheet.get_pixel(left, frame.frame.y)[3], 0);
            assert_eq!(sheet.get_pixel(frame.frame.x, top)[3], 0);
        }
    }

    #[test]
    fn bilinear_edge_samples_do_not_pick_up_neighbor_color() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input");
        let output = temp.path().join("out");
        fs::create_dir_all(&input).unwrap();
        write_test_sprite(&input.join("a_0.png"), [255, 0, 0, 255], 8, 8);
        write_test_sprite(&input.join("b_0.png"), [0, 0, 255, 255], 8, 8);
        pack(PackOptions {
            input,
            output: output.clone(),
            format: OutputFormat::Png,
            packing_strategy: PackingStrategy::MaxRects,
            pot: true,
            padding: 2,
            max_size: DEFAULT_MAX_SIZE,
            rotate: false,
            trim: true,
            dry_run: false,
            watch: false,
            slice_mode: None,
            cell: None,
            prefix: "frame".to_string(),
            aseprite_json: false,
            godot_spriteframes: false,
            unity_importer: false,
            animation_regex: None,
            animations: Vec::new(),
            stable_pivot: false,
            dedup_threshold: None,
            linked_channels: Vec::new(),
            normalize_generated: false,
            frame_size: None,
        })
        .unwrap();

        let atlas: Atlas =
            serde_json::from_slice(&fs::read(output.join("atlas.json")).unwrap()).unwrap();
        let sheet = image::open(output.join("sheet.png")).unwrap().to_rgba8();
        for frame in atlas.frames {
            let left = bilinear_sample(
                &sheet,
                frame.frame.x as f32 - 0.5,
                frame.frame.y as f32 + 3.5,
            );
            let right = bilinear_sample(
                &sheet,
                (frame.frame.x + frame.frame.w) as f32 - 0.5,
                frame.frame.y as f32 + 3.5,
            );
            let top = bilinear_sample(
                &sheet,
                frame.frame.x as f32 + 3.5,
                frame.frame.y as f32 - 0.5,
            );
            let bottom = bilinear_sample(
                &sheet,
                frame.frame.x as f32 + 3.5,
                (frame.frame.y + frame.frame.h) as f32 - 0.5,
            );
            for sample in [left, right, top, bottom] {
                assert_eq!(sample[3], 128);
                assert!(
                    sample[0] == 128 || sample[2] == 128,
                    "edge sample should blend only the frame color with transparent padding"
                );
                assert_eq!(sample[1], 0);
            }
        }
    }

    #[test]
    fn project_file_supplies_defaults() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input");
        let output = temp.path().join("out");
        fs::create_dir_all(&input).unwrap();
        write_test_sprite(&input.join("hero_idle_0.png"), [255, 0, 0, 255], 3, 4);
        fs::write(
            temp.path().join("hero.atlasproj.json"),
            r#"{
              "sourceFolder": "input",
              "outputPath": "out",
              "format": "png",
              "pot": false,
              "padding": 1,
              "trim": true,
              "packingStrategy": "maxrects"
            }"#,
        )
        .unwrap();

        let options = parse_args(vec![
            "sprite-atlas-packer".to_string(),
            "--headless".to_string(),
            "--project".to_string(),
            temp.path()
                .join("hero.atlasproj.json")
                .to_string_lossy()
                .to_string(),
        ])
        .unwrap();
        pack(options).unwrap();
        assert!(output.join("sheet.png").exists());
        assert!(output.join("atlas.json").exists());
    }

    #[test]
    fn project_file_round_trips_options() {
        let temp = TempDir::new().unwrap();
        let project_path = temp.path().join("roundtrip.atlasproj.json");
        let options = PackOptions {
            input: PathBuf::from("sprites"),
            output: PathBuf::from("build/atlas"),
            format: OutputFormat::Both,
            packing_strategy: PackingStrategy::MaxRects,
            pot: false,
            padding: 3,
            max_size: DEFAULT_MAX_SIZE,
            rotate: false,
            trim: false,
            dry_run: false,
            watch: false,
            slice_mode: None,
            cell: None,
            prefix: "frame".to_string(),
            aseprite_json: true,
            godot_spriteframes: true,
            unity_importer: true,
            animation_regex: Some(r"^(?P<name>.+)-(?P<index>\d+)$".to_string()),
            animations: vec![AnimationDefinition {
                name: "idle".to_string(),
                frames: vec!["hero_idle_0".to_string()],
                looped: false,
                fps_hint: 12.0,
                pivot: Some(Pivot { x: 0.25, y: 0.75 }),
                duration_ms: Some(80),
                frame_overrides: BTreeMap::from([(
                    "hero_idle_0".to_string(),
                    FrameOverride {
                        pivot: Some(Pivot { x: 0.3, y: 0.7 }),
                        duration_ms: Some(110),
                    },
                )]),
            }],
            stable_pivot: true,
            dedup_threshold: Some(3),
            linked_channels: vec!["_n".to_string()],
            normalize_generated: false,
            frame_size: None,
        };

        write_project_file(&project_path, &options).unwrap();
        let project = read_project_file(&project_path).unwrap();
        assert_eq!(project.source_folder.unwrap(), PathBuf::from("sprites"));
        assert_eq!(project.output_path.unwrap(), PathBuf::from("build/atlas"));
        assert_eq!(project.format.unwrap(), "both");
        assert_eq!(project.pot, Some(false));
        assert_eq!(project.padding, Some(3));
        assert_eq!(project.trim, Some(false));
        assert_eq!(project.aseprite_json, Some(true));
        assert_eq!(project.godot_spriteframes, Some(true));
        assert_eq!(project.unity_importer, Some(true));
        assert_eq!(
            project.animation_regex.unwrap(),
            r"^(?P<name>.+)-(?P<index>\d+)$"
        );
        assert_eq!(project.animations[0].name, "idle");
        assert_eq!(project.animations[0].looped, false);
        assert_eq!(project.animations[0].fps_hint, 12.0);
        assert_eq!(project.animations[0].duration_ms, Some(80));
        assert_eq!(
            project.animations[0].frame_overrides["hero_idle_0"].duration_ms,
            Some(110)
        );
        assert_eq!(project.stable_pivot, Some(true));
        assert_eq!(project.dedup_threshold, Some(3));
        assert_eq!(project.linked_channels, vec!["_n"]);
        assert_eq!(project.packing_strategy.unwrap(), "maxrects");
    }

    #[test]
    fn linked_channel_sheets_follow_color_layout() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input");
        let output = temp.path().join("out");
        fs::create_dir_all(&input).unwrap();
        write_precise_sprite(
            &input.join("hero_idle_0.png"),
            8,
            8,
            Rect {
                x: 2,
                y: 1,
                w: 4,
                h: 5,
            },
            [255, 0, 0, 255],
        );
        write_precise_sprite(
            &input.join("hero_idle_0_n.png"),
            8,
            8,
            Rect {
                x: 2,
                y: 1,
                w: 4,
                h: 5,
            },
            [64, 128, 255, 255],
        );

        let report = pack(PackOptions {
            input,
            output: output.clone(),
            format: OutputFormat::Png,
            packing_strategy: PackingStrategy::MaxRects,
            pot: true,
            padding: 2,
            max_size: DEFAULT_MAX_SIZE,
            rotate: false,
            trim: true,
            dry_run: false,
            watch: false,
            slice_mode: None,
            cell: None,
            prefix: "frame".to_string(),
            aseprite_json: false,
            godot_spriteframes: false,
            unity_importer: false,
            animation_regex: None,
            animations: Vec::new(),
            stable_pivot: false,
            dedup_threshold: None,
            linked_channels: vec!["_n".to_string()],
            normalize_generated: false,
            frame_size: None,
        })
        .unwrap();

        assert_eq!(report.input_count, 1);
        assert_eq!(report.frames, vec!["hero_idle_0"]);
        assert!(report.wrote.contains(&"sheet_n.png".to_string()));
        let frame = &report.atlas.frames[0].frame;
        let normal_sheet = image::open(output.join("sheet_n.png")).unwrap().to_rgba8();
        assert_eq!(
            normal_sheet.get_pixel(frame.x, frame.y),
            &Rgba([64, 128, 255, 255])
        );
        assert_eq!(
            normal_sheet.get_pixel(frame.x + frame.w - 1, frame.y + frame.h - 1),
            &Rgba([64, 128, 255, 255])
        );
    }

    #[test]
    fn watch_flag_is_parsed() {
        let options = parse_args(vec![
            "sprite-atlas-packer".to_string(),
            "--headless".to_string(),
            "--input".to_string(),
            "sprites".to_string(),
            "--output".to_string(),
            "build/atlas".to_string(),
            "--watch".to_string(),
        ])
        .unwrap();
        assert!(options.watch);
    }

    #[test]
    fn max_size_flag_is_parsed() {
        let options = parse_args(vec![
            "sprite-atlas-packer".to_string(),
            "--headless".to_string(),
            "--input".to_string(),
            "sprites".to_string(),
            "--output".to_string(),
            "build/atlas".to_string(),
            "--max-size".to_string(),
            "1024".to_string(),
        ])
        .unwrap();
        assert_eq!(options.max_size, 1024);
    }

    #[test]
    fn rotate_flag_is_parsed() {
        let options = parse_args(vec![
            "sprite-atlas-packer".to_string(),
            "--headless".to_string(),
            "--input".to_string(),
            "sprites".to_string(),
            "--output".to_string(),
            "build/atlas".to_string(),
            "--rotate".to_string(),
        ])
        .unwrap();
        assert!(options.rotate);
    }

    #[test]
    fn shelf_strategy_flag_is_parsed() {
        let options = parse_args(vec![
            "sprite-atlas-packer".to_string(),
            "--headless".to_string(),
            "--input".to_string(),
            "sprites".to_string(),
            "--output".to_string(),
            "build/atlas".to_string(),
            "--algo".to_string(),
            "shelf".to_string(),
        ])
        .unwrap();
        assert_eq!(options.packing_strategy, PackingStrategy::Shelf);
    }

    #[test]
    fn guillotine_strategy_flag_is_parsed() {
        let options = parse_args(vec![
            "sprite-atlas-packer".to_string(),
            "--headless".to_string(),
            "--input".to_string(),
            "sprites".to_string(),
            "--output".to_string(),
            "build/atlas".to_string(),
            "--algo".to_string(),
            "guillotine".to_string(),
        ])
        .unwrap();
        assert_eq!(options.packing_strategy, PackingStrategy::Guillotine);
    }

    #[test]
    fn quadtree_strategy_flag_is_parsed() {
        let options = parse_args(vec![
            "sprite-atlas-packer".to_string(),
            "--headless".to_string(),
            "--input".to_string(),
            "sprites".to_string(),
            "--output".to_string(),
            "build/atlas".to_string(),
            "--algo".to_string(),
            "quadtree".to_string(),
        ])
        .unwrap();
        assert_eq!(options.packing_strategy, PackingStrategy::Quadtree);
    }

    #[test]
    fn serve_flags_are_parsed() {
        let config = parse_serve_args(&[
            "sprite-atlas-packer".to_string(),
            "--headless".to_string(),
            "--serve".to_string(),
            "--host".to_string(),
            "0.0.0.0".to_string(),
            "--port".to_string(),
            "9999".to_string(),
            "--token".to_string(),
            "test-token".to_string(),
        ])
        .unwrap();
        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 9999);
        assert_eq!(config.token, "test-token");
    }

    #[test]
    fn serve_token_defaults_when_missing() {
        let config = parse_serve_args(&[
            "sprite-atlas-packer".to_string(),
            "--headless".to_string(),
            "--serve".to_string(),
        ])
        .unwrap();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 9877);
        assert!(config.token.starts_with("sap-"));
    }

    #[test]
    fn websocket_accept_key_matches_rfc_sample() {
        assert_eq!(
            websocket_accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn websocket_query_token_authorizes_events() {
        let request = HttpRequest {
            method: "GET".to_string(),
            path: "/api/v1/events".to_string(),
            query: "token=sap-local".to_string(),
            headers: BTreeMap::new(),
            body: Vec::new(),
        };

        assert!(is_authorized(&request, "sap-local"));
    }

    #[test]
    fn duplicate_frame_names_are_rejected() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input");
        fs::create_dir_all(input.join("a")).unwrap();
        fs::create_dir_all(input.join("b")).unwrap();
        write_test_sprite(&input.join("a/idle_0.png"), [255, 0, 0, 255], 2, 2);
        write_test_sprite(&input.join("b/idle_0.png"), [0, 255, 0, 255], 2, 2);

        let error = load_sources(&input, true).unwrap_err().to_string();
        assert!(error.contains("Duplicate frame name 'idle_0'"));
    }

    #[test]
    fn serialized_atlas_uses_texturepacker_field_names() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input");
        let output = temp.path().join("out");
        fs::create_dir_all(&input).unwrap();
        write_test_sprite(&input.join("hero_idle_0.png"), [255, 0, 0, 255], 4, 4);

        pack(PackOptions {
            input,
            output: output.clone(),
            format: OutputFormat::Png,
            packing_strategy: PackingStrategy::MaxRects,
            pot: true,
            padding: 2,
            max_size: DEFAULT_MAX_SIZE,
            rotate: false,
            trim: true,
            dry_run: false,
            watch: false,
            slice_mode: None,
            cell: None,
            prefix: "frame".to_string(),
            aseprite_json: false,
            godot_spriteframes: false,
            unity_importer: false,
            animation_regex: None,
            animations: Vec::new(),
            stable_pivot: false,
            dedup_threshold: None,
            linked_channels: Vec::new(),
            normalize_generated: false,
            frame_size: None,
        })
        .unwrap();

        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(output.join("atlas.json")).unwrap()).unwrap();
        assert_eq!(value["meta"]["app"], APP_NAME);
        assert_eq!(value["meta"]["image"], "sheet.png");
        assert!(value["frames"][0].get("spriteSourceSize").is_some());
        assert!(value["frames"][0].get("sourceSize").is_some());
        assert_eq!(value["frames"][0]["rotated"], false);
        assert_eq!(value["animations"][0]["loop"], true);
        assert!(value["animations"][0].get("looped").is_none());
    }

    #[test]
    fn project_animations_override_inferred_timing_and_pivot() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input");
        let output = temp.path().join("out");
        fs::create_dir_all(&input).unwrap();
        write_test_sprite(&input.join("hero_idle_0.png"), [255, 0, 0, 255], 4, 4);
        write_test_sprite(&input.join("hero_idle_1.png"), [0, 255, 0, 255], 4, 4);

        let mut options = PackOptions {
            input,
            output: output.clone(),
            format: OutputFormat::Png,
            packing_strategy: PackingStrategy::MaxRects,
            pot: true,
            padding: 2,
            max_size: DEFAULT_MAX_SIZE,
            rotate: false,
            trim: true,
            dry_run: false,
            watch: false,
            slice_mode: None,
            cell: None,
            prefix: "frame".to_string(),
            aseprite_json: false,
            godot_spriteframes: false,
            unity_importer: false,
            animation_regex: None,
            animations: Vec::new(),
            stable_pivot: false,
            dedup_threshold: None,
            linked_channels: Vec::new(),
            normalize_generated: false,
            frame_size: None,
        };
        options.animations.push(AnimationDefinition {
            name: "idle_custom".to_string(),
            frames: vec!["hero_idle_1".to_string(), "hero_idle_0".to_string()],
            looped: false,
            fps_hint: 10.0,
            pivot: Some(Pivot { x: 0.25, y: 0.8 }),
            duration_ms: Some(90),
            frame_overrides: BTreeMap::new(),
        });

        let report = pack(options).unwrap();
        assert_eq!(report.atlas.animations.len(), 1);
        assert_eq!(report.atlas.animations[0].name, "idle_custom");
        assert_eq!(
            report.atlas.animations[0].frames,
            vec!["hero_idle_1", "hero_idle_0"]
        );
        assert!(!report.atlas.animations[0].looped);
        assert_eq!(report.atlas.animations[0].fps_hint, 10.0);
        let frame = report
            .atlas
            .frames
            .iter()
            .find(|frame| frame.filename == "hero_idle_0")
            .unwrap();
        assert_eq!(frame.pivot.x, 0.25);
        assert_eq!(frame.pivot.y, 0.8);
        assert_eq!(frame.duration_ms, Some(90));
    }

    #[test]
    fn project_frame_overrides_win_over_animation_defaults() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input");
        let output = temp.path().join("out");
        fs::create_dir_all(&input).unwrap();
        write_test_sprite(&input.join("hero_idle_0.png"), [255, 0, 0, 255], 4, 4);
        write_test_sprite(&input.join("hero_idle_1.png"), [0, 255, 0, 255], 4, 4);

        let mut options = PackOptions {
            input,
            output,
            format: OutputFormat::Png,
            packing_strategy: PackingStrategy::MaxRects,
            pot: true,
            padding: 2,
            max_size: DEFAULT_MAX_SIZE,
            rotate: false,
            trim: true,
            dry_run: false,
            watch: false,
            slice_mode: None,
            cell: None,
            prefix: "frame".to_string(),
            aseprite_json: false,
            godot_spriteframes: false,
            unity_importer: false,
            animation_regex: None,
            animations: Vec::new(),
            stable_pivot: false,
            dedup_threshold: None,
            linked_channels: Vec::new(),
            normalize_generated: false,
            frame_size: None,
        };
        options.animations.push(AnimationDefinition {
            name: "idle_custom".to_string(),
            frames: vec!["hero_idle_0".to_string(), "hero_idle_1".to_string()],
            looped: true,
            fps_hint: 8.0,
            pivot: Some(Pivot { x: 0.5, y: 0.5 }),
            duration_ms: Some(120),
            frame_overrides: BTreeMap::from([(
                "hero_idle_1".to_string(),
                FrameOverride {
                    pivot: Some(Pivot { x: 0.2, y: 0.9 }),
                    duration_ms: Some(240),
                },
            )]),
        });

        let report = pack(options).unwrap();
        let by_name = report
            .atlas
            .frames
            .iter()
            .map(|frame| (frame.filename.as_str(), frame))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(by_name["hero_idle_0"].pivot.x, 0.5);
        assert_eq!(by_name["hero_idle_0"].duration_ms, Some(120));
        assert_eq!(by_name["hero_idle_1"].pivot.x, 0.2);
        assert_eq!(by_name["hero_idle_1"].pivot.y, 0.9);
        assert_eq!(by_name["hero_idle_1"].duration_ms, Some(240));
    }

    #[test]
    fn stable_pivot_preserves_original_canvas_anchor_for_trimmed_frames() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input");
        let output = temp.path().join("out");
        fs::create_dir_all(&input).unwrap();
        write_precise_sprite(
            &input.join("hero_idle_0.png"),
            10,
            10,
            Rect {
                x: 2,
                y: 3,
                w: 4,
                h: 4,
            },
            [255, 0, 0, 255],
        );
        write_precise_sprite(
            &input.join("hero_idle_1.png"),
            10,
            10,
            Rect {
                x: 4,
                y: 1,
                w: 4,
                h: 4,
            },
            [0, 255, 0, 255],
        );

        let report = pack(PackOptions {
            input,
            output,
            format: OutputFormat::Png,
            packing_strategy: PackingStrategy::MaxRects,
            pot: true,
            padding: 2,
            max_size: DEFAULT_MAX_SIZE,
            rotate: false,
            trim: true,
            dry_run: false,
            watch: false,
            slice_mode: None,
            cell: None,
            prefix: "frame".to_string(),
            aseprite_json: false,
            godot_spriteframes: false,
            unity_importer: false,
            animation_regex: None,
            animations: Vec::new(),
            stable_pivot: true,
            dedup_threshold: None,
            linked_channels: Vec::new(),
            normalize_generated: false,
            frame_size: None,
        })
        .unwrap();

        let by_name = report
            .atlas
            .frames
            .iter()
            .map(|frame| (frame.filename.as_str(), frame))
            .collect::<BTreeMap<_, _>>();
        let first = by_name["hero_idle_0"].pivot;
        let second = by_name["hero_idle_1"].pivot;
        assert_eq!(first.x, 0.75);
        assert_eq!(first.y, 0.5);
        assert_eq!(second.x, 0.25);
        assert_eq!(second.y, 1.0);
    }

    #[test]
    fn duplicate_frames_alias_to_one_packed_rect() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input");
        let output = temp.path().join("out");
        fs::create_dir_all(&input).unwrap();
        write_precise_sprite(
            &input.join("hero_blink_0.png"),
            6,
            6,
            Rect {
                x: 1,
                y: 1,
                w: 4,
                h: 4,
            },
            [255, 0, 0, 255],
        );
        write_precise_sprite(
            &input.join("hero_blink_1.png"),
            6,
            6,
            Rect {
                x: 1,
                y: 1,
                w: 4,
                h: 4,
            },
            [255, 0, 0, 255],
        );

        let report = pack(PackOptions {
            input,
            output,
            format: OutputFormat::Png,
            packing_strategy: PackingStrategy::MaxRects,
            pot: true,
            padding: 2,
            max_size: DEFAULT_MAX_SIZE,
            rotate: false,
            trim: true,
            dry_run: false,
            watch: false,
            slice_mode: None,
            cell: None,
            prefix: "frame".to_string(),
            aseprite_json: false,
            godot_spriteframes: false,
            unity_importer: false,
            animation_regex: None,
            animations: Vec::new(),
            stable_pivot: false,
            dedup_threshold: Some(0),
            linked_channels: Vec::new(),
            normalize_generated: false,
            frame_size: None,
        })
        .unwrap();

        assert_eq!(report.input_count, 2);
        assert_eq!(report.frames.len(), 2);
        assert_eq!(report.aliases.len(), 1);
        assert_eq!(report.aliases[0].frame, "hero_blink_1");
        assert_eq!(report.aliases[0].target, "hero_blink_0");
        let by_name = report
            .atlas
            .frames
            .iter()
            .map(|frame| (frame.filename.as_str(), frame.frame))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(by_name["hero_blink_0"], by_name["hero_blink_1"]);
    }

    #[test]
    fn linked_channel_difference_prevents_aliasing() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input");
        let output = temp.path().join("out");
        fs::create_dir_all(&input).unwrap();
        for name in ["hero_blink_0.png", "hero_blink_1.png"] {
            write_precise_sprite(
                &input.join(name),
                6,
                6,
                Rect {
                    x: 1,
                    y: 1,
                    w: 4,
                    h: 4,
                },
                [255, 0, 0, 255],
            );
        }
        write_precise_sprite(
            &input.join("hero_blink_0_n.png"),
            6,
            6,
            Rect {
                x: 1,
                y: 1,
                w: 4,
                h: 4,
            },
            [64, 128, 255, 255],
        );
        write_precise_sprite(
            &input.join("hero_blink_1_n.png"),
            6,
            6,
            Rect {
                x: 1,
                y: 1,
                w: 4,
                h: 4,
            },
            [128, 64, 255, 255],
        );

        let report = pack(PackOptions {
            input,
            output: output.clone(),
            format: OutputFormat::Png,
            packing_strategy: PackingStrategy::MaxRects,
            pot: true,
            padding: 2,
            max_size: DEFAULT_MAX_SIZE,
            rotate: false,
            trim: true,
            dry_run: false,
            watch: false,
            slice_mode: None,
            cell: None,
            prefix: "frame".to_string(),
            aseprite_json: false,
            godot_spriteframes: false,
            unity_importer: false,
            animation_regex: None,
            animations: Vec::new(),
            stable_pivot: false,
            dedup_threshold: Some(0),
            linked_channels: vec!["_n".to_string()],
            normalize_generated: false,
            frame_size: None,
        })
        .unwrap();

        assert_eq!(report.input_count, 2);
        assert!(report.aliases.is_empty());
        assert!(output.join("sheet_n.png").exists());
        let by_name = report
            .atlas
            .frames
            .iter()
            .map(|frame| (frame.filename.as_str(), frame.frame))
            .collect::<BTreeMap<_, _>>();
        assert_ne!(by_name["hero_blink_0"], by_name["hero_blink_1"]);
    }

    #[test]
    fn near_duplicate_threshold_controls_aliasing() {
        let mut first = RgbaImage::from_pixel(2, 2, Rgba([10, 20, 30, 255]));
        let mut second = first.clone();
        second.put_pixel(1, 1, Rgba([11, 20, 30, 255]));
        let source = SourceSprite {
            name: "a".to_string(),
            path: PathBuf::from("a.png"),
            image: first.clone(),
            original_width: 2,
            original_height: 2,
            trim_x: 0,
            trim_y: 0,
            trimmed: false,
        };
        let near = SourceSprite {
            name: "b".to_string(),
            path: PathBuf::from("b.png"),
            image: second,
            original_width: 2,
            original_height: 2,
            trim_x: 0,
            trim_y: 0,
            trimmed: false,
        };
        assert!(!can_alias_source(&near, &source, 0));
        assert!(can_alias_source(&near, &source, 1));
        first.put_pixel(0, 0, Rgba([10, 20, 30, 254]));
        let alpha_changed = SourceSprite {
            image: first,
            ..source
        };
        assert!(can_alias_source(&alpha_changed, &near, 1));
    }

    #[test]
    fn aseprite_json_sidecar_uses_aseprite_field_names() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input");
        let output = temp.path().join("out");
        fs::create_dir_all(&input).unwrap();
        write_test_sprite(&input.join("hero_idle_0.png"), [255, 0, 0, 255], 4, 4);
        write_test_sprite(&input.join("hero_idle_1.png"), [0, 255, 0, 255], 4, 4);

        pack(PackOptions {
            input,
            output: output.clone(),
            format: OutputFormat::Png,
            packing_strategy: PackingStrategy::MaxRects,
            pot: true,
            padding: 2,
            max_size: DEFAULT_MAX_SIZE,
            rotate: false,
            trim: true,
            dry_run: false,
            watch: false,
            slice_mode: None,
            cell: None,
            prefix: "frame".to_string(),
            aseprite_json: true,
            godot_spriteframes: false,
            unity_importer: false,
            animation_regex: None,
            animations: Vec::new(),
            stable_pivot: false,
            dedup_threshold: None,
            linked_channels: Vec::new(),
            normalize_generated: false,
            frame_size: None,
        })
        .unwrap();

        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(output.join("aseprite.json")).unwrap()).unwrap();
        assert_eq!(value["meta"]["image"], "sheet.png");
        assert_eq!(value["frames"][0]["duration"], DEFAULT_DURATION_MS);
        assert!(value["frames"][0].get("duration_ms").is_none());
        assert_eq!(value["meta"]["frameTags"][0]["name"], "hero_idle");
        assert_eq!(value["meta"]["frameTags"][0]["from"], 0);
        assert_eq!(value["meta"]["frameTags"][0]["to"], 1);
        assert_eq!(value["meta"]["frameTags"][0]["direction"], "forward");
    }

    #[test]
    fn native_engine_exports_write_godot_and_unity_artifacts() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input");
        let output = temp.path().join("out");
        fs::create_dir_all(&input).unwrap();
        write_test_sprite(&input.join("hero_idle_0.png"), [255, 0, 0, 255], 4, 4);
        write_test_sprite(&input.join("hero_idle_1.png"), [0, 255, 0, 255], 4, 4);

        let report = pack(PackOptions {
            input,
            output: output.clone(),
            format: OutputFormat::Png,
            packing_strategy: PackingStrategy::MaxRects,
            pot: true,
            padding: 2,
            max_size: DEFAULT_MAX_SIZE,
            rotate: false,
            trim: true,
            dry_run: false,
            watch: false,
            slice_mode: None,
            cell: None,
            prefix: "frame".to_string(),
            aseprite_json: false,
            godot_spriteframes: true,
            unity_importer: true,
            animation_regex: None,
            animations: Vec::new(),
            stable_pivot: false,
            dedup_threshold: None,
            linked_channels: Vec::new(),
            normalize_generated: false,
            frame_size: None,
        })
        .unwrap();

        assert!(report
            .wrote
            .contains(&"godot_spriteframes.tres".to_string()));
        assert!(report
            .wrote
            .contains(&"SpriteAtlasPackerImporter.cs".to_string()));
        let godot = fs::read_to_string(output.join("godot_spriteframes.tres")).unwrap();
        assert!(godot.contains("[gd_resource type=\"SpriteFrames\""));
        assert!(godot.contains("res://sheet.png"));
        assert!(godot.contains("AtlasTexture_hero_idle_0"));
        let unity = fs::read_to_string(output.join("SpriteAtlasPackerImporter.cs")).unwrap();
        assert!(unity.contains("SpriteAtlasPackerImporter"));
        assert!(unity.contains("SpriteMetaData"));
        assert!(unity.contains("atlas.json"));
    }

    #[test]
    fn grid_slice_writes_frames_and_atlas() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("sheet.png");
        let output = temp.path().join("out");
        let mut sheet = RgbaImage::from_pixel(8, 4, Rgba([0, 0, 0, 0]));
        for y in 0..4 {
            for x in 0..4 {
                sheet.put_pixel(x, y, Rgba([255, 0, 0, 255]));
            }
            for x in 4..8 {
                sheet.put_pixel(x, y, Rgba([0, 0, 255, 255]));
            }
        }
        sheet.save(&input).unwrap();

        let report = slice_sheet(PackOptions {
            input,
            output: output.clone(),
            format: OutputFormat::Png,
            packing_strategy: PackingStrategy::MaxRects,
            pot: true,
            padding: 2,
            max_size: DEFAULT_MAX_SIZE,
            rotate: false,
            trim: true,
            dry_run: false,
            watch: false,
            slice_mode: Some(SliceMode::Grid),
            cell: Some((4, 4)),
            prefix: "tile".to_string(),
            aseprite_json: false,
            godot_spriteframes: false,
            unity_importer: false,
            animation_regex: None,
            animations: Vec::new(),
            stable_pivot: false,
            dedup_threshold: None,
            linked_channels: Vec::new(),
            normalize_generated: false,
            frame_size: None,
        })
        .unwrap();

        assert_eq!(report.frame_count, 2);
        assert_eq!(report.atlas.frames[1].frame.x, 4);
        assert!(output.join("atlas.json").exists());
        assert!(output.join("frames/tile_0000.png").exists());
        assert!(output.join("frames/tile_0001.png").exists());
    }

    #[test]
    fn generated_grid_slice_normalizes_frames_and_writes_manifest() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("generated.png");
        let output = temp.path().join("normalized");
        let mut sheet = RgbaImage::from_pixel(64, 64, Rgba([0, 255, 0, 255]));
        for row in 0..2 {
            for column in 0..2 {
                let origin_x = column * 32;
                let origin_y = row * 32;
                let offset = row + column;
                for y in origin_y + 5 + offset..origin_y + 27 + offset {
                    for x in origin_x + 9 - offset..origin_x + 23 - offset {
                        sheet.put_pixel(x, y, Rgba([220, 40, 64, 255]));
                    }
                }
            }
        }
        sheet.save(&input).unwrap();

        let report = slice_sheet(PackOptions {
            input,
            output: output.clone(),
            format: OutputFormat::Png,
            packing_strategy: PackingStrategy::MaxRects,
            pot: true,
            padding: 2,
            max_size: DEFAULT_MAX_SIZE,
            rotate: false,
            trim: true,
            dry_run: false,
            watch: false,
            slice_mode: Some(SliceMode::Grid),
            cell: Some((32, 32)),
            prefix: "walk".to_string(),
            aseprite_json: false,
            godot_spriteframes: false,
            unity_importer: false,
            animation_regex: None,
            animations: Vec::new(),
            stable_pivot: false,
            dedup_threshold: None,
            linked_channels: Vec::new(),
            normalize_generated: true,
            frame_size: Some((32, 32)),
        })
        .unwrap();

        let normalization = report.normalization.as_ref().unwrap();
        assert_eq!(report.frame_count, 4);
        assert_eq!((report.sheet_width, report.sheet_height), (64, 64));
        assert_eq!(normalization.output_frame_size, Size { w: 32, h: 32 });
        assert_eq!(normalization.source_cells.len(), 4);
        assert_eq!(normalization.frame_bboxes.len(), 4);
        assert!(normalization.scale.is_finite());
        assert!(output.join("spritesheet_normalization.json").exists());
        assert!(output.join("frames/walk_0000.png").exists());
        let normalized_sheet = decode_image(&output.join("sheet.png")).unwrap();
        assert_eq!(normalized_sheet.get_pixel(0, 0)[3], 0);
        assert_eq!(report.atlas.frames[0].frame.w, 32);
        assert!((report.atlas.frames[0].pivot.y - 0.305).abs() < 0.001);
    }

    #[test]
    fn generated_grid_rounds_approximate_explicit_cell_to_source_count() {
        let mut sheet = RgbaImage::from_pixel(1254, 1254, Rgba([0, 0, 0, 0]));
        let x_bounds = proportional_bounds(sheet.width(), 4);
        let y_bounds = proportional_bounds(sheet.height(), 4);
        for row in 0..4 {
            for column in 0..4 {
                let x0 = x_bounds[column as usize];
                let x1 = x_bounds[column as usize + 1];
                let y0 = y_bounds[row as usize];
                let y1 = y_bounds[row as usize + 1];
                let inset_x = (x1 - x0) / 4;
                let inset_y = (y1 - y0) / 4;
                for y in y0 + inset_y..y1 - inset_y {
                    for x in x0 + inset_x..x1 - inset_x {
                        sheet.put_pixel(x, y, Rgba([240, 120, 32, 255]));
                    }
                }
            }
        }

        let detection = detect_generated_grid_rects(&sheet, Some((314, 314))).unwrap();

        assert_eq!(detection.columns, Some(4));
        assert_eq!(detection.rows, Some(4));
        assert_eq!(detection.rects.len(), 16);
        assert!(detection
            .grid_detection
            .contains("explicit_cell_approx_grid"));
    }

    #[test]
    fn generated_grid_infers_common_square_sheet_without_cell_size() {
        let sheet = RgbaImage::from_pixel(1254, 1254, Rgba([240, 120, 32, 255]));

        let detection = detect_generated_grid_rects(&sheet, None).unwrap();

        assert_eq!(detection.columns, Some(4));
        assert_eq!(detection.rows, Some(4));
        assert_eq!(detection.rects.len(), 16);
        assert!(detection.grid_detection.contains("inferred_generated_4x4"));
    }

    #[test]
    fn generated_cleanup_removes_border_connected_dark_gray_matte() {
        let mut image = RgbaImage::from_pixel(12, 12, Rgba([38, 40, 42, 255]));
        for y in 4..8 {
            for x in 4..8 {
                image.put_pixel(x, y, Rgba([240, 120, 32, 255]));
            }
        }

        let cleaned = remove_generation_chroma_key(&image);

        assert_eq!(cleaned.get_pixel(0, 0)[3], 0);
        assert_eq!(cleaned.get_pixel(3, 6)[3], 0);
        assert_eq!(cleaned.get_pixel(5, 5)[3], 255);
    }

    #[test]
    fn generated_cleanup_neutralizes_green_edge_spill() {
        let mut image = RgbaImage::from_pixel(8, 8, Rgba([0, 0, 0, 0]));
        for y in 2..6 {
            for x in 2..6 {
                image.put_pixel(x, y, Rgba([240, 120, 32, 255]));
            }
        }
        image.put_pixel(1, 3, Rgba([72, 142, 58, 220]));
        image.put_pixel(1, 4, Rgba([72, 142, 58, 220]));

        let cleaned = remove_generation_chroma_key(&image);
        let edge = cleaned.get_pixel(1, 3);

        assert!(edge[3] <= 24 || edge[1] <= edge[0].max(edge[2]).saturating_add(8));
        assert_eq!(cleaned.get_pixel(3, 3)[3], 255);
    }

    #[test]
    fn generated_normalization_reports_loop_continuity_protocol() {
        let sheet = RgbaImage::from_pixel(64, 64, Rgba([240, 120, 32, 255]));
        let detection = detect_generated_grid_rects(&sheet, Some((32, 32))).unwrap();
        let temp = TempDir::new().unwrap();

        let normalized = normalize_generated_slice(
            &sheet,
            &detection,
            &PackOptions {
                input: temp.path().join("generated.png"),
                output: temp.path().join("out"),
                format: OutputFormat::Png,
                packing_strategy: PackingStrategy::MaxRects,
                pot: true,
                padding: 0,
                max_size: DEFAULT_MAX_SIZE,
                rotate: false,
                trim: false,
                dry_run: false,
                watch: false,
                slice_mode: Some(SliceMode::Grid),
                cell: Some((32, 32)),
                prefix: "walk".to_string(),
                aseprite_json: false,
                godot_spriteframes: false,
                unity_importer: false,
                animation_regex: None,
                animations: Vec::new(),
                stable_pivot: false,
                dedup_threshold: None,
                linked_channels: Vec::new(),
                normalize_generated: true,
                frame_size: Some((32, 32)),
            },
        )
        .unwrap();

        let continuity = normalized.report.continuity.as_ref().unwrap();
        assert_eq!(continuity.frame_count, 4);
        assert!(continuity.protocol.contains("single_walk_cycle"));
    }

    #[test]
    fn assemble_generated_loop_normalizes_separate_frame_files() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("frames_in");
        let output = temp.path().join("assembled");
        fs::create_dir_all(&input).unwrap();
        for index in 0..8 {
            let mut frame = RgbaImage::from_pixel(64, 64, Rgba([38, 40, 42, 255]));
            let x_offset = (index % 4) as u32;
            for y in 18..50 {
                for x in 22 + x_offset..42 + x_offset {
                    frame.put_pixel(x, y, Rgba([240, 120, 32, 255]));
                }
            }
            frame
                .save(input.join(format!("{index:03}_walk_pose.png")))
                .unwrap();
        }

        let report = assemble_generated_loop(GeneratedLoopOptions {
            input,
            output: output.clone(),
            format: OutputFormat::Png,
            dry_run: false,
            columns: None,
            prefix: "walk".to_string(),
            aseprite_json: true,
            frame_size: Some((32, 32)),
        })
        .unwrap();

        let normalization = report.normalization.as_ref().unwrap();
        assert_eq!(report.frame_count, 8);
        assert_eq!((report.sheet_width, report.sheet_height), (128, 64));
        assert_eq!(normalization.source_columns, Some(4));
        assert_eq!(normalization.source_rows, Some(2));
        assert!(normalization
            .grid_detection
            .contains("individual_generated_frames_sorted"));
        assert!(output.join("atlas.json").exists());
        assert!(output.join("aseprite.json").exists());
        assert!(output.join("spritesheet_normalization.json").exists());
        assert!(output.join("frames/walk_0000.png").exists());
        let sheet = decode_image(&output.join("sheet.png")).unwrap();
        assert_eq!(sheet.get_pixel(0, 0)[3], 0);
    }

    #[test]
    fn animation_job_accepts_generated_frames_and_assembles() {
        let temp = TempDir::new().unwrap();
        let job_dir = temp.path().join("cat_job");
        let generated = temp.path().join("generated");
        fs::create_dir_all(&generated).unwrap();
        let started = start_animation_job(AnimationJobStartOptions {
            output: job_dir.to_string_lossy().to_string(),
            action: "cat_walk".to_string(),
            frame_count: 8,
            columns: 4,
            prefix: "cat_walk".to_string(),
            frame_width: 64,
            frame_height: 64,
        })
        .unwrap();
        assert_eq!(started.job.frames.len(), 8);
        assert!(Path::new(&started.job.frames[0].pose_guide_path).exists());

        for index in 0..8 {
            let mut frame = RgbaImage::from_pixel(96, 96, Rgba([38, 40, 42, 255]));
            let sway = (index % 3) as u32;
            for y in 28..58 {
                for x in 24 + sway..70 + sway {
                    frame.put_pixel(x, y, Rgba([240, 120, 32, 255]));
                }
            }
            for y in 22..46 {
                for x in 60 + sway..82 + sway {
                    frame.put_pixel(x, y, Rgba([250, 150, 64, 255]));
                }
            }
            for (paw_x, paw_w) in [
                (22 + sway, 8),
                (42 + sway, 8),
                (58 + sway, 8),
                (74 + sway, 7),
            ] {
                for y in 66..76 {
                    for x in paw_x..paw_x + paw_w {
                        frame.put_pixel(x, y, Rgba([255, 212, 164, 255]));
                    }
                }
            }
            let path = generated.join(format!("{index:03}.png"));
            frame.save(&path).unwrap();
            let submitted = submit_animation_job_frame(AnimationJobSubmitFrameOptions {
                job_dir: job_dir.to_string_lossy().to_string(),
                frame_index: index,
                source_path: path.to_string_lossy().to_string(),
                force_accept: false,
            })
            .unwrap();
            assert!(submitted.accepted, "{:?}", submitted.errors);
        }

        let report = assemble_animation_job(AnimationJobAssembleOptions {
            job_dir: job_dir.to_string_lossy().to_string(),
            output: None,
            format: "png".to_string(),
            aseprite_json: true,
        })
        .unwrap();

        assert_eq!(report.frame_count, 8);
        assert!(Path::new(&started.job.assembled_dir)
            .join("atlas.json")
            .exists());
        assert!(Path::new(&started.job.assembled_dir)
            .join("spritesheet_normalization.json")
            .exists());
    }

    #[test]
    fn animation_job_rejects_cat_frame_without_readable_paw_contacts() {
        let temp = TempDir::new().unwrap();
        let job_dir = temp.path().join("cat_job");
        let source_path = temp.path().join("blob.png");
        let started = start_animation_job(AnimationJobStartOptions {
            output: job_dir.to_string_lossy().to_string(),
            action: "cat_walk".to_string(),
            frame_count: 8,
            columns: 4,
            prefix: "cat_walk".to_string(),
            frame_width: 64,
            frame_height: 64,
        })
        .unwrap();
        let mut frame = RgbaImage::from_pixel(96, 96, Rgba([38, 40, 42, 255]));
        for y in 24..76 {
            for x in 24..72 {
                frame.put_pixel(x, y, Rgba([240, 120, 32, 255]));
            }
        }
        frame.save(&source_path).unwrap();

        let submitted = submit_animation_job_frame(AnimationJobSubmitFrameOptions {
            job_dir: job_dir.to_string_lossy().to_string(),
            frame_index: 0,
            source_path: source_path.to_string_lossy().to_string(),
            force_accept: false,
        })
        .unwrap();

        assert!(!submitted.accepted);
        assert!(submitted
            .errors
            .iter()
            .any(|error| error.contains("readable paw-contact clusters")));
        assert!(Path::new(&started.job.frames[0].pose_guide_path).exists());
    }

    #[test]
    fn alpha_slice_detects_connected_components() {
        let mut sheet = RgbaImage::from_pixel(12, 8, Rgba([0, 0, 0, 0]));
        for y in 1..4 {
            for x in 1..4 {
                sheet.put_pixel(x, y, Rgba([255, 0, 0, 255]));
            }
        }
        for y in 2..7 {
            for x in 7..11 {
                sheet.put_pixel(x, y, Rgba([0, 255, 0, 255]));
            }
        }

        let rects = detect_alpha_components(&sheet);
        assert_eq!(rects.len(), 2);
        assert_eq!(
            rects[0],
            Rect {
                x: 1,
                y: 1,
                w: 3,
                h: 3
            }
        );
        assert_eq!(
            rects[1],
            Rect {
                x: 7,
                y: 2,
                w: 4,
                h: 5
            }
        );
    }

    fn frame(filename: &str) -> Frame {
        Frame {
            filename: filename.to_string(),
            frame: JsonRect {
                x: 0,
                y: 0,
                w: 1,
                h: 1,
            },
            rotated: false,
            trimmed: false,
            sprite_source_size: JsonRect {
                x: 0,
                y: 0,
                w: 1,
                h: 1,
            },
            source_size: Size { w: 1, h: 1 },
            pivot: Pivot { x: 0.5, y: 0.5 },
            duration_ms: Some(DEFAULT_DURATION_MS),
        }
    }

    fn write_test_sprite(path: &Path, color: [u8; 4], width: u32, height: u32) {
        let mut image = RgbaImage::from_pixel(width + 4, height + 4, Rgba([0, 0, 0, 0]));
        for y in 2..height + 2 {
            for x in 2..width + 2 {
                image.put_pixel(x, y, Rgba(color));
            }
        }
        image.save(path).unwrap();
    }

    fn write_precise_sprite(path: &Path, width: u32, height: u32, rect: Rect, color: [u8; 4]) {
        let mut image = RgbaImage::from_pixel(width, height, Rgba([0, 0, 0, 0]));
        for y in rect.y..rect.y + rect.h {
            for x in rect.x..rect.x + rect.w {
                image.put_pixel(x, y, Rgba(color));
            }
        }
        image.save(path).unwrap();
    }

    fn write_oriented_jpeg(path: &Path, width: u32, height: u32, orientation: u16) {
        let image = vec![220, 20, 40].repeat((width * height) as usize);
        let file = fs::File::create(path).unwrap();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(file, 100);
        encoder
            .set_exif_metadata(exif_orientation_chunk(orientation))
            .unwrap();
        encoder
            .write_image(&image, width, height, ColorType::Rgb8.into())
            .unwrap();
    }

    fn write_two_frame_gif(path: &Path) {
        let file = fs::File::create(path).unwrap();
        let mut encoder = GifEncoder::new(file);
        encoder.set_repeat(Repeat::Infinite).unwrap();
        let red = RgbaImage::from_pixel(3, 2, Rgba([255, 0, 0, 255]));
        let blue = RgbaImage::from_pixel(3, 2, Rgba([0, 0, 255, 255]));
        encoder
            .encode_frame(image::Frame::from_parts(
                red,
                0,
                0,
                Delay::from_numer_denom_ms(80, 1),
            ))
            .unwrap();
        encoder
            .encode_frame(image::Frame::from_parts(
                blue,
                0,
                0,
                Delay::from_numer_denom_ms(80, 1),
            ))
            .unwrap();
    }

    fn assert_image_has_no_exif_or_icc(path: &Path) {
        let mut decoder = image::ImageReader::open(path)
            .unwrap()
            .with_guessed_format()
            .unwrap()
            .into_decoder()
            .unwrap();
        assert!(decoder.exif_metadata().unwrap().is_none());
        assert!(decoder.icc_profile().unwrap().is_none());
    }

    fn exif_orientation_chunk(orientation: u16) -> Vec<u8> {
        let mut chunk = Vec::new();
        chunk.extend_from_slice(b"II");
        chunk.extend_from_slice(&42u16.to_le_bytes());
        chunk.extend_from_slice(&8u32.to_le_bytes());
        chunk.extend_from_slice(&1u16.to_le_bytes());
        chunk.extend_from_slice(&0x0112u16.to_le_bytes());
        chunk.extend_from_slice(&3u16.to_le_bytes());
        chunk.extend_from_slice(&1u32.to_le_bytes());
        chunk.extend_from_slice(&orientation.to_le_bytes());
        chunk.extend_from_slice(&0u16.to_le_bytes());
        chunk.extend_from_slice(&0u32.to_le_bytes());
        chunk
    }

    fn bilinear_sample(image: &RgbaImage, x: f32, y: f32) -> [u8; 4] {
        let x0 = x.floor().max(0.0) as u32;
        let y0 = y.floor().max(0.0) as u32;
        let x1 = (x0 + 1).min(image.width() - 1);
        let y1 = (y0 + 1).min(image.height() - 1);
        let tx = x - x.floor();
        let ty = y - y.floor();
        let p00 = image.get_pixel(x0, y0).0;
        let p10 = image.get_pixel(x1, y0).0;
        let p01 = image.get_pixel(x0, y1).0;
        let p11 = image.get_pixel(x1, y1).0;
        let mut out = [0; 4];
        for channel in 0..4 {
            let top = p00[channel] as f32 * (1.0 - tx) + p10[channel] as f32 * tx;
            let bottom = p01[channel] as f32 * (1.0 - tx) + p11[channel] as f32 * tx;
            out[channel] = (top * (1.0 - ty) + bottom * ty).round() as u8;
        }
        out
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        let digest = sha256(bytes);
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn sha256(input: &[u8]) -> [u8; 32] {
        const K: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
            0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
            0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
            0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
            0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
            0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
            0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
            0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
            0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
            0xc67178f2,
        ];
        let mut h = [
            0x6a09e667u32,
            0xbb67ae85,
            0x3c6ef372,
            0xa54ff53a,
            0x510e527f,
            0x9b05688c,
            0x1f83d9ab,
            0x5be0cd19,
        ];
        let bit_len = (input.len() as u64) * 8;
        let mut data = input.to_vec();
        data.push(0x80);
        while data.len() % 64 != 56 {
            data.push(0);
        }
        data.extend_from_slice(&bit_len.to_be_bytes());
        for chunk in data.chunks(64) {
            let mut w = [0u32; 64];
            for (index, word) in w.iter_mut().take(16).enumerate() {
                *word = u32::from_be_bytes([
                    chunk[index * 4],
                    chunk[index * 4 + 1],
                    chunk[index * 4 + 2],
                    chunk[index * 4 + 3],
                ]);
            }
            for index in 16..64 {
                let s0 = w[index - 15].rotate_right(7)
                    ^ w[index - 15].rotate_right(18)
                    ^ (w[index - 15] >> 3);
                let s1 = w[index - 2].rotate_right(17)
                    ^ w[index - 2].rotate_right(19)
                    ^ (w[index - 2] >> 10);
                w[index] = w[index - 16]
                    .wrapping_add(s0)
                    .wrapping_add(w[index - 7])
                    .wrapping_add(s1);
            }
            let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
            for index in 0..64 {
                let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
                let ch = (e & f) ^ ((!e) & g);
                let temp1 = hh
                    .wrapping_add(s1)
                    .wrapping_add(ch)
                    .wrapping_add(K[index])
                    .wrapping_add(w[index]);
                let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
                let maj = (a & b) ^ (a & c) ^ (b & c);
                let temp2 = s0.wrapping_add(maj);
                hh = g;
                g = f;
                f = e;
                e = d.wrapping_add(temp1);
                d = c;
                c = b;
                b = a;
                a = temp1.wrapping_add(temp2);
            }
            h[0] = h[0].wrapping_add(a);
            h[1] = h[1].wrapping_add(b);
            h[2] = h[2].wrapping_add(c);
            h[3] = h[3].wrapping_add(d);
            h[4] = h[4].wrapping_add(e);
            h[5] = h[5].wrapping_add(f);
            h[6] = h[6].wrapping_add(g);
            h[7] = h[7].wrapping_add(hh);
        }
        let mut out = [0; 32];
        for (index, word) in h.iter().enumerate() {
            out[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }
}
