use serde_json::json;
use vatra_core::{ItemChange, NewItem, ProposalPayload, ProposalState, Store, WorkspaceError, WorkspaceProposal};

fn fixture() -> (Store, i64, i64, i64) {
    let store = Store::open_in_memory().unwrap();
    let project = store.create_project("A").unwrap();
    let session = store.create_session(project, "local").unwrap();
    let item = store
        .create_item(
            &NewItem {
                kind: "document".into(),
                title: "Draft".into(),
                body: "old".into(),
                fields: json!({}),
                project_id: Some(project),
            },
            1,
        )
        .unwrap();
    (store, project, session, item)
}
fn edit(project: i64, session: i64, item: i64) -> WorkspaceProposal {
    WorkspaceProposal {
        id: 0,
        project_id: project,
        session_id: session,
        client_op_id: "operation-1".into(),
        target_id: Some(item),
        base_revision: Some(1),
        payload: ProposalPayload::Edit {
            base_body: "old".into(),
            new_body: "new".into(),
        },
        state: ProposalState::Pending,
        created_at: 2,
    }
}
fn task(project: i64, title: &str) -> NewItem {
    NewItem {
        kind: "task".into(),
        title: title.into(),
        body: String::new(),
        fields: json!({"status":"pending"}),
        project_id: Some(project),
    }
}

#[test]
fn proposal_does_not_change_the_source() {
    let (store, p, s, id) = fixture();
    let proposal = edit(p, s, id);
    let key = store.save_workspace_proposal(&proposal).unwrap();
    assert_eq!(store.workspace_item(p, id, false).unwrap().body, "old");
    let listed = store.workspace_proposals(p, Some(id), 50).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, key);
    assert_eq!(store.save_workspace_proposal(&proposal).unwrap(), key);
    assert_eq!(store.accept_workspace_proposal(p, key, 3).unwrap().items[0].body, "new");
    assert_eq!(store.workspace_item(p, id, false).unwrap().revision, 2);
}
#[test]
fn stale_proposal_cannot_apply() {
    let (store, p, s, id) = fixture();
    let key = store.save_workspace_proposal(&edit(p, s, id)).unwrap();
    store
        .save_workspace_item(
            p,
            &ItemChange {
                id,
                expected_revision: 1,
                title: None,
                body: Some("human".into()),
                fields: None,
            },
            3,
        )
        .unwrap();
    assert!(matches!(
        store.accept_workspace_proposal(p, key, 4),
        Err(WorkspaceError::Conflict(_))
    ));
    assert_eq!(store.workspace_item(p, id, false).unwrap().body, "human");
    assert_eq!(
        store.workspace_proposals(p, Some(id), 50).unwrap()[0].state,
        ProposalState::Pending
    );
}
#[test]
fn duplicate_accept_creates_tasks_only_once() {
    let (store, p, s, id) = fixture();
    let mut proposal = edit(p, s, id);
    proposal.payload = ProposalPayload::Tasks {
        items: vec![task(p, "One"), task(p, "Two"), task(p, "Three")],
    };
    let key = store.save_workspace_proposal(&proposal).unwrap();
    assert!(store.items_by_kind("task", 10).unwrap().is_empty());
    let first = store.accept_workspace_proposal(p, key, 3).unwrap();
    let second = store.accept_workspace_proposal(p, key, 4).unwrap();
    assert_eq!(
        first.items.iter().map(|i| i.id).collect::<Vec<_>>(),
        second.items.iter().map(|i| i.id).collect::<Vec<_>>()
    );
    assert_eq!(store.items_by_kind("task", 10).unwrap().len(), 3);
    assert_eq!(store.workspace_links(p, id).unwrap().len(), 3);
}
#[test]
fn task_batch_is_all_or_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("batch.db");
    let store = Store::open(&path).unwrap();
    let p = store.create_project("A").unwrap();
    let s = store.create_session(p, "local").unwrap();
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TRIGGER reject_one BEFORE INSERT ON items WHEN NEW.title='reject' BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;").unwrap();
    let proposal = WorkspaceProposal {
        id: 0,
        project_id: p,
        session_id: s,
        client_op_id: "batch".into(),
        target_id: None,
        base_revision: None,
        payload: ProposalPayload::Tasks {
            items: vec![task(p, "first"), task(p, "reject")],
        },
        state: ProposalState::Pending,
        created_at: 2,
    };
    let key = store.save_workspace_proposal(&proposal).unwrap();
    assert!(store.accept_workspace_proposal(p, key, 3).is_err());
    assert!(store.items_by_kind("task", 100).unwrap().is_empty());
    assert!(store.search_workspace("first", &[], 10).unwrap().is_empty());
    assert_eq!(
        store.workspace_proposals(p, None, 50).unwrap()[0].state,
        ProposalState::Pending
    );
}
#[test]
fn a_deleted_or_other_project_target_is_refused() {
    let (store, p, s, id) = fixture();
    let other = store.create_project("B").unwrap();
    let foreign_session = store.create_session(other, "local").unwrap();
    assert!(
        store
            .save_workspace_proposal(&edit(other, foreign_session, id))
            .is_err()
    );
    assert!(store.save_workspace_proposal(&edit(p, foreign_session, id)).is_err());
    let key = store.save_workspace_proposal(&edit(p, s, id)).unwrap();
    store.delete_workspace_item(p, id, 1, 3).unwrap();
    assert!(store.accept_workspace_proposal(p, key, 4).is_err());
    assert!(store.accept_workspace_proposal(other, key, 4).is_err());
}
#[test]
fn rejection_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("proposal.db");
    let store = Store::open(&path).unwrap();
    let p = store.create_project("A").unwrap();
    let s = store.create_session(p, "local").unwrap();
    let id = store
        .create_item(
            &NewItem {
                kind: "note".into(),
                title: "Draft".into(),
                body: "old".into(),
                fields: json!({}),
                project_id: Some(p),
            },
            1,
        )
        .unwrap();
    let key = store.save_workspace_proposal(&edit(p, s, id)).unwrap();
    store.reject_workspace_proposal(p, key).unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    assert_eq!(
        store.workspace_proposals(p, Some(id), 50).unwrap()[0].state,
        ProposalState::Rejected
    );
    assert!(store.accept_workspace_proposal(p, key, 3).is_err());
    assert_eq!(store.workspace_item(p, id, false).unwrap().body, "old");
}
