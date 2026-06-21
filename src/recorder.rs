//! End-to-end recording: `capture(ring buffer) → state machine → before/after
//! captures → compress → export`.
//!
//! The state machine is a faithful port of MimicCLI's `src/new_record.rs`,
//! restructured from coexisting closures into a single `RecorderState` struct
//! (cleaner borrows). Differences, per the user's spec:
//!   * `timestamp`/`duration` are **milliseconds** (reference used seconds);
//!   * action fields are `before`/`after` (reference used `*_screenshot`);
//!   * `keys`/`button` are native rdev types (reference stringified them);
//!   * `Capture.focused` is `{ window, screen }` (reference had window only).
//!
//! Recording is opt-in (the `record` command) and stops when you press Enter.
//! Nothing is sent anywhere without passing the local review gate first.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rdev::{Button, Event, EventType, Key};

use crate::actions::{
    distance, held_keys, scroll_amount, scroll_direction, BaseAction, Capture, Coordinate, Focused,
    UserAction,
};
use crate::capture;
use crate::compress::{CompressionOptions, CompressionStats};
use crate::export;
use crate::overlay::{self, Shared};
use crate::review;
use crate::telegram;

// ── Tuning (from new_record.rs) ──────────────────────────────────────────────
const DEBOUNCE_DELAY: Duration = Duration::from_millis(800);
const DRAG_THRESHOLD: f64 = 10.0;
const CLICK_TIME_THRESHOLD: Duration = Duration::from_millis(400);
const CLICK_DIST_THRESHOLD: f64 = 5.0;
const SCREENSHOT_BEFORE_OFFSET: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Sai,
    Dataset,
}

#[derive(Debug, Clone)]
pub struct RecordOptions {
    pub mode: Mode,
    pub out_dir: PathBuf,
    pub compression: CompressionOptions,
    pub fps: f32,
    pub history_secs: u64,
}

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Build the focus context (`{ window, screen }`) at the current instant.
fn focused_now() -> Focused {
    match capture::get_focused_context() {
        Ok(ctx) => Focused {
            window: ctx.window.as_ref().map(capture::get_window_metadata),
            screen: capture::get_screen_metadata(&ctx.screen),
        },
        Err(_) => Focused {
            window: None,
            screen: capture::get_screen_metadata(&capture::get_primary_screen()),
        },
    }
}

// ── State machine ────────────────────────────────────────────────────────────

struct RecorderState {
    opts: CompressionOptions,
    recording_start: SystemTime,

    pressed: HashSet<Key>,
    last_x: f64,
    last_y: f64,
    drag_start_x: f64,
    drag_start_y: f64,
    button_down: Option<Button>,
    button_down_time: Option<SystemTime>,

    last_click_time: Option<Instant>,
    last_click_x: f64,
    last_click_y: f64,
    click_count: u32,

    key_buffer: String,
    typing_start: Option<SystemTime>,
    before_typing: Option<Capture>,
    before_mouse: Option<Capture>,
    before_scroll: Option<Capture>,

    scroll_delta_x: i64,
    scroll_delta_y: i64,
    scroll_start: Option<SystemTime>,

    pending_press: Option<(Key, SystemTime, Capture)>,

    actions: Vec<UserAction>,
    stats: CompressionStats,
}

impl RecorderState {
    fn new(opts: CompressionOptions, recording_start: SystemTime) -> Self {
        Self {
            opts,
            recording_start,
            pressed: HashSet::new(),
            last_x: 0.0,
            last_y: 0.0,
            drag_start_x: 0.0,
            drag_start_y: 0.0,
            button_down: None,
            button_down_time: None,
            last_click_time: None,
            last_click_x: 0.0,
            last_click_y: 0.0,
            click_count: 0,
            key_buffer: String::new(),
            typing_start: None,
            before_typing: None,
            before_mouse: None,
            before_scroll: None,
            scroll_delta_x: 0,
            scroll_delta_y: 0,
            scroll_start: None,
            pending_press: None,
            actions: Vec::new(),
            stats: CompressionStats::default(),
        }
    }

