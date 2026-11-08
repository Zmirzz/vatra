//! Versioned, project-scoped workspace operations. Every read-before-write holds a SQLite write transaction.
use crate::workspace::{ITEM_COLUMNS, index_item, item_at};
use crate::{ITEM_KINDS, Item, ItemHit, Store, StoreError};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) const SCHEMA_V7: &str = r#"
ALTER TABLE items ADD COLUMN revision INTEGER NOT NULL DEFAULT 1 CHECK(revision > 0);
CREATE TABLE IF NOT EXISTS workspace_proposals (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 project_id INTEGER NOT NULL REFERENCES projects(id),
 session_id INTEGER NOT NULL REFERENCES sessions(id),
 client_op_id TEXT NOT NULL,
 target_id INTEGER REFERENCES items(id),
 base_revision INTEGER,
 payload_json TEXT NOT NULL,
 state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','applied','rejected')),
 created_at INTEGER NOT NULL,
 applied_item_ids_json TEXT NOT NULL DEFAULT '[]',
 UNIQUE(session_id,client_op_id));
CREATE TABLE IF NOT EXISTS workspace_turn_contexts (
 session_id INTEGER PRIMARY KEY REFERENCES sessions(id),
 project_id INTEGER NOT NULL REFERENCES projects(id),
 item_id INTEGER NOT NULL REFERENCES items(id),
 base_revision INTEGER NOT NULL,
 base_body TEXT NOT NULL,
 selection_json TEXT);
CREATE TABLE IF NOT EXISTS reminders (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 item_id INTEGER NOT NULL REFERENCES items(id),
 project_id INTEGER NOT NULL REFERENCES projects(id),
 generation INTEGER NOT NULL,
 due_at INTEGER NOT NULL,
 state TEXT NOT NULL DEFAULT 'scheduled' CHECK(state IN ('scheduled','attempted','submitted','blocked','failed','unknown','cancelled')),
 attempted_at INTEGER,
 result TEXT,
 UNIQUE(item_id,generation));
CREATE INDEX IF NOT EXISTS reminders_due ON reminders(state,due_at);
CREATE TABLE IF NOT EXISTS reminder_inbox (
 reminder_id INTEGER PRIMARY KEY REFERENCES reminders(id),
 created_at INTEGER NOT NULL,
 acked_at INTEGER);
