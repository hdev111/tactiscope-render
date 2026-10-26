//! Cross-platform font resolution for the native render service.
//!
//! Resolution order:
//! 1. an explicit `fontPath` from the render plan,
//! 2. a `RENDER_FONT_MAP` JSON override,
//! 3. platform fonts (fontconfig on Linux, known Windows/macOS font files),
//! 4. the bundled font manifest (`<fonts-root>/manifest.json`),
//! 5. the bundled fallback family.
//!
//! `fontdue` only understands single-face TTF/OTF files, so TrueType
//! collections (`.ttc`) are intentionally skipped.

use anyhow::{Result, bail};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::process::Command;
use std::sync::OnceLock;

const FONT_EXTENSIONS: [&str; 2] = ["ttf", "otf"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontStyleKey {
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

impl FontStyleKey {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Regular => "regular",
            Self::Bold => "bold",
            Self::Italic => "italic",
            Self::BoldItalic => "boldItalic",
        }
    }
}

/// Map the render-plan weight/style strings to one of the four bundled faces.
pub fn style_key(weight: &str, style: &str) -> FontStyleKey {
    let bold = weight.eq_ignore_ascii_case("bold")
        || weight.eq_ignore_ascii_case("bolder")
        || weight
            .parse::<u16>()
            .map(|value| value >= 600)
            .unwrap_or(false);
    let italic = style.eq_ignore_ascii_case("italic") || style.eq_ignore_ascii_case("oblique");
    match (bold, italic) {
        (true, true) => FontStyleKey::BoldItalic,
        (true, false) => FontStyleKey::Bold,
        (false, true) => FontStyleKey::Italic,
        (false, false) => FontStyleKey::Regular,
    }
}

pub fn normalize_family(family: &str) -> String {
    family
        .split(',')
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches(|character| character == '"' || character == '\'')
        .trim()
        .to_lowercase()
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum FamilyEntry {
    Single(String),
    Styles(HashMap<String, String>),
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct FontManifest {
    #[serde(default)]
    fallback: HashMap<String, String>,
    #[serde(default)]
    families: HashMap<String, FamilyEntry>,
}

struct FontRoots {
    roots: Vec<PathBuf>,
    assets_root: PathBuf,
}

impl FontRoots {
    fn new(assets_root: &Path) -> Self {
        let mut roots = Vec::new();
        if let Ok(value) = std::env::var("RENDER_FONTS_PATH")
            && !value.trim().is_empty()
        {
            roots.push(PathBuf::from(value));
        }
        // `assets_root` itself may be a fonts root (tests and direct sidecar use).
        roots.push(assets_root.to_path_buf());
        roots.push(assets_root.join("fonts"));
        if let Some(parent) = assets_root.parent() {
            roots.push(parent.join("fonts"));
        }
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            roots.push(dir.join("fonts"));
            roots.push(dir.join("..").join("..").join("..").join("fonts"));
        }
        let mut unique = Vec::new();
        for root in roots {
            if !unique.iter().any(|existing: &PathBuf| existing == &root) {
                unique.push(root);
            }
        }
        Self {
            roots: unique,
            assets_root: assets_root.to_path_buf(),
        }
    }
}

fn is_supported_font(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    FONT_EXTENSIONS
        .iter()
        .any(|candidate| extension.eq_ignore_ascii_case(candidate))
}

fn existing_font(path: &Path) -> Option<PathBuf> {
    if path.is_file() && is_supported_font(path) {
        Some(path.to_path_buf())
    } else {
        None
    }
}

fn lookup_root_file(roots: &FontRoots, file: &str) -> Option<PathBuf> {
    let candidate = Path::new(file);
    if candidate.is_absolute() {
        return existing_font(candidate);
    }
    let cleaned = file.trim_start_matches('/').trim_start_matches('\\');
    for root in &roots.roots {
        if let Some(path) = existing_font(&root.join(cleaned)) {
            return Some(path);
        }
    }
    existing_font(&roots.assets_root.join(cleaned))
}

fn resolve_explicit(path: &str, roots: &FontRoots) -> Result<PathBuf> {
    if let Some(found) = lookup_root_file(roots, path) {
        return Ok(found);
    }
    bail!(
        "fontPath '{path}' does not exist or is not a TTF/OTF file (searched {:?})",
        roots.roots
    )
}

fn load_manifest(roots: &FontRoots) -> Option<FontManifest> {
    for root in &roots.roots {
        let path = root.join("manifest.json");
        let Ok(data) = std::fs::read(&path) else {
            continue;
        };
        match serde_json::from_slice::<FontManifest>(&data) {
            Ok(manifest) => return Some(manifest),
            Err(error) => {
                eprintln!(
                    "[fonts] ignoring invalid font manifest {}: {error}",
                    path.display()
                );
            }
        }
    }
    None
}

fn manifest_entry<'a>(
    manifest: &'a FontManifest,
    family: &str,
    style: FontStyleKey,
) -> Option<&'a str> {
    let entry = manifest.families.get(family)?;
    match entry {
        FamilyEntry::Single(file) => Some(file.as_str()),
        FamilyEntry::Styles(files) => files
            .get(style.as_str())
            .or_else(|| files.get("regular"))
            .map(String::as_str),
    }
}

