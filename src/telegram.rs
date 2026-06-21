//! Telegram Bot API sender — delivers an **approved** compressed payload to the
//! user's own Sai agent. This is intentionally a thin stub: the actual network
//! send only runs after [`crate::review::confirm_upload`] returns true.
//!
//! Configuration is read from env vars or a JSON config file:
//!   * `SAI_TG_BOT_TOKEN` / `SAI_TG_CHAT_ID`, or
//!   * `sai-recorder.config.json` → `{ "bot_token": "...", "chat_id": "..." }`

use std::path::Path;

use serde::Deserialize;

const CONFIG_FILE: &str = "sai-recorder.config.json";
const ENV_TOKEN: &str = "SAI_TG_BOT_TOKEN";
const ENV_CHAT: &str = "SAI_TG_CHAT_ID";

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramConfig {
    pub bot_token: String,
    pub chat_id: String,
}

impl TelegramConfig {
    /// Resolve config from env vars first, then a local JSON file.
    pub fn load() -> Option<Self> {
        if let (Ok(bot_token), Ok(chat_id)) = (std::env::var(ENV_TOKEN), std::env::var(ENV_CHAT)) {
            if !bot_token.is_empty() && !chat_id.is_empty() {
                return Some(Self { bot_token, chat_id });
            }
        }
        Self::from_file(Path::new(CONFIG_FILE))
    }

    pub fn from_file(path: &Path) -> Option<Self> {
        let raw = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&raw).ok()
    }
}

/// Send a text payload to the configured chat via the Telegram Bot API.
///
/// NOTE: callers MUST gate this behind the local review confirmation. The
/// recorder never calls `send` until the user types `y`.
pub fn send(config: &TelegramConfig, payload: &str) -> Result<(), String> {
    let url = format!("https://api.telegram.org/bot{}/sendMessage", config.bot_token);

    // Telegram messages cap at 4096 chars; for larger payloads a real
    // implementation would send a document. TODO: switch to sendDocument for
    // full trajectories / large workflows.
    let text = if payload.chars().count() > 4000 {
        format!(
            "{}\n… [truncated: {} chars total — use sendDocument for full payload]",
            payload.chars().take(4000).collect::<String>(),
            payload.chars().count()
        )
    } else {
        payload.to_string()
    };

    let client = reqwest::blocking::Client::new();
    let resp = client
        .post(url.as_str())
        .json(&serde_json::json!({
            "chat_id": config.chat_id,
            "text": text,
        }))
        .send()
        .map_err(|e| format!("request failed: {e}"))?;

    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("telegram returned status {}", resp.status()))
    }
}
