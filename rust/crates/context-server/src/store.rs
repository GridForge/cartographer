//! Persistent context store backed by `SQLite` + FTS5.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};
use telemetry::hash_content;

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

const SCHEMA_SQL: &str = "
PRAGMA journal_mode=WAL;

CREATE TABLE IF NOT EXISTS context_entries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    content_hash TEXT NOT NULL,
    content TEXT NOT NULL,
    tag TEXT NOT NULL,
    files_referenced TEXT,
    created_at INTEGER NOT NULL
);

CREATE VIRTUAL TABLE IF NOT EXISTS context_fts USING fts5(
    content, tag,
    content='context_entries', content_rowid='id',
    tokenize='unicode61 remove_diacritics 0'
);

CREATE TRIGGER IF NOT EXISTS context_entries_ai AFTER INSERT ON context_entries BEGIN
    INSERT INTO context_fts(rowid, content, tag) VALUES (new.id, new.content, new.tag);
END;
CREATE TRIGGER IF NOT EXISTS context_entries_ad AFTER DELETE ON context_entries BEGIN
    INSERT INTO context_fts(context_fts, rowid, content, tag) VALUES('delete', old.id, old.content, old.tag);
END;
CREATE TRIGGER IF NOT EXISTS context_entries_au AFTER UPDATE ON context_entries BEGIN
    INSERT INTO context_fts(context_fts, rowid, content, tag) VALUES('delete', old.id, old.content, old.tag);
    INSERT INTO context_fts(rowid, content, tag) VALUES (new.id, new.content, new.tag);
END;

PRAGMA user_version = 1;
";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A single result from a context query, including its relevance score.
#[derive(Debug)]
pub struct ContextEntry {
    pub id: i64,
    pub content: String,
    pub tag: String,
    pub files_referenced: Option<String>,
    pub created_at: i64,
    pub score: f64,
}

// ---------------------------------------------------------------------------
// ContextStore
// ---------------------------------------------------------------------------

/// Persistent context store backed by `SQLite` with FTS5 full-text search.
///
/// Owns a `Connection` directly -- designed for a single-threaded runtime.
pub struct ContextStore {
    conn: Connection,
}

impl ContextStore {
    /// Open (or create) a store at the given filesystem path.
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    /// Open an in-memory store. Useful for tests and as a fallback when the
    /// filesystem path is not writable.
    pub fn open_in_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> rusqlite::Result<Self> {
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version < 1 {
            conn.execute_batch(SCHEMA_SQL)?;
        }
        Ok(Self { conn })
    }

    /// Store a context entry. Returns the new row id.
    pub fn store(
        &self,
        content: &str,
        tag: &str,
        files_referenced: Option<&str>,
    ) -> rusqlite::Result<i64> {
        let now = now_ms();
        self.store_with_timestamp_inner(content, tag, files_referenced, now)
    }