fn env_font_map(family: &str, style: FontStyleKey) -> Option<String> {
    let value = std::env::var("RENDER_FONT_MAP").ok()?;
    let map = serde_json::from_str::<HashMap<String, String>>(&value).ok()?;
    let style_specific = format!("{family}:{}", style.as_str());
    map.get(&style_specific)
        .or_else(|| map.get(family))
        .cloned()
}

fn resolve_bundled(roots: &FontRoots, family: &str, style: FontStyleKey) -> Option<PathBuf> {
    let manifest = load_manifest(roots)?;
    let lookup = |key: &str| manifest_entry(&manifest, key, style);
    let file = lookup(family)
        .or_else(|| lookup("sans-serif"))
        .or_else(|| manifest.fallback.get(style.as_str()).map(String::as_str))
        .or_else(|| manifest.fallback.get("regular").map(String::as_str))?;
    lookup_root_file(roots, file)
}

const ARIAL: [&[&str]; 4] = [
    &["arial.ttf", "Arial.ttf"],
    &["arialbd.ttf", "Arial Bold.ttf", "Arial-Bold.ttf"],
    &["ariali.ttf", "Arial Italic.ttf", "Arial-Italic.ttf"],
    &[
        "arialbi.ttf",
        "Arial Bold Italic.ttf",
        "Arial-BoldItalic.ttf",
    ],
];

const TIMES_NEW_ROMAN: [&[&str]; 4] = [
    &["times.ttf", "Times New Roman.ttf"],
    &[
        "timesbd.ttf",
        "Times New Roman Bold.ttf",
        "Times New Roman-Bold.ttf",
    ],
    &[
        "timesi.ttf",
        "Times New Roman Italic.ttf",
        "Times New Roman-Italic.ttf",
    ],
    &[
        "timesbi.ttf",
        "Times New Roman Bold Italic.ttf",
        "Times New Roman-BoldItalic.ttf",
    ],
];

const COURIER_NEW: [&[&str]; 4] = [
    &["cour.ttf", "Courier New.ttf"],
    &["courbd.ttf", "Courier New Bold.ttf", "Courier New-Bold.ttf"],
    &[
        "couri.ttf",
        "Courier New Italic.ttf",
        "Courier New-Italic.ttf",
    ],
    &[
        "courbi.ttf",
        "Courier New Bold Italic.ttf",
        "Courier New-BoldItalic.ttf",
    ],
];

const VERDANA: [&[&str]; 4] = [
    &["verdana.ttf", "Verdana.ttf"],
    &["verdanab.ttf", "Verdana Bold.ttf", "Verdana-Bold.ttf"],
    &["verdanai.ttf", "Verdana Italic.ttf", "Verdana-Italic.ttf"],
    &[
        "verdanaz.ttf",
        "Verdana Bold Italic.ttf",
        "Verdana-BoldItalic.ttf",
    ],
];

const GEORGIA: [&[&str]; 4] = [
    &["georgia.ttf", "Georgia.ttf"],
    &["georgiab.ttf", "Georgia Bold.ttf", "Georgia-Bold.ttf"],
    &["georgiai.ttf", "Georgia Italic.ttf", "Georgia-Italic.ttf"],
    &[
        "georgiaz.ttf",
        "Georgia Bold Italic.ttf",
        "Georgia-BoldItalic.ttf",
    ],
];

