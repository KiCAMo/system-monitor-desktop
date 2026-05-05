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

## 빌드

```bash
# 첫 셋업
cd src-tauri && cargo build
cd ..

# 개발 모드 (변경 시 재시작)
./.venv/bin/cargo tauri dev   # 또는 cargo tauri dev — 설치되면

# 릴리즈 빌드 (.dmg)
cargo tauri build
```

## 자동 업데이트

- `tauri-plugin-updater` 가 `https://github.com/KiCAMo/system-monitor-desktop/releases/latest/download/latest.json` 폴링
- GitHub Actions 가 태그 푸시 시 빌드 + 서명 + `latest.json` 갱신
- 본인용이라 Apple Developer 인증서 없이 운영 (첫 설치 시 우클릭 → 열기로 우회)

## 라이선스

본인 사용 한정 도구. 내부 운영용.
