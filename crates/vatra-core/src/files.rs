//! Session-owned immutable file snapshots. Bytes live in SQLite so ordinary backups are complete.
use crate::{Store, StoreError};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_FILE_BYTES: usize = 25 * 1024 * 1024;
pub const FILE_CHUNK_BYTES: usize = 256 * 1024;
const MAX_TEXT_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS file_blobs(hash TEXT PRIMARY KEY, bytes BLOB NOT NULL);
CREATE TABLE IF NOT EXISTS files(
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
 name TEXT NOT NULL, media_type TEXT NOT NULL,
 origin TEXT NOT NULL CHECK(origin IN ('attached','created','modified')),
 source_path TEXT, revision INTEGER NOT NULL DEFAULT 1,
 created_at INTEGER NOT NULL DEFAULT(unixepoch()), deleted INTEGER NOT NULL DEFAULT 0);
CREATE INDEX IF NOT EXISTS files_by_session ON files(session_id,deleted,id);
CREATE TABLE IF NOT EXISTS file_versions(
 file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
 revision INTEGER NOT NULL, hash TEXT NOT NULL REFERENCES file_blobs(hash),
 text TEXT NOT NULL, operation TEXT, created_at INTEGER NOT NULL DEFAULT(unixepoch()),
 PRIMARY KEY(file_id,revision));
CREATE TABLE IF NOT EXISTS message_files(
 session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
 message_seq INTEGER NOT NULL REFERENCES events(seq) ON DELETE CASCADE,
 file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
 revision INTEGER NOT NULL, context_text TEXT NOT NULL DEFAULT '', PRIMARY KEY(message_seq,file_id));
CREATE TABLE IF NOT EXISTS file_uploads(
 id TEXT PRIMARY KEY, session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
 size INTEGER NOT NULL, received INTEGER NOT NULL DEFAULT 0,
 created_at INTEGER NOT NULL DEFAULT(unixepoch()));
CREATE TABLE IF NOT EXISTS file_upload_chunks(
 upload_id TEXT NOT NULL REFERENCES file_uploads(id) ON DELETE CASCADE,
 offset INTEGER NOT NULL, bytes BLOB NOT NULL, PRIMARY KEY(upload_id,offset));
