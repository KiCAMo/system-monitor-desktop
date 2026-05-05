// history window logic.
const { invoke } = window.__TAURI__.core;
const { listen, emit } = window.__TAURI__.event;
const { getCurrentWebviewWindow } = window.__TAURI__.webviewWindow;

const $ = (id) => document.getElementById(id);
const me = getCurrentWebviewWindow();

me.onCloseRequested(async (event) => {
  event.preventDefault();
  await me.hide();
});

let _selectedHistoryId = null;

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
  }[c]));
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

$("historyHide").addEventListener("click", () => me.hide());
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
    await emit("history-mutated", { id: _selectedHistoryId, kind: "inject" });
    await emit("agent-injected", { id: _selectedHistoryId });
  } catch (_) {
    alert("재주입 실패: 에이전트 세션이 안 떠있을 수 있어요.");
  }
});

$("historyDel").addEventListener("click", async () => {
  if (!_selectedHistoryId) return;
  if (!confirm("이 매칭 기록을 삭제할까요?")) return;
  const id = _selectedHistoryId;
  await invoke("delete_history", { match_id: id });
  $("historyDetail").classList.add("hidden");
  _selectedHistoryId = null;
  await loadHistoryList();
  await emit("history-mutated", { id, kind: "delete" });
});

$("historyClear").addEventListener("click", async () => {
  if (!confirm("전체 매칭 히스토리를 모두 삭제할까요? 되돌릴 수 없습니다.")) return;
  await invoke("clear_history");
  $("historyDetail").classList.add("hidden");
  _selectedHistoryId = null;
  await loadHistoryList();
  await emit("history-mutated", { kind: "clear" });
});

// Refresh list whenever a new match comes in.
listen("match-detected", () => { loadHistoryList(); });

// Refresh node filter when config changes.
listen("config-changed", () => { populateHistoryNodeFilter(); });
listen("profile-loaded", () => { populateHistoryNodeFilter(); });

// Refresh on focus (covers re-show after hide).
me.onFocusChanged(async ({ payload: focused }) => {
  if (focused) {
    try {
      await populateHistoryNodeFilter();
      await loadHistoryList();
    } catch (_) {}
  }
});

(async () => {
  await populateHistoryNodeFilter();
  await loadHistoryList();
})();
