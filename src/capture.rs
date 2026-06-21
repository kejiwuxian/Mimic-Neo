//! Focused-window / monitor metadata, screenshots, and the ring-buffer frame
//! recorder — ported from the MimicCLI reference (`src/capture.rs`) and kept
//! close to it so the emitted metadata matches `types/actions.d.ts`.
//!
//! Capture is **opt-in**: the ring buffer only runs between [`start_recording`]
//! and [`stop_recording`] (driven by the `record` command), and the rdev
//! listener spawned by [`spawn_listener`] only exists for the duration of a
//! recording session.

use anyhow::Result;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::ops::Sub;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use rdev::Event;
use serde::{Deserialize, Serialize};
use xcap::{
    image::{codecs::webp::WebPEncoder, ColorType, ImageEncoder},
    Monitor, VideoRecorder, Window,
};

// ── Metadata (camelCase to match types/actions.d.ts) ────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusedWindowMetadata {
    pub id: Option<u32>,
    pub pid: Option<u32>,
    pub name: Option<String>,
    pub title: Option<String>,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub z: Option<i32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub minimized: Option<bool>,
    pub maximized: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusedScreenMetadata {
    pub id: Option<u32>,
    pub name: Option<String>,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub primary: Option<bool>,
}

pub struct FocusedContext {
    pub window: Option<Window>,
    pub screen: Monitor,
}

thread_local! {
    static LAST_PRIMARY_SCREEN: RefCell<Option<Monitor>> = const { RefCell::new(None) };
    static LAST_FOCUSED_WINDOW: RefCell<Option<Window>> = const { RefCell::new(None) };
}

pub fn get_primary_screen() -> Monitor {
    LAST_PRIMARY_SCREEN.with(|cache| {
        let mut cached = cache.borrow_mut();

        if let Some(screen) = cached.as_ref() {
            if screen.is_primary().unwrap_or(false) {
                return screen.clone();
            }
        }

        let primary = Monitor::all()
            .ok()
            .and_then(|monitors| monitors.into_iter().find(|m| m.is_primary().unwrap_or(false)))
            .expect("No monitor found");

        *cached = Some(primary.clone());
        primary
    })
}

pub fn get_focused_window() -> Option<Window> {
    LAST_FOCUSED_WINDOW.with(|cache| {
        let mut cached = cache.borrow_mut();

        if let Some(window) = cached.as_ref() {
            if window.is_focused().unwrap_or(false) {
                return Some(window.clone());
            }
        }

        let focused = Window::all()
            .ok()
            .and_then(|windows| windows.into_iter().find(|w| w.is_focused().unwrap_or(false)));

        *cached = focused.clone();
        focused
    })
}

pub fn get_screen_metadata(screen: &Monitor) -> FocusedScreenMetadata {
    FocusedScreenMetadata {
        id: screen.id().ok(),
        name: screen.name().ok(),
        x: screen.x().ok(),
        y: screen.y().ok(),
        width: screen.width().ok(),
        height: screen.height().ok(),
        primary: screen.is_primary().ok(),
    }
}

pub fn get_window_metadata(window: &Window) -> FocusedWindowMetadata {
    FocusedWindowMetadata {
        id: window.id().ok(),
        pid: window.pid().ok(),
        name: window.app_name().ok(),
        title: window.title().ok(),
        x: window.x().ok(),
        y: window.y().ok(),
        z: window.z().ok(),
        width: window.width().ok(),
        height: window.height().ok(),
        minimized: window.is_minimized().ok(),
        maximized: window.is_maximized().ok(),
    }
}

pub fn get_focused_context() -> Result<FocusedContext> {
    let window = get_focused_window();
    let screen = if let Some(ref w) = window {
        w.current_monitor().unwrap_or_else(|_| get_primary_screen())
    } else {
        get_primary_screen()
    };
    Ok(FocusedContext { window, screen })
}

// ── Ring-buffer frame recorder (verbatim approach from MimicCLI) ─────────────

#[derive(Debug, Clone)]
struct BufferedFrame {
    timestamp: Instant,
    data: Vec<u8>, // lossless WebP-encoded (this is the compression baseline)
}

struct RingState {
    capacity: usize,
    frames: VecDeque<BufferedFrame>,
}

