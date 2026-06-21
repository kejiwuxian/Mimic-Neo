//! The "Token Company" compression layer, on top of MimicCLI's lossless WebP.
//!
//! The ring buffer hands us **lossless full-frame WebP** bytes — that is the
//! baseline. This module can, per the CLI flags, shrink each capture by:
//!   * cropping to the focused-window region,
//!   * downscaling to a max dimension,
//!   * re-encoding as smaller lossless WebP, or lossy JPEG with a quality knob.
//!
//! It reports savings in **tokens** (LLM bill) and **disk bytes** (dataset
//! scale) against the lossless full-frame baseline.
//!
//! Note: the `image` crate's WebP encoder is lossless-only, so "lossy" is
//! implemented via JPEG (`data:image/jpeg`). See README "Reconciliation notes".

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use image::{imageops, ColorType, DynamicImage, ImageEncoder, RgbaImage};

/// How to compress a captured frame. Defaults reproduce MimicCLI byte-for-byte
/// (lossless full-frame WebP).
#[derive(Debug, Clone)]
pub struct CompressionOptions {
    /// Lossy JPEG instead of lossless WebP.
    pub lossy: bool,
    /// JPEG quality (1–100) when `lossy`.
    pub quality: u8,
    /// Downscale so the longest side is at most this many pixels.
    pub max_dim: Option<u32>,
    /// Crop to the focused-window region before scaling.
    pub crop_focus: bool,
}

impl Default for CompressionOptions {
    fn default() -> Self {
        Self {
            lossy: false,
            quality: 80,
            max_dim: None,
            crop_focus: false,
        }
    }
}

impl CompressionOptions {
    /// True if any transform is requested (otherwise the baseline is passed through).
    pub fn is_active(&self) -> bool {
        self.lossy || self.max_dim.is_some() || self.crop_focus
    }
}

/// Encoded image plus its MIME type and file extension.
pub struct Encoded {
    pub mime: &'static str,
    pub ext: &'static str,
    pub bytes: Vec<u8>,
}

impl Encoded {
    /// `data:<mime>;base64,<...>` URL.
    pub fn data_url(&self) -> String {
        format!("data:{};base64,{}", self.mime, BASE64.encode(&self.bytes))
    }
}

/// Compress a baseline (lossless WebP) frame per `opts`. `crop` is an optional
/// `(x, y, w, h)` region (image-relative) used when `opts.crop_focus`.
///
/// Returns the encoded result; on any decode/encode failure, falls back to the
/// untouched baseline WebP so a capture is never lost.
pub fn compress_frame(
    baseline_webp: &[u8],
    crop: Option<(u32, u32, u32, u32)>,
    opts: &CompressionOptions,
) -> Encoded {
    if !opts.is_active() {
        return Encoded {
            mime: "image/webp",
            ext: "webp",
            bytes: baseline_webp.to_vec(),
        };
    }

    let Ok(decoded) = image::load_from_memory(baseline_webp) else {
        return passthrough(baseline_webp);
    };
    let mut img: RgbaImage = decoded.to_rgba8();

    if opts.crop_focus {
        if let Some((x, y, w, h)) = crop {
            img = crop_image(&img, x, y, w, h);
        }
    }
    if let Some(max_dim) = opts.max_dim {
        img = downscale(&img, max_dim);
    }

    if opts.lossy {
        match encode_jpeg(&img, opts.quality) {
            Some(bytes) => Encoded { mime: "image/jpeg", ext: "jpg", bytes },
            None => passthrough(baseline_webp),
        }
    } else {
        match encode_webp_lossless(&img) {
            Some(bytes) => Encoded { mime: "image/webp", ext: "webp", bytes },
            None => passthrough(baseline_webp),
        }
    }
}

fn passthrough(baseline_webp: &[u8]) -> Encoded {
    Encoded {
        mime: "image/webp",
        ext: "webp",
        bytes: baseline_webp.to_vec(),
    }
}

// ── Image ops ───────────────────────────────────────────────────────────────

pub fn crop_image(img: &RgbaImage, x: u32, y: u32, w: u32, h: u32) -> RgbaImage {
    let (iw, ih) = img.dimensions();
    if iw == 0 || ih == 0 {
        return img.clone();
    }
    let x = x.min(iw - 1);
    let y = y.min(ih - 1);
    let w = w.min(iw - x).max(1);
    let h = h.min(ih - y).max(1);
    imageops::crop_imm(img, x, y, w, h).to_image()
}