    fn ms_since(&self, t: SystemTime) -> f64 {
        t.duration_since(self.recording_start)
            .unwrap_or_default()
            .as_secs_f64()
            * 1000.0
    }

    /// Region of the focused window within the captured (primary) screen frame.
    fn crop_region(&self, focused: &Focused) -> Option<(u32, u32, u32, u32)> {
        if !self.opts.crop_focus || focused.screen.primary != Some(true) {
            return None;
        }
        let w = focused.window.as_ref()?;
        let rx = (w.x? - focused.screen.x.unwrap_or(0)).max(0) as u32;
        let ry = (w.y? - focused.screen.y.unwrap_or(0)).max(0) as u32;
        Some((rx, ry, w.width?, w.height?))
    }

    /// Build a [`Capture`] from the ring-buffer frame `offset` ago, applying
    /// compression and tallying baseline-vs-compressed bytes.
    fn make_shot(&mut self, offset: Duration) -> Capture {
        let focused = focused_now();
        let crop = self.crop_region(&focused);
        let capture = match capture::retrieve_frame(offset) {
            Some(bytes) => {
                let enc = crate::compress::compress_frame(&bytes, crop, &self.opts);
                self.stats.add_shot(bytes.len(), enc.bytes.len());
                enc.data_url()
            }
            None => String::new(),
        };
        Capture { capture, focused }
    }

    fn flush_typing(&mut self, now: SystemTime, shared_after: Option<&Capture>) -> bool {
        if self.key_buffer.is_empty() {
            return false;
        }
        let ts = self.ms_since(self.typing_start.unwrap_or(now));
        let duration = self.ms_since(now) - ts;
        let before = self
            .before_typing
            .take()
            .unwrap_or_else(|| self.make_shot(SCREENSHOT_BEFORE_OFFSET));
        let after = match shared_after {
            Some(c) => c.clone(),
            None => self.make_shot(Duration::ZERO),
        };
        let keys = held_keys(&self.pressed);
        let text = std::mem::take(&mut self.key_buffer);
        self.actions.push(UserAction::Type {
            base: BaseAction { timestamp: ts, duration, before, after },
            text,
            keys,
        });
        self.typing_start = None;
        true
    }

    fn flush_scroll(&mut self, now: SystemTime, shared_after: Option<&Capture>) -> bool {
        if self.scroll_delta_x == 0 && self.scroll_delta_y == 0 {
            return false;
        }
        let ts = self.ms_since(self.scroll_start.unwrap_or(now));
        let duration = self.ms_since(now) - ts;
        let before = self
            .before_scroll
            .take()
            .unwrap_or_else(|| self.make_shot(SCREENSHOT_BEFORE_OFFSET));
        let after = match shared_after {
            Some(c) => c.clone(),
            None => self.make_shot(Duration::ZERO),
        };
        let keys = held_keys(&self.pressed);
        let (dx, dy) = (self.scroll_delta_x, self.scroll_delta_y);
        self.actions.push(UserAction::Scroll {
            base: BaseAction { timestamp: ts, duration, before, after },
            coordinate: Coordinate { x: self.last_x, y: self.last_y },
            direction: scroll_direction(dx, dy),
            amount: scroll_amount(dx, dy),
            keys,
        });
        self.scroll_delta_x = 0;
        self.scroll_delta_y = 0;
        self.scroll_start = None;
        true
    }

    /// Emit a pending Press. `shared_after`, when given, is reused as the
    /// "after" capture (sharing a transition frame with the next action).
    fn flush_press(&mut self, now: SystemTime, shared_after: Option<&Capture>) -> bool {
        let (key, press_time, before) = match self.pending_press.take() {
            Some(v) => v,
            None => return false,
        };
        let ts = self.ms_since(press_time);
        let duration = self.ms_since(now) - ts;
        let after = match shared_after {
            Some(c) => c.clone(),
            None => self.make_shot(Duration::ZERO),
        };
        self.actions.push(UserAction::Press {
            base: BaseAction { timestamp: ts, duration, before, after },
            keys: vec![key],
        });
        true
    }

