use rusqlite::{Connection, Result, Row, params};
use serde::{Deserialize, Serialize};

use crate::agents::{LogEntry, LogSource};
use crate::snapshots::SnapshotRow;

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

/// One row in any of the Usage-tab breakdown tables. `count` is sessions for
/// project/model and events for tool/shell/activity. `cost_usd` is real for
/// project/model; for tool/shell/activity it's an even-split approximation
/// (session cost / event count, summed per group).
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BreakdownRow {
    pub name: String,
    pub count: i64,
    pub tokens: i64,
    pub cost_usd: f64,
    pub share_pct: f64,
}

/// Range-aware totals shown above the breakdown sections.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct UsageTotals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_tokens: u64,
    pub cost_usd: f64,
    pub session_count: u64,
    pub event_count: u64,
}

/// Single payload returned by `get_usage_breakdown` so a range change is one
/// round-trip instead of six.
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct UsageBreakdown {
    pub total: UsageTotals,
    pub by_day: Vec<DayStats>,
    pub by_project: Vec<BreakdownRow>,
    pub by_model: Vec<BreakdownRow>,
    pub by_tool: Vec<BreakdownRow>,
    pub by_shell: Vec<BreakdownRow>,
    pub by_activity: Vec<BreakdownRow>,
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

            CREATE TABLE IF NOT EXISTS file_snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                project_path TEXT NOT NULL DEFAULT '',
                file_path TEXT NOT NULL,
                tool_name TEXT NOT NULL,
                phase TEXT NOT NULL,
                paired_id INTEGER,
                blob_path TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                sha256 TEXT NOT NULL,
                is_binary INTEGER NOT NULL DEFAULT 0,
                oversized INTEGER NOT NULL DEFAULT 0,
                ts TEXT NOT NULL,
                tool_use_id TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_snapshots_session_ts
                ON file_snapshots(session_id, ts DESC);

            CREATE INDEX IF NOT EXISTS idx_snapshots_tool_use_id
                ON file_snapshots(tool_use_id);
        ")?;
        // Idempotent migration: add the three split-cache columns if they
        // don't exist yet. Older rows keep `cache_tokens` as the only cache
        // figure; new rows populate both legacy and split columns.
        self.add_column_if_missing("sessions", "cache_write_5m_tokens", "INTEGER NOT NULL DEFAULT 0")?;
        self.add_column_if_missing("sessions", "cache_write_1h_tokens", "INTEGER NOT NULL DEFAULT 0")?;
        self.add_column_if_missing("sessions", "cache_read_tokens", "INTEGER NOT NULL DEFAULT 0")?;
        // Per-event tool input snapshot. Currently populated for `Bash`
        // PreToolUse hooks (the raw `command` arg, truncated). Other tool
        // kinds leave it NULL. Powers the shell-command breakdown.
        self.add_column_if_missing("agent_events", "tool_params", "TEXT")?;
        Ok(())
    }

    fn add_column_if_missing(&self, table: &str, name: &str, decl: &str) -> Result<()> {
        let pragma = format!("SELECT 1 FROM pragma_table_info('{table}') WHERE name = ?1");
        let exists: bool = self
            .conn
            .prepare(&pragma)?
            .query_row(params![name], |_| Ok(true))
            .unwrap_or(false);
        if !exists {
            self.conn
                .execute(&format!("ALTER TABLE {table} ADD COLUMN {name} {decl}"), [])?;
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
                "INSERT INTO agent_events
                    (session_id, ts, source, kind, summary, details, tool_params)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
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
                    e.tool_params,
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
            "SELECT session_id, ts, source, kind, summary, details, tool_params
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
                tool_params: row.get(6)?,
            })
        })?;
        let mut out: Vec<LogEntry> = rows.collect::<Result<Vec<_>>>()?;
        out.reverse();
        Ok(out)
    }

    // ---- File snapshots (History tab) ----

    /// Insert a `file_snapshots` row. `blob_path` is filled in after the row
    /// id is known (the blob filename derives from the id), so callers should
    /// follow up with `update_file_snapshot_blob_path`.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_file_snapshot(
        &mut self,
        session_id: &str,
        project_path: &str,
        file_path: &str,
        tool_name: &str,
        phase: &str,
        paired_id: Option<i64>,
        blob_path: &str,
        size_bytes: i64,
        sha256: &str,
        is_binary: bool,
        oversized: bool,
        ts: &str,
        tool_use_id: Option<&str>,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO file_snapshots
                (session_id, project_path, file_path, tool_name, phase,
                 paired_id, blob_path, size_bytes, sha256, is_binary,
                 oversized, ts, tool_use_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                session_id,
                project_path,
                file_path,
                tool_name,
                phase,
                paired_id,
                blob_path,
                size_bytes,
                sha256,
                is_binary as i64,
                oversized as i64,
                ts,
                tool_use_id,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn update_file_snapshot_blob_path(&self, id: i64, blob_path: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE file_snapshots SET blob_path = ?1 WHERE id = ?2",
            params![blob_path, id],
        )?;
        Ok(())
    }

    pub fn set_paired_id(&self, id: i64, paired_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE file_snapshots SET paired_id = ?1 WHERE id = ?2",
            params![paired_id, id],
        )?;
        Ok(())
    }

    pub fn find_paired_id(&self, id: i64) -> Result<Option<i64>> {
        match self.conn.query_row(
            "SELECT paired_id FROM file_snapshots WHERE id = ?1",
            params![id],
            |row| row.get::<_, Option<i64>>(0),
        ) {
            Ok(opt) => Ok(opt),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Most recent unpaired `pre` row for the given `tool_use_id`. Used by
    /// `PostToolUse` capture to link the matching pair.
    pub fn find_unpaired_pre(&self, session_id: &str, tool_use_id: &str) -> Result<Option<i64>> {
        match self.conn.query_row(
            "SELECT id FROM file_snapshots
             WHERE session_id = ?1 AND tool_use_id = ?2
               AND phase = 'pre' AND paired_id IS NULL
             ORDER BY id DESC LIMIT 1",
            params![session_id, tool_use_id],
            |row| row.get::<_, i64>(0),
        ) {
            Ok(id) => Ok(Some(id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn get_file_snapshot(&self, id: i64) -> Result<Option<SnapshotRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, project_path, file_path, tool_name, phase,
                    paired_id, blob_path, size_bytes, sha256, is_binary,
                    oversized, ts, tool_use_id
             FROM file_snapshots WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], row_to_snapshot)?;
        match rows.next() {
            Some(r) => r.map(Some),
            None => Ok(None),
        }
    }

    pub fn list_session_snapshots(&self, session_id: &str) -> Result<Vec<SnapshotRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, project_path, file_path, tool_name, phase,
                    paired_id, blob_path, size_bytes, sha256, is_binary,
                    oversized, ts, tool_use_id
             FROM file_snapshots
             WHERE session_id = ?1
             ORDER BY ts DESC, id DESC",
        )?;
        let rows = stmt.query_map(params![session_id], row_to_snapshot)?;
        rows.collect()
    }

    pub fn list_recent_snapshots(&self, limit: usize) -> Result<Vec<SnapshotRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, project_path, file_path, tool_name, phase,
                    paired_id, blob_path, size_bytes, sha256, is_binary,
                    oversized, ts, tool_use_id
             FROM file_snapshots
             ORDER BY ts DESC, id DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], row_to_snapshot)?;
        rows.collect()
    }

    /// Returns the blob_paths of every deleted row so the caller can unlink
    /// the on-disk blobs.
    pub fn delete_file_snapshots_older_than(&mut self, days: i64) -> Result<Vec<String>> {
        let cutoff = format!("-{} days", days);
        let mut paths: Vec<String> = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT blob_path FROM file_snapshots
                 WHERE ts < datetime('now', ?1)",
            )?;
            let rows = stmt.query_map(params![cutoff], |row| row.get::<_, String>(0))?;
            for r in rows {
                paths.push(r?);
            }
        }
        self.conn.execute(
            "DELETE FROM file_snapshots WHERE ts < datetime('now', ?1)",
            params![cutoff],
        )?;
        Ok(paths)
    }

    pub fn delete_file_snapshots_for_session(&mut self, session_id: &str) -> Result<Vec<String>> {
        let mut paths: Vec<String> = Vec::new();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT blob_path FROM file_snapshots WHERE session_id = ?1")?;
            let rows = stmt.query_map(params![session_id], |row| row.get::<_, String>(0))?;
            for r in rows {
                paths.push(r?);
            }
        }
        self.conn.execute(
            "DELETE FROM file_snapshots WHERE session_id = ?1",
            params![session_id],
        )?;
        Ok(paths)
    }

    /// (count, total size in bytes) across the whole snapshot store.
    pub fn snapshot_totals(&self) -> Result<(i64, i64)> {
        self.conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(size_bytes), 0) FROM file_snapshots",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
    }

    // ---- Usage-tab breakdowns ----

    /// SUM-everything totals over the range. `session_count` filters by
    /// `sessions.updated_at`; `event_count` filters by `agent_events.ts`.
    pub fn get_totals_in_range(&self, start: &str, end: &str) -> Result<UsageTotals> {
        let (input, output, cache, cost, sess): (i64, i64, i64, f64, i64) = self
            .conn
            .query_row(
                "SELECT
                    COALESCE(SUM(input_tokens), 0),
                    COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(cache_tokens), 0),
                    COALESCE(SUM(cost_usd), 0.0),
                    COUNT(*)
                 FROM sessions
                 WHERE date(updated_at) BETWEEN ?1 AND ?2",
                params![start, end],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap_or((0, 0, 0, 0.0, 0));

        let events: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM agent_events
                 WHERE date(ts) BETWEEN ?1 AND ?2",
                params![start, end],
                |r| r.get(0),
            )
            .unwrap_or(0);

        Ok(UsageTotals {
            input_tokens: input as u64,
            output_tokens: output as u64,
            cache_tokens: cache as u64,
            cost_usd: cost,
            session_count: sess as u64,
            event_count: events as u64,
        })
    }

    /// Sessions GROUP BY project_path. Real cost (no approximation).
    ///
    /// Windows drive-letter casing is normalized so `C:\foo` and `c:\foo`
    /// collapse to one row (they're the same project — NTFS is case-
    /// insensitive). The displayed name uses `MIN(...)` so the conventional
    /// uppercase drive letter wins (uppercase sorts before lowercase in
    /// ASCII). Non-Windows paths fall through unchanged.
    pub fn get_breakdown_by_project(&self, start: &str, end: &str) -> Result<Vec<BreakdownRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT
                MIN(COALESCE(NULLIF(project_path, ''), '(unknown)')) AS name,
                COUNT(*) AS count,
                COALESCE(SUM(input_tokens + output_tokens + cache_tokens), 0) AS tokens,
                COALESCE(SUM(cost_usd), 0.0) AS cost
             FROM sessions
             WHERE date(updated_at) BETWEEN ?1 AND ?2
             GROUP BY
                CASE
                    WHEN SUBSTR(project_path, 2, 1) = ':'
                    THEN LOWER(SUBSTR(project_path, 1, 1)) || SUBSTR(project_path, 2)
                    ELSE COALESCE(NULLIF(project_path, ''), '(unknown)')
                END
             ORDER BY cost DESC, count DESC",
        )?;
        let rows = stmt.query_map(params![start, end], |r| {
            Ok(BreakdownRow {
                name: r.get(0)?,
                count: r.get(1)?,
                tokens: r.get(2)?,
                cost_usd: r.get(3)?,
                share_pct: 0.0,
            })
        })?;
        Ok(with_share_pct(rows.collect::<Result<Vec<_>>>()?, ShareMetric::Cost))
    }

    /// Sessions GROUP BY model. Real cost.
    pub fn get_breakdown_by_model(&self, start: &str, end: &str) -> Result<Vec<BreakdownRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT
                COALESCE(NULLIF(model, ''), '(unknown)') AS name,
                COUNT(*) AS count,
                COALESCE(SUM(input_tokens + output_tokens + cache_tokens), 0) AS tokens,
                COALESCE(SUM(cost_usd), 0.0) AS cost
             FROM sessions
             WHERE date(updated_at) BETWEEN ?1 AND ?2
             GROUP BY name
             ORDER BY cost DESC, count DESC",
        )?;
        let rows = stmt.query_map(params![start, end], |r| {
            Ok(BreakdownRow {
                name: r.get(0)?,
                count: r.get(1)?,
                tokens: r.get(2)?,
                cost_usd: r.get(3)?,
                share_pct: 0.0,
            })
        })?;
        Ok(with_share_pct(rows.collect::<Result<Vec<_>>>()?, ShareMetric::Cost))
    }

    /// Tool breakdown with approximate cost (session cost / event count, per
    /// session, summed per tool). Filters to ToolUseStart-style events so we
    /// don't count user messages or text-only turns.
    pub fn get_breakdown_by_tool(&self, start: &str, end: &str) -> Result<Vec<BreakdownRow>> {
        let mut stmt = self.conn.prepare(
            "WITH session_event_n AS (
                SELECT session_id, COUNT(*) AS n
                FROM agent_events
                WHERE date(ts) BETWEEN ?1 AND ?2
                GROUP BY session_id
            )
            SELECT
                e.summary AS name,
                COUNT(*) AS count,
                0 AS tokens,
                COALESCE(SUM(s.cost_usd / NULLIF(n.n, 0)), 0.0) AS cost
             FROM agent_events e
             JOIN session_event_n n ON e.session_id = n.session_id
             LEFT JOIN sessions s   ON e.session_id = s.id
             WHERE date(e.ts) BETWEEN ?1 AND ?2
               AND e.kind IN ('ToolUseStart', 'Hook:PreToolUse')
               AND e.summary <> ''
             GROUP BY name
             ORDER BY count DESC",
        )?;
        let rows = stmt.query_map(params![start, end], |r| {
            Ok(BreakdownRow {
                name: r.get(0)?,
                count: r.get(1)?,
                tokens: r.get(2)?,
                cost_usd: r.get(3)?,
                share_pct: 0.0,
            })
        })?;
        Ok(with_share_pct(rows.collect::<Result<Vec<_>>>()?, ShareMetric::Count))
    }

    /// Activity breakdown — every event kind, approx cost.
    pub fn get_breakdown_by_activity(&self, start: &str, end: &str) -> Result<Vec<BreakdownRow>> {
        let mut stmt = self.conn.prepare(
            "WITH session_event_n AS (
                SELECT session_id, COUNT(*) AS n
                FROM agent_events
                WHERE date(ts) BETWEEN ?1 AND ?2
                GROUP BY session_id
            )
            SELECT
                e.kind AS name,
                COUNT(*) AS count,
                0 AS tokens,
                COALESCE(SUM(s.cost_usd / NULLIF(n.n, 0)), 0.0) AS cost
             FROM agent_events e
             JOIN session_event_n n ON e.session_id = n.session_id
             LEFT JOIN sessions s   ON e.session_id = s.id
             WHERE date(e.ts) BETWEEN ?1 AND ?2
             GROUP BY name
             ORDER BY count DESC",
        )?;
        let rows = stmt.query_map(params![start, end], |r| {
            Ok(BreakdownRow {
                name: r.get(0)?,
                count: r.get(1)?,
                tokens: r.get(2)?,
                cost_usd: r.get(3)?,
                share_pct: 0.0,
            })
        })?;
        Ok(with_share_pct(rows.collect::<Result<Vec<_>>>()?, ShareMetric::Count))
    }

    /// Shell-command breakdown — bucketed by the first whitespace-separated
    /// token of the captured `command` arg. Only Bash tool calls are eligible.
    /// Aggregation done in Rust because SQLite lacks robust string splitting.
    pub fn get_breakdown_by_shell(&self, start: &str, end: &str) -> Result<Vec<BreakdownRow>> {
        let mut stmt = self.conn.prepare(
            "WITH session_event_n AS (
                SELECT session_id, COUNT(*) AS n
                FROM agent_events
                WHERE date(ts) BETWEEN ?1 AND ?2
                GROUP BY session_id
            )
            SELECT
                e.tool_params AS cmd,
                e.session_id  AS sid,
                CAST(s.cost_usd AS REAL) / NULLIF(n.n, 0) AS approx_cost
             FROM agent_events e
             JOIN session_event_n n ON e.session_id = n.session_id
             LEFT JOIN sessions s   ON e.session_id = s.id
             WHERE date(e.ts) BETWEEN ?1 AND ?2
               AND e.summary = 'Bash'
               AND e.tool_params IS NOT NULL",
        )?;
        let rows = stmt.query_map(params![start, end], |r| {
            let cmd: String = r.get::<_, Option<String>>(0)?.unwrap_or_default();
            let approx_cost: f64 = r.get::<_, Option<f64>>(2)?.unwrap_or(0.0);
            Ok((cmd, approx_cost))
        })?;

        // Bucket by the first token of the command, e.g. "git status -s" → "git".
        use std::collections::HashMap;
        let mut buckets: HashMap<String, (i64, f64)> = HashMap::new();
        for r in rows {
            let (cmd, cost) = r?;
            let head = cmd
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_' && c != '.');
            let key = if head.is_empty() {
                continue;
            } else {
                head.to_string()
            };
            let entry = buckets.entry(key).or_insert((0, 0.0));
            entry.0 += 1;
            entry.1 += cost;
        }
        let mut out: Vec<BreakdownRow> = buckets
            .into_iter()
            .map(|(name, (count, cost))| BreakdownRow {
                name,
                count,
                tokens: 0,
                cost_usd: cost,
                share_pct: 0.0,
            })
            .collect();
        out.sort_by(|a, b| b.count.cmp(&a.count));
        Ok(with_share_pct(out, ShareMetric::Count))
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

