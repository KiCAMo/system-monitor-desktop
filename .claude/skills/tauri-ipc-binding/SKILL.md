---
name: tauri-ipc-binding
description: Tauri 2 의 #[tauri::command] + app_handle.emit + tauri::State 패턴을 적용해 Rust 백엔드와 webview 프런트엔드를 결합. FastAPI 라우트를 Tauri command 로, WebSocket 메시지를 Tauri event 로 전환할 때 반드시 사용. capability 권한 정렬과 invoke 인자 케이스 일치 규칙 포함.
---

# Tauri IPC Binding

FastAPI + WebSocket 기반 백엔드를 Tauri 의 invoke/emit 모델로 옮기는 패턴.

## 핵심 매핑

| FastAPI                                         | Tauri 2                                                                |
|-------------------------------------------------|-----------------------------------------------------------------------|
| `@app.get("/api/foo")` async fn                 | `#[tauri::command] async fn foo(...) -> Result<T, String>`             |
| `@app.post("/api/foo")` (body: dict)            | command 인자에 strict struct 받기 (serde Deserialize)                  |
| `app.state.s = AppState(...)`                   | `app.manage(AppState(...))` in `setup`                                 |
| 의존성 주입 `app_state: AppState`               | command 시그니처에 `state: tauri::State<'_, AppState>`                 |
| `Event` → WebSocket broadcast                   | broadcast::Receiver 에서 받아 `app_handle.emit(name, payload)`          |
| HTTPException(400, "msg")                       | `Err("msg".into())` (Result<_, String>)                                |
| 파일 다운로드 / 스트리밍                          | (이 도구에선 미사용 — emit 으로 청크 흘리기)                            |

## Command 시그니처 규칙

```rust
#[tauri::command]
async fn save_settings(
    state: tauri::State<'_, AppState>,
    config: AppConfig,             // serde Deserialize 자동
) -> Result<bool, String> {
    state.0.lock().await.reload(config).await
        .map_err(|e| e.to_string())
}
```

- 모든 command 는 `Result<T, String>` 반환 (에러 메시지를 frontend 가 그대로 표시).
- 인자 이름은 frontend invoke 의 args 키와 정확히 일치 (snake_case 그대로).
- `tauri::State<'_, AppState>` 는 항상 첫 인자.

## 프런트 호출 형태

```js
import { invoke } from "@tauri-apps/api/core";

// command 인자가 fn save_settings(state, config: AppConfig) 일 때:
await invoke("save_settings", { config: { ... } });
//                              ↑ key 가 백엔드 매개변수명과 일치
```

snake_case ↔ camelCase 자동 변환 안 됨. 백엔드 인자가 `match_id` 면 invoke 도 `{ match_id: 7 }`.

## Event Relay 패턴

setup 안에서 한 번만 spawn:

```rust
.setup(|app| {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let app_handle = app.handle().clone();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let _ = app_handle.emit(ev.tauri_event_name(), &ev);
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    let _ = app_handle.emit("console-msg",
                        Event::console("WARN", format!("event lag {n}"), None));
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    app.manage(AppState(Arc::new(Mutex::new(Inner::initial(bus)))));
    Ok(())
})
```

## Capability 정렬

`capabilities/default.json` 의 `permissions` 배열에 사용한 plugin 의 권한이 빠짐없이 있어야 한다. 누락 시 런타임 거부.

```json
{
  "permissions": [
    "core:default",
    "core:event:default",
    "dialog:default",
    "notification:default",
    "shell:default",
    "updater:default"
  ]
}
```

새 plugin 추가 시 capability 도 추가 — 잊으면 invoke 가 silently 실패한다.

## 함정

1. **`generate_handler!` 의 컴파일 에러** — 시그니처 불일치 90%. `Result<T, E>` 의 E 가 `Serialize + Display` 가 아니면 거부.
2. **`tauri::State<'_, AppState>` 의 lifetime** — async fn 안에서 사용할 땐 별 문제 없지만, command 가 sync 면 `'_` 가 안 맞을 수 있음.
3. **emit 시 `&Event` 인지 `Event` 인지** — `serde::Serialize` 만족하면 둘 다 OK.
4. **frontend listen 의 unlisten** — `const unlisten = await listen(name, fn)` 후 컴포넌트 unmount 시 `unlisten()` 호출. SPA 라이프사이클이 없는 단일 페이지에선 무시 가능.
