// Persistent SQLite store of error captures so the user can browse, search,
// and re-inject past matches even after the live console has scrolled them
// away. Ported 1:1 from app/history.py — same schema, same filters.

use anyhow::Result;
use chrono::{SecondsFormat, Utc};
use rusqlite::{params_from_iter, Connection};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS matches (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  ts TEXT NOT NULL,
  node TEXT NOT NULL,
  container TEXT,
  pattern TEXT,
  matched_line TEXT NOT NULL,
  block TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_matches_ts ON matches(ts DESC);
CREATE INDEX IF NOT EXISTS idx_matches_node ON matches(node);
"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchRow {
    pub id: i64,
    pub ts: String,
    pub node: String,
    pub container: Option<String>,
    pub pattern: Option<String>,
    pub matched_line: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<String>,
}

pub struct HistoryStore {
    pub path: String,
    write_lock: Mutex<()>,
}

impl HistoryStore {
    /// Open (or create) the sqlite database at `path`. Use `":memory:"` for
    /// tests. Schema is applied synchronously on construction — cheap and
    /// only happens once.
    pub fn new(path: impl Into<String>) -> Result<Self> {
        let path = path.into();
        let conn = Connection::open(&path)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            path,
            write_lock: Mutex::new(()),
        })
    }

    pub async fn record(
        &self,
        node: &str,
        container: Option<&str>,
        matched_line: &str,
        block: &str,
        pattern: Option<&str>,
    ) -> Result<i64> {
        let _g = self.write_lock.lock().await;
        let path = self.path.clone();
        let node = node.to_string();
        let container = container.map(|s| s.to_string());
        // Truncate matched_line to 500 chars (char-wise, matching Python's
        // string slicing semantics rather than byte-wise).
        let matched_line: String = matched_line.chars().take(500).collect();
        let block = block.to_string();
        let pattern = pattern.map(|s| s.to_string());
        let ts = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);

        tokio::task::spawn_blocking(move || -> Result<i64> {
            let conn = Connection::open(&path)?;
            conn.execute(
                "INSERT INTO matches(ts, node, container, pattern, matched_line, block) \
                 VALUES (?, ?, ?, ?, ?, ?)",
                rusqlite::params![ts, node, container, pattern, matched_line, block],
            )?;
            Ok(conn.last_insert_rowid())
        })
        .await?
    }

    pub async fn list(
        &self,
        limit: usize,
        node: Option<&str>,
        q: Option<&str>,
        before_id: Option<i64>,
    ) -> Result<Vec<MatchRow>> {
        let path = self.path.clone();
        let node = node.map(|s| s.to_string());
        let q = q.map(|s| s.to_string());
        let limit = limit.clamp(1, 500) as i64;

        tokio::task::spawn_blocking(move || -> Result<Vec<MatchRow>> {
            let mut sql = String::from(
                "SELECT id, ts, node, container, pattern, matched_line FROM matches WHERE 1=1",
            );
            let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
            if let Some(n) = node {
                sql.push_str(" AND node = ?");
                params.push(Box::new(n));
            }
            if let Some(qq) = q {
                sql.push_str(" AND (matched_line LIKE ? OR block LIKE ?)");
                let like = format!("%{}%", qq);
                params.push(Box::new(like.clone()));
                params.push(Box::new(like));
            }
            if let Some(bid) = before_id {
                sql.push_str(" AND id < ?");
                params.push(Box::new(bid));
            }
            sql.push_str(" ORDER BY id DESC LIMIT ?");
            params.push(Box::new(limit));

            let conn = Connection::open(&path)?;
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(
                    params_from_iter(params.iter().map(|b| b.as_ref())),
                    |row| {
                        Ok(MatchRow {
                            id: row.get(0)?,
                            ts: row.get(1)?,
                            node: row.get(2)?,
                            container: row.get(3)?,
                            pattern: row.get(4)?,
                            matched_line: row.get(5)?,
                            block: None,
                        })
                    },
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await?
    }

    pub async fn get(&self, match_id: i64) -> Result<Option<MatchRow>> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || -> Result<Option<MatchRow>> {
            let conn = Connection::open(&path)?;
            let mut stmt = conn.prepare(
                "SELECT id, ts, node, container, pattern, matched_line, block \
                 FROM matches WHERE id = ?",
            )?;
            let mut rows = stmt.query(rusqlite::params![match_id])?;
            if let Some(row) = rows.next()? {
                Ok(Some(MatchRow {
                    id: row.get(0)?,
                    ts: row.get(1)?,
                    node: row.get(2)?,
                    container: row.get(3)?,
                    pattern: row.get(4)?,
                    matched_line: row.get(5)?,
                    block: Some(row.get(6)?),
                }))
            } else {
                Ok(None)
            }
        })
        .await?
    }

    pub async fn delete(&self, match_id: i64) -> Result<()> {
        let _g = self.write_lock.lock().await;
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let conn = Connection::open(&path)?;
            conn.execute(
                "DELETE FROM matches WHERE id = ?",
                rusqlite::params![match_id],
            )?;
            Ok(())
        })
        .await?
    }

    pub async fn clear(&self) -> Result<usize> {
        let _g = self.write_lock.lock().await;
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || -> Result<usize> {
            let conn = Connection::open(&path)?;
            let n = conn.execute("DELETE FROM matches", [])?;
            Ok(n)
        })
        .await?
    }
}

