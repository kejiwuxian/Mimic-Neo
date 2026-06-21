# Mimic Neo — Agent Handoff

> Pick up this project on a fresh workspace. Read top-to-bottom before touching code.
> **Where we left off:** Stitch UI fully ported + wired; 3 new backend commands compile & are registered; build green. Remaining headline work = the **OSWorld token-measurement pass** to backfill doc placeholders, plus polish. See §7–§8.

## 1. What this is
**Mimic Neo** is a Windows desktop app (**Tauri v2 + Rust**) that records on-screen user actions (mouse/keyboard + screenshots) into structured, compressed trajectories. Two outputs:
- **Automation / "sai" mode** — compact JSON workflows replayable by the Sai agent or the `run_task` command.
- **Dataset mode** — JSONL trajectories for computer-use training/eval datasets.

**Hero differentiator:** keyframe-based compression (temporal dedup of near-identical frames + animated-WebP concept) → ~6.6× fewer tokens vs a 1 fps lossless stream, while preserving replayability.

**Hackathon framing (CalHacks AI):** targets the **Toolbox** main track + the **Token Company** sponsor challenge (measurable token-cost reduction).

> **NAMING:** product is now **Mimic Neo**, but the crate / folder / exe / appdata dir are still named **`sai-recorder`**. Renaming is a deliberate TODO (§7.5) — do **not** assume it's done.

## 2. Repo
- GitHub: https://github.com/kejiwuxian/Mimic-Neo  (default branch: `main`)
- Clone: `git clone https://github.com/kejiwuxian/Mimic-Neo.git`
- The original push used a PAT embedded in the URL **which is being rotated** — use your own `gh auth` / credentials. No token is stored in the repo or this doc.

## 3. Layout
**`src/` — Rust backend (Tauri + engine)**
- `main.rs` — entry; CLI flags `--selftest`, `--selftest-record`; Tauri builder
- `commands.rs` — all `#[tauri::command]`s + `generate_handler!` registration (authoritative command list)
- `recorder.rs` — start/stop/finalize orchestration; `StopHotkey` (Ctrl+Alt+S); worker thread
- `capture.rs` — 3-thread capture: `rdev::listen` input hook + ring-buffer screenshots (GDI BitBlt fallback for VMs)
- `actions.rs` — `UserAction` enum (Hover/Click/DoubleClick/TripleClick/Drag/Scroll/Type/Press); `BaseAction{ timestamp, duration, before, after }`; `Capture{ capture: <data-URI> }`; `.base()` accessor
- `compress.rs` — frame compression + token/byte metrics
- `context.rs` — window metadata + screenshot context
- `export.rs` — write `sai.json` / `trajectory.jsonl` + dataset screenshots
- `tasks.rs` — persistence (`tasks/<id>/meta.json` + `sai.json`); `load_actions`, `compact_payload`, `export_jsonl`, `task_dir`
- `telegram.rs` — send task to Telegram (config: `%APPDATA%/sai-recorder/sai-recorder.config.json`)
- `review.rs` — finalize/review helpers · `log.rs` — file log to `%APPDATA%/sai-recorder/recorder.log`

**`ui/` — static frontend (multi-page; `withGlobalTauri`).** Ported from a Stitch (Material Design 3 dark) export.
- Screens: `index.html` (Onboarding), `dashboard`, `setup`, `recording`, `task-detail` (keyframe **player**), `library`, `replay`, `export`, `compression`, `settings`, `states`
- `float.html` — recording overlay (**Stop + drag grips only**)
- `app.js` — **ALL** wiring (nav + Tauri calls) in one file. `float.js`/`main.js`/`styles.css` are **legacy** pre-Stitch files superseded by `app.js` — safe to delete later.

`tauri.conf.json` — `frontendDist: "ui"`, `withGlobalTauri: true`.
`docs/` — DevPost write-up, pitch deck, demo scripts, README, `osworld-tasks.md`.

