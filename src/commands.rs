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

/// Logical top-right position for the float overlay (small margin from the edge).
fn float_position(app: &AppHandle) -> (f64, f64) {
    const W: f64 = 160.0;
    const MARGIN: f64 = 24.0;
    if let Some(main) = app.get_webview_window("main") {
        if let Ok(Some(m)) = main.primary_monitor() {
            let scale = m.scale_factor();
            let logical_w = m.size().width as f64 / scale;
            let x = (logical_w - W - MARGIN).max(0.0);
            return (x, MARGIN);
        }
    }
    (80.0, 80.0)
}

/// Create (or reuse) and show the float overlay. MUST be called on the main
/// thread (webview creation requirement) — callers schedule via run_on_main_thread.
fn open_and_show_float(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    if let Some(w) = app.get_webview_window("float") {
        let _ = w.show();
        return Ok(w);
    }
    let (x, y) = float_position(app);
    let w = WebviewWindowBuilder::new(app, "float", WebviewUrl::App("float.html".into()))
        .title("Sai Recorder - Recording")
        .inner_size(160.0, 48.0)
        .position(x, y)
        .resizable(false)
        .decorations(false)
        .skip_taskbar(true)
        .always_on_top(true)
        .content_protected(true)
        .focusable(false)
        .visible(true)
        .build()?;
    let _ = w.show();
    let _ = w.set_always_on_top(true);
    Ok(w)
}

/// Schedule float-overlay creation on the main thread (non-blocking for caller).
fn schedule_open_float(app: &AppHandle) {
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || match open_and_show_float(&app2) {
        Ok(_) => crate::log::line("float window result: ok"),
        Err(e) => crate::log::line(&format!("float window result: err: {e}")),
    });
}

fn close_float(app: &AppHandle) {
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(w) = app2.get_webview_window("float") {
            let _ = w.close();
        }
    });
}

#[tauri::command]
pub async fn open_float_window(app: AppHandle) -> Result<(), String> {
    schedule_open_float(&app);
    Ok(())
}

// ── Recording control ────────────────────────────────────────────────────────
// NOTE: start/stop are `#[tauri::command(async)]` so they run on the threadpool,
// NOT the main thread. A sync command would block the main thread's message pump
// while it runs — freezing our own webview (no input / no `invoke`). Window
// creation is still marshaled to the main thread via run_on_main_thread.

#[tauri::command(async)]
pub fn start_recording(app: AppHandle, opts: RecordOptionsDto) -> Result<(), String> {
    crate::log::line("start_recording invoked");
    let state = app.state::<AppState>();

    if state.replaying.load(SeqCst) {
        crate::log::line("start_recording: rejected (replay in progress)");
        return Err("A replay is in progress.".into());
    }
    if state.rec.lock().unwrap().is_some() {
        crate::log::line("start_recording: rejected (already recording)");
        return Err("Already recording.".into());
    }

    let options = opts.into_options();
    crate::log::line(&format!(
        "options: mode={:?} fps={} history={}s crop={} lossy={} quality={} max_dim={:?}",
        options.mode,
        options.fps,
        options.history_secs,
        options.compression.crop_focus,
        options.compression.lossy,
        options.compression.quality,
        options.compression.max_dim
    ));

    // Spawn the worker (records until stop). The global hotkey calls perform_stop.
    let stop = Arc::new(AtomicBool::new(false));
    let app_hk = app.clone();
    let on_hotkey: recorder::StopHotkey = Arc::new(move || {
        let app = app_hk.clone();
        std::thread::spawn(move || {
            crate::log::line("stop via hotkey");
            let _ = perform_stop(&app);
        });
    });
    let worker = recorder::start_worker(options.clone(), stop.clone(), on_hotkey);
    crate::log::line("worker spawned");

    *state.rec.lock().unwrap() = Some(Session { stop, options, worker });
    let _ = app.emit("recording-started", ());

    // Create the overlay on the main thread (non-fatal, non-blocking).
    crate::log::line("scheduling float window on main thread");
    schedule_open_float(&app);
    Ok(())
}

#[tauri::command(async)]
pub fn stop_recording(app: AppHandle) -> Result<TaskMeta, String> {
    perform_stop(&app)
}