"#;

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error("{0}")]
    Invalid(String),
    #[error("The workspace item was not found in this project.")]
    NotFound,
    #[error("The item changed. Review the saved version before trying again.")]
    Conflict(Box<Item>),
    #[error(transparent)]
    Store(#[from] StoreError),
}
impl From<rusqlite::Error> for WorkspaceError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.into())
    }
}
impl From<serde_json::Error> for WorkspaceError {
    fn from(error: serde_json::Error) -> Self {
        Self::Invalid(error.to_string())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ItemChange {
    pub id: i64,
    pub expected_revision: u64,
    pub title: Option<String>,
    pub body: Option<String>,
    pub fields: Option<Value>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentFormat {
    Text,
    #[default]
    Markdown,
    Html,
    Csv,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DocumentFields {
    #[serde(default)]
    pub format: DocumentFormat,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TaskFields {
    #[serde(default)]
    pub status: TaskStatus,
    pub due_date: Option<String>,
    pub remind_at: Option<i64>,
    pub timezone: Option<String>,
}

pub(crate) fn scoped_item(
    connection: &Connection,
    project: i64,
    id: i64,
    include_deleted: bool,
) -> Result<Item, WorkspaceError> {
    connection
        .query_row(
            &format!("SELECT {ITEM_COLUMNS} FROM items WHERE id=?1 AND project_id=?2 AND (?3 OR deleted_at IS NULL)"),
            params![id, project, include_deleted],
            |row| item_at(row, 0),
        )
        .optional()?
        .ok_or(WorkspaceError::NotFound)
}
pub(crate) fn cancel_inactive_reminders(connection: &Connection, id: i64) -> Result<(), StoreError> {
    connection.execute("UPDATE reminders SET state='cancelled' WHERE item_id=?1 AND state IN ('scheduled','attempted') AND EXISTS (SELECT 1 FROM items WHERE id=?1 AND (deleted_at IS NOT NULL OR json_extract(fields_json,'$.status')='completed' OR json_extract(fields_json,'$.remind_at') IS NULL OR json_extract(fields_json,'$.remind_at') != reminders.due_at))",[id])?;
    Ok(())
}
fn validate_kinds(kinds: &[&str]) -> Result<(), WorkspaceError> {
    if kinds.len() > ITEM_KINDS.len() || kinds.iter().any(|k| !ITEM_KINDS.contains(k)) {
        return Err(WorkspaceError::Invalid(
            "Choose at most six known workspace kinds.".into(),
        ));
    }
    Ok(())
}

impl Store {
    pub fn workspace_item_title(&self,project:i64,id:i64,include_deleted:bool)->Result<Option<String>,StoreError> {
        Ok(self.connection().query_row("SELECT substr(title,1,240) FROM items WHERE project_id=?1 AND id=?2 AND (?3 OR deleted_at IS NULL)",params![project,id,include_deleted],|r|r.get(0)).optional()?)
    }
    /// Bounded list previews; callers must fetch the exact item before editing a truncated source.
    pub fn workspace_item_previews(&self, project:i64, kinds:&[&str], deleted:bool, after_id:Option<i64>, limit:usize) -> Result<Vec<WorkspaceItemPreview>,WorkspaceError> {
        self.workspace_previews(project,kinds,deleted,after_id,limit,None)
    }
    pub fn workspace_task_previews(&self,project:i64,view:TaskView,today:&str,deleted:bool,after:Option<i64>,limit:usize)->Result<Vec<WorkspaceItemPreview>,WorkspaceError> {
        if !valid_date(today) {return Err(WorkspaceError::Invalid("Choose a valid local date.".into()));}
        self.workspace_previews(project,&["task"],deleted,after,limit,if deleted {None}else{Some((view,today))})
    }
    fn workspace_previews(&self, project:i64, kinds:&[&str], deleted:bool, after_id:Option<i64>, limit:usize, task:Option<(TaskView,&str)>) -> Result<Vec<WorkspaceItemPreview>,WorkspaceError> {
        validate_kinds(kinds)?;
        let connection=self.connection();let kind_json=serde_json::to_string(kinds)?;
        let view=task.map(|(view,_)|match view {TaskView::Today=>"today",TaskView::Upcoming=>"upcoming",TaskView::Completed=>"completed"});
        let today=task.map(|(_,today)|today);
        let mut query=connection.prepare("SELECT id,kind,substr(title,1,240),substr(body,1,512),CASE WHEN length(CAST(fields_json AS BLOB))<=65536 THEN fields_json ELSE '{}' END,project_id,created_at,updated_at,revision,deleted_at,(length(CAST(body AS BLOB))>length(CAST(substr(body,1,512) AS BLOB)) OR length(CAST(title AS BLOB))>length(CAST(substr(title,1,240) AS BLOB)) OR length(CAST(fields_json AS BLOB))>65536) FROM items WHERE project_id=?1 AND (deleted_at IS NOT NULL)=?2 AND (?3 IS NULL OR id<?3) AND (?4='[]' OR kind IN (SELECT value FROM json_each(?4))) AND (?6 IS NULL OR (?6='completed' AND json_extract(fields_json,'$.status')='completed') OR (COALESCE(json_extract(fields_json,'$.status'),'pending')!='completed' AND ((?6='today' AND json_extract(fields_json,'$.due_date') IS NOT NULL AND json_extract(fields_json,'$.due_date')<=?7) OR (?6='upcoming' AND (json_extract(fields_json,'$.due_date') IS NULL OR json_extract(fields_json,'$.due_date')>?7))))) ORDER BY id DESC LIMIT ?5")?;
        Ok(query.query_map(params![project,deleted,after_id,kind_json,limit.min(100) as i64,view,today],|r|Ok(WorkspaceItemPreview{item:item_at(r,0)?,body_truncated:r.get(10)?}))?.collect::<Result<_,_>>()?)
    }
    pub fn workspace_item(&self, project: i64, id: i64, include_deleted: bool) -> Result<Item, WorkspaceError> {
        scoped_item(&self.connection(), project, id, include_deleted)
    }
    pub fn workspace_items(
        &self,
        project: i64,
        kinds: &[&str],
        deleted: bool,
        after_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<Item>, WorkspaceError> {
        validate_kinds(kinds)?;
        let connection = self.connection();
        let kind_json = serde_json::to_string(kinds)?;
        let mut statement=connection.prepare(&format!("SELECT {ITEM_COLUMNS} FROM items WHERE project_id=?1 AND (deleted_at IS NOT NULL)=?2 AND (?3 IS NULL OR id<?3) AND (?4='[]' OR kind IN (SELECT value FROM json_each(?4))) ORDER BY id DESC LIMIT ?5"))?;
        let rows = statement.query_map(
            params![project, deleted, after_id, kind_json, limit.min(100) as i64],
            |r| item_at(r, 0),
        )?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
    pub fn search_workspace_in_project(
        &self,
        project: i64,
        query: &str,
        kinds: &[&str],
        limit: usize,
    ) -> Result<Vec<ItemHit>, WorkspaceError> {
        Ok(self.search_workspace_scoped(Some(project), query, kinds, limit)?)
    }
    pub fn save_workspace_item(&self, project: i64, change: &ItemChange, now: i64) -> Result<Item, WorkspaceError> {
        let mut connection = self.connection();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let item = scoped_item(&tx, project, change.id, false)?;
        if item.revision != change.expected_revision {
            return Err(WorkspaceError::Conflict(Box::new(item)));
        }
        if !matches!(item.kind.as_str(), "document" | "note" | "task") {
            return Err(WorkspaceError::Invalid("This item kind cannot be edited here.".into()));
        }
        let title = change.title.as_deref().unwrap_or(&item.title);
        let body = change.body.as_deref().unwrap_or(&item.body);
        let mut fields = item.fields.clone();
        if let Some(patch) = &change.fields {
            let Some(patch) = patch.as_object() else {
                return Err(WorkspaceError::Invalid("The fields must be an object.".into()));
            };
            let Some(existing) = fields.as_object_mut() else {
                return Err(WorkspaceError::Invalid("The saved fields are not an object.".into()));
            };
            existing.extend(patch.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        validate_workspace_content(&item.kind, title, body, &fields)?;
        tx.execute("UPDATE items SET title=?2,body=?3,fields_json=?4,updated_at=?5,revision=revision+1 WHERE id=?1 AND revision=?6",params![item.id,title,body,serde_json::to_string(&fields)?,now,item.revision as i64])?;
        tx.execute("DELETE FROM items_fts WHERE rowid=?1", [item.id])?;
        index_item(&tx, item.id, &item.kind, title, body)?;
        crate::reminders::reconcile(&tx, item.id, item.fields.get("remind_at").and_then(Value::as_i64), false)?;
        let updated = scoped_item(&tx, project, item.id, false)?;
        tx.commit()?;
        Ok(updated)
    }
    pub fn delete_workspace_item(
        &self,
        project: i64,
        id: i64,
        expected_revision: u64,
        now: i64,
    ) -> Result<Item, WorkspaceError> {
        self.set_workspace_deleted(project, id, expected_revision, Some(now), now)
    }
    pub fn restore_workspace_item(
        &self,
        project: i64,
        id: i64,
        expected_revision: u64,
        now: i64,
    ) -> Result<Item, WorkspaceError> {
        self.set_workspace_deleted(project, id, expected_revision, None, now)
    }
    fn set_workspace_deleted(
        &self,
        project: i64,
        id: i64,
        expected: u64,
        deleted: Option<i64>,
        now: i64,
    ) -> Result<Item, WorkspaceError> {
        let mut connection = self.connection();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let item = scoped_item(&tx, project, id, true)?;
        if item.revision != expected {
            return Err(WorkspaceError::Conflict(Box::new(item)));
        }
        if item.deleted_at.is_some() == deleted.is_some() {
            return Err(WorkspaceError::Invalid("The item is already in this state.".into()));
        }
        tx.execute(
            "UPDATE items SET deleted_at=?2,updated_at=?3,revision=revision+1 WHERE id=?1",
            params![id, deleted, now],
        )?;
        tx.execute("DELETE FROM items_fts WHERE rowid=?1", [id])?;
        if deleted.is_none() {
            index_item(&tx, id, &item.kind, &item.title, &item.body)?;
        } else {
            cancel_inactive_reminders(&tx, id)?;
        }
        let item = scoped_item(&tx, project, id, true)?;
        tx.commit()?;
        Ok(item)
    }
    pub fn workspace_links(&self, project: i64, id: i64) -> Result<Vec<(String, Item)>, WorkspaceError> {
        let connection = self.connection();
        scoped_item(&connection, project, id, false)?;
        let mut statement=connection.prepare("SELECT l.rel,i.id,i.kind,i.title,i.body,i.fields_json,i.project_id,i.created_at,i.updated_at,i.revision,i.deleted_at FROM item_links l JOIN items i ON i.id=CASE WHEN l.from_id=?1 THEN l.to_id ELSE l.from_id END WHERE (l.from_id=?1 OR l.to_id=?1) AND i.project_id=?2 AND i.deleted_at IS NULL ORDER BY l.rel,i.id LIMIT 200")?;
        let rows = statement.query_map(params![id, project], |r| Ok((r.get(0)?, item_at(r, 1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
    pub fn link_workspace_items(&self, project: i64, from: i64, to: i64, rel: &str) -> Result<(), WorkspaceError> {
        self.change_workspace_link(project, from, to, rel, true)
    }
    pub fn unlink_workspace_items(&self, project: i64, from: i64, to: i64, rel: &str) -> Result<(), WorkspaceError> {
        self.change_workspace_link(project, from, to, rel, false)
    }
    fn change_workspace_link(
        &self,
        project: i64,
        from: i64,
        to: i64,
        rel: &str,
        add: bool,
    ) -> Result<(), WorkspaceError> {
        if rel.trim().is_empty() || rel.chars().count() > 64 {
            return Err(WorkspaceError::Invalid(
                "A link relation must contain 1 to 64 characters.".into(),
            ));
        }
        let mut connection = self.connection();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        scoped_item(&tx, project, from, false)?;
        scoped_item(&tx, project, to, false)?;
        if add {
            tx.execute(
                "INSERT OR IGNORE INTO item_links(from_id,to_id,rel) VALUES(?1,?2,?3)",
                params![from, to, rel],
            )?;
        } else {
            tx.execute(
                "DELETE FROM item_links WHERE ((from_id=?1 AND to_id=?2) OR (from_id=?2 AND to_id=?1)) AND rel=?3",
                params![from, to, rel],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

pub fn validate_workspace_content(kind: &str, title: &str, body: &str, fields: &Value) -> Result<(), WorkspaceError> {
    if title.trim().is_empty()
        || title.chars().count() > 240
        || body.len() > 2 * 1024 * 1024
        || serde_json::to_vec(fields)?.len() > 64 * 1024
        || !fields.is_object()
    {
        return Err(WorkspaceError::Invalid(
            "Use a title of 1 to 240 characters, a body up to 2 MiB, and fields up to 64 KiB.".into(),
        ));
    }
    match kind {
        "document" | "note" => {
            let parsed: DocumentFields = serde_json::from_value(fields.clone())?;
            if kind == "note" && !matches!(parsed.format, DocumentFormat::Text | DocumentFormat::Markdown) {
                return Err(WorkspaceError::Invalid("Notes use text or Markdown.".into()));
            }
            crate::workspace_formats::validate_document(parsed.format,body)?;
        }
        "task" => {
            validate_task_metadata(fields)?;
        }
        _ => {
            return Err(WorkspaceError::Invalid(
                "This workspace kind cannot be edited here.".into(),
            ));
        }
    }
    Ok(())
}
pub(crate) fn validate_task_metadata(fields: &Value) -> Result<TaskFields,WorkspaceError> {
    let task:TaskFields=serde_json::from_value(fields.clone())?;
    if task.due_date.as_deref().is_some_and(|d|!valid_date(d)) {return Err(WorkspaceError::Invalid("The due date must be a valid YYYY-MM-DD date.".into()));}
    if task.timezone.as_ref().is_some_and(|z|!include_str!("../data/timezones.txt").lines().any(|known|known==z)) {return Err(WorkspaceError::Invalid("Choose a supported IANA timezone.".into()));}
    if let Some(at)=task.remind_at
        && (!(0..=253402300799).contains(&at) || task.timezone.is_none()) {return Err(WorkspaceError::Invalid("A reminder needs a valid UTC time and timezone.".into()));}
    Ok(task)
}
#[derive(Clone,Debug,Serialize)]
pub struct WorkspaceItemPreview { #[serde(flatten)] pub item:Item, pub body_truncated:bool }
#[derive(Clone,Copy,Debug,Serialize,Deserialize)]
#[serde(rename_all="snake_case")]
pub enum TaskView {Today,Upcoming,Completed}
fn valid_date(date: &str) -> bool {
    if !date.is_ascii() || date.len() != 10 || date.as_bytes()[4] != b'-' || date.as_bytes()[7] != b'-' {
        return false;
    }
    let (Ok(year), Ok(month), Ok(day)) = (
        date[..4].parse::<u32>(),
        date[5..7].parse::<u32>(),
        date[8..].parse::<u32>(),
    ) else {
        return false;
    };
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if year % 400 == 0 || (year % 4 == 0 && year % 100 != 0) {
                29
            } else {
                28
            }
        }
        _ => 0,
    };
    year > 0 && day > 0 && day <= days
}
