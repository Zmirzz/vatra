//! Research runs (a question, the sources read, the report) and blind comparisons of models. A run is saved at every
//! step, so what was read is never lost; a comparison keeps which model wrote which answer until the person has voted.

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::store::{Store, StoreError};

pub(crate) const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS research_runs (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        question TEXT NOT NULL,
        depth TEXT NOT NULL,
        state TEXT NOT NULL,
        step TEXT NOT NULL DEFAULT '',
        plan_json TEXT NOT NULL DEFAULT '[]',
        sources_json TEXT NOT NULL DEFAULT '[]',
        report TEXT,
        followups_json TEXT NOT NULL DEFAULT '[]',
        error TEXT,
        created_at INTEGER NOT NULL,
        finished_at INTEGER
    );
    CREATE TABLE IF NOT EXISTS compare_runs (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        prompt TEXT NOT NULL,
        entries_json TEXT NOT NULL,
        vote TEXT,
        created_at INTEGER NOT NULL
    );
";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResearchRun {
    pub id: i64,
    pub question: String,
    pub depth: String,
    /// `planning`, `searching`, `reading`, `writing`, `done`, `failed` or `cancelled`.
    pub state: String,
    pub step: String,
    pub plan: Vec<String>,
    pub sources: Value,
    pub report: Option<String>,
    pub followups: Value,
    pub error: Option<String>,
    pub created_at: i64,
    pub finished_at: Option<i64>,
    /// How the research went: rounds, searches, sources by kind and provider, whether the report is partial.
    pub stats: Value,
    /// What the person chose: light mode, answers to clarifying questions.
    pub options: Value,
}

/// Columns added in schema 13, as (table, column, definition).
pub(crate) const COLUMNS_V13: [(&str, &str, &str); 2] = [
    ("research_runs", "stats_json", "TEXT NOT NULL DEFAULT '{}'"),
    ("research_runs", "options_json", "TEXT NOT NULL DEFAULT '{}'"),
];

const RUN_COLUMNS: &str = "id,question,depth,state,step,plan_json,sources_json,report,followups_json,error,created_at,finished_at,stats_json,options_json";

fn run_row(row: &rusqlite::Row) -> rusqlite::Result<ResearchRun> {
    let plan: String = row.get(5)?;
    let sources: String = row.get(6)?;
    let followups: String = row.get(8)?;
    Ok(ResearchRun {
        id: row.get(0)?,
        question: row.get(1)?,
        depth: row.get(2)?,
        state: row.get(3)?,
        step: row.get(4)?,
        plan: serde_json::from_str(&plan).unwrap_or_default(),
        sources: serde_json::from_str(&sources).unwrap_or(Value::Array(vec![])),
        report: row.get(7)?,
        followups: serde_json::from_str(&followups).unwrap_or(Value::Array(vec![])),
        error: row.get(9)?,
        created_at: row.get(10)?,
        finished_at: row.get(11)?,
        stats: serde_json::from_str(&row.get::<_, String>(12)?).unwrap_or(Value::Null),
        options: serde_json::from_str(&row.get::<_, String>(13)?).unwrap_or(Value::Null),
    })
}

/// One answer of a comparison. `agent` stays on the server until the vote.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompareEntry {
    pub label: String,
    pub agent: String,
    pub answer: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompareRun {
    pub id: i64,
    pub prompt: String,
    pub entries: Vec<CompareEntry>,
    pub vote: Option<String>,
    pub created_at: i64,
}

impl Store {
    pub fn create_research_run(&self, question: &str, depth: &str, now: i64) -> Result<i64, StoreError> {
        self.create_research_run_with(question, depth, &serde_json::json!({}), now)
    }

    pub fn create_research_run_with(&self, question: &str, depth: &str, options: &Value, now: i64) -> Result<i64, StoreError> {
        if question.trim().is_empty() || question.chars().count() > 2000 {
            return Err(StoreError::Invalid("Ask a question of up to 2000 characters.".into()));
        }
        if !["quick", "standard", "deep"].contains(&depth) {
            return Err(StoreError::Invalid("Choose quick, standard or deep research.".into()));
        }
        let connection = self.connection();
        connection.execute(
            "INSERT INTO research_runs (question,depth,state,step,created_at,options_json) VALUES (?1,?2,'planning','',?3,?4)",
            params![question.trim(), depth, now, options.to_string()],
        )?;
        Ok(connection.last_insert_rowid())
    }