pub fn downscale(img: &RgbaImage, max_dim: u32) -> RgbaImage {
    let (w, h) = img.dimensions();
    let longest = w.max(h);
    if longest <= max_dim || longest == 0 {
        return img.clone();
    }
    let scale = max_dim as f32 / longest as f32;
    let nw = ((w as f32 * scale).round() as u32).max(1);
    let nh = ((h as f32 * scale).round() as u32).max(1);
    imageops::resize(img, nw, nh, imageops::FilterType::Triangle)
}

fn encode_webp_lossless(img: &RgbaImage) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut out)
        .write_image(img.as_raw(), img.width(), img.height(), ColorType::Rgba8.into())
        .ok()?;
    Some(out)
}

fn encode_jpeg(img: &RgbaImage, quality: u8) -> Option<Vec<u8>> {
    // JPEG has no alpha; flatten to RGB.
    let rgb = DynamicImage::ImageRgba8(img.clone()).to_rgb8();
    let mut out = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100));
    enc.encode(rgb.as_raw(), rgb.width(), rgb.height(), ColorType::Rgb8.into())
        .ok()?;
    Some(out)
}

// ── Token / disk-byte accounting ────────────────────────────────────────────

/// ~4 chars per token.
pub fn text_tokens(s: &str) -> usize {
    (s.chars().count() + 3) / 4
}

/// Rough LLM token cost of an image sent as base64 (expansion ~4/3, a few
/// base64 chars per token).
pub fn image_tokens(bytes: u64) -> usize {
    let b64 = (bytes + 2) / 3 * 4;
    (b64 / 3) as usize
}

/// Ledger comparing the lossless full-frame baseline against what we actually
/// stored, in bytes and tokens.
#[derive(Debug, Clone, Default)]
pub struct CompressionStats {
    pub shots: usize,
    pub baseline_bytes: u64,
    pub compressed_bytes: u64,
    pub json_chars: usize,
}

impl CompressionStats {
    /// Record one capture: baseline (lossless full frame) vs stored bytes.
    pub fn add_shot(&mut self, baseline_bytes: usize, compressed_bytes: usize) {
        self.shots += 1;
        self.baseline_bytes += baseline_bytes as u64;
        self.compressed_bytes += compressed_bytes as u64;
    }

    pub fn set_json(&mut self, json: &str) {
        self.json_chars = json.chars().count();
    }

    pub fn baseline_tokens(&self) -> usize {
        self.json_chars / 4 + image_tokens(self.baseline_bytes)
    }

    pub fn compressed_tokens(&self) -> usize {
        self.json_chars / 4 + image_tokens(self.compressed_bytes)
    }

    pub fn size_ratio(&self) -> f64 {
        ratio(self.baseline_bytes as f64, self.compressed_bytes as f64)
    }

    pub fn token_ratio(&self) -> f64 {
        ratio(self.baseline_tokens() as f64, self.compressed_tokens() as f64)
    }

    pub fn bytes_per_shot(&self) -> u64 {
        if self.shots == 0 {
            0
        } else {
            self.compressed_bytes / self.shots as u64
        }
    }

    /// Serializable summary for the dataset manifest.
    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({
            "shots": self.shots,
            "baselineBytes": self.baseline_bytes,
            "compressedBytes": self.compressed_bytes,
            "sizeRatio": round2(self.size_ratio()),
            "baselineTokensEst": self.baseline_tokens(),
            "compressedTokensEst": self.compressed_tokens(),
            "tokenRatio": round2(self.token_ratio()),
            "compressedBytesPerShot": self.bytes_per_shot(),
        })
    }

    /// Human-readable demo report.
    pub fn report(&self) -> String {
        format!(
            "── Compression report ───────────────────────────────\n\
             captures   : {shots}\n\
             screenshots: {bmb:.2} MB (lossless baseline)  →  {cmb:.3} MB   {sr:.2}× smaller\n\
             tokens(img): {bt} (baseline)  →  {ct} (compressed incl. JSON)   {tr:.2}× smaller\n\
             per capture: {bps} compressed bytes avg\n\
             ─────────────────────────────────────────────────────",
            shots = self.shots,
            bmb = self.baseline_bytes as f64 / 1_048_576.0,
            cmb = self.compressed_bytes as f64 / 1_048_576.0,
            sr = self.size_ratio(),
            bt = self.baseline_tokens(),
            ct = self.compressed_tokens(),
            tr = self.token_ratio(),
            bps = self.bytes_per_shot(),
        )
    }
}