enum ShareMetric {
    Cost,
    Count,
}

/// Compute `share_pct` (0..100) for each row based on the chosen metric.
/// Project/model use Cost (real). Tool/shell/activity use Count because
/// approx-cost can be flat-zero across rows when sessions have no cost data.
fn with_share_pct(mut rows: Vec<BreakdownRow>, metric: ShareMetric) -> Vec<BreakdownRow> {
    let total: f64 = match metric {
        ShareMetric::Cost => rows.iter().map(|r| r.cost_usd).sum(),
        ShareMetric::Count => rows.iter().map(|r| r.count as f64).sum(),
    };
    if total > 0.0 {
        for r in &mut rows {
            let v = match metric {
                ShareMetric::Cost => r.cost_usd,
                ShareMetric::Count => r.count as f64,
            };
            r.share_pct = (v / total) * 100.0;
        }
    }
    rows
}

fn row_to_snapshot(row: &Row) -> Result<SnapshotRow> {
    let is_binary: i64 = row.get(10)?;
    let oversized: i64 = row.get(11)?;
    Ok(SnapshotRow {
        id: row.get(0)?,
        session_id: row.get(1)?,
        project_path: row.get(2)?,
        file_path: row.get(3)?,
        tool_name: row.get(4)?,
        phase: row.get(5)?,
        paired_id: row.get(6)?,
        blob_path: row.get(7)?,
        size_bytes: row.get(8)?,
        sha256: row.get(9)?,
        is_binary: is_binary != 0,
        oversized: oversized != 0,
        ts: row.get(12)?,
        tool_use_id: row.get(13)?,
    })
}
