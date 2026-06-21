# sai-recorder

**Opt-in record-and-replay workflow capture with token-efficient compression** —
built for the **CalHacks AI Hackathon 2026**.

During an explicit, opt-in recording session it captures input + screenshots,
coalesces them into high-level **actions**, attaches focused-window/monitor
metadata, **compresses** the screenshots, and exports a compact payload — either
for the user's own **Simular Sai** agent (over Telegram) or as a standardized
**computer-use dataset** trajectory.

The emitted JSON conforms to the **MimicCLI** reference schema
(`types/actions.d.ts`); sai-recorder layers two things on top: **dual export
modes** and a **compression layer** (the "Token Company" angle), plus an
**on-screen recording-control overlay**.

> Tracks: **Ddoski's Toolbox** (polished utility) · **The Token Company**
> (context compression) · **Simular Sai** integration.

---

## Use cases

- **(a) Personal workflow recording → Sai.** Perform a task once; sai-recorder
  turns it into a compact, replayable description for your own Sai agent.
- **(b) Computer-use dataset collection.** The same pipeline emits a
  standardized trajectory of `(before → action → after)` steps for
  training/evaluating computer-use agents. At dataset scale screenshots dominate
  storage, so the compression layer reports per-capture and aggregate **disk-byte**
  savings, not just token estimates.

---

## Quick start

```bash
cargo build --release
cargo run -- --help
cargo run -- record --help

# Workflow capture for Sai (inline-base64 JSON, lossless WebP like MimicCLI)
cargo run -- record --mode sai --out ./recording

# Computer-use dataset (JSONL + manifest + externalized screenshots/)
cargo run -- record --mode dataset --out ./trajectory-001

# Token Company compression knobs (apply to either mode)
cargo run -- record --mode dataset --lossy --quality 70 --max-dim 1024 --crop
```

A floating **recording control** appears on screen: a red REC dot, an elapsed
timer, and a **Stop** button. Click **Stop** (or close the overlay) to end the
session; **Enter** in the console is a fallback if the overlay can't launch.
Nothing is uploaded until you confirm at the local review prompt.

> **Permissions:** global input + screen capture need OS permission (macOS:
> Accessibility + Screen Recording). The overlay's capture-exclusion is
> Windows-only (see below).

### CLI

```
sai-recorder record [OPTIONS]

  --mode <sai|dataset>   Export format                        [default: sai]
  -o, --out <DIR>        Output directory                     [default: ./recording]
  --lossy                Encode captures as lossy JPEG (vs lossless WebP)
  --quality <1-100>      JPEG quality with --lossy            [default: 80]
  --max-dim <PX>         Downscale longest side to <= PX
  --crop                 Crop captures to the focused window
  --fps <FPS>            Ring-buffer frame rate               [default: 10]
  --history <SECS>       Ring-buffer history length           [default: 10]
  -h, --help
```

---

## Architecture

```
            main thread                         worker thread
   ┌───────────────────────────┐      ┌──────────────────────────────────┐
   │  overlay.rs (egui)        │      │  capture.rs                       │
   │  ● REC  00:12   [Stop]    │      │   • ring-buffer VideoRecorder     │
   │                           │      │     (lossless WebP frames)        │
   │  Stop → stop_requested ───┼──┐   │   • rdev listener → events        │
   │  HWND → overlay_hwnd ─────┼─┐│   │                                   │
   └───────────────────────────┘ ││   │  recorder.rs (state machine)      │
        Shared { stop, hwnd }    ││   │   • drop events targeting overlay │
                                 │└──▶│   • coalesce → UserAction         │
                                 │    │   • before/after = closest frame  │
                                 └───▶│   • compress.rs each capture      │
                                      └─────────────────┬────────────────┘
                                                        ▼
                              export.rs (sai | dataset) → review.rs (y/N) → telegram.rs
```

