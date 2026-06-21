// Float control window: count-up timer + Stop button.
const { invoke } = window.__TAURI__.core;

const start = Date.now();
const timerEl = document.getElementById("timer");

function tick() {
  const s = Math.floor((Date.now() - start) / 1000);
  const mm = String(Math.floor(s / 60)).padStart(2, "0");
  const ss = String(s % 60).padStart(2, "0");
  timerEl.textContent = `${mm}:${ss}`;
}
setInterval(tick, 250);
tick();

document.getElementById("stop").addEventListener("click", async () => {
  try { await invoke("stop_recording"); }
  catch (e) { console.error(e); }
});