    fn handle(&mut self, event: Event) {
        match event.event_type {
            EventType::KeyPress(key) => self.on_key_press(key, event.name, event.time),
            EventType::KeyRelease(key) => self.on_key_release(key, event.time),
            EventType::ButtonPress(button) => self.on_button_press(button, event.time),
            EventType::ButtonRelease(button) => self.on_button_release(button, event.time),
            EventType::MouseMove { x, y } => {
                self.last_x = x;
                self.last_y = y;
            }
            EventType::Wheel { delta_x, delta_y } => self.on_wheel(delta_x, delta_y, event.time),
        }
    }

    fn on_key_press(&mut self, key: Key, name: Option<String>, time: SystemTime) {
        self.pressed.insert(key);

        if let Some(name) = name.filter(|s| !s.is_empty()) {
            // Printable character → accumulate into the typing buffer.
            if self.key_buffer.is_empty() {
                self.typing_start = Some(time);
                self.before_typing = Some(self.make_shot(SCREENSHOT_BEFORE_OFFSET));
            }
            self.key_buffer.push_str(&name);
            return;
        }

        // Non-printable key → flush typing/scroll, then track as a pending press
        // (emitted on release for a real duration). Modifiers don't stand alone.
        self.flush_typing(time, None);
        self.flush_scroll(time, None);
        if !crate::actions::MODIFIER_KEYS.contains(&key) {
            self.flush_press(time, None);
            let before = self.make_shot(SCREENSHOT_BEFORE_OFFSET);
            self.pending_press = Some((key, time, before));
        }
    }

    fn on_key_release(&mut self, key: Key, time: SystemTime) {
        self.pressed.remove(&key);
        if matches!(&self.pending_press, Some((pk, ..)) if *pk == key) {
            self.flush_press(time, None);
        }
    }

    fn on_button_press(&mut self, button: Button, time: SystemTime) {
        // Capture one transition frame to share as flushed actions' "after"
        // and the click's "before".
        let shared = self.make_shot(SCREENSHOT_BEFORE_OFFSET);
        self.flush_typing(time, Some(&shared));
        self.flush_scroll(time, Some(&shared));
        self.flush_press(time, Some(&shared));

        self.before_mouse = Some(shared);
        self.button_down = Some(button);
        self.button_down_time = Some(time);
        self.drag_start_x = self.last_x;
        self.drag_start_y = self.last_y;
    }

    fn on_button_release(&mut self, button: Button, time: SystemTime) {
        let press_time = self.button_down_time.unwrap_or(time);
        let ts = self.ms_since(press_time);
        let duration = self.ms_since(time) - ts;

        let is_drag = button == Button::Left
            && distance(self.drag_start_x, self.drag_start_y, self.last_x, self.last_y)
                > DRAG_THRESHOLD;

        self.button_down = None;
        self.button_down_time = None;

        let before = self
            .before_mouse
            .take()
            .unwrap_or_else(|| self.make_shot(SCREENSHOT_BEFORE_OFFSET));
        let after = self.make_shot(Duration::ZERO);
        let keys = held_keys(&self.pressed);

        if is_drag {
            self.actions.push(UserAction::Drag {
                base: BaseAction { timestamp: ts, duration, before, after },
                start_coordinate: Coordinate { x: self.drag_start_x, y: self.drag_start_y },
                coordinate: Coordinate { x: self.last_x, y: self.last_y },
                keys,
            });
            return;
        }

        // Click / DoubleClick / TripleClick.
        let now = Instant::now();
        let same_pos = self.last_click_time.map_or(false, |t| {
            t.elapsed() < CLICK_TIME_THRESHOLD
                && distance(self.last_click_x, self.last_click_y, self.last_x, self.last_y)
                    < CLICK_DIST_THRESHOLD
        });
        self.click_count = if same_pos { self.click_count + 1 } else { 1 };
        self.last_click_time = Some(now);
        self.last_click_x = self.last_x;
        self.last_click_y = self.last_y;

        let base = BaseAction { timestamp: ts, duration, before, after };
        let coordinate = Coordinate { x: self.last_x, y: self.last_y };
        self.actions.push(match self.click_count {
            1 => UserAction::Click { base, button, coordinate, keys },
            2 => UserAction::DoubleClick { base, button, coordinate, keys },
            _ => UserAction::TripleClick { base, button, coordinate, keys },
        });
    }