    pub fn research_run(&self, id: i64) -> Result<Option<ResearchRun>, StoreError> {
        Ok(self
            .connection()
            .query_row(&format!("SELECT {RUN_COLUMNS} FROM research_runs WHERE id=?1"), [id], run_row)
            .optional()?)
    }

    pub fn research_runs(&self, limit: usize) -> Result<Vec<ResearchRun>, StoreError> {
        let connection = self.connection();
        let mut statement =
            connection.prepare(&format!("SELECT {RUN_COLUMNS} FROM research_runs ORDER BY id DESC LIMIT ?1"))?;
        let rows = statement.query_map([limit.min(200) as i64], run_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn set_research_progress(&self, id: i64, state: &str, step: &str) -> Result<(), StoreError> {
        self.connection().execute(
            "UPDATE research_runs SET state=?2,step=?3 WHERE id=?1 AND state NOT IN ('done','failed','cancelled')",
            params![id, state, step],
        )?;
        Ok(())
    }

    pub fn set_research_plan(&self, id: i64, plan: &[String]) -> Result<(), StoreError> {
        self.connection().execute("UPDATE research_runs SET plan_json=?2 WHERE id=?1", params![id, serde_json::to_string(plan)?])?;
        Ok(())
    }

    pub fn set_research_sources(&self, id: i64, sources: &Value) -> Result<(), StoreError> {
        self.connection().execute("UPDATE research_runs SET sources_json=?2 WHERE id=?1", params![id, sources.to_string()])?;
        Ok(())
    }

    pub fn set_research_stats(&self, id: i64, stats: &Value) -> Result<(), StoreError> {
        self.connection().execute("UPDATE research_runs SET stats_json=?2 WHERE id=?1", params![id, stats.to_string()])?;
        Ok(())
    }

    pub fn finish_research(&self, id: i64, state: &str, report: Option<&str>, error: Option<&str>, now: i64) -> Result<(), StoreError> {
        self.connection().execute(
            "UPDATE research_runs SET state=?2,report=COALESCE(?3,report),error=?4,finished_at=?5 WHERE id=?1 AND state NOT IN ('done','failed','cancelled')",
            params![id, state, report, error, now],
        )?;
        Ok(())
    }

    pub fn add_research_followup(&self, id: i64, question: &str, answer: &str) -> Result<(), StoreError> {
        let connection = self.connection();
        let current: String = connection.query_row("SELECT followups_json FROM research_runs WHERE id=?1", [id], |r| r.get(0))?;
        let mut list: Vec<Value> = serde_json::from_str(&current).unwrap_or_default();
        list.push(serde_json::json!({"question": question, "answer": answer}));
        if list.len() > 50 {
            list.remove(0);
        }
        connection.execute("UPDATE research_runs SET followups_json=?2 WHERE id=?1", params![id, serde_json::to_string(&list)?])?;
        Ok(())
    }

    pub fn delete_research_run(&self, id: i64) -> Result<bool, StoreError> {
        Ok(self.connection().execute("DELETE FROM research_runs WHERE id=?1", [id])? > 0)
    }

    /// At start-up a run still searching or reading was cut short with the service; what it read is kept.
    pub fn recover_research(&self, now: i64) -> Result<usize, StoreError> {
        Ok(self.connection().execute(
            "UPDATE research_runs SET state='failed',error='The service stopped during this research.',finished_at=?1 WHERE state IN ('planning','searching','reading','writing')",
            [now],
        )?)
    }

    pub fn create_compare_run(&self, prompt: &str, entries: &[CompareEntry], now: i64) -> Result<i64, StoreError> {
        let connection = self.connection();
        connection.execute(
            "INSERT INTO compare_runs (prompt,entries_json,created_at) VALUES (?1,?2,?3)",
            params![prompt, serde_json::to_string(entries)?, now],
        )?;
        Ok(connection.last_insert_rowid())
    }

    pub fn compare_run(&self, id: i64) -> Result<Option<CompareRun>, StoreError> {
        let found = self
            .connection()
            .query_row("SELECT id,prompt,entries_json,vote,created_at FROM compare_runs WHERE id=?1", [id], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, i64>(4)?))
            })
            .optional()?;
        Ok(found.map(|(id, prompt, entries, vote, created_at)| CompareRun {
            id,
            prompt,
            entries: serde_json::from_str(&entries).unwrap_or_default(),
            vote,
            created_at,
        }))
    }

    pub fn compare_runs(&self, limit: usize) -> Result<Vec<CompareRun>, StoreError> {
        let ids: Vec<i64> = {
            let connection = self.connection();
            let mut statement = connection.prepare("SELECT id FROM compare_runs ORDER BY id DESC LIMIT ?1")?;
            statement.query_map([limit.min(200) as i64], |r| r.get(0))?.collect::<Result<_, _>>()?
        };
        let mut out = Vec::new();
        for id in ids {
            if let Some(run) = self.compare_run(id)? {
                out.push(run);
            }
        }
        Ok(out)
    }

    /// Records the vote once. A second vote on the same comparison is refused.
    pub fn vote_compare(&self, id: i64, vote: &str) -> Result<bool, StoreError> {
        Ok(self.connection().execute("UPDATE compare_runs SET vote=?2 WHERE id=?1 AND vote IS NULL", params![id, vote])? > 0)
    }

    pub fn delete_compare_run(&self, id: i64) -> Result<bool, StoreError> {
        Ok(self.connection().execute("DELETE FROM compare_runs WHERE id=?1", [id])? > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_keeps_its_progress_and_cannot_be_reopened_once_finished() {
        let store = Store::open_in_memory().unwrap();
        let id = store.create_research_run("Cat costa X?", "standard", 10).unwrap();
        store.set_research_progress(id, "searching", "Looking for sources").unwrap();
        store.set_research_plan(id, &["a".into(), "b".into()]).unwrap();
        store.set_research_sources(id, &serde_json::json!([{"n": 1, "url": "https://x.example"}])).unwrap();
        store.finish_research(id, "done", Some("Raport [1]"), None, 20).unwrap();
        store.set_research_progress(id, "searching", "again").unwrap();
        store.finish_research(id, "failed", None, Some("late"), 30).unwrap();
        let run = store.research_run(id).unwrap().unwrap();
        assert_eq!((run.state.as_str(), run.report.as_deref(), run.plan.len()), ("done", Some("Raport [1]"), 2));
        assert_eq!(run.finished_at, Some(20));
        assert!(store.create_research_run("", "quick", 1).is_err());
        assert!(store.create_research_run("q", "huge", 1).is_err());
    }

    #[test]
    fn a_stopped_service_marks_unfinished_runs_failed_and_keeps_the_sources() {
        let store = Store::open_in_memory().unwrap();
        let id = store.create_research_run("q", "quick", 1).unwrap();
        store.set_research_sources(id, &serde_json::json!([{"n": 1}])).unwrap();
        store.set_research_progress(id, "reading", "x").unwrap();
        assert_eq!(store.recover_research(5).unwrap(), 1);
        let run = store.research_run(id).unwrap().unwrap();
        assert_eq!(run.state, "failed");
        assert_eq!(run.sources.as_array().unwrap().len(), 1);
    }

    #[test]
    fn a_comparison_is_voted_once() {
        let store = Store::open_in_memory().unwrap();
        let entries = vec![
            CompareEntry { label: "A".into(), agent: "x".into(), answer: Some("1".into()), error: None },
            CompareEntry { label: "B".into(), agent: "y".into(), answer: None, error: Some("no".into()) },
        ];
        let id = store.create_compare_run("p", &entries, 1).unwrap();
        assert!(store.vote_compare(id, "A").unwrap());
        assert!(!store.vote_compare(id, "B").unwrap());
        assert_eq!(store.compare_run(id).unwrap().unwrap().vote.as_deref(), Some("A"));
    }

    #[test]
    fn follow_ups_are_appended() {
        let store = Store::open_in_memory().unwrap();
        let id = store.create_research_run("q", "quick", 1).unwrap();
        store.add_research_followup(id, "si apoi?", "asa").unwrap();
        assert_eq!(store.research_run(id).unwrap().unwrap().followups[0]["answer"], "asa");
    }
}
