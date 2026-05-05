# system-monitor-desktop

Tauri 2 + Rust 데스크톱 포트 — system-management (Python+FastAPI) 의 macOS 데스크톱 후속.

## 하네스: Python→Rust+Tauri 포팅

**목표:** EC2 docker 로그 모니터링 + claude/codex PTY 주입 + 자동 업데이트 데스크톱 앱을 Python 도구와 1:1 동등성으로 안전하게 포팅한다.

**트리거:** 다음 요청에서 `monitor-rewrite-orchestrator` 스킬을 사용한다 — "포팅 진행", "다음 모듈", "Rust 변환 작업", "ssh_monitor 옮겨줘", "tauri 마저 끝내줘", "재실행", "이어서 진행", "다시 실행". 단순 질문(스택 설명, 모듈 위치 등)은 직접 응답.

**팀 구성:**
- `rust-systems-porter` — ssh_monitor / metrics / pty_session
- `rust-storage-porter` — history / state
- `tauri-ipc-binder` — commands/* / lib.rs invoke_handler / capabilities
- `frontend-migrator` — dist/* (fetch→invoke, WebSocket→listen)
- `release-engineer` — GitHub Actions + Tauri signer
- `rust-qa` — 매 모듈 완료 직후 incremental 검증

**모드:** 에이전트 팀 (TeamCreate + SendMessage + TaskCreate). Phase 별 모드 동일.

**진행 상황:**
- ✅ 스캐폴드 (cargo check 통과)
- ✅ src/config.rs (8 단위 테스트)
- ✅ src/events.rs (3 단위 테스트)
- ⬜ src/ssh_monitor.rs / metrics.rs / pty_session.rs / history.rs / state.rs / commands/* / dist/* / .github/workflows/release.yml

**변경 이력:**

| 날짜 | 변경 내용 | 대상 | 사유 |
|------|----------|------|------|
| 2026-05-05 | 초기 하네스 구성 (6 에이전트 + 3 스킬) | 전체 | Tauri 포팅 전체 흐름 조율 필요 |
