use crate::{Event, Store, StoreError};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS imported_sessions (
 id INTEGER PRIMARY KEY, origin TEXT NOT NULL, native_id TEXT NOT NULL,
 project_id INTEGER REFERENCES projects(id), cwd TEXT, title TEXT NOT NULL, started_at TEXT NOT NULL,
 UNIQUE(origin,native_id));
CREATE TABLE IF NOT EXISTS imported_messages (
 session_id INTEGER NOT NULL REFERENCES imported_sessions(id) ON DELETE CASCADE,
 role TEXT NOT NULL,text TEXT NOT NULL,at TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS imported_messages_session ON imported_messages(session_id);
CREATE VIRTUAL TABLE IF NOT EXISTS history_fts USING fts5(
 kind UNINDEXED, session_id UNINDEXED, project_id UNINDEXED, title UNINDEXED, role UNINDEXED, text, at UNINDEXED,
 tokenize='unicode61 remove_diacritics 2');";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum SessionRef {
    Vatra(i64),
    Imported(i64),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistoryHit {
    pub session: SessionRef,
    pub title: String,
    pub snippet: String,
    pub at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistoryMessage {
    pub role: String,
    pub text: String,
    pub at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImportedSessionInfo {
    pub id: i64,
    pub origin: String,
    pub native_id: String,
    pub project_id: Option<i64>,
    pub cwd: Option<String>,
    pub title: String,
    pub started_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImportedSession {
    #[serde(flatten)]
    pub info: ImportedSessionInfo,
    pub messages: Vec<HistoryMessage>,
}

pub(crate) fn index_event(
    connection: &Connection,
    session: i64,
    event: &Event,
) -> Result<(), StoreError> {
    let (role, text) = match event {
        Event::UserMessage { text } => ("user", text),
        Event::AssistantText { text } => ("assistant", text),
        _ => return Ok(()),
    };
    connection.execute("INSERT INTO history_fts(kind,session_id,project_id,title,role,text,at)
        SELECT 'vatra',id,project_id,COALESCE(title,'New session'),?2,?3,'' FROM sessions WHERE id=?1",
        params![session,role,text])?;
    Ok(())
}

impl Store {
    /// Searches literal words; punctuation cannot introduce FTS operators. Results remain local.
    pub fn search_history(
        &self,
        query: &str,
        project_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<HistoryHit>, StoreError> {
        let words: Vec<_> = query
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .take(64)
            .map(|s| format!("\"{s}\""))
            .collect();
        if words.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let connection = self.connection();
        let mut statement = connection.prepare(
            "SELECT kind,session_id,title,snippet(history_fts,5,'','', ' … ',24),at
            FROM history_fts WHERE history_fts MATCH ?1 AND (?2 IS NULL OR project_id=?2)
            ORDER BY rank LIMIT ?3",
        )?;
        let rows = statement.query_map(
            params![words.join(" AND "), project_id, limit.min(100) as i64],
            |row| {
                let kind: String = row.get(0)?;
                let id = row.get(1)?;
                Ok(HistoryHit {
                    session: if kind == "vatra" {
                        SessionRef::Vatra(id)
                    } else {
                        SessionRef::Imported(id)
                    },
                    title: row.get(2)?,
                    snippet: row.get(3)?,
                    at: row.get(4)?,
                })
            },
        )?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn index_message(
        &self,
        session: SessionRef,
        role: &str,
        text: &str,
    ) -> Result<(), StoreError> {
        if !["user", "assistant"].contains(&role) {
            return Ok(());
        }
        let connection = self.connection();
        match session {
            SessionRef::Vatra(id) => index_event(
                &connection,
                id,
                &if role == "user" {
                    Event::UserMessage { text: text.into() }
                } else {
                    Event::AssistantText { text: text.into() }
                },
            ),
            SessionRef::Imported(id) => {
                connection.execute("INSERT INTO history_fts(kind,session_id,project_id,title,role,text,at)
                    SELECT 'imported',id,project_id,title,?2,?3,'' FROM imported_sessions WHERE id=?1",params![id,role,text])?;
                Ok(())
            }
        }
    }

    /// Streams messages into a transaction. An error or cancellation rolls back the complete reimport.
    pub fn import_history(
        &self,
        meta: &ImportedSessionInfo,
        messages: impl Iterator<Item = Result<HistoryMessage, String>>,
    ) -> Result<i64, StoreError> {
        let connection = self.connection();
        let tx = connection.unchecked_transaction()?;
        tx.execute("INSERT INTO imported_sessions(origin,native_id,project_id,cwd,title,started_at) VALUES(?1,?2,?3,?4,?5,?6)
            ON CONFLICT(origin,native_id) DO UPDATE SET project_id=excluded.project_id,cwd=excluded.cwd,title=excluded.title,started_at=excluded.started_at",
            params![meta.origin,meta.native_id,meta.project_id,meta.cwd,meta.title,meta.started_at])?;
        let id: i64 = tx.query_row(
            "SELECT id FROM imported_sessions WHERE origin=?1 AND native_id=?2",
            params![meta.origin, meta.native_id],
            |r| r.get(0),
        )?;
        tx.execute("DELETE FROM imported_messages WHERE session_id=?1", [id])?;
        tx.execute(
            "DELETE FROM history_fts WHERE kind='imported' AND session_id=?1",
            [id],
        )?;
        {
            let mut insert = tx.prepare(
                "INSERT INTO imported_messages(session_id,role,text,at) VALUES(?1,?2,?3,?4)",
            )?;
            let mut index=tx.prepare("INSERT INTO history_fts(kind,session_id,project_id,title,role,text,at) VALUES('imported',?1,?2,?3,?4,?5,?6)")?;
            for message in messages {
                let message = message.map_err(StoreError::Invalid)?;
                if !["user", "assistant"].contains(&message.role.as_str())
                    || message.text.trim().is_empty()
                {
                    continue;
                }
                insert.execute(params![id, message.role, message.text, message.at])?;
                index.execute(params![
                    id,
                    meta.project_id,
                    meta.title,
                    message.role,
                    message.text,
                    message.at
                ])?;
            }
        }
        tx.commit()?;
        Ok(id)
    }

    pub fn imported_session_messages(&self, id: i64) -> Result<Vec<HistoryMessage>, StoreError> {
        let connection = self.connection();
        let mut statement = connection.prepare(
            "SELECT role,text,at FROM imported_messages WHERE session_id=?1 ORDER BY rowid",
        )?;
        Ok(statement
            .query_map([id], |r| {
                Ok(HistoryMessage {
                    role: r.get(0)?,
                    text: r.get(1)?,
                    at: r.get(2)?,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn open_imported(&self, id: i64) -> Result<Option<ImportedSession>, StoreError> {
        let info=self.connection().query_row("SELECT id,origin,native_id,project_id,cwd,title,started_at FROM imported_sessions WHERE id=?1",[id],|r|Ok(ImportedSessionInfo {
            id:r.get(0)?,origin:r.get(1)?,native_id:r.get(2)?,project_id:r.get(3)?,cwd:r.get(4)?,title:r.get(5)?,started_at:r.get(6)?
        })).optional()?;
        info.map(|info| {
            Ok(ImportedSession {
                info,
                messages: self.imported_session_messages(id)?,
            })
        })
        .transpose()
    }
}
