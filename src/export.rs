//! Export to one of two formats:
//!
//! * **sai** — a single JSON array of [`UserAction`]s (matching
//!   `types/actions.d.ts`) with inline `data:image/...;base64,...` captures,
//!   written to `actions.json` (the MimicCLI-style inline payload).
//! * **dataset** — a computer-use trajectory: each capture is externalized to a
//!   `screenshots/` file and referenced by a relative path; actions are written
//!   one-per-line to `trajectory.jsonl`, with a `manifest.json` carrying session
//!   metadata, compression stats, and a self-documenting schema block.

use std::fs;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde::Serialize;
use serde_json::Value;

use crate::actions::UserAction;
use crate::capture::FocusedScreenMetadata;

// ── sai mode ────────────────────────────────────────────────────────────────

/// Pretty JSON array with inline base64 captures (written to `actions.json`).
pub fn sai_json(actions: &[UserAction]) -> String {
    serde_json::to_string_pretty(actions).unwrap_or_else(|_| "[]".to_string())
}

/// Same payload but with base64 capture blobs truncated, for terminal preview.
pub fn sai_json_preview(actions: &[UserAction]) -> String {
    let mut value = serde_json::to_value(actions).unwrap_or(Value::Null);
    for_each_capture(&mut value, &mut |s| {
        if s.starts_with("data:") && s.len() > 48 {
            let head: String = s.chars().take(40).collect();
            *s = format!("{head}…<base64 {} chars>", s.len());
        }
    });
    serde_json::to_string_pretty(&value).unwrap_or_default()
}

/// Structural JSON with capture blobs blanked — used only for token estimates
/// (so image bytes aren't double-counted against the inline base64).
pub fn structural_json(actions: &[UserAction]) -> String {
    let mut value = serde_json::to_value(actions).unwrap_or(Value::Null);
    for_each_capture(&mut value, &mut |s| s.clear());
    serde_json::to_string(&value).unwrap_or_default()
}

// ── dataset mode ────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct SchemaDoc {
    pub description: &'static str,
    pub trajectory_file: &'static str,
    pub step_format: &'static str,
    pub observation: &'static str,
    pub action: &'static str,
    pub screenshots: &'static str,
}

impl Default for SchemaDoc {
    fn default() -> Self {
        SchemaDoc {
            description:
                "Computer-use trajectory of high-level (before → action → after) steps captured \
                 from real user interaction. Action schema matches types/actions.d.ts.",
            trajectory_file: "trajectory.jsonl — one JSON action object per line.",
            step_format:
                "{ type, timestamp(ms), duration(ms), before:{capture,focused}, after:{capture,focused}, ...action fields }",
            observation:
                "before/after each carry a `capture` (relative screenshots/ path) and `focused` { window, screen } metadata.",
            action:
                "Internally tagged by `type`: Hover|Click|DoubleClick|TripleClick|Drag|Scroll|Type|Press. Keys/buttons use rdev native names.",
            screenshots: "Files under screenshots/, optionally focus-cropped/downscaled, WebP (lossless) or JPEG (lossy).",
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub format: &'static str,
    pub version: u32,
    pub os: String,
    pub session_start_ms: u64,
    pub session_end_ms: u64,
    pub primary_screen: FocusedScreenMetadata,
    pub total_steps: usize,
    pub compression: Value,
    pub schema: SchemaDoc,
}

pub fn build_manifest(
    os: String,
    session_start_ms: u64,
    session_end_ms: u64,
    primary_screen: FocusedScreenMetadata,
    total_steps: usize,
    compression: Value,
) -> Manifest {
    Manifest {
        format: "computer-use-trajectory",
        version: 1,
        os,
        session_start_ms,
        session_end_ms,
        primary_screen,
        total_steps,
        compression,
        schema: SchemaDoc::default(),
    }
}

/// Paths written by [`write_dataset`].
pub struct Written {
    pub jsonl: PathBuf,
    pub manifest: PathBuf,
}

/// Externalize captures to `screenshots/` and write `trajectory.jsonl` +
/// `manifest.json`.
pub fn write_dataset(dir: &Path, actions: &[UserAction], manifest: &Manifest) -> std::io::Result<Written> {
    let shots_dir = dir.join("screenshots");
    fs::create_dir_all(&shots_dir)?;

    let mut counter = 0usize;
    let mut jsonl = String::new();
    for action in actions {
        let mut value = serde_json::to_value(action).unwrap_or(Value::Null);
        // Replace each inline data-URL capture with a written file + rel path.
        for_each_capture(&mut value, &mut |s| {
            if let Some((ext, bytes)) = decode_data_url(s) {
                let name = format!("shot_{counter:05}.{ext}");
                if fs::write(shots_dir.join(&name), bytes).is_ok() {
                    *s = format!("screenshots/{name}");
                }
                counter += 1;
            }
        });
        jsonl.push_str(&serde_json::to_string(&value).unwrap_or_default());
        jsonl.push('\n');
    }

    let jsonl_path = dir.join("trajectory.jsonl");
    fs::write(&jsonl_path, jsonl)?;

    let manifest_path = dir.join("manifest.json");
    fs::write(
        &manifest_path,
        serde_json::to_string_pretty(manifest).unwrap_or_else(|_| "{}".to_string()),
    )?;

    Ok(Written { jsonl: jsonl_path, manifest: manifest_path })
}

// ── helpers ─────────────────────────────────────────────────────────────────

/// Visit every `before.capture` / `after.capture` string in a serialized
/// action (or array of actions) and apply `f`.
fn for_each_capture(value: &mut Value, f: &mut dyn FnMut(&mut String)) {
    match value {
        Value::Array(items) => {
            for item in items {
                for_each_capture(item, f);
            }
        }
        Value::Object(map) => {
            for key in ["before", "after"] {
                if let Some(Value::Object(cap)) = map.get_mut(key) {
                    if let Some(Value::String(s)) = cap.get_mut("capture") {
                        f(s);
                    }
                }
            }
        }
        _ => {}
    }
}

/// Decode a `data:image/<x>;base64,<...>` URL into `(ext, bytes)`.
fn decode_data_url(s: &str) -> Option<(&'static str, Vec<u8>)> {
    if !s.starts_with("data:") {
        return None;
    }
    let ext = if s.contains("image/webp") {
        "webp"
    } else if s.contains("image/jpeg") {
        "jpg"
    } else {
        "bin"
    };
    let payload = s.split_once(";base64,").map(|(_, b)| b)?;
    let bytes = BASE64.decode(payload.as_bytes()).ok()?;
    Some((ext, bytes))
}
