use vatra_core::{Event, HistoryMessage, ImportedSessionInfo, SessionRef, Store};

#[test]
fn new_messages_indexed_and_tools_excluded_and_query_escaped() {
    let store = Store::open_in_memory().unwrap();
    let project = store.create_project("test").unwrap();
    let session = store.create_session(project, "model").unwrap();
    store
        .append(
            session,
            &Event::UserMessage {
                text: "București database".into(),
            },
        )
        .unwrap();
    store
        .append(
            session,
            &Event::AssistantText {
                text: "SQLite works".into(),
            },
        )
        .unwrap();
    assert_eq!(
        store
            .search_history("Bucuresti", Some(project), 20)
            .unwrap()[0]
            .session,
        SessionRef::Vatra(session)
    );
    assert_eq!(store.search_history("SQLite", None, 20).unwrap().len(), 1);
    assert!(
        store
            .search_history("database", Some(project + 1), 20)
            .unwrap()
            .is_empty()
    );
    assert!(store.search_history("\" : () OR *", None, 20).is_ok());
}

#[test]
fn imported_messages_searchable_idempotent_and_readable() {
    let store = Store::open_in_memory().unwrap();
    let meta = ImportedSessionInfo {
        id: 0,
        origin: "claude".into(),
        native_id: "abc".into(),
        project_id: None,
        cwd: Some("E:/x".into()),
        title: "Old conversation".into(),
        started_at: "2026-11-01".into(),
    };
    let messages = vec![HistoryMessage {
        role: "user".into(),
        text: "Remember diacritics: școală".into(),
        at: "today".into(),
    }];
    let id = store
        .import_history(&meta, messages.clone().into_iter().map(Ok))
        .unwrap();
    assert_eq!(
        store
            .import_history(&meta, messages.into_iter().map(Ok))
            .unwrap(),
        id
    );
    let hits = store.search_history("scoala", None, 20).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].session, SessionRef::Imported(id));
    assert_eq!(store.imported_session_messages(id).unwrap().len(), 1);
    assert_eq!(
        store.open_imported(id).unwrap().unwrap().info.title,
        "Old conversation"
    );
}

#[test]
fn cancelled_reimport_preserves_previous_messages() {
    let store = Store::open_in_memory().unwrap();
    let meta = ImportedSessionInfo {
        id: 0,
        origin: "codex".into(),
        native_id: "abc".into(),
        project_id: None,
        cwd: None,
        title: "Old".into(),
        started_at: String::new(),
    };
    let msg = HistoryMessage {
        role: "assistant".into(),
        text: "Original answer".into(),
        at: String::new(),
    };
    let id = store
        .import_history(&meta, vec![Ok(msg)].into_iter())
        .unwrap();
    assert!(
        store
            .import_history(&meta, vec![Err("cancelled".to_string())].into_iter())
            .is_err()
    );
    assert_eq!(store.imported_session_messages(id).unwrap().len(), 1);
}

