//! Local review gate. **Nothing leaves the machine without explicit confirmation.**
//!
//! Prints the payload preview and the compression report, then requires an
//! interactive `y/N` confirmation on stdin (default: No).

use std::io::{self, Write};

use crate::compress::CompressionStats;

/// Show the preview + compression report and ask the user to confirm an upload.
/// Returns `true` only on an explicit yes.
pub fn confirm_upload(preview: &str, stats: &CompressionStats, destination: &str) -> bool {
    println!("\n===== REVIEW (nothing has been sent) =====\n");
    println!("{preview}\n");
    println!("{}", stats.report());
    println!("\nDestination: {destination}");
    print!("\nSend this payload? [y/N]: ");
    let _ = io::stdout().flush();

    let mut line = String::new();
    if io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_lowercase().as_str(), "y" | "yes")
}

/// Truncate a long preview for display, keeping it readable on a terminal.
pub fn truncate_preview(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let head: String = s.chars().take(max_chars).collect();
    format!("{head}\n… [truncated; full payload written to disk]")
}