    fn on_wheel(&mut self, delta_x: i64, delta_y: i64, time: SystemTime) {
        let accumulating = self.scroll_delta_x != 0 || self.scroll_delta_y != 0;
        let dir_changed = accumulating
            && scroll_direction(self.scroll_delta_x, self.scroll_delta_y)
                != scroll_direction(delta_x, delta_y);
        let need_shared =
            dir_changed || !self.key_buffer.is_empty() || self.pending_press.is_some() || !accumulating;

        if need_shared {
            let shared = self.make_shot(SCREENSHOT_BEFORE_OFFSET);
            if dir_changed {
                self.flush_scroll(time, Some(&shared));
            }
            self.flush_typing(time, Some(&shared));
            self.flush_press(time, Some(&shared));
            // Begin a new batch if nothing is currently accumulating.
            if self.scroll_delta_x == 0 && self.scroll_delta_y == 0 {
                self.scroll_start = Some(time);
                self.before_scroll = Some(shared);
            }
        }

        self.scroll_delta_x += delta_x;
        self.scroll_delta_y += delta_y;
    }

    fn finish(&mut self) {
        let now = SystemTime::now();
        self.flush_typing(now, None);
        self.flush_scroll(now, None);
        self.flush_press(now, None);
    }
}

// ── Orchestration ────────────────────────────────────────────────────────────

/// What the worker thread hands back to the main thread for finalization.
struct RecordingResult {
    actions: Vec<UserAction>,
    stats: CompressionStats,
    primary_meta: capture::FocusedScreenMetadata,
    start_ms: u64,
    end_ms: u64,
}

pub fn run(opts: RecordOptions) -> Result<()> {
    std::fs::create_dir_all(&opts.out_dir)?;

    println!("sai-recorder — opt-in recording (MimicCLI-compatible schema)");
    println!("  mode   : {:?}", opts.mode);
    println!("  output : {}", opts.out_dir.display());
    print_compression(&opts.compression);
    println!("\n● RECORDING. A floating control is on screen — click ⏹ Stop (or press <Enter> here) to finish.\n");

    let shared = Shared::new();

    // Worker thread: the whole capture pipeline runs off the main thread so the
    // egui event loop can own the main thread.
    let worker = {
        let shared = shared.clone();
        let opts = opts.clone();
        std::thread::spawn(move || record_worker(opts, shared))
    };

    // Overlay owns the main thread until Stop/close; then it sets `stop`. If it
    // can't launch, fall back to a console Enter-to-stop (single stdin reader,
    // so it never competes with the review prompt below).
    if !overlay::run_overlay(shared.clone()) {
        println!("Press <Enter> to stop recording.");
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
    }
    shared.request_stop();

    let result = match worker.join() {
        Ok(r) => r?,
        Err(_) => anyhow::bail!("recording worker thread panicked"),
    };

    println!("\n■ Stopped. {} action(s) captured.\n", result.actions.len());
    if result.actions.is_empty() {
        println!("Nothing recorded; exiting without writing a payload.");
        return Ok(());
    }

    let mut stats = result.stats;
    match opts.mode {
        Mode::Sai => finalize_sai(&opts, &result.actions, &mut stats),
        Mode::Dataset => finalize_dataset(
            &opts,
            &result.actions,
            &mut stats,
            result.primary_meta,
            result.start_ms,
            result.end_ms,
        ),
    }
}