static FRAME_RING: LazyLock<Mutex<Option<RingState>>> = LazyLock::new(|| Mutex::new(None));
static RECORDER: LazyLock<Mutex<Option<VideoRecorder>>> = LazyLock::new(|| Mutex::new(None));
static SCHEDULER: LazyLock<Mutex<Option<(timer::Timer, timer::Guard)>>> =
    LazyLock::new(|| Mutex::new(None));
/// Count of frames the xcap ring buffer has actually delivered (for logging).
static RING_FRAMES: AtomicUsize = AtomicUsize::new(0);

/// Encode an RGBA buffer to lossless WebP (the compression baseline). Shared by
/// the xcap ring buffer and the GDI fallback so both feed the same downstream.
fn encode_rgba_webp(raw: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let mut webp = Vec::new();
    match WebPEncoder::new_lossless(&mut webp).write_image(raw, width, height, ColorType::Rgba8.into()) {
        Ok(()) => Some(webp),
        Err(e) => {
            crate::log::line(&format!("capture: webp encode error: {e}"));
            None
        }
    }
}

/// Start the background frame pump on `screen`, keeping `history` seconds of
/// frames at `frequency` fps. Idempotent.
pub fn start_recording(screen: &Monitor, history: Duration, frequency: f32) -> Result<()> {
    {
        let guard = RECORDER.lock().unwrap();
        if guard.is_some() {
            return Ok(());
        }
    }

    let capture_interval = Duration::from_secs_f32(1.0 / frequency);
    let capacity = (history.as_secs_f64() * frequency as f64).ceil() as usize;

    let (recorder, raw_rx) = screen.video_recorder()?;

    *FRAME_RING.lock().unwrap() = Some(RingState {
        capacity,
        frames: VecDeque::with_capacity(capacity),
    });
    RING_FRAMES.store(0, Ordering::Relaxed);

    let timer = timer::Timer::new();
    let guard = timer.schedule_repeating(
        chrono::Duration::from_std(capture_interval).unwrap(),
        move || {
            // Don't block the timer thread forever if xcap never delivers frames
            // (common on VM/virtual displays — the GDI fallback covers that case).
            let frame = match raw_rx.recv_timeout(Duration::from_millis(500)) {
                Ok(f) => f,
                Err(_) => return,
            };

            let timestamp = Instant::now();
            let width = frame.width;
            let height = frame.height;
            let raw = frame.raw;

            std::thread::spawn(move || {
                let Some(webp) = encode_rgba_webp(&raw, width, height) else {
                    return;
                };
                let n = RING_FRAMES.fetch_add(1, Ordering::Relaxed) + 1;
                if n == 1 || n % 10 == 0 {
                    crate::log::line(&format!(
                        "ring-buffer: captured frame {width}x{height}, {} bytes (#{n})",
                        webp.len()
                    ));
                }

                if let Some(ref mut state) = *FRAME_RING.lock().unwrap() {
                    while state.frames.len() >= state.capacity {
                        state.frames.pop_front();
                    }
                    let idx = state
                        .frames
                        .iter()
                        .rposition(|f| f.timestamp < timestamp)
                        .map(|i| i + 1)
                        .unwrap_or(0);
                    state
                        .frames
                        .insert(idx, BufferedFrame { timestamp, data: webp });
                }
            });
        },
    );

    *SCHEDULER.lock().unwrap() = Some((timer, guard));
    recorder.start()?;
    *RECORDER.lock().unwrap() = Some(recorder);

    Ok(())
}

pub fn stop_recording() -> Result<()> {
    SCHEDULER.lock().unwrap().take();
    if let Some(recorder) = RECORDER.lock().unwrap().take() {
        recorder.stop()?;
    }
    FRAME_RING.lock().unwrap().take();
    Ok(())
}

/// Frame closest to `delta` ago (lossless WebP bytes), e.g. the "before" frame.
/// Falls back to an on-demand GDI capture when the ring buffer is empty (xcap's
/// WGC/DXGI path fails or yields no frames on VM/virtual displays).
pub fn retrieve_frame(delta: Duration) -> Option<Vec<u8>> {
    {
        let ring_opt = FRAME_RING.lock().unwrap();
        if let Some(state) = ring_opt.as_ref() {
            if !state.frames.is_empty() {
                let target = Instant::now().checked_sub(delta).unwrap_or_else(Instant::now);
                return state
                    .frames
                    .iter()
                    .rfind(|f| f.timestamp <= target)
                    .or_else(|| state.frames.front())
                    .map(|f| f.data.clone());
            }
        }
    } // release the lock before the (slower) GDI grab
    gdi_capture_webp()
}

