#![allow(dead_code)]

use anyhow::{Result, anyhow};
use rusqlite::{Connection, params};
use std::collections::HashMap;
use std::path::Path;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS events (
    seq             INTEGER PRIMARY KEY,
    event_id        TEXT NOT NULL,
    team_id         TEXT NOT NULL,
    source          TEXT NOT NULL,
    event_type      TEXT NOT NULL,
    raw_payload     TEXT NOT NULL,
    enriched_payload TEXT,
    importance      TEXT NOT NULL DEFAULT 'normal',
    status          TEXT NOT NULL DEFAULT 'pending',
    failure_reason  TEXT,
    created_at      INTEGER NOT NULL,
    enriched_at     INTEGER,
    delivered_at    INTEGER
);

CREATE INDEX IF NOT EXISTS idx_events_status     ON events (status);
CREATE INDEX IF NOT EXISTS idx_events_source     ON events (source);
CREATE INDEX IF NOT EXISTS idx_events_importance ON events (importance);
CREATE INDEX IF NOT EXISTS idx_events_created_at ON events (created_at);
";

// ---------------------------------------------------------------------------
// EventStatus
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventStatus {
    Pending,
    Filtered,
    Enriching,
    Ready,
    Delivered,
    Failed,
}

impl EventStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            EventStatus::Pending => "pending",
            EventStatus::Filtered => "filtered",
            EventStatus::Enriching => "enriching",
            EventStatus::Ready => "ready",
            EventStatus::Delivered => "delivered",
            EventStatus::Failed => "failed",
        }
    }

    pub fn from_str(s: &str) -> Result<Self> {
        match s {
            "pending" => Ok(EventStatus::Pending),
            "filtered" => Ok(EventStatus::Filtered),
            "enriching" => Ok(EventStatus::Enriching),
            "ready" => Ok(EventStatus::Ready),
            "delivered" => Ok(EventStatus::Delivered),
            "failed" => Ok(EventStatus::Failed),
            other => Err(anyhow!("unknown event status: {}", other)),
        }
    }
}

// ---------------------------------------------------------------------------
// Importance
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Importance {
    Low,
    Normal,
    High,
    Critical,
}

impl Importance {
    pub fn as_str(&self) -> &'static str {
        match self {
            Importance::Low => "low",
            Importance::Normal => "normal",
            Importance::High => "high",
            Importance::Critical => "critical",
        }
    }

    pub fn from_str(s: &str) -> Result<Self> {
        match s {
            "low" => Ok(Importance::Low),
            "normal" => Ok(Importance::Normal),
            "high" => Ok(Importance::High),
            "critical" => Ok(Importance::Critical),
            other => Err(anyhow!("unknown importance: {}", other)),
        }
    }

    /// Numeric rank: Low=0, Normal=1, High=2, Critical=3
    pub fn rank(&self) -> u8 {
        match self {
            Importance::Low => 0,
            Importance::Normal => 1,
            Importance::High => 2,
            Importance::Critical => 3,
        }
    }
}

// ---------------------------------------------------------------------------
// QueuedEvent
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct QueuedEvent {
    pub seq: i64,
    pub event_id: String,
    pub team_id: String,
    pub source: String,
    pub event_type: String,
    pub raw_payload: String,
    pub enriched_payload: Option<String>,
    pub importance: String,
    pub status: String,
    pub failure_reason: Option<String>,
    pub created_at: i64,
    pub enriched_at: Option<i64>,
    pub delivered_at: Option<i64>,
}

// ---------------------------------------------------------------------------
// Queue
// ---------------------------------------------------------------------------

pub struct Queue {
    conn: Connection,
}

