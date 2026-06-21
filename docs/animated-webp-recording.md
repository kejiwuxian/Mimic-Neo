# Action Recorder → Animated-WebP Pipeline (Implementation Guide)

A reference for re-implementing the screen-action recording pipeline in another
project. It records user input (mouse/keyboard), captures the screen around each
action, and produces a compact, lossless **animated WebP** plus a JSON action
timeline suitable for feeding to an LLM. Every design choice is explained so you
can adapt it rather than copy it blindly.

Reference stack (Rust, Windows): `rdev` (input hook), `xcap` (screen capture +
`image` crate for WebP), `webp-animation` (libwebp `WebPAnimEncoder`),
`lz4_flex` (fast LZ4), `windows-sys` (DPI awareness), `serde`/`serde_json`.
The *concepts* are language-agnostic; the gotchas are mostly Windows/DPI-specific.

---

## 1. What it produces

For a recording session the tool writes to the working directory:

- **`session.webp`** — a single lossless **animated WebP** containing only the
  **per-action keyframes** (a "before" and an "after" frame for each recorded
  action, deduplicated). This is the canonical visual record.
- **`actions.json`** — the typed action timeline. Each action references frames
  in `session.webp` **by millisecond timestamp** (`beforeTs`/`afterTs`) instead
  of embedding image data.
- **`combined.webp`** *(optional, behind a `--overlay` flag)* — the same frames
  with action **markers drawn on top** (click ripples, drag arrows, etc.), for
  human review. Never consumed by the AI.

> Earlier iterations also emitted a separate transparent `overlay.webp` layer and
> inlined base64 screenshots per action. Both were dropped — see §10.

---

## 2. High-level architecture

Three threads + a finalize pass:

```
                 rdev::listen (main thread)
                       │ raw input events
                       ▼
   ┌───────────────────────────────┐        ┌──────────────────────────────┐
   │ Producer thread                │ Msg::* │ Resolver thread (consumer)   │
   │  - input state machine         ├───────▶│  - holds "pending" actions   │
   │  - coalesces typing/scroll     │        │  - matures each after AFTER_ │
   │  - emits UserAction + instant  │        │    DELAY, pulls before/after │
   │  - detects stop shortcut       │        │    frames from the ring      │
   └───────────────────────────────┘        │  - on End: finalize()        │
                                             └──────────────┬───────────────┘
   ┌───────────────────────────────┐                       │
   │ Capture thread (in ring mod)   │  LZ4 frames           │ at finalize:
   │  - pulls raw frames from xcap  │──────────────────────▶│  encode session.webp
   │  - throttles to RING_FPS       │     ring buffer       │  (+ combined.webp)
   │  - LZ4-compresses, stores ring │                       │  write actions.json
   └───────────────────────────────┘                       │  exit
```

**Why split producer/consumer/capture across threads?** The OS input hook
(`rdev::listen`) must stay responsive — any slow work on it (screenshot encode,
window enumeration) drops or delays events, including the stop shortcut. So the
hook does nothing but forward events to the producer over a channel. The producer
does only cheap bookkeeping. All heavy work (image encode, diffing) is deferred to
the resolver/finalize, off the input path.

---

## 3. The frame ring buffer (lazy, LZ4)

A background **capture thread** continuously pulls raw RGBA frames from the screen
recorder and stores a rolling window (e.g. last 10 s at 10 fps) in a ring buffer.

```
struct RawFrame { lz4: Vec<u8>, width: u32, height: u32 }  // LZ4 of RGBA8
struct BufferedFrame { timestamp: Instant, frame: RawFrame }
ring: VecDeque<BufferedFrame>  // capacity = history_secs * fps
```

Capture loop:
```
loop {
    frame = raw_rx.recv()            // blocks; Err when recorder dropped → exit
    if stop_flag { break }
    while let Ok(f) = raw_rx.try_recv() { frame = f }   // drain to the freshest
    if now - last_stored < 1/fps { continue }           // throttle to target fps
    store BufferedFrame { now, lz4_compress(frame.rgba) }  // evict oldest if full
    last_stored = now
}
```

