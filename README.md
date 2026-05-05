# system-monitor-desktop

Tauri 2 (Rust) 데스크톱 포트 — [system-management](https://github.com/KiCAMo/system-management) (Python+FastAPI) 의 후속.

EC2 위 docker 컨테이너 로그를 SSH 로 실시간 감시하다가 정규식 매칭 시 컨텍스트를 PTY로 띄운 claude/codex 입력창에 paste, OS 알림 + SQLite 매칭 히스토리.

## 차이점

| | 이전 (Python) | 현재 (Tauri) |
|---|---|---|
| 형태 | localhost FastAPI + 브라우저 | macOS 데스크톱 앱 |
| 백엔드 | asyncssh / pexpect / sqlite3 | russh / portable-pty / rusqlite |
| IPC | WebSocket + fetch | Tauri `invoke()` + `listen()` |
| 알림 | Browser Notification API | Native macOS notification |
| 자동 업데이트 | 없음 | tauri-plugin-updater + GitHub Releases |
| 트레이 / 자동 시작 | 없음 | (예정) macOS 메뉴바 트레이 |

## 첫 설치 (사용자)

1. [Releases](https://github.com/KiCAMo/system-monitor-desktop/releases) 페이지에서 최신 `.dmg` 다운로드.
2. `.dmg` 더블클릭 → 마운트 → `시스템 모니터링.app` 을 `/Applications` 폴더로 드래그.
3. **첫 실행만**: Finder 에서 앱을 **우클릭 → 열기 → "열기" 확인**. (Apple Developer 인증서 없이 운영하므로 Gatekeeper 우회 필요. 이후 실행은 더블클릭으로 정상.)
4. 이후 새 버전이 릴리즈되면 앱이 자동으로 업데이트 알림을 띄움.

## 개발 / 로컬 실행

```bash
# 1. 첫 셋업 — 의존성 컴파일
cd src-tauri && cargo build
cd ..

# 2. 개발 모드 (HMR + 변경 시 재시작)
cargo tauri dev

# 3. 릴리즈 빌드 (.dmg) — 로컬 검증용. 정식 배포는 GitHub Actions 사용.
cargo tauri build
```

> `cargo tauri dev` 로 띄운 앱은 자동 업데이트가 동작하지 않는다. 업데이터 동작 검증은 정식 `.dmg` 빌드로.

## 자동 업데이트 / 릴리즈

- `tauri-plugin-updater` 가 `https://github.com/KiCAMo/system-monitor-desktop/releases/latest/download/latest.json` 폴링.
- GitHub Actions (`.github/workflows/release.yml`) 가 `v*` 태그 푸시 시 빌드 + 서명 + `latest.json` 첨부된 릴리즈 자동 생성.
- 본인용이라 Apple Developer 인증서 없이 운영 (첫 설치 시 우클릭 → 열기로 우회).
- 1회성 셋업(서명키 생성, secret 등록, 첫 릴리즈) 가이드: **[docs/release-setup.md](docs/release-setup.md)**.

## 라이선스

본인 사용 한정 도구. 내부 운영용.
