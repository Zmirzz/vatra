//! What Vatra keeps about email beside the mailbox itself: labels, a triage class and a summary per message, and the
//! drafts a person is writing. The mail stays on the server; a message here is named only by its Message-ID header.
//! A draft is never sent by being saved: sending is a separate step the person takes.

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::store::{Store, StoreError};

pub(crate) const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS email_meta (
        account TEXT NOT NULL,
        msg_key TEXT NOT NULL,
        labels_json TEXT NOT NULL DEFAULT '[]',
        triage TEXT,
        summary TEXT,
        updated_at INTEGER NOT NULL,
        PRIMARY KEY (account, msg_key)
    );
    CREATE TABLE IF NOT EXISTS email_drafts (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        account TEXT NOT NULL,
        in_reply_to TEXT,
        to_json TEXT NOT NULL,
        subject TEXT NOT NULL,
        body TEXT NOT NULL,
        state TEXT NOT NULL DEFAULT 'draft',
        error TEXT,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        sent_at INTEGER
    );
";

pub(crate) const SCHEMA_V13: &str = "
    CREATE TABLE IF NOT EXISTS email_contacts (
        account TEXT NOT NULL,
        address TEXT NOT NULL,
        name TEXT NOT NULL DEFAULT '',
        uses INTEGER NOT NULL DEFAULT 1,
        last_seen INTEGER NOT NULL,
        PRIMARY KEY (account, address)
    );
";

