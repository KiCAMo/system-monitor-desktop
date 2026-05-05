// main window logic: topbar, panels, xterm, event listeners.
const { invoke } = window.__TAURI__.core;
const { listen, emit } = window.__TAURI__.event;
const { getAllWebviewWindows } = window.__TAURI__.webviewWindow;
const {
  sendNotification,
  isPermissionGranted,
  requestPermission,
} = window.__TAURI__.notification;

const $ = (id) => document.getElementById(id);
const consoleEl = $("consoleStream");

// ---------- xterm.js for the agent PTY panel ----------
const term = new Terminal({
  fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
  fontSize: 12,
  cursorBlink: true,
  convertEol: true,
  theme: {
    background: '#1a1d24',
    foreground: '#d0d4dc',
    cursor: '#c4b46a',
  },
});
const fitAddon = new FitAddon.FitAddon();
term.loadAddon(fitAddon);
term.open($("claudeStream"));
const fitNow = () => { try { fitAddon.fit(); } catch (_) {} };
fitNow();
window.addEventListener("resize", fitNow);
setTimeout(fitNow, 100);
setTimeout(fitNow, 500);

let _claudeBuf = "";
let _claudeScheduled = false;

term.onData(async (data) => { await invoke("pty_raw", { data }); });
term.onResize(async ({ cols, rows }) => { await invoke("pty_resize", { cols, rows }); });

// ---------- Batched DOM rendering ----------
const MAX_NODES_ACTIVE = 1500;
const MAX_NODES_HIDDEN = 300;
const MAX_QUEUED = 4000;

const _queues = new WeakMap();
let _scheduled = false;

function append(el, text, cls) {
  let q = _queues.get(el);
  if (!q) { q = []; _queues.set(el, q); }
  q.push({ text, cls });
  if (q.length > MAX_QUEUED) q.splice(0, q.length - MAX_QUEUED);
  if (!_scheduled) {
    _scheduled = true;
    requestAnimationFrame(flushAll);
  }
}

function flushAll() {
  _scheduled = false;
  for (const el of document.querySelectorAll(".stream, .node-stream")) {
    const q = _queues.get(el);
    if (!q || !q.length) continue;
    const isHidden = el.classList.contains("hidden") || el.offsetParent === null;
    const cap = isHidden ? MAX_NODES_HIDDEN : MAX_NODES_ACTIVE;
    let lines = q.splice(0, q.length);
    if (lines.length > cap) lines = lines.slice(-cap);
    const frag = document.createDocumentFragment();
    for (const { text, cls } of lines) {
      const span = document.createElement("span");
      if (cls) span.className = cls;
      span.textContent = text + "\n";
      frag.appendChild(span);
    }
    el.appendChild(frag);
    while (el.childElementCount > cap) el.removeChild(el.firstChild);
    if (!isHidden) el.scrollTop = el.scrollHeight;
  }
}

function setStatus(running) {
  $("statusDot").classList.toggle("on", !!running);
  $("statusText").textContent = running ? "running" : "idle";
}

// ---------- Per-node EC2 log panels (tabs) ----------
const ec2Streams = {};
let activeTab = null;
function ensureNodeStream(name) {
  if (ec2Streams[name]) return ec2Streams[name];
  const tabs = $("ec2Tabs");
  const tab = document.createElement("button");
  tab.className = "tab";
  tab.textContent = name;
  tab.addEventListener("click", () => activateTab(name));
  tabs.appendChild(tab);

  const pre = document.createElement("pre");
  pre.className = "stream small node-stream";
  pre.dataset.node = name;
  $("ec2Streams").appendChild(pre);

  ec2Streams[name] = pre;
  if (!activeTab) activateTab(name);
  return pre;
}
function activateTab(name) {
  activeTab = name;
  for (const el of document.querySelectorAll("#ec2Tabs .tab"))
    el.classList.toggle("active", el.textContent === name);
  for (const el of document.querySelectorAll(".node-stream"))
    el.classList.toggle("hidden", el.dataset.node !== name);
}

// ---------- Per-node dashboard cards ----------
const dashCards = {};
function ensureDashCard(name) {
  if (dashCards[name]) return dashCards[name];
  const root = $("dashRoot");
  const wrap = document.createElement("div");
  wrap.className = "dash-node";
  wrap.innerHTML = `
    <div class="dash-node-title">${name}</div>
    <div class="cards">
      ${["state","cpu","mem","mem_pct","net","block","pids","health"]
        .map(k => `<div class="card"><div class="card-title">${k}</div>
                   <div class="card-value" data-k="${k}">—</div></div>`).join("")}
    </div>`;
  root.appendChild(wrap);
  const refs = {};
  wrap.querySelectorAll(".card-value").forEach(e => refs[e.dataset.k] = e);
  dashCards[name] = refs;
  return refs;
}
function applyMetrics(payload) {
  const refs = ensureDashCard(payload.node || "node-1");
  const d = payload.docker || {};
  for (const k of ["state","cpu","mem","mem_pct","net","block","pids"])
    if (refs[k]) refs[k].textContent = d[k] || "—";
  if (refs.health) {
    const h = payload.health;
    refs.health.textContent = !h ? "—" :
      (h.status_code ? `HTTP ${h.status_code}` :
       (h.error ? "ERR" : "—"));
  }
}

