//! Recording-side input filtering.
//!
//! We capture input with a **passive** `rdev::listen` hook (it always calls
//! `CallNextHookEx`, so it NEVER blocks delivery to any window — including our
//! own). This module only decides, per event, whether it should be **recorded**.
//!
//! Operating the recorder must not pollute the trajectory, so we drop any event
//! that targets one of **this process's own windows** (the main window, the float
//! overlay, and the wry/WebView2 helper windows). Delivery is untouched — the
//! click still reaches the button; we simply don't append it to the action stream.
//!
//! Win32-only; on other platforms this is a no-op (TODO: cross-platform).

/// True if `event` targets a window owned by the current process (so it must not
/// be recorded). `cursor` is the last known absolute cursor position (rdev
/// button/wheel events carry no coordinates).
#[cfg(windows)]
pub fn event_targets_self(event: &rdev::Event, cursor: (f64, f64)) -> bool {
    use rdev::EventType::*;
    use windows::Win32::Foundation::{HWND, POINT};
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetAncestor, GetForegroundWindow, GetWindowThreadProcessId, WindowFromPoint, GA_ROOT,
    };

    unsafe {
        let hwnd: HWND = match &event.event_type {
            KeyPress(_) | KeyRelease(_) => GetForegroundWindow(),
            MouseMove { x, y } => GetAncestor(WindowFromPoint(POINT { x: *x as i32, y: *y as i32 }), GA_ROOT),
            ButtonPress(_) | ButtonRelease(_) | Wheel { .. } => {
                GetAncestor(WindowFromPoint(POINT { x: cursor.0 as i32, y: cursor.1 as i32 }), GA_ROOT)
            }
        };
        if hwnd.0.is_null() {
            return false;
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32));
        pid == GetCurrentProcessId()
    }
}

#[cfg(not(windows))]
pub fn event_targets_self(_event: &rdev::Event, _cursor: (f64, f64)) -> bool {
    // TODO(cross-platform): identify our own windows on macOS/Linux.
    false
}
