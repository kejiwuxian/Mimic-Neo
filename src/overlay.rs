//! Floating recording-control overlay: a small, always-on-top, borderless,
//! draggable window with a red REC indicator + elapsed timer + Stop button.
//!
//! It runs on the **main thread** (winit/eframe requirement); the capture
//! pipeline runs on a worker thread and coordinates via [`Shared`]:
//!   * `stop` — set by the Stop button / window close; polled by the pipeline.
//!   * `hwnd` — the overlay's top-level Win32 `HWND` (as `isize`, `0` until
//!     ready), used to (a) exclude the overlay from screen captures and
//!     (b) drop input events that target the overlay.
//!
//! Cross-platform TODO: the Win32 bits (capture exclusion + hit-testing) are
//! gated behind `#[cfg(windows)]`; macOS/Linux equivalents are not implemented.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;

/// Shared coordination state between the overlay (main thread) and the capture
/// pipeline (worker thread).
#[derive(Clone)]
pub struct Shared {
    /// Set true to end the recording session.
    pub stop: Arc<AtomicBool>,
    /// Overlay top-level HWND as isize; `0` until the window exists.
    pub hwnd: Arc<AtomicIsize>,
}

impl Shared {
    pub fn new() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            hwnd: Arc::new(AtomicIsize::new(0)),
        }
    }

    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    pub fn stop_requested(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    pub fn overlay_hwnd(&self) -> isize {
        self.hwnd.load(Ordering::SeqCst)
    }
}

impl Default for Shared {
    fn default() -> Self {
        Self::new()
    }
}

/// Run the overlay window. Blocks until the window closes (Stop or close).
///
/// Returns `true` if the overlay actually ran (in which case `stop` is set on
/// exit), or `false` if it failed to launch — letting the caller fall back to a
/// console Enter-to-stop without a competing stdin reader.
pub fn run_overlay(shared: Shared) -> bool {
    let start = Instant::now();
    let viewport = egui::ViewportBuilder::default()
        .with_title("sai-recorder")
        .with_decorations(false)
        .with_always_on_top()
        .with_resizable(false)
        .with_inner_size([196.0, 56.0]);

    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let app_shared = shared.clone();
    match eframe::run_native(
        "sai-recorder-overlay",
        native_options,
        Box::new(move |_cc| Ok(Box::new(OverlayApp::new(app_shared, start)))),
    ) {
        Ok(()) => {
            // Window closed (Stop/close) — ensure the pipeline stops.
            shared.request_stop();
            true
        }
        Err(e) => {
            eprintln!("[overlay] failed to launch ({e}); falling back to Enter-to-stop.");
            false
        }
    }
}

struct OverlayApp {
    shared: Shared,
    start: Instant,
    hwnd_ready: bool,
}

impl OverlayApp {
    fn new(shared: Shared, start: Instant) -> Self {
        Self {
            shared,
            start,
            hwnd_ready: false,
        }
    }
}

impl eframe::App for OverlayApp {
    // eframe 0.34 made `ui` the required trait method. We override `update`
    // below and render the overlay manually, so this stub is never called —
    // it exists only to satisfy the trait bound.
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Once the native window exists, grab its HWND, publish it, and exclude
        // it from screen capture.
        if !self.hwnd_ready {
            if let Some(hwnd) = native_hwnd(frame) {
                self.shared.hwnd.store(hwnd, Ordering::SeqCst);
                exclude_from_capture(hwnd);
                self.hwnd_ready = true;
            }
        }

        // Window close (e.g. Alt+F4) also stops recording.
        if ctx.input(|i| i.viewport().close_requested()) {
            self.shared.request_stop();
        }

        let elapsed = self.start.elapsed();
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                // Drag handle: red dot + timer. Dragging it moves the window.
                let label = egui::Label::new(
                    egui::RichText::new(format!("● REC  {}", fmt_elapsed(elapsed)))
                        .color(egui::Color32::from_rgb(240, 64, 64))
                        .strong(),
                )
                .sense(egui::Sense::drag());
                let handle = ui.add(label);
                if handle.drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }

                ui.add_space(8.0);

                if ui.button("Stop").clicked() {
                    self.shared.request_stop();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });

        // Keep the timer ticking.
        ctx.request_repaint_after(Duration::from_millis(250));
    }

    fn on_exit(&mut self) {
        self.shared.request_stop();
    }
}

fn fmt_elapsed(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}", s / 60, s % 60)
}

// ── Native window handle ─────────────────────────────────────────────────────

/// The overlay's top-level window handle as `isize` (HWND on Windows).
fn native_hwnd(frame: &eframe::Frame) -> Option<isize> {
    #[cfg(windows)]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let handle = frame.window_handle().ok()?;
        match handle.as_raw() {
            RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
            _ => None,
        }
    }
    #[cfg(not(windows))]
    {
        let _ = frame;
        None
    }
}

// ── Win32: capture exclusion ─────────────────────────────────────────────────

/// Make the overlay invisible to screen capture (DXGI dup / WGC / BitBlt on
/// Win10 2004+) via `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`.
#[cfg(windows)]
pub fn exclude_from_capture(hwnd_isize: isize) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE};
    if hwnd_isize == 0 {
        return;
    }
    unsafe {
        let hwnd = HWND(hwnd_isize as *mut core::ffi::c_void);
        // Ignore failure: capture still works, the overlay just isn't hidden.
        // Fallback (not implemented): blank the overlay's rect in compress.rs.
        let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
    }
}

#[cfg(not(windows))]
pub fn exclude_from_capture(_hwnd_isize: isize) {
    // TODO(cross-platform): macOS NSWindow.sharingType = .none; Linux unsupported.
}

// ── Win32: drop input that targets the overlay ───────────────────────────────

/// True if `event` is aimed at the overlay (so it must not become a recorded
/// action): mouse events whose point resolves to the overlay's root window, or
/// key events while the overlay is foreground. `cursor` is the last known
/// absolute cursor position (rdev button/wheel events carry no coordinates).
#[cfg(windows)]
pub fn event_targets_overlay(event: &rdev::Event, overlay_hwnd: isize, cursor: (f64, f64)) -> bool {
    use rdev::EventType::*;
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    if overlay_hwnd == 0 {
        return false;
    }

    unsafe {
        match &event.event_type {
            KeyPress(_) | KeyRelease(_) => GetForegroundWindow().0 as isize == overlay_hwnd,
            MouseMove { x, y } => point_in_overlay(*x, *y, overlay_hwnd),
            ButtonPress(_) | ButtonRelease(_) | Wheel { .. } => {
                point_in_overlay(cursor.0, cursor.1, overlay_hwnd)
            }
        }
    }
}

/// True if the screen point `(x, y)` resolves to the overlay's root window.
#[cfg(windows)]
unsafe fn point_in_overlay(x: f64, y: f64, overlay_hwnd: isize) -> bool {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, WindowFromPoint, GA_ROOT};
    let w = WindowFromPoint(POINT { x: x as i32, y: y as i32 });
    let root = GetAncestor(w, GA_ROOT);
    root.0 as isize == overlay_hwnd
}

#[cfg(not(windows))]
pub fn event_targets_overlay(_event: &rdev::Event, _overlay_hwnd: isize, _cursor: (f64, f64)) -> bool {
    false
}