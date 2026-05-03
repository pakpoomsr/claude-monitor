use rusqlite::{Connection, Result, params};
use serde::{Deserialize, Serialize};
use dirs;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Session {
    pub id: String,
    pub project_path: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_tokens: u64,
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
        ")
    }

    /// Replace (insert-or-overwrite) a session row. Token totals are running
    /// totals already aggregated by AgentRegistry, so we overwrite rather
    /// than accumulate to avoid double-counting.
    pub fn replace_session(&self, session: &Session) -> Result<()> {
        self.conn.execute(
            "INSERT INTO sessions
                (id, project_path, model, input_tokens, output_tokens,
                 cache_tokens, cost_usd, started_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET
                input_tokens = excluded.input_tokens,
                output_tokens = excluded.output_tokens,
                cache_tokens = excluded.cache_tokens,
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
                total_input_tokens: row.get::<_, i64>(0)? as u64,
                total_output_tokens: row.get::<_, i64>(1)? as u64,
                total_cost_usd: row.get(2)?,
                session_count: row.get::<_, i64>(3)? as u64,
                top_model: row.get(4)?,
            })
        })
    }

    pub fn get_recent_sessions(&self, limit: usize) -> Result<Vec<Session>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_path, model, input_tokens, output_tokens,
                    cache_tokens, cost_usd, started_at, updated_at
             FROM sessions
             ORDER BY updated_at DESC
             LIMIT ?1"
        )?;

        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(Session {
                id: row.get(0)?,
                project_path: row.get(1)?,
                model: row.get(2)?,
                input_tokens: row.get::<_, i64>(3)? as u64,
                output_tokens: row.get::<_, i64>(4)? as u64,
                cache_tokens: row.get::<_, i64>(5)? as u64,
                cost_usd: row.get(6)?,
                started_at: row.get(7)?,
                updated_at: row.get(8)?,
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
                input_tokens: row.get::<_, i64>(1)? as u64,
                output_tokens: row.get::<_, i64>(2)? as u64,
                cost_usd: row.get(3)?,
            })
        })?;

        rows.collect()
    }
}
