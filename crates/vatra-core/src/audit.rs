//! The audit log of actions with an effect outside the computer. It keeps the tool, the connector, a short summary
//! and a hash of the arguments, never the content of a message or any other body text.

use rusqlite::{ErrorCode, params};
use serde::{Deserialize, Serialize};

use crate::store::{Store, StoreError};

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS audit_log (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 at INTEGER NOT NULL,
 session_id INTEGER,
 tool TEXT NOT NULL,
 connector TEXT NOT NULL,
 summary TEXT NOT NULL,
 decision TEXT NOT NULL CHECK (decision IN ('approved','denied','blocked')),
 payload_hash TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS audit_log_connector ON audit_log(connector, at);
";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Unix seconds.
    pub at: i64,
    pub session_id: Option<i64>,
    pub tool: String,
    pub connector: String,
    pub summary: String,
    /// `approved`, `denied` or `blocked`.
    pub decision: String,
    /// The SHA-256 of the arguments, in lowercase hex.
    pub payload_hash: String,
}

/// The most characters of a summary that the log keeps.
const SUMMARY_LIMIT: usize = 200;

impl Store {
    /// Atomically checks the connector's quota and records an approval. False means the quota was exhausted.
    /// The conditional INSERT is one SQLite write transaction, including with independent Store connections.
    pub fn try_record_external_approval(&self, entry: &AuditEntry, since: i64, limit: u32) -> Result<bool, StoreError> {
        if entry.decision != "approved" {
            return Err(StoreError::Invalid("Only an approved action can reserve the external quota.".into()));
        }
        let summary: String = entry.summary.chars().take(SUMMARY_LIMIT).collect();
        let inserted = self.connection().execute(
            "INSERT INTO audit_log (at, session_id, tool, connector, summary, decision, payload_hash)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7
             WHERE (SELECT COUNT(*) FROM audit_log WHERE connector = ?4 AND decision = 'approved' AND at >= ?8) < ?9",
            params![entry.at, entry.session_id, entry.tool, entry.connector, summary, entry.decision, entry.payload_hash, since, limit],
        )?;
        Ok(inserted == 1)
    }

    /// Adds an entry. A decision other than approved, denied or blocked is refused.
    /// The summary is cut to `SUMMARY_LIMIT` characters, so a long one can't turn the log into a copy of the content.
    pub fn record_audit(&self, entry: &AuditEntry) -> Result<(), StoreError> {
        let summary: String = entry.summary.chars().take(SUMMARY_LIMIT).collect();
        let result = self.connection().execute(
            "INSERT INTO audit_log (at, session_id, tool, connector, summary, decision, payload_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                entry.at,
                entry.session_id,
                entry.tool,
                entry.connector,
                summary,
                entry.decision,
                entry.payload_hash
            ],
        );
        match result {
            Ok(_) => Ok(()),
            Err(rusqlite::Error::SqliteFailure(failure, _)) if failure.code == ErrorCode::ConstraintViolation => {
                Err(StoreError::Invalid(format!("an audit decision must be approved, denied or blocked, not `{}`", entry.decision)))
            }
            Err(error) => Err(error.into()),
        }
    }

    /// The newest entries first, at most `limit` of them.
    pub fn audit_recent(&self, limit: usize) -> Result<Vec<AuditEntry>, StoreError> {
        let connection = self.connection();
        let mut statement = connection.prepare(
            "SELECT at, session_id, tool, connector, summary, decision, payload_hash
             FROM audit_log ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = statement.query_map([i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
            Ok(AuditEntry {
                at: row.get(0)?,
                session_id: row.get(1)?,
                tool: row.get(2)?,
                connector: row.get(3)?,
                summary: row.get(4)?,
                decision: row.get(5)?,
                payload_hash: row.get(6)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// How many actions on a connector were approved at or after `since` (unix seconds).
    pub fn external_count_since(&self, connector: &str, since: i64) -> Result<u32, StoreError> {
        let count: i64 = self.connection().query_row(
            "SELECT COUNT(*) FROM audit_log WHERE connector = ?1 AND decision = 'approved' AND at >= ?2",
            params![connector, since],
            |row| row.get(0),
        )?;
        Ok(u32::try_from(count).unwrap_or(u32::MAX))
    }
}
