//! Input filtering for the recording-control overlay.
//!
//! With Tauri owning the windows, the only thing left here is dropping rdev
//! events that target the floating control window, so clicking **Stop** or
//! dragging the overlay is never recorded as a user action. Capture-exclusion
//! (hiding the overlay from screenshots) is handled by Tauri's
//! `content_protected(true)` on the float window, so the old manual
//! `SetWindowDisplayAffinity` call is gone.
//!
//! All Win32 here is `#[cfg(windows)]`; on other platforms it's a no-op
//! (cross-platform hit-testing is a TODO).

/// True if `event` is aimed at the overlay window (identified by `overlay_hwnd`,
/// its top-level HWND as `isize`; `0` = not ready). `cursor` is the last known
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
    // TODO(cross-platform): hit-test the overlay on macOS/Linux.
    false
}