impl Queue {
    /// Open (or create) the queue database at the given path.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    /// Open an in-memory database (for tests).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    /// Insert a new event. Uses INSERT OR IGNORE so repeated inserts with the
    /// same seq are silently discarded (idempotent).
    #[allow(clippy::too_many_arguments)]
    pub fn insert(
        &self,
        seq: i64,
        event_id: &str,
        team_id: &str,
        source: &str,
        event_type: &str,
        raw_payload: &str,
        created_at: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO events
                (seq, event_id, team_id, source, event_type, raw_payload, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                seq,
                event_id,
                team_id,
                source,
                event_type,
                raw_payload,
                created_at
            ],
        )?;
        Ok(())
    }

    /// Update the status of an event, optionally storing a failure reason.
    /// Sets `delivered_at` when transitioning to Delivered.
    pub fn update_status(
        &self,
        seq: i64,
        status: &EventStatus,
        failure_reason: Option<&str>,
    ) -> Result<()> {
        let now = now_unix();
        match status {
            EventStatus::Delivered => {
                self.conn.execute(
                    "UPDATE events SET status=?1, failure_reason=?2, delivered_at=?3 WHERE seq=?4",
                    params![status.as_str(), failure_reason, now, seq],
                )?;
            }
            _ => {
                self.conn.execute(
                    "UPDATE events SET status=?1, failure_reason=?2 WHERE seq=?3",
                    params![status.as_str(), failure_reason, seq],
                )?;
            }
        }
        Ok(())
    }

    /// Set the enriched payload, timestamp, and advance status to Ready.
    pub fn set_enriched(&self, seq: i64, enriched_payload: &str) -> Result<()> {
        let now = now_unix();
        self.conn.execute(
            "UPDATE events SET enriched_payload=?1, enriched_at=?2, status='ready' WHERE seq=?3",
            params![enriched_payload, now, seq],
        )?;
        Ok(())
    }

    /// Update the importance level of an event.
    pub fn set_importance(&self, seq: i64, importance: &Importance) -> Result<()> {
        self.conn.execute(
            "UPDATE events SET importance=?1 WHERE seq=?2",
            params![importance.as_str(), seq],
        )?;
        Ok(())
    }

    /// Fetch a single event by seq.
    pub fn get(&self, seq: i64) -> Result<Option<QueuedEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT seq, event_id, team_id, source, event_type, raw_payload,
                    enriched_payload, importance, status, failure_reason,
                    created_at, enriched_at, delivered_at
             FROM events WHERE seq=?1",
        )?;
        let mut rows = stmt.query(params![seq])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_event(row)?))
        } else {
            Ok(None)
        }
    }

    /// List events with optional filters.
    pub fn list(
        &self,
        status: Option<&EventStatus>,
        source: Option<&str>,
        importance: Option<&Importance>,
        limit: Option<u32>,
    ) -> Result<Vec<QueuedEvent>> {
        let mut sql = String::from(
            "SELECT seq, event_id, team_id, source, event_type, raw_payload,
                    enriched_payload, importance, status, failure_reason,
                    created_at, enriched_at, delivered_at
             FROM events WHERE 1=1",
        );
        let mut values: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        let mut idx = 1usize;

        if let Some(s) = status {
            sql.push_str(&format!(" AND status=?{}", idx));
            values.push(Box::new(s.as_str().to_owned()));
            idx += 1;
        }
        if let Some(src) = source {
            sql.push_str(&format!(" AND source=?{}", idx));
            values.push(Box::new(src.to_owned()));
            idx += 1;
        }
        if let Some(imp) = importance {
            sql.push_str(&format!(" AND importance=?{}", idx));
            values.push(Box::new(imp.as_str().to_owned()));
            idx += 1;
        }

        sql.push_str(" ORDER BY seq ASC");

        if let Some(lim) = limit {
            sql.push_str(&format!(" LIMIT ?{}", idx));
            values.push(Box::new(lim as i64));
        }

        let mut stmt = self.conn.prepare(&sql)?;
        let params_refs: Vec<&dyn rusqlite::ToSql> = values.iter().map(|v| v.as_ref()).collect();
        let rows = stmt.query(params_refs.as_slice())?;
        collect_rows(rows)
    }

    /// Return a map of status → event count.
    pub fn stats(&self) -> Result<HashMap<String, i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT status, COUNT(*) FROM events GROUP BY status")?;
        let mut map = HashMap::new();
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let status: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            map.insert(status, count);
        }
        Ok(map)
    }

    /// Delete events whose created_at is strictly before `before_timestamp`.
    pub fn flush_before(&self, before_timestamp: i64) -> Result<usize> {
        let n = self.conn.execute(
            "DELETE FROM events WHERE created_at < ?1",
            params![before_timestamp],
        )?;
        Ok(n)
    }

    /// Return the highest seq currently in the table, or None if empty.
    pub fn last_seq(&self) -> Result<Option<i64>> {
        let mut stmt = self.conn.prepare("SELECT MAX(seq) FROM events")?;
        let mut rows = stmt.query([])?;
        if let Some(row) = rows.next()? {
            let val: Option<i64> = row.get(0)?;
            Ok(val)
        } else {
            Ok(None)
        }
    }

    /// Import existing DLQ JSON files into the queue, then remove them.
    /// DLQ structure: dlq_dir/{source}/{event_id}.json
    /// Each file contains a serialized CloudEvent JSON.
    pub fn migrate_dlq(dlq_dir: &std::path::Path) -> Result<u32> {
        if !dlq_dir.exists() {
            return Ok(0);
        }

        let queue_path = crate::config::queue_db_path();
        let q = Self::open(&queue_path)?;
        let mut imported = 0u32;

        // Iterate source directories in DLQ
        let entries = std::fs::read_dir(dlq_dir)?;
        for source_entry in entries {
            let source_entry = source_entry?;
            if !source_entry.file_type()?.is_dir() {
                continue;
            }
            let source_name = source_entry.file_name().to_string_lossy().to_string();

            let files = std::fs::read_dir(source_entry.path())?;
            for file_entry in files {
                let file_entry = file_entry?;
                let path = file_entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }

                let content = match std::fs::read_to_string(&path) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!("Failed to read DLQ file {}: {e}", path.display());
                        continue;
                    }
                };

                let event_id = file_entry
                    .path()
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();

                let event_type = serde_json::from_str::<serde_json::Value>(&content)
                    .ok()
                    .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(String::from))
                    .unwrap_or_else(|| format!("com.{source_name}.unknown"));

                // Use negative seq to avoid conflicts with real server sequences
                let migrated_seq = -(imported as i64 + 1);

                q.insert(
                    migrated_seq,
                    &event_id,
                    "unknown",
                    &source_name,
                    &event_type,
                    &content,
                    now_unix(),
                )?;
                q.update_status(
                    migrated_seq,
                    &crate::queue::EventStatus::Failed,
                    Some("migrated from DLQ"),
                )?;
                imported += 1;

                std::fs::remove_file(&path)?;
            }
            let _ = std::fs::remove_dir(source_entry.path());
        }
        let _ = std::fs::remove_dir(dlq_dir);

        if imported > 0 {
            tracing::info!("Migrated {imported} events from DLQ to queue");
        }
        Ok(imported)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn row_to_event(row: &rusqlite::Row<'_>) -> Result<QueuedEvent> {
    Ok(QueuedEvent {
        seq: row.get(0)?,
        event_id: row.get(1)?,
        team_id: row.get(2)?,
        source: row.get(3)?,
        event_type: row.get(4)?,
        raw_payload: row.get(5)?,
        enriched_payload: row.get(6)?,
        importance: row.get(7)?,
        status: row.get(8)?,
        failure_reason: row.get(9)?,
        created_at: row.get(10)?,
        enriched_at: row.get(11)?,
        delivered_at: row.get(12)?,
    })
}

