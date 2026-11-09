use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::event::{Event, StoredEvent};

const SCHEMA_VERSION: i64 = 15;
/// How long a write waits for another connection to the same file, such as the extension catalog's.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const TITLE_MAX_CHARS: usize = 60;
const UNTITLED: &str = "New session";

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS projects (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        name TEXT NOT NULL UNIQUE
    );
    CREATE TABLE IF NOT EXISTS sessions (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        project_id INTEGER REFERENCES projects(id),
        agent TEXT NOT NULL,
        native_session TEXT
    );
    CREATE TABLE IF NOT EXISTS events (
        seq INTEGER PRIMARY KEY AUTOINCREMENT,
        session_id INTEGER NOT NULL REFERENCES sessions(id),
        payload TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS events_by_session ON events(session_id, seq);
";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{0}")]
    Invalid(String),
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("invalid event: {0}")]
    Json(#[from] serde_json::Error),
    #[error("this database is from a newer version ({0}); update Vatra")]
    NewerSchema(i64),
    #[error("file error: {0}")]
    Io(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: i64,
    pub name: String,
    pub path: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LastTurn {
    None,
    Completed,
    Failed,
    Unfinished,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: i64,
    pub project_id: Option<i64>,
    pub agent: String,
    pub title: String,
    pub updated_seq: i64,
    pub last_turn: LastTurn,
}

pub trait IntoProjectId {
    fn into_project_id(self) -> Option<i64>;
}

impl IntoProjectId for i64 {
    fn into_project_id(self) -> Option<i64> {
        Some(self)
    }
}

impl IntoProjectId for Option<i64> {
    fn into_project_id(self) -> Option<i64> {
        self
    }
}

impl LastTurn {
    fn as_str(self) -> &'static str {
        match self {
            LastTurn::None => "none",
            LastTurn::Completed => "completed",
            LastTurn::Failed => "failed",
            LastTurn::Unfinished => "unfinished",
        }
    }

    fn parse(text: &str) -> LastTurn {
        match text {
            "completed" => LastTurn::Completed,
            "failed" => LastTurn::Failed,
            "unfinished" => LastTurn::Unfinished,
            _ => LastTurn::None,
        }
    }

    /// The state an event leaves its session in, if it changes it.
    fn after(event: &Event) -> Option<LastTurn> {
        match event {
            Event::UserMessage { .. } => Some(LastTurn::Unfinished),
            Event::TurnCompleted { .. } => Some(LastTurn::Completed),
            Event::TurnFailed { .. } => Some(LastTurn::Failed),
            _ => None,
        }
    }
}

fn title_of(text: &str) -> String {
    text.chars().take(TITLE_MAX_CHARS).collect()
}

/// Keeps the listing columns current, so showing the session list never parses event payloads.
pub(crate) fn update_summary(
    connection: &Connection,
    session_id: i64,
    event: &Event,
) -> Result<(), StoreError> {
    if let Some(turn) = LastTurn::after(event) {
        connection.execute(
            "UPDATE sessions SET last_turn = ?1 WHERE id = ?2",
            params![turn.as_str(), session_id],
        )?;
    }
    if let Event::UserMessage { text } = event {
        connection.execute(
            "UPDATE sessions SET title = COALESCE(title, ?1) WHERE id = ?2",
            params![title_of(text), session_id],
        )?;
    }
    Ok(())
}

fn migrate(connection: &mut Connection, version: i64) -> Result<(), StoreError> {
    let transaction = connection.transaction()?;
    if version < 1 {
        transaction.execute_batch("ALTER TABLE projects ADD COLUMN path TEXT;")?;
    }
    if version < 2 {
        transaction.execute_batch(
            "ALTER TABLE sessions ADD COLUMN title TEXT;
             ALTER TABLE sessions ADD COLUMN last_turn TEXT NOT NULL DEFAULT 'none';",
        )?;
        let events: Vec<(i64, String)> = {
            let mut statement =
                transaction.prepare("SELECT session_id, payload FROM events ORDER BY seq")?;
            let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect::<Result<_, _>>()?
        };
        for (session_id, payload) in events {
            if let Ok(event) = serde_json::from_str::<Event>(&payload) {
                update_summary(&transaction, session_id, &event)?;
            }
        }
    }
    if version < 3 {
        transaction.execute_batch(crate::history::SCHEMA)?;
        let mut statement =
            transaction.prepare("SELECT session_id,payload FROM events ORDER BY seq")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (session, payload) = row?;
            if let Ok(event) = serde_json::from_str::<Event>(&payload) {
                crate::history::index_event(&transaction, session, &event)?;
            }
        }
    }
    if version < 4 {
        transaction.execute_batch(crate::local_models::SCHEMA)?;
    }
    if version < 5 {
        transaction.execute_batch(crate::app_tables::SCHEMA)?;
    }
    if version < 6 {
        transaction.execute_batch(crate::workspace::SCHEMA)?;
        transaction.execute_batch(crate::audit::SCHEMA)?;
        transaction.execute_batch(crate::connectors::SCHEMA)?;
    }
    if version < 7 {
        transaction.execute_batch(crate::workspace_versions::SCHEMA_V7)?;
    }
    if version < 8 {
        transaction.execute_batch(
            "CREATE TABLE IF NOT EXISTS sessions_v8 (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                project_id INTEGER REFERENCES projects(id),
                agent TEXT NOT NULL,
                native_session TEXT,
                title TEXT,
                last_turn TEXT NOT NULL DEFAULT 'none'
            );
            INSERT INTO sessions_v8 (id, project_id, agent, native_session, title, last_turn)
                SELECT id, project_id, agent, native_session, title, last_turn FROM sessions;
            DROP TABLE sessions;
            ALTER TABLE sessions_v8 RENAME TO sessions;
            CREATE INDEX IF NOT EXISTS events_by_session ON events(session_id, seq);"
        )?;
    }
    if version < 9 {
        transaction.execute_batch(crate::files::SCHEMA)?;
    }
    if version < 10 {
        transaction.execute_batch(crate::schedule::SCHEMA)?;
    }
    if version < 11 {
        transaction.execute_batch(crate::email_store::SCHEMA)?;
    }
    if version < 12 {
        transaction.execute_batch(crate::research_store::SCHEMA)?;
    }
    if version < 13 {
        // A copy that was migrated before may already have a column, so each one is added only when it is missing.
        for (table, column, definition) in crate::schedule::COLUMNS_V13.iter().chain(crate::research_store::COLUMNS_V13.iter()) {
            let exists: bool = transaction.query_row(
                &format!("SELECT EXISTS (SELECT 1 FROM pragma_table_info('{table}') WHERE name = '{column}')"),
                [],
                |row| row.get(0),
            )?;
            if !exists {
                transaction.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {definition};"))?;
            }
        }
        transaction.execute_batch(crate::email_store::SCHEMA_V13)?;
    }
    if version < 14 {
        transaction.execute_batch(crate::semantic::SCHEMA)?;
    }
    if version < 15 {
        transaction.execute_batch(crate::user_memory::SCHEMA)?;
    }
    transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(())
}