fn ratio(baseline: f64, compressed: f64) -> f64 {
    if compressed <= 0.0 {
        1.0
    } else {
        baseline / compressed
    }
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

// ── Offline measurement ─────────────────────────────────────────────────────

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Default)]
struct SettingAccum {
    shots: usize,
    baseline_bytes: u64,
    compressed_bytes: u64,
}

impl SettingAccum {
    fn add(&mut self, baseline: usize, compressed: usize) {
        self.shots += 1;
        self.baseline_bytes += baseline as u64;
        self.compressed_bytes += compressed as u64;
    }

    fn merge(&mut self, other: &SettingAccum) {
        self.shots += other.shots;
        self.baseline_bytes += other.baseline_bytes;
        self.compressed_bytes += other.compressed_bytes;
    }

    fn size_ratio(&self) -> f64 {
        if self.compressed_bytes == 0 {
            1.0
        } else {
            self.baseline_bytes as f64 / self.compressed_bytes as f64
        }
    }

    fn baseline_tokens(&self) -> usize {
        image_tokens(self.baseline_bytes)
    }

    fn compressed_tokens(&self) -> usize {
        image_tokens(self.compressed_bytes)
    }

    fn token_ratio(&self) -> f64 {
        let ct = self.compressed_tokens() as f64;
        if ct == 0.0 {
            1.0
        } else {
            self.baseline_tokens() as f64 / ct
        }
    }

    fn token_reduction_pct(&self) -> f64 {
        let bt = self.baseline_tokens() as f64;
        if bt == 0.0 {
            0.0
        } else {
            (1.0 - self.compressed_tokens() as f64 / bt) * 100.0
        }
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "baseline_bytes": self.baseline_bytes,
            "compressed_bytes": self.compressed_bytes,
            "size_ratio": round3(self.size_ratio()),
            "baseline_tokens": self.baseline_tokens(),
            "compressed_tokens": self.compressed_tokens(),
            "token_ratio": round3(self.token_ratio()),
            "token_reduction_pct": round1(self.token_reduction_pct()),
        })
    }
}

const SWEEP_MAX_DIMS: [u32; 3] = [384, 512, 768];
const SWEEP_QUALITIES: [u8; 3] = [60, 70, 80];

fn setting_key(max_dim: u32, quality: u8) -> String {
    format!("{max_dim}q{quality}")
}

struct TaskResult {
    name: String,
    shots: usize,
    settings: BTreeMap<String, SettingAccum>,
}

