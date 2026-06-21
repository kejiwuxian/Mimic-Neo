//! Minimal append-only file logging to `%APPDATA%\sai-recorder\recorder.log`.
//!
//! This is a GUI-subsystem app (no console), so `eprintln!` is not capturable.
//! Every major step in the record/stop/replay commands logs here so the pipeline
//! can be verified headlessly by reading the file.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

/// `%APPDATA%\sai-recorder`
pub fn app_dir() -> PathBuf {
    let mut d = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    d.push("sai-recorder");
    d
}

/// Append a timestamped line to the log file (creating the app dir first).
pub fn line(msg: &str) {
    let dir = app_dir();
    let _ = fs::create_dir_all(&dir);
    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("recorder.log"))
    {
        let _ = writeln!(f, "[{ts}] {msg}");
    }
}
