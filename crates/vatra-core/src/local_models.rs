//! Local models Vatra knows about, and the results of their short checks. The details stay JSON:
//! their shape belongs to `vatra-models`, which this crate doesn't depend on.

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::store::{Store, StoreError};

pub(crate) const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS local_models (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        slug TEXT NOT NULL UNIQUE,
        repo TEXT,
        variant TEXT,
        files_json TEXT NOT NULL,
        sha256_json TEXT NOT NULL,
        path TEXT NOT NULL,
        size_bytes INTEGER NOT NULL,
        metadata_json TEXT,
        added_by TEXT NOT NULL,
        settings_json TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS model_checks (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        agent TEXT NOT NULL,
        started_at INTEGER NOT NULL,
        finished_at INTEGER NOT NULL,
        llama_build INTEGER,
        settings_json TEXT NOT NULL,
        results_json TEXT NOT NULL,
        label TEXT NOT NULL,
        consented_cost INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS model_checks_by_agent ON model_checks(agent, id);
";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LocalModelRow {
    pub slug: String,
    pub repo: Option<String>,
    pub variant: Option<String>,
    pub files_json: String,
    pub sha256_json: String,
    /// The main file: the first part of a split model.
    pub path: String,
    pub size_bytes: u64,
    pub metadata_json: Option<String>,
    /// `download` or `path`.
    pub added_by: String,
    pub settings_json: String,
    /// Unix seconds.
    pub created_at: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelCheckRow {
    pub agent: String,
    pub started_at: i64,
    pub finished_at: i64,
    pub llama_build: Option<u32>,
    pub settings_json: String,
    pub results_json: String,
    pub label: String,
    pub consented_cost: bool,
}

const MODEL_COLUMNS: &str = "slug, repo, variant, files_json, sha256_json, path, size_bytes, metadata_json, added_by, settings_json, created_at";

fn model_row(row: &rusqlite::Row) -> rusqlite::Result<LocalModelRow> {
    Ok(LocalModelRow {
        slug: row.get(0)?,
        repo: row.get(1)?,
        variant: row.get(2)?,
        files_json: row.get(3)?,
        sha256_json: row.get(4)?,
        path: row.get(5)?,
        size_bytes: row.get::<_, i64>(6)? as u64,
        metadata_json: row.get(7)?,
        added_by: row.get(8)?,
        settings_json: row.get(9)?,
        created_at: row.get(10)?,
    })
}

impl Store {
    /// Fails when the slug is taken.
    pub fn insert_local_model(&self, model: &LocalModelRow) -> Result<(), StoreError> {
        self.connection().execute(
            &format!("INSERT INTO local_models ({MODEL_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"),
            params![
                model.slug,
                model.repo,
                model.variant,
                model.files_json,
                model.sha256_json,
                model.path,
                model.size_bytes as i64,
                model.metadata_json,
                model.added_by,
                model.settings_json,
                model.created_at
            ],
        )?;
        Ok(())
    }

    pub fn local_models(&self) -> Result<Vec<LocalModelRow>, StoreError> {
        let connection = self.connection();
        let mut statement = connection.prepare(&format!("SELECT {MODEL_COLUMNS} FROM local_models ORDER BY id"))?;
        let rows = statement.query_map([], model_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn local_model(&self, slug: &str) -> Result<Option<LocalModelRow>, StoreError> {
        Ok(self
            .connection()
            .query_row(&format!("SELECT {MODEL_COLUMNS} FROM local_models WHERE slug = ?1"), [slug], model_row)
            .optional()?)
    }

    pub fn set_local_model_settings(&self, slug: &str, settings_json: &str) -> Result<(), StoreError> {
        self.connection()
            .execute("UPDATE local_models SET settings_json = ?1 WHERE slug = ?2", params![settings_json, slug])?;
        Ok(())
    }

    /// Removes the record and the checks made for it. Returns whether a model was removed.
    pub fn remove_local_model(&self, slug: &str, agent: &str) -> Result<bool, StoreError> {
        let mut connection = self.connection();
        let transaction = connection.transaction()?;
        let removed = transaction.execute("DELETE FROM local_models WHERE slug = ?1", [slug])? > 0;
        transaction.execute("DELETE FROM model_checks WHERE agent = ?1", [agent])?;
        transaction.commit()?;
        Ok(removed)
    }

    pub fn insert_model_check(&self, check: &ModelCheckRow) -> Result<i64, StoreError> {
        let connection = self.connection();
        connection.execute(
            "INSERT INTO model_checks (agent, started_at, finished_at, llama_build, settings_json, results_json, label, consented_cost)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                check.agent,
                check.started_at,
                check.finished_at,
                check.llama_build,
                check.settings_json,
                check.results_json,
                check.label,
                check.consented_cost
            ],
        )?;
        Ok(connection.last_insert_rowid())
    }

    pub fn latest_model_check(&self, agent: &str) -> Result<Option<ModelCheckRow>, StoreError> {
        Ok(self
            .connection()
            .query_row(
                "SELECT agent, started_at, finished_at, llama_build, settings_json, results_json, label, consented_cost
                 FROM model_checks WHERE agent = ?1 ORDER BY id DESC LIMIT 1",
                [agent],
                |row| {
                    Ok(ModelCheckRow {
                        agent: row.get(0)?,
                        started_at: row.get(1)?,
                        finished_at: row.get(2)?,
                        llama_build: row.get(3)?,
                        settings_json: row.get(4)?,
                        results_json: row.get(5)?,
                        label: row.get(6)?,
                        consented_cost: row.get(7)?,
                    })
                },
            )
            .optional()?)
    }
}
