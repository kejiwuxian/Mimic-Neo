// Main window logic. Uses the global Tauri API (app.withGlobalTauri = true).
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);

function toast(msg) {
  const t = $("toast");
  t.textContent = String(msg);
  t.classList.remove("hidden");
  clearTimeout(toast._t);
  toast._t = setTimeout(() => t.classList.add("hidden"), 3500);
}

function setRecording(on) {
  $("recIndicator").classList.toggle("hidden", !on);
  $("startBtn").disabled = on;
}

function pct(ratio) {
  if (!ratio || ratio <= 0) return "0%";
  return Math.round((1 - 1 / ratio) * 100) + "%";
}

function renderTasks(tasks) {
  const list = $("taskList");
  list.innerHTML = "";
  $("emptyTasks").classList.toggle("hidden", tasks.length > 0);
  for (const t of tasks) {
    const c = t.compression || {};
    const tokenRatio = c.tokenRatio || 1;
    const sizeRatio = c.sizeRatio || 1;
    const row = document.createElement("div");
    row.className = "task";
    row.innerHTML = `
      <div class="info">
        <div class="name">${escapeHtml(t.name)}</div>
        <div class="meta">
          <span class="badge">${t.mode}</span>
          ${new Date(t.created).toLocaleString()} ·
          ${t.action_count} actions · ${(t.duration_ms/1000).toFixed(1)}s ·
          <span class="save">tokens ${tokenRatio.toFixed(1)}× (−${pct(tokenRatio)})</span>,
          <span class="save">disk ${sizeRatio.toFixed(1)}×</span>
        </div>
      </div>
      <div class="row-actions">
        <button class="primary" data-act="run">▶ Run</button>
        <button class="ghost" data-act="view">View</button>
        <button class="ghost" data-act="rename">Rename</button>
        <button class="ghost" data-act="send">Send</button>
        <button class="danger" data-act="delete">Delete</button>
      </div>`;
    row.querySelector('[data-act=run]').onclick = () => runTask(t.id);
    row.querySelector('[data-act=view]').onclick = () => viewTask(t.id);
    row.querySelector('[data-act=rename]').onclick = () => renameTask(t);
    row.querySelector('[data-act=send]').onclick = () => sendTask(t.id);
    row.querySelector('[data-act=delete]').onclick = () => deleteTask(t.id);
    list.appendChild(row);
  }
}

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (m) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[m]));
}

async function refreshTasks() {
  try { renderTasks(await invoke("list_tasks")); }
  catch (e) { toast(e); }
}

async function startRecording() {
  const opts = {
    mode: $("mode").value,
    fps: Number($("fps").value) || 10,
    historySecs: Number($("history").value) || 10,
    crop: $("crop").checked,
    lossy: $("lossy").checked,
    quality: Number($("quality").value) || 80,
    maxDim: $("maxDim").value ? Number($("maxDim").value) : null,
  };
  try { await invoke("start_recording", { opts }); }
  catch (e) { toast(e); }
}

async function runTask(id) {
  try { await invoke("run_task", { id }); }
  catch (e) { toast(e); }
}

async function viewTask(id) {
  try {
    const detail = await invoke("get_task", { id });
    const m = detail.meta, c = m.compression || {};
    $("modalTitle").textContent = m.name;
    $("modalMeta").innerHTML = `
      ${kv("Mode", m.mode)}
      ${kv("Actions", m.action_count)}
      ${kv("Duration", (m.duration_ms/1000).toFixed(1) + "s")}
      ${kv("Created", new Date(m.created).toLocaleString())}
      ${kv("Tokens", `${(c.baselineTokensEst||0)} → ${(c.compressedTokensEst||0)} (${(c.tokenRatio||1).toFixed(2)}×)`)}
      ${kv("Screenshots", `${fmtBytes(c.baselineBytes)} → ${fmtBytes(c.compressedBytes)} (${(c.sizeRatio||1).toFixed(2)}×)`)}
      ${kv("Per capture", fmtBytes(c.compressedBytesPerShot))}
    `;
    $("modalPreview").textContent = detail.preview;
    $("modal").classList.remove("hidden");
  } catch (e) { toast(e); }
}

function kv(k, v) { return `<div class="kv"><div class="k">${k}</div><div class="v">${escapeHtml(v)}</div></div>`; }
function fmtBytes(b) {
  b = Number(b) || 0;
  if (b > 1048576) return (b/1048576).toFixed(2) + " MB";
  if (b > 1024) return (b/1024).toFixed(1) + " KB";
  return b + " B";
}

async function renameTask(t) {
  const name = prompt("Rename recording:", t.name);
  if (!name) return;
  try { await invoke("rename_task", { id: t.id, name }); await refreshTasks(); }
  catch (e) { toast(e); }
}

async function deleteTask(id) {
  if (!confirm("Delete this recording permanently?")) return;
  try { await invoke("delete_task", { id }); await refreshTasks(); }
  catch (e) { toast(e); }
}

async function sendTask(id) {
  try { await invoke("send_task_telegram", { id }); toast("Sent to Telegram ✓"); }
  catch (e) { toast(e); }
}

// Replay overlay
function showReplay(big, msg) {
  $("replayBig").textContent = big;
  $("replayMsg").textContent = msg || "";
  $("replayOverlay").classList.remove("hidden");
}
function hideReplay() { $("replayOverlay").classList.add("hidden"); }

// Wire events
$("startBtn").onclick = startRecording;
$("refreshBtn").onclick = refreshTasks;
$("modalClose").onclick = () => $("modal").classList.add("hidden");

listen("recording-started", () => setRecording(true));
listen("recording-finished", () => { setRecording(false); refreshTasks(); });

listen("replay-countdown", (e) => {
  const n = e.payload;
  if (n > 0) showReplay(n, "Get ready — replaying soon…");
  else showReplay("▶", "Replaying…");
});
listen("replay-progress", (e) => {
  const p = e.payload || {};
  showReplay("▶", `Replaying ${p.index}/${p.total}…`);
});
listen("replay-finished", () => hideReplay());

// Init
refreshTasks();
