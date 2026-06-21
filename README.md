# sai-recorder

**Opt-in record-and-replay workflow capture with token-efficient compression** —
a **Tauri v2** desktop app (Windows, GNU toolchain).

Record a workflow once: sai-recorder captures input + screenshots, coalesces them
into high-level **actions** (MimicCLI `types/actions.d.ts` schema), compresses the
screenshots, stores each run as a **task**, reports **token + disk savings**, and
can **replay** a task or send it to your **Simular Sai** agent over Telegram.

> Built for the CalHacks AI Hackathon 2026 — Ddoski's Toolbox · The Token Company
> (compression) · Simular Sai.

---

## UI

* **Main window** (`ui/index.html`): a *New recording* panel (mode Sai/Dataset,
  fps, history, max-dim, crop-to-focus, lossy/quality) and a **task library**
  (Run / View / Rename / Delete / Send) showing each recording's token- and
  disk-savings headline. Refreshes itself on `recording-finished`.
* **Float overlay** (`ui/float.html`): a 160×48 borderless, always-on-top,
  draggable control with a pulsing red REC dot, an `MM:SS` timer, and **Stop**.
  Created at runtime by the `open_float_window` command; excluded from screen
  capture via Tauri's `content_protected(true)`.

The frontend is **static** (no Node/bundler). `app.withGlobalTauri = true`, so it
calls `window.__TAURI__.core.invoke(...)` and `window.__TAURI__.event.listen(...)`
directly.

---

## Architecture

```
            main thread (Tauri event loop)                worker thread (recorder::run_worker)
   ┌──────────────────────────────────────┐      ┌───────────────────────────────────────────┐
   │ commands.rs (#[tauri::command])       │      │ capture.rs  ring-buffer VideoRecorder       │
   │  start_recording ─ spawns ───────────►│      │ rdev listener → events                      │
   │  stop_recording  ─ sets stop, joins   │◄──── │ overlay::event_targets_overlay drops the    │
   │  run_task (replay via rdev::simulate) │      │   float window's own clicks/drag/typing     │
   │  list/get/rename/delete_task          │      │ recorder state machine → Vec<UserAction>    │
   │  open_float_window                    │      │ compress.rs per before/after capture        │
   └───────────────┬──────────────────────┘      └──────────────────┬──────────────────────────┘
       AppState { rec: Mutex<Option<Session>>,                       │ RecorderOutput
                  replaying: AtomicBool }                            ▼
                                                  tasks.rs → %APPDATA%\sai-recorder\tasks\<id>\
                                                  (sai.json | trajectory.jsonl+manifest+screenshots/ + meta.json)
```

| Module      | Role |
|-------------|------|
| `main.rs`   | Tauri builder: `manage(AppState)`, `invoke_handler`, `run(generate_context!())`. |
| `commands.rs` | All `#[tauri::command]`s + `AppState` + replay engine. |
| `recorder.rs` | `start_worker(opts, stop, overlay_hwnd) -> JoinHandle<RecorderOutput>` + the action state machine (unchanged logic). |
| `tasks.rs`  | Task persistence under `%APPDATA%`, list/get/rename/delete/load. |
| `overlay.rs` | `event_targets_overlay()` — drops input aimed at the float window (Win32 hit-test). |
| `capture.rs` | Focused window/monitor metadata + ring-buffer screenshots (engine, unchanged). |
| `actions.rs` | `UserAction` schema (`types/actions.d.ts`), native rdev `Key`/`Button`; now `Serialize + Deserialize` for replay. |
| `compress.rs` | Crop / downscale / re-encode (lossless WebP or lossy JPEG) + token & byte stats. |
| `export.rs` | `sai` inline-base64 JSON, or `dataset` JSONL + `manifest.json` + `screenshots/`. |
| `telegram.rs` | Sends a task's payload to the user's Sai agent. |
| `review.rs` | Kept from the CLI version (currently unused by the GUI). |

---

## Build & run (Windows, GNU toolchain)

This machine uses `stable-x86_64-pc-windows-gnu` (no MSVC). The toolchain is
preconfigured: a global `~/.cargo/config.toml` points rustc at the self-contained
`dlltool` (`-Cdlltool=...`) so raw-dylib crates (the `windows` crate, etc.) link,
and the self-contained binutils dir (with `windres`/`ar`/`as`/`gcc` + sibling
DLLs) is on PATH for Tauri's resource compiler.

```bash
# Do NOT set RUSTFLAGS (it would override the global -Cdlltool config).
cargo build      # first build pulls wry/webview2-com/tao/windows — ~20+ min, ~340MB debug exe
cargo run        # launches the main window
```

Requirements:
* `icons/icon.ico` — a spec-compliant ICO (Tauri embeds it as a Windows resource).
* **WebView2 runtime** at run time. On this machine the dedicated WebView2
  Runtime was **not detected** (only full Edge). If the window fails to appear,
  install the Evergreen WebView2 runtime (per-user, no admin needed).

---

## Output schema (MimicCLI-compatible)

Each action carries `timestamp`/`duration` (ms) and always-present `before`/`after`
captures (`{ capture, focused:{window,screen} }`); `Key`/`Button` use rdev's native
serde names. Two payload layouts:

* **sai** → `sai.json`: a JSON array of `UserAction` with inline
  `data:image/...;base64` captures.
* **dataset** → `trajectory.jsonl` (one action per line, captures externalized to
  `screenshots/`) + `manifest.json` (session metadata + compression summary +
  self-documenting schema).

`meta.json` (every task) holds `id`, `name`, `created`, `mode`, `action_count`,
`duration_ms`, and the compression summary (baseline vs compressed **tokens** and
**bytes**, ratios, bytes/shot).

---

## Compression (The Token Company)

The ring buffer stores lossless full-frame WebP (the baseline). Per capture, the
options add focus-crop, downscale, and re-encode (smaller lossless WebP, or lossy
JPEG with a quality knob). The savings — token estimate **and** disk bytes vs the
lossless full-frame baseline — are stored per task and shown in the library.

---

## Replay (`run_task`)

Loads a task's actions and plays them back with `rdev::simulate` after a 3-2-1
countdown, on a background thread, guarded so it can't run during a recording:

* **Click / DoubleClick / TripleClick** — move to absolute coord, press/release ×N.
* **Drag** — press, interpolated moves to the end coord, release.
* **Scroll** — wheel events in the recorded direction × amount.
* **Type** — per-character via a US-QWERTY char→key map (Shift handled).
* **Press** — key down/up for each key.
* **Hover** — move only.

Inter-action timing is reconstructed from `timestamp`/`duration`, capped at 3 s.

### Replay compromises
* `Type` uses a **US-QWERTY** mapping; non-US layouts / non-ASCII characters are
  skipped (TODO: layout-aware / unicode typing).
* `Drag` is a single press → interpolated move → release; complex gesture drags
  aren't modeled.
* Coordinates are physical absolute pixels; replay assumes the same display
  layout/scale as recording.

---

## Privacy

Recording is opt-in (only inside a session, with a visible REC overlay). The
overlay's own clicks/drag/typing are dropped (`event_targets_overlay`) and it's
hidden from screenshots (`content_protected`). Nothing is sent anywhere unless you
click **Send** on a task; Telegram is configured via `SAI_TG_BOT_TOKEN` /
`SAI_TG_CHAT_ID` or `sai-recorder.config.json`.

## License

MIT