pub struct Store {
    connection: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store, StoreError> {
        let connection = Connection::open(path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(connection)
    }

    /// The store of a database, read only: nothing is created or changed. A dry run reads through it.
    pub fn open_read_only(path: &Path) -> Result<Store, StoreError> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        connection.busy_timeout(BUSY_TIMEOUT)?;
        Ok(Store {
            connection: Mutex::new(connection),
        })
    }

    pub fn open_in_memory() -> Result<Store, StoreError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut connection: Connection) -> Result<Store, StoreError> {
        connection.busy_timeout(BUSY_TIMEOUT)?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(StoreError::NewerSchema(version));
        }
        if version < 8 {
            connection.pragma_update(None, "foreign_keys", "OFF")?;
        }
        connection.execute_batch(SCHEMA)?;
        connection.execute_batch(crate::local_models::SCHEMA)?;
        connection.execute_batch(crate::app_tables::SCHEMA)?;
        connection.execute_batch(crate::workspace::SCHEMA)?;
        connection.execute_batch(crate::audit::SCHEMA)?;
        connection.execute_batch(crate::connectors::SCHEMA)?;
        if version < SCHEMA_VERSION {
            migrate(&mut connection, version)?;
        }
        connection.pragma_update(None, "foreign_keys", "ON")?;
        Ok(Store {
            connection: Mutex::new(connection),
        })
    }

    pub(crate) fn connection(&self) -> MutexGuard<'_, Connection> {
        self.connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn create_project(&self, name: &str) -> Result<i64, StoreError> {
        let connection = self.connection();
        connection.execute("INSERT OR IGNORE INTO projects (name) VALUES (?1)", [name])?;
        Ok(
            connection.query_row("SELECT id FROM projects WHERE name = ?1", [name], |row| {
                row.get(0)
            })?,
        )
    }

    pub fn set_project_path(&self, id: i64, path: Option<&str>) -> Result<(), StoreError> {
        self.connection().execute(
            "UPDATE projects SET path = ?1 WHERE id = ?2",
            params![path, id],
        )?;
        Ok(())
    }

    pub fn list_projects(&self) -> Result<Vec<Project>, StoreError> {
        let connection = self.connection();
        let mut statement =
            connection.prepare("SELECT id, name, path FROM projects ORDER BY name")?;
        let rows = statement.query_map([], |row| {
            Ok(Project {
                id: row.get(0)?,
                name: row.get(1)?,
                path: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn create_session(&self, project_id: impl IntoProjectId, agent: &str) -> Result<i64, StoreError> {
        let project_id = project_id.into_project_id();
        let connection = self.connection();
        connection.execute(
            "INSERT INTO sessions (project_id, agent) VALUES (?1, ?2)",
            params![project_id, agent],
        )?;
        Ok(connection.last_insert_rowid())
    }

    /// Deletes a conversation with its events and the suggestions and turn context that belong to it. The audit log
    /// keeps its entries. Returns false when there is no such conversation.
    pub fn delete_session(&self, session_id: i64) -> Result<bool, StoreError> {
        let mut connection = self.connection();
        let transaction = connection.transaction()?;
        for table in ["events", "workspace_proposals", "workspace_turn_contexts"] {
            transaction.execute(&format!("DELETE FROM {table} WHERE session_id = ?1"), [session_id])?;
        }
        let removed = transaction.execute("DELETE FROM sessions WHERE id = ?1", [session_id])?;
        transaction.commit()?;
        Ok(removed > 0)
    }

    pub fn session_agent(&self, session_id: i64) -> Result<String, StoreError> {
        Ok(self.connection().query_row(
            "SELECT agent FROM sessions WHERE id = ?1",
            [session_id],
            |row| row.get(0),
        )?)
    }
    pub fn rename_session(&self,session:i64,title:&str)->Result<(),StoreError>{
        let title=title.trim();
        if title.is_empty()||title.chars().count()>120||title.chars().any(char::is_control){return Err(StoreError::Invalid("Choose a conversation title of 1–120 characters.".into()));}
        if self.connection().execute("UPDATE sessions SET title=?2 WHERE id=?1",params![session,title])?!=1{return Err(StoreError::Invalid("Conversation not found.".into()));}
        Ok(())
    }

    pub fn session_project(&self, session_id: i64) -> Result<Option<i64>, StoreError> {
        Ok(self.connection().query_row(
            "SELECT project_id FROM sessions WHERE id = ?1",
            [session_id],
            |row| row.get(0),
        )?)
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionInfo>, StoreError> {
        let connection = self.connection();
        let mut statement = connection.prepare(
            "SELECT s.id, s.project_id, s.agent, s.title, s.last_turn,
                    COALESCE((SELECT MAX(e.seq) FROM events e WHERE e.session_id = s.id), 0) AS updated
             FROM sessions s ORDER BY updated DESC, s.id DESC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(SessionInfo {
                id: row.get(0)?,
                project_id: row.get(1)?,
                agent: row.get(2)?,
                title: row
                    .get::<_, Option<String>>(3)?
                    .unwrap_or_else(|| UNTITLED.to_string()),
                last_turn: LastTurn::parse(&row.get::<_, String>(4)?),
                updated_seq: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn unfinished_sessions(&self) -> Result<Vec<i64>, StoreError> {
        Ok(self
            .list_sessions()?
            .into_iter()
            .filter(|session| session.last_turn == LastTurn::Unfinished)
            .map(|session| session.id)
            .collect())
    }

    pub fn append(&self, session_id: i64, event: &Event) -> Result<i64, StoreError> {
        let payload = serde_json::to_string(event)?;
        let connection = self.connection();
        let transaction = connection.unchecked_transaction()?;
        transaction.execute(
            "INSERT INTO events (session_id, payload) VALUES (?1, ?2)",
            params![session_id, payload],
        )?;
        let seq = transaction.last_insert_rowid();
        update_summary(&transaction, session_id, event)?;
        crate::history::index_event(&transaction, session_id, event)?;
        transaction.commit()?;
        Ok(seq)
    }

    pub fn events(&self, session_id: i64) -> Result<Vec<StoredEvent>, StoreError> {
        self.events_after(session_id, 0)
    }

    pub fn events_after(
        &self,
        session_id: i64,
        after_seq: i64,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        let connection = self.connection();
        let mut statement = connection.prepare(
            "SELECT seq, payload FROM events WHERE session_id = ?1 AND seq > ?2 ORDER BY seq",
        )?;
        let rows = statement.query_map(params![session_id, after_seq], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.map(|row| {
            let (seq, payload) = row?;
            Ok(StoredEvent {
                seq,
                session_id,
                event: serde_json::from_str(&payload)?,
            })
        })
        .collect()
    }

    pub fn native_session(&self, session_id: i64) -> Result<Option<String>, StoreError> {
        Ok(self
            .connection()
            .query_row(
                "SELECT native_session FROM sessions WHERE id = ?1",
                [session_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
    }

    pub fn set_native_session(&self, session_id: i64, native: &str) -> Result<(), StoreError> {
        self.connection().execute(
            "UPDATE sessions SET native_session = ?1 WHERE id = ?2",
            params![native, session_id],
        )?;
        Ok(())
    }
}
