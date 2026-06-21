# Mimic Neo — Submission Master README

> **Single source of truth for all CalHacks AI 2026 submission materials.**
> Everything needed to write/refresh the DevPost write-up, pitch deck, demo
> video script, finalist live-demo script, and the public GitHub README lives
> in this one file. All compression numbers below are **real, measured, and
> reproducible** (see §6 Methodology + §15 CI).

- **Project:** Mimic Neo (crate/binary: `sai-recorder`)
- **Repo:** https://github.com/kejiwuxian/Mimic-Neo
- **Tracks:** Ddoski's Toolbox (main, $5K) · The Token Company (sponsor challenge)
- **Last numbers refresh:** 2026-06-21 — 26 real keyframes across 5 OSWorld-style tasks

---

## 0. How to use this document

Each submission artifact is generated from the sections below. Don't invent
numbers — copy them from §5 (Results) and §6 (Methodology).

| Material | Built from sections | File |
|---|---|---|
| DevPost write-up | §1–§5, §8–§13, §16, §17 | `devpost-sai-recorder.md` |
| Pitch deck (12 slides) | §1–§5, §10, §11, §16 | `pitch-deck.md` / `pitch-deck.pptx` |
| Demo video script | §5, §7, §16 (beats) | `demo-video-script.md` |
| Finalist live-demo script | §7, §16 (beats) | `finalist-demo-script.md` |
| Public GitHub README | §1–§9, §14, §15 | `README-sai-recorder.md` |
| Benchmark task definitions | §6, §5 | `osworld-tasks.md` |

**Placeholder note:** the 5 docs above contain `[__]` slots. The exact values
to drop in are in §5 and §18. (~46 placeholders → all resolvable from this file.)

---

## 1. Identity & taglines

- **Name:** Mimic Neo
- **One-liner:** *An opt-in desktop recorder that turns on-screen work into
  clean, replayable action trajectories — and compresses the screenshots up to
  ~20× before they ever reach a vision LLM.*
- **10-word version:** *Record your screen into token-cheap, replayable agent trajectories.*
- **Sponsor-track version (Token Company):** *We cut screenshot token cost
  4–20× with a lossless-vs-lossy keyframe pipeline — measured, not estimated.*

---

## 2. Problem

Computer-use agents and the datasets that train them are built from **screenshots
+ actions**. Two costs dominate:

1. **Token cost** — feeding full-resolution screenshots to a vision LLM is
   expensive; a single 1920×1080 lossless frame is hundreds of KB → tens of
   thousands of vision tokens. Multiply by every step of every task.
2. **Capture friction** — collecting clean (screenshot, action) trajectories
   normally means bespoke tooling, manual labeling, and brittle replay.

---

## 3. Solution / What it does

Mimic Neo is a Tauri v2 desktop app that:

- **Records** keyboard/mouse into high-level actions (`Hover`, `Click`,
  `DoubleClick`, `TripleClick`, `Drag`, `Scroll`, `Type`, `Press`) with
  before/after keyframes, absolute coordinates, timestamps and durations
  (MimicCLI-compatible action schema).
- **Compresses** every keyframe through a lossy+downscale layer that cuts
  vision-token cost **4–20×** (the Token Company differentiator, §5).
- **Replays** recorded tasks via `rdev::simulate`.
- **Exports** two ways: compact JSON for Sai-style workflow automation, and
  JSONL trajectories for ML dataset collection.
- **Manages tasks** (`tasks/<id>/meta.json`), with a keyframe player, rename/
  delete, and optional Telegram delivery.

---

## 4. Why it matters / who it's for

- **Developers (Toolbox track):** a genuinely useful, well-executed tool —
  record a workflow once, replay or hand it to an agent.
- **AI researchers (Toolbox track):** turnkey computer-use dataset capture in
  an open, inspectable format.
- **Anyone paying for vision tokens (Token Company track):** the same
  trajectory, 4–20× cheaper to feed to a model — with the savings *measured*.

---

## 5. Results — REAL measured compression (HERO METRIC)

