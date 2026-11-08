use serde_json::json;
use std::sync::{Arc, Barrier};
use vatra_core::{ItemChange, NewItem, ReminderResult, Store};
fn task(store: &Store, project: i64, due: i64) -> i64 {
    store
        .create_item(
            &NewItem {
                kind: "task".into(),
                title: "Call".into(),
                body: String::new(),
                fields: json!({"remind_at":due,"timezone":"Europe/Bucharest"}),
                project_id: Some(project),
            },
            1,
        )
        .unwrap()
}

#[test]
fn independent_stores_claim_once_and_inbox_is_durable_before_delivery() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.db");
    let a = Store::open(&path).unwrap();
    let project = a.create_project("p").unwrap();
    task(&a, project, 100);
    drop(a);
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let store = Store::open(&path).unwrap();
                barrier.wait();
                store.claim_due_reminders(100, 50).unwrap().len()
            })
        })
        .collect();
    assert_eq!(
        threads
            .into_iter()
            .map(|t| t.join().unwrap())
            .sum::<usize>(),
        1
    );
    let store = Store::open(&path).unwrap();
    let inbox = store.reminder_inbox(project, 50).unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].state, "attempted");
    store.recover_reminders().unwrap();
    assert_eq!(
        store.reminder_inbox(project, 50).unwrap()[0].state,
        "unknown"
    );
    assert!(store.claim_due_reminders(200, 50).unwrap().is_empty());
}
#[test]
fn restart_before_claim_recovers_due_work_and_pages_do_not_drop_items() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.db");
    let store = Store::open(&path).unwrap();
    let project = store.create_project("p").unwrap();
    for _ in 0..3 {
        task(&store, project, 100);
    }
    drop(store);
    let store = Store::open(&path).unwrap();
    assert!(store.claim_due_reminders(99, 2).unwrap().is_empty());
    assert_eq!(store.claim_due_reminders(100, 2).unwrap().len(), 2);
    assert_eq!(store.claim_due_reminders(100, 2).unwrap().len(), 1);
    assert!(store.claim_due_reminders(100, 2).unwrap().is_empty());
}
#[test]
fn rename_does_not_rearm_but_changed_time_invalidates_a_late_outcome() {
    let store = Store::open_in_memory().unwrap();
    let project = store.create_project("p").unwrap();
    let id = task(&store, project, 100);
    let old = store.claim_due_reminders(100, 50).unwrap().remove(0);
    store
        .save_workspace_item(
            project,
            &ItemChange {
                id,
                expected_revision: 1,
                title: Some("Renamed".into()),
                body: None,
                fields: None,
            },
            101,
        )
        .unwrap();
    assert!(store.claim_due_reminders(101, 50).unwrap().is_empty());
    store
        .save_workspace_item(
            project,
            &ItemChange {
                id,
                expected_revision: 2,
                title: None,
                body: None,
                fields: Some(json!({"remind_at":200})),
            },
            102,
        )
        .unwrap();
    store
        .finish_reminder(old.id, old.generation, ReminderResult::Submitted, 103)
        .unwrap();
    assert_eq!(
        store.reminder_inbox(project, 50).unwrap()[0].state,
        "cancelled"
    );
    let next = store.claim_due_reminders(200, 50).unwrap().remove(0);
    assert_eq!(next.generation, old.generation + 1);
    store
        .finish_reminder(next.id, next.generation, ReminderResult::Blocked, 200)
        .unwrap();
    assert_eq!(
        store.reminder_inbox(project, 50).unwrap()[0].state,
        "blocked"
    );
    assert!(
        store
            .ack_reminder(project + 1, next.id, next.generation)
            .is_err()
    );
    store
        .ack_reminder(project, next.id, next.generation)
        .unwrap();
    store
        .ack_reminder(project, next.id, next.generation)
        .unwrap();
    assert_eq!(store.reminder_inbox(project, 50).unwrap().len(), 1);
    assert!(store.claim_due_reminders(300, 50).unwrap().is_empty());
}
#[test]
fn legacy_mutations_completion_deletion_and_restore_cannot_reactivate_an_alarm() {
    let store = Store::open_in_memory().unwrap();
    let project = store.create_project("p").unwrap();
    let id = task(&store, project, 100);
    store
        .update_item(
            id,
            None,
            None,
            Some(&json!({"remind_at":200,"timezone":"UTC"})),
            2,
        )
        .unwrap();
    assert!(store.claim_due_reminders(100, 50).unwrap().is_empty());
    assert_eq!(store.claim_due_reminders(200, 50).unwrap().len(), 1);
    store
        .update_item(
            id,
            None,
            None,
            Some(&json!({"remind_at":200,"timezone":"UTC","status":"completed"})),
            3,
        )
        .unwrap();
    assert_eq!(
        store.reminder_inbox(project, 50).unwrap()[0].state,
        "cancelled"
    );
    let id2 = task(&store, project, 300);
    store.delete_item(id2, 3).unwrap();
    let deleted = store.workspace_item(project, id2, true).unwrap();
    store
        .restore_workspace_item(project, id2, deleted.revision, 4)
        .unwrap();
    assert!(store.claim_due_reminders(300, 50).unwrap().is_empty());
}
#[test]
fn malformed_times_and_dates_fail_without_partial_task_writes() {
    let store = Store::open_in_memory().unwrap();
    let project = store.create_project("p").unwrap();
    for fields in [
        json!({"remind_at":-1,"timezone":"UTC"}),
        json!({"remind_at":253402300800_i64,"timezone":"UTC"}),
        json!({"remind_at":1.5,"timezone":"UTC"}),
        json!({"remind_at":100}),
        json!({"remind_at":100,"timezone":"bad zone"}),
        json!({"due_date":"123é56-78"}),
    ] {
        assert!(
            store
                .create_item(
                    &NewItem {
                        kind: "task".into(),
                        title: "Task".into(),
                        body: String::new(),
                        fields,
                        project_id: Some(project)
                    },
                    1
                )
                .is_err()
        );
    }
    assert!(
        store
            .workspace_items(project, &["task"], false, None, 100)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn accepted_task_batches_schedule_atomically_and_retry_does_not_rearm() {
    use vatra_core::{WorkspaceProposal,ProposalPayload,ProposalState};
    let store=Store::open_in_memory().unwrap();let p=store.create_project("p").unwrap();let session=store.create_session(p,"fixture").unwrap();
    let proposal=store.save_workspace_proposal(&WorkspaceProposal{id:0,project_id:p,session_id:session,client_op_id:"batch".into(),target_id:None,base_revision:None,payload:ProposalPayload::Tasks{items:(0..3).map(|_|NewItem{kind:"task".into(),title:"Meeting".into(),body:String::new(),fields:json!({"remind_at":100,"timezone":"UTC"}),project_id:Some(p)}).collect()},state:ProposalState::Pending,created_at:1}).unwrap();
    assert!(store.claim_due_reminders(100,50).unwrap().is_empty());store.accept_workspace_proposal(p,proposal,2).unwrap();assert_eq!(store.claim_due_reminders(100,50).unwrap().len(),3);store.accept_workspace_proposal(p,proposal,3).unwrap();assert!(store.claim_due_reminders(100,50).unwrap().is_empty());
}
#[test]
fn cancelled_generation_stays_cancelled_after_restoration_and_restart() {
    let dir=tempfile::tempdir().unwrap();let path=dir.path().join("data.db");let store=Store::open(&path).unwrap();let p=store.create_project("p").unwrap();let id=task(&store,p,100);store.delete_item(id,2).unwrap();let deleted=store.workspace_item(p,id,true).unwrap();store.restore_workspace_item(p,id,deleted.revision,3).unwrap();drop(store);
    let store=Store::open(&path).unwrap();store.recover_reminders().unwrap();assert!(store.claim_due_reminders(100,50).unwrap().is_empty());
}
