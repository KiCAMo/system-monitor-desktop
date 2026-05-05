// Tauri globals (enabled via tauri.conf.json `withGlobalTauri: true`)
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
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

// Forward keystrokes typed in the terminal back to the PTY
term.onData(async (data) => {
  await invoke("pty_raw", { data });
});
// Notify backend so the PTY winsize matches xterm columns/rows
term.onResize(async ({ cols, rows }) => {
  await invoke("pty_resize", { cols, rows });
});

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
    const isHidden = el.classList.contains("hidden")
      || el.offsetParent === null;
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

// ---------- Tauri event listeners (replaces WebSocket) ----------
// Each Rust-emitted Event has shape { ts, channel, payload }; the Tauri
// listener wraps it as `{ event, id, payload, windowLabel }`, so the
// inner data is at `e.payload.payload`.

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

listen("metrics-update", (e) => {
  applyMetrics(e.payload.payload);
});

listen("monitor-status", (e) => {
  setStatus(e.payload.payload.running);
});

listen("match-detected", (e) => {
  notifyMatch(e.payload.payload);
  if (!historyModal.classList.contains("hidden")) loadHistoryList();
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

// ---------- History modal ----------
const historyModal = $("historyModal");
let _selectedHistoryId = null;

async function openHistory(focusId) {
  historyModal.classList.remove("hidden");
  await populateHistoryNodeFilter();
  await loadHistoryList();
  if (focusId) await openHistoryDetail(focusId);
}
function closeHistory() {
  historyModal.classList.add("hidden");
  $("historyDetail").classList.add("hidden");
  _selectedHistoryId = null;
}

async function populateHistoryNodeFilter() {
  const cfg = await invoke("get_config_summary");
  const sel = $("historyNode");
  const cur = sel.value;
  sel.innerHTML = '<option value="">(전체 노드)</option>' +
    (cfg.nodes || []).map(n => `<option value="${n.name}">${n.name}</option>`).join("");
  if (cur) sel.value = cur;
}

async function loadHistoryList() {
  const q = $("historyQuery").value.trim();
  const node = $("historyNode").value;
  const args = { limit: 200 };
  if (q) args.q = q;
  if (node) args.node = node;
  const rows = await invoke("list_history", args);
  const list = $("historyList");
  list.innerHTML = "";
  if (!rows || !rows.length) {
    list.innerHTML = '<div class="picker-row" style="color:var(--muted)">매칭 기록이 없습니다.</div>';
    return;
  }
  for (const row of rows) {
    const el = document.createElement("div");
    el.className = "history-row";
    el.dataset.id = row.id;
    const ts = (row.ts || "").replace("T", " ").replace("Z", "");
    el.innerHTML =
      `<span class="ts">${ts}</span>` +
      `<span class="node">${row.node}</span>` +
      `<span class="line">${escapeHtml(row.matched_line || "")}</span>`;
    el.addEventListener("click", () => openHistoryDetail(row.id));
    list.appendChild(el);
  }
}

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
  }[c]));
}

async function openHistoryDetail(id) {
  _selectedHistoryId = id;
  let row;
  try {
    row = await invoke("get_history", { match_id: id });
  } catch (_) { return; }
  if (!row) return;
  $("historyDetailMeta").textContent =
    `#${row.id} · ${row.ts} · ${row.node} (${row.container || "?"}) · pattern=${row.pattern || "?"}`;
  $("historyDetailBlock").textContent = row.block || "";
  $("historyDetail").classList.remove("hidden");
}

$("historyBtn").addEventListener("click", () => openHistory());
$("historyClose").addEventListener("click", closeHistory);
$("historyDetailClose").addEventListener("click", () => $("historyDetail").classList.add("hidden"));
$("historyRefresh").addEventListener("click", loadHistoryList);
$("historyQuery").addEventListener("keydown", (e) => {
  if (e.key === "Enter") { e.preventDefault(); loadHistoryList(); }
});
$("historyNode").addEventListener("change", loadHistoryList);

$("historyInject").addEventListener("click", async () => {
  if (!_selectedHistoryId) return;
  try {
    await invoke("inject_history", { match_id: _selectedHistoryId });
    $("historyDetailMeta").textContent += "  ← 재주입 완료";
  } catch (_) {
    alert("재주입 실패: 에이전트 세션이 안 떠있을 수 있어요.");
  }
});