#[test]
fn schema_two_to_three_preserves_events_and_backfills_only_conversation_text_once() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("previous.db");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("PRAGMA user_version=2;
        CREATE TABLE projects(id INTEGER PRIMARY KEY AUTOINCREMENT,name TEXT NOT NULL UNIQUE,path TEXT);
        CREATE TABLE sessions(id INTEGER PRIMARY KEY AUTOINCREMENT,project_id INTEGER NOT NULL REFERENCES projects(id),agent TEXT NOT NULL,native_session TEXT,title TEXT,last_turn TEXT NOT NULL DEFAULT 'none');
        CREATE TABLE events(seq INTEGER PRIMARY KEY AUTOINCREMENT,session_id INTEGER NOT NULL REFERENCES sessions(id),payload TEXT NOT NULL);
        INSERT INTO projects(id,name,path) VALUES(7,'Existing project','C:/existing');
        INSERT INTO sessions(id,project_id,agent,native_session,title,last_turn) VALUES(12,7,'local','native-12','Existing title','completed');").unwrap();
    let events = [
        Event::UserMessage {
            text: "migrationuserneedle București".into(),
        },
        Event::ToolCall {
            id: "call".into(),
            name: "Bash".into(),
            arguments: serde_json::json!({"command":"toolargumentneedle"}),
        },
        Event::ToolResult {
            id: "call".into(),
            output: "tooloutputneedle".into(),
            is_error: false,
        },
        Event::AssistantText {
            text: "migrationanswerneedle".into(),
        },
    ];
    for event in &events {
        connection
            .execute(
                "INSERT INTO events(session_id,payload) VALUES(12,?1)",
                [serde_json::to_string(event).unwrap()],
            )
            .unwrap();
    }
    drop(connection);
    let store = Store::open(&path).unwrap();
    let projects = store.list_projects().unwrap();
    assert_eq!(projects[0].path.as_deref(), Some("C:/existing"));
    let sessions = store.list_sessions().unwrap();
    assert_eq!(sessions[0].id, 12);
    assert_eq!(sessions[0].title, "Existing title");
    assert_eq!(sessions[0].last_turn, vatra_core::LastTurn::Completed);
    assert_eq!(
        store
            .events(12)
            .unwrap()
            .into_iter()
            .map(|stored| stored.event)
            .collect::<Vec<_>>(),
        events
    );
    for query in ["migrationuserneedle", "migrationanswerneedle", "Bucuresti"] {
        let hits = store.search_history(query, Some(7), 20).unwrap();
        assert_eq!(hits.len(), 1, "backfilled {query}");
        assert_eq!(hits[0].session, SessionRef::Vatra(12));
    }
    for query in ["toolargumentneedle", "tooloutputneedle"] {
        assert!(store.search_history(query, None, 20).unwrap().is_empty());
    }
    drop(store);
    let check = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        check
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        15
    );
    assert_eq!(
        check
            .query_row(
                "SELECT native_session FROM sessions WHERE id=12",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "native-12"
    );
    drop(check);
    let reopened = Store::open(&path).unwrap();
    assert_eq!(
        reopened
            .search_history("migrationuserneedle", None, 20)
            .unwrap()
            .len(),
        1,
        "reopening does not reindex old events"
    );
}

#[test]
fn live_tool_arguments_results_and_imported_tool_roles_are_not_searchable() {
    let store = Store::open_in_memory().unwrap();
    let project = store.create_project("tools").unwrap();
    let session = store.create_session(project, "local").unwrap();
    store
        .append(
            session,
            &Event::ToolCall {
                id: "tool".into(),
                name: "Bash".into(),
                arguments: serde_json::json!({"command":"liveargumentneedle"}),
            },
        )
        .unwrap();
    store
        .append(
            session,
            &Event::ToolResult {
                id: "tool".into(),
                output: "liveresultneedle".into(),
                is_error: false,
            },
        )
        .unwrap();
    store
        .append(
            session,
            &Event::AssistantText {
                text: "visibleanswerneedle".into(),
            },
        )
        .unwrap();
    let metadata = ImportedSessionInfo {
        id: 0,
        origin: "codex".into(),
        native_id: "tools-only".into(),
        project_id: Some(project),
        cwd: None,
        title: "Imported tools".into(),
        started_at: String::new(),
    };
    let messages = [
        HistoryMessage {
            role: "tool".into(),
            text: "importedtoolneedle".into(),
            at: String::new(),
        },
        HistoryMessage {
            role: "assistant".into(),
            text: "importedanswerneedle".into(),
            at: String::new(),
        },
    ];
    let imported = store
        .import_history(&metadata, messages.into_iter().map(Ok))
        .unwrap();
    for query in [
        "liveargumentneedle",
        "liveresultneedle",
        "importedtoolneedle",
    ] {
        assert!(
            store.search_history(query, None, 20).unwrap().is_empty(),
            "excluded {query}"
        );
    }
    assert_eq!(
        store
            .search_history("visibleanswerneedle", None, 20)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(store.imported_session_messages(imported).unwrap().len(), 1);
    assert_eq!(
        store
            .search_history("importedanswerneedle", None, 20)
            .unwrap()[0]
            .session,
        SessionRef::Imported(imported)
    );
}
