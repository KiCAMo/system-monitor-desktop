# 릴리즈 셋업 가이드

`system-monitor-desktop` 의 자동 업데이트 + GitHub Releases 파이프라인 1회성 셋업 안내.

---

## 1. Tauri Signer 키페어 생성

자동 업데이트 매니페스트(`latest.json`) 와 번들에 서명할 Ed25519 키페어가 필요하다.

```bash
# tauri-cli 설치 (한 번만)
cargo install tauri-cli --version "^2.0"

# 키페어 생성 (~/.tauri/system-monitor.key 에 저장)
mkdir -p ~/.tauri
cargo tauri signer generate -w ~/.tauri/system-monitor.key
```

실행 시:
- 패스워드를 묻는다 (빈 값 가능하나, 권장: 강한 패스워드 입력 후 1Password 등에 저장).
- stdout 으로 **public key** (base64) 와 **private key** (base64, `-----BEGIN ...` 헤더 포함) 두 가지가 출력된다.
- 동일 내용이 `~/.tauri/system-monitor.key` (private) 와 `~/.tauri/system-monitor.key.pub` (public) 로도 저장된다.

> 분실 시 모든 기존 사용자가 자동 업데이트를 받을 수 없게 된다. 안전한 곳에 백업할 것.

---

## 2. 키 등록 위치

### 2-1. Public Key → 리포 커밋

`src-tauri/tauri.conf.json` 의 `plugins.updater.pubkey` 자리표시자를 실제 public key 로 교체:

```jsonc
"plugins": {
  "updater": {
    "active": true,
    "endpoints": ["https://github.com/KiCAMo/system-monitor-desktop/releases/latest/download/latest.json"],
    "pubkey": "REPLACE_WITH_TAURI_SIGNER_PUBKEY"   // ← 여기에 base64 public key 붙여넣기
  }
}
```

이 파일은 그대로 커밋해도 된다 (public key 는 비밀이 아님).

### 2-2. Private Key → GitHub Secrets

리포 페이지 → **Settings** → **Secrets and variables** → **Actions** → **New repository secret** 두 개 등록:

| Secret 이름 | 값 |
|---|---|
| `TAURI_SIGNING_PRIVATE_KEY` | private key 의 base64 전문 (`-----BEGIN ...` 부터 `-----END ...` 까지 줄바꿈 포함 그대로) |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | 키 생성 시 입력한 패스워드 (없으면 빈 문자열) |

---

## 3. 첫 릴리즈

```bash
# tauri.conf.json 의 version 을 0.1.0 으로 두고
git add src-tauri/tauri.conf.json
git commit -m "chore: set tauri signer pubkey"
git push origin main

git tag v0.1.0
git push origin v0.1.0
```

→ 리포 **Actions** 탭에서 `release` workflow → `build-tauri` job 진행 확인.
→ 완료 시 **Releases** 페이지에 `v0.1.0` 릴리즈가 생성되며 다음 자산이 첨부됨:
- `시스템 모니터링_0.1.0_aarch64.dmg` (또는 x64, 빌더 환경에 따라)
- `시스템 모니터링.app.tar.gz` + `.sig` (자동 업데이트용)
- `latest.json` (업데이터 매니페스트)

---

## 4. 자동 업데이트 동작 검증

- **첫 사용자 설치**: `.dmg` 다운로드 → 마운트 → `시스템 모니터링.app` 을 `/Applications` 로 드래그.
  - Apple Developer 인증서가 없으므로 **Gatekeeper** 가 차단함.
  - **우클릭 → 열기 → "열기" 확인**. 첫 실행만 이렇게, 이후는 더블클릭으로 정상 실행.
- **개발 빌드는 자동 업데이트 동작 안 함**: `cargo tauri dev` 로 띄운 앱은 production 엔드포인트를 폴링하지 않는다. 정식 `.dmg` 빌드로 검증할 것.
- **새 버전 릴리즈 절차**:
  1. `src-tauri/tauri.conf.json` 의 `version` 을 `0.1.1` 등으로 갱신
  2. commit + push
  3. `git tag v0.1.1 && git push origin v0.1.1`
  4. 사용자 앱이 다음 시작 시 (또는 폴링 주기에 따라) 업데이트 알림 수신 → 적용 → 재시작

---

## 5. 트러블슈팅

| 증상 | 원인 / 조치 |
|---|---|
| GitHub Actions 에서 `signer not found` / `private key not set` | `TAURI_SIGNING_PRIVATE_KEY` secret 미등록 또는 오타. 등록 후 재트리거. |
| 클라이언트에서 `signature mismatch` / `failed to verify signature` | `tauri.conf.json` 의 `pubkey` 가 잘못 들어갔거나 secret 의 private key 와 pair 불일치. 키페어 재생성 후 둘 다 재등록. |
| 사용자가 `.app` 실행 시 "확인되지 않은 개발자" 차단 | Apple Developer 인증서 없으므로 정상. **우클릭 → 열기** 절차 안내. 첫 실행만 필요. |
| `latest.json` 이 릴리즈에 첨부되지 않음 | tauri-action 이 `bundle.createUpdaterArtifacts` 옵션 또는 `plugins.updater` 설정을 인식 못 한 것. `tauri.conf.json` 의 updater 블록 활성화 상태 확인. |
| 아이콘이 placeholder 인 채로 빌드됨 | 1024x1024 PNG 준비 후 `cargo tauri icon path/to/icon.png` 한 번 실행 → `src-tauri/icons/` 자동 생성/교체 → commit. |

---

## 참고

- Tauri Updater 공식 문서: https://v2.tauri.app/plugin/updater/
- tauri-action: https://github.com/tauri-apps/tauri-action
- 본 리포는 macOS 전용. Windows/Linux 빌더는 워크플로에 포함되지 않음.
