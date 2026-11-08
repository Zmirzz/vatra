use vatra_core::{NewFile, Store};

#[test]
fn files_survive_restart_keep_versions_and_enforce_session_scope() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("files.sqlite");
    let store = Store::open(&db).unwrap();
    let a = store.create_session(None, "local").unwrap();
    let b = store.create_session(None, "local").unwrap();
    let input = NewFile {
        name: "report.md".into(),
        media_type: "text/markdown".into(),
        origin: "attached".into(),
        source_path: None,
        operation: None,
        text: "first".into(),
    };
    let file = store.import_file(a, &input, b"first").unwrap();
    assert!(store.file_bytes(b, file.id, None, 0, 100).is_err());
    store
        .revise_file(a, file.id, 1, "second", b"second", "edit-1")
        .unwrap();
    assert!(
        store
            .revise_file(a, file.id, 1, "stale", b"stale", "edit-2")
            .is_err()
    );
    drop(store);
    let store = Store::open(&db).unwrap();
    assert_eq!(store.list_files(a).unwrap()[0].revision, 2);
    assert_eq!(
        store.file_bytes(a, file.id, Some(1), 0, 100).unwrap(),
        b"first"
    );
    assert_eq!(
        store.file_bytes(a, file.id, None, 0, 100).unwrap(),
        b"second"
    );
    assert_eq!(store.file_versions(a, file.id).unwrap().len(), 2);
    store.rename_file(a, file.id, "renamed.md").unwrap();
    assert!(store.rename_file(a, file.id, "../escape").is_err());
    assert_eq!(store.search_files(a, "second").unwrap().len(), 1);
    store.remove_file(a, file.id).unwrap();
    assert!(store.list_files(a).unwrap().is_empty());
    assert!(store.file_bytes(a, file.id, None, 0, 100).is_err());
    assert!(store.delete_session(a).unwrap());
}

#[test]
fn invalid_files_never_enter_the_catalog() {
    let store = Store::open_in_memory().unwrap();
    let session = store.create_session(None, "local").unwrap();
    let file = NewFile {
        name: "../escape".into(),
        media_type: "text/plain".into(),
        origin: "attached".into(),
        source_path: None,
        operation: None,
        text: String::new(),
    };
    assert!(store.import_file(session, &file, b"text").is_err());
    assert!(store.list_files(session).unwrap().is_empty());
}

#[test]
fn upload_chunks_are_ordered_bounded_and_session_owned() {
    let store = Store::open_in_memory().unwrap();
    let a = store.create_session(None, "local").unwrap();
    let b = store.create_session(None, "local").unwrap();
    store.begin_file_upload(a, "upload-1", 6).unwrap();
    assert!(store.append_file_upload(b, "upload-1", 0, b"abc").is_err());
    assert!(store.append_file_upload(a, "upload-1", 3, b"def").is_err());
    store.append_file_upload(a, "upload-1", 0, b"abc").unwrap();
    assert!(store.read_file_upload(a, "upload-1").is_err());
    store.append_file_upload(a, "upload-1", 3, b"def").unwrap();
    assert_eq!(store.read_file_upload(a, "upload-1").unwrap(), b"abcdef");
    assert!(
        store
            .append_file_upload(a, "upload-1", 6, b"overflow")
            .is_err()
    );
    store.cancel_file_upload(a, "upload-1").unwrap();
    assert!(store.read_file_upload(a, "upload-1").is_err());
}