| Module      | Responsibility |
|-------------|----------------|
| `capture`   | Focused window/monitor metadata + ring-buffer `VideoRecorder` (lossless WebP frames) + opt-in rdev listener. **Ported from MimicCLI `src/capture.rs`.** |
| `actions`   | Canonical schema (`types/actions.d.ts`): `Coordinate`, `Direction`, `Focused`, `Capture`, `BaseAction`, `UserAction` — native rdev `Key`/`Button`. |
| `overlay`   | Floating egui Stop control; Win32 capture-exclusion + overlay-targeted input filtering. |
| `recorder`  | State machine (ported from MimicCLI `src/new_record.rs`) + worker/overlay orchestration. |
| `compress`  | Crop / downscale / re-encode (lossless WebP or lossy JPEG); baseline-vs-compressed token & byte report. |
| `export`    | `sai` inline-base64 JSON, or `dataset` JSONL + `manifest.json` + `screenshots/`. |
| `review`    | Local preview + `y/N` gate before anything is sent. |
| `telegram`  | Sends the approved payload to the user's Sai agent (gated behind review). |

---

## Data model (matches `types/actions.d.ts`)

Every action carries a flattened base — `timestamp` (ms from session start),
`duration` (ms), and **always-present** `before`/`after` captures:

```jsonc
{
  "type": "Click",
  "timestamp": 4120.5,
  "duration": 98.0,
  "before": { "capture": "data:image/webp;base64,…", "focused": { "window": {…}, "screen": {…} } },
  "after":  { "capture": "data:image/webp;base64,…", "focused": { "window": {…}, "screen": {…} } },
  "button": "Left",
  "coordinate": { "x": 712.0, "y": 344.0 },
  "keys": ["ControlLeft"]
}
```

* **`Button` / `Key`** use rdev's **native** serde names (rdev `serialize`
  feature): `"Left"`, `"KeyA"`, `"ControlLeft"`, and `{ "Unknown": 5 }` for
  unknown codes — no canonicalization.
* **`focused`** is `{ window, screen }` with camelCase metadata
  (`FocusedWindowMetadata` / `FocusedScreenMetadata`).
* Action set: `Hover, Click, DoubleClick, TripleClick, Drag (start_coordinate +
  coordinate), Scroll (direction + amount), Type (text), Press (keys[])`.
  Mouse/Type/Scroll carry an optional `keys` modifier array.

---

## On-screen recording control (overlay)

`overlay.rs` shows a small, always-on-top, borderless, **draggable** window with
a red REC indicator, an elapsed timer, and a **Stop** button (egui/eframe). It
runs on the **main thread**; the capture pipeline runs on a **worker thread**.
They coordinate through `Shared { stop: Arc<AtomicBool>, hwnd: Arc<AtomicIsize> }`:
Stop (or closing the window) sets `stop`, which the pipeline polls; the overlay
publishes its `HWND` into `hwnd` once the window exists.

**Excluded from all screen captures.** Once the window exists, its Win32 `HWND`
is obtained via raw-window-handle (`Frame::window_handle()` → `RawWindowHandle::Win32`)
and `SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)` (= `0x11`) is called
through the `windows` crate. This hides the overlay from xcap's frames across
DXGI desktop-duplication, Windows.Graphics.Capture, and BitBlt (Win10 2004+), so
the control never appears in recorded screenshots.
*Documented fallback (not implemented):* if a backend ever ignores the affinity
flag, blank the overlay's rect in the captured frame during compression.

**Input targeting the overlay is dropped.** Before an rdev event becomes an
action, the worker discards it if it targets the overlay:
* mouse events — `WindowFromPoint(cursor)` → `GetAncestor(.., GA_ROOT)` equals
  the overlay HWND (rdev button/wheel events carry no coordinates, so the last
  `MouseMove` position is used);
* key events — `GetForegroundWindow()` equals the overlay HWND.

This reliably drops the Stop click, dragging the overlay, and any keystrokes
typed while it's focused, so controlling the recorder never pollutes the data.

**Cross-platform:** all Win32 bits are behind `#[cfg(windows)]`; on other
platforms capture-exclusion and input-filtering are no-ops (TODO: macOS
`NSWindow.sharingType = .none`; Linux unsupported).

---

## Compression (The Token Company)

The ring buffer stores **lossless full-frame WebP** — the MimicCLI baseline.
With no flags, captures are emitted exactly that way. The flags add, per capture:

1. **Focus crop** (`--crop`) — crop to the focused-window region.
2. **Downscale** (`--max-dim`) — longest side capped, aspect preserved.
3. **Re-encode** — smaller lossless WebP, or **lossy JPEG** (`--lossy --quality`).

Structural compression is inherent in the schema: the state machine coalesces
many raw rdev events into a few semantic actions (a 200-keystroke form fill → a
few `Type` actions).

### Cost report (shown at review)

```
── Compression report ───────────────────────────────
captures   : 54
screenshots: 22.10 MB (lossless baseline)  →  1.83 MB   12.05× smaller
tokens(img): 30912 (baseline)  →  2640 (compressed incl. JSON)   11.71× smaller
per capture: 35k compressed bytes avg
─────────────────────────────────────────────────────
```

Token estimates are rough (~4 chars/token for text; base64 expansion for
images). In `dataset` mode the same numbers are written to `manifest.json`.

---

## Output formats

### `--mode sai` → `actions.json`
A single JSON array of `UserAction`s with **inline** `data:image/...;base64,…`
captures (the MimicCLI inline-payload convention).

### `--mode dataset`
```
trajectory-001/
├── manifest.json     # os, session start/end, total steps, compression stats, self-describing schema
├── trajectory.jsonl  # one UserAction per line; captures are relative paths
└── screenshots/
    ├── shot_00000.webp   # or .jpg with --lossy
    └── …
```
Each capture is externalized to `screenshots/` and the `capture` field becomes a
relative path (e.g. `"screenshots/shot_00012.webp"`).

---

## Privacy

Recording is **opt-in** (only inside a `record` session, with a visible REC
overlay) and **nothing is uploaded without a local `y/N` review** that shows the
payload preview + compression report. The Telegram sender runs only after you
confirm.

> Per the current spec there is **no redaction/secret-masking module** — do not
> record credential entry. Configure Telegram via `SAI_TG_BOT_TOKEN` /
> `SAI_TG_CHAT_ID` or `sai-recorder.config.json` (see
> `sai-recorder.config.example.json`); with no config, a confirmed session is a
> local dry-run.

---

## Reconciliation notes (vs the MimicCLI reference)

- **Schema source of truth:** `types/actions.d.ts`. The reference `new_record.rs`
  used field names `before_screenshot`/`after_screenshot` in **seconds** and
  stringified keys/buttons via `{:?}`; per the user's decisions this project uses
  `before`/`after` in **milliseconds** and rdev's **native** serde names. The
  reference also modeled `Capture.focused` as window-only; here it is
  `{ window, screen }` as specified.
- **Sample `test/actions.json` is stale:** it is the *old* `record.rs` wrapper
  format (`{actions, focused, capture}` with `start`/`end` items and JPEG
  captures) and matches neither `actions.d.ts` nor `new_record.rs`. It was used
  only to confirm rdev's serialize output, not as the schema.
- **State machine:** ported from `new_record.rs` but restructured from coexisting
  mutable closures into a single `RecorderState` struct (cleaner borrows, same
  behavior). One small optimization: a transition frame is only captured when a
  flush/new-scroll actually needs it (avoids counting discarded frames).
- **"Lossy WebP":** the `image` crate's WebP encoder is **lossless-only**, so
  lossy compression is implemented as **JPEG** (`data:image/jpeg`, quality knob).
  True lossy WebP would require adding a libwebp binding (`webp` crate); flagged,
  not added.
- **Could not compile here:** no Rust toolchain in this environment. Code was
  written against verified crate docs — xcap 0.9.6 (`features=["image"]`,
  `VideoRecorder`, `is_focused`/`z`/`current_monitor`), rdev 0.5.3
  (`serialize`), eframe 0.34 (`Frame: HasWindowHandle`), raw-window-handle 0.6
  (`Win32WindowHandle.hwnd: NonZeroIsize`), and the `windows` 0.58 Win32 APIs
  (`SetWindowDisplayAffinity`, `WindowFromPoint`, `GetAncestor`,
  `GetForegroundWindow`; `HWND(*mut c_void)`). Run `cargo build` to confirm.

## License

MIT
