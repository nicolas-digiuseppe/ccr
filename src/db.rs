use anyhow::Result;
use rusqlite::{Connection, params};
use std::path::PathBuf;

/// Full row including fts_content — used for insertion/upsert.
pub struct SessionRow {
    pub session_id: String,
    pub project: String,
    pub project_path: String,
    pub jsonl_path: String,
    pub first_message: String,
    pub summary: String,
    pub slug: Option<String>,
    pub started_at: String,
    pub last_modified: String,
    pub message_count: i64,
    pub file_size: i64,
    pub duration_minutes: i64,
    pub fts_content: String,
    // Token data (NULL if session not yet re-indexed with new parser)
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub total_cost_usd: f64,
}

/// Lightweight result returned by queries (no fts_content).
pub struct SessionInfo {
    pub session_id: String,
    pub project: String,
    pub project_path: String,
    pub jsonl_path: String,
    pub first_message: String,
    pub slug: Option<String>,
    pub started_at: String,
    pub last_modified: String,
    pub message_count: i64,
    pub file_size: i64,
    pub duration_minutes: i64,
    pub summary: Option<String>,
}

#[allow(dead_code)]
pub struct DayTokens {
    pub date: String,                   // "2026-04-09"
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub total_tokens: u64,
    pub total_cost: f64,
    pub sessions_count: u64,            // filled from our DB
}

#[allow(dead_code)]
pub struct Stats {
    pub total: i64,
    pub total_duration: i64,
    pub total_messages: i64,
    pub projects: Vec<(String, i64, i64)>,  // (name, count, total_duration)
    pub this_week: i64,
    pub this_month: i64,
    pub favorites: i64,
    /// last 90 days: ("YYYY-MM-DD", count) — ISO format for heatmap
    pub daily_sessions: Vec<(String, u64)>,
    pub daily_duration: Vec<u64>,              // last 30 days: minutes per day
    pub avg_session_duration: i64,
    pub avg_messages_per_session: i64,
    // Token / cost data (populated from sessions table during indexing)
    pub daily_tokens: Vec<DayTokens>,
    pub monthly_cost: f64,
    pub total_cost: f64,
    pub total_tokens: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_creation_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub cache_hit_rate: f64,             // cache_read_tokens / (input_tokens + cache_read_tokens) * 100
    /// Earliest and latest session dates (ISO "YYYY-MM-DD") for the header date range
    pub first_date: String,
    pub last_date: String,
}


pub struct Database {
    pub conn: Connection,
}

impl Database {
    /// Open a database at the given path, initialising the schema.
    pub fn open(path: &PathBuf) -> Result<Self> {
        let conn = Connection::open(path)?;
        let db = Database { conn };
        db.init()?;
        Ok(db)
    }

