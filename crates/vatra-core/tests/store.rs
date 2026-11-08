use vatra_core::{Event, LastTurn, Store};

#[test]
fn a_conversation_can_be_renamed_without_changing_its_history(){
    let store=Store::open_in_memory().unwrap();let id=store.create_session(None,"local").unwrap();
    store.append(id,&Event::UserMessage{text:"original prompt".into()}).unwrap();
    store.rename_session(id,"Research notes").unwrap();
    assert_eq!(store.list_sessions().unwrap()[0].title,"Research notes");
    assert_eq!(store.events(id).unwrap().len(),1);
    assert!(store.rename_session(id,"  ").is_err());
}

#[test]
fn appended_events_come_back_in_order_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cale cu spații.db");
    let store = Store::open(&path).unwrap();
    let project = store.create_project("ț").unwrap();
    let session = store.create_session(project, "local").unwrap();
    store.append(session, &Event::UserMessage { text: "bună ziua, ășțîâ".into() }).unwrap();
    store.append(session, &Event::AssistantText { text: "salut".into() }).unwrap();
    drop(store);
    let events = Store::open(&path).unwrap().events(session).unwrap();
    assert_eq!(events.len(), 2);
    assert!(events[0].seq < events[1].seq);
    assert_eq!(events[0].event, Event::UserMessage { text: "bună ziua, ășțîâ".into() });
}

#[test]
fn create_project_is_idempotent_by_name() {
    let store = Store::open_in_memory().unwrap();
    assert_eq!(store.create_project("a").unwrap(), store.create_project("a").unwrap());
}

#[test]
fn append_to_missing_session_is_an_error() {
    let store = Store::open_in_memory().unwrap();
    assert!(store.append(999, &Event::AssistantText { text: "x".into() }).is_err());
}

#[test]
fn native_session_round_trips() {
    let store = Store::open_in_memory().unwrap();
    let project = store.create_project("a").unwrap();
    let session = store.create_session(project, "claude").unwrap();
    assert_eq!(store.native_session(session).unwrap(), None);
    store.set_native_session(session, "abc").unwrap();
    assert_eq!(store.native_session(session).unwrap().as_deref(), Some("abc"));
}

fn fixture() -> (Store, i64) {
    let store = Store::open_in_memory().unwrap();
    let project = store.create_project("p").unwrap();
    let session = store.create_session(project, "local").unwrap();
    (store, session)
}

fn create_stage1_database(path: &std::path::Path) {
    let connection = rusqlite::Connection::open(path).unwrap();
    connection.execute_batch(
        "CREATE TABLE projects (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL UNIQUE);
         CREATE TABLE sessions (id INTEGER PRIMARY KEY AUTOINCREMENT, project_id INTEGER NOT NULL REFERENCES projects(id), agent TEXT NOT NULL, native_session TEXT);
         CREATE TABLE events (seq INTEGER PRIMARY KEY AUTOINCREMENT, session_id INTEGER NOT NULL REFERENCES sessions(id), payload TEXT NOT NULL);
         INSERT INTO projects (name) VALUES ('vechi');
         INSERT INTO sessions (project_id, agent) VALUES (1, 'local');
         INSERT INTO events (session_id, payload) VALUES (1, '{\"type\":\"user_message\",\"text\":\"x\"}');",
    ).unwrap();
}

#[test]
fn database_from_stage_1_is_migrated_without_losing_events() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vechi.db");
    create_stage1_database(&path);
    let store = Store::open(&path).unwrap();
    assert_eq!(store.list_projects().unwrap()[0].path, None);
    assert_eq!(store.events(1).unwrap().len(), 1);
    drop(store);
    assert_eq!(Store::open(&path).unwrap().events(1).unwrap().len(), 1);
}

#[test]
fn database_from_a_newer_version_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nou.db");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.pragma_update(None, "user_version", 999).unwrap();
    drop(connection);
    let error = Store::open(&path).err().unwrap();
    assert!(error.to_string().contains("newer"));
}

#[test]
fn project_path_round_trips() {
    let (store, _) = fixture();
    store.set_project_path(1, Some("E:/cale cu spații")).unwrap();
    assert_eq!(store.list_projects().unwrap()[0].path.as_deref(), Some("E:/cale cu spații"));
    store.set_project_path(1, None).unwrap();
    assert_eq!(store.list_projects().unwrap()[0].path, None);
}