const TREBUCHET_MS: [&[&str]; 4] = [
    &["trebuc.ttf", "Trebuchet MS.ttf"],
    &[
        "trebucbd.ttf",
        "Trebuchet MS Bold.ttf",
        "Trebuchet MS-Bold.ttf",
    ],
    &[
        "trebucit.ttf",
        "Trebuchet MS Italic.ttf",
        "Trebuchet MS-Italic.ttf",
    ],
    &[
        "trebucbi.ttf",
        "Trebuchet MS Bold Italic.ttf",
        "Trebuchet MS-BoldItalic.ttf",
    ],
];

const COMIC_SANS_MS: [&[&str]; 4] = [
    &["comic.ttf", "Comic Sans MS.ttf"],
    &[
        "comicbd.ttf",
        "Comic Sans MS Bold.ttf",
        "Comic Sans MS-Bold.ttf",
    ],
    &["comici.ttf", "Comic Sans MS Italic.ttf"],
    &["comicbi.ttf", "Comic Sans MS Bold Italic.ttf"],
];

const TAHOMA: [&[&str]; 4] = [
    &["tahoma.ttf"],
    &["tahomabd.ttf", "tahoma.ttf"],
    &["tahoma.ttf"],
    &["tahomabd.ttf", "tahoma.ttf"],
];

const SEGOE_UI: [&[&str]; 4] = [
    &["segoeui.ttf"],
    &["segoeuib.ttf"],
    &["segoeuii.ttf"],
    &["segoeuiz.ttf"],
];

const HELVETICA: [&[&str]; 4] = [
    &["Helvetica.ttf", "HelveticaNeue.ttf"],
    &[
        "Helvetica-Bold.ttf",
        "Helvetica Bold.ttf",
        "HelveticaNeue-Bold.ttf",
    ],
    &[
        "Helvetica-Oblique.ttf",
        "Helvetica Oblique.ttf",
        "HelveticaNeue-Italic.ttf",
    ],
    &[
        "Helvetica-BoldOblique.ttf",
        "Helvetica Bold Oblique.ttf",
        "HelveticaNeue-BoldItalic.ttf",
    ],
];

const IMPACT: [&[&str]; 4] = [
    &["impact.ttf", "Impact.ttf"],
    &["impact.ttf", "Impact.ttf"],
    &["impact.ttf", "Impact.ttf"],
    &["impact.ttf", "Impact.ttf"],
];

const CALIBRI: [&[&str]; 4] = [
    &["calibri.ttf"],
    &["calibrib.ttf"],
    &["calibrii.ttf"],
    &["calibriz.ttf"],
];

const CONSOLAS: [&[&str]; 4] = [
    &["consola.ttf"],
    &["consolab.ttf"],
    &["consolai.ttf"],
    &["consolaz.ttf"],
];

const NOTO_SANS: [&[&str]; 4] = [
    &["NotoSans-Regular.ttf"],
    &["NotoSans-Bold.ttf"],
    &["NotoSans-Italic.ttf"],
    &["NotoSans-BoldItalic.ttf"],
];

const NOTO_SERIF: [&[&str]; 4] = [
    &["NotoSerif-Regular.ttf"],
    &["NotoSerif-Bold.ttf"],
    &["NotoSerif-Italic.ttf"],
    &["NotoSerif-BoldItalic.ttf"],
];

const NOTO_MONO: [&[&str]; 4] = [
    &["NotoSansMono-Regular.ttf"],
    &["NotoSansMono-Bold.ttf"],
    &["NotoSansMono-Regular.ttf"],
    &["NotoSansMono-Bold.ttf"],
];

const DEJAVU_SANS: [&[&str]; 4] = [
    &["DejaVuSans.ttf"],
    &["DejaVuSans-Bold.ttf"],
    &["DejaVuSans-Oblique.ttf"],
    &["DejaVuSans-BoldOblique.ttf"],
];