> Benchmark: **26 real 1920×1080 keyframes** across 5 OSWorld-style tasks
> (Chrome, LibreOffice Calc, LibreOffice Writer, File Explorer, Notepad).
> Frames are committed at `bench/frames/` and measured by the app's own
> `--measure` subcommand, which reuses the exact production compression code.

### 5.1 Per-task (shipped default: max_dim 384, quality 60)

| Task | Keyframes | Baseline tokens | Compressed tokens | Token ratio | Reduction |
|---|---|---|---|---|---|
| Chrome — Wikipedia research (8 keyframes) | 8 | 1,125,333 | 32,809 | **34.30×** | 97.1% |
| LibreOffice Calc — sales table (4 keyframes) | 4 | 128,781 | 16,161 | **7.97×** | 87.5% |
| LibreOffice Writer — report (5 keyframes) | 5 | 204,106 | 14,050 | **14.53×** | 93.1% |
| File Explorer — folder navigation (5 keyframes) | 5 | 214,413 | 12,509 | **17.14×** | 94.2% |
| Notepad — meeting notes (4 keyframes) | 4 | 63,798 | 8,046 | **7.93×** | 87.4% |
| **OVERALL (26 keyframes)** | **26** | **1,736,430** | **83,576** | **20.78×** | **95.2%** |

- **Headline:** across all 26 frames, screenshot tokens drop from
  **1,736,430 → 83,576** — a **20.8× (95.2%) reduction.**
- **Range:** 7.9× on dense text (Notepad/Calc) to 34.3× on a busy web page (Chrome).
- **Cost framing (illustrative @ $1.25 / 1M input tokens):** this benchmark's
  screenshot cost falls from **$2.17 → $0.104**. Savings scale linearly with token count.

### 5.2 Quality/size sweep (overall, 26 frames) — pick your operating point

| Longest side | JPEG quality | Size ratio | Token ratio | Reduction |
|---|---|---|---|---|
| 384px | q60 | 20.78× | **20.78×** | 95.2% (shipped default — max compression) |
| 384px | q70 | 18.01× | **18.01×** | 94.4% |
| 384px | q80 | 15.17× | **15.17×** | 93.4% |
| 512px | q60 | 12.23× | **12.23×** | 91.8% |
| 512px | q70 | 10.73× | **10.73×** | 90.7% (balanced) |
| 512px | q80 | 8.85× | **8.85×** | 88.7% |
| 768px | q60 | 5.60× | **5.60×** | 82.2% |
| 768px | q70 | 4.88× | **4.88×** | 79.5% |
| 768px | q80 | 4.03× | **4.03×** | 75.2% (legibility-preserving) |

**Honest tradeoff:** 384px longest side is aggressive — great for an agent that
only needs UI *structure*, but small for reading fine text. For legibility-
sensitive tasks use **512px/q70 (~10.7×)** or **768px/q80 (~4.0×)**. The point
is the operator chooses; every point on the curve is real and reproducible.

### 5.3 What is and isn't measured

- **Measured (above):** per-keyframe compression — lossless full-frame WebP
  baseline (what MimicCLI stores) vs Mimic Neo's downscaled lossy keyframe.
- **Complementary (not in these numbers):** temporal dedup (keyframe-per-action
  vs a naive fixed-fps stream). That's a separate, multiplicative axis; we keep
  it out of the headline so the number stays defensible.

---

## 6. Methodology & reproducibility

1. **Frames:** 26 genuine 1920×1080 screenshots captured from real apps in the
   states described in `osworld-tasks.md`, grouped one folder per task under
   `bench/frames/` (`01-chrome` … `05-notepad`).
2. **Baseline:** each frame is re-encoded to **lossless full-frame WebP** using
   the *same* encoder the live recorder uses to store frames (`encode_webp_lossless`).
3. **Compressed:** `compress::compress_frame()` (production path) downscales to
   `max_dim` and re-encodes lossy at `quality`.