#[test]
fn session_title_is_the_first_prompt_shortened_on_a_char_boundary() {
    let (store, session) = fixture();
    store.append(session, &Event::UserMessage { text: "ă".repeat(200) }).unwrap();
    let title = store.list_sessions().unwrap()[0].title.clone();
    assert!(title.chars().count() <= 60);
    assert!(title.starts_with('ă'));
}

#[test]
fn sessions_are_sorted_by_last_activity_and_report_their_last_turn() {
    let (store, first) = fixture();
    let second = store.create_session(1, "local").unwrap();
    store.append(second, &Event::UserMessage { text: "b".into() }).unwrap();
    store.append(second, &Event::TurnCompleted { agent: "local".into() }).unwrap();
    store.append(first, &Event::UserMessage { text: "a".into() }).unwrap();
    let sessions = store.list_sessions().unwrap();
    assert_eq!(sessions[0].id, first);
    assert_eq!(sessions[0].last_turn, LastTurn::Unfinished);
    assert_eq!(sessions[1].last_turn, LastTurn::Completed);
}

#[test]
fn session_without_events_has_a_placeholder_title_and_no_turn() {
    let (store, _) = fixture();
    let session = &store.list_sessions().unwrap()[0];
    assert_eq!(session.title, "New session");
    assert_eq!(session.last_turn, LastTurn::None);
}

#[test]
fn events_after_returns_only_newer_events() {
    let (store, session) = fixture();
    let first = store.append(session, &Event::AssistantText { text: "a".into() }).unwrap();
    store.append(session, &Event::AssistantText { text: "b".into() }).unwrap();
    assert_eq!(store.events_after(session, first).unwrap().len(), 1);
}

#[test]
fn unfinished_sessions_are_those_whose_last_turn_never_ended() {
    let (store, session) = fixture();
    store.append(session, &Event::UserMessage { text: "a".into() }).unwrap();
    assert_eq!(store.unfinished_sessions().unwrap(), vec![session]);
    store.append(session, &Event::TurnFailed { agent: "local".into(), message: "x".into() }).unwrap();
    assert!(store.unfinished_sessions().unwrap().is_empty());
}

#[test]
fn listing_sessions_does_not_read_event_payloads() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.db");
    let store = Store::open(&path).unwrap();
    let project = store.create_project("p").unwrap();
    let session = store.create_session(project, "local").unwrap();
    store.append(session, &Event::UserMessage { text: "titlu".into() }).unwrap();
    drop(store);
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute("INSERT INTO events (session_id, payload) VALUES (1, 'nu e json')", []).unwrap();
    drop(connection);
    let sessions = Store::open(&path).unwrap().list_sessions().unwrap();
    assert_eq!(sessions[0].title, "titlu");
}

#[test]
fn a_stage_1_database_gets_titles_and_turn_state_on_migration() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vechi.db");
    create_stage1_database(&path);
    let session = &Store::open(&path).unwrap().list_sessions().unwrap()[0];
    assert_eq!(session.title, "x");
    assert_eq!(session.last_turn, LastTurn::Unfinished);
}

#[test]
fn a_user_message_after_a_finished_turn_is_unfinished_again_and_keeps_the_first_title() {
    let (store, session) = fixture();
    store.append(session, &Event::UserMessage { text: "primul".into() }).unwrap();
    store.append(session, &Event::TurnCompleted { agent: "local".into() }).unwrap();
    store.append(session, &Event::UserMessage { text: "al doilea".into() }).unwrap();
    let listed = &store.list_sessions().unwrap()[0];
    assert_eq!(listed.title, "primul");
    assert_eq!(listed.last_turn, LastTurn::Unfinished);
}

