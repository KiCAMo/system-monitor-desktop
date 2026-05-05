---
name: rust-port
description: Python 모듈을 Rust 로 1:1 안전 포팅한다. Pydantic 모델·asyncio 동시성·외부 시스템 통합(asyncssh/pexpect/sqlite3/httpx) 을 serde·tokio·russh·portable-pty·rusqlite·reqwest 로 변환. ssh_monitor.py / pty_session.py / metrics.py / history.py / state.py 를 Rust 로 옮길 때 반드시 사용. 동시성 매핑표·동작 등가성 보존·단위 테스트 의무 사이클을 강제한다.
---

# Rust Port — Python→Rust 안전 포팅 절차

Python 도구의 의미를 Rust 에 이식하면서 잃지 않기 위한 체크리스트. **동작 등가성 > 코드 미학.**

## 사이클

```
1. Read    Python 원본 끝까지 — 동작/엣지케이스 100% 이해
2. Map     Pydantic 모델 → serde struct, asyncio → tokio 변환표 적용
3. Write   src-tauri/src/<module>.rs + 단위 테스트 (3개 이상)
4. Check   cargo check --tests 통과
5. Test    cargo test --lib 통과
6. Wire    src-tauri/src/lib.rs 에 pub mod 추가
7. Commit  모듈 1개 = 커밋 1개
```

매 단계 통과 못 하면 다음 단계로 넘어가지 않는다.

## 변환표 (필수)

| Python                               | Rust                                                              |
|--------------------------------------|-------------------------------------------------------------------|
| `pydantic.BaseModel`                 | `#[derive(Debug, Clone, Serialize, Deserialize)] struct`          |
| `Field(default_factory=lambda: ...)` | `#[serde(default = "fn_returning_default")]`                      |
| `str \| None`                        | `Option<String>`                                                  |
| `list[T]`                            | `Vec<T>`                                                          |
| `int / float / bool`                 | `i64`/`u64`/`f64`/`bool` (의미에 맞게)                            |
| `dict[str, Any]`                     | `serde_json::Value` 또는 strict struct                            |
| `Path("~/...").expanduser()`         | `shellexpand::tilde(s).into_owned()`                              |
| `yaml.safe_load(text)`               | `serde_yaml::from_str(&text)?`                                    |
| `asyncio.Lock`                       | `tokio::sync::Mutex<()>`                                          |
| `asyncio.Event`                      | `tokio::sync::Notify` (notify_one + notified().await)             |
| `asyncio.Event` for stop             | `tokio_util::sync::CancellationToken` (cancel + cancelled().await) |
| `asyncio.Queue` per-subscriber       | `tokio::sync::broadcast::Receiver` (lagged 시 drop-oldest)        |
| `asyncio.create_task(coro)`          | `tokio::spawn(future)` → `JoinHandle<T>`                          |
| `asyncio.to_thread(blocking_fn)`     | `tokio::task::spawn_blocking(\|\| blocking_fn())`                 |
| `asyncio.gather(*tasks)`             | `tokio::join!(...)` 또는 `futures::future::join_all`              |
| `asyncio.sleep(s)`                   | `tokio::time::sleep(Duration::from_secs(s))`                      |
| `asyncio.wait_for(coro, timeout=N)`  | `tokio::time::timeout(Duration::from_secs(N), coro).await`        |
| `asyncssh.connect(...)`              | `russh::client::connect(...)`                                     |
| `pexpect.spawn(cmd, args, env, dimensions)` | `portable_pty::PtySize { rows, cols, ... }` + `CommandBuilder` |
| `pexpect.read_nonblocking(N, timeout=t)` | `AsyncFd<RawFd>::readable().await` + `try_read` until WouldBlock |
| `httpx.AsyncClient().get(url)`       | `reqwest::Client::new().get(url).send().await`                    |
| `sqlite3.connect(path)`              | `rusqlite::Connection::open(path)?` (spawn_blocking 안에서)       |
| `re.compile(p, re.IGNORECASE)`       | `regex::RegexBuilder::new(p).case_insensitive(true).build()?`     |

## 단위 테스트 의무

각 모듈에 최소 3개. 각 테스트는 한 가지만 검증.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_yaml_preserves_fields() { ... }

    #[tokio::test]
    async fn broadcast_publish_with_no_subscribers_does_not_panic() { ... }

    #[test]
    fn legacy_format_migrates_to_new_shape() { ... }
}
```

외부 시스템(SSH/PTY/HTTP) 통합 모듈도 가능한 범위에서 테스트:
- ssh_monitor: 라인 매칭 / before-after 윈도우 / 쿨다운 로직은 SSH 없이 단독 테스트 가능
- pty_session: env 정리 / bracketed paste 마커 strip 로직 / inject_pending 토글은 PTY 없이 테스트
- history: in-memory sqlite (`:memory:`) 사용

## 등가성 보존 체크포인트

포팅 후 반드시 자문:
- [ ] Python 의 모든 default 값이 Rust 에 그대로 있는가?
- [ ] Python 의 None 가능 필드가 Rust 에서 `Option<T>` 인가?
- [ ] Python 의 `try/except` 가 Rust 에서 `Result<T, E>` + `?` 로 동등 처리됐는가?
- [ ] async 흐름이 동일하게 동작하는가? (특히 `asyncio.Lock` 의 fairness, `asyncio.Event` 의 일회성 vs `Notify` 의 multi-trigger 차이 주의)
- [ ] 배경 태스크가 stop_all 시 정상 cancel 되는가?

## 자주 빠뜨리는 함정

1. **`tokio::sync::Mutex` vs `std::sync::Mutex`** — async 컨텍스트에서 `.await` 가 lock 보유 중 일어나면 후자는 deadlock. 반드시 `tokio::sync::Mutex` 사용.
2. **`broadcast::Sender::send` 가 Err 반환 시** — 구독자가 0명일 때 정상. silently 무시 OK.
3. **`broadcast::Receiver::recv` 의 `Lagged(n)`** — 무시하지 말고 콘솔 이벤트로 변환해서 사용자에게 표시.
4. **PTY 의 multi-byte 한글** — `read_nonblocking` 이 한 글자 중간에서 잘릴 수 있음. UTF-8 boundary 고려해서 chunk 누적 후 emit.
5. **rusqlite 의 sync 호출을 async 함수에서 직접 부르면** — runtime 멈춤. 반드시 `spawn_blocking` 안에.

## 이미 포팅된 의존성을 활용

```rust
use crate::config::{AppConfig, Ec2Config, NodeConfig, ...};
use crate::events::{Event, EventBus, EventPayload};
use crate::history::HistoryStore;  // (포팅 후)
```

이들의 시그니처가 안정화되어 있으니 그대로 호출. 새 시그니처 필요하면 SendMessage 로 해당 에이전트(rust-storage-porter 등) 에 협의 요청.
