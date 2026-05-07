use rusqlite::{Connection, Result, Row, params};
use serde::{Deserialize, Serialize};

use crate::agents::{LogEntry, LogSource};

/// Best-effort 0600 on Unix; no-op on Windows. Local sessions DB can hold
/// per-project usage history that shouldn't be world-readable on shared hosts.
fn restrict_to_owner(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

// SQLite stores integers as i64; rusqlite 0.39 dropped the implicit u64 conversion.
// Round-trip through i64 explicitly so callers can keep `u64` field types.
fn get_u64(row: &Row, idx: usize) -> Result<u64> {
    let v: i64 = row.get(idx)?;
    Ok(v as u64)
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Session {
    pub id: String,
    pub project_path: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Legacy combined cache total. Equals
    /// `cache_write_5m_tokens + cache_write_1h_tokens + cache_read_tokens`
    /// for new rows; for rows written before the schema split, this is the
    /// only cache figure available.
    pub cache_tokens: u64,
    pub cache_write_5m_tokens: u64,
    pub cache_write_1h_tokens: u64,
    pub cache_read_tokens: u64,
    pub cost_usd: f64,
    pub started_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DailySummary {
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cost_usd: f64,
    pub session_count: u64,
    pub top_model: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DayStats {
    pub date: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn new() -> Result<Self> {
        let path = dirs::data_local_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("claude-monitor")
            .join("data.db");

        std::fs::create_dir_all(path.parent().unwrap()).ok();
        let conn = Connection::open(&path)?;
        restrict_to_owner(&path);
        let db = Self { conn };
        db.init()?;
        Ok(db)
    }

    fn init(&self) -> Result<()> {
        self.conn.execute_batch("
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                project_path TEXT NOT NULL DEFAULT '',
                model TEXT NOT NULL DEFAULT '',
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_tokens INTEGER NOT NULL DEFAULT 0,
                cost_usd REAL NOT NULL DEFAULT 0.0,
                started_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_sessions_updated
                ON sessions(updated_at);

            CREATE TABLE IF NOT EXISTS agent_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                ts TEXT NOT NULL,
                source TEXT NOT NULL,
                kind TEXT NOT NULL,
                summary TEXT NOT NULL,
                details TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_agent_events_session_ts
                ON agent_events(session_id, ts DESC);
        ")?;
        // Idempotent migration: add the three split-cache columns if they
        // don't exist yet. Older rows keep `cache_tokens` as the only cache
        // figure; new rows populate both legacy and split columns.
        self.add_column_if_missing("cache_write_5m_tokens", "INTEGER NOT NULL DEFAULT 0")?;
        self.add_column_if_missing("cache_write_1h_tokens", "INTEGER NOT NULL DEFAULT 0")?;
        self.add_column_if_missing("cache_read_tokens", "INTEGER NOT NULL DEFAULT 0")?;
        Ok(())
    }

    fn add_column_if_missing(&self, name: &str, decl: &str) -> Result<()> {
        let exists: bool = self
            .conn
            .prepare("SELECT 1 FROM pragma_table_info('sessions') WHERE name = ?1")?
            .query_row(params![name], |_| Ok(true))
            .unwrap_or(false);
        if !exists {
            self.conn
                .execute(&format!("ALTER TABLE sessions ADD COLUMN {name} {decl}"), [])?;
        }
        Ok(())
    }

    /// Replace (insert-or-overwrite) a session row. Token totals are running
    /// totals already aggregated by AgentRegistry, so we overwrite rather
    /// than accumulate to avoid double-counting.
    pub fn replace_session(&self, session: &Session) -> Result<()> {
        self.conn.execute(
            "INSERT INTO sessions
                (id, project_path, model, input_tokens, output_tokens,
                 cache_tokens, cache_write_5m_tokens, cache_write_1h_tokens,
                 cache_read_tokens, cost_usd, started_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(id) DO UPDATE SET
                input_tokens = excluded.input_tokens,
                output_tokens = excluded.output_tokens,
                cache_tokens = excluded.cache_tokens,
                cache_write_5m_tokens = excluded.cache_write_5m_tokens,
                cache_write_1h_tokens = excluded.cache_write_1h_tokens,
                cache_read_tokens = excluded.cache_read_tokens,
                cost_usd = excluded.cost_usd,
                model = excluded.model,
                project_path = excluded.project_path,
                updated_at = excluded.updated_at",
            params![
                session.id,
                session.project_path,
                session.model,
                session.input_tokens as i64,
                session.output_tokens as i64,
                session.cache_tokens as i64,
                session.cache_write_5m_tokens as i64,
                session.cache_write_1h_tokens as i64,
                session.cache_read_tokens as i64,
                session.cost_usd,
                session.started_at,
                session.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_daily_summary(&self) -> Result<DailySummary> {
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();

        let mut stmt = self.conn.prepare(
            "SELECT
                COALESCE(SUM(input_tokens), 0),
                COALESCE(SUM(output_tokens), 0),
                COALESCE(SUM(cost_usd), 0.0),
                COUNT(*),
                COALESCE(
                    (SELECT model FROM sessions
                     WHERE date(updated_at) = ?1
                     GROUP BY model ORDER BY COUNT(*) DESC LIMIT 1),
                    'unknown'
                )
             FROM sessions
             WHERE date(updated_at) = ?1"
        )?;

        stmt.query_row(params![today], |row| {
            Ok(DailySummary {
                total_input_tokens: get_u64(row, 0)?,
                total_output_tokens: get_u64(row, 1)?,
                total_cost_usd: row.get(2)?,
                session_count: get_u64(row, 3)?,
                top_model: row.get(4)?,
            })
        })
    }

    pub fn get_recent_sessions(&self, limit: usize) -> Result<Vec<Session>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_path, model, input_tokens, output_tokens,
                    cache_tokens, cache_write_5m_tokens, cache_write_1h_tokens,
                    cache_read_tokens, cost_usd, started_at, updated_at
             FROM sessions
             ORDER BY updated_at DESC
             LIMIT ?1"
        )?;

        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(Session {
                id: row.get(0)?,
                project_path: row.get(1)?,
                model: row.get(2)?,
                input_tokens: get_u64(row, 3)?,
                output_tokens: get_u64(row, 4)?,
                cache_tokens: get_u64(row, 5)?,
                cache_write_5m_tokens: get_u64(row, 6)?,
                cache_write_1h_tokens: get_u64(row, 7)?,
                cache_read_tokens: get_u64(row, 8)?,
                cost_usd: row.get(9)?,
                started_at: row.get(10)?,
                updated_at: row.get(11)?,
            })
        })?;

        rows.collect()
    }

    pub fn get_weekly_stats(&self) -> Result<Vec<DayStats>> {
        let mut stmt = self.conn.prepare(
            "SELECT
                date(updated_at) as day,
                SUM(input_tokens),
                SUM(output_tokens),
                SUM(cost_usd)
             FROM sessions
             WHERE updated_at >= datetime('now', '-7 days')
             GROUP BY day
             ORDER BY day ASC"
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(DayStats {
                date: row.get(0)?,
                input_tokens: get_u64(row, 1)?,
                output_tokens: get_u64(row, 2)?,
                cost_usd: row.get(3)?,
            })
        })?;

        rows.collect()
    }

    /// Persist a batch of LogEntries from the live ring buffer. Idempotent
    /// in spirit but not enforced — callers should pass each entry once.
    /// Wrapped in a transaction so a few hundred rapid events don't pay
    /// fsync cost per insert.
    pub fn insert_events(&mut self, entries: &[LogEntry]) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO agent_events (session_id, ts, source, kind, summary, details)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for e in entries {
                let source = match e.source {
                    LogSource::Jsonl => "jsonl",
                    LogSource::Hook => "hook",
                };
                stmt.execute(params![
                    e.session_id,
                    e.timestamp.to_rfc3339(),
                    source,
                    e.kind,
                    e.summary,
                    e.details,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Most-recent N entries for a session, oldest first / newest last
    /// (matching the ring buffer ordering callers expect).
    pub fn events_for(&self, session_id: &str, limit: usize) -> Result<Vec<LogEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT session_id, ts, source, kind, summary, details
             FROM agent_events
             WHERE session_id = ?1
             ORDER BY ts DESC, id DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![session_id, limit as i64], |row| {
            let ts_str: String = row.get(1)?;
            let timestamp = chrono::DateTime::parse_from_rfc3339(&ts_str)
                .map(|d| d.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| chrono::Utc::now());
            let source_str: String = row.get(2)?;
            let source = if source_str == "hook" {
                LogSource::Hook
            } else {
                LogSource::Jsonl
            };
            Ok(LogEntry {
                session_id: row.get(0)?,
                timestamp,
                source,
                kind: row.get(3)?,
                summary: row.get(4)?,
                details: row.get(5)?,
            })
        })?;
        let mut out: Vec<LogEntry> = rows.collect::<Result<Vec<_>>>()?;
        out.reverse();
        Ok(out)
    }

    /// Per-day usage between two YYYY-MM-DD dates, inclusive on both ends.
    /// Days with no sessions are omitted (the frontend fills gaps so the
    /// chart axis is contiguous).
    pub fn get_usage_range(&self, start: &str, end: &str) -> Result<Vec<DayStats>> {
        let mut stmt = self.conn.prepare(
            "SELECT
                date(updated_at) as day,
                SUM(input_tokens),
                SUM(output_tokens),
                SUM(cost_usd)
             FROM sessions
             WHERE date(updated_at) BETWEEN ?1 AND ?2
             GROUP BY day
             ORDER BY day ASC"
        )?;

        let rows = stmt.query_map(params![start, end], |row| {
            Ok(DayStats {
                date: row.get(0)?,
                input_tokens: get_u64(row, 1)?,
                output_tokens: get_u64(row, 2)?,
                cost_usd: row.get(3)?,
            })
        })?;

        rows.collect()
    }
}