#[tokio::test]
async fn attached_inputs_reach_the_agent_without_polluting_the_visible_prompt() {
    mod_agent_test::verify().await;
}
mod mod_agent_test {
    use super::*;
    use std::sync::Mutex;
    use vatra_core::{
        Agent, AgentError, Event, TurnFile, TurnOutcome, TurnRequest, run_turn_with_files,
    };
    struct Capture(Mutex<Option<TurnRequest>>);
    #[async_trait::async_trait]
    impl Agent for Capture {
        fn name(&self) -> &str {
            "fixture"
        }
        async fn run_turn(
            &self,
            request: TurnRequest,
            _: tokio::sync::mpsc::UnboundedSender<Event>,
        ) -> Result<TurnOutcome, AgentError> {
            *self.0.lock().unwrap() = Some(request);
            Ok(TurnOutcome::default())
        }
    }
    pub async fn verify() {
        let store = Store::open_in_memory().unwrap();
        let id = store.create_session(None, "fixture").unwrap();
        let input = NewFile {
            name: "note.txt".into(),
            media_type: "text/plain".into(),
            origin: "attached".into(),
            source_path: None,
            operation: None,
            text: "source".into(),
        };
        let f = store.import_file(id, &input, b"source").unwrap();
        let agent = Capture(Mutex::new(None));
        let file = TurnFile {
            id: f.id,
            revision: 1,
            name: f.name,
            media_type: f.media_type,
            text: "source".into(),
            image_base64: None,
            history_index: None,
        };
        run_turn_with_files(&store, &agent, id, "explain", vec![file], true, &mut |_| {})
            .await
            .unwrap();
        let request = agent.0.lock().unwrap().take().unwrap();
        assert_eq!(request.prompt, "explain");
        assert!(request.input_text().contains("source"));
        let events = store.events(id).unwrap();
        assert_eq!(
            events[0].event,
            Event::UserMessage {
                text: "explain".into()
            }
        );
        assert_eq!(
            store.message_files(id, events[0].seq).unwrap(),
            vec![(f.id, 1)]
        );
    }
}

#[tokio::test]
async fn history_images_are_omitted_with_a_note_when_the_route_has_no_vision() {
    mod omit_test {
        use super::*;
        use std::sync::Mutex;
        use vatra_core::{Agent, AgentError, Event, TurnFile, TurnOutcome, TurnRequest, run_turn_with_files};
        struct Capture(Mutex<Option<TurnRequest>>);
        #[async_trait::async_trait]
        impl Agent for Capture {
            fn name(&self) -> &str { "fixture" }
            async fn run_turn(
                &self,
                request: TurnRequest,
                _: tokio::sync::mpsc::UnboundedSender<Event>,
            ) -> Result<TurnOutcome, AgentError> {
                *self.0.lock().unwrap() = Some(request);
                Ok(TurnOutcome::default())
            }
        }
        pub async fn verify() {
            let store = Store::open_in_memory().unwrap();
            let id = store.create_session(None, "fixture").unwrap();
            let input = NewFile {
                name: "shot.png".into(),
                media_type: "image/png".into(),
                origin: "attached".into(),
                source_path: None,
                operation: None,
                text: String::new(),
            };
            let f = store.import_file(id, &input, b"\x89PNG\r\n\x1a\nimage").unwrap();
            let agent = Capture(Mutex::new(None));
            run_turn_with_files(&store, &agent, id, "first",
                vec![TurnFile{id:f.id,revision:1,name:f.name.clone(),media_type:f.media_type.clone(),text:String::new(),image_base64:Some("aW1hZ2U=".into()),history_index:None}],
                true, &mut |_| {}).await.unwrap();
            run_turn_with_files(&store, &agent, id, "second", vec![], false, &mut |_| {}).await.unwrap();
            let request = agent.0.lock().unwrap().take().unwrap();
            let image = request.files.iter().find(|f| f.media_type.starts_with("image/")).unwrap();
            assert_eq!(image.image_base64, None);
            assert!(request.history_with_files().iter().any(|event| matches!(event,
                Event::UserMessage{text} if text.contains("no confirmed image input"))));
            // The image is still a real catalog entry with its bytes.
            assert_eq!(store.file_bytes(id, f.id, None, 0, 64).unwrap(), b"\x89PNG\r\n\x1a\nimage");
        }
    }
    omit_test::verify().await;
}

#[test]
fn verified_file_observations_capture_before_after_and_deduplicate() {
    let store = Store::open_in_memory().unwrap();
    let session = store.create_session(None, "local").unwrap();
    let file = NewFile {
        name: "result.txt".into(),
        media_type: "text/plain".into(),
        origin: "modified".into(),
        source_path: Some("C:/workspace/result.txt".into()),
        operation: Some("call-1".into()),
        text: "after".into(),
    };
    let captured = store
        .capture_file(session, &file, Some(b"before"), b"after")
        .unwrap();
    assert_eq!(captured.revision, 2);
    assert_eq!(
        store
            .file_bytes(session, captured.id, Some(1), 0, 100)
            .unwrap(),
        b"before"
    );
    let again = store
        .capture_file(session, &file, Some(b"before"), b"after")
        .unwrap();
    assert_eq!(again.id, captured.id);
    assert_eq!(again.revision, 2);
    assert_eq!(store.list_files(session).unwrap().len(), 1);
}