## 4. Tauri command API
- `open_float_window()`
- `start_recording({ opts: { mode, fps, history_secs, crop, lossy, quality, max_dim } })` — `mode`: "dataset" ⇒ Dataset, else Sai
- `stop_recording() -> TaskMeta`
- `list_tasks() -> TaskMeta[]`
- `get_task({ id }) -> { meta: TaskMeta, preview: String }` — `preview` is **truncated JSON text, not an image**
- `rename_task({ id, name }) -> TaskMeta`  ·  `delete_task({ id })`
- `run_task({ id })` — replay via `rdev::simulate`; emits `replay-*` events
- `get_telegram_status() -> TelegramStatus`  ·  `send_task_telegram({ id })`
- `get_task_playback({ id }) -> { frames: [{ src, t_ms, label }], duration_ms, count }` — **READ-ONLY** keyframe stream for the player
- `export_task_json({ id }) -> String` (compact sai.json)  ·  `export_task_jsonl({ id }) -> String` (on-disk trajectory.jsonl, else synthesized 1-JSON-per-action)

`TaskMeta = { id, name, created, mode, action_count, duration_ms, compression }`
`compression = { shots, baselineBytes, compressedBytes, sizeRatio, baselineTokensEst, compressedTokensEst, tokenRatio, compressedBytesPerShot }`
**Events:** `recording-started`, `recording-finished` (TaskMeta), `replay-countdown` (n; 0=go), `replay-progress` ({index,total}), `replay-finished`.

## 5. Build & run — TWO paths
Requires **WebView2 Runtime**, Rust, Tauri CLI.

**Path A — MSVC (preferred, if you have admin + VS Build Tools):** standard, no special config.
```
rustup default stable-x86_64-pc-windows-msvc
cargo build        # or: cargo tauri dev
```
**Path B — GNU (what the original workspace used; no admin/MSVC):**
Modern crates (parking_lot, windows) use raw-dylib linking on `x86_64-pc-windows-gnu` → rustc shells out to `dlltool.exe`, which needs ~80 sibling MinGW DLLs. Fix is a **global** `~/.cargo/config.toml`:
```toml
[target.x86_64-pc-windows-gnu]
rustflags = ["-Cdlltool=<ABS path to self-contained MinGW binutils dir>\\dlltool.exe"]
```
Point at the binutils **directory** (so Windows resolves its sibling DLLs); do **not** copy `dlltool.exe` out. Then `cargo build`.

> Frontend edits (`ui/*`) need **no recompile** — just relaunch the exe (or `cargo tauri dev`).

