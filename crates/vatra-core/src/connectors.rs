//! Connectors set up in the app: an account or integration with its settings as JSON. Their secrets are never here;
//! they stay in the keychain.

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::store::{Store, StoreError};

pub(crate) const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS connectors (
        id TEXT PRIMARY KEY,
        kind TEXT NOT NULL,
        name TEXT NOT NULL,
        settings_json TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );
";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConnectorRow {
    pub id: String,
    pub kind: String,
    pub name: String,
    /// The connector's settings as JSON, without any secret.
    pub settings_json: String,
    pub created_at: i64,
}

impl Store {
    /// Adds the connector, or replaces the one with the same id. A replaced connector keeps its `created_at`.
    pub fn save_connector(&self, row: &ConnectorRow) -> Result<(), StoreError> {
        self.connection().execute(
            "INSERT INTO connectors (id, kind, name, settings_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET kind = excluded.kind, name = excluded.name, settings_json = excluded.settings_json",
            params![row.id, row.kind, row.name, row.settings_json, row.created_at],
        )?;
        Ok(())
    }

    /// The connectors in the order they were created.
    pub fn connectors(&self) -> Result<Vec<ConnectorRow>, StoreError> {
        let connection = self.connection();
        let mut statement = connection
            .prepare("SELECT id, kind, name, settings_json, created_at FROM connectors ORDER BY created_at, id")?;
        let rows = statement.query_map([], |row| {
            Ok(ConnectorRow {
                id: row.get(0)?,
                kind: row.get(1)?,
                name: row.get(2)?,
                settings_json: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn remove_connector(&self, id: &str) -> Result<bool, StoreError> {
        Ok(self.connection().execute("DELETE FROM connectors WHERE id = ?1", [id])? > 0)
    }
}