#[test]
fn branching_copies_only_selected_history_and_its_file_versions() {
    let store = Store::open_in_memory().unwrap();
    let session = store.create_session(None, "local").unwrap();
    let input = NewFile {
        name: "a.txt".into(),
        media_type: "text/plain".into(),
        origin: "attached".into(),
        source_path: None,
        operation: None,
        text: "first".into(),
    };
    let file = store.import_file(session, &input, b"first").unwrap();
    let seq = store
        .append_message_with_files(
            session,
            "read",
            &[vatra_core::TurnFile {
                id: file.id,
                revision: 1,
                name: file.name,
                media_type: file.media_type,
                text: "first".into(),
                image_base64: None,
                history_index: None,
            }],
        )
        .unwrap();
    store
        .revise_file(session, file.id, 1, "future", b"future", "edit")
        .unwrap();
    store
        .append(
            session,
            &vatra_core::Event::AssistantText {
                text: "future response".into(),
            },
        )
        .unwrap();
    let branch = store.branch_session(session, seq).unwrap();
    assert_eq!(store.events(branch).unwrap().len(), 1);
    assert!(store.native_session(branch).unwrap().is_none());
    let copied = store.list_files(branch).unwrap();
    assert_eq!(copied.len(), 1);
    assert_eq!(copied[0].revision, 1);
    assert_eq!(
        store.file_text(branch, copied[0].id, None).unwrap(),
        "first"
    );
    store.delete_session(session).unwrap();
    assert_eq!(
        store
            .file_bytes(branch, copied[0].id, None, 0, 100)
            .unwrap(),
        b"first"
    );
}

#[test]
fn attachment_race_rolls_back_the_entire_visible_message() {
    let store = Store::open_in_memory().unwrap();
    let session = store.create_session(None, "local").unwrap();
    let missing = vatra_core::TurnFile {
        id: 999,
        revision: 1,
        name: "gone".into(),
        media_type: "text/plain".into(),
        text: "body".into(),
        image_base64: None,
        history_index: None,
    };
    assert!(
        store
            .append_message_with_files(session, "must not appear", &[missing])
            .is_err()
    );
    assert!(store.events(session).unwrap().is_empty());
}

#[test]
fn migration_of_a_v8_copy_and_backup_preserve_originals_and_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("v8.db");
    let store = Store::open(&original).unwrap();
    let session = store.create_session(None, "local").unwrap();
    drop(store);
    let conn = rusqlite::Connection::open(&original).unwrap();
    conn.execute_batch("DROP TABLE message_files;DROP TABLE file_versions;DROP TABLE files;DROP TABLE file_blobs;DROP TABLE file_upload_chunks;DROP TABLE file_uploads;PRAGMA user_version=8;").unwrap();
    drop(conn);
    let untouched = std::fs::read(&original).unwrap();
    let copy = dir.path().join("migrated.db");
    std::fs::copy(&original, &copy).unwrap();
    let store = Store::open(&copy).unwrap();
    assert_eq!(store.session_project(session).unwrap(), None);
    let input = NewFile {
        name: "proof.txt".into(),
        media_type: "text/plain".into(),
        origin: "attached".into(),
        source_path: None,
        operation: None,
        text: "original".into(),
    };
    let file = store.import_file(session, &input, b"original").unwrap();
    store
        .revise_file(session, file.id, 1, "edited", b"edited", "edit")
        .unwrap();
    let backup = dir.path().join("backup.db");
    store.backup_to(&backup).unwrap();
    let restored = Store::open(&backup).unwrap();
    assert_eq!(
        restored
            .file_bytes(session, file.id, Some(1), 0, 100)
            .unwrap(),
        b"original"
    );
    assert_eq!(
        restored
            .file_bytes(session, file.id, Some(2), 0, 100)
            .unwrap(),
        b"edited"
    );
    assert_eq!(std::fs::read(&original).unwrap(), untouched);
}