// ---------- Tauri event listeners ----------
function tsOf(ev) {
  return (ev.ts || "").replace("T", " ").replace("Z", "");
}

listen("ec2-line", (e) => {
  const ev = e.payload;
  const node = ev.payload.node || "node-1";
  append(ensureNodeStream(node), ev.payload.line);
});

listen("console-msg", (e) => {
  const ev = e.payload;
  const ts = tsOf(ev);
  const lvl = ev.payload.level || "INFO";
  const node = ev.payload.node ? `[${ev.payload.node}] ` : "";
  const cls = lvl === "ERROR" || lvl === "MATCH" ? "line-error"
            : lvl === "MONITOR" ? "line-monitor"
            : lvl === "SYSTEM" ? "line-system" : "";
  append(consoleEl, `[${ts}] [${lvl}] ${node}${ev.payload.msg}`, cls);
});

listen("claude-chunk", (e) => {
  const ev = e.payload;
  _claudeBuf += ev.payload.chunk;
  if (!_claudeScheduled) {
    _claudeScheduled = true;
    requestAnimationFrame(() => {
      _claudeScheduled = false;
      const buf = _claudeBuf;
      _claudeBuf = "";
      if (buf) term.write(buf);
    });
  }
});

listen("metrics-update", (e) => { applyMetrics(e.payload.payload); });
listen("monitor-status", (e) => { setStatus(e.payload.payload.running); });

listen("match-detected", (e) => { notifyMatch(e.payload.payload); });

// ---------- Cross-window events (settings/history → main) ----------
listen("config-changed", () => { refreshMeta(); });
listen("profile-loaded", (e) => {
  refreshMeta();
  const name = (e.payload && e.payload.name) || "";
  const ts = new Date().toISOString().replace("T"," ").slice(0,19);
  append(consoleEl, `[${ts}] [SYSTEM] 프로필 적용: ${name}`, "line-system");
});
listen("agent-injected", (e) => {
  const id = (e.payload && e.payload.id) || "?";
  const ts = new Date().toISOString().replace("T"," ").slice(0,19);
  append(consoleEl, `[${ts}] [SYSTEM] 히스토리 #${id} 재주입됨`, "line-system");
});

// ---------- Top bar buttons ----------
$("startBtn").addEventListener("click", () => invoke("start_monitoring"));
$("stopBtn").addEventListener("click", () => invoke("stop_monitoring"));

function pruneStaleNodes(activeNames) {
  const active = new Set(activeNames);
  for (const name of Object.keys(dashCards)) {
    if (!active.has(name)) {
      const card = dashCards[name];
      const wrap = card?.state?.closest(".dash-node");
      if (wrap) wrap.remove();
      delete dashCards[name];
    }
  }
  for (const name of Object.keys(ec2Streams)) {
    if (!active.has(name)) {
      ec2Streams[name].remove();
      const tab = [...document.querySelectorAll("#ec2Tabs .tab")]
        .find(b => b.textContent === name);
      if (tab) tab.remove();
      delete ec2Streams[name];
      if (activeTab === name) activeTab = null;
    }
  }
  if (!activeTab && activeNames.length) activateTab(activeNames[0]);
}

async function refreshMeta() {
  const c = await invoke("get_config_summary");
  const ns = (c.nodes || []).map(n => `${n.name}@${n.host}:${n.container}`).join(", ");
  $("meta").textContent = `nodes=${ns || "(none)"}  auto=${c.auto_submit ? "on" : "off"}`;
  setStatus(c.running);
  const names = (c.nodes || []).map(n => n.name);
  pruneStaleNodes(names);
  for (const n of c.nodes || []) {
    ensureNodeStream(n.name);
    ensureDashCard(n.name);
  }
}
refreshMeta();

// ---------- Native notifications (Tauri) ----------
async function ensureNotifyPermission() {
  let granted = await isPermissionGranted();
  if (!granted) {
    const p = await requestPermission();
    granted = p === "granted";
  }
  return granted;
}

async function notifyMatch(m) {
  try {
    const granted = await isPermissionGranted();
    if (!granted) return;
    const title = `[${m.node}] ${m.container || ""} 에러 감지`;
    const body = (m.matched_line || "").slice(0, 200);
    await sendNotification({ title, body });
  } catch (_) {}
}

$("notifyBtn").addEventListener("click", async () => {
  const granted = await ensureNotifyPermission();
  if (granted) {
    await sendNotification({ title: "알림 활성화됨", body: "에러 매칭 시 푸시됩니다." });
  } else {
    alert("시스템 설정에서 알림을 허용해주세요.");
  }
});

// ---------- Open / focus secondary windows ----------
async function showWindow(label) {
  const wins = await getAllWebviewWindows();
  const w = wins.find(x => x.label === label);
  if (!w) return;
  await w.show();
  await w.setFocus();
}

$("settingsBtn").addEventListener("click", () => showWindow("settings"));
$("historyBtn").addEventListener("click", () => showWindow("history"));