    fn store_with_timestamp_inner(
        &self,
        content: &str,
        tag: &str,
        files_referenced: Option<&str>,
        created_at: i64,
    ) -> rusqlite::Result<i64> {
        let content_hash = hash_content(content);
        self.conn.execute(
            "INSERT INTO context_entries (content_hash, content, tag, files_referenced, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![content_hash, content, tag, files_referenced, created_at],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Query the store using FTS5 full-text search with recency scoring.
    pub fn query(
        &self,
        query: &str,
        tag_filter: Option<&str>,
        limit: usize,
    ) -> rusqlite::Result<Vec<ContextEntry>> {
        let now = now_ms();
        self.query_with_now(query, tag_filter, limit, now)
    }

    fn query_with_now(
        &self,
        query: &str,
        tag_filter: Option<&str>,
        limit: usize,
        now_ms: i64,
    ) -> rusqlite::Result<Vec<ContextEntry>> {
        let sanitized = sanitize_fts5_query(query);
        if sanitized.is_empty() {
            return Ok(vec![]);
        }

        // Fetch 4x limit candidates from FTS5
        let fetch_limit = limit.saturating_mul(4).max(1);

        let (sql, do_tag_filter) = if tag_filter.is_some() {
            (
                "SELECT e.id, e.content, e.tag, e.files_referenced, e.created_at, f.rank
                 FROM context_fts f
                 JOIN context_entries e ON e.id = f.rowid
                 WHERE context_fts MATCH ?1 AND e.tag = ?2
                 LIMIT ?3",
                true,
            )
        } else {
            (
                "SELECT e.id, e.content, e.tag, e.files_referenced, e.created_at, f.rank
                 FROM context_fts f
                 JOIN context_entries e ON e.id = f.rowid
                 WHERE context_fts MATCH ?1
                 LIMIT ?2",
                false,
            )
        };

        let mut stmt = self.conn.prepare(sql)?;

        #[allow(clippy::cast_possible_wrap)]
        let fetch_limit_i64 = fetch_limit as i64;

        let map_row = |row: &rusqlite::Row<'_>| -> rusqlite::Result<RawRow> {
            Ok(RawRow {
                id: row.get(0)?,
                content: row.get(1)?,
                tag: row.get(2)?,
                files_referenced: row.get(3)?,
                created_at: row.get(4)?,
                rank: row.get(5)?,
            })
        };

        let raw_rows: Vec<RawRow> = if do_tag_filter {
            let tag = tag_filter.unwrap_or("");
            stmt.query_map(params![sanitized, tag, fetch_limit_i64], map_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        } else {
            stmt.query_map(params![sanitized, fetch_limit_i64], map_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };

        let mut entries: Vec<ContextEntry> = Vec::new();
        for row in raw_rows {
            let fts_relevance = (-row.rank).min(30.0) / 30.0;
            #[allow(clippy::cast_precision_loss)]
            let age_hours = (now_ms - row.created_at) as f64 / 3_600_000.0;
            let recency = 1.0 / (1.0 + age_hours / 24.0);
            let combined = fts_relevance * 0.7 + recency * 0.3;

            entries.push(ContextEntry {
                id: row.id,
                content: row.content,
                tag: row.tag,
                files_referenced: row.files_referenced,
                created_at: row.created_at,
                score: combined,
            });
        }

        entries.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        entries.truncate(limit);

        Ok(entries)
    }

    /// Return the total number of entries in the store.
    pub fn entry_count(&self) -> rusqlite::Result<i64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM context_entries", [], |row| row.get(0))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

struct RawRow {
    id: i64,
    content: String,
    tag: String,
    files_referenced: Option<String>,
    created_at: i64,
    rank: f64,
}

/// Sanitize a raw query string for FTS5 by wrapping each whitespace-delimited
/// token in double quotes. This prevents FTS5 operator injection (AND, OR, NOT,
/// NEAR, etc.) while still allowing multi-word matching.
fn sanitize_fts5_query(query: &str) -> String {
    query
        .split_whitespace()
        .map(|token| {
            let clean: String = token.chars().filter(|&c| c != '"').collect();
            if clean.is_empty() {
                String::new()
            } else {
                format!("\"{clean}\"")
            }
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[allow(clippy::cast_possible_truncation)]
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
impl ContextStore {
    /// Store a context entry with an explicit timestamp (for deterministic tests).
    pub fn store_with_timestamp(
        &self,
        content: &str,
        tag: &str,
        files_referenced: Option<&str>,
        created_at: i64,
    ) -> rusqlite::Result<i64> {
        self.store_with_timestamp_inner(content, tag, files_referenced, created_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_store() -> ContextStore {
        ContextStore::open_in_memory().expect("in-memory store should open")
    }

    #[test]
    fn store_and_query_round_trip() {
        let store = mem_store();
        store
            .store("Decided to use SQLite FTS5", "decision", None)
            .unwrap();

        let results = store.query("SQLite", None, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tag, "decision");
        assert!(results[0].content.contains("SQLite FTS5"));
    }

    #[test]
    fn query_with_tag_filter() {
        let store = mem_store();
        store.store("error in parsing", "error", None).unwrap();
        store
            .store("decided on parsing strategy", "decision", None)
            .unwrap();

        let all = store.query("parsing", None, 10).unwrap();
        assert_eq!(all.len(), 2);

        let decisions = store.query("parsing", Some("decision"), 10).unwrap();
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].tag, "decision");
    }

    #[test]
    fn empty_query_returns_empty() {
        let store = mem_store();
        store.store("some content", "note", None).unwrap();

        let results = store.query("xyznonexistenttokenxyz", None, 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn recency_bias_with_explicit_timestamps() {
        let store = mem_store();
        let now = 1_700_000_000_000i64; // fixed "now"
        let one_day_ago = now - 86_400_000;
        let one_hour_ago = now - 3_600_000;

        store
            .store_with_timestamp("context about rust patterns", "note", None, one_day_ago)
            .unwrap();
        store
            .store_with_timestamp("context about rust macros", "note", None, one_hour_ago)
            .unwrap();

        let results = store.query_with_now("rust", None, 10, now).unwrap();

        assert_eq!(results.len(), 2);
        // The more recent entry should score higher due to recency bias
        assert!(
            results[0].created_at == one_hour_ago,
            "more recent entry should rank first"
        );
        assert!(
            results[0].score >= results[1].score,
            "recent entry score ({}) should be >= old entry score ({})",
            results[0].score,
            results[1].score,
        );
    }

    #[test]
    fn fts5_injection_is_sanitized() {
        let store = mem_store();
        store.store("normal content", "note", None).unwrap();

        // These FTS5 operators should not cause SQL errors
        let dangerous_queries = [
            "AND OR NOT",
            "NEAR(test, 5)",
            "test OR DROP TABLE",
            "\"already quoted\"",
            "*",
        ];
        for q in dangerous_queries {
            let result = store.query(q, None, 10);
            assert!(result.is_ok(), "query '{q}' should not error: {result:?}");
        }
    }

    #[test]
    fn entry_count_works() {
        let store = mem_store();
        assert_eq!(store.entry_count().unwrap(), 0);

        store.store("first", "note", None).unwrap();
        store.store("second", "error", None).unwrap();
        assert_eq!(store.entry_count().unwrap(), 2);
    }

    #[test]
    fn schema_version_is_set() {
        let store = mem_store();
        let version: i64 = store
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 1);
    }

    #[test]
    fn double_open_does_not_fail() {
        // Opening in-memory twice should work (idempotent init)
        let _store1 = mem_store();
        let _store2 = mem_store();

        // File-backed: open the same path twice
        let dir = std::env::temp_dir().join("cartographer_test_double_open.db");
        let _ = std::fs::remove_file(&dir);
        let s1 = ContextStore::open(&dir).expect("first open");
        s1.store("test", "note", None).unwrap();
        drop(s1);

        let s2 = ContextStore::open(&dir).expect("second open should not fail");
        assert_eq!(s2.entry_count().unwrap(), 1);
        let _ = std::fs::remove_file(&dir);
    }
}