"#;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NewFile {
    pub name: String,
    pub media_type: String,
    pub origin: String,
    pub source_path: Option<String>,
    pub operation: Option<String>,
    pub text: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileRecord {
    pub id: i64,
    pub session_id: i64,
    pub name: String,
    pub media_type: String,
    pub origin: String,
    pub source_path: Option<String>,
    pub revision: i64,
    pub created_at: i64,
    pub size: usize,
    pub hash: String,
    pub operation: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileVersion {
    pub revision: i64,
    pub hash: String,
    pub size: usize,
    pub operation: Option<String>,
    pub created_at: i64,
}
const SELECT_FILES: &str = "SELECT f.id,f.session_id,f.name,f.media_type,f.origin,f.source_path,f.revision,f.created_at,length(b.bytes),v.hash,v.operation FROM files f JOIN file_versions v ON v.file_id=f.id AND v.revision=f.revision JOIN file_blobs b ON b.hash=v.hash";
fn file_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<FileRecord> {
    Ok(FileRecord {
        id: row.get(0)?,
        session_id: row.get(1)?,
        name: row.get(2)?,
        media_type: row.get(3)?,
        origin: row.get(4)?,
        source_path: row.get(5)?,
        revision: row.get(6)?,
        created_at: row.get(7)?,
        size: row.get::<_, i64>(8)? as usize,
        hash: row.get(9)?,
        operation: row.get(10)?,
    })
}
fn invalid(message: &str) -> StoreError {
    StoreError::Invalid(message.into())
}
fn validate_name(name: &str) -> Result<(), StoreError> {
    if name.trim().is_empty()
        || name.len() > 240
        || name
            .chars()
            .any(|c| c.is_control() || "/\\:<>\"|?*".contains(c))
        || name == "."
        || name == ".."
    {
        return Err(invalid(
            "Choose a file name without path separators or special characters.",
        ));
    }
    Ok(())
}
fn validate_content(text: &str, bytes: &[u8]) -> Result<(), StoreError> {
    if bytes.len() > MAX_FILE_BYTES || text.len() > MAX_TEXT_BYTES {
        return Err(invalid(
            "File exceeds the 25 MiB file or 8 MiB extracted text limit.",
        ));
    }
    Ok(())
}

impl Store {
    pub fn branch_session(&self, session: i64, through_seq: i64) -> Result<i64, StoreError> {
        if through_seq < 0 {
            return Err(invalid("Invalid branch position."));
        }
        let mut conn = self.connection();
        let tx = conn.transaction()?;
        let (project, agent, title): (Option<i64>, String, Option<String>) = tx.query_row(
            "SELECT project_id,agent,title FROM sessions WHERE id=?1",
            [session],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        tx.execute(
            "INSERT INTO sessions(project_id,agent,title) VALUES(?1,?2,?3)",
            params![project, agent, title.map(|t| format!("{t} · branch"))],
        )?;
        let target = tx.last_insert_rowid();
        let events: Vec<(i64, String)> = {
            let mut stmt = tx.prepare(
                "SELECT seq,payload FROM events WHERE session_id=?1 AND seq<=?2 ORDER BY seq",
            )?;
            stmt.query_map(params![session, through_seq], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?
            .collect::<Result<_, _>>()?
        };
        let referenced: Vec<(i64, i64)> = {
            let mut stmt=tx.prepare("SELECT file_id,max(revision) FROM message_files WHERE session_id=?1 AND message_seq<=?2 GROUP BY file_id")?;
            stmt.query_map(params![session, through_seq], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?
            .collect::<Result<_, _>>()?
        };
        let mut ids = std::collections::BTreeMap::new();
        for (old, revision) in referenced {
            tx.execute("INSERT INTO files(session_id,name,media_type,origin,source_path,revision) SELECT ?1,name,media_type,origin,source_path,?3 FROM files WHERE id=?2 AND session_id=?4",params![target,old,revision,session])?;
            let new = tx.last_insert_rowid();
            tx.execute("INSERT INTO file_versions(file_id,revision,hash,text,operation,created_at) SELECT ?1,revision,hash,text,operation,created_at FROM file_versions WHERE file_id=?2 AND revision<=?3",params![new,old,revision])?;
            ids.insert(old, new);
        }
        for (seq, payload) in events {
            let event: crate::Event = serde_json::from_str(&payload)?;
            // Native session identifiers belong only to the original provider conversation.
            if matches!(event, crate::Event::NativeSession { .. }) {
                continue;
            }
            tx.execute(
                "INSERT INTO events(session_id,payload) VALUES(?1,?2)",
                params![target, payload],
            )?;
            let new_seq = tx.last_insert_rowid();
            crate::store::update_summary(&tx, target, &event)?;
            crate::history::index_event(&tx, target, &event)?;
            for (old, new) in &ids {
                tx.execute("INSERT INTO message_files(session_id,message_seq,file_id,revision,context_text) SELECT ?1,?2,?3,revision,context_text FROM message_files WHERE session_id=?4 AND message_seq=?5 AND file_id=?6",params![target,new_seq,new,session,seq,old])?;
            }
        }
        tx.commit()?;
        Ok(target)
    }
    pub fn append_message_with_files(
        &self,
        session: i64,
        prompt: &str,
        files: &[crate::TurnFile],
    ) -> Result<i64, StoreError> {
        let event = crate::Event::UserMessage {
            text: prompt.into(),
        };
        let payload = serde_json::to_string(&event)?;
        let mut conn = self.connection();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO events(session_id,payload) VALUES(?1,?2)",
            params![session, payload],
        )?;
        let seq = tx.last_insert_rowid();
        for file in files.iter().filter(|f| f.history_index.is_none()) {
            let changed=tx.execute("INSERT INTO message_files(session_id,message_seq,file_id,revision,context_text) SELECT session_id,?3,id,revision,?5 FROM files WHERE session_id=?1 AND id=?2 AND revision=?4 AND deleted=0",params![session,file.id,seq,file.revision,file.text])?;
            if changed != 1 {
                return Err(invalid("An attachment changed. Review it and send again."));
            }
        }
        crate::store::update_summary(&tx, session, &event)?;
        crate::history::index_event(&tx, session, &event)?;
        tx.commit()?;
        Ok(seq)
    }
    /// Capture only verified bytes from a completed operation. Reconciliation is idempotent by source path and hash.
    pub fn capture_file(
        &self,
        session: i64,
        input: &NewFile,
        before: Option<&[u8]>,
        after: &[u8],
    ) -> Result<FileRecord, StoreError> {
        let existing = self
            .list_files(session)?
            .into_iter()
            .find(|f| f.origin != "attached" && f.source_path == input.source_path);
        let hash = format!("{:x}", Sha256::digest(after));
        if let Some(file) = existing {
            if file.hash == hash {
                return Ok(file);
            }
            return self.revise_file(
                session,
                file.id,
                file.revision,
                &input.text,
                after,
                input.operation.as_deref().unwrap_or("observed"),
            );
        }
        if let Some(before) = before.filter(|bytes| *bytes != after) {
            let mut initial = input.clone();
            initial.text = std::str::from_utf8(before).unwrap_or("").to_string();
            initial.operation = Some(format!(
                "{}:before",
                input.operation.as_deref().unwrap_or("observed")
            ));
            let baseline = self.import_file(session, &initial, before)?;
            self.revise_file(
                session,
                baseline.id,
                1,
                &input.text,
                after,
                input.operation.as_deref().unwrap_or("observed"),
            )
        } else {
            self.import_file(session, input, after)
        }
    }
    pub fn begin_file_upload(&self, session: i64, id: &str, size: usize) -> Result<(), StoreError> {
        if size > MAX_FILE_BYTES || id.len() > 80 || id.is_empty() {
            return Err(invalid("Invalid upload size or identifier."));
        }
        let mut conn = self.connection();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM file_uploads WHERE created_at<unixepoch()-86400",
            [],
        )?;
        let count: i64 = tx.query_row(
            "SELECT count(*) FROM file_uploads WHERE session_id=?1",
            [session],
            |r| r.get(0),
        )?;
        if count >= 20 {
            return Err(invalid("Finish or cancel the pending uploads first."));
        }
        tx.execute(
            "INSERT INTO file_uploads(id,session_id,size) VALUES(?1,?2,?3)",
            params![id, session, size as i64],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn append_file_upload(
        &self,
        session: i64,
        id: &str,
        offset: usize,
        bytes: &[u8],
    ) -> Result<(), StoreError> {
        if bytes.is_empty() || bytes.len() > FILE_CHUNK_BYTES || offset > MAX_FILE_BYTES {
            return Err(invalid("Invalid upload chunk."));
        }
        let mut conn = self.connection();
        let tx = conn.transaction()?;
        let n=tx.execute("UPDATE file_uploads SET received=received+?4 WHERE id=?1 AND session_id=?2 AND received=?3 AND received+?4<=size",params![id,session,offset as i64,bytes.len() as i64])?;
        if n != 1 {
            return Err(invalid(
                "Upload not found, out of order, or larger than declared.",
            ));
        }
        tx.execute(
            "INSERT INTO file_upload_chunks(upload_id,offset,bytes) VALUES(?1,?2,?3)",
            params![id, offset as i64, bytes],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn read_file_upload(&self, session: i64, id: &str) -> Result<Vec<u8>, StoreError> {
        let conn = self.connection();
        let (size, received): (i64, i64) = conn.query_row(
            "SELECT size,received FROM file_uploads WHERE session_id=?1 AND id=?2",
            params![session, id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if size != received {
            return Err(invalid("The upload is incomplete."));
        }
        let mut stmt = conn
            .prepare("SELECT bytes FROM file_upload_chunks WHERE upload_id=?1 ORDER BY offset")?;
        let mut bytes = Vec::with_capacity(size as usize);
        for part in stmt.query_map([id], |r| r.get::<_, Vec<u8>>(0))? {
            bytes.extend(part?);
        }
        if bytes.len() != size as usize {
            return Err(invalid("The upload is incomplete."));
        }
        Ok(bytes)
    }
    pub fn cancel_file_upload(&self, session: i64, id: &str) -> Result<(), StoreError> {
        self.connection().execute(
            "DELETE FROM file_uploads WHERE session_id=?1 AND id=?2",
            params![session, id],
        )?;
        Ok(())
    }
    pub fn import_file(
        &self,
        session: i64,
        input: &NewFile,
        bytes: &[u8],
    ) -> Result<FileRecord, StoreError> {
        validate_name(&input.name)?;
        validate_content(&input.text, bytes)?;
        if !["attached", "created", "modified"].contains(&input.origin.as_str())
            || input.media_type.len() > 128
        {
            return Err(invalid("Invalid file origin or media type."));
        }
        let hash = format!("{:x}", Sha256::digest(bytes));
        let id = {
            let mut conn = self.connection();
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT OR IGNORE INTO file_blobs(hash,bytes) VALUES(?1,?2)",
                params![hash, bytes],
            )?;
            tx.execute("INSERT INTO files(session_id,name,media_type,origin,source_path) VALUES(?1,?2,?3,?4,?5)",params![session,input.name,input.media_type,input.origin,input.source_path])?;
            let id = tx.last_insert_rowid();
            tx.execute("INSERT INTO file_versions(file_id,revision,hash,text,operation) VALUES(?1,1,?2,?3,?4)",params![id,hash,input.text,input.operation])?;
            tx.commit()?;
            id
        };
        self.file(session, id)
    }
    pub fn file(&self, session: i64, id: i64) -> Result<FileRecord, StoreError> {
        Ok(self.connection().query_row(
            &format!("{SELECT_FILES} WHERE f.session_id=?1 AND f.id=?2 AND f.deleted=0"),
            params![session, id],
            file_row,
        )?)
    }
    pub fn list_files(&self, session: i64) -> Result<Vec<FileRecord>, StoreError> {
        let conn = self.connection();
        let mut stmt = conn.prepare(&format!(
            "{SELECT_FILES} WHERE f.session_id=?1 AND f.deleted=0 ORDER BY f.id DESC"
        ))?;
        Ok(stmt
            .query_map([session], file_row)?
            .collect::<Result<_, _>>()?)
    }
    pub fn search_files(&self, session: i64, query: &str) -> Result<Vec<FileRecord>, StoreError> {
        let conn = self.connection();
        let mut stmt=conn.prepare(&format!("{SELECT_FILES} WHERE f.session_id=?1 AND f.deleted=0 AND (instr(lower(f.name),lower(?2))>0 OR instr(lower(v.text),lower(?2))>0) ORDER BY f.id DESC LIMIT 100"))?;
        Ok(stmt
            .query_map(params![session, query], file_row)?
            .collect::<Result<_, _>>()?)
    }
    pub fn file_bytes(
        &self,
        session: i64,
        id: i64,
        revision: Option<i64>,
        offset: usize,
        length: usize,
    ) -> Result<Vec<u8>, StoreError> {
        if length > FILE_CHUNK_BYTES || offset > MAX_FILE_BYTES {
            return Err(invalid("Invalid file chunk range."));
        }
        let conn = self.connection();
        Ok(conn.query_row("SELECT substr(b.bytes,?4+1,?5) FROM files f JOIN file_versions v ON v.file_id=f.id AND v.revision=COALESCE(?3,f.revision) JOIN file_blobs b ON b.hash=v.hash WHERE f.id=?2 AND f.session_id=?1 AND f.deleted=0",params![session,id,revision,offset as i64,length as i64],|r|r.get(0))?)
    }
    pub fn file_text(
        &self,
        session: i64,
        id: i64,
        revision: Option<i64>,
    ) -> Result<String, StoreError> {
        Ok(self.connection().query_row("SELECT v.text FROM files f JOIN file_versions v ON v.file_id=f.id AND v.revision=COALESCE(?3,f.revision) WHERE f.session_id=?1 AND f.id=?2 AND f.deleted=0",params![session,id,revision],|r|r.get(0))?)
    }
    pub fn prepare_file_text(
        &self,
        session: i64,
        id: i64,
        revision: i64,
        text: &str,
    ) -> Result<(), StoreError> {
        if text.len() > MAX_TEXT_BYTES {
            return Err(invalid("Extracted document text is too large."));
        }
        let file = self.file(session, id)?;
        if ![
            "application/pdf",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            "text/html",
        ]
        .contains(&file.media_type.as_str())
        {
            return Err(invalid("This file does not need document extraction."));
        }
        let changed=self.connection().execute("UPDATE file_versions SET text=?4 WHERE file_id=?2 AND revision=?3 AND EXISTS(SELECT 1 FROM files WHERE id=?2 AND session_id=?1 AND deleted=0)",params![session,id,revision,text])?;
        if changed != 1 {
            return Err(invalid("The file version no longer exists."));
        }
        Ok(())
    }
    pub fn file_versions(&self, session: i64, id: i64) -> Result<Vec<FileVersion>, StoreError> {
        self.file(session, id)?;
        let conn = self.connection();
        let mut stmt=conn.prepare("SELECT v.revision,v.hash,length(b.bytes),v.operation,v.created_at FROM file_versions v JOIN file_blobs b ON b.hash=v.hash WHERE file_id=?1 ORDER BY revision DESC")?;
        Ok(stmt
            .query_map([id], |r| {
                Ok(FileVersion {
                    revision: r.get(0)?,
                    hash: r.get(1)?,
                    size: r.get::<_, i64>(2)? as usize,
                    operation: r.get(3)?,
                    created_at: r.get(4)?,
                })
            })?
            .collect::<Result<_, _>>()?)
    }
    pub fn revise_file(
        &self,
        session: i64,
        id: i64,
        expected: i64,
        text: &str,
        bytes: &[u8],
        operation: &str,
    ) -> Result<FileRecord, StoreError> {
        validate_content(text, bytes)?;
        let hash = format!("{:x}", Sha256::digest(bytes));
        {
            let mut conn = self.connection();
            let tx = conn.transaction()?;
            let changed=tx.execute("UPDATE files SET revision=revision+1 WHERE session_id=?1 AND id=?2 AND revision=?3 AND deleted=0",params![session,id,expected])?;
            if changed != 1 {
                return Err(invalid(
                    "The file changed or was removed. Reload before saving.",
                ));
            }
            tx.execute(
                "INSERT OR IGNORE INTO file_blobs(hash,bytes) VALUES(?1,?2)",
                params![hash, bytes],
            )?;
            tx.execute("INSERT INTO file_versions(file_id,revision,hash,text,operation) VALUES(?1,?2,?3,?4,?5)",params![id,expected+1,hash,text,operation])?;
            tx.commit()?;
        }
        self.file(session, id)
    }
    pub fn rename_file(&self, session: i64, id: i64, name: &str) -> Result<(), StoreError> {
        validate_name(name)?;
        if self.connection().execute(
            "UPDATE files SET name=?3 WHERE session_id=?1 AND id=?2 AND deleted=0",
            params![session, id, name],
        )? != 1
        {
            return Err(invalid("File not found."));
        }
        Ok(())
    }
    /// Unlinks the catalog entry; never removes an original workspace file.
    pub fn remove_file(&self, session: i64, id: i64) -> Result<(), StoreError> {
        self.connection().execute(
            "UPDATE files SET deleted=1 WHERE session_id=?1 AND id=?2",
            params![session, id],
        )?;
        Ok(())
    }
    pub fn link_message_files(
        &self,
        session: i64,
        seq: i64,
        ids: &[i64],
    ) -> Result<(), StoreError> {
        let mut conn = self.connection();
        let tx = conn.transaction()?;
        let owner: Option<i64> = tx
            .query_row("SELECT session_id FROM events WHERE seq=?1", [seq], |r| {
                r.get(0)
            })
            .optional()?;
        if owner != Some(session) {
            return Err(invalid("Message not found in this conversation."));
        }
        for id in ids {
            let changed=tx.execute("INSERT OR IGNORE INTO message_files(session_id,message_seq,file_id,revision) SELECT session_id,?3,id,revision FROM files WHERE session_id=?1 AND id=?2 AND deleted=0",params![session,id,seq])?;
            if changed!=1 && !tx.query_row("SELECT EXISTS(SELECT 1 FROM message_files WHERE message_seq=?1 AND file_id=?2)",params![seq,id],|r|r.get::<_,bool>(0))? {return Err(invalid("Attachment not found in this conversation."));}
        }
        tx.commit()?;
        Ok(())
    }
    pub fn message_files(&self, session: i64, seq: i64) -> Result<Vec<(i64, i64)>, StoreError> {
        let conn = self.connection();
        let mut stmt=conn.prepare("SELECT file_id,revision FROM message_files WHERE session_id=?1 AND message_seq=?2 ORDER BY file_id")?;
        Ok(stmt
            .query_map(params![session, seq], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?)
    }
    pub fn all_message_files(&self, session: i64) -> Result<Vec<(i64, i64, i64)>, StoreError> {
        let conn = self.connection();
        let mut stmt=conn.prepare("SELECT message_seq,file_id,revision FROM message_files WHERE session_id=?1 ORDER BY message_seq,file_id")?;
        Ok(stmt
            .query_map([session], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<_, _>>()?)
    }
    pub fn set_message_file_text(
        &self,
        session: i64,
        seq: i64,
        id: i64,
        text: &str,
    ) -> Result<(), StoreError> {
        if text.len() > MAX_TEXT_BYTES {
            return Err(invalid("Context is too large."));
        }
        self.connection().execute("UPDATE message_files SET context_text=?4 WHERE session_id=?1 AND message_seq=?2 AND file_id=?3",params![session,seq,id,text])?;
        Ok(())
    }
    pub fn message_file_text(&self, session: i64, seq: i64, id: i64) -> Result<String, StoreError> {
        Ok(self.connection().query_row("SELECT context_text FROM message_files WHERE session_id=?1 AND message_seq=?2 AND file_id=?3",params![session,seq,id],|r|r.get(0))?)
    }
}