**Design choices & rationale:**

- **Store LZ4-compressed raw frames, NOT encoded WebP.** This is the single most
  important decision. Lossless WebP/PNG encoding is expensive (tens of ms for a 4K
  frame); doing it on every captured frame either drops FPS or piles up work.
  LZ4 runs at multiple GB/s, so the capture thread never falls behind. **WebP
  encoding is deferred to retrieval**, where only the ~2 frames per action that
  are actually used get encoded — typically <2% of captured frames. (Measured:
  LZ4 of a 2560×1600 RGBA frame ≈ 0.6 MB in ~4 ms vs. lossless WebP ≈ 0.6 MB in
  much longer, per frame.)
- **Drain-to-latest each iteration.** If the consumer (encode/store) ever lags,
  draining the channel to the newest frame and discarding the rest bounds memory
  and keeps timestamps current. The queue can never grow unboundedly.
- **A single capture thread, not one-thread-per-frame.** Spawning an encode thread
  per frame (a tempting first design) lets threads pile up without bound under load,
  each pinning a full raw frame. One thread doing one unit of work at a time can't
  pile up.
- **Throttle to a target FPS.** The OS recorder may deliver frames faster than
  needed; we only keep one per `1/fps` interval to bound the ring and keep
  inter-frame spacing roughly uniform.

Why a *ring* (bounded history) and not "store everything"? We only need frames in
a small time window around each action (a "before" slightly in the past and an
"after" a beat later). A 10 s rolling window covers that with bounded memory,
regardless of session length.

---

## 4. Action parsing (producer state machine)

This is the heart of the recorder: it turns a noisy stream of raw OS input events
(`KeyPress`, `KeyRelease`, `ButtonPress`, `ButtonRelease`, `MouseMove`, `Wheel`)
into a clean sequence of semantic `UserAction`s. It runs on its own thread, reads
events from a channel fed by the input hook, and does **no image work** — only
bookkeeping and timing, so it can never fall behind and miss the stop shortcut.

Output action types: `Click` / `DoubleClick` / `TripleClick`, `Drag`, `Scroll`,
`Type`, `Press`, `Hover`.

### 4.1 Why a state machine (not 1:1 event→action)

Raw events are too granular and ambiguous to use directly:
- A word typed is dozens of `KeyPress`/`KeyRelease` events → should be **one**
  `Type` action with the text.
- A scroll gesture is many `Wheel` ticks → **one** `Scroll` with a summed amount.
- A button-down + move + button-up is **either** a click **or** a drag, decided by
  distance.
- Two quick clicks at the same spot are a **double-click**, not two clicks.
- Modifier keys held during a click aren't their own actions — they're context on
  the click.

So the producer buffers and coalesces, and only emits an action when it's
"complete." Completion is triggered by a **boundary event** (a different kind of
action starting) or an **idle timeout**.

### 4.2 State the producer carries

```
pressed: Set<Key>                 // currently-held keys (for modifiers + stop combo)
last_x, last_y: f64               // latest mouse position (from MouseMove)

// mouse button / drag
button_down_instant: Option<Instant>
drag_start_x, drag_start_y: f64   // mouse pos at button-down

// multi-click detection
last_click_time: Option<Instant>
last_click_x, last_click_y: f64
click_count: u32

// typing coalescing
key_buffer: String
typing_start: Option<Instant>

// scroll coalescing
scroll_dx, scroll_dy: i64
scroll_start: Option<Instant>

// pending non-printable key (emitted on release for true duration)
pending_press: Option<(Key, Instant)>

// hover / dwell
last_move_instant: Option<Instant>
hover_armed: bool

watching: bool                    // false after stop shortcut
ended: bool
```

There are three independent "in-progress" buffers — **typing**, **scroll**, and
**pending press** — each with its own start instant. They're flushed
independently. (Mouse clicks/drags are emitted immediately on button-up, so they
need no buffer beyond the down-instant.)