    /// Open an in-memory database (used in tests).
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Database { conn };
        db.init()?;
        Ok(db)
    }

    /// Default path: `~/.claude/ccr.db`
    pub fn default_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".claude")
            .join("ccr.db")
    }

    fn init(&self) -> Result<()> {
        // WAL mode for better concurrent read performance
        self.conn.pragma_update_and_check(None, "journal_mode", "WAL", |_row| Ok(()))?;

        let current_version: i64 = self.conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        const SCHEMA_VERSION: i64 = 1;
        let needs_fts_rebuild = current_version < SCHEMA_VERSION;

        self.conn.execute_batch("
            CREATE TABLE IF NOT EXISTS sessions (
                session_id      TEXT PRIMARY KEY,
                project         TEXT NOT NULL,
                project_path    TEXT NOT NULL,
                jsonl_path      TEXT NOT NULL,
                first_message   TEXT NOT NULL,
                slug            TEXT,
                started_at      TEXT NOT NULL,
                last_modified   TEXT NOT NULL,
                message_count   INTEGER NOT NULL DEFAULT 0,
                file_size       INTEGER NOT NULL DEFAULT 0,
                duration_minutes INTEGER NOT NULL DEFAULT 0,
                input_tokens    INTEGER,
                output_tokens   INTEGER,
                cache_creation_tokens INTEGER,
                cache_read_tokens INTEGER,
                total_cost_usd  REAL
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS sessions_fts USING fts5(
                session_id UNINDEXED,
                content
            );

            CREATE TABLE IF NOT EXISTS session_tags (
                session_id TEXT NOT NULL,
                tag        TEXT NOT NULL,
                PRIMARY KEY (session_id, tag)
            );
        ")?;

        // Migration: add slug column if it doesn't exist (for existing DBs).
        let _ = self.conn.execute_batch(
            "ALTER TABLE sessions ADD COLUMN slug TEXT;"
        );
        // Migration: add summary column
        let _ = self.conn.execute_batch(
            "ALTER TABLE sessions ADD COLUMN summary TEXT;"
        );
        // Migration: add token columns for direct JSONL parsing (removes ccusage dependency)
        // Each statement is run independently so a "duplicate column" error on one doesn't block others.
        let _ = self.conn.execute_batch("ALTER TABLE sessions ADD COLUMN input_tokens INTEGER;");
        let _ = self.conn.execute_batch("ALTER TABLE sessions ADD COLUMN output_tokens INTEGER;");
        let _ = self.conn.execute_batch("ALTER TABLE sessions ADD COLUMN cache_creation_tokens INTEGER;");
        let _ = self.conn.execute_batch("ALTER TABLE sessions ADD COLUMN cache_read_tokens INTEGER;");
        let _ = self.conn.execute_batch("ALTER TABLE sessions ADD COLUMN total_cost_usd REAL;");

        if needs_fts_rebuild {
            self.conn.execute_batch("DELETE FROM sessions_fts;")?;
            self.conn.execute_batch("UPDATE sessions SET last_modified = '0';")?;
            self.conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }

        Ok(())
    }

    /// Insert or replace a session row (also refreshes FTS index).
    pub fn upsert_session(&self, row: &SessionRow) -> Result<()> {
        // Remove any existing FTS entry first
        self.conn.execute(
            "DELETE FROM sessions_fts WHERE session_id = ?1",
            params![row.session_id],
        )?;

        // Write NULL when no token data (all zeros), actual value otherwise
        let token_input: Option<i64>  = if row.input_tokens == 0 && row.output_tokens == 0 && row.cache_read_tokens == 0 { None } else { Some(row.input_tokens as i64) };
        let token_output: Option<i64> = token_input.map(|_| row.output_tokens as i64);
        let token_cw: Option<i64>     = token_input.map(|_| row.cache_creation_tokens as i64);
        let token_cr: Option<i64>     = token_input.map(|_| row.cache_read_tokens as i64);
        let token_cost: Option<f64>   = token_input.map(|_| row.total_cost_usd);

        // Upsert the main sessions table (preserve existing AI summary)
        self.conn.execute(
            "INSERT INTO sessions
             (session_id, project, project_path, jsonl_path, first_message, summary, slug,
              started_at, last_modified, message_count, file_size, duration_minutes,
              input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens, total_cost_usd)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
             ON CONFLICT(session_id) DO UPDATE SET
              project = excluded.project,
              project_path = excluded.project_path,
              jsonl_path = excluded.jsonl_path,
              first_message = excluded.first_message,
              summary = CASE WHEN sessions.summary IS NOT NULL AND sessions.summary != '' THEN sessions.summary ELSE excluded.summary END,
              slug = excluded.slug,
              started_at = excluded.started_at,
              last_modified = excluded.last_modified,
              message_count = excluded.message_count,
              file_size = excluded.file_size,
              duration_minutes = excluded.duration_minutes,
              input_tokens = excluded.input_tokens,
              output_tokens = excluded.output_tokens,
              cache_creation_tokens = excluded.cache_creation_tokens,
              cache_read_tokens = excluded.cache_read_tokens,
              total_cost_usd = excluded.total_cost_usd",
            params![
                row.session_id,
                row.project,
                row.project_path,
                row.jsonl_path,
                row.first_message,
                row.summary,
                row.slug,
                row.started_at,
                row.last_modified,
                row.message_count,
                row.file_size,
                row.duration_minutes,
                token_input,
                token_output,
                token_cw,
                token_cr,
                token_cost,
            ],
        )?;

        // Insert fresh FTS entry (prepend project + summary for searchability)
        let enriched_fts = {
            let mut parts = Vec::new();
            if !row.project.is_empty() {
                parts.push(row.project.as_str());
            }
            let db_summary: Option<String> = self.conn.query_row(
                "SELECT summary FROM sessions WHERE session_id = ?1",
                params![row.session_id],
                |r| r.get(0),
            ).ok().flatten();
            let summary = if row.summary.is_empty() {
                db_summary.as_deref().unwrap_or("")
            } else {
                &row.summary
            };
            if !summary.is_empty() {
                parts.push(summary);
            }
            if parts.is_empty() {
                row.fts_content.clone()
            } else {
                format!("{}\n{}", parts.join("\n"), row.fts_content)
            }
        };
        self.conn.execute(
            "INSERT INTO sessions_fts (session_id, content) VALUES (?1, ?2)",
            params![row.session_id, enriched_fts],
        )?;

        Ok(())
    }

    /// List sessions, optionally filtered by project and/or a metadata text search.
    /// Results are ordered by last_modified DESC.
    pub fn list_sessions(
        &self,
        project_filter: Option<&str>,
        search_query: Option<&str>,
    ) -> Result<Vec<SessionInfo>> {
        let mut sql = String::from(
            "SELECT session_id, project, project_path, jsonl_path, first_message, slug,
                    started_at, last_modified, message_count, file_size, duration_minutes, summary
             FROM sessions WHERE 1=1",
        );

        let mut owned_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(proj) = project_filter {
            sql.push_str(" AND project = ?");
            owned_params.push(Box::new(proj.to_string()));
        }

        if let Some(q) = search_query {
            sql.push_str(" AND (first_message LIKE ? OR project LIKE ?)");
            let pattern = format!("%{}%", q);
            owned_params.push(Box::new(pattern.clone()));
            owned_params.push(Box::new(pattern));
        }

        sql.push_str(" ORDER BY last_modified DESC");

        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            owned_params.iter().map(|b| b.as_ref()).collect();

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_refs.as_slice(), Self::map_session_info)?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Full-text search across session content via FTS5.
    /// Sanitizes input for FTS5 syntax, appends * to last term for prefix matching.
    pub fn search_fulltext(&self, query: &str) -> Result<Vec<SessionInfo>> {
        let terms: Vec<String> = query
            .split_whitespace()
            .map(|w| {
                w.chars()
                    .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
                    .collect::<String>()
            })
            .filter(|w| !w.is_empty())
            .filter(|w| !matches!(w.to_uppercase().as_str(), "AND" | "OR" | "NOT" | "NEAR"))
            .collect();

        if terms.is_empty() {
            return Ok(Vec::new());
        }

        // Append * to last term for prefix matching while typing
        let fts_query = terms
            .iter()
            .enumerate()
            .map(|(i, t)| {
                if i == terms.len() - 1 {
                    format!("\"{}\"*", t)
                } else {
                    format!("\"{}\"", t)
                }
            })
            .collect::<Vec<_>>()
            .join(" ");

        let sql = "
            SELECT s.session_id, s.project, s.project_path, s.jsonl_path, s.first_message, s.slug,
                   s.started_at, s.last_modified, s.message_count, s.file_size, s.duration_minutes, s.summary
            FROM sessions_fts f
            JOIN sessions s ON s.session_id = f.session_id
            WHERE sessions_fts MATCH ?1
            ORDER BY rank
        ";

        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params![fts_query], Self::map_session_info);

        match rows {
            Ok(rows) => {
                let mut results = Vec::new();
                for row in rows {
                    results.push(row?);
                }
                Ok(results)
            }
            Err(_) => Ok(Vec::new()),
        }
    }

    /// List all distinct projects with session counts.
    pub fn list_projects(&self) -> Result<Vec<(String, usize)>> {
        let mut stmt = self.conn.prepare(
            "SELECT project, COUNT(*) as cnt FROM sessions GROUP BY project ORDER BY project",
        )?;

        let rows = stmt.query_map([], |row| {
            let name: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            Ok((name, count as usize))
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Return the stored last_modified timestamp for a session, or None if not found.
    pub fn get_session_mtime(&self, session_id: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT last_modified FROM sessions WHERE session_id = ?1")?;

        let mut rows = stmt.query(params![session_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    /// Delete a single session by ID from all tables.
    pub fn delete_session(&self, session_id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM sessions WHERE session_id = ?1", params![session_id])?;
        self.conn.execute("DELETE FROM sessions_fts WHERE session_id = ?1", params![session_id])?;
        self.conn.execute("DELETE FROM session_tags WHERE session_id = ?1", params![session_id])?;
        Ok(())
    }

    /// Delete sessions (and their FTS entries) whose jsonl_path no longer exists on disk.
    pub fn purge_missing(&self) -> Result<()> {
        let paths: Vec<(String, String)> = {
            let mut stmt = self
                .conn
                .prepare("SELECT session_id, jsonl_path FROM sessions")?;
            let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };

        for (session_id, jsonl_path) in paths {
            if !std::path::Path::new(&jsonl_path).exists() {
                self.conn.execute(
                    "DELETE FROM sessions_fts WHERE session_id = ?1",
                    params![session_id],
                )?;
                self.conn.execute(
                    "DELETE FROM sessions WHERE session_id = ?1",
                    params![session_id],
                )?;
            }
        }
        Ok(())
    }

    /// Compute global usage statistics.
    pub fn get_stats(&self) -> Result<Stats> {
        // Total sessions
        let total: i64 = self.conn.query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))?;

        // Total duration
        let total_duration: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(duration_minutes), 0) FROM sessions", [], |row| row.get(0)
        )?;

        // Total messages
        let total_messages: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(message_count), 0) FROM sessions", [], |row| row.get(0)
        )?;

        // Sessions per project (top 10)
        let mut stmt = self.conn.prepare(
            "SELECT project, COUNT(*), SUM(duration_minutes) FROM sessions GROUP BY project ORDER BY COUNT(*) DESC LIMIT 10"
        )?;
        let projects = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?))
        })?.filter_map(|r| r.ok()).collect();

        // Sessions this week (last 7 days) — last_modified is stored as unix timestamp string
        let week_ago = (chrono::Utc::now() - chrono::Duration::days(7)).timestamp().to_string();
        let this_week: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sessions WHERE CAST(last_modified AS INTEGER) > CAST(?1 AS INTEGER)",
            params![week_ago], |row| row.get(0)
        )?;

        // Sessions this month (last 30 days)
        let month_ago = (chrono::Utc::now() - chrono::Duration::days(30)).timestamp().to_string();
        let this_month: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sessions WHERE CAST(last_modified AS INTEGER) > CAST(?1 AS INTEGER)",
            params![month_ago], |row| row.get(0)
        )?;

        // Favorites count
        let favorites: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM session_tags WHERE tag = '⭐'", [], |row| row.get(0)
        )?;

        // Average session duration and messages per session
        let avg_session_duration: i64 = self.conn.query_row(
            "SELECT COALESCE(CAST(AVG(duration_minutes) AS INTEGER), 0) FROM sessions", [], |row| row.get(0)
        )?;
        let avg_messages_per_session: i64 = self.conn.query_row(
            "SELECT COALESCE(CAST(AVG(message_count) AS INTEGER), 0) FROM sessions", [], |row| row.get(0)
        )?;

        // Daily sessions for last 90 days (ISO date keys for heatmap)
        let ninety_ago_ts = (chrono::Utc::now() - chrono::Duration::days(90)).timestamp();
        let mut daily_stmt = self.conn.prepare(
            "SELECT date(CAST(last_modified AS INTEGER), 'unixepoch') as d, COUNT(*)
             FROM sessions
             WHERE CAST(last_modified AS INTEGER) > CAST(?1 AS INTEGER)
             GROUP BY d ORDER BY d ASC"
        )?;
        let daily_rows: Vec<(String, u64)> = daily_stmt.query_map(
            params![ninety_ago_ts],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64))
        )?.filter_map(|r| r.ok()).collect();

        // Daily duration for last 30 days
        let month_ago_ts = (chrono::Utc::now() - chrono::Duration::days(30)).timestamp();
        let mut dur_stmt = self.conn.prepare(
            "SELECT date(CAST(last_modified AS INTEGER), 'unixepoch') as d, COALESCE(SUM(duration_minutes), 0)
             FROM sessions
             WHERE CAST(last_modified AS INTEGER) > CAST(?1 AS INTEGER)
             GROUP BY d ORDER BY d ASC"
        )?;
        let dur_rows: Vec<(String, u64)> = dur_stmt.query_map(
            params![month_ago_ts],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64))
        )?.filter_map(|r| r.ok()).collect();

        // Build aligned daily session array for 90 days (ISO "YYYY-MM-DD" keys)
        let today = chrono::Utc::now().date_naive();
        let mut daily_sessions: Vec<(String, u64)> = Vec::with_capacity(90);
        for day_offset in (0..90i64).rev() {
            let date = today - chrono::Duration::days(day_offset);
            let date_str = date.format("%Y-%m-%d").to_string();
            let count = daily_rows.iter().find(|(d, _)| d == &date_str).map(|(_, c)| *c).unwrap_or(0);
            daily_sessions.push((date_str, count));
        }

        // Determine the first and last session dates across all sessions (not just 90 days)
        let first_date: String = self.conn.query_row(
            "SELECT COALESCE(MIN(date(CAST(last_modified AS INTEGER), 'unixepoch')), '') FROM sessions",
            [], |row| row.get(0)
        ).unwrap_or_default();
        let last_date: String = self.conn.query_row(
            "SELECT COALESCE(MAX(date(CAST(last_modified AS INTEGER), 'unixepoch')), '') FROM sessions",
            [], |row| row.get(0)
        ).unwrap_or_default();

        // Build aligned daily duration array for 30 days
        let mut daily_duration: Vec<u64> = Vec::with_capacity(30);
        for day_offset in (0..30i64).rev() {
            let date = today - chrono::Duration::days(day_offset);
            let date_str = date.format("%Y-%m-%d").to_string();
            let dur = dur_rows.iter().find(|(d, _)| d == &date_str).map(|(_, d)| *d).unwrap_or(0);
            daily_duration.push(dur);
        }

        // Token data — aggregated from sessions table (populated during indexing)
        let mut token_stmt = self.conn.prepare(
            "SELECT
                 date(CAST(last_modified AS INTEGER), 'unixepoch') AS day,
                 COALESCE(SUM(input_tokens), 0),
                 COALESCE(SUM(output_tokens), 0),
                 COALESCE(SUM(cache_creation_tokens), 0),
                 COALESCE(SUM(cache_read_tokens), 0),
                 COALESCE(SUM(total_cost_usd), 0.0),
                 COUNT(*)
             FROM sessions
             WHERE input_tokens IS NOT NULL
               AND CAST(last_modified AS INTEGER) > CAST(?1 AS INTEGER)
             GROUP BY day
             ORDER BY day ASC"
        )?;
        let daily_tokens: Vec<DayTokens> = token_stmt.query_map(
            params![ninety_ago_ts],
            |row| {
                let inp: u64 = row.get::<_, i64>(1)? as u64;
                let outp: u64 = row.get::<_, i64>(2)? as u64;
                let cw: u64 = row.get::<_, i64>(3)? as u64;
                let cr: u64 = row.get::<_, i64>(4)? as u64;
                Ok(DayTokens {
                    date: row.get::<_, String>(0)?,
                    input_tokens: inp,
                    output_tokens: outp,
                    cache_creation_tokens: cw,
                    cache_read_tokens: cr,
                    total_tokens: inp + outp + cw + cr,
                    total_cost: row.get::<_, f64>(5)?,
                    sessions_count: row.get::<_, i64>(6)? as u64,
                })
            }
        )?.filter_map(|r| r.ok()).collect();

        // Grand totals across all sessions (not just 90 days)
        let (total_input, total_output, total_cache_creation, total_cache_read, total_cost_val): (u64, u64, u64, u64, f64) =
            self.conn.query_row(
                "SELECT
                     COALESCE(SUM(input_tokens), 0),
                     COALESCE(SUM(output_tokens), 0),
                     COALESCE(SUM(cache_creation_tokens), 0),
                     COALESCE(SUM(cache_read_tokens), 0),
                     COALESCE(SUM(total_cost_usd), 0.0)
                 FROM sessions WHERE input_tokens IS NOT NULL",
                [],
                |row| Ok((
                    row.get::<_, i64>(0)? as u64,
                    row.get::<_, i64>(1)? as u64,
                    row.get::<_, i64>(2)? as u64,
                    row.get::<_, i64>(3)? as u64,
                    row.get::<_, f64>(4)?,
                ))
            ).unwrap_or((0, 0, 0, 0, 0.0));

        let total_tokens_val = total_input + total_output + total_cache_creation + total_cache_read;

        let cache_hit_rate = {
            let denominator = total_input + total_cache_read;
            if denominator > 0 { total_cache_read as f64 / denominator as f64 * 100.0 } else { 0.0 }
        };

        // Monthly cost: sum daily_tokens entries from last 30 days
        let thirty_ago = chrono::Utc::now().date_naive() - chrono::Duration::days(30);
        let monthly_cost: f64 = daily_tokens.iter()
            .filter(|d| {
                chrono::NaiveDate::parse_from_str(&d.date, "%Y-%m-%d")
                    .map(|nd| nd >= thirty_ago)
                    .unwrap_or(false)
            })
            .map(|d| d.total_cost)
            .sum();

        Ok(Stats {
            total, total_duration, total_messages, projects,
            this_week, this_month, favorites,
            daily_sessions, daily_duration,
            avg_session_duration, avg_messages_per_session,
            daily_tokens,
            monthly_cost,
            total_cost: total_cost_val,
            total_tokens: total_tokens_val,
            total_input_tokens: total_input,
            total_output_tokens: total_output,
            total_cache_creation_tokens: total_cache_creation,
            total_cache_read_tokens: total_cache_read,
            cache_hit_rate,
            first_date,
            last_date,
        })
    }

}

