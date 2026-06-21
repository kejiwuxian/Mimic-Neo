//! Tauri command layer: recording control, task library management, and replay.

use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use rdev::{simulate, Button, EventType, Key};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::actions::{Direction, UserAction};
use crate::compress::CompressionOptions;
use crate::recorder::{self, Mode, RecordOptions, RecorderOutput};
use crate::tasks::{self, TaskDetail, TaskMeta};
use crate::telegram;

// ── Shared application state (Tauri-managed) ─────────────────────────────────

#[derive(Default)]
pub struct AppState {
    rec: Mutex<Option<Session>>,
    replaying: Arc<AtomicBool>,
}

struct Session {
    stop: Arc<AtomicBool>,
    options: RecordOptions,
    worker: JoinHandle<RecorderOutput>,
}

/// Recording options as sent from the frontend.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordOptionsDto {
    pub mode: String,
    pub fps: f32,
    pub history_secs: u64,
    pub crop: bool,
    pub lossy: bool,
    pub quality: u8,
    pub max_dim: Option<u32>,
}

impl RecordOptionsDto {
    fn into_options(self) -> RecordOptions {
        let mode = if self.mode.eq_ignore_ascii_case("dataset") {
            Mode::Dataset
        } else {
            Mode::Sai
        };
        RecordOptions {
            mode,
            compression: CompressionOptions {
                lossy: self.lossy,
                quality: self.quality,
                max_dim: self.max_dim,
                crop_focus: self.crop,
            },
            fps: if self.fps > 0.0 { self.fps } else { 10.0 },
            history_secs: self.history_secs.max(1),
        }
    }
}

#[derive(Serialize)]
pub struct TelegramStatus {
    pub configured: bool,
}

// ── Float overlay window ─────────────────────────────────────────────────────

fn ensure_float_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    if let Some(w) = app.get_webview_window("float") {
        return Ok(w);
    }
    WebviewWindowBuilder::new(app, "float", WebviewUrl::App("float.html".into()))
        .title("Sai Recorder - Recording")
        .inner_size(160.0, 48.0)
        .resizable(false)
        .decorations(false)
        .skip_taskbar(true)
        .always_on_top(true)
        .content_protected(true)
        .visible(false)
        .focusable(false)
        .build()
}

#[cfg(windows)]
fn window_hwnd_isize(w: &WebviewWindow) -> isize {
    w.hwnd().map(|h| h.0 as isize).unwrap_or(0)
}

#[cfg(not(windows))]
fn window_hwnd_isize(_w: &WebviewWindow) -> isize {
    0
}

#[tauri::command]
pub async fn open_float_window(app: AppHandle) -> Result<(), String> {
    let w = ensure_float_window(&app).map_err(|e| e.to_string())?;
    w.show().map_err(|e| e.to_string())?;
    Ok(())
}

// ── Recording control ────────────────────────────────────────────────────────

#[tauri::command]
pub fn start_recording(
    app: AppHandle,
    state: State<'_, AppState>,
    opts: RecordOptionsDto,
) -> Result<(), String> {
    if state.replaying.load(SeqCst) {
        return Err("A replay is in progress.".into());
    }
    if state.rec.lock().unwrap().is_some() {
        return Err("Already recording.".into());
    }

    let options = opts.into_options();

    // Float control window: show it and grab its HWND so its own input is ignored.
    let w = ensure_float_window(&app).map_err(|e| e.to_string())?;
    let _ = w.show();
    let hwnd = window_hwnd_isize(&w);

    let stop = Arc::new(AtomicBool::new(false));
    let worker = recorder::start_worker(options.clone(), stop.clone(), hwnd);

    *state.rec.lock().unwrap() = Some(Session { stop, options, worker });
    let _ = app.emit("recording-started", ());
    Ok(())
}

#[tauri::command]
pub fn stop_recording(app: AppHandle, state: State<'_, AppState>) -> Result<TaskMeta, String> {
    let session = state.rec.lock().unwrap().take().ok_or("Not recording.")?;
    session.stop.store(true, SeqCst);
    let output = session.worker.join().map_err(|_| "Recorder thread panicked.")?;

    let meta = tasks::save_recording(&output, &session.options).map_err(|e| e.to_string())?;

    if let Some(w) = app.get_webview_window("float") {
        let _ = w.close();
    }
    let _ = app.emit("recording-finished", meta.clone());
    Ok(meta)
}

#[tauri::command]
pub fn recording_state(state: State<'_, AppState>) -> bool {
    state.rec.lock().unwrap().is_some()
}

// ── Task library ─────────────────────────────────────────────────────────────

#[tauri::command]
pub fn list_tasks() -> Vec<TaskMeta> {
    tasks::list_tasks()
}