$("historyDel").addEventListener("click", async () => {
  if (!_selectedHistoryId) return;
  if (!confirm("이 매칭 기록을 삭제할까요?")) return;
  await invoke("delete_history", { match_id: _selectedHistoryId });
  $("historyDetail").classList.add("hidden");
  _selectedHistoryId = null;
  await loadHistoryList();
});

$("historyClear").addEventListener("click", async () => {
  if (!confirm("전체 매칭 히스토리를 모두 삭제할까요? 되돌릴 수 없습니다.")) return;
  await invoke("clear_history");
  $("historyDetail").classList.add("hidden");
  _selectedHistoryId = null;
  await loadHistoryList();
});

// ---------- Settings modal ----------
const modal = $("settingsModal");
const form = $("settingsForm");
const msgEl = $("settingsMsg");

function flatten(obj, prefix = "") {
  const out = {};
  for (const [k, v] of Object.entries(obj || {})) {
    const key = prefix ? `${prefix}.${k}` : k;
    if (v && typeof v === "object" && !Array.isArray(v)) Object.assign(out, flatten(v, key));
    else out[key] = v;
  }
  return out;
}
function unflatten(flat) {
  const out = {};
  for (const [k, v] of Object.entries(flat)) {
    const parts = k.split(".");
    let cur = out;
    for (let i = 0; i < parts.length - 1; i++) {
      cur[parts[i]] = cur[parts[i]] || {};
      cur = cur[parts[i]];
    }
    cur[parts.at(-1)] = v;
  }
  return out;
}

const LIST_FIELDS = new Set([
  "monitor.error_patterns", "claude.command", "claude.deny_patterns",
]);
const NUM_FIELDS = new Set([
  "ec2.ssh_port",
  "monitor.before_lines", "monitor.after_lines",
  "monitor.after_delay_seconds", "monitor.cooldown_seconds",
  "monitor.max_block_chars",
  "deploy.build_timeout_seconds",
]);
const NULLABLE_FIELDS = new Set([
  "ec2.user", "ec2.known_hosts", "ec2.deploy_key", "ec2.diag_key",
  "deploy.build_cmd", "deploy.build_cwd", "deploy.artifact_path",
  "deploy.sftp_target", "deploy.remote_deploy_cmd",
]);

// --- nodes list editor ---
function renderNodeRow(node = { name: "", host: "", container: "", health_url: null, deploy_key: null }) {
  const row = document.createElement("div");
  row.className = "node-row node-row-2line";
  row.innerHTML = `
    <input class="n-name" placeholder="이름 (예: web-1)" />
    <input class="n-host" placeholder="호스트 IP" />
    <input class="n-container" placeholder="컨테이너 이름" />
    <input class="n-health" placeholder="health URL (옵션)" />
    <button type="button" class="n-del">×</button>
    <span class="ifield n-key-wrap">
      <input class="n-key" placeholder="배포 키 경로 (비우면 EC2 공통 키 사용)" />
      <button type="button" class="pick" data-kind="file">📁</button>
    </span>`;
  row.querySelector(".n-name").value = node.name || "";
  row.querySelector(".n-host").value = node.host || "";
  row.querySelector(".n-container").value = node.container || "";
  row.querySelector(".n-health").value = node.health_url || "";
  row.querySelector(".n-key").value = node.deploy_key || "";
  row.querySelector(".n-del").addEventListener("click", () => row.remove());
  return row;
}
function fillNodes(nodes) {
  const list = $("nodesList");
  list.innerHTML = "";
  for (const n of nodes || []) list.appendChild(renderNodeRow(n));
  if (!list.children.length) list.appendChild(renderNodeRow());
}
function readNodes() {
  return [...$("nodesList").querySelectorAll(".node-row")].map(row => ({
    name: row.querySelector(".n-name").value.trim(),
    host: row.querySelector(".n-host").value.trim(),
    container: row.querySelector(".n-container").value.trim(),
    health_url: row.querySelector(".n-health").value.trim() || null,
    deploy_key: row.querySelector(".n-key").value.trim() || null,
  })).filter(n => n.name && n.host && n.container);
}
$("addNodeBtn").addEventListener("click", () =>
  $("nodesList").appendChild(renderNodeRow()));

