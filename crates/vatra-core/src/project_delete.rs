//! Removing a project: its conversations, documents, notes, tasks and reminders go with it. A folder the project
//! points to on the computer is never touched, and history imported from other tools only loses its project link.

use crate::store::{Store, StoreError};

/// What a deletion took away, so the app can say it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct ProjectRemoval {
    pub conversations: usize,
    pub items: usize,
}

impl Store {
    /// The sessions of a project, for the caller to check that none is running before it deletes.
    pub fn project_sessions(&self, id: i64) -> Result<Vec<i64>, StoreError> {
        let connection = self.connection();
        let mut statement = connection.prepare("SELECT id FROM sessions WHERE project_id = ?1")?;
        Ok(statement.query_map([id], |row| row.get(0))?.collect::<Result<_, _>>()?)
    }

    pub fn rename_project(&self, id: i64, name: &str) -> Result<(), StoreError> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 120 || name.chars().any(char::is_control) {
            return Err(StoreError::Invalid("Choose a project name of 1-120 characters.".into()));
        }
        let connection = self.connection();
        let taken: bool = connection.query_row(
            "SELECT EXISTS (SELECT 1 FROM projects WHERE name = ?1 AND id != ?2)",
            rusqlite::params![name, id],
            |r| r.get(0),
        )?;
        if taken {
            return Err(StoreError::Invalid("Another project already has this name.".into()));
        }
        if connection.execute("UPDATE projects SET name = ?2 WHERE id = ?1", rusqlite::params![id, name])? != 1 {
            return Err(StoreError::Invalid("This project doesn't exist.".into()));
        }
        Ok(())
    }

    /// Deletes the project and everything that belongs to it, in one transaction. False when there is no such project.
    pub fn delete_project(&self, id: i64) -> Result<Option<ProjectRemoval>, StoreError> {
        let mut connection = self.connection();
        let tx = connection.transaction()?;
        let exists: bool = tx.query_row("SELECT EXISTS (SELECT 1 FROM projects WHERE id = ?1)", [id], |r| r.get(0))?;
        if !exists {
            return Ok(None);
        }
        let sessions: Vec<i64> = {
            let mut statement = tx.prepare("SELECT id FROM sessions WHERE project_id = ?1")?;
            statement.query_map([id], |row| row.get(0))?.collect::<Result<_, _>>()?
        };
        for session in &sessions {
            for table in ["events", "workspace_proposals", "workspace_turn_contexts"] {
                tx.execute(&format!("DELETE FROM {table} WHERE session_id = ?1"), [session])?;
            }
            tx.execute("DELETE FROM sessions WHERE id = ?1", [session])?;
        }
        tx.execute("DELETE FROM reminder_inbox WHERE reminder_id IN (SELECT id FROM reminders WHERE project_id = ?1)", [id])?;
        tx.execute("DELETE FROM reminders WHERE project_id = ?1", [id])?;
        tx.execute("DELETE FROM workspace_proposals WHERE project_id = ?1", [id])?;
        tx.execute("DELETE FROM workspace_turn_contexts WHERE project_id = ?1", [id])?;
        tx.execute("DELETE FROM items_fts WHERE item_id IN (SELECT id FROM items WHERE project_id = ?1)", [id])?;
        let items = tx.execute("DELETE FROM items WHERE project_id = ?1", [id])?;
        // Not owned by the project: imported history keeps its text, scheduled tasks keep running without a project.
        tx.execute("UPDATE imported_sessions SET project_id = NULL WHERE project_id = ?1", [id])?;
        tx.execute("UPDATE scheduled_tasks SET project_id = NULL WHERE project_id = ?1", [id])?;
        tx.execute("DELETE FROM projects WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(Some(ProjectRemoval { conversations: sessions.len(), items }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Event, NewItem};

    #[test]
    fn a_project_leaves_with_its_conversations_and_documents_and_nothing_else() {
        let store = Store::open_in_memory().unwrap();
        let keep = store.create_project("Pastrat").unwrap();
        let gone = store.create_project("Sters").unwrap();
        let kept_session = store.create_session(keep, "a").unwrap();
        let gone_session = store.create_session(gone, "a").unwrap();
        store.append(gone_session, &Event::UserMessage { text: "salut".into() }).unwrap();
        store.append(kept_session, &Event::UserMessage { text: "ramane".into() }).unwrap();
        let note = |project, title: &str| NewItem {
            kind: "note".into(),
            title: title.into(),
            body: "text".into(),
            fields: serde_json::json!({"format": "text"}),
            project_id: Some(project),
        };
        store.create_item(&note(gone, "de sters"), 1).unwrap();
        store.create_item(&note(gone, "si asta"), 1).unwrap();
        let survivor = store.create_item(&note(keep, "ramane"), 1).unwrap();

        let removal = store.delete_project(gone).unwrap().unwrap();
        assert_eq!((removal.conversations, removal.items), (1, 2));
        let names: Vec<String> = store.list_projects().unwrap().into_iter().map(|p| p.name).collect();
        assert_eq!(names, vec!["Pastrat"]);
        assert_eq!(store.events(kept_session).unwrap().len(), 1);
        assert!(store.events(gone_session).unwrap().is_empty());
        assert!(store.workspace_item(keep, survivor, false).is_ok());
        assert_eq!(store.delete_project(gone).unwrap(), None, "a second delete finds nothing");
    }

    #[test]
    fn renaming_checks_the_name() {
        let store = Store::open_in_memory().unwrap();
        let a = store.create_project("A").unwrap();
        store.create_project("B").unwrap();
        assert!(store.rename_project(a, "B").is_err());
        assert!(store.rename_project(a, "  ").is_err());
        store.rename_project(a, "Casa").unwrap();
        assert!(store.list_projects().unwrap().iter().any(|p| p.name == "Casa"));
    }
}
