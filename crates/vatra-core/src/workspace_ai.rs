//! A revocable turn capability binds agent proposals to one immutable snapshot.
use crate::workspace_versions::scoped_item;
use crate::{
    NewItem, ProposalPayload, ProposalState, Store, StoreError, WorkspaceError, WorkspaceProposal,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub start_utf16: usize,
    pub end_utf16: usize,
    pub selected_text: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceTurnContext {
    pub session_id: i64,
    pub project_id: i64,
    pub item_id: i64,
    pub base_revision: u64,
    pub base_body: String,
    pub selection: Option<Selection>,
    pub scope_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceTaskInput {
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default = "fields")]
    pub fields: Value,
}
fn fields() -> Value {
    json!({})
}
impl Selection {
    pub fn byte_range(&self, body: &str) -> Result<std::ops::Range<usize>, WorkspaceError> {
        let invalid =
            || WorkspaceError::Invalid("The selection no longer matches this document.".into());
        if self.start_utf16 > self.end_utf16 {
            return Err(invalid());
        }
        let mut utf16 = 0;
        let mut start = None;
        let mut end = None;
        for (byte, ch) in body
            .char_indices()
            .chain(std::iter::once((body.len(), '\0')))
        {
            if utf16 == self.start_utf16 {
                start = Some(byte);
            }
            if utf16 == self.end_utf16 {
                end = Some(byte);
            }
            if byte < body.len() {
                utf16 += ch.len_utf16();
            }
        }
        let range = start.ok_or_else(invalid)?..end.ok_or_else(invalid)?;
        if body.get(range.clone()) != Some(self.selected_text.as_str()) {
            return Err(invalid());
        }
        Ok(range)
    }
}
fn load(
    connection: &Connection,
    session: i64,
) -> Result<Option<WorkspaceTurnContext>, WorkspaceError> {
    let row=connection.query_row("SELECT project_id,item_id,base_revision,base_body,selection_json FROM workspace_turn_contexts WHERE session_id=?1",[session],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?))).optional()?;
    row.map(|(project_id, item_id, revision, base_body, raw)| {
        let data: Value = serde_json::from_str(&raw)?;
        Ok(WorkspaceTurnContext {
            session_id: session,
            project_id,
            item_id,
            base_revision: u64::try_from(revision)
                .map_err(|_| WorkspaceError::Invalid("Invalid revision.".into()))?,
            base_body,
            selection: serde_json::from_value(data["selection"].clone())?,
            scope_id: data["scope_id"]
                .as_str()
                .ok_or_else(|| WorkspaceError::Invalid("Invalid turn capability.".into()))?
                .into(),
        })
    })
    .transpose()
}
pub(crate) fn require_context(
    connection: &Connection,
    scope: &WorkspaceTurnContext,
) -> Result<(), WorkspaceError> {
    if load(connection, scope.session_id)?.as_ref() != Some(scope) {
        return Err(WorkspaceError::Invalid(
            "This workspace turn has ended.".into(),
        ));
    }
    Ok(())
}
fn validate_snapshot(
    connection: &Connection,
    scope: &WorkspaceTurnContext,
) -> Result<(), WorkspaceError> {
    let session_project = connection
        .query_row(
            "SELECT project_id FROM sessions WHERE id=?1",
            [scope.session_id],
            |r| r.get::<_, Option<i64>>(0),
        )
        .optional()?
        .flatten();
    if session_project != Some(scope.project_id) {
        return Err(WorkspaceError::NotFound);
    }
    let item = scoped_item(connection, scope.project_id, scope.item_id, false)?;
    if item.revision != scope.base_revision {
        return Err(WorkspaceError::Conflict(Box::new(item)));
    }
    if item.body != scope.base_body || !matches!(item.kind.as_str(), "document" | "note" | "task") {
        return Err(WorkspaceError::Invalid(
            "The workspace snapshot is invalid.".into(),
        ));
    }
    if let Some(selection) = &scope.selection {
        selection.byte_range(&scope.base_body)?;
    }
    Ok(())
}
impl Store {
    pub fn capture_workspace_context(
        &self,
        session_id: i64,
        project_id: i64,
        item_id: i64,
        base_revision: u64,
        selection: Option<Selection>,
    ) -> Result<WorkspaceTurnContext, WorkspaceError> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let connection = self.connection();
        let item = scoped_item(&connection, project_id, item_id, false)?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let scope = WorkspaceTurnContext {
            session_id,
            project_id,
            item_id,
            base_revision,
            base_body: item.body,
            selection,
            scope_id: format!(
                "{}-{stamp}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ),
        };
        validate_snapshot(&connection, &scope)?;
        Ok(scope)
    }
    pub fn activate_workspace_context(
        &self,
        scope: &WorkspaceTurnContext,
    ) -> Result<(), WorkspaceError> {
        let mut connection = self.connection();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        validate_snapshot(&tx, scope)?;
        tx.execute("INSERT INTO workspace_turn_contexts(session_id,project_id,item_id,base_revision,base_body,selection_json) VALUES(?1,?2,?3,?4,?5,?6)",params![scope.session_id,scope.project_id,scope.item_id,scope.base_revision as i64,scope.base_body,json!({"scope_id":scope.scope_id,"selection":scope.selection}).to_string()])?;
        tx.commit()?;
        Ok(())
    }
    pub fn workspace_turn_context(
        &self,
        session: i64,
    ) -> Result<Option<WorkspaceTurnContext>, WorkspaceError> {
        load(&self.connection(), session)
    }
    pub fn clear_workspace_context(&self, session: i64) -> Result<(), StoreError> {
        self.connection().execute(
            "DELETE FROM workspace_turn_contexts WHERE session_id=?1",
            [session],
        )?;
        Ok(())
    }
    pub fn clear_workspace_contexts(&self) -> Result<(), StoreError> {
        self.connection()
            .execute("DELETE FROM workspace_turn_contexts", [])?;
        Ok(())
    }
    pub fn check_workspace_context(
        &self,
        scope: &WorkspaceTurnContext,
    ) -> Result<(), WorkspaceError> {
        require_context(&self.connection(), scope)
    }
    pub fn propose_workspace_edit(
        &self,
        scope: &WorkspaceTurnContext,
        op: &str,
        replacement: &str,
        now: i64,
    ) -> Result<i64, WorkspaceError> {
        let new_body = if let Some(selection) = &scope.selection {
            let range = selection.byte_range(&scope.base_body)?;
            let mut body = scope.base_body.clone();
            body.replace_range(range, replacement);
            body
        } else {
            replacement.into()
        };
        self.save_workspace_proposal_checked(
            &WorkspaceProposal {
                id: 0,
                session_id: scope.session_id,
                project_id: scope.project_id,
                target_id: Some(scope.item_id),
                base_revision: Some(scope.base_revision),
                client_op_id: op.into(),
                payload: ProposalPayload::Edit {
                    base_body: scope.base_body.clone(),
                    new_body,
                },
                state: ProposalState::Pending,
                created_at: now,
            },
            Some(scope),
        )
    }
    pub fn propose_workspace_tasks(
        &self,
        scope: &WorkspaceTurnContext,
        op: &str,
        tasks: Vec<WorkspaceTaskInput>,
        now: i64,
    ) -> Result<i64, WorkspaceError> {
        let items = tasks
            .into_iter()
            .map(|t| NewItem {
                kind: "task".into(),
                title: t.title,
                body: t.body,
                fields: t.fields,
                project_id: Some(scope.project_id),
            })
            .collect();
        self.save_workspace_proposal_checked(
            &WorkspaceProposal {
                id: 0,
                session_id: scope.session_id,
                project_id: scope.project_id,
                target_id: Some(scope.item_id),
                base_revision: Some(scope.base_revision),
                client_op_id: op.into(),
                payload: ProposalPayload::Tasks { items },
                state: ProposalState::Pending,
                created_at: now,
            },
            Some(scope),
        )
    }
}
