//! Command-line interface (clap derive).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::compress::CompressionOptions;
use crate::recorder::{Mode, RecordOptions};

/// Opt-in record-and-replay workflow capture with token-efficient compression.
///
/// Records input + screenshots ONLY during an explicit session (with an
/// on-screen Stop control), coalesces them into high-level actions matching the
/// MimicCLI `types/actions.d.ts` schema, compresses the screenshots, and exports
/// a compact payload — for the Simular Sai agent or as a computer-use dataset.
#[derive(Parser, Debug)]
#[command(name = "sai-recorder", version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Start an interactive recording session (floating Stop control; Enter also stops).
    Record(RecordArgs),
}

#[derive(Args, Debug)]
pub struct RecordArgs {
    /// Export format: `sai` (inline-base64 JSON) or `dataset` (JSONL + manifest + screenshots/).
    #[arg(long, value_enum, default_value = "sai")]
    pub mode: ModeArg,

    /// Output directory for the payload + screenshots.
    #[arg(short, long, default_value = "./recording")]
    pub out: PathBuf,

    /// Encode captures as lossy JPEG instead of lossless WebP.
    #[arg(long, default_value_t = false)]
    pub lossy: bool,

    /// JPEG quality (1–100), used with --lossy.
    #[arg(long, default_value_t = 80)]
    pub quality: u8,

    /// Downscale captures so the longest side is at most this many pixels.
    #[arg(long)]
    pub max_dim: Option<u32>,

    /// Crop captures to the focused-window region before scaling.
    #[arg(long, default_value_t = false)]
    pub crop: bool,

    /// Ring-buffer frame rate (frames per second).
    #[arg(long, default_value_t = 10.0)]
    pub fps: f32,

    /// Ring-buffer history length (seconds) kept for before/after lookup.
    #[arg(long, default_value_t = 10)]
    pub history: u64,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum ModeArg {
    /// Single JSON array with inline `data:image/...;base64` captures.
    Sai,
    /// Computer-use trajectory: JSONL + manifest.json + externalized screenshots/.
    Dataset,
}

impl From<ModeArg> for Mode {
    fn from(m: ModeArg) -> Self {
        match m {
            ModeArg::Sai => Mode::Sai,
            ModeArg::Dataset => Mode::Dataset,
        }
    }
}

impl RecordArgs {
    pub fn into_options(self) -> RecordOptions {
        RecordOptions {
            mode: self.mode.into(),
            out_dir: self.out,
            compression: CompressionOptions {
                lossy: self.lossy,
                quality: self.quality,
                max_dim: self.max_dim,
                crop_focus: self.crop,
            },
            fps: self.fps,
            history_secs: self.history,
        }
    }
}