/// Frame nearest a specific instant; GDI fallback when the ring is empty.
pub fn closest_frame(time: Instant) -> Option<Vec<u8>> {
    {
        let ring_opt = FRAME_RING.lock().unwrap();
        if let Some(state) = ring_opt.as_ref() {
            if !state.frames.is_empty() {
                return state
                    .frames
                    .iter()
                    .min_by_key(|f| (time.sub(f.timestamp)).as_nanos())
                    .map(|f| f.data.clone());
            }
        }
    }
    gdi_capture_webp()
}

// ── GDI BitBlt fallback ──────────────────────────────────────────────────────
// xcap uses Windows.Graphics.Capture / DXGI Desktop Duplication, which commonly
// fails (E_INVALIDARG / 0x80070057) or returns no frames on VMs and virtual
// displays. GDI BitBlt works there, so we grab the primary screen on demand and
// encode it to the same lossless WebP the ring buffer would have produced.

fn log_gdi_once(width: u32, height: u32, bytes: usize) {
    static LOGGED: AtomicBool = AtomicBool::new(false);
    if !LOGGED.swap(true, Ordering::Relaxed) {
        crate::log::line(&format!(
            "gdi: capturing primary screen {width}x{height} -> {bytes} bytes (xcap fallback)"
        ));
    }
}

#[cfg(windows)]
fn gdi_capture_webp() -> Option<Vec<u8>> {
    use std::mem::size_of;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
        GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
        SRCCOPY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

    unsafe {
        let w = GetSystemMetrics(SM_CXSCREEN);
        let h = GetSystemMetrics(SM_CYSCREEN);
        if w <= 0 || h <= 0 {
            crate::log::line("gdi: bad screen metrics");
            return None;
        }

        let screen = GetDC(HWND::default());
        if screen.0.is_null() {
            crate::log::line("gdi: GetDC failed");
            return None;
        }
        let mem = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bmp);
        let blt = BitBlt(mem, 0, 0, w, h, screen, 0, 0, SRCCOPY);

        let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = w;
        bmi.bmiHeader.biHeight = -h; // negative => top-down rows
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB.0;
        let lines = GetDIBits(
            mem,
            bmp,
            0,
            h as u32,
            Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
            &mut bmi,
            DIB_RGB_COLORS,
        );

        // Cleanup GDI objects.
        SelectObject(mem, old);
        let _ = DeleteObject(bmp);
        let _ = DeleteDC(mem);
        ReleaseDC(HWND::default(), screen);

        if blt.is_err() || lines == 0 {
            crate::log::line(&format!(
                "gdi: capture failed (blt_ok={}, lines={lines})",
                blt.is_ok()
            ));
            return None;
        }

        // GDI returns BGRA (32bpp); convert to RGBA and force opaque alpha.
        for px in buf.chunks_exact_mut(4) {
            px.swap(0, 2);
            px[3] = 255;
        }

        let bytes = encode_rgba_webp(&buf, w as u32, h as u32)?;
        log_gdi_once(w as u32, h as u32, bytes.len());
        Some(bytes)
    }
}

#[cfg(not(windows))]
fn gdi_capture_webp() -> Option<Vec<u8>> {
    None
}

// ── Opt-in input listener ───────────────────────────────────────────────────

/// Spawn the global rdev listener for an opt-in session. Returns a receiver of
/// raw events. `rdev::listen` blocks and cannot be torn down, so the listener
/// thread is detached; the recorder stops consuming when the session ends.
pub fn spawn_listener() -> Receiver<Event> {
    let (tx, rx) = channel::<Event>();
    std::thread::spawn(move || {
        if let Err(err) = rdev::listen(move |event| {
            let _ = tx.send(event);
        }) {
            eprintln!("[capture] input listener failed: {err:?}");
        }
    });
    rx
}