pub const TRIAGE_CLASSES: [&str; 5] = ["urgent", "reply", "fyi", "newsletter", "other"];

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EmailMeta {
    pub labels: Vec<String>,
    pub triage: Option<String>,
    pub summary: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EmailDraft {
    pub id: i64,
    pub account: String,
    pub in_reply_to: Option<String>,
    pub to: Vec<String>,
    pub subject: String,
    pub body: String,
    /// `draft`, `sent` or `failed`.
    pub state: String,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub sent_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NewEmailDraft {
    pub account: String,
    #[serde(default)]
    pub in_reply_to: Option<String>,
    pub to: Vec<String>,
    pub subject: String,
    pub body: String,
}

const DRAFT_COLUMNS: &str = "id,account,in_reply_to,to_json,subject,body,state,error,created_at,updated_at,sent_at";

fn draft_row(row: &rusqlite::Row) -> rusqlite::Result<EmailDraft> {
    let to: String = row.get(3)?;
    Ok(EmailDraft {
        id: row.get(0)?,
        account: row.get(1)?,
        in_reply_to: row.get(2)?,
        to: serde_json::from_str(&to).unwrap_or_default(),
        subject: row.get(4)?,
        body: row.get(5)?,
        state: row.get(6)?,
        error: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
        sent_at: row.get(10)?,
    })
}

/// A label is a short plain word: letters, digits, spaces, dashes and underscores.
pub fn valid_label(label: &str) -> bool {
    let label = label.trim();
    !label.is_empty() && label.chars().count() <= 32 && label.chars().all(|c| c.is_alphanumeric() || " -_".contains(c))
}

/// An address that can go in a To line: one `@`, no spaces, no characters that could start another header.
pub fn valid_address(address: &str) -> bool {
    let address = address.trim();
    let Some((local, domain)) = address.split_once('@') else { return false };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && address.len() <= 254
        && address.chars().all(|c| c.is_ascii_graphic() && !"<>(),;:\\\"[]".contains(c))
        && !domain.contains('@')
}

impl Store {
    pub fn email_meta(&self, account: &str, key: &str) -> Result<EmailMeta, StoreError> {
        let found = self
            .connection()
            .query_row(
                "SELECT labels_json,triage,summary FROM email_meta WHERE account=?1 AND msg_key=?2",
                params![account, key],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?)),
            )
            .optional()?;
        Ok(found
            .map(|(labels, triage, summary)| EmailMeta { labels: serde_json::from_str(&labels).unwrap_or_default(), triage, summary })
            .unwrap_or_default())
    }

    pub fn set_email_labels(&self, account: &str, key: &str, labels: &[String], now: i64) -> Result<(), StoreError> {
        let mut clean: Vec<String> = Vec::new();
        for label in labels {
            if !valid_label(label) {
                return Err(StoreError::Invalid("A label is a short word of letters, digits, spaces or dashes.".into()));
            }
            let label = label.trim().to_string();
            if !clean.contains(&label) {
                clean.push(label);
            }
        }
        if clean.len() > 12 {
            return Err(StoreError::Invalid("A message can have at most 12 labels.".into()));
        }
        self.connection().execute(
            "INSERT INTO email_meta (account,msg_key,labels_json,updated_at) VALUES (?1,?2,?3,?4)
             ON CONFLICT(account,msg_key) DO UPDATE SET labels_json=excluded.labels_json,updated_at=excluded.updated_at",
            params![account, key, serde_json::to_string(&clean)?, now],
        )?;
        Ok(())
    }

    pub fn set_email_triage(&self, account: &str, key: &str, triage: &str, now: i64) -> Result<(), StoreError> {
        if !TRIAGE_CLASSES.contains(&triage) {
            return Err(StoreError::Invalid("That isn't a triage class.".into()));
        }
        self.connection().execute(
            "INSERT INTO email_meta (account,msg_key,triage,updated_at) VALUES (?1,?2,?3,?4)
             ON CONFLICT(account,msg_key) DO UPDATE SET triage=excluded.triage,updated_at=excluded.updated_at",
            params![account, key, triage, now],
        )?;
        Ok(())
    }

    pub fn set_email_summary(&self, account: &str, key: &str, summary: &str, now: i64) -> Result<(), StoreError> {
        let summary: String = summary.chars().take(4000).collect();
        self.connection().execute(
            "INSERT INTO email_meta (account,msg_key,summary,updated_at) VALUES (?1,?2,?3,?4)
             ON CONFLICT(account,msg_key) DO UPDATE SET summary=excluded.summary,updated_at=excluded.updated_at",
            params![account, key, summary, now],
        )?;
        Ok(())
    }

    /// Remembers someone the person has had mail with, for completing addresses later.
    pub fn remember_contact(&self, account: &str, address: &str, name: &str, now: i64) -> Result<(), StoreError> {
        let address = address.trim().to_lowercase();
        if !valid_address(&address) {
            return Ok(());
        }
        let name: String = name.trim().trim_matches('"').chars().filter(|c| !c.is_control()).take(80).collect();
        self.connection().execute(
            "INSERT INTO email_contacts (account,address,name,last_seen) VALUES (?1,?2,?3,?4)
             ON CONFLICT(account,address) DO UPDATE SET uses=uses+1,last_seen=excluded.last_seen,
               name=CASE WHEN excluded.name!='' THEN excluded.name ELSE name END",
            params![account, address, name, now],
        )?;
        Ok(())
    }

    /// People whose name or address starts like `query`, the most used first.
    pub fn email_contacts(&self, account: &str, query: &str, limit: usize) -> Result<Vec<(String, String)>, StoreError> {
        let like = format!("%{}%", query.trim().to_lowercase().replace(['%', '_'], ""));
        let connection = self.connection();
        let mut statement = connection.prepare(
            "SELECT address,name FROM email_contacts WHERE account=?1 AND (lower(address) LIKE ?2 OR lower(name) LIKE ?2)
             ORDER BY uses DESC,last_seen DESC LIMIT ?3",
        )?;
        let rows = statement.query_map(params![account, like, limit.min(20) as i64], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn create_email_draft(&self, draft: &NewEmailDraft, now: i64) -> Result<i64, StoreError> {
        check_draft(&draft.to, &draft.subject, &draft.body)?;
        let connection = self.connection();
        connection.execute(
            "INSERT INTO email_drafts (account,in_reply_to,to_json,subject,body,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?6)",
            params![draft.account, draft.in_reply_to, serde_json::to_string(&draft.to)?, draft.subject, draft.body, now],
        )?;
        Ok(connection.last_insert_rowid())
    }

    /// Changes a draft that has not been sent.
    pub fn update_email_draft(&self, id: i64, to: &[String], subject: &str, body: &str, now: i64) -> Result<bool, StoreError> {
        check_draft(to, subject, body)?;
        Ok(self.connection().execute(
            "UPDATE email_drafts SET to_json=?2,subject=?3,body=?4,updated_at=?5,state='draft',error=NULL WHERE id=?1 AND state!='sent'",
            params![id, serde_json::to_string(to)?, subject, body, now],
        )? > 0)
    }

    pub fn email_draft(&self, id: i64) -> Result<Option<EmailDraft>, StoreError> {
        Ok(self
            .connection()
            .query_row(&format!("SELECT {DRAFT_COLUMNS} FROM email_drafts WHERE id=?1"), [id], draft_row)
            .optional()?)
    }

    pub fn email_drafts(&self, account: Option<&str>) -> Result<Vec<EmailDraft>, StoreError> {
        let connection = self.connection();
        let mut statement = connection.prepare(&format!(
            "SELECT {DRAFT_COLUMNS} FROM email_drafts WHERE (?1 IS NULL OR account=?1) ORDER BY updated_at DESC,id DESC LIMIT 200"
        ))?;
        let rows = statement.query_map([account], draft_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn delete_email_draft(&self, id: i64) -> Result<bool, StoreError> {
        Ok(self.connection().execute("DELETE FROM email_drafts WHERE id=?1 AND state!='sent'", [id])? > 0)
    }

    /// Marks the draft sent. Only a draft still in `draft` or `failed` can be marked; a draft is sent once.
    pub fn finish_email_draft(&self, id: i64, error: Option<&str>, now: i64) -> Result<bool, StoreError> {
        let (state, sent) = if error.is_some() { ("failed", None) } else { ("sent", Some(now)) };
        Ok(self.connection().execute(
            "UPDATE email_drafts SET state=?2,error=?3,sent_at=?4,updated_at=?5 WHERE id=?1 AND state!='sent'",
            params![id, state, error, sent, now],
        )? > 0)
    }

    /// Takes the draft for sending: only one caller wins, and only a draft that wasn't sent.
    pub fn claim_email_draft(&self, id: i64, now: i64) -> Result<Option<EmailDraft>, StoreError> {
        let connection = self.connection();
        let changed = connection.execute(
            "UPDATE email_drafts SET state='sending',updated_at=?2 WHERE id=?1 AND state IN ('draft','failed')",
            params![id, now],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        Ok(connection
            .query_row(&format!("SELECT {DRAFT_COLUMNS} FROM email_drafts WHERE id=?1"), [id], draft_row)
            .optional()?)
    }

    /// At start-up a draft left `sending` may or may not have gone out; it is marked failed so the person decides.
    pub fn recover_email_drafts(&self, now: i64) -> Result<usize, StoreError> {
        Ok(self.connection().execute(
            "UPDATE email_drafts SET state='failed',error='The service stopped while sending. Check your Sent folder before sending again.',updated_at=?1 WHERE state='sending'",
            [now],
        )?)
    }
}

fn check_draft(to: &[String], subject: &str, body: &str) -> Result<(), StoreError> {
    if to.is_empty() || to.len() > 20 {
        return Err(StoreError::Invalid("A message needs between 1 and 20 recipients.".into()));
    }
    if let Some(bad) = to.iter().find(|a| !valid_address(a)) {
        return Err(StoreError::Invalid(format!("`{}` isn't an email address.", bad.chars().take(60).collect::<String>())));
    }
    if subject.chars().any(char::is_control) || subject.chars().count() > 300 {
        return Err(StoreError::Invalid("The subject must be one line of at most 300 characters.".into()));
    }
    if body.len() > 200_000 {
        return Err(StoreError::Invalid("The message is too long.".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft() -> NewEmailDraft {
        NewEmailDraft { account: "a".into(), in_reply_to: None, to: vec!["ana@example.com".into()], subject: "Salut".into(), body: "Buna".into() }
    }

    #[test]
    fn addresses_cannot_smuggle_headers() {
        assert!(valid_address("ana@example.com"));
        assert!(!valid_address("ana@example.com\r\nBcc: x@y.z"));
        assert!(!valid_address("a b@example.com"));
        assert!(!valid_address("<a@example.com>"));
        assert!(!valid_address("a@b"));
        assert!(!valid_address("a@@example.com"));
    }

    #[test]
    fn a_draft_is_sent_once() {
        let store = Store::open_in_memory().unwrap();
        let id = store.create_email_draft(&draft(), 10).unwrap();
        assert!(store.claim_email_draft(id, 11).unwrap().is_some());
        assert!(store.claim_email_draft(id, 12).unwrap().is_none(), "a second send is refused while the first runs");
        store.finish_email_draft(id, None, 13).unwrap();
        assert!(store.claim_email_draft(id, 14).unwrap().is_none(), "and after it");
        assert!(!store.update_email_draft(id, &["x@y.zz".into()], "s", "b", 15).unwrap());
        assert!(!store.delete_email_draft(id).unwrap());
    }

    #[test]
    fn a_failed_send_can_be_tried_again_and_a_stopped_one_is_marked() {
        let store = Store::open_in_memory().unwrap();
        let id = store.create_email_draft(&draft(), 10).unwrap();
        store.claim_email_draft(id, 11).unwrap().unwrap();
        store.finish_email_draft(id, Some("refused"), 12).unwrap();
        assert!(store.claim_email_draft(id, 13).unwrap().is_some());
        assert_eq!(store.recover_email_drafts(14).unwrap(), 1);
        let after = store.email_draft(id).unwrap().unwrap();
        assert_eq!(after.state, "failed");
        assert!(after.error.unwrap().contains("Sent folder"));
    }

    #[test]
    fn labels_triage_and_summary_live_together() {
        let store = Store::open_in_memory().unwrap();
        store.set_email_labels("a", "<1@x>", &["Work".into(), "Work".into(), "Bills 2026".into()], 1).unwrap();
        store.set_email_triage("a", "<1@x>", "urgent", 2).unwrap();
        store.set_email_summary("a", "<1@x>", "Plateste factura.", 3).unwrap();
        let meta = store.email_meta("a", "<1@x>").unwrap();
        assert_eq!(meta.labels, vec!["Work", "Bills 2026"]);
        assert_eq!(meta.triage.as_deref(), Some("urgent"));
        assert_eq!(meta.summary.as_deref(), Some("Plateste factura."));
        assert!(store.set_email_triage("a", "<1@x>", "boom", 4).is_err());
        assert!(store.set_email_labels("a", "<1@x>", &["<script>".into()], 4).is_err());
        assert_eq!(store.email_meta("a", "<missing>").unwrap(), EmailMeta::default());
    }

    #[test]
    fn bad_drafts_are_refused() {
        let store = Store::open_in_memory().unwrap();
        let mut bad = draft();
        bad.subject = "a\r\nBcc: x@y.zz".into();
        assert!(store.create_email_draft(&bad, 1).is_err());
        bad = draft();
        bad.to = vec![];
        assert!(store.create_email_draft(&bad, 1).is_err());
    }
}