/// Shared stop + finalize used by the Stop button(s) AND the global hotkey.
/// Whoever takes the session from the mutex first performs the finalize; the
/// other caller sees `None` and no-ops.
fn perform_stop(app: &AppHandle) -> Result<TaskMeta, String> {
    crate::log::line("stop invoked");
    let session = {
        let state = app.state::<AppState>();
        let taken = state.rec.lock().unwrap().take();
        taken
    };
    let session = match session {
        Some(s) => s,
        None => {
            crate::log::line("stop: rejected (not recording)");
            return Err("Not recording.".into());
        }
    };
    session.stop.store(true, SeqCst);
    let output = match session.worker.join() {
        Ok(o) => o,
        Err(_) => {
            crate::log::line("stop: worker thread panicked");
            return Err("Recorder thread panicked.".into());
        }
    };
    crate::log::line(&format!(
        "worker joined: {} actions, {} captures, baseline={}B compressed={}B",
        output.actions.len(),
        output.stats.shots,
        output.stats.baseline_bytes,
        output.stats.compressed_bytes
    ));

    let meta = match tasks::save_recording(&output, &session.options) {
        Ok(m) => m,
        Err(e) => {
            crate::log::line(&format!("export error: {e}"));
            close_float(app);
            return Err(e.to_string());
        }
    };
    crate::log::line(&format!(
        "task saved {} -> {} | tokens baseline={} compressed={} ratio={} | size ratio={}",
        meta.id,
        meta.name,
        meta.compression.get("baselineTokensEst").cloned().unwrap_or_default(),
        meta.compression.get("compressedTokensEst").cloned().unwrap_or_default(),
        meta.compression.get("tokenRatio").cloned().unwrap_or_default(),
        meta.compression.get("sizeRatio").cloned().unwrap_or_default()
    ));

    close_float(app);
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

// ── Playback / Export ────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct PlaybackFrame {
    pub src: String,
    pub t_ms: f64,
    pub label: String,
}

#[derive(serde::Serialize)]
pub struct PlaybackDto {
    pub frames: Vec<PlaybackFrame>,
    pub duration_ms: f64,
    pub count: usize,
}

fn variant_label(a: &crate::actions::UserAction) -> String {
    use crate::actions::UserAction::*;
    match a {
        Hover { .. } => "Hover".into(),
        Click { .. } => "Click".into(),
        DoubleClick { .. } => "Double Click".into(),
        TripleClick { .. } => "Triple Click".into(),
        Drag { .. } => "Drag".into(),
        Scroll { .. } => "Scroll".into(),
        Type { .. } => "Type".into(),
        Press { .. } => "Press".into(),
    }
}

#[tauri::command]
pub fn get_task_playback(id: String) -> Result<PlaybackDto, String> {
    let actions = crate::tasks::load_actions(&id).map_err(|e| e.to_string())?;
    let mut frames: Vec<PlaybackFrame> = Vec::new();
    for a in &actions {
        let b = a.base();
        let label = variant_label(a);
        for (src, t) in [
            (&b.before.capture, b.timestamp),
            (&b.after.capture, b.timestamp + b.duration),
        ] {
            if src.is_empty() {
                continue;
            }
            if frames.last().map(|f| &f.src == src).unwrap_or(false) {
                continue;
            }
            frames.push(PlaybackFrame {
                src: src.clone(),
                t_ms: t,
                label: label.clone(),
            });
        }
    }
    let duration_ms = frames.last().map(|f| f.t_ms).unwrap_or(0.0);
    let count = frames.len();
    Ok(PlaybackDto {
        frames,
        duration_ms,
        count,
    })
}

