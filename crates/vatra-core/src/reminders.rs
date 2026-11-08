//! Durable at-most-once reminder attempts; notification submission is never an observation.
use crate::{Store, StoreError};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reminder {
    pub id: i64,
    pub item_id: i64,
    pub project_id: i64,
    pub generation: i64,
    pub due_at: i64,
    pub state: String,
    pub attempted_at: Option<i64>,
    pub result: Option<String>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReminderResult {
    Submitted,
    Blocked,
    Failed,
    Unknown,
}
impl ReminderResult {
    fn name(self) -> &'static str {
        match self {
            Self::Submitted => "submitted",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }
}
const COLUMNS: &str =
    "r.id,r.item_id,r.project_id,r.generation,r.due_at,r.state,r.attempted_at,r.result";
fn at(r: &rusqlite::Row) -> rusqlite::Result<Reminder> {
    Ok(Reminder {
        id: r.get(0)?,
        item_id: r.get(1)?,
        project_id: r.get(2)?,
        generation: r.get(3)?,
        due_at: r.get(4)?,
        state: r.get(5)?,
        attempted_at: r.get(6)?,
        result: r.get(7)?,
    })
}

/// Called inside the item's write transaction. An unchanged instant never rearms a cancelled attempt.
pub(crate) fn reconcile(
    connection: &Connection,
    id: i64,
    previous: Option<i64>,
    new_item: bool,
) -> Result<(), StoreError> {
    crate::workspace_versions::cancel_inactive_reminders(connection, id)?;
    let (kind, raw, project, deleted): (String, String, Option<i64>, Option<i64>) = connection
        .query_row(
            "SELECT kind,fields_json,project_id,deleted_at FROM items WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
    if kind != "task" {
        return Ok(());
    }
    let fields: serde_json::Value = serde_json::from_str(&raw)?;
    let task = crate::workspace_versions::validate_task_metadata(&fields)
        .map_err(|e| StoreError::Invalid(e.to_string()))?;
    let (Some(due), Some(project)) = (task.remind_at, project) else {
        return Ok(());
    };
    if deleted.is_some()
        || task.status == crate::TaskStatus::Completed
        || (!new_item && previous == Some(due))
    {
        return Ok(());
    }
    let generation: i64 = connection.query_row(
        "SELECT COALESCE(MAX(generation),0)+1 FROM reminders WHERE item_id=?1",
        [id],
        |r| r.get(0),
    )?;
    connection.execute(
        "INSERT INTO reminders(item_id,project_id,generation,due_at) VALUES(?1,?2,?3,?4)",
        params![id, project, generation, due],
    )?;
    Ok(())
}
impl Store {
    pub fn recover_reminders(&self) -> Result<(), StoreError> {
        let mut connection = self.connection();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE reminders SET state='unknown',result='unknown' WHERE state='attempted'",
            [],
        )?;
        // A v6 database can already contain task metadata. Only rows with no history are first scheduled;
        // cancelled/restored or already attempted generations are never rearmed here.
        let candidates = {
            let mut query=tx.prepare("SELECT id,fields_json FROM items i WHERE kind='task' AND project_id IS NOT NULL AND deleted_at IS NULL AND NOT EXISTS (SELECT 1 FROM reminders r WHERE r.item_id=i.id)")?;
            query
                .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
                .collect::<Result<Vec<_>, _>>()?
        };
        for (id, raw) in candidates {
            let fields: serde_json::Value = serde_json::from_str(&raw)?;
            if crate::workspace_versions::validate_task_metadata(&fields).is_ok() {
                reconcile(&tx, id, None, true)?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    pub fn claim_due_reminders(&self, now: i64, limit: usize) -> Result<Vec<Reminder>, StoreError> {
        let mut connection = self.connection();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut query=tx.prepare(&format!("SELECT {COLUMNS} FROM reminders r JOIN items i ON i.id=r.item_id WHERE r.state='scheduled' AND r.due_at<=?1 AND i.deleted_at IS NULL AND i.project_id=r.project_id AND i.kind='task' AND COALESCE(json_extract(i.fields_json,'$.status'),'pending')!='completed' AND json_extract(i.fields_json,'$.remind_at')=r.due_at AND NOT EXISTS (SELECT 1 FROM reminders newer WHERE newer.item_id=r.item_id AND newer.generation>r.generation) ORDER BY r.due_at,r.id LIMIT ?2"))?;
        let mut reminders = query
            .query_map(params![now, limit.min(100) as i64], at)?
            .collect::<Result<Vec<_>, _>>()?;
        drop(query);
        for reminder in &mut reminders {
            tx.execute("UPDATE reminders SET state='attempted',attempted_at=?2 WHERE id=?1 AND state='scheduled'",params![reminder.id,now])?;
            tx.execute(
                "INSERT INTO reminder_inbox(reminder_id,created_at) VALUES(?1,?2)",
                params![reminder.id, now],
            )?;
            reminder.state = "attempted".into();
            reminder.attempted_at = Some(now);
        }
        tx.commit()?;
        Ok(reminders)
    }
    pub fn reminder_is_current(&self, id: i64, generation: i64) -> Result<bool, StoreError> {
        Ok(self.connection().query_row("SELECT 1 FROM reminders r JOIN items i ON i.id=r.item_id WHERE r.id=?1 AND r.generation=?2 AND r.state='attempted' AND i.deleted_at IS NULL AND COALESCE(json_extract(i.fields_json,'$.status'),'pending')!='completed' AND json_extract(i.fields_json,'$.remind_at')=r.due_at AND NOT EXISTS (SELECT 1 FROM reminders n WHERE n.item_id=r.item_id AND n.generation>r.generation)",params![id,generation],|r|r.get::<_,i64>(0)).optional()?.is_some())
    }
    pub fn finish_reminder(
        &self,
        id: i64,
        generation: i64,
        result: ReminderResult,
        _now: i64,
    ) -> Result<(), StoreError> {
        self.connection().execute("UPDATE reminders SET state=?3,result=?3 WHERE id=?1 AND generation=?2 AND state='attempted'",params![id,generation,result.name()])?;
        Ok(())
    }
    pub fn reminder_inbox(&self, project: i64, limit: usize) -> Result<Vec<Reminder>, StoreError> {
        let connection = self.connection();
        let mut query=connection.prepare(&format!("SELECT {COLUMNS} FROM reminder_inbox b JOIN reminders r ON r.id=b.reminder_id WHERE r.project_id=?1 AND b.acked_at IS NULL ORDER BY b.created_at DESC,r.id DESC LIMIT ?2"))?;
        Ok(query
            .query_map(params![project, limit.min(100) as i64], at)?
            .collect::<Result<_, _>>()?)
    }
    pub fn ack_reminder(&self, project: i64, id: i64, generation: i64) -> Result<(), StoreError> {
        let changed=self.connection().execute("UPDATE reminder_inbox SET acked_at=COALESCE(acked_at,created_at) WHERE reminder_id IN (SELECT id FROM reminders WHERE id=?1 AND project_id=?2 AND generation=?3)",params![id,project,generation])?;
        if changed == 0 {
            return Err(StoreError::Invalid(
                "The reminder was not found in this project.".into(),
            ));
        }
        Ok(())
    }
}