// ---------- Path picker (custom modal — kept identical to web UX) ----------
const picker = $("pickerModal");
let pickerTarget = null;
let pickerKind = "any";
let pickerSelected = null;

async function pickerLoad(path) {
  const d = await invoke("browse_fs", {
    path: path || null,
    kind: pickerKind,
    show_hidden: $("pickerHidden").checked,
  });
  $("pickerPath").value = d.cwd;
  pickerSelected = pickerKind === "dir" ? d.cwd : null;
  $("pickerSel").textContent = pickerSelected || "(파일을 선택하세요)";
  const list = $("pickerList"); list.innerHTML = "";
  if (d.parent) {
    const row = document.createElement("div");
    row.className = "picker-row dir";
    row.innerHTML = `<span class="ico">⬆</span><span>..</span>`;
    row.addEventListener("click", () => pickerLoad(d.parent));
    list.appendChild(row);
  }
  for (const e of d.entries) {
    const row = document.createElement("div");
    row.className = "picker-row " + (e.is_dir ? "dir" : "file");
    row.innerHTML = `<span class="ico">${e.is_dir ? "📁" : "📄"}</span><span>${e.name}</span>`;
    const full = d.cwd.replace(/\/$/, "") + "/" + e.name;
    row.addEventListener("click", (ev) => {
      ev.stopPropagation();
      if (e.is_dir) {
        pickerLoad(full);
      } else {
        for (const r of list.querySelectorAll(".picker-row.selected")) r.classList.remove("selected");
        row.classList.add("selected");
        pickerSelected = full;
        $("pickerSel").textContent = pickerSelected;
      }
    });
    list.appendChild(row);
  }
}

function openPicker(input, kind) {
  pickerTarget = input;
  pickerKind = kind || "any";
  $("pickerKind").textContent = kind === "dir" ? "디렉터리" : (kind === "file" ? "파일" : "파일/디렉터리");
  const start = input.value || "";
  picker.classList.remove("hidden");
  pickerLoad(start);
}
function closePicker() { picker.classList.add("hidden"); pickerTarget = null; }

document.addEventListener("click", (e) => {
  const btn = e.target.closest("button.pick");
  if (!btn) return;
  e.preventDefault();
  const input = btn.parentElement.querySelector("input");
  openPicker(input, btn.dataset.kind);
});

$("pickerClose").addEventListener("click", closePicker);
$("pickerCancel").addEventListener("click", closePicker);
$("pickerHome").addEventListener("click", () => pickerLoad(""));
$("pickerUp").addEventListener("click", () => {
  const cur = $("pickerPath").value;
  pickerLoad(cur.replace(/\/[^/]+\/?$/, "") || "/");
});
$("pickerPath").addEventListener("keydown", (e) => {
  if (e.key === "Enter") { e.preventDefault(); pickerLoad(e.target.value); }
});
$("pickerHidden").addEventListener("change", () => pickerLoad($("pickerPath").value));
$("pickerSelect").addEventListener("click", () => {
  if (!pickerSelected || !pickerTarget) return;
  pickerTarget.value = pickerSelected;
  closePicker();
});

const PROVIDER_DEFAULT_CMD = { claude: "claude", codex: "codex" };

function fillForm(cfg) {
  const flat = flatten(cfg);
  for (const el of form.elements) {
    if (!el.name) continue;
    let v = flat[el.name];
    if (v === undefined || v === null) v = "";
    if (el.type === "checkbox") {
      el.checked = !!v;
    } else if (el.tagName === "SELECT") {
      el.value = v || el.options[0].value;
    } else if (LIST_FIELDS.has(el.name)) {
      el.value = Array.isArray(v) ? v.join(el.name === "claude.command" ? " " : "\n") : v;
    } else {
      el.value = v;
    }
  }
  fillNodes(cfg.nodes);
}

form.addEventListener("change", (e) => {
  if (e.target.name !== "claude.provider") return;
  const cmdEl = form.elements["claude.command"];
  const cur = (cmdEl.value || "").trim();
  const others = Object.values(PROVIDER_DEFAULT_CMD);
  if (!cur || others.includes(cur)) {
    cmdEl.value = PROVIDER_DEFAULT_CMD[e.target.value] || "";
  }
});

