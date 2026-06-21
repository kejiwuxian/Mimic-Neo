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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rdev::{Button, Event, EventType, Key};

use crate::actions::{
    distance, held_keys, scroll_amount, scroll_direction, BaseAction, Capture, Coordinate, Focused,
    UserAction,
};
use crate::capture;
use crate::compress::{CompressionOptions, CompressionStats};
use crate::overlay;

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
            None => {
                crate::log::line("shot: MISSING (no frame from ring buffer or GDI)");
                String::new()
            }
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

    fn action_count(&self) -> usize {
        self.actions.len()
    }
}

// ── Recording driver (worker thread) ─────────────────────────────────────────

/// Output of a recording session, handed back when the worker thread joins.
pub struct RecorderOutput {
    pub actions: Vec<UserAction>,
    pub stats: CompressionStats,
    pub started_ms: u64,
    pub ended_ms: u64,
    pub duration_ms: u64,
}

/// Callback invoked when the global stop hotkey (Ctrl+Alt+S) is detected.
pub type StopHotkey = Arc<dyn Fn() + Send + Sync + 'static>;

/// Start the capture pipeline on a background thread. Returns a join handle that
/// yields the recorded actions + compression stats once `stop` is set.
///
/// `on_hotkey` is called from the capture thread when Ctrl+Alt+S is pressed, so a
/// recording can always be ended even if the UI is unresponsive.
pub fn start_worker(
    opts: RecordOptions,
    stop: Arc<AtomicBool>,
    on_hotkey: StopHotkey,
) -> JoinHandle<RecorderOutput> {
    std::thread::spawn(move || run_worker(opts, stop, on_hotkey))
}

fn run_worker(opts: RecordOptions, stop: Arc<AtomicBool>, on_hotkey: StopHotkey) -> RecorderOutput {
    let started_ms = epoch_ms();
    let recording_start = SystemTime::now();
    crate::log::line(&format!(
        "worker: started (fps={}, history={}s, mode={:?})",
        opts.fps, opts.history_secs, opts.mode
    ));

    // Ring-buffer frame pump (best-effort) + global input listener.
    let primary = capture::get_primary_screen();
    match capture::start_recording(
        &primary,
        Duration::from_secs(opts.history_secs.max(1)),
        opts.fps.max(1.0),
    ) {
        Ok(()) => crate::log::line("worker: ring-buffer recorder started"),
        Err(e) => crate::log::line(&format!("worker: ring-buffer start error: {e}")),
    }
    let event_rx = capture::spawn_listener();
    crate::log::line("worker: input listener spawned; capturing");

    let mut state = RecorderState::new(opts.compression.clone(), recording_start);
    // Last known cursor (rdev button/wheel events carry no coords) for hit-testing.
    let mut cursor = (0.0f64, 0.0f64);
    let mut last_logged = 0usize;
    // Modifier tracking for the global stop hotkey (Ctrl+Alt+S).
    let mut ctrl = false;
    let mut alt = false;

    loop {
        match event_rx.recv_timeout(DEBOUNCE_DELAY) {
            Ok(event) => {
                match &event.event_type {
                    EventType::KeyPress(k) => {
                        if matches!(k, Key::ControlLeft | Key::ControlRight) {
                            ctrl = true;
                        }
                        if matches!(k, Key::Alt | Key::AltGr) {
                            alt = true;
                        }
                        if ctrl && alt && matches!(k, Key::KeyS) {
                            crate::log::line("worker: hotkey Ctrl+Alt+S -> stop");
                            (on_hotkey)();
                            stop.store(true, Ordering::SeqCst);
                            break;
                        }
                    }
                    EventType::KeyRelease(k) => {
                        if matches!(k, Key::ControlLeft | Key::ControlRight) {
                            ctrl = false;
                        }
                        if matches!(k, Key::Alt | Key::AltGr) {
                            alt = false;
                        }
                    }
                    EventType::MouseMove { x, y } => {
                        cursor = (*x, *y);
                    }
                    _ => {}
                }
                // Never record input aimed at our own windows (overlay/main/helpers).
                if overlay::event_targets_self(&event, cursor) {
                    continue;
                }
                state.handle(event);
                let n = state.action_count();
                if n != last_logged {
                    crate::log::line(&format!("worker: captured action #{n}"));
                    last_logged = n;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                let now = SystemTime::now();
                state.flush_typing(now, None);
                state.flush_scroll(now, None);
                let n = state.action_count();
                if n != last_logged {
                    crate::log::line(&format!("worker: captured action #{n}"));
                    last_logged = n;
                }
                if stop.load(Ordering::SeqCst) {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    state.finish();
    capture::stop_recording().ok();

    let ended_ms = epoch_ms();
    let RecorderState { actions, stats, .. } = state;
    crate::log::line(&format!(
        "worker: finishing, {} action(s), {} captures",
        actions.len(),
        stats.shots
    ));
    RecorderOutput {
        actions,
        stats,
        started_ms,
        ended_ms,
        duration_ms: ended_ms.saturating_sub(started_ms),
    }
}
