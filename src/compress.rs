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
