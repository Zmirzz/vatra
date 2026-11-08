//! Workspace items: documents, notes, tasks, events, messages and drafts. Each write keeps the full-text index
//! in step inside the same transaction; a soft-deleted item keeps its row but leaves the index.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, ToSql, params, params_from_iter};
use serde::{Deserialize, Serialize};

use crate::store::{Store, StoreError};

#[cfg(test)]
mod destination_tests {
    use super::*;

    #[test]
    fn network_and_extended_paths_are_refused_without_filesystem_access() {
        for path in [r"\\server\share\file.jsonl", r"\\?\C:\file.jsonl", "//server/share/file.jsonl", r"\/?\C:\file.jsonl"] {
            assert!(validate_destination(Path::new(path), &["jsonl"]).is_err(), "{path}");
        }
    }

    #[test]
    fn publication_never_overwrites_a_destination_created_after_validation() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("export.jsonl");
        validate_destination(&destination, &["jsonl"]).unwrap();
        let temporary = dir.path().join("completed.tmp");
        std::fs::write(&temporary, "new export").unwrap();
        std::fs::write(&destination, "concurrent winner").unwrap();
        assert!(publish_new(&temporary, &destination).is_err());
        assert_eq!(std::fs::read_to_string(destination).unwrap(), "concurrent winner");
    }
}

pub const ITEM_KINDS: [&str; 6] = ["document", "note", "task", "event", "message", "draft"];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ItemHit {
    pub id: i64,
    pub kind: String,
    pub title: String,
    pub snippet: String,
    pub updated_at: i64,
}

fn hit_at(row: &rusqlite::Row) -> rusqlite::Result<ItemHit> {
    Ok(ItemHit {
        id: row.get(0)?,
        kind: row.get(1)?,
        title: row.get(2)?,
        snippet: row.get(3)?,
        updated_at: row.get(4)?,
    })
}

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS items (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 kind TEXT NOT NULL,
 title TEXT NOT NULL,
 body TEXT NOT NULL,
 fields_json TEXT NOT NULL DEFAULT '{}',
 project_id INTEGER REFERENCES projects(id),
 created_at INTEGER NOT NULL,
 updated_at INTEGER NOT NULL,
 deleted_at INTEGER);
CREATE INDEX IF NOT EXISTS items_by_kind ON items(kind, deleted_at, updated_at);
CREATE TABLE IF NOT EXISTS item_links (
 from_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
 to_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
 rel TEXT NOT NULL,
 PRIMARY KEY (from_id, to_id, rel));
CREATE INDEX IF NOT EXISTS item_links_to ON item_links(to_id);
CREATE VIRTUAL TABLE IF NOT EXISTS items_fts USING fts5(
 kind UNINDEXED, item_id UNINDEXED, title, body,
 tokenize='unicode61 remove_diacritics 2');";

pub(crate) const ITEM_COLUMNS: &str = "id, kind, title, body, fields_json, project_id, created_at, updated_at, revision, deleted_at";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NewItem {
    pub kind: String,
    pub title: String,
    pub body: String,
    pub fields: serde_json::Value,
    pub project_id: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: i64,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub fields: serde_json::Value,
    pub project_id: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub revision: u64,
    pub deleted_at: Option<i64>,
}

fn parse_fields(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap_or_else(|_| serde_json::json!({}))
}

/// Reads an item whose columns start at `at`, in the order of `ITEM_COLUMNS`.
pub(crate) fn item_at(row: &rusqlite::Row, at: usize) -> rusqlite::Result<Item> {
    let fields: String = row.get(at + 4)?;
    let revision: i64 = row.get(at + 8)?;
    Ok(Item {
        id: row.get(at)?,
        kind: row.get(at + 1)?,
        title: row.get(at + 2)?,
        body: row.get(at + 3)?,
        fields: parse_fields(&fields),
        project_id: row.get(at + 5)?,
        created_at: row.get(at + 6)?,
        updated_at: row.get(at + 7)?,
        revision: u64::try_from(revision).map_err(|_|rusqlite::Error::IntegralValueOutOfRange(at + 8, revision))?,
        deleted_at: row.get(at + 9)?,
    })
}

/// One line of an export: the item, then its links to live items.
#[derive(Serialize)]
struct ExportedItem {
    #[serde(flatten)]
    item: Item,
    links: Vec<ExportedLink>,
}

#[derive(Serialize)]
struct ExportedLink {
    rel: String,
    /// The item at the other end of the link.
    to: i64,
    direction: String,
}