function readForm() {
  const flat = {};
  for (const el of form.elements) {
    if (!el.name) continue;
    let v;
    if (el.type === "checkbox") v = el.checked;
    else if (LIST_FIELDS.has(el.name)) {
      const sep = el.name === "claude.command" ? /\s+/ : /\n+/;
      v = el.value.split(sep).map(s => s.trim()).filter(Boolean);
    } else if (NUM_FIELDS.has(el.name)) {
      v = el.value === "" ? null : Number(el.value);
    } else if (NULLABLE_FIELDS.has(el.name)) {
      v = el.value === "" ? null : el.value;
    } else {
      v = el.value;
    }
    flat[el.name] = v;
  }
  const obj = unflatten(flat);
  obj.nodes = readNodes();
  return obj;
}

async function openSettings() {
  msgEl.textContent = ""; msgEl.className = "msg";
  const cfg = await invoke("get_settings");
  fillForm(cfg);
  await refreshProfiles();
  modal.classList.remove("hidden");
}

// ---------- Profiles ----------
async function refreshProfiles() {
  const profiles = await invoke("list_profiles");
  const sel = $("profileSelect");
  const cur = sel.value;
  sel.innerHTML = '<option value="">(프로필 선택)</option>' +
    (profiles || []).map(n => `<option value="${n}">${n}</option>`).join("");
  if (cur && (profiles || []).includes(cur)) sel.value = cur;
}

$("profileLoad").addEventListener("click", async () => {
  const name = $("profileSelect").value;
  if (!name) { msgEl.textContent = "프로필을 선택하세요"; msgEl.className = "msg err"; return; }
  msgEl.textContent = `'${name}' 불러오는 중...`; msgEl.className = "msg";
  try {
    await invoke("load_profile", { name });
  } catch (err) {
    msgEl.textContent = "오류: " + (err?.toString() || "load failed");
    msgEl.className = "msg err";
    return;
  }
  const cfg = await invoke("get_settings");
  fillForm(cfg);
  refreshMeta();
  msgEl.textContent = `'${name}' 적용 + 재시작 완료`;
  msgEl.className = "msg ok";
});

$("profileSaveAs").addEventListener("click", async () => {
  let name = $("profileNewName").value.trim() || $("profileSelect").value;
  if (!name) { msgEl.textContent = "새 프로필 이름을 입력하세요"; msgEl.className = "msg err"; return; }
  const body = readForm();
  if (!body.nodes.length) {
    msgEl.textContent = "최소 1개 노드는 등록해야 합니다.";
    msgEl.className = "msg err";
    return;
  }
  try {
    await invoke("save_profile", { name, config: body });
  } catch (err) {
    msgEl.textContent = "오류: " + (err?.toString() || "save failed");
    msgEl.className = "msg err";
    return;
  }
  $("profileNewName").value = "";
  await refreshProfiles();
  $("profileSelect").value = name;
  msgEl.textContent = `'${name}' 프로필 저장됨`;
  msgEl.className = "msg ok";
});

$("profileDelete").addEventListener("click", async () => {
  const name = $("profileSelect").value;
  if (!name) return;
  if (!confirm(`'${name}' 프로필을 삭제할까요?`)) return;
  try {
    await invoke("delete_profile", { name });
    await refreshProfiles();
    msgEl.textContent = `'${name}' 삭제됨`;
    msgEl.className = "msg";
  } catch (_) {}
});
function closeSettings() { modal.classList.add("hidden"); }

$("settingsBtn").addEventListener("click", openSettings);
$("settingsClose").addEventListener("click", closeSettings);
$("settingsCancel").addEventListener("click", closeSettings);
form.addEventListener("submit", async (e) => {
  e.preventDefault();
  msgEl.textContent = "저장 중..."; msgEl.className = "msg";
  const body = readForm();
  if (!body.nodes.length) {
    msgEl.textContent = "최소 1개 노드는 등록해야 합니다.";
    msgEl.className = "msg err";
    return;
  }
  try {
    await invoke("save_settings", { config: body });
    msgEl.textContent = "저장 완료. 재시작됨.";
    msgEl.className = "msg ok";
    pruneStaleNodes(body.nodes.map(n => n.name));
    for (const n of body.nodes) { ensureNodeStream(n.name); ensureDashCard(n.name); }
    refreshMeta();
    setTimeout(closeSettings, 800);
  } catch (err) {
    msgEl.textContent = "오류: " + (err?.toString() || "save failed");
    msgEl.className = "msg err";
  }
});
