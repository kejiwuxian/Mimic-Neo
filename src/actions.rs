//! Canonical action schema — mirrors `types/actions.d.ts` from the MimicCLI
//! reference. Emitted JSON conforms to that schema exactly:
//!
//! * `Button` / `Key` use rdev's **native** serde names (rdev `serialize`
//!   feature): unit variants serialize as strings (`"Left"`, `"KeyA"`,
//!   `"ControlLeft"`), and `Unknown(n)` as `{ "Unknown": n }`. We do NOT
//!   canonicalize them.
//! * Every action carries a flattened [`BaseAction`] (`timestamp`, `duration`,
//!   `before`, `after`) — `before`/`after` are ALWAYS present.
//! * The enum is internally tagged with `type` (`"Click"`, `"Type"`, …).

use rdev::{Button, Key};
use serde::Serialize;

use crate::capture::{FocusedScreenMetadata, FocusedWindowMetadata};

/// Absolute screen coordinate.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Coordinate {
    pub x: f64,
    pub y: f64,
}

/// Scroll direction (serializes as `"Up"`/`"Down"`/`"Left"`/`"Right"`).
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

/// `focused: { window, screen }` — the focus context at capture time.
#[derive(Debug, Clone, Serialize)]
pub struct Focused {
    pub window: Option<FocusedWindowMetadata>,
    pub screen: FocusedScreenMetadata,
}

/// `Capture { capture, focused }`. `capture` is a `data:image/...;base64,...`
/// URL in `sai` mode, or a relative `screenshots/...` path in `dataset` mode.
#[derive(Debug, Clone, Serialize)]
pub struct Capture {
    pub capture: String,
    pub focused: Focused,
}

/// Fields shared by every action. `timestamp` and `duration` are milliseconds.
#[derive(Debug, Clone, Serialize)]
pub struct BaseAction {
    /// Milliseconds from session start.
    pub timestamp: f64,
    /// Milliseconds the action took.
    pub duration: f64,
    pub before: Capture,
    pub after: Capture,
}

/// A high-level user action. Internally tagged by `type`, with [`BaseAction`]
/// flattened in. Matches `UserAction` in `types/actions.d.ts`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum UserAction {
    Hover {
        #[serde(flatten)]
        base: BaseAction,
        coordinate: Coordinate,
        #[serde(skip_serializing_if = "Option::is_none")]
        keys: Option<Vec<Key>>,
    },
    Click {
        #[serde(flatten)]
        base: BaseAction,
        button: Button,
        coordinate: Coordinate,
        #[serde(skip_serializing_if = "Option::is_none")]
        keys: Option<Vec<Key>>,
    },
    DoubleClick {
        #[serde(flatten)]
        base: BaseAction,
        button: Button,
        coordinate: Coordinate,
        #[serde(skip_serializing_if = "Option::is_none")]
        keys: Option<Vec<Key>>,
    },
    TripleClick {
        #[serde(flatten)]
        base: BaseAction,
        button: Button,
        coordinate: Coordinate,
        #[serde(skip_serializing_if = "Option::is_none")]
        keys: Option<Vec<Key>>,
    },
    Drag {
        #[serde(flatten)]
        base: BaseAction,
        start_coordinate: Coordinate,
        coordinate: Coordinate,
        #[serde(skip_serializing_if = "Option::is_none")]
        keys: Option<Vec<Key>>,
    },
    Scroll {
        #[serde(flatten)]
        base: BaseAction,
        coordinate: Coordinate,
        direction: Direction,
        amount: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        keys: Option<Vec<Key>>,
    },
    Type {
        #[serde(flatten)]
        base: BaseAction,
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        keys: Option<Vec<Key>>,
    },
    Press {
        #[serde(flatten)]
        base: BaseAction,
        keys: Vec<Key>,
    },
}

impl UserAction {
    /// Mutable access to this action's base (used to backfill `after` captures).
    pub fn base_mut(&mut self) -> &mut BaseAction {
        match self {
            UserAction::Hover { base, .. }
            | UserAction::Click { base, .. }
            | UserAction::DoubleClick { base, .. }
            | UserAction::TripleClick { base, .. }
            | UserAction::Drag { base, .. }
            | UserAction::Scroll { base, .. }
            | UserAction::Type { base, .. }
            | UserAction::Press { base, .. } => base,
        }
    }
}

// ── Coalescing helpers (shared by the recorder state machine) ───────────────

/// Modifier keys tracked for the optional `keys` array on actions.
pub const MODIFIER_KEYS: &[Key] = &[
    Key::ShiftLeft,
    Key::ShiftRight,
    Key::ControlLeft,
    Key::ControlRight,
    Key::Alt,
    Key::AltGr,
    Key::MetaLeft,
    Key::MetaRight,
    Key::CapsLock,
];

/// Modifier keys currently held, as native rdev keys (or `None` if empty).
pub fn held_keys(pressed: &std::collections::HashSet<Key>) -> Option<Vec<Key>> {
    let mut held: Vec<Key> = pressed
        .iter()
        .copied()
        .filter(|k| MODIFIER_KEYS.contains(k))
        .collect();
    if held.is_empty() {
        None
    } else {
        // Stable order for deterministic output.
        held.sort_by_key(|k| format!("{k:?}"));
        Some(held)
    }
}

/// Dominant scroll direction from accumulated deltas.
pub fn scroll_direction(delta_x: i64, delta_y: i64) -> Direction {
    if delta_y.abs() > delta_x.abs() {
        if delta_y > 0 {
            Direction::Up
        } else {
            Direction::Down
        }
    } else if delta_x > 0 {
        Direction::Right
    } else {
        Direction::Left
    }
}

/// Scroll magnitude (max of the two axes).
pub fn scroll_amount(delta_x: i64, delta_y: i64) -> i64 {
    delta_x.abs().max(delta_y.abs())
}

/// Euclidean distance between two points.
pub fn distance(x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    ((x1 - x2).powi(2) + (y1 - y2).powi(2)).sqrt()
}