pub fn measure_sweep(dir: &Path) -> anyhow::Result<String> {
    let mut subdirs: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    subdirs.sort();

    let mut task_results: Vec<TaskResult> = Vec::new();
    let mut overall: BTreeMap<String, SettingAccum> = BTreeMap::new();
    let mut overall_shots = 0usize;

    for sub in &subdirs {
        let task_name = sub
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let mut pngs: Vec<PathBuf> = std::fs::read_dir(sub)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.eq_ignore_ascii_case("png"))
                    .unwrap_or(false)
            })
            .collect();
        pngs.sort();
        if pngs.is_empty() {
            continue;
        }

        let mut task_settings: BTreeMap<String, SettingAccum> = BTreeMap::new();
        let mut task_shots = 0usize;

        for p in &pngs {
            let raw = std::fs::read(p)?;
            let Ok(decoded) = image::load_from_memory(&raw) else {
                continue;
            };
            let img = decoded.to_rgba8();
            let Some(baseline) = encode_webp_lossless(&img) else {
                continue;
            };
            task_shots += 1;
            let bl = baseline.len();

            for &md in &SWEEP_MAX_DIMS {
                for &q in &SWEEP_QUALITIES {
                    let opts = CompressionOptions {
                        lossy: true,
                        quality: q,
                        max_dim: Some(md),
                        crop_focus: false,
                    };
                    let enc = compress_frame(&baseline, None, &opts);
                    let key = setting_key(md, q);
                    task_settings
                        .entry(key)
                        .or_default()
                        .add(bl, enc.bytes.len());
                }
            }
        }

        for (k, v) in &task_settings {
            overall.entry(k.clone()).or_default().merge(v);
        }
        overall_shots += task_shots;

        task_results.push(TaskResult {
            name: task_name,
            shots: task_shots,
            settings: task_settings,
        });
    }

    let default_key = setting_key(384, 60);

    // ── stdout table ────────────────────────────────────────────────────────
    let mut out = String::new();

    out.push_str(&format!(
        "\n{:=<78}\n  Compression Measurement — {} tasks, {} total frames\n{:=<78}\n\n",
        "", task_results.len(), overall_shots, ""
    ));

    out.push_str(&format!(
        "{:<16} {:>5}  {:>12} {:>12} {:>6}  {:>8} {:>8} {:>6} {:>6}\n",
        "TASK", "SHOTS", "BASELINE_B", "COMPR_B", "SIZE_R",
        "BL_TOK", "CM_TOK", "TOK_R", "RED%"
    ));
    out.push_str(&format!("{:-<100}\n", ""));

    for tr in &task_results {
        if let Some(s) = tr.settings.get(&default_key) {
            out.push_str(&format!(
                "{:<16} {:>5}  {:>12} {:>12} {:>6.2}  {:>8} {:>8} {:>6.2} {:>5.1}%\n",
                tr.name,
                s.shots,
                s.baseline_bytes,
                s.compressed_bytes,
                s.size_ratio(),
                s.baseline_tokens(),
                s.compressed_tokens(),
                s.token_ratio(),
                s.token_reduction_pct(),
            ));
        }
    }

    out.push_str(&format!("{:-<100}\n", ""));
    if let Some(ov) = overall.get(&default_key) {
        out.push_str(&format!(
            "{:<16} {:>5}  {:>12} {:>12} {:>6.2}  {:>8} {:>8} {:>6.2} {:>5.1}%\n",
            "OVERALL",
            ov.shots,
            ov.baseline_bytes,
            ov.compressed_bytes,
            ov.size_ratio(),
            ov.baseline_tokens(),
            ov.compressed_tokens(),
            ov.token_ratio(),
            ov.token_reduction_pct(),
        ));
    }

    out.push_str(&format!(
        "\n\n{:=<78}\n  Full 9-combo sweep (overall aggregate)\n{:=<78}\n\n", "", ""
    ));
    out.push_str(&format!(
        "{:<10} {:>5}  {:>12} {:>12} {:>6}  {:>8} {:>8} {:>6} {:>6}  {}\n",
        "SETTING", "SHOTS", "BASELINE_B", "COMPR_B", "SIZE_R",
        "BL_TOK", "CM_TOK", "TOK_R", "RED%", ""
    ));
    out.push_str(&format!("{:-<100}\n", ""));

    for &md in &SWEEP_MAX_DIMS {
        for &q in &SWEEP_QUALITIES {
            let key = setting_key(md, q);
            let marker = if key == default_key { " *default" } else { "" };
            if let Some(s) = overall.get(&key) {
                out.push_str(&format!(
                    "{:<10} {:>5}  {:>12} {:>12} {:>6.2}  {:>8} {:>8} {:>6.2} {:>5.1}%{}\n",
                    key,
                    s.shots,
                    s.baseline_bytes,
                    s.compressed_bytes,
                    s.size_ratio(),
                    s.baseline_tokens(),
                    s.compressed_tokens(),
                    s.token_ratio(),
                    s.token_reduction_pct(),
                    marker,
                ));
            }
        }
    }
    out.push('\n');

    // ── JSON output ─────────────────────────────────────────────────────────
    let mut tasks_json = serde_json::Map::new();
    for tr in &task_results {
        let mut settings_json = serde_json::Map::new();
        for (k, v) in &tr.settings {
            settings_json.insert(k.clone(), v.to_json());
        }
        tasks_json.insert(
            tr.name.clone(),
            serde_json::json!({
                "shots": tr.shots,
                "settings": settings_json,
            }),
        );
    }

    let mut overall_settings_json = serde_json::Map::new();
    for (k, v) in &overall {
        overall_settings_json.insert(k.clone(), v.to_json());
    }

    let report = serde_json::json!({
        "default_setting": { "max_dim": 384, "quality": 60 },
        "tasks": tasks_json,
        "overall": {
            "shots": overall_shots,
            "settings": overall_settings_json,
        },
    });

    let json_str = serde_json::to_string_pretty(&report)?;
    let json_path = std::env::current_dir()?.join("measure-results.json");
    std::fs::write(&json_path, &json_str)?;
    out.push_str(&format!("JSON written to: {}\n", json_path.display()));

    print!("{out}");
    Ok(json_str)
}