const DEJAVU_SERIF: [&[&str]; 4] = [
    &["DejaVuSerif.ttf"],
    &["DejaVuSerif-Bold.ttf"],
    &["DejaVuSerif-Italic.ttf"],
    &["DejaVuSerif-BoldItalic.ttf"],
];

const DEJAVU_MONO: [&[&str]; 4] = [
    &["DejaVuSansMono.ttf"],
    &["DejaVuSansMono-Bold.ttf"],
    &["DejaVuSansMono-Oblique.ttf"],
    &["DejaVuSansMono-BoldOblique.ttf"],
];

const LIBERATION_SANS: [&[&str]; 4] = [
    &["LiberationSans-Regular.ttf"],
    &["LiberationSans-Bold.ttf"],
    &["LiberationSans-Italic.ttf"],
    &["LiberationSans-BoldItalic.ttf"],
];

const LIBERATION_SERIF: [&[&str]; 4] = [
    &["LiberationSerif-Regular.ttf"],
    &["LiberationSerif-Bold.ttf"],
    &["LiberationSerif-Italic.ttf"],
    &["LiberationSerif-BoldItalic.ttf"],
];

const LIBERATION_MONO: [&[&str]; 4] = [
    &["LiberationMono-Regular.ttf"],
    &["LiberationMono-Bold.ttf"],
    &["LiberationMono-Italic.ttf"],
    &["LiberationMono-BoldItalic.ttf"],
];

const SANS_SERIF: [&[&str]; 4] = [
    &[
        "arial.ttf",
        "Arial.ttf",
        "Helvetica.ttf",
        "segoeui.ttf",
        "DejaVuSans.ttf",
        "LiberationSans-Regular.ttf",
        "NotoSans-Regular.ttf",
    ],
    &[
        "arialbd.ttf",
        "Arial Bold.ttf",
        "Helvetica-Bold.ttf",
        "segoeuib.ttf",
        "DejaVuSans-Bold.ttf",
        "LiberationSans-Bold.ttf",
        "NotoSans-Bold.ttf",
    ],
    &[
        "ariali.ttf",
        "Arial Italic.ttf",
        "Helvetica-Oblique.ttf",
        "segoeuii.ttf",
        "DejaVuSans-Oblique.ttf",
        "LiberationSans-Italic.ttf",
        "NotoSans-Italic.ttf",
    ],
    &[
        "arialbi.ttf",
        "Arial Bold Italic.ttf",
        "Helvetica-BoldOblique.ttf",
        "segoeuiz.ttf",
        "DejaVuSans-BoldOblique.ttf",
        "LiberationSans-BoldItalic.ttf",
        "NotoSans-BoldItalic.ttf",
    ],
];

const SERIF: [&[&str]; 4] = [
    &[
        "times.ttf",
        "Times New Roman.ttf",
        "Georgia.ttf",
        "DejaVuSerif.ttf",
        "LiberationSerif-Regular.ttf",
        "NotoSerif-Regular.ttf",
    ],
    &[
        "timesbd.ttf",
        "Times New Roman Bold.ttf",
        "Georgia Bold.ttf",
        "DejaVuSerif-Bold.ttf",
        "LiberationSerif-Bold.ttf",
        "NotoSerif-Bold.ttf",
    ],
    &[
        "timesi.ttf",
        "Times New Roman Italic.ttf",
        "Georgia Italic.ttf",
        "DejaVuSerif-Italic.ttf",
        "LiberationSerif-Italic.ttf",
        "NotoSerif-Italic.ttf",
    ],
    &[
        "timesbi.ttf",
        "Times New Roman Bold Italic.ttf",
        "Georgia Bold Italic.ttf",
        "DejaVuSerif-BoldItalic.ttf",
        "LiberationSerif-BoldItalic.ttf",
        "NotoSerif-BoldItalic.ttf",
    ],
];