### 4.3 Per-event handling

At the top of every event take `now = Instant::now()`. Use this **one**
dequeue-time `Instant` clock for all ring lookups and for `before_instant`; the
ring is keyed on `Instant`, so do **not** use the event's wall-clock
(`SystemTime`) time for lookups. Keep wall-clock only for the human-facing
`timestamp`/`duration` fields if you want absolute times.

**`KeyPress(key)`**
1. `pressed.insert(key)`.
2. **Stop check first**, before anything else: if `!ended` and every key of the
   stop combo is in `pressed`, flush typing/scroll/press, set `ended/watching`,
   send `End`, return. Two rules here: (a) do it *first* so it fires even if the
   producer is mid-stream; (b) **never `process::exit` from this thread** — only
   signal `End`. The resolver owns shutdown, because there may be unresolved
   actions (their "after" frames not yet matured) that finalize must still write.
3. If `!watching`, return.
4. **Printable vs. not** (the crucial split): input libraries expose the produced
   character as `event.name` (e.g. `"a"`, `"1"`, `" "`); modifiers/arrows/F-keys
   have empty/none. If `name` is non-empty → it's text:
   - if `key_buffer` is empty, set `typing_start = now`;
   - append `name` to `key_buffer`; **return** (stay in typing mode).
5. Otherwise it's a control key → it **ends** any typing/scroll run: `flush_typing`,
   `flush_scroll`. Then, if it isn't a modifier, it becomes a **pending press**:
   `flush_press` (emit any previous one), then `pending_press = Some((key, now))`.
   Modifiers are deliberately *not* turned into `Press` actions — they only live in
   `pressed` so they can decorate other actions via `keys`.

**`KeyRelease(key)`**
1. `pressed.remove(key)`.
2. If `pending_press` is this key, `flush_press(now)` — emitting the `Press` here
   gives its true held duration (down→up).

**`ButtonPress(button)`** (if watching)
1. A mouse action is a boundary: `flush_typing`, `flush_scroll`, `flush_press`.
2. Record `button_down_instant = now`, `drag_start = (last_x, last_y)`.
   (Nothing is emitted yet — we don't know if it's a click or drag.)

**`ButtonRelease(button)`** (if watching)
1. `start = button_down_instant` (fallback `now`).
2. **Drag vs click:** `is_drag = button == Left && distance(drag_start, last) >
   DRAG_THRESHOLD`.
3. If drag → emit `Drag { start_coordinate, coordinate, keys }`; reset the
   multi-click state (`click_count = 0`, `last_click_time = None`).
4. Else → **multi-click detection**:
   `same_spot = last_click_time within CLICK_TIME_THRESHOLD AND
   distance(last_click_pos, now_pos) < CLICK_DIST_THRESHOLD`.
   `click_count = same_spot ? click_count + 1 : 1`. Update `last_click_*`.
   Emit `Click` / `DoubleClick` / `TripleClick` by `click_count` (cap at 3).
5. `keys = held_modifiers(pressed)` attached to whichever action.

**`MouseMove { x, y }`**
- Update `last_x/last_y`, set `last_move_instant = now`, `hover_armed = true`.
  (Movement re-arms hover detection; positions are read by click/drag/scroll/hover.)

**`Wheel { dx, dy }`** (if watching)
1. Scroll is a boundary for typing/press but coalesces with itself:
   `flush_typing`, `flush_press`.
2. If a scroll is already accumulating **and the dominant direction flips**
   (`scroll_direction(old) != scroll_direction(new)`), `flush_scroll` first so a
   direction change starts a new action.
3. If `(scroll_dx, scroll_dy) == 0`, set `scroll_start = now`.
4. Accumulate `scroll_dx += dx; scroll_dy += dy`. (Emitted later by a boundary or
   the idle timeout.)

### 4.4 Idle timeout (`on_idle`)

