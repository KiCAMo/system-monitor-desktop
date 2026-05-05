---
name: monitor-rewrite-orchestrator
description: system-monitor-desktop (Tauri+Rust) 의 Python→Rust 포팅 작업 전체를 조율한다. ssh_monitor, metrics, pty_session, history, state, commands, frontend, release 단계를 6 명의 전문 에이전트에 분배하고 각 단계 완료 직후 rust-qa 가 검증하도록 강제. 트리거: "포팅 진행", "다음 모듈", "Rust 변환 작업", "ssh_monitor 옮겨줘", "tauri 마저 끝내줘", "재실행", "이어서 진행", "다시 실행".
---

# Monitor Rewrite Orchestrator

`system-monitor-desktop` 의 Python→Rust+Tauri 포팅 흐름 조율. 6 명의 전문 에이전트(rust-systems-porter, rust-storage-porter, tauri-ipc-binder, frontend-migrator, release-engineer, rust-qa) 를 통해 모듈별 안전 포팅을 수행한다.

## Phase 0: 컨텍스트 확인

작업 시작 전에 현재 진행 상태를 파악한다.

1. `src-tauri/src/` 안에 있는 모듈 파일 나열
2. 각 모듈의 cargo test 통과 여부 확인 (`cargo test --lib` 결과)
3. 다음 작업할 모듈 결정 — 의존성 그래프 따라:
   - config / events       (의존성 없음 — 가장 먼저)
   - history               (config, events 에 의존)
   - ssh_monitor / metrics (config, events, history 에 의존)
   - pty_session           (config, events 에 의존)
   - state                 (위 모든 모듈에 의존)
   - commands/*            (state 에 의존)
   - frontend (dist/)      (commands 시그니처 표 필요)
   - release (.github/workflows) (위 모든 게 dev build 통과 후)

상태 별 분기:
- 초기 실행: config 부터 시작
- 부분 완료: 다음 미완료 모듈부터
- 사용자가 특정 모듈 수정 요청: 그 모듈만 재포팅 → rust-qa 검증

## Phase 1: 작업 분배

각 모듈을 담당 에이전트에 할당:

| 모듈                 | 담당 에이전트            |
|---------------------|-------------------------|
| ssh_monitor.rs      | rust-systems-porter     |
| metrics.rs          | rust-systems-porter     |
| pty_session.rs      | rust-systems-porter     |
| history.rs          | rust-storage-porter     |
| state.rs            | rust-storage-porter     |
| commands/*          | tauri-ipc-binder        |
| lib.rs (invoke_handler) | tauri-ipc-binder    |
| dist/* migration    | frontend-migrator       |
| .github/workflows/release.yml | release-engineer |
| QA 검증             | rust-qa (모든 모듈 후)  |

병렬 가능 조합 (의존성 없음):
- history.rs + ssh_monitor.rs + pty_session.rs (다 같이 시작 가능)
- metrics.rs 는 ssh_monitor 의 _ssh_connect_kwargs 를 참조하므로 그 다음

## Phase 2: 단일 모듈 작업 사이클

각 모듈마다 다음 사이클 강제:

```
1. 오케스트레이터 → 담당 에이전트:
   "X 모듈 포팅. 의존성: [...]. 출력 위치: src-tauri/src/X.rs.
    Python 원본: /Users/admin/Documents/system-management/app/X.py.
    rust-port 스킬 사이클 따를 것. 완료 보고 시 cargo test 결과 포함."

2. 담당 에이전트 작업 (rust-port 스킬 사이클 따름)
   - Read python 원본
   - Map 변환표 적용
   - Write 코드 + 단위 테스트
   - cargo check --tests
   - cargo test --lib

3. 담당 에이전트 → 오케스트레이터 + rust-qa:
   "X 완료. cargo test: M passed / N failed. 시그니처 [...]"

4. rust-qa → 검증:
   - cargo check / test / clippy 재확인
   - cross-module shape 검증 (다른 모듈에서 X 의 시그니처 사용 시)
   - 결과 보고

5. 오케스트레이터:
   - rust-qa OK → git commit (모듈 1개 = 커밋 1개) → 다음 모듈
   - rust-qa FAIL → 담당 에이전트에 수정 요청 → 2 단계로 돌아감
```

## Phase 3: 통합 검증

모든 모듈 + commands + frontend 포팅 완료 후:

1. `cargo tauri dev` 가 에러 없이 띄워지는지 확인
2. webview 가 백엔드와 통신 (invoke 결과 + listen 이벤트 둘 다 동작)
3. 시작/중단 / 설정 저장 / 프로필 전환 / 히스토리 조회 / 재주입 흐름 수동 확인
4. release-engineer 에 GitHub Actions 워크플로우 작성 요청

## Phase 4: 릴리즈 준비

release-engineer 가 다음을 수행:
- `.github/workflows/release.yml` 작성
- `docs/release-setup.md` (Tauri signer 키 생성 + GitHub Secret 등록 가이드)
- pubkey 자리표시자 → 사용자가 키 생성 후 직접 채우도록 안내
- 첫 릴리즈 절차 정리 (`git tag v0.1.0 && git push --tags`)

## 데이터 전달 프로토콜

- **태스크 기반:** TaskCreate 로 모듈별 작업 의존성 표현, TaskUpdate 로 진행 추적.
- **메시지 기반:** SendMessage 로 인터페이스 시그니처 합의 / QA 결과 피드백.
- **파일 기반:** 모든 산출물은 `src-tauri/src/` 또는 `dist/` 의 실제 파일.
- **반환값 기반:** 사용 안 함 (팀 모드).

## 에러 핸들링

| 에러                              | 대응                                                           |
|----------------------------------|---------------------------------------------------------------|
| cargo check 실패                  | 담당 에이전트가 1회 자체 수정 시도 → 재실패 시 오케스트레이터에 보고 |
| cargo test 실패                   | rust-qa 가 정확한 실패 케이스 + 의심 원인을 담당 에이전트에 전달 |
| cross-module shape mismatch       | rust-qa 가 양 에이전트(보낸 측 / 받는 측) 에 동시 알림 → 합의 후 양쪽 수정 |
| 외부 라이브러리 API 변화          | 담당 에이전트가 docs.rs 확인 + 새 시그니처로 갱신, 영향받는 다른 모듈 동기 갱신 |
| `cargo tauri dev` 가 webview 에러 | tauri-ipc-binder + frontend-migrator 양쪽이 함께 console 로그 분석 |

## 테스트 시나리오

**시나리오 1 — 정상 흐름 (config 까지 완료된 상태에서 history 포팅 요청):**
1. 오케스트레이터 → rust-storage-porter: "history.rs 포팅"
2. rust-storage-porter: rust-port 사이클 → cargo test 3 passed → SendMessage 보고
3. rust-qa: 검증 OK → 오케스트레이터에 alert
4. 오케스트레이터: git commit "Port history to rusqlite" → 다음 모듈 (state.rs)

**시나리오 2 — cross-module shape 어긋남:**
1. rust-systems-porter 가 ssh_monitor 에서 `history.record(node, container, line, block, pattern)` 호출
2. rust-storage-porter 가 history 의 record 시그니처를 `record(node, container, pattern, matched_line, block)` 으로 정의 (인자 순서 다름)
3. cargo check 실패 → rust-qa 가 양쪽에 동시 보고
4. 두 에이전트가 SendMessage 로 합의 → rust-storage-porter 가 시그니처 수정 → 다시 검증
