//! Pending agent proposals and idempotent, transactional human acceptance.
use crate::workspace::{index_item, insert_item};
use crate::workspace_versions::{scoped_item, validate_workspace_content};
use crate::{Item, NewItem, Store, WorkspaceError};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProposalPayload {
    Edit { base_body: String, new_body: String },
    Tasks { items: Vec<NewItem> },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalState {
    Pending,
    Applied,
    Rejected,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceProposal {
    pub id: i64,
    pub project_id: i64,
    pub session_id: i64,
    pub client_op_id: String,
    pub target_id: Option<i64>,
    pub base_revision: Option<u64>,
    pub payload: ProposalPayload,
    pub state: ProposalState,
    pub created_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProposalResult {
    pub proposal_id: i64,
    pub items: Vec<Item>,
}
const COLUMNS: &str = "id,project_id,session_id,client_op_id,target_id,base_revision,payload_json,state,created_at";

fn at(row: &rusqlite::Row) -> rusqlite::Result<WorkspaceProposal> {
    let payload: String = row.get(6)?;
    let state: String = row.get(7)?;
    let revision: Option<i64> = row.get(5)?;
    let decoded = serde_json::from_str(&payload)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, Box::new(e)))?;
    let state = match state.as_str() {
        "pending" => ProposalState::Pending,
        "applied" => ProposalState::Applied,
        "rejected" => ProposalState::Rejected,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    Ok(WorkspaceProposal {
        id: row.get(0)?,
        project_id: row.get(1)?,
        session_id: row.get(2)?,
        client_op_id: row.get(3)?,
        target_id: row.get(4)?,
        base_revision: revision.map(|r| r as u64),
        payload: decoded,
        state,
        created_at: row.get(8)?,
    })
}
fn proposal(connection: &Connection, project: i64, id: i64) -> Result<WorkspaceProposal, WorkspaceError> {
    connection
        .query_row(
            &format!("SELECT {COLUMNS} FROM workspace_proposals WHERE id=?1 AND project_id=?2"),
            params![id, project],
            at,
        )
        .optional()?
        .ok_or(WorkspaceError::NotFound)
}
fn validate(connection: &Connection, p: &WorkspaceProposal) -> Result<Option<Item>, WorkspaceError> {
    let session_project = connection
        .query_row("SELECT project_id FROM sessions WHERE id=?1", [p.session_id], |r| {
            r.get::<_, Option<i64>>(0)
        })
        .optional()?
        .flatten();
    if session_project != Some(p.project_id) {
        return Err(WorkspaceError::NotFound);
    }
    let target = p
        .target_id
        .map(|id| scoped_item(connection, p.project_id, id, false))
        .transpose()?;
    if let Some(item) = &target {
        if p.base_revision != Some(item.revision) {
            return Err(WorkspaceError::Conflict(Box::new(item.clone())));
        }
    } else if p.base_revision.is_some() {
        return Err(WorkspaceError::Invalid(
            "A base revision requires a target item.".into(),
        ));
    }
    match &p.payload {
        ProposalPayload::Edit { base_body, new_body } => {
            let item = target
                .as_ref()
                .ok_or_else(|| WorkspaceError::Invalid("An edit needs a target item.".into()))?;
            if base_body != &item.body {
                return Err(WorkspaceError::Invalid(
                    "The proposed base text does not match its revision.".into(),
                ));
            }
            validate_workspace_content(&item.kind, &item.title, new_body, &item.fields)?;
        }
        ProposalPayload::Tasks { items } => {
            if items.is_empty() || items.len() > 100 {
                return Err(WorkspaceError::Invalid("Propose between 1 and 100 tasks.".into()));
            }
            for item in items {
                if item.kind != "task" || item.project_id != Some(p.project_id) {
                    return Err(WorkspaceError::Invalid(
                        "Every proposed task must belong to this project.".into(),
                    ));
                }
                validate_workspace_content(&item.kind, &item.title, &item.body, &item.fields)?;
            }
        }
    }
    Ok(target)
}

impl Store {
    pub fn save_workspace_proposal(&self, p: &WorkspaceProposal) -> Result<i64, WorkspaceError> {
        self.save_workspace_proposal_checked(p, None)
    }
    pub(crate) fn save_workspace_proposal_checked(&self, p: &WorkspaceProposal, scope: Option<&crate::WorkspaceTurnContext>) -> Result<i64, WorkspaceError> {
        if p.id != 0
            || p.state != ProposalState::Pending
            || p.client_op_id.trim().is_empty()
            || p.client_op_id.len() > 128
        {
            return Err(WorkspaceError::Invalid(
                "A new proposal needs a fresh operation id and pending state.".into(),
            ));
        }
        let mut connection = self.connection();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(scope) = scope { crate::workspace_ai::require_context(&tx, scope)?; }
        // A retry identifies immutable content before revalidating a target that may already have changed/applied.
        if let Some(existing) = tx
            .query_row(
                &format!("SELECT {COLUMNS} FROM workspace_proposals WHERE session_id=?1 AND client_op_id=?2"),
                params![p.session_id, p.client_op_id],
                at,
            )
            .optional()?
        {
            if existing.project_id == p.project_id
                && existing.target_id == p.target_id
                && existing.base_revision == p.base_revision
                && existing.payload == p.payload
            {
                return Ok(existing.id);
            }
            return Err(WorkspaceError::Invalid(
                "This operation id already identifies a different proposal.".into(),
            ));
        }
        validate(&tx, p)?;
        let payload = serde_json::to_string(&p.payload)?;
        if payload.len() > 4 * 1024 * 1024 {
            return Err(WorkspaceError::Invalid("The proposal is too large.".into()));
        }
        let revision = p
            .base_revision
            .map(|r| i64::try_from(r).map_err(|_| WorkspaceError::Invalid("Invalid base revision.".into())))
            .transpose()?;
        tx.execute("INSERT INTO workspace_proposals(project_id,session_id,client_op_id,target_id,base_revision,payload_json,state,created_at) VALUES(?1,?2,?3,?4,?5,?6,'pending',?7)",params![p.project_id,p.session_id,p.client_op_id,p.target_id,revision,payload,p.created_at])?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(id)
    }
    pub fn workspace_proposals(
        &self,
        project: i64,
        target: Option<i64>,
        limit: usize,
    ) -> Result<Vec<WorkspaceProposal>, WorkspaceError> {
        let connection = self.connection();
        let mut statement=connection.prepare(&format!("SELECT {COLUMNS} FROM workspace_proposals WHERE project_id=?1 AND (?2 IS NULL OR target_id=?2) ORDER BY id DESC LIMIT ?3"))?;
        let rows = statement.query_map(params![project, target, limit.min(100) as i64], at)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
    /// Bound serialized pages, not only their row count. A cursor keeps every older proposal reachable.
    pub fn workspace_proposals_page(&self,project:i64,target:Option<i64>,after:Option<i64>,limit:usize)->Result<Vec<WorkspaceProposal>,WorkspaceError> {
        let connection=self.connection();
        let mut query=connection.prepare(&format!("SELECT {COLUMNS} FROM (SELECT *,SUM(length(CAST(payload_json AS BLOB))+2048) OVER (ORDER BY id DESC) AS bytes FROM workspace_proposals WHERE project_id=?1 AND (?2 IS NULL OR target_id=?2) AND (?3 IS NULL OR id<?3) ORDER BY id DESC LIMIT ?4) WHERE bytes<=6291456 ORDER BY id DESC"))?;
        Ok(query.query_map(params![project,target,after,limit.min(100) as i64],at)?.collect::<Result<_,_>>()?)
    }
    pub fn accept_workspace_proposal(&self, project: i64, id: i64, now: i64) -> Result<ProposalResult, WorkspaceError> {
        let mut connection = self.connection();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let p = proposal(&tx, project, id)?;
        if p.state == ProposalState::Rejected {
            return Err(WorkspaceError::Invalid("This proposal was discarded.".into()));
        }
        if p.state == ProposalState::Applied {
            let text: String = tx.query_row(
                "SELECT applied_item_ids_json FROM workspace_proposals WHERE id=?1",
                [id],
                |r| r.get(0),
            )?;
            let ids: Vec<i64> = serde_json::from_str(&text)?;
            let items = ids
                .into_iter()
                .map(|id| scoped_item(&tx, project, id, true))
                .collect::<Result<_, _>>()?;
            return Ok(ProposalResult { proposal_id: id, items });
        }
        let target = validate(&tx, &p)?;
        let mut ids = Vec::new();
        match &p.payload {
            ProposalPayload::Edit { new_body, .. } => {
                let target = target.ok_or(WorkspaceError::NotFound)?;
                tx.execute(
                    "UPDATE items SET body=?2,updated_at=?3,revision=revision+1 WHERE id=?1",
                    params![target.id, new_body, now],
                )?;
                tx.execute("DELETE FROM items_fts WHERE rowid=?1", [target.id])?;
                index_item(&tx, target.id, &target.kind, &target.title, new_body)?;
                ids.push(target.id);
            }
            ProposalPayload::Tasks { items } => {
                for item in items {
                    let item_id = insert_item(&tx, item, now)?;
                    if let Some(target) = &target {
                        tx.execute(
                            "INSERT INTO item_links(from_id,to_id,rel) VALUES(?1,?2,'derived_from')",
                            params![item_id, target.id],
                        )?;
                    }
                    ids.push(item_id);
                }
            }
        }
        tx.execute(
            "UPDATE workspace_proposals SET state='applied',applied_item_ids_json=?2 WHERE id=?1",
            params![id, serde_json::to_string(&ids)?],
        )?;
        let items = ids
            .into_iter()
            .map(|id| scoped_item(&tx, project, id, false))
            .collect::<Result<_, _>>()?;
        tx.commit()?;
        Ok(ProposalResult { proposal_id: id, items })
    }
    pub fn reject_workspace_proposal(&self, project: i64, id: i64) -> Result<(), WorkspaceError> {
        let mut connection = self.connection();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let p = proposal(&tx, project, id)?;
        if p.state == ProposalState::Applied {
            return Err(WorkspaceError::Invalid(
                "An applied proposal cannot be discarded.".into(),
            ));
        }
        tx.execute("UPDATE workspace_proposals SET state='rejected' WHERE id=?1", [id])?;
        tx.commit()?;
        Ok(())
    }
}
