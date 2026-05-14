// Shared helpers for popover. Uses window.__TAURI__ (withGlobalTauri).

const tauri = window.__TAURI__ || {};
const { invoke } = tauri.core || tauri.tauri || {};
const { listen } = tauri.event || {};

export async function getSnapshot() {
  return await invoke("get_snapshot");
}

export async function refreshNow() {
  await invoke("refresh_now");
}

export async function showSettings() {
  await invoke("show_settings");
}

export async function saveConfig(cfg) {
  await invoke("save_config", { cfg });
}

export async function autoDetect() {
  return await invoke("auto_detect");
}

export function onUsageUpdated(cb) {
  if (!listen) return () => {};
  return listen("usage-updated", (ev) => cb(ev.payload));
}

export function colorClass(pct) {
  if (pct >= 0.85) return "red";
  if (pct >= 0.6) return "amber";
  return "";
}

export function formatPct(pct) {
  return Math.round((pct || 0) * 100) + "%";
}

export function formatReset(iso) {
  if (!iso) return "—";
  const t = new Date(iso);
  const delta = Math.max(0, t - new Date());
  const mins = Math.floor(delta / 60000);
  if (mins < 60) return `${mins}m`;
  const hrs = Math.floor(mins / 60);
  const rem = mins % 60;
  if (hrs < 24) return `${hrs}h ${rem}m`;
  const days = Math.floor(hrs / 24);
  return `${days}d ${hrs % 24}h`;
}

export function renderProfile(update, opts = {}) {
  const compact = opts.compact || false;

  const div = document.createElement("div");
  div.className = "profile";
  div.dataset.id = update.profile_id;

  const state = update.state;
  let pct5h = 0, pct7d = 0, reset5h = null, reset7d = null, statusLabel = "";

  if (state.kind === "ok") {
    pct5h = state.sess_pct;
    pct7d = state.week_pct;
    reset5h = state.sess_reset;
    reset7d = state.week_reset;
    statusLabel = state.status;
  } else if (state.kind === "stale") {
    pct5h = state.last.sess_pct;
    pct7d = state.last.week_pct;
    reset5h = state.last.sess_reset;
    reset7d = state.last.week_reset;
    statusLabel = "stale";
    div.classList.add("error");
  } else if (state.kind === "needs_relogin") {
    div.classList.add("error");
    statusLabel = "re-login required";
  } else if (state.kind === "not_pro_max") {
    div.classList.add("error");
    statusLabel = "not Pro/Max";
  } else if (state.kind === "error") {
    div.classList.add("error");
    statusLabel = "error";
  }

  // Prefer email over profile name as the display label.
  const displayName = update.email || update.profile_name;

  div.innerHTML = `
    <h3><span>${escapeHtml(displayName)}</span></h3>
    <div class="bar-row">
      <span class="label">5h</span>
      <div class="bar ${colorClass(pct5h)}"><span style="width:${Math.min(100, pct5h * 100)}%"></span></div>
      <span class="num ${colorClass(pct5h)}">${formatPct(pct5h)}</span>
    </div>
    <div class="bar-row">
      <span class="label">7d</span>
      <div class="bar ${colorClass(pct7d)}"><span style="width:${Math.min(100, pct7d * 100)}%"></span></div>
      <span class="num ${colorClass(pct7d)}">${formatPct(pct7d)}</span>
    </div>
    ${compact ? "" : `
    <div class="meta">
      <span>resets 5h in ${formatReset(reset5h)} · 7d in ${formatReset(reset7d)}</span>
      <span>${escapeHtml(statusLabel)}</span>
    </div>`}
  `;

  if (state.kind === "needs_relogin") {
    const hint = document.createElement("div");
    hint.className = "meta";
    hint.style.cssText = "display:flex;align-items:center;gap:8px;justify-content:space-between;";
    const reasonSpan = document.createElement("span");
    reasonSpan.textContent = state.reason || "login required";
    const openBtn = document.createElement("button");
    openBtn.className = "ghost";
    openBtn.textContent = "Open Settings";
    openBtn.style.cssText = "font-size:11px;padding:2px 8px;";
    openBtn.addEventListener("click", () => { showSettings(); });
    hint.appendChild(reasonSpan);
    hint.appendChild(openBtn);
    div.appendChild(hint);
  }

  return div;
}

export function escapeHtml(s) {
  return String(s || "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}
