use rusqlite::{Connection, params};
use std::path::PathBuf;
use std::sync::Mutex;


/// SQLite-backed persistent storage for tokens, quota state, and request logs.
pub struct Storage {
    conn: Mutex<Connection>,
}

impl Storage {
    /// Open or create the database at the given path.
    pub fn open(path: &PathBuf) -> crate::error::Result<Self> {
        let conn = Connection::open(path)?;
        let storage = Self {
            conn: Mutex::new(conn),
        };
        storage.init_tables()?;
        Ok(storage)
    }

    /// In-memory database for testing.
    pub fn in_memory() -> crate::error::Result<Self> {
        let conn = Connection::open_in_memory()?;
        let storage = Self {
            conn: Mutex::new(conn),
        };
        storage.init_tables()?;
        Ok(storage)
    }

    fn init_tables(&self) -> crate::error::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS kv_store (
                key    TEXT PRIMARY KEY,
                value  TEXT NOT NULL,
                updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
            );

            CREATE TABLE IF NOT EXISTS provider_tokens (
                provider   TEXT PRIMARY KEY,
                token      TEXT NOT NULL,
                expires_at INTEGER,
                updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
            );

            CREATE TABLE IF NOT EXISTS quota_state (
                provider   TEXT NOT NULL,
                model      TEXT,
                used       INTEGER NOT NULL DEFAULT 0,
                spent_usd  REAL NOT NULL DEFAULT 0.0,
                window_start INTEGER NOT NULL,
                updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
                PRIMARY KEY (provider, model)
            );

            CREATE TABLE IF NOT EXISTS request_log (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp   INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
                tier        TEXT NOT NULL,
                score       REAL NOT NULL,
                provider    TEXT NOT NULL,
                model       TEXT NOT NULL,
                input_tokens  INTEGER,
                output_tokens INTEGER,
                cost_usd    REAL,
                latency_ms  INTEGER,
                success     INTEGER NOT NULL DEFAULT 1
            );

            CREATE INDEX IF NOT EXISTS idx_request_log_ts ON request_log(timestamp);
            CREATE INDEX IF NOT EXISTS idx_request_log_provider ON request_log(provider);

            CREATE TABLE IF NOT EXISTS model_overrides (
                provider   TEXT NOT NULL,
                model_id   TEXT NOT NULL,
                field      TEXT NOT NULL,
                value      TEXT NOT NULL,
                PRIMARY KEY (provider, model_id, field)
            );