/// Refuses a path that already exists, including a dangling symbolic link.
fn refuse_existing(path: &Path) -> Result<(), StoreError> {
    if path.symlink_metadata().is_ok() {
        return Err(StoreError::Invalid(format!("the file already exists: {}", path.display())));
    }
    Ok(())
}

fn io_error(error: std::io::Error) -> StoreError {
    StoreError::Io(error.to_string())
}

/// Writes the lines to a new temporary file and returns how many items it holds.
fn write_export(
    temporary: &Path,
    items: Vec<Item>,
    mut links: HashMap<i64, Vec<ExportedLink>>,
) -> Result<usize, StoreError> {
    let file = OpenOptions::new().write(true).create_new(true).open(temporary).map_err(io_error)?;
    let mut writer = BufWriter::new(file);
    let count = items.len();
    for item in items {
        let links = links.remove(&item.id).unwrap_or_default();
        serde_json::to_writer(&mut writer, &ExportedItem { item, links })?;
        writer.write_all(b"\n").map_err(io_error)?;
    }
    writer.flush().map_err(io_error)?;
    let file = writer.into_inner().map_err(|error| io_error(error.into_error()))?;
    file.sync_all().map_err(io_error)?;
    Ok(count)
}

/// The index row shares the item's rowid, so a row is found by id without scanning.
pub(crate) fn index_item(connection: &Connection, id: i64, kind: &str, title: &str, body: &str) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO items_fts(rowid, kind, item_id, title, body) VALUES(?1, ?2, ?1, ?3, ?4)",
        params![id, kind, title, body],
    )?;
    Ok(())
}

impl Store {
    /// Stores a new item. `now` is unix seconds, given by the caller.
    pub fn create_item(&self, item: &NewItem, now: i64) -> Result<i64, StoreError> {
        let connection = self.connection();
        let transaction = connection.unchecked_transaction()?;
        let id = insert_item(&transaction, item, now)?;
        transaction.commit()?;
        Ok(id)
    }

    /// The live item, or None when it is missing or deleted.
    pub fn item(&self, id: i64) -> Result<Option<Item>, StoreError> {
        Ok(self
            .connection()
            .query_row(
                &format!("SELECT {ITEM_COLUMNS} FROM items WHERE id = ?1 AND deleted_at IS NULL"),
                [id],
                |row| item_at(row, 0),
            )
            .optional()?)
    }