impl Default for HistoryStore {
    fn default() -> Self {
        // Mirrors Python's default `path: str | Path = "history.db"`.
        Self::new("history.db").expect("failed to open default history.db")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // NOTE: `:memory:` opens a *fresh* DB on every `Connection::open` call.
    // Since each method opens a new connection, an in-memory DB would not
    // persist across calls. Use a tempfile path instead.
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn tmp_path() -> String {
        let dir = std::env::temp_dir();
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = dir.join(format!(
            "history_test_{}_{}_{}.db",
            std::process::id(),
            nanos,
            n
        ));
        let _ = std::fs::remove_file(&p);
        p.to_string_lossy().into_owned()
    }

    #[tokio::test]
    async fn record_then_list_returns_row() {
        let path = tmp_path();
        let store = HistoryStore::new(&path).unwrap();
        let id = store
            .record("nodeA", Some("ctr"), "boom: oops", "block-text", Some("err"))
            .await
            .unwrap();
        assert!(id > 0);

        let rows = store.list(100, None, None, None).await.unwrap();
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.id, id);
        assert_eq!(r.node, "nodeA");
        assert_eq!(r.container.as_deref(), Some("ctr"));
        assert_eq!(r.pattern.as_deref(), Some("err"));
        assert_eq!(r.matched_line, "boom: oops");
        // list does not populate block (mirrors Python SELECT column list).
        assert!(r.block.is_none());
        assert!(!r.ts.is_empty());

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn list_filter_by_node() {
        let path = tmp_path();
        let store = HistoryStore::new(&path).unwrap();
        store
            .record("nodeA", None, "line A", "blockA", None)
            .await
            .unwrap();
        store
            .record("nodeB", None, "line B", "blockB", None)
            .await
            .unwrap();
        store
            .record("nodeA", None, "line A2", "blockA2", None)
            .await
            .unwrap();

        let rows = store.list(100, Some("nodeA"), None, None).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.node == "nodeA"));

        let rows = store.list(100, Some("nodeB"), None, None).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].matched_line, "line B");

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn list_filter_by_q_matches_line_or_block() {
        let path = tmp_path();
        let store = HistoryStore::new(&path).unwrap();
        store
            .record("n", None, "panic at the disco", "irrelevant block", None)
            .await
            .unwrap();
        store
            .record("n", None, "totally fine line", "block has needle inside", None)
            .await
            .unwrap();
        store
            .record("n", None, "nothing matches", "or here", None)
            .await
            .unwrap();

        // matches via matched_line
        let rows = store.list(100, None, Some("panic"), None).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].matched_line, "panic at the disco");

        // matches via block (note: list still doesn't return block, but filter applies)
        let rows = store.list(100, None, Some("needle"), None).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].matched_line, "totally fine line");
        assert!(rows[0].block.is_none());

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn delete_and_clear() {
        let path = tmp_path();
        let store = HistoryStore::new(&path).unwrap();
        let id1 = store.record("n", None, "a", "ba", None).await.unwrap();
        let _id2 = store.record("n", None, "b", "bb", None).await.unwrap();
        let _id3 = store.record("n", None, "c", "bc", None).await.unwrap();

        store.delete(id1).await.unwrap();
        let rows = store.list(100, None, None, None).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.id != id1));

        let cleared = store.clear().await.unwrap();
        assert_eq!(cleared, 2);
        let rows = store.list(100, None, None, None).await.unwrap();
        assert!(rows.is_empty());

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn get_returns_block_list_does_not() {
        let path = tmp_path();
        let store = HistoryStore::new(&path).unwrap();
        let id = store
            .record("n", Some("c"), "line", "the full block", Some("p"))
            .await
            .unwrap();

        let got = store.get(id).await.unwrap().expect("row exists");
        assert_eq!(got.id, id);
        assert_eq!(got.block.as_deref(), Some("the full block"));
        assert_eq!(got.matched_line, "line");
        assert_eq!(got.container.as_deref(), Some("c"));
        assert_eq!(got.pattern.as_deref(), Some("p"));

        let rows = store.list(100, None, None, None).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].block.is_none(), "list must omit block");

        // Missing id returns None.
        assert!(store.get(999_999).await.unwrap().is_none());

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn matched_line_truncated_to_500_chars() {
        let path = tmp_path();
        let store = HistoryStore::new(&path).unwrap();
        let long = "x".repeat(1000);
        let id = store.record("n", None, &long, "b", None).await.unwrap();
        let got = store.get(id).await.unwrap().unwrap();
        assert_eq!(got.matched_line.chars().count(), 500);
        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn list_before_id_pagination() {
        let path = tmp_path();
        let store = HistoryStore::new(&path).unwrap();
        let ids: Vec<i64> = {
            let mut v = Vec::new();
            for i in 0..5 {
                v.push(
                    store
                        .record("n", None, &format!("line {i}"), "b", None)
                        .await
                        .unwrap(),
                );
            }
            v
        };
        // before_id = ids[3] should give us ids[0..3] in DESC order
        let rows = store
            .list(100, None, None, Some(ids[3]))
            .await
            .unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].id, ids[2]);
        assert_eq!(rows[2].id, ids[0]);
        std::fs::remove_file(&path).ok();
    }
}