The event channel is read with `recv_timeout(DEBOUNCE_DELAY)`. A timeout means the
user paused. On timeout (if watching):
1. `flush_typing`, `flush_scroll` — a pause ends an in-progress typing/scroll run
   even without a boundary event.
2. **Hover/dwell:** if `hover_armed` and nothing else is in progress (no button
   down, empty typing/scroll, no pending press), emit a `Hover` at the current
   position with `before_instant = last_move_instant` (when the cursor arrived),
   then `hover_armed = false` (don't re-emit until the next move). A stationary
   cursor produces no events, so the timeout firing *is* the dwell signal — a
   natural, zero-cost detector. (Hovers that produced no visible change are dropped
   later in finalize, §6.)

### 4.5 Flush helpers (emit a buffered action)

Each returns whether it emitted, and clears its buffer:

- **`flush_typing(now)`**: if `key_buffer` non-empty → emit
  `Type { text: take(key_buffer), keys }`, `timestamp = typing_start`,
  `duration = now - typing_start`.
- **`flush_scroll(now)`**: if any delta → emit
  `Scroll { coordinate: (last_x,last_y), direction: scroll_direction(dx,dy),
  amount: max(|dx|,|dy|), keys }`; reset deltas/start.
- **`flush_press(now)`**: if `pending_press` set → emit `Press { keys: [key] }`,
  `timestamp = press_instant`, `duration = now - press_instant`.

Helper rules:
- `held_modifiers(pressed)` = the held keys that are in the modifier set
  (Ctrl/Shift/Alt/Meta/AltGr/CapsLock), or none.
- `scroll_direction(dx, dy)` = if `|dy| > |dx|` then `dy>0 ? Up : Down` else
  `dx>0 ? Right : Left`.

### 4.6 Emitting to the resolver

Every flushed/emitted action is sent with the instant the action **started** (used
later for the ring "before" lookup) and an `is_hover` flag:
```
enum Msg {
    Action { action: UserAction, before_instant: Instant, is_hover: bool },
    End,
}
```
The action carries its `before_focused` (focused-window metadata captured *now*,
on the producer thread — cheap thanks to the focus-scan throttle, §9.2) and an
empty `after_focused`/`beforeTs`/`afterTs` to be filled by the resolver/finalize.

### 4.7 Thresholds (tune per platform/feel)

| Rule | Reference | Notes |
|---|---|---|
| typing/scroll flush on pause | `DEBOUNCE_DELAY` 800 ms | also the hover dwell trigger |
| drag vs click | `DRAG_THRESHOLD` 10 px | movement beyond this between down/up = drag |
| multi-click window | `CLICK_TIME_THRESHOLD` 400 ms | gap under this = same click streak |
| multi-click jitter | `CLICK_DIST_THRESHOLD` 5 px | must stay within this to count as multi |

### 4.8 Boundary/flush ordering — why it matters

The invariant is **at most one in-progress buffer flushes into a coherent order**.
Any action that "starts something new" first flushes what's pending, so the emitted
timeline is correctly ordered and no buffer leaks into the wrong action:
- a control key, mouse button, or scroll ends typing;
- a mouse button or non-scroll key ends scrolling;
- a direction flip ends the current scroll and starts a new one;
- a pause (timeout) ends typing and scrolling;
- the stop shortcut and channel-disconnect flush everything.

Get this ordering wrong and you see artifacts like a `Type` emitted *after* the
`Click` that interrupted it, or two scrolls merged across a direction change.

---

## 5. Deferred "after" screenshots (resolver)

The "after" frame of an action must reflect the **settled** post-action UI, not
the instant the click landed. So each action is resolved after a delay:

- On `Msg::Action`, push to a `pending` list with
  `after_instant = before_instant + AFTER_DELAY` (~800 ms).
- Each tick (`recv_timeout` poll, ~100 ms), resolve every pending whose
  `after_instant` has passed: pull the **before** frame at `before_instant` and
  the **after** frame at `after_instant` from the ring (nearest-timestamp lookup),
  and capture the focused-window metadata for "after".
- On `Msg::End`, keep ticking until `pending` is empty, then `finalize()`.

**Rationale:**
- **Why defer instead of capturing "after" immediately?** A click's visual effect
  (menu opens, page navigates) takes a beat to render. Grabbing the newest frame at
  the moment of the click captures the *pre*-effect screen. Waiting `AFTER_DELAY`
  and reading the ring at that timestamp captures the result.
- **Why this is free:** the frame already exists in the ring (the capture thread
  runs continuously). The resolver just reads a past timestamp — no extra capture.
- **The stop path waits out `AFTER_DELAY`** so the last actions' "after" frames are
  captured before finalize.

---

## 6. Finalize: keyframes, dedup, encode

When recording stops and all pending actions resolve:

1. **Change detection per action.** Diff the before vs. after frames with a cheap
   coarse metric (downscaled tile diff: split into 32×32 tiles, a tile "changed" if
   its mean per-channel delta exceeds a threshold; `changed_area` = changed/total,
   plus a union bounding box). Set `changed` and `bbox` on the action. **Drop
   `Hover` actions whose before/after didn't change** — a dwell that produced no
   visible effect (tooltip, highlight) isn't worth recording.
2. **Build keyframe entries**: each kept action contributes a before frame (at its
   before timestamp) and an after frame (at its after timestamp). Sort by timestamp.
3. **Dedup adjacent near-identical frames.** Walking in time order, if a frame is
   within `DEDUP_THRESH` of the previously kept keyframe (same tile-diff metric),
   reuse that keyframe's timestamp instead of adding a new frame. This collapses
   "action N's after ≈ action N+1's before" (the screen was static between them)
   into one stored frame. Assign **strictly increasing** timestamps (bump on
   collision) — the animation encoder requires monotonic frame timestamps.
4. **Write the assigned timestamps back** onto each action's `beforeTs`/`afterTs`.
5. **Encode `session.webp`** from the keyframes (see §7), then **write
   `actions.json`**, then exit.

**Rationale:**
- **Only keyframes in the animation, not the whole 10 fps stream.** The AI needs
  the before/after of each action, not a full video. Storing ~2 frames/action
  (deduped) keeps the file tiny while inter-frame WebP compression handles the rest.
- **The change metric does triple duty**: gates `Hover`, drives dedup, and yields a
  `bbox`/`changed` flag useful downstream — all from one cheap function run off the
  hot path.

---

## 7. Animated WebP encoding

Use libwebp's animation encoder (`webp-animation` crate). Frames are added in
strictly-increasing-timestamp order; the encoder does inter-frame compression
internally.

**Encode lazily from an iterator** so only one full frame is materialized at a
time (the whole session never lives in RAM):
```
encode_animation(frames: impl Iterator<Item=(ts_ms, rgba, w, h)>, opaque: bool) -> bytes
    for (ts, rgba, w, h) in frames:
        if first: create Encoder((w,h))
        if opaque: set every pixel's alpha = 255
        encoder.add_frame(rgba, ts)
    encoder.finalize(last_ts + 1)
```
The caller passes a `map` over the LZ4 keyframes that decompresses each on demand.

**Force opaque alpha for `session.webp`.** Some capture backends return a zeroed
alpha channel; encoded as-is the frames would be fully transparent (the AI sees
blank images). Forcing `alpha = 255` (or encoding stills as RGB, dropping the alpha
plane) guarantees visible frames. libwebp then omits the alpha plane entirely.
(Measured: dropping alpha before LZ4 saves ~13% ring memory because interleaved
constant alpha bytes break LZ4's run-length matching, but the *output* WebP is
unaffected since libwebp already omits constant alpha — so we only force-opaque at
the encode boundary and leave the ring as RGBA.)

**Extraction for the LLM.** To feed a still to the model, decode `session.webp`,
find the frame whose timestamp is nearest the requested `beforeTs`/`afterTs`,
re-encode that single frame as a still WebP data URL. Because the analyzer requests
the *exact* keyframe timestamps, it always gets the clean before/after frames.

> **Can a single animated WebP hold a transparent overlay layer over a base?**
> Not as simultaneous layers — it's one canvas, one timeline. Each frame supports
> alpha + a blend mode, so you can composite a transparent frame *temporally* over
> the previous one, but the high-level encoder only takes full-canvas frames. For
> annotations we composite ourselves (§8), not via a WebP layer.

---

## 8. Action overlay (optional `combined.webp`)

When enabled, draw a marker for each positioned action and composite it onto the
frames, producing a second animation for humans. The AI's `session.webp` stays
clean.

- **Markers (hand-rolled drawing, no font/image-draw dependency):** click =
  concentric rings + dot; drag = line + arrowhead + start dot; scroll = directional
  chevrons; hover = ring. Keyboard actions (`Type`/`Press`) get none — no screen
  position.
- **Timeline:** the clean keyframes plus one **annotated mid-frame per action**
  (marker drawn over the action's *before* frame, inserted at a timestamp between
  before and after). So playback reads *before → before+marker → after*.
- **Compositing is done in code** (`out = base*(1-α) + marker*α`), then encoded
  opaque. The overlay frames are built transparent (alpha 0 except the marker) only
  as an intermediate.
- The presentation timeline uses synthetic increasing timestamps (e.g. index ×
  400 ms) — it doesn't need to match `session.webp`'s real timestamps, since only
  `session.webp` is queried by timestamp.

**Rationale:** keeps annotations out of the AI's input (markers could bias/confuse
the model), while giving a human-reviewable artifact. Gating it behind a flag
avoids the extra encode cost when not needed.

