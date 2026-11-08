//! What the app keeps that used to live only in vatra.toml: agents set up in the app, small settings such as the
//! summarizer and whether setup was finished, and parity runs. Secrets never come here; they stay in the keychain.

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::store::{Store, StoreError};

pub(crate) const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS configured_agents (
        name TEXT PRIMARY KEY,
        kind TEXT NOT NULL,
        settings_json TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS app_settings (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS parity_runs (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        agent TEXT NOT NULL,
        native TEXT,
        model TEXT,
        build TEXT,
        started_at INTEGER NOT NULL,
        finished_at INTEGER NOT NULL,
        results_json TEXT NOT NULL,
        consented_cost INTEGER NOT NULL
    );
";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConfiguredAgentRow {
    pub name: String,
    /// `model`, `claude` or `acp`, as in vatra.toml.
    pub kind: String,
    /// The agent's settings in vatra.toml's shape, as JSON, without any key.
    pub settings_json: String,
    pub created_at: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParityRunRow {
    pub id: i64,
    pub agent: String,
    /// `claude-code` or `codex` when the run used the native harness instead of Vatra.
    pub native: Option<String>,
    pub model: Option<String>,
    pub build: Option<String>,
    pub started_at: i64,
    pub finished_at: i64,
    pub results_json: String,
    pub consented_cost: bool,
}

impl Store {
    /// Adds the agent, or replaces the one with the same name.
    pub fn save_configured_agent(&self, row: &ConfiguredAgentRow) -> Result<(), StoreError> {
        self.connection().execute(
            "INSERT INTO configured_agents (name, kind, settings_json, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(name) DO UPDATE SET kind = excluded.kind, settings_json = excluded.settings_json",
            params![row.name, row.kind, row.settings_json, row.created_at],
        )?;
        Ok(())
    }

    pub fn configured_agents(&self) -> Result<Vec<ConfiguredAgentRow>, StoreError> {
        let connection = self.connection();
        let mut statement =
            connection.prepare("SELECT name, kind, settings_json, created_at FROM configured_agents ORDER BY created_at, name")?;
        let rows = statement.query_map([], |row| {
            Ok(ConfiguredAgentRow { name: row.get(0)?, kind: row.get(1)?, settings_json: row.get(2)?, created_at: row.get(3)? })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn remove_configured_agent(&self, name: &str) -> Result<bool, StoreError> {
        Ok(self.connection().execute("DELETE FROM configured_agents WHERE name = ?1", [name])? > 0)
    }

    pub fn app_setting(&self, key: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .connection()
            .query_row("SELECT value FROM app_settings WHERE key = ?1", [key], |row| row.get(0))
            .optional()?)
    }

    /// Sets a value, or removes it when `value` is None.
    pub fn set_app_setting(&self, key: &str, value: Option<&str>) -> Result<(), StoreError> {
        let connection = self.connection();
        match value {
            Some(value) => connection.execute(
                "INSERT INTO app_settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )?,
            None => connection.execute("DELETE FROM app_settings WHERE key = ?1", [key])?,
        };
        Ok(())
    }

    pub fn insert_parity_run(&self, row: &ParityRunRow) -> Result<i64, StoreError> {
        let connection = self.connection();
        connection.execute(
            "INSERT INTO parity_runs (agent, native, model, build, started_at, finished_at, results_json, consented_cost)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                row.agent,
                row.native,
                row.model,
                row.build,
                row.started_at,
                row.finished_at,
                row.results_json,
                row.consented_cost
            ],
        )?;
        Ok(connection.last_insert_rowid())
    }

    /// The newest runs first.
    pub fn parity_runs(&self, limit: usize) -> Result<Vec<ParityRunRow>, StoreError> {
        let connection = self.connection();
        let mut statement = connection.prepare(
            "SELECT id, agent, native, model, build, started_at, finished_at, results_json, consented_cost
             FROM parity_runs ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = statement.query_map([limit as i64], |row| {
            Ok(ParityRunRow {
                id: row.get(0)?,
                agent: row.get(1)?,
                native: row.get(2)?,
                model: row.get(3)?,
                build: row.get(4)?,
                started_at: row.get(5)?,
                finished_at: row.get(6)?,
                results_json: row.get(7)?,
                consented_cost: row.get(8)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}