    /// Changes the given parts and leaves the others as they are. False when the item is missing or deleted.
    pub fn update_item(
        &self,
        id: i64,
        title: Option<&str>,
        body: Option<&str>,
        fields: Option<&serde_json::Value>,
        now: i64,
    ) -> Result<bool, StoreError> {
        let fields = fields.map(serde_json::to_string).transpose()?;
        let connection = self.connection();
        let transaction = connection.unchecked_transaction()?;
        let previous_remind = transaction.query_row("SELECT json_extract(fields_json,'$.remind_at') FROM items WHERE id=?1",[id],|r|r.get::<_,Option<i64>>(0)).optional()?.flatten();
        let changed = transaction.execute(
            "UPDATE items SET title = COALESCE(?2, title), body = COALESCE(?3, body),
                fields_json = COALESCE(?4, fields_json), updated_at = ?5, revision = revision + 1
             WHERE id = ?1 AND deleted_at IS NULL",
            params![id, title, body, fields, now],
        )?;
        if changed == 0 {
            return Ok(false);
        }
        let (kind, title, body): (String, String, String) = transaction.query_row(
            "SELECT kind, title, body FROM items WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        transaction.execute("DELETE FROM items_fts WHERE rowid = ?1", [id])?;
        index_item(&transaction, id, &kind, &title, &body)?;
        if kind == "task" {
            crate::reminders::reconcile(&transaction, id, previous_remind, false)?;
        }
        transaction.commit()?;
        Ok(true)
    }

    /// Soft delete: the row stays with `deleted_at` set, and the item leaves the index. False when it is
    /// missing or already deleted.
    pub fn delete_item(&self, id: i64, now: i64) -> Result<bool, StoreError> {
        let connection = self.connection();
        let transaction = connection.unchecked_transaction()?;
        let changed = transaction.execute(
            "UPDATE items SET deleted_at = ?2, updated_at = ?2, revision = revision + 1 WHERE id = ?1 AND deleted_at IS NULL",
            params![id, now],
        )?;
        if changed == 0 {
            return Ok(false);
        }
        transaction.execute("DELETE FROM items_fts WHERE rowid = ?1", [id])?;
        crate::workspace_versions::cancel_inactive_reminders(&transaction, id)?;
        transaction.commit()?;
        Ok(true)
    }

    /// Searches literal words in live items; punctuation cannot introduce FTS operators. Empty `kinds` means all
    /// kinds. The limit is capped at 100.
    pub fn search_workspace(
        &self,
        query: &str,
        kinds: &[&str],
        limit: usize,
    ) -> Result<Vec<ItemHit>, StoreError> {
        self.search_workspace_scoped(None, query, kinds, limit)
    }

    pub fn search_workspace_scoped(&self, project: Option<i64>, query: &str, kinds: &[&str], limit: usize) -> Result<Vec<ItemHit>, StoreError> {
        if kinds.len() > ITEM_KINDS.len() || kinds.iter().any(|kind| !ITEM_KINDS.contains(kind)) {
            return Err(StoreError::Invalid("Choose at most six workspace kinds: document, note, task, event, message, draft.".into()));
        }
        let words: Vec<String> = query
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .take(64)
            .map(|s| format!("\"{s}\""))
            .collect();
        if words.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let matching = words.join(" AND ");
        let limit = limit.min(100) as i64;
        let kind_filter = if kinds.is_empty() {
            String::new()
        } else {
            let placeholders: Vec<String> = (0..kinds.len()).map(|i| format!("?{}", i + 4)).collect();
            format!(" AND i.kind IN ({})", placeholders.join(","))
        };
        let connection = self.connection();
        let mut statement = connection.prepare(&format!(
            "SELECT i.id, i.kind, i.title, snippet(items_fts, -1, '', '', ' … ', 24), i.updated_at
             FROM items_fts JOIN items i ON i.id = items_fts.rowid
             WHERE items_fts MATCH ?1 AND i.deleted_at IS NULL AND (?3 IS NULL OR i.project_id = ?3){kind_filter}
             ORDER BY items_fts.rank LIMIT ?2"
        ))?;
        let mut bound: Vec<&dyn ToSql> = vec![&matching, &limit, &project];
        bound.extend(kinds.iter().map(|kind| kind as &dyn ToSql));
        let rows = statement.query_map(params_from_iter(bound), hit_at)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Live items of one kind, most recently updated first.
    pub fn items_by_kind(&self, kind: &str, limit: usize) -> Result<Vec<Item>, StoreError> {
        let connection = self.connection();
        let mut statement = connection.prepare(&format!(
            "SELECT {ITEM_COLUMNS} FROM items WHERE kind = ?1 AND deleted_at IS NULL
             ORDER BY updated_at DESC, id DESC LIMIT ?2"
        ))?;
        let rows = statement.query_map(params![kind, limit as i64], |row| item_at(row, 0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Writes every live item as one JSON object per line, ordered by id, each with its links to live items
    /// (`direction` is "out" when the item is `from_id`, "in" when it is `to_id`). Returns the number of items.
    /// Refuses an existing file. The lines go to a temporary file in the same folder, linked into place at the end.
    pub fn export_items_json(&self, to: &Path) -> Result<usize, StoreError> {
        validate_destination(to, &["jsonl", "json"])?;
        let file_name = to
            .file_name()
            .ok_or_else(|| StoreError::Invalid(format!("no file name in {}", to.display())))?;
        let (items, links) = {
            let connection = self.connection();
            let mut statement = connection.prepare(&format!(
                "SELECT {ITEM_COLUMNS} FROM items WHERE deleted_at IS NULL ORDER BY id"
            ))?;
            let items: Vec<Item> = statement
                .query_map([], |row| item_at(row, 0))?
                .collect::<Result<_, _>>()?;
            // The item itself is filtered by the caller's list; only the other end must be live here.
            let mut statement = connection.prepare(
                "SELECT l.from_id, 'out', l.rel, l.to_id FROM item_links l
                   JOIN items o ON o.id = l.to_id AND o.deleted_at IS NULL
                 UNION ALL
                 SELECT l.to_id, 'in', l.rel, l.from_id FROM item_links l
                   JOIN items o ON o.id = l.from_id AND o.deleted_at IS NULL
                 ORDER BY 1, 2 DESC, 4, 3",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    ExportedLink {
                        direction: row.get(1)?,
                        rel: row.get(2)?,
                        to: row.get(3)?,
                    },
                ))
            })?;
            let mut links: HashMap<i64, Vec<ExportedLink>> = HashMap::new();
            for row in rows {
                let (item, link) = row?;
                links.entry(item).or_default().push(link);
            }
            (items, links)
        };
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let temporary = to.with_file_name(format!(".{}.{nanos}.tmp", file_name.to_string_lossy()));
        let count = match write_export(&temporary, items, links) {
            Ok(count) => count,
            Err(error) => {
                let _ = std::fs::remove_file(&temporary);
                return Err(error);
            }
        };
        publish_new(&temporary, to)?;
        Ok(count)
    }

    /// Copies the whole database to a new file with `VACUUM INTO`. The path is bound, not spliced into the SQL.
    /// Refuses an existing file. The copy goes to a temporary file in the same folder, linked into place at the
    /// end, so a failure leaves no partial file under the name asked for.
    pub fn backup_to(&self, to: &Path) -> Result<(), StoreError> {
        validate_destination(to, &["db"])?;
        let file_name = to
            .file_name()
            .ok_or_else(|| StoreError::Invalid(format!("the path has no file name: {}", to.display())))?;
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let temporary = to.with_file_name(format!(".{}.{nanos}.tmp", file_name.to_string_lossy()));
        let text = temporary
            .to_str()
            .ok_or_else(|| StoreError::Invalid(format!("the path is not valid UTF-8: {}", to.display())))?;
        if let Err(error) = self.connection().execute("VACUUM INTO ?1", [text]) {
            let _ = std::fs::remove_file(&temporary);
            return Err(error.into());
        }
        publish_new(&temporary, to)?;
        Ok(())
    }

    /// Links two items. Linking the same pair with the same relation again changes nothing.
    pub fn link_items(&self, from: i64, to: i64, rel: &str) -> Result<(), StoreError> {
        self.connection().execute(
            "INSERT OR IGNORE INTO item_links (from_id, to_id, rel) VALUES (?1, ?2, ?3)",
            params![from, to, rel],
        )?;
        Ok(())
    }

    /// The live items linked to `id` in either direction, each with the relation that links it.
    pub fn linked_items(&self, id: i64) -> Result<Vec<(String, Item)>, StoreError> {
        let connection = self.connection();
        let mut statement = connection.prepare(
            "SELECT l.rel, i.id, i.kind, i.title, i.body, i.fields_json, i.project_id, i.created_at, i.updated_at, i.revision, i.deleted_at
             FROM item_links l
             JOIN items i ON i.id = CASE WHEN l.from_id = ?1 THEN l.to_id ELSE l.from_id END
             WHERE (l.from_id = ?1 OR l.to_id = ?1) AND i.deleted_at IS NULL
             ORDER BY l.rel, i.id",
        )?;
        let rows = statement.query_map([id], |row| Ok((row.get::<_, String>(0)?, item_at(row, 1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

pub(crate) fn insert_item(connection: &Connection, item: &NewItem, now: i64) -> Result<i64, StoreError> {
    if !ITEM_KINDS.contains(&item.kind.as_str()) {
        return Err(StoreError::Invalid(format!("unknown item kind: {}", item.kind)));
    }
    connection.execute("INSERT INTO items(kind,title,body,fields_json,project_id,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?6)", params![item.kind,item.title,item.body,serde_json::to_string(&item.fields)?,item.project_id,now])?;
    let id=connection.last_insert_rowid();index_item(connection,id,&item.kind,&item.title,&item.body)?;
    crate::reminders::reconcile(connection,id,None,true)?;
    Ok(id)
}

fn validate_destination(to: &Path, extensions: &[&str]) -> Result<(), StoreError> {
    // Check the original spelling before any filesystem call, including existence checks.
    if to.to_string_lossy().replace('/', "\\").starts_with("\\\\") {
        return Err(StoreError::Invalid("Use a local path without a network or extended-path prefix.".into()));
    }
    let extension = to.extension().and_then(|part| part.to_str()).unwrap_or("");
    if !extensions.iter().any(|allowed| extension.eq_ignore_ascii_case(allowed)) {
        return Err(StoreError::Invalid(format!("Choose a file ending in {}.", extensions.join(" or "))));
    }
    refuse_existing(to)
}

/// A hard link creates a new directory entry atomically and refuses an existing destination on Windows and Unix.
fn publish_new(temporary: &Path, to: &Path) -> Result<(), StoreError> {
    let published = std::fs::hard_link(temporary, to).map_err(io_error);
    let _ = std::fs::remove_file(temporary);
    published
}