/// The capture pipeline (worker thread): ring buffer + listener + state machine.
/// Drops any input event that targets the overlay before it can be recorded.
fn record_worker(opts: RecordOptions, shared: Shared) -> Result<RecordingResult> {
    let primary = capture::get_primary_screen();
    let primary_meta = capture::get_screen_metadata(&primary);

    let start_ms = epoch_ms();
    let recording_start = SystemTime::now();

    capture::start_recording(&primary, Duration::from_secs(opts.history_secs), opts.fps)?;
    let event_rx = capture::spawn_listener();

    let mut state = RecorderState::new(opts.compression.clone(), recording_start);
    // Last known cursor position (rdev button/wheel events carry no coords),
    // used for overlay hit-testing.
    let mut cursor = (0.0f64, 0.0f64);

    loop {
        match event_rx.recv_timeout(DEBOUNCE_DELAY) {
            Ok(event) => {
                if let EventType::MouseMove { x, y } = &event.event_type {
                    cursor = (*x, *y);
                }
                // Ignore anything aimed at the overlay (Stop click, dragging,
                // typing while it's focused).
                if overlay::event_targets_overlay(&event, shared.overlay_hwnd(), cursor) {
                    continue;
                }
                state.handle(event);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                let now = SystemTime::now();
                state.flush_typing(now, None);
                state.flush_scroll(now, None);
                if shared.stop_requested() {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    state.finish();
    capture::stop_recording().ok();

    let end_ms = epoch_ms();
    let RecorderState { actions, stats, .. } = state;
    Ok(RecordingResult { actions, stats, primary_meta, start_ms, end_ms })
}

fn print_compression(c: &CompressionOptions) {
    if !c.is_active() {
        println!("  capture: lossless full-frame WebP (MimicCLI baseline)");
        return;
    }
    let codec = if c.lossy { format!("lossy JPEG q{}", c.quality) } else { "lossless WebP".into() };
    let scale = c.max_dim.map(|d| format!(", max {d}px")).unwrap_or_default();
    let crop = if c.crop_focus { ", focus-cropped" } else { "" };
    println!("  capture: {codec}{scale}{crop}");
}

fn finalize_sai(opts: &RecordOptions, actions: &[UserAction], stats: &mut CompressionStats) -> Result<()> {
    stats.set_json(&export::structural_json(actions));
    let json = export::sai_json(actions);
    let path = opts.out_dir.join("actions.json");
    std::fs::write(&path, &json)?;
    println!("Wrote {}", path.display());

    let preview = review::truncate_preview(&export::sai_json_preview(actions), 3500);
    gated_send(opts, &preview, stats, &json)
}

fn finalize_dataset(
    opts: &RecordOptions,
    actions: &[UserAction],
    stats: &mut CompressionStats,
    primary: capture::FocusedScreenMetadata,
    session_start_ms: u64,
    session_end_ms: u64,
) -> Result<()> {
    stats.set_json(&export::structural_json(actions));
    let manifest = export::build_manifest(
        std::env::consts::OS.to_string(),
        session_start_ms,
        session_end_ms,
        primary,
        actions.len(),
        stats.summary(),
    );
    let written = export::write_dataset(&opts.out_dir, actions, &manifest)?;
    println!("Wrote {}", written.jsonl.display());
    println!("Wrote {}", written.manifest.display());
    println!("Screenshots in {}", opts.out_dir.join("screenshots").display());

    let manifest_json = serde_json::to_string_pretty(&manifest).unwrap_or_default();
    gated_send(opts, &manifest_json, stats, &manifest_json)
}

/// Print the review gate; on confirm, attempt the Telegram send.
fn gated_send(opts: &RecordOptions, preview: &str, stats: &CompressionStats, payload: &str) -> Result<()> {
    let tg = telegram::TelegramConfig::load();
    let dest = match &tg {
        Some(_) => "Telegram → your Sai agent (config found)",
        None => "Telegram → your Sai agent (NO config — local dry-run)",
    };
    if review::confirm_upload(preview, stats, dest) {
        match tg {
            Some(cfg) => match telegram::send(&cfg, payload) {
                Ok(()) => println!("✓ Sent to Telegram."),
                Err(e) => println!("✗ Telegram send failed: {e}"),
            },
            None => println!(
                "Confirmed, but no Telegram config present — dry-run only. \
                 Set SAI_TG_BOT_TOKEN / SAI_TG_CHAT_ID or sai-recorder.config.json to enable."
            ),
        }
    } else {
        println!("Not sent. Output remains local at {}", opts.out_dir.display());
    }
    Ok(())
}