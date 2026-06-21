//! Task library: persist each recording under `%APPDATA%\sai-recorder\tasks\<id>\`
//! and provide list/get/rename/delete plus action reload for replay.
//!
//! Layout per task:
//!   * sai mode     → `sai.json` (inline-base64 UserAction array)
//!   * dataset mode → `trajectory.jsonl` + `manifest.json` + `screenshots/`
//!   * always       → `meta.json` (id, name, created, mode, counts, compression)

use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::actions::UserAction;
use crate::capture;
use crate::export;
use crate::recorder::{Mode, RecordOptions, RecorderOutput};

/// Serializable task metadata (also the row model the UI lists).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskMeta {
    pub id: String,
    pub name: String,
    pub created: String,
    pub mode: String,
    pub action_count: usize,
    pub duration_ms: u64,
    pub compression: Value,
}

/// Task metadata plus a (truncated) JSON preview, for the detail view.
#[derive(Debug, Clone, Serialize)]
pub struct TaskDetail {
    pub meta: TaskMeta,
    pub preview: String,
}

/// `%APPDATA%\sai-recorder\tasks`
pub fn base_dir() -> PathBuf {
    let mut d = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    d.push("sai-recorder");
    d.push("tasks");
    d
}

fn task_dir(id: &str) -> PathBuf {
    base_dir().join(id)
}

fn read_meta(id: &str) -> Result<TaskMeta> {
    let path = task_dir(id).join("meta.json");
    let raw = fs::read_to_string(&path)
        .map_err(|e| anyhow!("reading {}: {e}", path.display()))?;
    Ok(serde_json::from_str(&raw)?)
}

fn write_meta(meta: &TaskMeta) -> Result<()> {
    let path = task_dir(&meta.id).join("meta.json");
    fs::write(&path, serde_json::to_string_pretty(meta)?)?;
    Ok(())
}

/// Persist a finished recording as a new task; returns its metadata.
pub fn save_recording(output: &RecorderOutput, opts: &RecordOptions) -> Result<TaskMeta> {
    let base = base_dir();
    fs::create_dir_all(&base)?;

    let now = chrono::Local::now();
    let mut id = now.format("%Y%m%d-%H%M%S").to_string();
    let mut dir = base.join(&id);
    let mut n = 1;
    while dir.exists() {
        id = format!("{}-{n}", now.format("%Y%m%d-%H%M%S"));
        dir = base.join(&id);
        n += 1;
    }
    fs::create_dir_all(&dir)?;

    // Compression summary (token + byte savings) for the manifest and meta.
    let mut stats = output.stats.clone();
    stats.set_json(&export::structural_json(&output.actions));
    let compression = stats.summary();

    let mode_str = match opts.mode {
        Mode::Sai => "sai",
        Mode::Dataset => "dataset",
    };

    match opts.mode {
        Mode::Sai => {
            fs::write(dir.join("sai.json"), export::sai_json(&output.actions))?;
        }
        Mode::Dataset => {
            let primary = capture::get_screen_metadata(&capture::get_primary_screen());
            let manifest = export::build_manifest(
                std::env::consts::OS.to_string(),
                output.started_ms,
                output.ended_ms,
                primary,
                output.actions.len(),
                compression.clone(),
            );
            export::write_dataset(&dir, &output.actions, &manifest)?;
        }
    }

    let meta = TaskMeta {
        id: id.clone(),
        name: format!("Recording {}", now.format("%Y-%m-%d %H:%M")),
        created: now.to_rfc3339(),
        mode: mode_str.to_string(),
        action_count: output.actions.len(),
        duration_ms: output.duration_ms,
        compression,
    };
    write_meta(&meta)?;
    Ok(meta)
}

/// All tasks, newest first.
pub fn list_tasks() -> Vec<TaskMeta> {
    let base = base_dir();
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(&base) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    if let Ok(meta) = read_meta(name) {
                        out.push(meta);
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| b.id.cmp(&a.id));
    out
}

/// Task metadata + a truncated JSON preview.
pub fn get_task(id: &str) -> Result<TaskDetail> {
    let meta = read_meta(id)?;
    let actions = load_actions(id).unwrap_or_default();
    let preview = truncate(&export::sai_json_preview(&actions), 6000);
    Ok(TaskDetail { meta, preview })
}

pub fn rename_task(id: &str, name: &str) -> Result<TaskMeta> {
    let mut meta = read_meta(id)?;
    meta.name = name.to_string();
    write_meta(&meta)?;
    Ok(meta)
}

pub fn delete_task(id: &str) -> Result<()> {
    // Guard against path traversal in the id.
    if id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err(anyhow!("invalid task id"));
    }
    let dir = task_dir(id);
    if dir.exists() {
        fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

/// Re-read a task's recorded actions (for replay). Captures are ignored.
pub fn load_actions(id: &str) -> Result<Vec<UserAction>> {
    let dir = task_dir(id);
    let sai = dir.join("sai.json");
    if sai.exists() {
        let raw = fs::read_to_string(&sai)?;
        return Ok(serde_json::from_str(&raw)?);
    }
    let jsonl = dir.join("trajectory.jsonl");
    if jsonl.exists() {
        let raw = fs::read_to_string(&jsonl)?;
        let mut actions = Vec::new();
        for line in raw.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(a) = serde_json::from_str::<UserAction>(line) {
                actions.push(a);
            }
        }
        return Ok(actions);
    }
    Err(anyhow!("no payload found for task {id}"))
}

/// Compact payload string for Telegram (sai.json, else manifest.json).
pub fn compact_payload(id: &str) -> Result<String> {
    let dir = task_dir(id);
    for name in ["sai.json", "manifest.json"] {
        let p = dir.join(name);
        if p.exists() {
            return Ok(fs::read_to_string(p)?);
        }
    }
    Err(anyhow!("no payload to send for task {id}"))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}\n… [truncated]")
    }
}