const MONOSPACE: [&[&str]; 4] = [
    &[
        "cour.ttf",
        "Courier New.ttf",
        "consola.ttf",
        "DejaVuSansMono.ttf",
        "LiberationMono-Regular.ttf",
        "NotoSansMono-Regular.ttf",
    ],
    &[
        "courbd.ttf",
        "Courier New Bold.ttf",
        "consolab.ttf",
        "DejaVuSansMono-Bold.ttf",
        "LiberationMono-Bold.ttf",
        "NotoSansMono-Bold.ttf",
    ],
    &[
        "couri.ttf",
        "Courier New Italic.ttf",
        "consolai.ttf",
        "DejaVuSansMono-Oblique.ttf",
        "LiberationMono-Italic.ttf",
        "NotoSansMono-Regular.ttf",
    ],
    &[
        "courbi.ttf",
        "Courier New Bold Italic.ttf",
        "consolaz.ttf",
        "DejaVuSansMono-BoldOblique.ttf",
        "LiberationMono-BoldItalic.ttf",
        "NotoSansMono-Bold.ttf",
    ],
];

fn style_index(style: FontStyleKey) -> usize {
    match style {
        FontStyleKey::Regular => 0,
        FontStyleKey::Bold => 1,
        FontStyleKey::Italic => 2,
        FontStyleKey::BoldItalic => 3,
    }
}

/// Filenames to try for a requested family. macOS uses spaced names such as
/// `Arial Bold.ttf`, Windows uses compact names such as `arialbd.ttf`, and
/// Linux uses DejaVu/Liberation/Noto names. All are tried in order.
fn family_candidates(family: &str, style: FontStyleKey) -> Vec<&'static str> {
    let candidates: &[&[&str]; 4] = match family {
        "arial" => &ARIAL,
        "times new roman" | "times" => &TIMES_NEW_ROMAN,
        "courier new" | "courier" => &COURIER_NEW,
        "verdana" => &VERDANA,
        "georgia" => &GEORGIA,
        "trebuchet ms" => &TREBUCHET_MS,
        "comic sans ms" | "comic sans" => &COMIC_SANS_MS,
        "tahoma" => &TAHOMA,
        "segoe ui" => &SEGOE_UI,
        "helvetica" | "helvetica neue" => &HELVETICA,
        "impact" => &IMPACT,
        "calibri" => &CALIBRI,
        "consolas" => &CONSOLAS,
        "noto sans" => &NOTO_SANS,
        "noto serif" => &NOTO_SERIF,
        "noto mono" | "noto sans mono" => &NOTO_MONO,
        "dejavu sans" => &DEJAVU_SANS,
        "dejavu serif" => &DEJAVU_SERIF,
        "dejavu mono" | "dejavu sans mono" => &DEJAVU_MONO,
        "liberation sans" => &LIBERATION_SANS,
        "liberation serif" => &LIBERATION_SERIF,
        "liberation mono" => &LIBERATION_MONO,
        "sans-serif" | "system-ui" | "ui-sans-serif" => &SANS_SERIF,
        "serif" | "ui-serif" => &SERIF,
        "monospace" | "ui-monospace" => &MONOSPACE,
        _ => return Vec::new(),
    };
    candidates[style_index(style)].to_vec()
}

fn system_font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    #[cfg(target_os = "windows")]
    {
        if let Ok(windir) = std::env::var("WINDIR") {
            dirs.push(PathBuf::from(windir).join("Fonts"));
        }
        dirs.push(PathBuf::from("C:\\Windows\\Fonts"));
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            dirs.push(
                PathBuf::from(local)
                    .join("Microsoft")
                    .join("Windows")
                    .join("Fonts"),
            );
        } else if let Ok(profile) = std::env::var("USERPROFILE") {
            dirs.push(
                PathBuf::from(profile)
                    .join("AppData")
                    .join("Local")
                    .join("Microsoft")
                    .join("Windows")
                    .join("Fonts"),
            );
        }
    }
    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from("/System/Library/Fonts/Supplemental"));
        dirs.push(PathBuf::from("/System/Library/Fonts"));
        dirs.push(PathBuf::from("/Library/Fonts"));
        dirs.push(PathBuf::from("/Network/Library/Fonts"));
        if let Ok(home) = std::env::var("HOME") {
            dirs.push(PathBuf::from(home).join("Library").join("Fonts"));
        }
    }
    #[cfg(target_os = "linux")]
    {
        dirs.push(PathBuf::from("/usr/share/fonts"));
        dirs.push(PathBuf::from("/usr/local/share/fonts"));
        if let Ok(home) = std::env::var("HOME") {
            dirs.push(PathBuf::from(&home).join(".fonts"));
            dirs.push(PathBuf::from(&home).join(".local/share/fonts"));
        }
        if let Ok(xdg_data_home) = std::env::var("XDG_DATA_HOME") {
            dirs.push(PathBuf::from(xdg_data_home).join("fonts"));
        }
        if let Ok(xdg_data_dirs) = std::env::var("XDG_DATA_DIRS") {
            for root in xdg_data_dirs.split(':').filter(|value| !value.is_empty()) {
                dirs.push(PathBuf::from(root).join("fonts"));
            }
        }
    }
    dirs
}