**Pipeline smoke test (no GUI):**
```
target\debug\sai-recorder.exe --selftest     # synthesizes a task, exit 0 = OK
```
Output: `%APPDATA%\sai-recorder\tasks\<id>\` (meta.json + sai.json).

## 6. Current status (verified)
- Stitch UI renders in webview; nav onboarding→dashboard→setup→recording works.
- `start_recording` → worker + ring buffer + float overlay + input capture confirmed in `recorder.log`.
- 3 new commands compile + registered; build **EXIT=0**.
- Compression `meta.json` contract matches frontend keys.
- Overlay = Stop + drag only. Task Detail = keyframe player (no editing). Export wires JSON/JSONL + copy.

## 7. Known issues / blockers
1. **Input hook wedges INJECTED input during recording.** `rdev::listen` is passive — **physical** mouse/keyboard pass through, so a human can click on-screen Stop or use **Ctrl+Alt+S** (both verified via selftest). But synthetic/automation input is starved → you can't drive Stop via automation; use physical input or kill the process. *Optional hardening:* filter the recorder's own injected events; ensure the capture channel never backpressures the hook thread; add a non-blocking stop path.
2. **Dashboard + Compression Insights now data-bound.** ✅ `pgDashboard` binds the 4 stat cards (Total Recordings, Time Captured, Tokens Saved, Avg Compression) and the Recent Activity table to `list_tasks`; `pgCompression` binds the hero ratio, the proportional Sai bar, and the per-recording token table. Both degrade gracefully to an empty-state row when there are no tasks. (Library was already bound.)
3. **Sidebar nav.** ✅ Default window is now 1200×800, min 1024×640 in `tauri.conf.json` — always wider than the dashboard's `md:` (768px) sidebar breakpoint, so the nav is always visible. (Requires a rebuild to take effect — config is compiled into the binary.)
4. **No literal `combined.webp` file.** The player streams keyframes instead. The `image` crate here is still-only; animated-WebP needs native libwebp (risky on GNU/no-MSVC). Optional: add an encoder on an MSVC workspace if a downloadable `.webp` is wanted.
5. **Naming.** crate/dir/exe/appdata still `sai-recorder`. Rename to `mimic-neo` as one coordinated pass: `Cargo.toml` name, tauri identifier, appdata dir in code, exe, README/docs.

## 8. Next steps (priority)
1. **OSWorld measurement pass** — record 5 real tasks (Chrome, LibreOffice Calc, Writer, File Explorer, Notepad per `docs/osworld-tasks.md`) using **physical input**; read each `meta.json` compression block; backfill the **18 `[__]` token placeholders** across `docs/`. Use **real** OSWorld numbers, **not** the selftest's inflated ~700× (selftest frames are near-identical).
2. **Polish** — ✅ dashboard + compression insights data-bound; ✅ default window size set so the sidebar shows.
3. *(Optional)* input-hook hardening · crate rename · combined.webp encoder.
4. **Finalize DevPost submission** (materials in `docs/`).

## 9. Verification recipe
- Build: `cargo build` (GNU: ensure the `~/.cargo/config.toml` dlltool fix).
- Pipeline: `sai-recorder.exe --selftest` → check `%APPDATA%\sai-recorder\tasks\<id>\meta.json`.
- GUI: launch → onboarding → setup → Start Recording → record with **physical** input → Stop on overlay → Task Detail player → Export JSON/JSONL.
- Logs: `%APPDATA%\sai-recorder\recorder.log`.

## 10. Message to the next agent

Hi — handing this back to you. Since last time:

- **Polish (§8.2) is done.** Dashboard and Compression Insights are now data-bound to `list_tasks` (stat cards, recent-activity table, hero ratio, per-recording token table). Default window is 1200×800 / min 1024×640 so the sidebar always shows. `cargo build --release` is green; `--selftest` exits 0.
- **CI/CD added** — [.github/workflows/ci.yml](.github/workflows/ci.yml). Every push/PR to `main` (and manual *Run workflow*) builds for Windows on `windows-latest`, runs the `--selftest` smoke test, and uploads the runnable exe as an artifact. Tag pushes (`v*`) also publish a Release with the exe + MSI/NSIS installers.

**👉 You don't have to build locally first — grab the prebuilt binary and try it:**
1. GitHub → **Actions** tab → latest **"CI (Windows build)"** run on `main`.
2. Scroll to **Artifacts** → download **`mimic-neo-windows-x64`** (it's a zip containing `sai-recorder.exe`).
3. Unzip and run `sai-recorder.exe` on a Windows box with the **WebView2 Runtime** installed (preinstalled on Win10/11). The `ui/` frontend is compiled into the exe — it's self-contained, no separate assets needed.
4. Smoke-test headless first: `sai-recorder.exe --selftest` → expect `SELFTEST OK ...` and a new task under `%APPDATA%\sai-recorder\tasks\<id>\`.
5. Then launch the GUI and walk: onboarding → setup → **Start Recording** (use **physical** input — see §7.1) → Stop → Task Detail player → check the **Home** dashboard + **Insights** screens now show real numbers.

If CI hasn't run yet, push any commit to `main` or use **Actions → CI (Windows build) → Run workflow** to trigger it.

The big remaining item is still the **OSWorld measurement pass (§8.1)** — it needs real apps + physical input, which I couldn't do from my environment. Heads-up: the `docs/osworld-tasks.md` / DevPost files referenced in §8 aren't in the repo yet (only `docs/animated-webp-recording.md` exists), so there are currently no `[__]` placeholders to backfill — you may need to author those docs as part of that pass.

⚠️ Rotate the GitHub PAT that's embedded in the `origin` remote URL and reset the remote to a clean `https://github.com/kejiwuxian/Mimic-Neo.git`.