#[tauri::command]
pub fn export_task_json(id: String) -> Result<String, String> {
    crate::tasks::compact_payload(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn export_task_jsonl(id: String) -> Result<String, String> {
    crate::tasks::export_jsonl(&id).map_err(|e| e.to_string())
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

// ── Headless self-test ───────────────────────────────────────────────────────

/// Exercises the real compress → export → meta path without the GUI and logs
/// the resulting token metrics. Invoked via `sai-recorder.exe --selftest`.
pub fn selftest() {
    use image::{ColorType, ImageEncoder};

    crate::log::line("selftest: begin");

    // Synthetic high-entropy frame (noise) at a realistic resolution, encoded to
    // lossless WebP exactly like the ring buffer — so the baseline is sizable and
    // the lossy+downscaled output shows real savings (like a true screenshot).
    let img = image::RgbaImage::from_fn(1280, 800, |x, y| {
        let h = x
            .wrapping_mul(2_654_435_761)
            ^ y.wrapping_mul(40_503)
            ^ x.wrapping_add(y).wrapping_mul(2_246_822_519);
        image::Rgba([(h & 0xff) as u8, ((h >> 8) & 0xff) as u8, ((h >> 16) & 0xff) as u8, 255])
    });
    let mut baseline = Vec::new();
    if image::codecs::webp::WebPEncoder::new_lossless(&mut baseline)
        .write_image(img.as_raw(), img.width(), img.height(), ColorType::Rgba8.into())
        .is_err()
    {
        crate::log::line("selftest: webp encode failed");
        println!("SELFTEST FAIL: webp encode");
        return;
    }

    let options = RecordOptions {
        mode: Mode::Sai,
        compression: CompressionOptions { lossy: true, quality: 70, max_dim: Some(256), crop_focus: false },
        fps: 10.0,
        history_secs: 10,
    };

    // Real compression of a few captures.
    let mut stats = crate::compress::CompressionStats::default();
    let mut sample_url = String::new();
    for _ in 0..3 {
        let enc = crate::compress::compress_frame(&baseline, None, &options.compression);
        stats.add_shot(baseline.len(), enc.bytes.len());
        sample_url = enc.data_url();
    }

    let mk_cap = || crate::actions::Capture {
        capture: sample_url.clone(),
        focused: crate::actions::Focused {
            window: None,
            screen: crate::capture::FocusedScreenMetadata {
                id: None, name: None, x: None, y: None, width: None, height: None, primary: None,
            },
        },
    };
    let mut actions = Vec::new();
    for i in 0..2u64 {
        actions.push(UserAction::Click {
            base: crate::actions::BaseAction {
                timestamp: (i * 100) as f64,
                duration: 10.0,
                before: mk_cap(),
                after: mk_cap(),
            },
            button: Button::Left,
            coordinate: crate::actions::Coordinate { x: 10.0, y: 20.0 },
            keys: None,
        });
    }

    let output = RecorderOutput {
        actions,
        stats,
        started_ms: 0,
        ended_ms: 1000,
        duration_ms: 1000,
    };

    match tasks::save_recording(&output, &options) {
        Ok(meta) => {
            crate::log::line(&format!("selftest: task saved {} | compression={}", meta.id, meta.compression));
            println!("SELFTEST OK task={} compression={}", meta.id, meta.compression);
        }
        Err(e) => {
            crate::log::line(&format!("selftest: save error: {e}"));
            println!("SELFTEST FAIL: {e}");
        }
    }
}

/// Headless verification of the LIVE pipeline: spawns the real worker, injects a
/// couple of benign input events + the Ctrl+Alt+S hotkey, then stops and saves —
/// confirming (a) events are captured (listen not blocked), (c) the hotkey fires,
/// and that a real recorded session writes token metrics. Run via
/// `sai-recorder.exe --selftest-record`.
pub fn selftest_record() {
    use rdev::{simulate, Button, EventType, Key};

    crate::log::line("selftest_record: begin");
    let options = RecordOptions {
        mode: Mode::Sai,
        compression: CompressionOptions { lossy: true, quality: 60, max_dim: Some(384), crop_focus: false },
        fps: 6.0,
        history_secs: 4,
    };

    let stop = Arc::new(AtomicBool::new(false));
    let fired = Arc::new(AtomicBool::new(false));
    let fired2 = fired.clone();
    let on_hotkey: recorder::StopHotkey = Arc::new(move || {
        fired2.store(true, SeqCst);
        crate::log::line("selftest_record: on_hotkey fired");
    });

    let worker = recorder::start_worker(options.clone(), stop.clone(), on_hotkey);
    std::thread::sleep(Duration::from_millis(900)); // warm up listener + ring buffer

    let tap = |et: EventType| {
        let _ = simulate(&et);
        std::thread::sleep(Duration::from_millis(140));
    };
    // One benign real action (move + a single wheel notch) → exercises capture+compress.
    tap(EventType::MouseMove { x: 500.0, y: 400.0 });
    tap(EventType::Wheel { delta_x: 0, delta_y: -1 });
    let _ = simulate(&EventType::ButtonPress(Button::Middle)); // no-op-ish; ensure an action flushes
    std::thread::sleep(Duration::from_millis(120));
    let _ = simulate(&EventType::ButtonRelease(Button::Middle));
    std::thread::sleep(Duration::from_millis(1100)); // allow debounce flush

    // Global stop hotkey: Ctrl+Alt+S
    tap(EventType::KeyPress(Key::ControlLeft));
    tap(EventType::KeyPress(Key::Alt));
    tap(EventType::KeyPress(Key::KeyS));
    tap(EventType::KeyRelease(Key::KeyS));
    tap(EventType::KeyRelease(Key::Alt));
    tap(EventType::KeyRelease(Key::ControlLeft));
    std::thread::sleep(Duration::from_millis(400));

    stop.store(true, SeqCst); // belt-and-suspenders if injection didn't register
    let output = match worker.join() {
        Ok(o) => o,
        Err(_) => {
            println!("SELFTEST_RECORD FAIL: worker panicked");
            return;
        }
    };
    let hotkey_ok = fired.load(SeqCst);
    crate::log::line(&format!(
        "selftest_record: actions={} captures={} hotkey_fired={}",
        output.actions.len(),
        output.stats.shots,
        hotkey_ok
    ));
    match tasks::save_recording(&output, &options) {
        Ok(meta) => {
            crate::log::line(&format!("selftest_record: task saved {} | compression={}", meta.id, meta.compression));
            println!(
                "SELFTEST_RECORD OK actions={} captures={} hotkey_fired={} task={} compression={}",
                output.actions.len(), output.stats.shots, hotkey_ok, meta.id, meta.compression
            );
        }
        Err(e) => println!("SELFTEST_RECORD FAIL save: {e}"),
    }
}