#[test]
fn migrates_3_to_4_keeping_events() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vatra.db");
    {
        let store = Store::open(&path).unwrap();
        let project = store.create_project("p").unwrap();
        let session = store.create_session(project, "local").unwrap();
        store.append(session, &vatra_core::Event::UserMessage { text: "hi".into() }).unwrap();
    }
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch("DROP TABLE reminder_inbox; DROP TABLE reminders; DROP TABLE workspace_proposals; DROP TABLE workspace_turn_contexts; DROP TABLE item_links; DROP TABLE items_fts; DROP TABLE items; DROP TABLE local_models; DROP TABLE model_checks; PRAGMA user_version = 3;")
            .unwrap();
    }
    let store = Store::open(&path).unwrap();
    assert_eq!(store.events(1).unwrap().len(), 1);
    let row = vatra_core::LocalModelRow {
        slug: "qwen3-4b-q4_k_m".into(),
        repo: Some("unsloth/Qwen3-4B-Instruct-2507-GGUF".into()),
        variant: Some("Q4_K_M".into()),
        files_json: "[]".into(),
        sha256_json: "[]".into(),
        path: r"C:\Users\Răzvan Test\modele\x.gguf".into(),
        size_bytes: 2_500_000_000,
        metadata_json: None,
        added_by: "download".into(),
        settings_json: "{}".into(),
        created_at: 1,
    };
    store.insert_local_model(&row).unwrap();
    assert!(store.insert_local_model(&row).is_err(), "slugs are unique");
    assert_eq!(store.local_models().unwrap(), vec![row.clone()]);
    let check = vatra_core::ModelCheckRow {
        agent: "local:qwen3-4b-q4_k_m".into(),
        started_at: 1,
        finished_at: 2,
        llama_build: Some(12019),
        settings_json: "{}".into(),
        results_json: "{}".into(),
        label: "complete".into(),
        consented_cost: false,
    };
    store.insert_model_check(&check).unwrap();
    assert_eq!(store.latest_model_check(&check.agent).unwrap(), Some(check.clone()));
    assert!(store.remove_local_model(&row.slug, &check.agent).unwrap());
    assert!(store.local_models().unwrap().is_empty());
    assert_eq!(store.latest_model_check(&check.agent).unwrap(), None);
}

#[test]
fn app_tables_keep_agents_settings_and_parity_runs() {
    let store = Store::open_in_memory().unwrap();
    let row = vatra_core::ConfiguredAgentRow {
        name: "openrouter".into(),
        kind: "model".into(),
        settings_json: r#"{"kind":"model","model":"m"}"#.into(),
        created_at: 1,
    };
    store.save_configured_agent(&row).unwrap();
    store.save_configured_agent(&vatra_core::ConfiguredAgentRow { settings_json: "{}".into(), ..row.clone() }).unwrap();
    assert_eq!(store.configured_agents().unwrap().len(), 1, "saving the same name replaces it");
    assert!(store.remove_configured_agent("openrouter").unwrap());
    store.set_app_setting("summarizer", Some("local:x")).unwrap();
    assert_eq!(store.app_setting("summarizer").unwrap().as_deref(), Some("local:x"));
    store.set_app_setting("summarizer", None).unwrap();
    assert_eq!(store.app_setting("summarizer").unwrap(), None);
    let run = vatra_core::ParityRunRow {
        id: 0,
        agent: "local:x".into(),
        native: None,
        model: Some("m".into()),
        build: None,
        started_at: 1,
        finished_at: 2,
        results_json: "[]".into(),
        consented_cost: false,
    };
    let id = store.insert_parity_run(&run).unwrap();
    assert_eq!(store.parity_runs(5).unwrap(), vec![vatra_core::ParityRunRow { id, ..run }]);
}

#[test]
fn session_without_project_round_trips() {
    let store = Store::open_in_memory().unwrap();
    let session = store.create_session(None, "echo").unwrap();
    assert_eq!(store.session_project(session).unwrap(), None);
    let listed = store.list_sessions().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, session);
    assert_eq!(listed[0].project_id, None);
}

#[test]
fn migrates_7_to_8_allowing_null_project_id() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v7.db");
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE projects (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL UNIQUE, path TEXT);
                 CREATE TABLE sessions (
                     id INTEGER PRIMARY KEY AUTOINCREMENT,
                     project_id INTEGER NOT NULL REFERENCES projects(id),
                     agent TEXT NOT NULL,
                     native_session TEXT,
                     title TEXT,
                     last_turn TEXT NOT NULL DEFAULT 'none'
                 );
                 CREATE TABLE events (
                     seq INTEGER PRIMARY KEY AUTOINCREMENT,
                     session_id INTEGER NOT NULL REFERENCES sessions(id),
                     payload TEXT NOT NULL
                 );
                 INSERT INTO projects (id, name) VALUES (1, 'Proiect existent');
                 INSERT INTO sessions (id, project_id, agent, title, last_turn)
                     VALUES (10, 1, 'agent', 'Titlu vechi', 'completed');
                 PRAGMA user_version = 7;",
            )
            .unwrap();
    }
    let store = Store::open(&path).unwrap();
    let sessions = store.list_sessions().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, 10);
    assert_eq!(sessions[0].project_id, Some(1));
    assert_eq!(sessions[0].title, "Titlu vechi");

    // Creating a session without project now succeeds
    let standalone = store.create_session(None, "echo").unwrap();
    assert_eq!(store.session_project(standalone).unwrap(), None);
    let all = store.list_sessions().unwrap();
    assert_eq!(all.len(), 2);
}