---

## 9. Critical platform gotchas (with reasons)

These caused real bugs; budget time for them.

1. **DPI awareness — input coords vs. captured pixels must share one space.**
   On Windows at non-100% display scaling, a DPI-*unaware* process receives mouse
   coordinates in **logical** space (e.g. ~1707×1067 at 150%) while the screen
   capture is in **physical** pixels (2560×1600). Markers/bboxes land ~1.5× off.
   **Fix:** make the process **per-monitor DPI-aware at startup**
   (`SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)`), so the input library and
   the capture library both report physical pixels → coordinates map 1:1. Verify on
   your platform; the scale factor is queryable from the monitor.

2. **Focused-window lookup must be cheap when nothing is focused.**
   Capturing focused-window metadata via "enumerate all windows, find the focused
   one" is expensive. When the user is on the desktop (no focused window) the cache
   misses and you re-enumerate on **every** action — which backs up the input
   thread and can make the stop shortcut feel dead. **Fix:** cache the focused
   window; fast-path return it if it's still focused (one syscall); otherwise
   re-scan **at most once per ~250 ms** (throttle). Bounds the worst case to ~4
   scans/sec regardless of action rate.

3. **Never `join()` the capture thread on shutdown.**
   If the screen-capture library doesn't close its frame channel on stop, the
   capture thread's blocking `recv()` never returns and a `join()` hangs the whole
   program. **Fix:** set a stop flag, drop/stop the recorder, then **detach** the
   thread (drop the handle) rather than joining. On the normal exit path you can
   skip stopping entirely and just `process::exit` — the OS reclaims everything.