            CREATE TABLE IF NOT EXISTS profiles (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                name        TEXT NOT NULL UNIQUE,
                description TEXT,
                config_json TEXT NOT NULL,
                created_at  INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
                updated_at  INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
                is_active   INTEGER NOT NULL DEFAULT 0
            );
            ",
        )?;
        Ok(())
    }

    // --- Key-Value Store ---

    pub fn set(&self, key: &str, value: &str) -> crate::error::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO kv_store (key, value, updated_at) VALUES (?1, ?2, strftime('%s', 'now'))",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get(&self, key: &str) -> crate::error::Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT value FROM kv_store WHERE key = ?1")?;
        let result = stmt
            .query_row(params![key], |row| row.get(0))
            .ok();
        Ok(result)
    }

    /// Find all key-value pairs matching a key pattern (SQL LIKE)
    pub fn get_matching(&self, pattern: &str) -> crate::error::Result<Vec<(String, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT key, value FROM kv_store WHERE key LIKE ?1")?;
        let rows = stmt.query_map(params![pattern], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut result = Vec::new();
        for r in rows {
            result.push(r?);
        }
        Ok(result)
    }

    // --- Provider Tokens ---

    pub fn save_token(
        &self,
        provider: &str,
        token: &str,
        expires_at: Option<i64>,
    ) -> crate::error::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO provider_tokens (provider, token, expires_at, updated_at)
             VALUES (?1, ?2, ?3, strftime('%s', 'now'))",
            params![provider, token, expires_at],
        )?;
        Ok(())
    }

    pub fn get_token(&self, provider: &str) -> crate::error::Result<Option<(String, Option<i64>)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT token, expires_at FROM provider_tokens WHERE provider = ?1")?;
        let result = stmt
            .query_row(params![provider], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?))
            })
            .ok();
        Ok(result)
    }

    // --- Quota State ---

    pub fn save_quota(
        &self,
        provider: &str,
        model: Option<&str>,
        used: u32,
        spent_usd: f64,
        window_start: i64,
    ) -> crate::error::Result<()> {
        let conn = self.conn.lock().unwrap();
        let model_str = model.unwrap_or("");
        conn.execute(
            "INSERT OR REPLACE INTO quota_state (provider, model, used, spent_usd, window_start, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, strftime('%s', 'now'))",
            params![provider, model_str, used, spent_usd, window_start],
        )?;
        Ok(())
    }

    pub fn get_quota(
        &self,
        provider: &str,
        model: Option<&str>,
    ) -> crate::error::Result<Option<(u32, f64, i64)>> {
        let conn = self.conn.lock().unwrap();
        let model_str = model.unwrap_or("");
        let mut stmt = conn.prepare(
            "SELECT used, spent_usd, window_start FROM quota_state WHERE provider = ?1 AND model = ?2",
        )?;
        let result = stmt
            .query_row(params![provider, model_str], |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, f64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .ok();
        Ok(result)
    }

    // --- Request Logging ---

    pub fn log_request(&self, entry: &RequestLogEntry) -> crate::error::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO request_log (tier, score, provider, model, input_tokens, output_tokens, cost_usd, latency_ms, success)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                entry.tier,
                entry.score,
                entry.provider,
                entry.model,
                entry.input_tokens,
                entry.output_tokens,
                entry.cost_usd,
                entry.latency_ms,
                entry.success as i32,
            ],
        )?;
        Ok(())
    }

    pub fn recent_requests(&self, limit: u32) -> crate::error::Result<Vec<RequestLogEntry>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, timestamp, tier, score, provider, model, input_tokens, output_tokens, cost_usd, latency_ms, success
             FROM request_log ORDER BY id DESC LIMIT ?1",
        )?;
        let entries = stmt
            .query_map(params![limit], |row| {
                Ok(RequestLogEntry {
                    id: Some(row.get(0)?),
                    timestamp: Some(row.get(1)?),
                    tier: row.get(2)?,
                    score: row.get(3)?,
                    provider: row.get(4)?,
                    model: row.get(5)?,
                    input_tokens: row.get(6)?,
                    output_tokens: row.get(7)?,
                    cost_usd: row.get(8)?,
                    latency_ms: row.get(9)?,
                    success: row.get::<_, i32>(10)? != 0,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(entries)
    }

    /// Get cost breakdown grouped by provider
    pub fn costs_by_provider(&self) -> crate::error::Result<Vec<CostBreakdown>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT provider, '' as model,
                COUNT(*) as requests,
                COALESCE(SUM(input_tokens), 0) as input_tokens,
                COALESCE(SUM(output_tokens), 0) as output_tokens,
                COALESCE(SUM(cost_usd), 0.0) as total_cost,
                COALESCE(AVG(latency_ms), 0) as avg_latency
             FROM request_log
             GROUP BY provider
             ORDER BY total_cost DESC",
        )?;
        let entries = stmt
            .query_map([], |row| {
                Ok(CostBreakdown {
                    group: row.get(0)?,
                    subgroup: row.get(1)?,
                    requests: row.get(2)?,
                    input_tokens: row.get(3)?,
                    output_tokens: row.get(4)?,
                    total_cost_usd: row.get(5)?,
                    avg_latency_ms: row.get(6)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(entries)
    }

    /// Get cost breakdown grouped by model
    pub fn costs_by_model(&self) -> crate::error::Result<Vec<CostBreakdown>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT model, provider,
                COUNT(*) as requests,
                COALESCE(SUM(input_tokens), 0) as input_tokens,
                COALESCE(SUM(output_tokens), 0) as output_tokens,
                COALESCE(SUM(cost_usd), 0.0) as total_cost,
                COALESCE(AVG(latency_ms), 0) as avg_latency
             FROM request_log
             GROUP BY model, provider
             ORDER BY total_cost DESC",
        )?;
        let entries = stmt
            .query_map([], |row| {
                Ok(CostBreakdown {
                    group: row.get(0)?,
                    subgroup: row.get(1)?,
                    requests: row.get(2)?,
                    input_tokens: row.get(3)?,
                    output_tokens: row.get(4)?,
                    total_cost_usd: row.get(5)?,
                    avg_latency_ms: row.get(6)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(entries)
    }

    /// Get daily cost totals for charting
    pub fn costs_by_day(&self, days: u32) -> crate::error::Result<Vec<DailyCost>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT date(timestamp, 'unixepoch') as day,
                COUNT(*) as requests,
                COALESCE(SUM(input_tokens), 0) as input_tokens,
                COALESCE(SUM(output_tokens), 0) as output_tokens,
                COALESCE(SUM(cost_usd), 0.0) as total_cost,
                SUM(CASE WHEN cost_usd = 0 OR cost_usd IS NULL THEN 1 ELSE 0 END) as free_requests
             FROM request_log
             WHERE timestamp >= strftime('%s', 'now', ?1)
             GROUP BY day
             ORDER BY day ASC",
        )?;
        let offset = format!("-{} days", days);
        let entries = stmt
            .query_map(params![offset], |row| {
                Ok(DailyCost {
                    date: row.get(0)?,
                    requests: row.get(1)?,
                    input_tokens: row.get(2)?,
                    output_tokens: row.get(3)?,
                    total_cost_usd: row.get(4)?,
                    free_requests: row.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(entries)
    }

    /// Filtered request query with optional provider, tier, model, date range, and failures_only
    pub fn filtered_requests(
        &self,
        limit: u32,
        provider: Option<&str>,
        tier: Option<&str>,
        failures_only: bool,
    ) -> crate::error::Result<Vec<RequestLogEntry>> {
        self.search_requests(limit, 0, provider, tier, None, None, None, None, failures_only)
    }

    /// Advanced search with full filtering, pagination, model search, and date range
    pub fn search_requests(
        &self,
        limit: u32,
        offset: u32,
        provider: Option<&str>,
        tier: Option<&str>,
        model: Option<&str>,
        from_ts: Option<i64>,
        to_ts: Option<i64>,
        search: Option<&str>,
        failures_only: bool,
    ) -> crate::error::Result<Vec<RequestLogEntry>> {
        let conn = self.conn.lock().unwrap();
        let mut sql = String::from(
            "SELECT id, timestamp, tier, score, provider, model, input_tokens, output_tokens, cost_usd, latency_ms, success
             FROM request_log WHERE 1=1"
        );
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(p) = provider {
            sql.push_str(" AND provider = ?");
            params_vec.push(Box::new(p.to_string()));
        }
        if let Some(t) = tier {
            sql.push_str(" AND tier = ?");
            params_vec.push(Box::new(t.to_string()));
        }
        if let Some(m) = model {
            sql.push_str(" AND model LIKE ?");
            params_vec.push(Box::new(format!("%{}%", m)));
        }
        if let Some(from) = from_ts {
            sql.push_str(" AND timestamp >= ?");
            params_vec.push(Box::new(from));
        }
        if let Some(to) = to_ts {
            sql.push_str(" AND timestamp <= ?");
            params_vec.push(Box::new(to));
        }
        if let Some(q) = search {
            sql.push_str(" AND (model LIKE ? OR provider LIKE ? OR tier LIKE ?)");
            let pattern = format!("%{}%", q);
            params_vec.push(Box::new(pattern.clone()));
            params_vec.push(Box::new(pattern.clone()));
            params_vec.push(Box::new(pattern));
        }
        if failures_only {
            sql.push_str(" AND success = 0");
        }
        sql.push_str(" ORDER BY id DESC LIMIT ? OFFSET ?");
        params_vec.push(Box::new(limit));
        params_vec.push(Box::new(offset));

        let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let entries = stmt
            .query_map(params_refs.as_slice(), |row| {
                Ok(RequestLogEntry {
                    id: Some(row.get(0)?),
                    timestamp: Some(row.get(1)?),
                    tier: row.get(2)?,
                    score: row.get(3)?,
                    provider: row.get(4)?,
                    model: row.get(5)?,
                    input_tokens: row.get(6)?,
                    output_tokens: row.get(7)?,
                    cost_usd: row.get(8)?,
                    latency_ms: row.get(9)?,
                    success: row.get::<_, i32>(10)? != 0,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(entries)
    }

    /// Count total matching requests (for pagination metadata)
    pub fn count_requests(
        &self,
        provider: Option<&str>,
        tier: Option<&str>,
        model: Option<&str>,
        from_ts: Option<i64>,
        to_ts: Option<i64>,
        search: Option<&str>,
        failures_only: bool,
    ) -> crate::error::Result<u64> {
        let conn = self.conn.lock().unwrap();
        let mut sql = String::from("SELECT COUNT(*) FROM request_log WHERE 1=1");
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(p) = provider {
            sql.push_str(" AND provider = ?");
            params_vec.push(Box::new(p.to_string()));
        }
        if let Some(t) = tier {
            sql.push_str(" AND tier = ?");
            params_vec.push(Box::new(t.to_string()));
        }
        if let Some(m) = model {
            sql.push_str(" AND model LIKE ?");
            params_vec.push(Box::new(format!("%{}%", m)));
        }
        if let Some(from) = from_ts {
            sql.push_str(" AND timestamp >= ?");
            params_vec.push(Box::new(from));
        }
        if let Some(to) = to_ts {
            sql.push_str(" AND timestamp <= ?");
            params_vec.push(Box::new(to));
        }
        if let Some(q) = search {
            sql.push_str(" AND (model LIKE ? OR provider LIKE ? OR tier LIKE ?)");
            let pattern = format!("%{}%", q);
            params_vec.push(Box::new(pattern.clone()));
            params_vec.push(Box::new(pattern.clone()));
            params_vec.push(Box::new(pattern));
        }
        if failures_only {
            sql.push_str(" AND success = 0");
        }

        let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let count: u64 = stmt.query_row(params_refs.as_slice(), |row| row.get(0))?;
        Ok(count)
    }

    /// Get aggregate stats for dashboard
    pub fn stats(&self) -> crate::error::Result<RequestStats> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT
                COUNT(*) as total,
                SUM(CASE WHEN success = 1 THEN 1 ELSE 0 END) as successes,
                COALESCE(SUM(cost_usd), 0.0) as total_cost,
                COALESCE(AVG(latency_ms), 0) as avg_latency,
                COALESCE(SUM(input_tokens), 0) as total_input,
                COALESCE(SUM(output_tokens), 0) as total_output
             FROM request_log",
        )?;
        let stats = stmt.query_row([], |row| {
            Ok(RequestStats {
                total_requests: row.get(0)?,
                successful_requests: row.get(1)?,
                total_cost_usd: row.get(2)?,
                avg_latency_ms: row.get(3)?,
                total_input_tokens: row.get(4)?,
                total_output_tokens: row.get(5)?,
            })
        })?;
        Ok(stats)
    }

    // --- Configuration Profiles ---

    /// Save or update a profile. If a profile with the same name exists, it's replaced.
    pub fn save_profile(&self, name: &str, description: Option<&str>, config_json: &str) -> crate::error::Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO profiles (name, description, config_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, strftime('%s', 'now'), strftime('%s', 'now'))
             ON CONFLICT(name) DO UPDATE SET
                description = excluded.description,
                config_json = excluded.config_json,
                updated_at = strftime('%s', 'now')",
            params![name, description, config_json],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// List all profiles (without full config_json for efficiency)
    pub fn list_profiles(&self) -> crate::error::Result<Vec<ProfileSummary>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, description, created_at, updated_at, is_active FROM profiles ORDER BY name"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ProfileSummary {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
                is_active: row.get::<_, i32>(5)? != 0,
            })
        })?;
        let mut result = Vec::new();
        for r in rows {
            result.push(r?);
        }
        Ok(result)
    }

    /// Get a profile's full config by name
    pub fn get_profile(&self, name: &str) -> crate::error::Result<Option<ProfileRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, description, config_json, created_at, updated_at, is_active
             FROM profiles WHERE name = ?1"
        )?;
        let result = stmt.query_row(params![name], |row| {
            Ok(ProfileRow {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                config_json: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
                is_active: row.get::<_, i32>(6)? != 0,
            })
        }).ok();
        Ok(result)
    }

    /// Update profile metadata (name and/or description)
    pub fn update_profile_meta(&self, old_name: &str, new_name: Option<&str>, description: Option<&str>) -> crate::error::Result<bool> {
        let conn = self.conn.lock().unwrap();
        let mut sql = String::from("UPDATE profiles SET updated_at = strftime('%s', 'now')");
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if let Some(n) = new_name {
            sql.push_str(", name = ?");
            params_vec.push(Box::new(n.to_string()));
        }
        if let Some(d) = description {
            sql.push_str(", description = ?");
            params_vec.push(Box::new(d.to_string()));
        }
        sql.push_str(" WHERE name = ?");
        params_vec.push(Box::new(old_name.to_string()));
        let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
        let affected = conn.execute(&sql, params_refs.as_slice())?;
        Ok(affected > 0)
    }

    /// Delete a profile by name
    pub fn delete_profile(&self, name: &str) -> crate::error::Result<bool> {
        let conn = self.conn.lock().unwrap();
        let affected = conn.execute("DELETE FROM profiles WHERE name = ?1", params![name])?;
        Ok(affected > 0)
    }

    /// Set a profile as active (deactivates all others)
    pub fn set_active_profile(&self, name: &str) -> crate::error::Result<bool> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE profiles SET is_active = 0 WHERE is_active = 1", [])?;
        let affected = conn.execute(
            "UPDATE profiles SET is_active = 1 WHERE name = ?1",
            params![name],
        )?;
        Ok(affected > 0)
    }

    /// Clear the active profile
    pub fn clear_active_profile(&self) -> crate::error::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE profiles SET is_active = 0 WHERE is_active = 1", [])?;
        Ok(())
    }

    /// Get the currently active profile name
    pub fn get_active_profile_name(&self) -> crate::error::Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT name FROM profiles WHERE is_active = 1 LIMIT 1")?;
        let result = stmt.query_row([], |row| row.get(0)).ok();
        Ok(result)
    }

    // --- Model Overrides ---

    /// Set a per-model override (upsert). Field can be: quality_tier, reasoning, vision,
    /// tool_calling, canonical_family, input_price_per_m, output_price_per_m.
    pub fn set_model_override(&self, provider: &str, model_id: &str, field: &str, value: &str) -> crate::error::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO model_overrides (provider, model_id, field, value) VALUES (?1, ?2, ?3, ?4)",
            params![provider, model_id, field, value],
        )?;
        Ok(())
    }

    /// Remove a specific override
    pub fn remove_model_override(&self, provider: &str, model_id: &str, field: &str) -> crate::error::Result<bool> {
        let conn = self.conn.lock().unwrap();
        let affected = conn.execute(
            "DELETE FROM model_overrides WHERE provider = ?1 AND model_id = ?2 AND field = ?3",
            params![provider, model_id, field],
        )?;
        Ok(affected > 0)
    }

    /// Get all overrides for a specific model
    pub fn get_model_overrides(&self, provider: &str, model_id: &str) -> crate::error::Result<Vec<(String, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT field, value FROM model_overrides WHERE provider = ?1 AND model_id = ?2"
        )?;
        let rows = stmt.query_map(params![provider, model_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut result = Vec::new();
        for r in rows { result.push(r?); }
        Ok(result)
    }

    /// Get all overrides across all models (for bulk loading on startup)
    pub fn get_all_model_overrides(&self) -> crate::error::Result<Vec<ModelOverride>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT provider, model_id, field, value FROM model_overrides ORDER BY provider, model_id"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ModelOverride {
                provider: row.get(0)?,
                model_id: row.get(1)?,
                field: row.get(2)?,
                value: row.get(3)?,
            })
        })?;
        let mut result = Vec::new();
        for r in rows { result.push(r?); }
        Ok(result)
    }

    /// Remove all overrides for a model
    pub fn clear_model_overrides(&self, provider: &str, model_id: &str) -> crate::error::Result<u64> {
        let conn = self.conn.lock().unwrap();
        let affected = conn.execute(
            "DELETE FROM model_overrides WHERE provider = ?1 AND model_id = ?2",
            params![provider, model_id],
        )?;
        Ok(affected as u64)
    }

    /// Get aggregate stats grouped by provider
    pub fn stats_by_provider(&self) -> crate::error::Result<Vec<ProviderStats>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT
                provider,
                COUNT(*) as total,
                SUM(CASE WHEN success = 1 THEN 1 ELSE 0 END) as successes,
                COALESCE(SUM(input_tokens), 0) as total_input,
                COALESCE(SUM(output_tokens), 0) as total_output,
                COALESCE(SUM(cost_usd), 0.0) as total_cost,
                COALESCE(AVG(latency_ms), 0) as avg_latency
             FROM request_log
             GROUP BY provider",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ProviderStats {
                provider: row.get(0)?,
                total_requests: row.get(1)?,
                successful_requests: row.get(2)?,
                total_input_tokens: row.get(3)?,
                total_output_tokens: row.get(4)?,
                total_cost_usd: row.get(5)?,
                avg_latency_ms: row.get(6)?,
            })
        })?;
        let mut result = Vec::new();
        for r in rows {
            result.push(r?);
        }
        Ok(result)
    }
}