fn index_font_dir(dir: &Path, depth: u8, index: &mut HashMap<String, PathBuf>) {
    // Font roots are shallow (two levels is enough for DejaVu/Noto/Liberation
    // and macOS Supplemental). The depth cap also protects against symlink loops.
    if depth > 2 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            index_font_dir(&path, depth + 1, index);
            continue;
        }
        if !is_supported_font(&path) || !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        index
            .entry(name.to_ascii_lowercase())
            .or_insert_with(|| path.clone());
    }
}

fn system_font_index() -> &'static HashMap<String, PathBuf> {
    static INDEX: OnceLock<HashMap<String, PathBuf>> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut index = HashMap::new();
        for dir in system_font_dirs() {
            index_font_dir(&dir, 0, &mut index);
        }
        index
    })
}

fn lookup_system_font(file: &str) -> Option<PathBuf> {
    system_font_index().get(&file.to_ascii_lowercase()).cloned()
}

fn fc_match(family: &str, style: FontStyleKey) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        let style = match style {
            FontStyleKey::Regular => "Regular",
            FontStyleKey::Bold => "Bold",
            FontStyleKey::Italic => "Italic",
            FontStyleKey::BoldItalic => "Bold Italic",
        };
        let pattern = format!("{family}:style={style}");
        let output = Command::new("fc-match")
            .args(["-f", "%{file}", &pattern])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
        existing_font(&path)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (family, style);
        None
    }
}

fn resolve_system(family: &str, style: FontStyleKey) -> Option<PathBuf> {
    if let Some(path) = fc_match(family, style) {
        return Some(path);
    }
    // macOS and Windows do not expose a stable `fc-match` binary. Match known
    // family filenames against an index of the platform font directories.
    for file in family_candidates(family, style) {
        if let Some(path) = lookup_system_font(file) {
            return Some(path);
        }
    }
    None
}

fn resolve_fallback_file(roots: &FontRoots, style: FontStyleKey) -> Option<PathBuf> {
    let files: &[&str] = match style {
        FontStyleKey::Regular => &[
            "NotoSans-Regular.ttf",
            "DejaVuSans.ttf",
            "LiberationSans-Regular.ttf",
            "Arial.ttf",
            "arial.ttf",
        ],
        FontStyleKey::Bold => &[
            "NotoSans-Bold.ttf",
            "DejaVuSans-Bold.ttf",
            "LiberationSans-Bold.ttf",
            "Arial Bold.ttf",
            "arialbd.ttf",
        ],
        FontStyleKey::Italic => &[
            "NotoSans-Italic.ttf",
            "DejaVuSans-Oblique.ttf",
            "LiberationSans-Italic.ttf",
            "Arial Italic.ttf",
            "ariali.ttf",
        ],
        FontStyleKey::BoldItalic => &[
            "NotoSans-BoldItalic.ttf",
            "DejaVuSans-BoldOblique.ttf",
            "LiberationSans-BoldItalic.ttf",
            "Arial Bold Italic.ttf",
            "arialbi.ttf",
        ],
    };
    for file in files {
        if let Some(path) = lookup_root_file(roots, file) {
            return Some(path);
        }
        if let Some(path) = lookup_system_font(file) {
            return Some(path);
        }
    }
    None
}