#[tauri::command]
pub fn get_task(id: String) -> Result<TaskDetail, String> {
    tasks::get_task(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn rename_task(id: String, name: String) -> Result<TaskMeta, String> {
    tasks::rename_task(&id, &name).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_task(id: String) -> Result<(), String> {
    tasks::delete_task(&id).map_err(|e| e.to_string())
}

// ── Telegram (optional) ──────────────────────────────────────────────────────

#[tauri::command]
pub fn get_telegram_status() -> TelegramStatus {
    TelegramStatus {
        configured: telegram::TelegramConfig::load().is_some(),
    }
}

#[tauri::command]
pub fn send_task_telegram(id: String) -> Result<(), String> {
    let cfg = telegram::TelegramConfig::load().ok_or("No Telegram config (set SAI_TG_BOT_TOKEN / SAI_TG_CHAT_ID).")?;
    let payload = tasks::compact_payload(&id).map_err(|e| e.to_string())?;
    telegram::send(&cfg, &payload).map_err(|e| e)
}

// ── Replay ───────────────────────────────────────────────────────────────────

#[tauri::command]
pub fn run_task(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<(), String> {
    if state.rec.lock().unwrap().is_some() {
        return Err("Stop recording before replaying.".into());
    }
    if state.replaying.swap(true, SeqCst) {
        return Err("A replay is already running.".into());
    }

    let actions = match tasks::load_actions(&id) {
        Ok(a) => a,
        Err(e) => {
            state.replaying.store(false, SeqCst);
            return Err(e.to_string());
        }
    };

    let replaying = state.replaying.clone();
    let app2 = app.clone();
    std::thread::spawn(move || {
        replay(&app2, &actions);
        replaying.store(false, SeqCst);
        let _ = app2.emit("replay-finished", ());
    });
    Ok(())
}

/// Inter-event spacing and caps for replay.
const STEP_PAUSE: Duration = Duration::from_millis(14);
const MAX_GAP_MS: f64 = 3000.0;

fn replay(app: &AppHandle, actions: &[UserAction]) {
    // 3-2-1 countdown so the user can focus the target window.
    for n in (1..=3).rev() {
        let _ = app.emit("replay-countdown", n);
        std::thread::sleep(Duration::from_secs(1));
    }
    let _ = app.emit("replay-countdown", 0);

    let total = actions.len();
    let mut prev_end_ms = actions.first().map(|a| a.base().timestamp).unwrap_or(0.0);

    for (i, action) in actions.iter().enumerate() {
        let base = action.base();
        let gap = (base.timestamp - prev_end_ms).clamp(0.0, MAX_GAP_MS);
        if gap > 0.0 {
            std::thread::sleep(Duration::from_millis(gap as u64));
        }
        prev_end_ms = base.timestamp + base.duration;

        play_action(action);
        let _ = app.emit("replay-progress", serde_json::json!({ "index": i + 1, "total": total }));
    }
}

fn play_action(action: &UserAction) {
    match action {
        UserAction::Hover { coordinate, .. } => {
            move_to(coordinate.x, coordinate.y);
        }
        UserAction::Click { button, coordinate, .. } => {
            move_to(coordinate.x, coordinate.y);
            do_click(*button, 1);
        }
        UserAction::DoubleClick { button, coordinate, .. } => {
            move_to(coordinate.x, coordinate.y);
            do_click(*button, 2);
        }
        UserAction::TripleClick { button, coordinate, .. } => {
            move_to(coordinate.x, coordinate.y);
            do_click(*button, 3);
        }
        UserAction::Drag { start_coordinate, coordinate, .. } => {
            move_to(start_coordinate.x, start_coordinate.y);
            send(&EventType::ButtonPress(Button::Left));
            std::thread::sleep(STEP_PAUSE);
            // A few interpolated moves help apps register the drag.
            for t in 1..=8 {
                let f = t as f64 / 8.0;
                let x = start_coordinate.x + (coordinate.x - start_coordinate.x) * f;
                let y = start_coordinate.y + (coordinate.y - start_coordinate.y) * f;
                move_to(x, y);
            }
            send(&EventType::ButtonRelease(Button::Left));
            std::thread::sleep(STEP_PAUSE);
        }
        UserAction::Scroll { coordinate, direction, amount, .. } => {
            move_to(coordinate.x, coordinate.y);
            let steps = (*amount).max(1);
            let (dx, dy) = match direction {
                Direction::Up => (0i64, 1i64),
                Direction::Down => (0, -1),
                Direction::Right => (1, 0),
                Direction::Left => (-1, 0),
            };
            for _ in 0..steps {
                send(&EventType::Wheel { delta_x: dx, delta_y: dy });
                std::thread::sleep(STEP_PAUSE);
            }
        }
        UserAction::Type { text, .. } => {
            for ch in text.chars() {
                type_char(ch);
            }
        }
        UserAction::Press { keys, .. } => {
            for key in keys {
                send(&EventType::KeyPress(*key));
                std::thread::sleep(STEP_PAUSE);
                send(&EventType::KeyRelease(*key));
                std::thread::sleep(STEP_PAUSE);
            }
        }
    }
}

fn send(ev: &EventType) {
    let _ = simulate(ev);
}

fn move_to(x: f64, y: f64) {
    send(&EventType::MouseMove { x, y });
    std::thread::sleep(STEP_PAUSE);
}

fn do_click(button: Button, times: u32) {
    for _ in 0..times {
        send(&EventType::ButtonPress(button));
        std::thread::sleep(STEP_PAUSE);
        send(&EventType::ButtonRelease(button));
        std::thread::sleep(STEP_PAUSE);
    }
}

fn type_char(ch: char) {
    match ch {
        '\n' => tap(Key::Return, false),
        '\t' => tap(Key::Tab, false),
        ' ' => tap(Key::Space, false),
        _ => {
            if let Some((key, shift)) = char_to_key(ch) {
                tap(key, shift);
            }
            // Unmapped (e.g. non-US / unicode) characters are skipped.
            // TODO: full keyboard-layout / unicode support for Type replay.
        }
    }
}

fn tap(key: Key, shift: bool) {
    if shift {
        send(&EventType::KeyPress(Key::ShiftLeft));
        std::thread::sleep(STEP_PAUSE);
    }
    send(&EventType::KeyPress(key));
    std::thread::sleep(STEP_PAUSE);
    send(&EventType::KeyRelease(key));
    std::thread::sleep(STEP_PAUSE);
    if shift {
        send(&EventType::KeyRelease(Key::ShiftLeft));
        std::thread::sleep(STEP_PAUSE);
    }
}

/// Map a character to an rdev key + whether Shift is needed (US QWERTY layout).
fn char_to_key(ch: char) -> Option<(Key, bool)> {
    use Key::*;
    let lower = ch.to_ascii_lowercase();
    let letter = match lower {
        'a' => Some(KeyA), 'b' => Some(KeyB), 'c' => Some(KeyC), 'd' => Some(KeyD),
        'e' => Some(KeyE), 'f' => Some(KeyF), 'g' => Some(KeyG), 'h' => Some(KeyH),
        'i' => Some(KeyI), 'j' => Some(KeyJ), 'k' => Some(KeyK), 'l' => Some(KeyL),
        'm' => Some(KeyM), 'n' => Some(KeyN), 'o' => Some(KeyO), 'p' => Some(KeyP),
        'q' => Some(KeyQ), 'r' => Some(KeyR), 's' => Some(KeyS), 't' => Some(KeyT),
        'u' => Some(KeyU), 'v' => Some(KeyV), 'w' => Some(KeyW), 'x' => Some(KeyX),
        'y' => Some(KeyY), 'z' => Some(KeyZ),
        _ => None,
    };
    if let Some(k) = letter {
        return Some((k, ch.is_ascii_uppercase()));
    }
    // Digits and unshifted symbols.
    let plain = match ch {
        '1' => Some(Num1), '2' => Some(Num2), '3' => Some(Num3), '4' => Some(Num4),
        '5' => Some(Num5), '6' => Some(Num6), '7' => Some(Num7), '8' => Some(Num8),
        '9' => Some(Num9), '0' => Some(Num0),
        '-' => Some(Minus), '=' => Some(Equal),
        '[' => Some(LeftBracket), ']' => Some(RightBracket), '\\' => Some(BackSlash),
        ';' => Some(SemiColon), '\'' => Some(Quote), '`' => Some(BackQuote),
        ',' => Some(Comma), '.' => Some(Dot), '/' => Some(Slash),
        _ => None,
    };
    if let Some(k) = plain {
        return Some((k, false));
    }
    // Shifted symbols.
    let shifted = match ch {
        '!' => Some(Num1), '@' => Some(Num2), '#' => Some(Num3), '$' => Some(Num4),
        '%' => Some(Num5), '^' => Some(Num6), '&' => Some(Num7), '*' => Some(Num8),
        '(' => Some(Num9), ')' => Some(Num0),
        '_' => Some(Minus), '+' => Some(Equal),
        '{' => Some(LeftBracket), '}' => Some(RightBracket), '|' => Some(BackSlash),
        ':' => Some(SemiColon), '"' => Some(Quote), '~' => Some(BackQuote),
        '<' => Some(Comma), '>' => Some(Dot), '?' => Some(Slash),
        _ => None,
    };
    shifted.map(|k| (k, true))
}
