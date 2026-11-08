use vatra_core::{AuditEntry, Store};

fn entry(at: i64, connector: &str, decision: &str, summary: &str) -> AuditEntry {
    AuditEntry {
        at,
        session_id: Some(1),
        tool: "mcp__demo__send".into(),
        connector: connector.into(),
        summary: summary.into(),
        decision: decision.into(),
        payload_hash: "a".repeat(64),
    }
}

#[test]
fn audit_records_round_trip_and_counts_only_approved_since_a_time() {
    let store = Store::open_in_memory().unwrap();
    let old = entry(100, "mail", "approved", "Old message");
    let approved = entry(200, "mail", "approved", "Send a message to a@b.c");
    let denied = entry(300, "mail", "denied", "Refused");
    let other = entry(300, "chat", "approved", "Post to a channel");
    let blocked = entry(400, "mail", "blocked", "Too many");
    for item in [&old, &approved, &denied, &other, &blocked] {
        store.record_audit(item).unwrap();
    }

    // Newest first, and the round trip keeps every field.
    let recent = store.audit_recent(10).unwrap();
    assert_eq!(recent, vec![blocked.clone(), other.clone(), denied.clone(), approved.clone(), old.clone()]);
    assert_eq!(store.audit_recent(1).unwrap(), vec![blocked]);

    // Only approved rows count, only from the given time on, and only for the connector asked about.
    assert_eq!(store.external_count_since("mail", 200).unwrap(), 1);
    assert_eq!(store.external_count_since("mail", 0).unwrap(), 2);
    assert_eq!(store.external_count_since("mail", 1000).unwrap(), 0);
    assert_eq!(store.external_count_since("chat", 0).unwrap(), 1);
    assert_eq!(store.external_count_since("calendar", 0).unwrap(), 0);

    // A decision outside the three allowed values is refused and nothing is written.
    let bad = AuditEntry { decision: "maybe".into(), ..entry(500, "mail", "approved", "Unknown") };
    assert!(store.record_audit(&bad).is_err());
    assert_eq!(store.audit_recent(10).unwrap().len(), 5);
}

#[test]
fn a_long_summary_is_stored_cut_to_200_characters() {
    let store = Store::open_in_memory().unwrap();
    // Two-byte characters, so a cut by bytes would land inside one.
    store.record_audit(&entry(1, "mail", "approved", &"ă".repeat(1000))).unwrap();
    store.record_audit(&entry(2, "mail", "approved", &"a".repeat(200))).unwrap();
    let recent = store.audit_recent(10).unwrap();
    assert_eq!(recent[0].summary.chars().count(), 200);
    assert_eq!(recent[1].summary, "ă".repeat(200));
}

#[test]
fn approval_reservation_is_atomic_across_independent_connections() {
    use std::sync::{Arc,Barrier};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit.db");
    let store = Store::open(&path).unwrap();
    for _ in 0..19 { store.record_audit(&entry(100,"mail","approved","earlier")).unwrap(); }
    let stores: Vec<_> = (0..8).map(|_| Store::open(&path).unwrap()).collect();
    let barrier = Arc::new(Barrier::new(8));
    let workers: Vec<_> = stores.into_iter().map(|store| {
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            barrier.wait();
            store.try_record_external_approval(&entry(101,"mail","approved","new"),0,20).unwrap()
        })
    }).collect();
    let won = workers.into_iter().map(|worker| usize::from(worker.join().unwrap())).sum::<usize>();
    assert_eq!(won,1);
    assert_eq!(store.external_count_since("mail",0).unwrap(),20);
    assert!(store.try_record_external_approval(&entry(101,"calendar","approved","other connector"),0,20).unwrap());
    assert!(store.try_record_external_approval(&entry(200,"mail","approved","new window"),200,20).unwrap());
    assert!(store.try_record_external_approval(&entry(200,"mail","denied","not approved"),200,20).is_err());
}