/// Resolve a font file for a text layer or text mask.
pub fn resolve_font_path(
    family: &str,
    weight: &str,
    style: &str,
    explicit_path: Option<&str>,
    assets_root: &Path,
) -> Result<PathBuf> {
    let roots = FontRoots::new(assets_root);
    if let Some(path) = explicit_path
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return resolve_explicit(path, &roots);
    }

    let style = style_key(weight, style);
    let family = normalize_family(family);

    if let Some(path) = env_font_map(&family, style) {
        if let Some(found) = lookup_root_file(&roots, &path) {
            return Ok(found);
        }
        eprintln!("[fonts] RENDER_FONT_MAP path for {family} is missing: {path}");
    }

    if let Some(path) = resolve_system(&family, style) {
        return Ok(path);
    }

    if let Some(path) = resolve_bundled(&roots, &family, style) {
        return Ok(path);
    }

    if let Some(path) = resolve_fallback_file(&roots, style) {
        return Ok(path);
    }

    bail!(
        "could not resolve font family '{family}' ({}, {}); bundled fallback missing under {:?}",
        style.as_str(),
        if weight.is_empty() { "normal" } else { weight },
        roots.roots
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_root(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tactiscope-fonts-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn maps_weight_and_style() {
        assert_eq!(style_key("normal", "normal"), FontStyleKey::Regular);
        assert_eq!(style_key("bold", "normal"), FontStyleKey::Bold);
        assert_eq!(style_key("normal", "italic"), FontStyleKey::Italic);
        assert_eq!(style_key("700", "oblique"), FontStyleKey::BoldItalic);
    }

    #[test]
    fn normalizes_family_lists() {
        assert_eq!(normalize_family("\"Arial\", sans-serif"), "arial");
        assert_eq!(normalize_family(" Noto Sans "), "noto sans");
    }

    #[test]
    fn resolves_manifest_family_and_fallback() {
        let root = temp_root("manifest");
        let file = root.join("NotoSans-Regular.ttf");
        fs::write(&file, b"not-a-real-font").unwrap();
        let manifest = serde_json::json!({
            "fallback": { "regular": "NotoSans-Regular.ttf" },
            "families": { "arial": { "regular": "NotoSans-Regular.ttf" } }
        });
        fs::write(
            root.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let roots = FontRoots::new(&root);
        assert_eq!(
            resolve_bundled(&roots, "arial", FontStyleKey::Regular).unwrap(),
            file
        );
        assert_eq!(
            resolve_bundled(&roots, "some google font", FontStyleKey::Regular).unwrap(),
            file
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn explicit_font_path_wins() {
        let root = temp_root("explicit");
        let file = root.join("custom.ttf");
        fs::write(&file, b"not-a-real-font").unwrap();
        assert_eq!(
            resolve_font_path("Arial", "normal", "normal", Some("custom.ttf"), &root).unwrap(),
            file
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn family_candidates_cover_windows_and_macos_names() {
        let bold = family_candidates("arial", FontStyleKey::Bold);
        assert!(bold.contains(&"arialbd.ttf"));
        assert!(bold.contains(&"Arial Bold.ttf"));

        let mac_regular = family_candidates("times new roman", FontStyleKey::Regular);
        assert!(mac_regular.contains(&"Times New Roman.ttf"));

        let sans = family_candidates("sans-serif", FontStyleKey::Italic);
        assert!(
            sans.iter()
                .any(|file| file.to_ascii_lowercase().contains("dejavu"))
        );
        assert!(
            sans.iter()
                .any(|file| file.to_ascii_lowercase().contains("noto"))
        );

        // Generic families and unknown families are handled deterministically.
        assert!(family_candidates("monospace", FontStyleKey::Regular).len() > 1);
        assert!(family_candidates("no such font", FontStyleKey::Regular).is_empty());
    }

    #[test]
    fn indexes_system_fonts_case_insensitively_and_skips_collections() {
        let root = temp_root("index");
        let nested = root.join("truetype").join("custom");
        fs::create_dir_all(&nested).unwrap();
        let font = nested.join("Arial Bold.ttf");
        fs::write(&font, b"not-a-real-font").unwrap();
        fs::write(nested.join("Collection.ttc"), b"not-a-real-font").unwrap();

        let mut index = HashMap::new();
        index_font_dir(&root, 0, &mut index);
        assert_eq!(index.get("arial bold.ttf"), Some(&font));
        assert!(!index.contains_key("collection.ttc"));
        let _ = fs::remove_dir_all(root);
    }
}
