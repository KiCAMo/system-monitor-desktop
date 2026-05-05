// settings window logic.
const { invoke } = window.__TAURI__.core;
const { emit } = window.__TAURI__.event;
const { getCurrentWebviewWindow } = window.__TAURI__.webviewWindow;

const $ = (id) => document.getElementById(id);
const form = $("settingsForm");
const msgEl = $("settingsMsgFoot");

// Hide rather than close so reopening is instant.
const me = getCurrentWebviewWindow();
me.onCloseRequested(async (event) => {
  event.preventDefault();
  await me.hide();
});

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

// ---------- Path picker (in-page modal overlay) ----------
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

async function loadInitial() {
  msgEl.textContent = ""; msgEl.className = "msg";
  const cfg = await invoke("get_settings");
  fillForm(cfg);
  await refreshProfiles();
}

// Refresh on every show (window stays alive after first show).
me.onFocusChanged(async ({ payload: focused }) => {
  if (focused) {
    try { await loadInitial(); } catch (_) {}
  }
});

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
  await emit("config-changed", {});
  await emit("profile-loaded", { name });
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

$("settingsCancel").addEventListener("click", () => me.hide());

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
    await emit("config-changed", {});
    setTimeout(() => me.hide(), 800);
  } catch (err) {
    msgEl.textContent = "오류: " + (err?.toString() || "save failed");
    msgEl.className = "msg err";
  }
});

loadInitial();