4. **A "deselect"/show-desktop keystroke at startup is OS-specific.**
   A simulated Cmd-D (macOS deselect) becomes **Win+D = "Show Desktop"** on Windows,
   minimizing every window (→ no focused window → see gotcha #2, and screenshots of
   an empty desktop). Keep it only if intended for your target OS, and ensure the
   focused-window throttle (#2) absorbs the resulting desktop period. Simulated keys
   should fire **before** the input hook is installed so they aren't recorded.

5. **Animation frame timestamps must be strictly increasing.** Enforce when
   assigning keyframe timestamps and when building the overlay timeline.

6. **Give the user feedback.** Print on start ("Recording… press <shortcut> to
   stop"), on stop detection ("finalizing…"), and per encode step. Lossless encode
   of many keyframes can take seconds; without prints it looks hung and users
   Ctrl+C, corrupting output.

---

## 10. Rejected alternatives (and why)

- **Inline base64 screenshots per action in JSON** — original approach. Huge files
  (two full-screen images per action, +33% base64), unreadable JSON. Replaced by a
  single animated WebP + timestamp references.
- **Lossy WebP for ring frames** — considered for size, but the pure-Rust `image`
  WebP encoder is lossless-only, and lazy encoding made the size moot. Kept lossless.
- **Stream every frame into the animation during capture** — would put a sequential
  lossless encoder on the hot path: either drops FPS or grows memory unboundedly on
  weak hardware. Replaced by "LZ4 ring + encode only keyframes at finalize," which
  has neither problem.
- **Separate transparent `overlay.webp` file** — built and then dropped as
  redundant; the optional `combined.webp` covers human review, and the AI only
  needs the clean `session.webp`.
- **Stripping alpha before LZ4 in the ring** — ~13% ring-memory win, but adds a
  hot-path pass and a re-expansion on retrieval, for no change to output size.
  Not worth it; force-opaque only at the encode boundary instead.
- **Cropping the "after" to the changed bbox for the AI** — smaller, but loses
  surrounding context; the AI gets full stills. The `bbox` is still recorded as
  metadata.

---

## 11. `actions.json` shape (per action)

```jsonc
{
  "type": "Click",
  "timestamp": 12.34,          // seconds since recording start (human-facing)
  "duration": 0.0,
  "beforeTs": 12340,           // ms keyframe id into session.webp
  "afterTs": 13140,
  "beforeFocused": { /* window metadata at action time */ },
  "afterFocused":  { /* window metadata after settling */ },
  "changed": true,             // did the screen change before→after?
  "bbox": { "x": …, "y": …, "w": …, "h": … },   // changed region (optional)
  "button": "Left",            // type-specific fields
  "coordinate": { "x": …, "y": … },
  "keys": ["ControlLeft"]      // held modifiers (optional)
}
```
Variants add fields: `Drag` has `startCoordinate`+`coordinate`; `Scroll` has
`direction`+`amount`; `Type` has `text`; `Press` has `keys`.

---

## 12. Tunable constants

| Constant | Reference value | Meaning |
|---|---|---|
| `RING_HISTORY` | 10 s | rolling capture window |
| `RING_FPS` | 10 | capture/store rate |
| `AFTER_DELAY` | 800 ms | how long after an action to grab the "after" frame |
| `RESOLVE_POLL` | 100 ms | resolver tick |
| `DEBOUNCE_DELAY` | 800 ms | idle timeout to flush typing/scroll + detect dwell |
| `CHANGE_THRESH` | 0.5% tiles | "did the screen change" / keep-hover gate |
| `DEDUP_THRESH` | 0.2% tiles | "frames identical" → dedup |
| `FOCUS_SCAN_THROTTLE` | 250 ms | min spacing between full window scans |
| `FRAME_MS` (overlay) | 400 ms | playback spacing for combined.webp |
| WebP tile size | 32×32 | change-detection granularity |

---

## 13. Implementation order (suggested)

1. Ring buffer + capture thread (LZ4 store, throttle, drain-to-latest) and a
   `ring_frame_at(instant)` lookup. Verify with a standalone capture→retrieve test.
2. `encode_animation` (lazy iterator) + `extract_still`. Round-trip test: encode a
   couple synthetic frames, decode one back.
3. Producer state machine + stop shortcut + the `Msg` channel.
4. Resolver with deferred "after" + finalize (dedup, change detection, write).
5. DPI awareness, focus-scan throttle, non-hanging shutdown — the §9 gotchas.
6. Optional overlay/`combined.webp`.
7. Downstream: extract stills by timestamp where the timeline is consumed.

Test each stage in isolation; the input-hook + real-capture parts can't be unit
tested headlessly, so add progress prints and verify with one real recording.