impl Database {
    /// Update the AI-generated summary for a session.
    pub fn set_summary(&self, session_id: &str, summary: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET summary = ?2 WHERE session_id = ?1",
            params![session_id, summary],
        )?;
        Ok(())
    }

    /// List sessions that need an AI summary (have messages but no AI summary in the summary column,
    /// or summary was auto-generated from keywords).
    pub fn sessions_needing_summary(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT session_id, jsonl_path FROM sessions WHERE message_count > 0 AND (summary IS NULL OR summary = '')"
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Add a tag to a session.
    pub fn add_tag(&self, session_id: &str, tag: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO session_tags (session_id, tag) VALUES (?1, ?2)",
            params![session_id, tag],
        )?;
        Ok(())
    }

    /// Remove a tag from a session.
    pub fn remove_tag(&self, session_id: &str, tag: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM session_tags WHERE session_id = ?1 AND tag = ?2",
            params![session_id, tag],
        )?;
        Ok(())
    }

    /// Toggle the favorite tag ("⭐") on a session. Returns the new favorite state.
    pub fn toggle_favorite(&self, session_id: &str) -> Result<bool> {
        let fav = "⭐";
        if self.is_favorite(session_id)? {
            self.remove_tag(session_id, fav)?;
            Ok(false)
        } else {
            self.add_tag(session_id, fav)?;
            Ok(true)
        }
    }

    /// Check whether a session is marked as favorite.
    pub fn is_favorite(&self, session_id: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM session_tags WHERE session_id = ?1 AND tag = '⭐'",
            params![session_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// List all session_ids that have the favorite tag.
    pub fn list_favorites(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT session_id FROM session_tags WHERE tag = '⭐'",
        )?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row?);
        }
        Ok(ids)
    }

    fn map_session_info(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionInfo> {
        Ok(SessionInfo {
            session_id: row.get(0)?,
            project: row.get(1)?,
            project_path: row.get(2)?,
            jsonl_path: row.get(3)?,
            first_message: row.get(4)?,
            slug: row.get(5)?,
            started_at: row.get(6)?,
            last_modified: row.get(7)?,
            message_count: row.get(8)?,
            file_size: row.get(9)?,
            duration_minutes: row.get(10)?,
            summary: row.get(11)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Database {
        Database::open_in_memory().unwrap()
    }

    #[test]
    fn test_create_schema() {
        let db = test_db();
        db.conn.query_row("SELECT count(*) FROM sessions", [], |_| Ok(())).unwrap();
    }

    #[test]
    fn test_upsert_and_query_session() {
        let db = test_db();
        let row = SessionRow {
            session_id: "abc-123".to_string(),
            project: "bugpilot".to_string(),
            project_path: "/Users/test/www/bugpilot".to_string(),
            jsonl_path: "/tmp/test.jsonl".to_string(),
            first_message: "hello world".to_string(),
            slug: Some("mossy-exploring-seahorse".to_string()),
            started_at: "2026-04-09T08:00:00.000Z".to_string(),
            last_modified: "2026-04-09T10:00:00.000Z".to_string(),
            message_count: 42,
            file_size: 1024,
            duration_minutes: 120,
            fts_content: "hello world some conversation".to_string(),
            summary: String::new(),
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            total_cost_usd: 0.0,
        };
        db.upsert_session(&row).unwrap();
        let sessions = db.list_sessions(None, None).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "abc-123");
        assert_eq!(sessions[0].project, "bugpilot");
    }

    #[test]
    fn test_fts_search() {
        let db = test_db();
        let row = SessionRow {
            session_id: "s1".to_string(), project: "proj".to_string(),
            project_path: "/tmp".to_string(), jsonl_path: "/tmp/s1.jsonl".to_string(),
            first_message: "download excel feature".to_string(),
            slug: None,
            started_at: "2026-04-09T08:00:00.000Z".to_string(),
            last_modified: "2026-04-09T10:00:00.000Z".to_string(),
            message_count: 10, file_size: 500, duration_minutes: 30,
            fts_content: "working on download excel multi sheets feature".to_string(),
            summary: String::new(),
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            total_cost_usd: 0.0,
        };
        db.upsert_session(&row).unwrap();
        let results = db.search_fulltext("excel sheets").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].session_id, "s1");
        let no_results = db.search_fulltext("kubernetes deploy").unwrap();
        assert!(no_results.is_empty());
    }

    #[test]
    fn test_filter_by_project() {
        let db = test_db();
        for (id, proj) in [("s1", "bugpilot"), ("s2", "bugpilot"), ("s3", "frontend")] {
            db.upsert_session(&SessionRow {
                session_id: id.to_string(), project: proj.to_string(),
                project_path: "/tmp".to_string(), jsonl_path: format!("/tmp/{id}.jsonl"),
                first_message: "msg".to_string(),
                slug: None,
                started_at: "2026-04-09T08:00:00.000Z".to_string(),
                last_modified: "2026-04-09T10:00:00.000Z".to_string(),
                message_count: 5, file_size: 100, duration_minutes: 10,
                fts_content: "content".to_string(),
                summary: String::new(),
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
                total_cost_usd: 0.0,
            }).unwrap();
        }
        let all = db.list_sessions(None, None).unwrap();
        assert_eq!(all.len(), 3);
        let filtered = db.list_sessions(Some("bugpilot"), None).unwrap();
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn test_list_projects() {
        let db = test_db();
        for (id, proj) in [("s1", "bugpilot"), ("s2", "bugpilot"), ("s3", "frontend")] {
            db.upsert_session(&SessionRow {
                session_id: id.to_string(), project: proj.to_string(),
                project_path: "/tmp".to_string(), jsonl_path: format!("/tmp/{id}.jsonl"),
                first_message: "msg".to_string(),
                slug: None,
                started_at: "2026-04-09T08:00:00.000Z".to_string(),
                last_modified: "2026-04-09T10:00:00.000Z".to_string(),
                message_count: 5, file_size: 100, duration_minutes: 10,
                fts_content: "content".to_string(),
                summary: String::new(),
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
                total_cost_usd: 0.0,
            }).unwrap();
        }
        let projects = db.list_projects().unwrap();
        assert_eq!(projects.len(), 2);
        assert!(projects.iter().any(|(name, count)| name == "bugpilot" && *count == 2));
        assert!(projects.iter().any(|(name, count)| name == "frontend" && *count == 1));
    }

    #[test]
    fn test_purge_missing() {
        let db = test_db();
        db.upsert_session(&SessionRow {
            session_id: "gone".to_string(), project: "proj".to_string(),
            project_path: "/tmp".to_string(), jsonl_path: "/tmp/nonexistent.jsonl".to_string(),
            first_message: "msg".to_string(),
            summary: String::new(),
            slug: None,
            started_at: "2026-04-09T08:00:00.000Z".to_string(),
            last_modified: "2026-04-09T10:00:00.000Z".to_string(),
            message_count: 5, file_size: 100, duration_minutes: 10,
            fts_content: "content".to_string(),
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            total_cost_usd: 0.0,
        }).unwrap();
        db.purge_missing().unwrap();
        let sessions = db.list_sessions(None, None).unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn test_get_mtime() {
        let db = test_db();
        assert!(db.get_session_mtime("nonexistent").unwrap().is_none());
        db.upsert_session(&SessionRow {
            session_id: "s1".to_string(), project: "proj".to_string(),
            project_path: "/tmp".to_string(), jsonl_path: "/tmp/s1.jsonl".to_string(),
            first_message: "msg".to_string(),
            summary: String::new(),
            slug: None,
            started_at: "2026-04-09T08:00:00.000Z".to_string(),
            last_modified: "2026-04-09T10:00:00.000Z".to_string(),
            message_count: 5, file_size: 100, duration_minutes: 10,
            fts_content: "content".to_string(),
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            total_cost_usd: 0.0,
        }).unwrap();
        let mtime = db.get_session_mtime("s1").unwrap();
        assert_eq!(mtime, Some("2026-04-09T10:00:00.000Z".to_string()));
    }

    #[test]
    fn test_get_stats_computes_token_totals_from_db() {
        let db = Database::open_in_memory().unwrap();

        let now_ts = chrono::Utc::now().timestamp().to_string();
        // Insert two sessions with token data
        for (id, inp, outp, cw, cr, cost) in [
            ("s1", 1000i64, 200i64, 500i64,  5000i64, 0.05f64),
            ("s2", 2000i64, 400i64, 0i64,   10000i64, 0.10f64),
        ] {
            db.conn.execute(
                "INSERT INTO sessions (session_id, project, project_path, jsonl_path, first_message,
                 started_at, last_modified, message_count, file_size, duration_minutes,
                 input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens, total_cost_usd)
                 VALUES (?1, 'proj', '/tmp', '/tmp/a.jsonl', 'hi', ?2, ?2, 1, 100, 5, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![id, now_ts, inp, outp, cw, cr, cost]
            ).unwrap();
        }

        let stats = db.get_stats().unwrap();
        assert_eq!(stats.total_input_tokens, 3000);
        assert_eq!(stats.total_output_tokens, 600);
        assert_eq!(stats.total_cache_read_tokens, 15000);
        assert!((stats.total_cost - 0.15).abs() < 0.001, "got {}", stats.total_cost);
        // Cache hit rate: cache_read / (input + cache_read) * 100 = 15000 / 18000 * 100 = 83.33...
        assert!((stats.cache_hit_rate - 83.33).abs() < 0.1, "got {}", stats.cache_hit_rate);
        // daily_tokens should have 1 entry (today)
        assert_eq!(stats.daily_tokens.len(), 1);
        assert_eq!(stats.daily_tokens[0].sessions_count, 2);
    }

    #[test]
    fn test_upsert_and_query_token_columns() {
        let db = Database::open_in_memory().unwrap();
        let row = SessionRow {
            session_id: "tok-001".to_string(),
            project: "myproj".to_string(),
            project_path: "/tmp/myproj".to_string(),
            jsonl_path: "/tmp/myproj/tok-001.jsonl".to_string(),
            first_message: "hello".to_string(),
            summary: String::new(),
            slug: None,
            started_at: "1000000".to_string(),
            last_modified: "1000001".to_string(),
            message_count: 2,
            file_size: 100,
            duration_minutes: 5,
            fts_content: "hello".to_string(),
            input_tokens: 1000,
            output_tokens: 200,
            cache_creation_tokens: 500,
            cache_read_tokens: 3000,
            total_cost_usd: 0.042,
        };
        db.upsert_session(&row).unwrap();

        let val: f64 = db.conn.query_row(
            "SELECT total_cost_usd FROM sessions WHERE session_id = 'tok-001'",
            [], |row| row.get(0)
        ).unwrap();
        assert!((val - 0.042).abs() < 0.0001);

        let inp: i64 = db.conn.query_row(
            "SELECT input_tokens FROM sessions WHERE session_id = 'tok-001'",
            [], |row| row.get(0)
        ).unwrap();
        assert_eq!(inp, 1000);

        let outp: i64 = db.conn.query_row(
            "SELECT output_tokens FROM sessions WHERE session_id = 'tok-001'",
            [], |row| row.get(0)
        ).unwrap();
        assert_eq!(outp, 200);

        let cw: i64 = db.conn.query_row(
            "SELECT cache_creation_tokens FROM sessions WHERE session_id = 'tok-001'",
            [], |row| row.get(0)
        ).unwrap();
        assert_eq!(cw, 500);

        let cr: i64 = db.conn.query_row(
            "SELECT cache_read_tokens FROM sessions WHERE session_id = 'tok-001'",
            [], |row| row.get(0)
        ).unwrap();
        assert_eq!(cr, 3000);
    }

    #[test]
    fn test_search_fulltext_prefix() {
        let db = Database::open_in_memory().unwrap();
        db.upsert_session(&SessionRow {
            session_id: "s1".into(),
            project: "myproject".into(),
            project_path: "/tmp".into(),
            jsonl_path: "/tmp/s1.jsonl".into(),
            first_message: "hello world".into(),
            summary: String::new(),
            slug: None,
            started_at: "2026-01-01T00:00:00Z".into(),
            last_modified: "2026-01-01T01:00:00Z".into(),
            message_count: 5,
            file_size: 100,
            duration_minutes: 60,
            fts_content: "implementing authentication middleware for login".into(),
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            total_cost_usd: 0.0,
        }).unwrap();

        // Prefix match: "auth" should match "authentication"
        let results = db.search_fulltext("auth").unwrap();
        assert_eq!(results.len(), 1);

        // Multi-word: both words must match
        let results = db.search_fulltext("authentication login").unwrap();
        assert_eq!(results.len(), 1);

        // No match
        let results = db.search_fulltext("nonexistent").unwrap();
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_fts_indexes_project_and_summary() {
        let db = Database::open_in_memory().unwrap();
        db.upsert_session(&SessionRow {
            session_id: "s1".into(),
            project: "bugpilot".into(),
            project_path: "/tmp".into(),
            jsonl_path: "/tmp/s1.jsonl".into(),
            first_message: "fix login".into(),
            summary: String::new(),
            slug: None,
            started_at: "2026-01-01T00:00:00Z".into(),
            last_modified: "2026-01-01T01:00:00Z".into(),
            message_count: 5,
            file_size: 100,
            duration_minutes: 60,
            fts_content: "some code content".into(),
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            total_cost_usd: 0.0,
        }).unwrap();

        // Project name should be searchable via FTS
        let results = db.search_fulltext("bugpilot").unwrap();
        assert_eq!(results.len(), 1);

        // Now set a summary and re-upsert to refresh FTS
        db.set_summary("s1", "authentication middleware refactor").unwrap();
        db.upsert_session(&SessionRow {
            session_id: "s1".into(),
            project: "bugpilot".into(),
            project_path: "/tmp".into(),
            jsonl_path: "/tmp/s1.jsonl".into(),
            first_message: "fix login".into(),
            summary: "authentication middleware refactor".into(),
            slug: None,
            started_at: "2026-01-01T00:00:00Z".into(),
            last_modified: "2026-01-01T01:00:00Z".into(),
            message_count: 5,
            file_size: 100,
            duration_minutes: 60,
            fts_content: "some code content".into(),
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            total_cost_usd: 0.0,
        }).unwrap();

        let results = db.search_fulltext("middleware").unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_search_fulltext_sanitization() {
        let db = Database::open_in_memory().unwrap();
        db.upsert_session(&SessionRow {
            session_id: "s1".into(),
            project: "test".into(),
            project_path: "/tmp".into(),
            jsonl_path: "/tmp/s1.jsonl".into(),
            first_message: "hello".into(),
            summary: String::new(),
            slug: None,
            started_at: "2026-01-01T00:00:00Z".into(),
            last_modified: "2026-01-01T01:00:00Z".into(),
            message_count: 1,
            file_size: 50,
            duration_minutes: 10,
            fts_content: "some content here".into(),
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            total_cost_usd: 0.0,
        }).unwrap();

        // FTS5 special characters should not cause errors
        let results = db.search_fulltext("C++");
        assert!(results.is_ok());

        let results = db.search_fulltext("\"unclosed quote");
        assert!(results.is_ok());

        let results = db.search_fulltext("NOT OR AND");
        assert!(results.is_ok());

        // Empty after sanitization should return empty vec
        let results = db.search_fulltext("+++").unwrap();
        assert!(results.is_empty());
    }
}