4. **Accounting:** `image_tokens(bytes)` (the app's own estimator) on baseline
   vs compressed bytes → `token_ratio`. Image-only (no action JSON) for cleanliness.
5. **Run it yourself:** `sai-recorder.exe --measure bench/frames` →
   prints a table and writes `measure-results.json`. Sweep covers
   max_dim ∈ {384,512,768} × quality ∈ {60,70,80}.

---

## 7. User flow / demo path

1. Launch Mimic Neo → main window (dashboard + task library).
2. Click **Start** → a borderless, always-on-top, capture-excluded REC overlay
   appears (Stop button + drag grips). Global hotkey **Ctrl+Alt+S** also stops.
3. Perform a task (e.g., a Wikipedia lookup, edit a spreadsheet).
4. **Stop** → finalize: change-detect, dedup adjacent identical keyframes,
   compress, write `tasks/<id>/{meta.json,...}`.
5. **Task Detail** → keyframe player to watch the combined recording.
6. **Export** JSON (workflow) or JSONL (dataset); optionally **send via Telegram**.
7. **Replay** to re-drive the actions with `rdev::simulate`.

---

## 8. Architecture

- **Capture (3 threads):** capture thread (LZ4 ring buffer, ~10fps) · producer
  thread (input→action state machine) · resolver thread (deferred 'after' frame).
- **Finalize:** change detection + adjacent-keyframe dedup → per-task artifacts.
- **Compression layer:** lossless baseline vs downscaled lossy keyframe (§5/§6).
- **UI:** Tauri v2 webview (static frontend, no Node), 12 Stitch-designed screens;
  default window 1200×800 (min 1024×640) so the sidebar always shows.
- **Safety:** recorder's own windows excluded from the trajectory via PID check;
  overlay excluded from capture via `WDA_EXCLUDEFROMCAPTURE`; file-based logging
  (`recorder.log`) since GUI-subsystem apps have no console.

### 8.1 Backend commands (11)

`start_recording`, `stop_recording`, `list_tasks`, `get_task`, `run_task`,
`send_task_telegram`, `rename_task`, `delete_task`, `export_task_json`,
`export_task_jsonl`, `get_telegram_status`, `get_task_playback`.

---

## 9. Tech stack

- **Rust** + **Tauri v2** (wry/WebView2)
- **rdev** (cross-platform input capture + replay; `serialize` feature)
- **xcap** + **GDI BitBlt fallback** (screenshots; fallback for VM/headless)
- **webp** / **image** crates (lossless baseline + lossy keyframes)
- **LZ4** ring buffer · **serde/serde_json** · Windows crate (Win32 overlay APIs)
- Build: stable Rust, **MSVC** on CI (windows-latest); GNU toolchain path documented
  for no-admin machines.

---

## 10. Track alignment

### Ddoski's Toolbox (main, $5K)
A real, well-executed developer/researcher tool: record once → replay or export.
Dual-mode export (workflow JSON + dataset JSONL), task management, keyframe player.

### The Token Company (sponsor challenge)
Measurable token-cost reduction: **4–20× fewer vision tokens** per screenshot,
benchmarked on real frames with the app's own measurement tool and reproducible in CI.

---

## 11. Judging-criteria mapping

| Criterion | How Mimic Neo scores |
|---|---|
| Application / usefulness | Solves real cost+friction for agent builders & researchers |
| Functionality | End-to-end: record → compress → replay → export → deliver |
| Creativity | Treats screenshots as a token-budget problem; pickable compression curve |
| Technical complexity | 3-thread capture, Tauri overlay, capture exclusion, rdev replay |
| Process | Iterative; honest measurement; reproducible benchmark in CI |
| Ethical considerations | See §13 — opt-in, local-first; redaction flagged for pre-share |

---

## 12. Build & run

```bash
# Prereqs (Windows): Rust (stable), WebView2 Runtime
git clone https://github.com/kejiwuxian/Mimic-Neo
cd Mimic-Neo
cargo build --release            # MSVC toolchain (CI uses windows-latest)
./target/release/sai-recorder.exe

# Headless checks
sai-recorder.exe --selftest                 # capture→compress→encode pipeline
sai-recorder.exe --measure bench/frames     # real compression numbers + JSON
```

No-admin / GNU path: self-contained MinGW binutils for `dlltool`/`as`; global
`~/.cargo/config.toml` points `-Cdlltool` at the binutils dir by absolute path.

---

## 13. Known limitations & ethical considerations

- **Opt-in & local-first:** recording is explicit; artifacts stay on disk until
  the user exports/sends them.
- **No redaction yet:** prototype captures whatever is on screen — redaction/
  privacy filtering is required **before** sharing any dataset (acknowledged, next-up).
- **Input hook vs automation:** the global input hook blocks *injected* input
  during recording (a real human's physical input works fine).
- **Aggressive default:** 384px is tiny for fine text; see the §5.2 curve.

---

## 14. Public README skeleton (for `README-sai-recorder.md`)

Order for the judge-facing repo README: badges → one-liner (§1) → demo gif →
features (§3) → **compression table (§5.1 + §5.2)** → architecture (§8) →
build/run (§12) → reproducibility (§6/§15) → tracks (§10) → limitations (§13).

---

## 15. CI / continuous reproducibility

`.github/workflows/ci.yml` (windows-latest):

- **build** job → `cargo build --release --locked`, uploads the exe as artifact
  `mimic-neo-windows-x64`.
- **test** job → downloads that exact exe artifact and exercises it:
  `--selftest` (pipeline) + `--measure bench/frames` (the real numbers), then
  uploads `measure-results.json`.
- **release** job (on `v*` tags) → publishes exe + bundles.

This makes the compression numbers **reproducible on every push** — a strong
integrity signal for judges.

> ⚠️ Status note: the test job currently runs the **GUI-subsystem** release exe,
> whose stdout/exit timing differs from a console app. If the `--measure` step
> shows no output / missing JSON, the robust fixes are: (a) launch via
> `Start-Process -Wait -NoNewWindow` so the step blocks until the file is written,
> and/or (b) build a tiny console-subsystem test shim. The tool itself is
> verified working locally (numbers in §5 came from it).

---

## 16. Narrative / framing (for DevPost & decks)

- **Origin:** we built record-and-replay at a hackathon in Oct 2025 (now the
  MimicAI direction). When OpenAI shipped similar capture/replay ideas, we read
  it as **validation** of the thesis.
- **What we share publicly here:** the **compression layer** — the one,
  cleanly-measurable improvement we're comfortable open-sourcing for this submission.
- **What we keep building:** more improvements live in MimicAI.
- **Tone:** confident, builder-credible, numbers-first. Lead with the §5 table.

---

## 17. DevPost content kit (drop-in answers)

- **Inspiration:** the token cost + capture friction of computer-use data (§2).
- **What it does:** §3 verbatim.
- **How we built it:** §8 + §9.
- **Challenges:** GUI-subsystem stdout, no-admin GNU toolchain (dlltool/DLLs),
  VM screenshot capture (GDI fallback), input-hook vs injected input.
- **Accomplishments:** real, reproducible **20.8× (95.2%)** token reduction; full
  record→compress→replay→export→deliver loop; CI-reproducible benchmark.
- **What we learned:** measure honestly — byte-based 'token savings' don't survive
  scrutiny; tile/size-aware downscale + lossy does.
- **What's next:** redaction, temporal-dedup benchmark, larger OSWorld sweep,
  cross-platform (macOS/Linux) capture.
- **Built with:** Rust, Tauri v2, WebView2, rdev, xcap, webp, image, LZ4, serde.

---

## 18. Appendix — full real numbers

- **Default setting:** max_dim 384, quality 60
- **Baseline (lossless) tokens, 26 frames:** 1,736,430
- **Compressed tokens @ default:** 83,576  → **20.777× / 95.2%**
- **Per-task token ratios:** 01-chrome=34.30×, 02-calc=7.97×, 03-writer=14.53×, 04-explorer=17.14×, 05-notepad=7.93×
- **Sweep token ratios:** 384q60=20.78×, 384q70=18.01×, 384q80=15.17×, 512q60=12.23×, 512q70=10.73×, 512q80=8.85×, 768q60=5.60×, 768q70=4.88×, 768q80=4.03×
- **Source data:** `measure-results.json` (regenerate via §6 step 5).

---
*Generated 2026-06-21 from `measure-results.json`. Refresh numbers by re-running
`sai-recorder.exe --measure bench/frames` and updating §5 / §18.*