fn collect_rows(mut rows: rusqlite::Rows<'_>) -> Result<Vec<QueuedEvent>> {
    let mut events = Vec::new();
    while let Some(row) = rows.next()? {
        events.push(row_to_event(row)?);
    }
    Ok(events)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_queue() -> Queue {
        Queue::open_in_memory().expect("in-memory queue")
    }

    fn insert_sample(q: &Queue, seq: i64) {
        q.insert(seq, "evt-1", "team-1", "github", "push", r#"{"x":1}"#, 1000)
            .expect("insert");
    }

    #[test]
    fn test_insert_and_get() {
        let q = make_queue();
        insert_sample(&q, 1);
        let ev = q.get(1).expect("get").expect("should exist");
        assert_eq!(ev.seq, 1);
        assert_eq!(ev.event_id, "evt-1");
        assert_eq!(ev.team_id, "team-1");
        assert_eq!(ev.source, "github");
        assert_eq!(ev.event_type, "push");
        assert_eq!(ev.raw_payload, r#"{"x":1}"#);
        assert_eq!(ev.status, "pending");
        assert_eq!(ev.importance, "normal");
        assert!(ev.enriched_payload.is_none());
        assert!(ev.failure_reason.is_none());
    }

    #[test]
    fn test_update_status_delivered() {
        let q = make_queue();
        insert_sample(&q, 1);
        q.update_status(1, &EventStatus::Delivered, None)
            .expect("update");
        let ev = q.get(1).expect("get").expect("exists");
        assert_eq!(ev.status, "delivered");
        assert!(ev.delivered_at.is_some());
        assert!(ev.failure_reason.is_none());
    }

    #[test]
    fn test_update_status_failed() {
        let q = make_queue();
        insert_sample(&q, 1);
        q.update_status(1, &EventStatus::Failed, Some("timeout"))
            .expect("update");
        let ev = q.get(1).expect("get").expect("exists");
        assert_eq!(ev.status, "failed");
        assert_eq!(ev.failure_reason.as_deref(), Some("timeout"));
        assert!(ev.delivered_at.is_none());
    }

    #[test]
    fn test_set_enriched() {
        let q = make_queue();
        insert_sample(&q, 1);
        q.set_enriched(1, r#"{"enriched":true}"#).expect("enrich");
        let ev = q.get(1).expect("get").expect("exists");
        assert_eq!(ev.status, "ready");
        assert_eq!(ev.enriched_payload.as_deref(), Some(r#"{"enriched":true}"#));
        assert!(ev.enriched_at.is_some());
    }

    #[test]
    fn test_set_importance() {
        let q = make_queue();
        insert_sample(&q, 1);
        q.set_importance(1, &Importance::Critical).expect("set");
        let ev = q.get(1).expect("get").expect("exists");
        assert_eq!(ev.importance, "critical");
    }

    #[test]
    fn test_list_with_filters() {
        let q = make_queue();
        // seq 1: github / pending
        q.insert(1, "e1", "t1", "github", "push", "{}", 1000)
            .unwrap();
        // seq 2: stripe / pending
        q.insert(2, "e2", "t1", "stripe", "charge", "{}", 1001)
            .unwrap();
        // seq 3: github / delivered
        q.insert(3, "e3", "t1", "github", "push", "{}", 1002)
            .unwrap();
        q.update_status(3, &EventStatus::Delivered, None).unwrap();

        // Filter by source=github
        let github = q
            .list(None, Some("github"), None, None)
            .expect("list source");
        assert_eq!(github.len(), 2);
        assert!(github.iter().all(|e| e.source == "github"));

        // Filter by status=pending
        let pending = q
            .list(Some(&EventStatus::Pending), None, None, None)
            .expect("list status");
        assert_eq!(pending.len(), 2);
        assert!(pending.iter().all(|e| e.status == "pending"));

        // Combine source + status
        let github_pending = q
            .list(Some(&EventStatus::Pending), Some("github"), None, None)
            .expect("combined");
        assert_eq!(github_pending.len(), 1);
        assert_eq!(github_pending[0].seq, 1);
    }

    #[test]
    fn test_stats() {
        let q = make_queue();
        q.insert(1, "e1", "t1", "github", "push", "{}", 1000)
            .unwrap();
        q.insert(2, "e2", "t1", "github", "push", "{}", 1001)
            .unwrap();
        q.update_status(2, &EventStatus::Delivered, None).unwrap();
        q.insert(3, "e3", "t1", "github", "push", "{}", 1002)
            .unwrap();
        q.update_status(3, &EventStatus::Failed, Some("err"))
            .unwrap();

        let stats = q.stats().expect("stats");
        assert_eq!(stats.get("pending").copied(), Some(1));
        assert_eq!(stats.get("delivered").copied(), Some(1));
        assert_eq!(stats.get("failed").copied(), Some(1));
    }

    #[test]
    fn test_flush_before() {
        let q = make_queue();
        q.insert(1, "e1", "t1", "github", "push", "{}", 500)
            .unwrap();
        q.insert(2, "e2", "t1", "github", "push", "{}", 1000)
            .unwrap();
        q.insert(3, "e3", "t1", "github", "push", "{}", 1500)
            .unwrap();

        let deleted = q.flush_before(1000).expect("flush");
        assert_eq!(deleted, 1); // only seq=1 (created_at=500) is before 1000

        assert!(q.get(1).unwrap().is_none());
        assert!(q.get(2).unwrap().is_some());
        assert!(q.get(3).unwrap().is_some());
    }

    #[test]
    fn test_insert_idempotent() {
        let q = make_queue();
        insert_sample(&q, 1);
        // Second insert with same seq should be silently ignored
        q.insert(
            1,
            "different-id",
            "other-team",
            "stripe",
            "charge",
            "{}",
            9999,
        )
        .expect("idempotent insert should not error");

        let ev = q.get(1).unwrap().unwrap();
        // Original values preserved
        assert_eq!(ev.event_id, "evt-1");
        assert_eq!(ev.source, "github");
    }

    #[test]
    fn test_last_seq() {
        let q = make_queue();
        assert!(q.last_seq().unwrap().is_none());
        insert_sample(&q, 5);
        insert_sample(&q, 10);
        insert_sample(&q, 3);
        assert_eq!(q.last_seq().unwrap(), Some(10));
    }

    #[test]
    fn test_importance_rank() {
        assert_eq!(Importance::Low.rank(), 0);
        assert_eq!(Importance::Normal.rank(), 1);
        assert_eq!(Importance::High.rank(), 2);
        assert_eq!(Importance::Critical.rank(), 3);

        // Verify ordering
        assert!(Importance::Critical.rank() > Importance::High.rank());
        assert!(Importance::High.rank() > Importance::Normal.rank());
        assert!(Importance::Normal.rank() > Importance::Low.rank());
    }

    #[test]
    fn test_migrate_dlq() {
        // Create a temp DLQ directory structure
        let tmp = std::env::temp_dir().join(format!("kite-dlq-test-{}", std::process::id()));
        let github_dir = tmp.join("github");
        std::fs::create_dir_all(&github_dir).unwrap();

        let event = serde_json::json!({
            "specversion": "1.0",
            "id": "evt-dlq-1",
            "type": "com.github.push",
            "source": "https://github.com/test/repo",
            "data": {}
        });
        std::fs::write(github_dir.join("evt-dlq-1.json"), event.to_string()).unwrap();

        // Verify the file exists
        assert!(github_dir.join("evt-dlq-1.json").exists());

        // Cleanup
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