#[derive(Debug, Clone)]
pub struct ProviderStats {
    pub provider: String,
    pub total_requests: u64,
    pub successful_requests: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cost_usd: f64,
    pub avg_latency_ms: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RequestLogEntry {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,
    pub tier: String,
    pub score: f64,
    pub provider: String,
    pub model: String,
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    pub cost_usd: Option<f64>,
    pub latency_ms: Option<u64>,
    pub success: bool,
}

impl RequestLogEntry {
    /// Create a new entry for logging (id/timestamp auto-generated by DB)
    pub fn new(tier: String, score: f64, provider: String, model: String) -> Self {
        Self {
            id: None, timestamp: None,
            tier, score, provider, model,
            input_tokens: None, output_tokens: None, cost_usd: None, latency_ms: None,
            success: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CostBreakdown {
    pub group: String,
    pub subgroup: String,
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_cost_usd: f64,
    pub avg_latency_ms: f64,
}

#[derive(Debug, Clone)]
pub struct DailyCost {
    pub date: String,
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_cost_usd: f64,
    pub free_requests: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProfileSummary {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub is_active: bool,
}

#[derive(Debug, Clone)]
pub struct ProfileRow {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub config_json: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub is_active: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ModelOverride {
    pub provider: String,
    pub model_id: String,
    pub field: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct RequestStats {
    pub total_requests: u64,
    pub successful_requests: u64,
    pub total_cost_usd: f64,
    pub avg_latency_ms: f64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kv_store() {
        let storage = Storage::in_memory().unwrap();
        assert_eq!(storage.get("missing").unwrap(), None);

        storage.set("key1", "value1").unwrap();
        assert_eq!(storage.get("key1").unwrap(), Some("value1".into()));

        // Overwrite
        storage.set("key1", "value2").unwrap();
        assert_eq!(storage.get("key1").unwrap(), Some("value2".into()));
    }

    #[test]
    fn test_provider_tokens() {
        let storage = Storage::in_memory().unwrap();

        storage
            .save_token("copilot", "ghu_abc123", Some(1700000000))
            .unwrap();

        let (token, expires) = storage.get_token("copilot").unwrap().unwrap();
        assert_eq!(token, "ghu_abc123");
        assert_eq!(expires, Some(1700000000));

        assert!(storage.get_token("nonexistent").unwrap().is_none());
    }

    #[test]
    fn test_quota_state() {
        let storage = Storage::in_memory().unwrap();

        storage
            .save_quota("copilot", Some("gpt-4o"), 5, 0.0, 1700000000)
            .unwrap();

        let (used, spent, start) = storage
            .get_quota("copilot", Some("gpt-4o"))
            .unwrap()
            .unwrap();
        assert_eq!(used, 5);
        assert_eq!(spent, 0.0);
        assert_eq!(start, 1700000000);
    }

    #[test]
    fn test_request_logging() {
        let storage = Storage::in_memory().unwrap();

        storage
            .log_request(&RequestLogEntry {
                id: None, timestamp: None,
                tier: "SIMPLE".into(),
                score: 0.05,
                provider: "ollama".into(),
                model: "phi4-mini".into(),
                input_tokens: Some(15),
                output_tokens: Some(3),
                cost_usd: Some(0.0),
                latency_ms: Some(120),
                success: true,
            })
            .unwrap();

        storage
            .log_request(&RequestLogEntry {
                id: None, timestamp: None,
                tier: "COMPLEX".into(),
                score: 0.45,
                provider: "openrouter".into(),
                model: "claude-sonnet-4".into(),
                input_tokens: Some(500),
                output_tokens: Some(200),
                cost_usd: Some(0.0045),
                latency_ms: Some(2300),
                success: true,
            })
            .unwrap();

        let recent = storage.recent_requests(10).unwrap();
        assert_eq!(recent.len(), 2);
        // Most recent first
        assert_eq!(recent[0].provider, "openrouter");
        assert_eq!(recent[1].provider, "ollama");

        let stats = storage.stats().unwrap();
        assert_eq!(stats.total_requests, 2);
        assert_eq!(stats.successful_requests, 2);
        assert!((stats.total_cost_usd - 0.0045).abs() < 0.0001);
    }

    #[test]
    fn test_profiles_crud() {
        let storage = Storage::in_memory().unwrap();
        let config = r#"{"providers":{"copilot":{"key":"test"}},"priorities":{}}"#;

        // Create
        storage.save_profile("dev", Some("Development setup"), config).unwrap();
        storage.save_profile("prod", None, config).unwrap();

        // List
        let profiles = storage.list_profiles().unwrap();
        assert_eq!(profiles.len(), 2);

        // Get
        let p = storage.get_profile("dev").unwrap().unwrap();
        assert_eq!(p.name, "dev");
        assert_eq!(p.description, Some("Development setup".to_string()));
        assert_eq!(p.config_json, config);
        assert!(!p.is_active);

        // Update metadata
        assert!(storage.update_profile_meta("dev", Some("development"), Some("Updated")).unwrap());
        let p = storage.get_profile("development").unwrap().unwrap();
        assert_eq!(p.description, Some("Updated".to_string()));
        assert!(storage.get_profile("dev").unwrap().is_none());

        // Active profile
        assert!(storage.get_active_profile_name().unwrap().is_none());
        assert!(storage.set_active_profile("prod").unwrap());
        assert_eq!(storage.get_active_profile_name().unwrap(), Some("prod".to_string()));

        // Setting another active clears the first
        assert!(storage.set_active_profile("development").unwrap());
        assert_eq!(storage.get_active_profile_name().unwrap(), Some("development".to_string()));
        let profiles = storage.list_profiles().unwrap();
        let active_count = profiles.iter().filter(|p| p.is_active).count();
        assert_eq!(active_count, 1);

        // Clear active
        storage.clear_active_profile().unwrap();
        assert!(storage.get_active_profile_name().unwrap().is_none());

        // Delete
        assert!(storage.delete_profile("prod").unwrap());
        assert!(!storage.delete_profile("nonexistent").unwrap());
        assert_eq!(storage.list_profiles().unwrap().len(), 1);

        // Upsert (save_profile with same name updates)
        storage.save_profile("development", Some("v2"), r#"{"new":true}"#).unwrap();
        let p = storage.get_profile("development").unwrap().unwrap();
        assert_eq!(p.description, Some("v2".to_string()));
        assert!(p.config_json.contains("new"));
    }

    #[test]
    fn test_model_overrides() {
        let storage = Storage::in_memory().unwrap();

        // Set overrides
        storage.set_model_override("copilot", "gpt-4o", "quality_tier", "reasoning").unwrap();
        storage.set_model_override("copilot", "gpt-4o", "vision", "false").unwrap();
        storage.set_model_override("openai", "gpt-4.1", "canonical_family", "gpt-4.1").unwrap();

        // Get per-model
        let overrides = storage.get_model_overrides("copilot", "gpt-4o").unwrap();
        assert_eq!(overrides.len(), 2);

        // Get all
        let all = storage.get_all_model_overrides().unwrap();
        assert_eq!(all.len(), 3);

        // Upsert
        storage.set_model_override("copilot", "gpt-4o", "quality_tier", "complex").unwrap();
        let overrides = storage.get_model_overrides("copilot", "gpt-4o").unwrap();
        let tier = overrides.iter().find(|(f, _)| f == "quality_tier").unwrap();
        assert_eq!(tier.1, "complex");

        // Remove specific
        assert!(storage.remove_model_override("copilot", "gpt-4o", "vision").unwrap());
        assert!(!storage.remove_model_override("copilot", "gpt-4o", "nonexistent").unwrap());
        assert_eq!(storage.get_model_overrides("copilot", "gpt-4o").unwrap().len(), 1);

        // Clear all for model
        assert_eq!(storage.clear_model_overrides("copilot", "gpt-4o").unwrap(), 1);
        assert_eq!(storage.get_model_overrides("copilot", "gpt-4o").unwrap().len(), 0);
    }
}
