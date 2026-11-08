//! Scheduled agent tasks and the calendar. Times are seconds since the Unix epoch, in UTC; a schedule that means a
//! local time of day carries the person's offset from UTC in minutes. A task survives a restart: the next run is saved,
//! and a run that was cut short is marked interrupted instead of being run again.

use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

use crate::store::{Store, StoreError};

pub(crate) const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS scheduled_tasks (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        name TEXT NOT NULL,
        prompt TEXT NOT NULL,
        agent TEXT NOT NULL,
        project_id INTEGER,
        spec_json TEXT NOT NULL,
        enabled INTEGER NOT NULL DEFAULT 1,
        next_run_at INTEGER,
        last_run_at INTEGER,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS scheduled_runs (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        task_id INTEGER NOT NULL,
        scheduled_for INTEGER NOT NULL,
        started_at INTEGER NOT NULL,
        finished_at INTEGER,
        state TEXT NOT NULL,
        session_id INTEGER,
        summary TEXT
    );
    CREATE INDEX IF NOT EXISTS scheduled_runs_by_task ON scheduled_runs(task_id, id);
    CREATE TABLE IF NOT EXISTS calendar_events (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        calendar TEXT NOT NULL DEFAULT 'local',
        uid TEXT NOT NULL,
        title TEXT NOT NULL,
        notes TEXT NOT NULL DEFAULT '',
        location TEXT NOT NULL DEFAULT '',
        starts_at INTEGER NOT NULL,
        ends_at INTEGER NOT NULL,
        all_day INTEGER NOT NULL DEFAULT 0,
        rrule TEXT,
        source TEXT NOT NULL DEFAULT 'local',
        href TEXT,
        etag TEXT,
        updated_at INTEGER NOT NULL,
        UNIQUE(calendar, uid)
    );
    CREATE INDEX IF NOT EXISTS calendar_events_by_start ON calendar_events(starts_at);
";

/// When a task runs. `tz_offset_min` is minutes east of UTC (Romania in summer: 180).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Schedule {
    Once { at: i64 },
    Interval { every_secs: i64 },
    Daily { hour: u32, minute: u32, tz_offset_min: i32 },
    /// `days` are 0 (Sunday) to 6 (Saturday).
    Weekly { days: Vec<u32>, hour: u32, minute: u32, tz_offset_min: i32 },
}

impl Schedule {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Schedule::Once { at } if *at <= 0 => Err("the time of the run is missing".into()),
            Schedule::Interval { every_secs } if !(60..=366 * 86_400).contains(every_secs) => {
                Err("a repeating task runs at most once a minute and at least once a year".into())
            }
            Schedule::Daily { hour, minute, .. } | Schedule::Weekly { hour, minute, .. }
                if *hour > 23 || *minute > 59 =>
            {
                Err("the time of day isn't valid".into())
            }
            Schedule::Weekly { days, .. } if days.is_empty() || days.iter().any(|d| *d > 6) => {
                Err("pick at least one day of the week".into())
            }
            Schedule::Daily { tz_offset_min, .. } | Schedule::Weekly { tz_offset_min, .. }
                if tz_offset_min.abs() > 14 * 60 =>
            {
                Err("the time zone isn't valid".into())
            }
            _ => Ok(()),
        }
    }

    /// The first run strictly after `after`, or None when the schedule is spent.
    pub fn next_after(&self, after: i64) -> Option<i64> {
        match self {
            Schedule::Once { at } => (*at > after).then_some(*at),
            Schedule::Interval { every_secs } => Some(after + every_secs.max(&60)),
            Schedule::Daily { hour, minute, tz_offset_min } => {
                next_local(after, *hour, *minute, *tz_offset_min, |_| true)
            }
            Schedule::Weekly { days, hour, minute, tz_offset_min } => {
                next_local(after, *hour, *minute, *tz_offset_min, |weekday| days.contains(&weekday))
            }
        }
    }
}

fn next_local(after: i64, hour: u32, minute: u32, offset_min: i32, day_ok: impl Fn(u32) -> bool) -> Option<i64> {
    let offset = i64::from(offset_min) * 60;
    let local_day = (after + offset).div_euclid(86_400);
    for step in 0..9 {
        let day = local_day + step;
        // 1970-01-01 was a Thursday.
        let weekday = (day + 4).rem_euclid(7) as u32;
        if !day_ok(weekday) {
            continue;
        }
        let at = day * 86_400 + i64::from(hour) * 3600 + i64::from(minute) * 60 - offset;
        if at > after {
            return Some(at);
        }
    }
    None
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTask {
    pub id: i64,
    pub name: String,
    pub prompt: String,
    pub agent: String,
    pub project_id: Option<i64>,
    pub schedule: Schedule,
    pub enabled: bool,
    pub next_run_at: Option<i64>,
    pub last_run_at: Option<i64>,
    pub created_at: i64,
    /// `agent` or `research`.
    pub kind: String,
    pub options: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NewScheduledTask {
    pub name: String,
    pub prompt: String,
    pub agent: String,
    #[serde(default)]
    pub project_id: Option<i64>,
    pub schedule: Schedule,
    #[serde(default = "agent_kind")]
    pub kind: String,
    #[serde(default = "empty_options")]
    pub options: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScheduledRun {
    pub id: i64,
    pub task_id: i64,
    pub scheduled_for: i64,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    /// `running`, `done`, `failed` or `interrupted`.
    pub state: String,
    pub session_id: Option<i64>,
    pub summary: Option<String>,
    /// The research a research task produced in this run.
    pub research_id: Option<i64>,
}

const TASK_COLUMNS: &str = "id,name,prompt,agent,project_id,spec_json,enabled,next_run_at,last_run_at,created_at,task_kind,options_json";

/// Columns added in schema 13, as (table, column, definition).
pub(crate) const COLUMNS_V13: [(&str, &str, &str); 3] = [
    ("scheduled_tasks", "task_kind", "TEXT NOT NULL DEFAULT 'agent'"),
    ("scheduled_tasks", "options_json", "TEXT NOT NULL DEFAULT '{}'"),
    ("scheduled_runs", "research_id", "INTEGER"),
];

fn agent_kind() -> String {
    "agent".into()
}

fn empty_options() -> serde_json::Value {
    serde_json::json!({})
}

/// A task is either a prompt for an agent or a piece of research with its own options (depth, light mode).
pub fn check_kind(kind: &str, options: &serde_json::Value) -> Result<(), String> {
    match kind {
        "agent" => Ok(()),
        "research" => match options.get("depth").and_then(|d| d.as_str()) {
            Some("quick" | "standard" | "deep") | None => Ok(()),
            _ => Err("Choose quick, standard or deep research.".into()),
        },
        _ => Err("A task is either for an agent or a piece of research.".into()),
    }
}

fn task_row(row: &rusqlite::Row) -> rusqlite::Result<ScheduledTask> {
    let spec: String = row.get(5)?;
    let schedule = serde_json::from_str(&spec).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(ScheduledTask {
        id: row.get(0)?,
        name: row.get(1)?,
        prompt: row.get(2)?,
        agent: row.get(3)?,
        project_id: row.get(4)?,
        schedule,
        enabled: row.get::<_, i64>(6)? != 0,
        next_run_at: row.get(7)?,
        last_run_at: row.get(8)?,
        created_at: row.get(9)?,
        kind: row.get(10)?,
        options: serde_json::from_str(&row.get::<_, String>(11)?).unwrap_or_else(|_| empty_options()),
    })
}

fn run_row(row: &rusqlite::Row) -> rusqlite::Result<ScheduledRun> {
    Ok(ScheduledRun {
        id: row.get(0)?,
        task_id: row.get(1)?,
        scheduled_for: row.get(2)?,
        started_at: row.get(3)?,
        finished_at: row.get(4)?,
        state: row.get(5)?,
        session_id: row.get(6)?,
        summary: row.get(7)?,
        research_id: row.get(8)?,
    })
}

impl Store {
    pub fn create_scheduled_task(&self, task: &NewScheduledTask, now: i64) -> Result<i64, StoreError> {
        task.schedule.validate().map_err(StoreError::Invalid)?;
        check_kind(&task.kind, &task.options).map_err(StoreError::Invalid)?;
        if task.name.trim().is_empty() || task.prompt.trim().is_empty() {
            return Err(StoreError::Invalid("a task needs a name and something to do".into()));
        }
        let next = task.schedule.next_after(now);
        let connection = self.connection();
        connection.execute(
            "INSERT INTO scheduled_tasks (name,prompt,agent,project_id,spec_json,enabled,next_run_at,created_at,task_kind,options_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                task.name.trim(),
                task.prompt,
                task.agent,
                task.project_id,
                serde_json::to_string(&task.schedule)?,
                next.is_some(),
                next,
                now,
                task.kind,
                task.options.to_string()
            ],
        )?;
        Ok(connection.last_insert_rowid())
    }

    pub fn scheduled_tasks(&self) -> Result<Vec<ScheduledTask>, StoreError> {
        let connection = self.connection();
        let mut statement =
            connection.prepare(&format!("SELECT {TASK_COLUMNS} FROM scheduled_tasks ORDER BY id"))?;
        let rows = statement.query_map([], task_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn scheduled_task(&self, id: i64) -> Result<Option<ScheduledTask>, StoreError> {
        Ok(self
            .connection()
            .query_row(&format!("SELECT {TASK_COLUMNS} FROM scheduled_tasks WHERE id=?1"), [id], task_row)
            .optional()?)
    }

    /// Replaces what a task does and when. A changed schedule is counted again from `now`.
    pub fn update_scheduled_task(&self, id: i64, task: &NewScheduledTask, enabled: bool, now: i64) -> Result<bool, StoreError> {
        task.schedule.validate().map_err(StoreError::Invalid)?;
        check_kind(&task.kind, &task.options).map_err(StoreError::Invalid)?;
        let next = if enabled { task.schedule.next_after(now) } else { None };
        let enabled = enabled && next.is_some();
        Ok(self.connection().execute(
            "UPDATE scheduled_tasks SET name=?2,prompt=?3,agent=?4,project_id=?5,spec_json=?6,enabled=?7,next_run_at=?8,task_kind=?9,options_json=?10 WHERE id=?1",
            params![id, task.name.trim(), task.prompt, task.agent, task.project_id, serde_json::to_string(&task.schedule)?, enabled, next, task.kind, task.options.to_string()],
        )? > 0)
    }

    pub fn delete_scheduled_task(&self, id: i64) -> Result<bool, StoreError> {
        let mut connection = self.connection();
        let tx = connection.transaction()?;
        tx.execute("DELETE FROM scheduled_runs WHERE task_id=?1", [id])?;
        let removed = tx.execute("DELETE FROM scheduled_tasks WHERE id=?1", [id])? > 0;
        tx.commit()?;
        Ok(removed)
    }

    /// Takes every task that is due and has no run in progress: one run each, however many times it was missed
    /// (a service that was off for a day does not run a daily task a hundred times). The next run is saved at once.
    pub fn claim_due_tasks(&self, now: i64, limit: usize) -> Result<Vec<(ScheduledTask, i64)>, StoreError> {
        let mut connection = self.connection();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let due = {
            let mut statement = tx.prepare(&format!(
                "SELECT {TASK_COLUMNS} FROM scheduled_tasks t WHERE enabled=1 AND next_run_at IS NOT NULL AND next_run_at<=?1
                 AND NOT EXISTS (SELECT 1 FROM scheduled_runs r WHERE r.task_id=t.id AND r.state='running')
                 ORDER BY next_run_at,id LIMIT ?2"
            ))?;
            statement.query_map(params![now, limit.min(50) as i64], task_row)?.collect::<Result<Vec<_>, _>>()?
        };
        let mut claimed = Vec::new();
        for task in due {
            let scheduled_for = task.next_run_at.unwrap_or(now);
            let next = task.schedule.next_after(now);
            tx.execute(
                "UPDATE scheduled_tasks SET next_run_at=?2,enabled=?3,last_run_at=?4 WHERE id=?1",
                params![task.id, next, next.is_some(), now],
            )?;
            tx.execute(
                "INSERT INTO scheduled_runs (task_id,scheduled_for,started_at,state) VALUES (?1,?2,?3,'running')",
                params![task.id, scheduled_for, now],
            )?;
            claimed.push((task, tx.last_insert_rowid()));
        }
        tx.commit()?;
        Ok(claimed)
    }

    /// Starts a run of one task now, outside its schedule. None when the task already has a run in progress.
    pub fn claim_task_now(&self, id: i64, now: i64) -> Result<Option<i64>, StoreError> {
        let mut connection = self.connection();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let busy: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM scheduled_runs WHERE task_id=?1 AND state='running')",
            [id],
            |r| r.get(0),
        )?;
        if busy {
            return Ok(None);
        }
        tx.execute(
            "INSERT INTO scheduled_runs (task_id,scheduled_for,started_at,state) VALUES (?1,?2,?2,'running')",
            params![id, now],
        )?;
        let run = tx.last_insert_rowid();
        tx.execute("UPDATE scheduled_tasks SET last_run_at=?2 WHERE id=?1", params![id, now])?;
        tx.commit()?;
        Ok(Some(run))
    }

    pub fn attach_run_session(&self, run: i64, session: i64) -> Result<(), StoreError> {
        self.connection().execute("UPDATE scheduled_runs SET session_id=?2 WHERE id=?1", params![run, session])?;
        Ok(())
    }

    pub fn attach_run_research(&self, run: i64, research: i64) -> Result<(), StoreError> {
        self.connection().execute("UPDATE scheduled_runs SET research_id=?2 WHERE id=?1", params![run, research])?;
        Ok(())
    }

    pub fn finish_run(&self, run: i64, state: &str, summary: Option<&str>, now: i64) -> Result<(), StoreError> {
        let summary = summary.map(|s| s.chars().take(2000).collect::<String>());
        self.connection().execute(
            "UPDATE scheduled_runs SET state=?2,summary=?3,finished_at=?4 WHERE id=?1 AND state='running'",
            params![run, state, summary, now],
        )?;
        Ok(())
    }

    /// At start-up a run still marked running was cut short with the service.
    pub fn recover_runs(&self, now: i64) -> Result<usize, StoreError> {
        Ok(self.connection().execute(
            "UPDATE scheduled_runs SET state='interrupted',finished_at=?1,summary='The service stopped during this run.' WHERE state='running'",
            [now],
        )?)
    }

    /// The newest runs first; `task` limits them to one task.
    pub fn scheduled_runs(&self, task: Option<i64>, limit: usize) -> Result<Vec<ScheduledRun>, StoreError> {
        let connection = self.connection();
        let mut statement = connection.prepare(
            "SELECT id,task_id,scheduled_for,started_at,finished_at,state,session_id,summary,research_id FROM scheduled_runs
             WHERE (?1 IS NULL OR task_id=?1) ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = statement.query_map(params![task, limit.min(200) as i64], run_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

// ---------------------------------------------------------------- calendar

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CalendarEvent {
    pub id: i64,
    pub calendar: String,
    pub uid: String,
    pub title: String,
    pub notes: String,
    pub location: String,
    pub starts_at: i64,
    pub ends_at: i64,
    pub all_day: bool,
    pub rrule: Option<String>,
    pub source: String,
    pub href: Option<String>,
    pub etag: Option<String>,
    pub updated_at: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NewCalendarEvent {
    #[serde(default = "local_calendar")]
    pub calendar: String,
    /// Left empty for a new event; the same uid again replaces the event.
    #[serde(default)]
    pub uid: String,
    pub title: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub location: String,
    pub starts_at: i64,
    pub ends_at: i64,
    #[serde(default)]
    pub all_day: bool,
    #[serde(default)]
    pub rrule: Option<String>,
    #[serde(default = "local_calendar")]
    pub source: String,
    #[serde(default)]
    pub href: Option<String>,
    #[serde(default)]
    pub etag: Option<String>,
}

fn local_calendar() -> String {
    "local".into()
}

/// One occurrence of an event inside a range.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Occurrence {
    pub event_id: i64,
    pub title: String,
    pub location: String,
    pub notes: String,
    pub calendar: String,
    pub starts_at: i64,
    pub ends_at: i64,
    pub all_day: bool,
    pub recurring: bool,
}

const EVENT_COLUMNS: &str = "id,calendar,uid,title,notes,location,starts_at,ends_at,all_day,rrule,source,href,etag,updated_at";

fn event_row(row: &rusqlite::Row) -> rusqlite::Result<CalendarEvent> {
    Ok(CalendarEvent {
        id: row.get(0)?,
        calendar: row.get(1)?,
        uid: row.get(2)?,
        title: row.get(3)?,
        notes: row.get(4)?,
        location: row.get(5)?,
        starts_at: row.get(6)?,
        ends_at: row.get(7)?,
        all_day: row.get::<_, i64>(8)? != 0,
        rrule: row.get(9)?,
        source: row.get(10)?,
        href: row.get(11)?,
        etag: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

impl Store {
    /// Adds an event, or replaces the one of the same calendar and uid. Returns its id.
    pub fn save_calendar_event(&self, event: &NewCalendarEvent, now: i64) -> Result<i64, StoreError> {
        if event.title.trim().is_empty() {
            return Err(StoreError::Invalid("an event needs a title".into()));
        }
        if event.ends_at < event.starts_at {
            return Err(StoreError::Invalid("an event can't end before it starts".into()));
        }
        if let Some(rule) = &event.rrule {
            parse_rrule(rule).map_err(StoreError::Invalid)?;
        }
        let uid = if event.uid.is_empty() { format!("{}@vatra", uuid_like(now)) } else { event.uid.clone() };
        let connection = self.connection();
        connection.execute(
            "INSERT INTO calendar_events (calendar,uid,title,notes,location,starts_at,ends_at,all_day,rrule,source,href,etag,updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)
             ON CONFLICT(calendar,uid) DO UPDATE SET title=excluded.title,notes=excluded.notes,location=excluded.location,
               starts_at=excluded.starts_at,ends_at=excluded.ends_at,all_day=excluded.all_day,rrule=excluded.rrule,
               source=excluded.source,href=excluded.href,etag=excluded.etag,updated_at=excluded.updated_at",
            params![
                event.calendar, uid, event.title.trim(), event.notes, event.location, event.starts_at, event.ends_at,
                event.all_day, event.rrule, event.source, event.href, event.etag, now
            ],
        )?;
        Ok(connection.query_row(
            "SELECT id FROM calendar_events WHERE calendar=?1 AND uid=?2",
            params![event.calendar, uid],
            |r| r.get(0),
        )?)
    }

    pub fn calendar_event(&self, id: i64) -> Result<Option<CalendarEvent>, StoreError> {
        Ok(self
            .connection()
            .query_row(&format!("SELECT {EVENT_COLUMNS} FROM calendar_events WHERE id=?1"), [id], event_row)
            .optional()?)
    }

    pub fn delete_calendar_event(&self, id: i64) -> Result<bool, StoreError> {
        Ok(self.connection().execute("DELETE FROM calendar_events WHERE id=?1", [id])? > 0)
    }

    /// Removes the events of one calendar whose uid is not in `keep`: what a sync found gone on the server.
    pub fn prune_calendar(&self, calendar: &str, keep: &[String]) -> Result<usize, StoreError> {
        let mut connection = self.connection();
        let tx = connection.transaction()?;
        let existing: Vec<(i64, String)> = {
            let mut statement = tx.prepare("SELECT id,uid FROM calendar_events WHERE calendar=?1 AND source!='local'")?;
            statement.query_map([calendar], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?
        };
        let mut removed = 0;
        for (id, uid) in existing {
            if !keep.contains(&uid) {
                removed += tx.execute("DELETE FROM calendar_events WHERE id=?1", [id])?;
            }
        }
        tx.commit()?;
        Ok(removed)
    }

    pub fn calendar_events_in(&self, from: i64, to: i64) -> Result<Vec<CalendarEvent>, StoreError> {
        let connection = self.connection();
        let mut statement = connection.prepare(&format!(
            "SELECT {EVENT_COLUMNS} FROM calendar_events WHERE (rrule IS NOT NULL AND starts_at<?2) OR (starts_at<?2 AND ends_at>=?1)
             ORDER BY starts_at,id"
        ))?;
        let rows = statement.query_map(params![from, to], event_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Every occurrence that overlaps `[from, to)`, recurring events expanded, in time order. At most 2000.
    pub fn calendar_occurrences(&self, from: i64, to: i64) -> Result<Vec<Occurrence>, StoreError> {
        let mut all = Vec::new();
        for event in self.calendar_events_in(from, to)? {
            let length = event.ends_at - event.starts_at;
            let starts = match &event.rrule {
                Some(rule) => expand(rule, event.starts_at, from - length, to, 2000).unwrap_or_default(),
                None => vec![event.starts_at],
            };
            for start in starts {
                if start < to && start + length >= from && (length > 0 || start >= from) {
                    all.push(Occurrence {
                        event_id: event.id,
                        title: event.title.clone(),
                        location: event.location.clone(),
                        notes: event.notes.clone(),
                        calendar: event.calendar.clone(),
                        starts_at: start,
                        ends_at: start + length,
                        all_day: event.all_day,
                        recurring: event.rrule.is_some(),
                    });
                }
            }
        }
        all.sort_by_key(|o| (o.starts_at, o.event_id));
        all.truncate(2000);
        Ok(all)
    }
}

fn uuid_like(seed: i64) -> String {
    use sha2::{Digest, Sha256};
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let digest = Sha256::digest(format!("{seed}-{n}-{}", std::process::id()));
    digest.iter().take(12).map(|b| format!("{b:02x}")).collect()
}

// -------------------------------------------------------------- recurrence

#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub freq: Freq,
    pub interval: i64,
    pub count: Option<usize>,
    pub until: Option<i64>,
    /// 0 (Sunday) to 6.
    pub by_day: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Freq {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

/// Parses the part of an iCalendar RRULE that calendars use every day: FREQ, INTERVAL, COUNT, UNTIL and BYDAY.
pub fn parse_rrule(rule: &str) -> Result<Rule, String> {
    let rule = rule.trim().trim_start_matches("RRULE:");
    let mut parsed = Rule { freq: Freq::Daily, interval: 1, count: None, until: None, by_day: vec![] };
    let mut has_freq = false;
    for part in rule.split(';').filter(|p| !p.is_empty()) {
        let (key, value) = part.split_once('=').ok_or("the repeat rule isn't valid")?;
        match key.to_ascii_uppercase().as_str() {
            "FREQ" => {
                has_freq = true;
                parsed.freq = match value.to_ascii_uppercase().as_str() {
                    "DAILY" => Freq::Daily,
                    "WEEKLY" => Freq::Weekly,
                    "MONTHLY" => Freq::Monthly,
                    "YEARLY" => Freq::Yearly,
                    _ => return Err("only daily, weekly, monthly and yearly repeats are supported".into()),
                }
            }
            "INTERVAL" => parsed.interval = value.parse().map_err(|_| "the repeat interval isn't a number")?,
            "COUNT" => parsed.count = Some(value.parse().map_err(|_| "the repeat count isn't a number")?),
            "UNTIL" => parsed.until = Some(parse_ical_time(value).ok_or("the repeat end isn't a date")?.0),
            "BYDAY" => {
                for day in value.split(',') {
                    let letters: String = day.chars().filter(char::is_ascii_alphabetic).collect();
                    parsed.by_day.push(match letters.to_ascii_uppercase().as_str() {
                        "SU" => 0,
                        "MO" => 1,
                        "TU" => 2,
                        "WE" => 3,
                        "TH" => 4,
                        "FR" => 5,
                        "SA" => 6,
                        _ => return Err("a day of the week isn't valid".into()),
                    });
                }
            }
            _ => {}
        }
    }
    if !has_freq {
        return Err("the repeat rule has no frequency".into());
    }
    if parsed.interval < 1 {
        return Err("the repeat interval must be at least 1".into());
    }
    Ok(parsed)
}

/// The starts of the occurrences of a rule that begins at `first` and fall in `[from, to)`.
pub fn expand(rule: &str, first: i64, from: i64, to: i64, max: usize) -> Result<Vec<i64>, String> {
    let rule = parse_rrule(rule)?;
    let mut out = Vec::new();
    let mut produced = 0usize;
    let time_of_day = first.rem_euclid(86_400);
    let first_day = first.div_euclid(86_400);
    let (y0, m0, d0) = civil_from_days(first_day);
    let mut push = |start: i64, produced: &mut usize| -> bool {
        if start < first {
            return true;
        }
        if rule.until.is_some_and(|u| start > u) || rule.count.is_some_and(|c| *produced >= c) || start >= to {
            return false;
        }
        *produced += 1;
        if start >= from {
            out.push(start);
        }
        out.len() < max
    };
    match rule.freq {
        Freq::Daily => {
            let mut day = first_day;
            loop {
                if !push(day * 86_400 + time_of_day, &mut produced) {
                    break;
                }
                day += rule.interval;
            }
        }
        Freq::Weekly => {
            let days: Vec<u32> = if rule.by_day.is_empty() { vec![((first_day + 4).rem_euclid(7)) as u32] } else { rule.by_day.clone() };
            // Weeks start on Monday for counting `interval`.
            let week_start = first_day - (first_day + 3).rem_euclid(7);
            let mut week = week_start;
            'weeks: loop {
                let mut in_week: Vec<i64> = days.iter().map(|d| week + ((*d as i64 + 6) % 7)).collect();
                in_week.sort_unstable();
                for day in in_week {
                    if day < first_day {
                        continue;
                    }
                    if !push(day * 86_400 + time_of_day, &mut produced) {
                        break 'weeks;
                    }
                }
                week += 7 * rule.interval;
            }
        }
        Freq::Monthly | Freq::Yearly => {
            let step = if matches!(rule.freq, Freq::Monthly) { rule.interval } else { rule.interval * 12 };
            let mut k = 0i64;
            loop {
                let total = i64::from(m0) - 1 + k * step;
                let year = y0 + total.div_euclid(12);
                let month = (total.rem_euclid(12) + 1) as u32;
                // A day that month doesn't have (the 31st of April) is skipped, as the standard says.
                if d0 <= days_in_month(year, month) {
                    let day = days_from_civil(year, month, d0);
                    if !push(day * 86_400 + time_of_day, &mut produced) {
                        break;
                    }
                }
                k += 1;
                if k > 100_000 {
                    break;
                }
            }
        }
    }
    Ok(out)
}

pub fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        _ => 28,
    }
}

pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `20261107`, `20261107T093000` or `20261107T093000Z`: the instant in UTC and whether it is a whole day.
/// A time without a Z is taken as UTC; calendars that use named time zones are converted by the sync code first.
pub fn parse_ical_time(value: &str) -> Option<(i64, bool)> {
    let value = value.trim();
    let digits = |s: &str| -> Option<i64> { (s.len() >= 2 && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok()).flatten() };
    if value.len() == 8 {
        let (y, m, d) = (digits(&value[0..4])?, digits(&value[4..6])? as u32, digits(&value[6..8])? as u32);
        if !(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m) {
            return None;
        }
        return Some((days_from_civil(y, m, d) * 86_400, true));
    }
    let trimmed = value.trim_end_matches('Z');
    if trimmed.len() != 15 || trimmed.as_bytes()[8] != b'T' {
        return None;
    }
    let (y, m, d) = (digits(&trimmed[0..4])?, digits(&trimmed[4..6])? as u32, digits(&trimmed[6..8])? as u32);
    let (h, mi, s) = (digits(&trimmed[9..11])?, digits(&trimmed[11..13])?, digits(&trimmed[13..15])?);
    if !(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m) || h > 23 || mi > 59 || s > 60 {
        return None;
    }
    Some((days_from_civil(y, m, d) * 86_400 + h * 3600 + mi * 60 + s, false))
}

pub fn format_ical_time(at: i64, all_day: bool) -> String {
    let (y, m, d) = civil_from_days(at.div_euclid(86_400));
    if all_day {
        return format!("{y:04}{m:02}{d:02}");
    }
    let s = at.rem_euclid(86_400);
    format!("{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z", s / 3600, s % 3600 / 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(y: i64, m: u32, d: u32) -> i64 {
        days_from_civil(y, m, d) * 86_400
    }

    #[test]
    fn civil_round_trip() {
        for z in [-800_000, -1, 0, 1, 18_000, 20_000, 2_932_896] {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z);
        }
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }

    #[test]
    fn daily_schedule_uses_the_local_time_of_day() {
        // 09:00 in UTC+3 is 06:00 UTC.
        let schedule = Schedule::Daily { hour: 9, minute: 0, tz_offset_min: 180 };
        let after = day(2026, 11, 7) + 5 * 3600;
        assert_eq!(schedule.next_after(after), Some(day(2026, 11, 7) + 6 * 3600));
        assert_eq!(schedule.next_after(day(2026, 11, 7) + 6 * 3600), Some(day(2026, 11, 8) + 6 * 3600));
    }

    #[test]
    fn weekly_schedule_picks_the_next_listed_day() {
        // 2026-11-07 is a Saturday (6). Monday (1) is 2026-11-09.
        let schedule = Schedule::Weekly { days: vec![1, 3], hour: 8, minute: 30, tz_offset_min: 0 };
        assert_eq!(schedule.next_after(day(2026, 11, 7)), Some(day(2026, 11, 9) + 8 * 3600 + 30 * 60));
    }

    #[test]
    fn once_is_spent_after_it_ran() {
        let schedule = Schedule::Once { at: 1000 };
        assert_eq!(schedule.next_after(10), Some(1000));
        assert_eq!(schedule.next_after(1000), None);
    }

    #[test]
    fn invalid_schedules_are_refused() {
        assert!(Schedule::Interval { every_secs: 5 }.validate().is_err());
        assert!(Schedule::Weekly { days: vec![], hour: 1, minute: 1, tz_offset_min: 0 }.validate().is_err());
        assert!(Schedule::Daily { hour: 24, minute: 0, tz_offset_min: 0 }.validate().is_err());
    }

    #[test]
    fn a_missed_task_runs_once_and_the_service_restart_marks_the_run() {
        let store = Store::open_in_memory().unwrap();
        let id = store
            .create_scheduled_task(
                &NewScheduledTask {
                    name: "Morning".into(),
                    prompt: "Summarise the day".into(),
                    agent: "a".into(),
                    project_id: None,
                    schedule: Schedule::Interval { every_secs: 3600 },
                    kind: "agent".into(),
                    options: serde_json::json!({}),
                },
                1000,
            )
            .unwrap();
        // Ten hours later, one run, not ten.
        let claimed = store.claim_due_tasks(1000 + 10 * 3600, 10).unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].0.id, id);
        // A run in progress blocks the next claim.
        assert!(store.claim_due_tasks(1000 + 12 * 3600, 10).unwrap().is_empty());
        assert_eq!(store.recover_runs(1000 + 13 * 3600).unwrap(), 1);
        let runs = store.scheduled_runs(Some(id), 10).unwrap();
        assert_eq!(runs[0].state, "interrupted");
        assert_eq!(store.claim_due_tasks(1000 + 14 * 3600, 10).unwrap().len(), 1);
    }

    #[test]
    fn a_once_task_disables_itself_after_its_run() {
        let store = Store::open_in_memory().unwrap();
        let id = store
            .create_scheduled_task(
                &NewScheduledTask { name: "x".into(), prompt: "y".into(), agent: "a".into(), project_id: None, schedule: Schedule::Once { at: 2000 }, kind: "agent".into(), options: serde_json::json!({}) },
                1000,
            )
            .unwrap();
        let claimed = store.claim_due_tasks(2001, 5).unwrap();
        let run = claimed[0].1;
        store.finish_run(run, "done", Some("ok"), 2002).unwrap();
        let task = store.scheduled_task(id).unwrap().unwrap();
        assert!(!task.enabled);
        assert_eq!(task.next_run_at, None);
        assert!(store.claim_due_tasks(9999, 5).unwrap().is_empty());
    }

    #[test]
    fn weekly_recurrence_expands_by_day() {
        // Starts Monday 2026-11-02 09:00, every Monday and Thursday.
        let first = day(2026, 11, 2) + 9 * 3600;
        let starts = expand("FREQ=WEEKLY;BYDAY=MO,TH", first, day(2026, 11, 1), day(2026, 11, 15), 100).unwrap();
        assert_eq!(
            starts,
            vec![first, day(2026, 11, 5) + 9 * 3600, day(2026, 11, 9) + 9 * 3600, day(2026, 11, 12) + 9 * 3600]
        );
    }

    #[test]
    fn count_and_until_stop_a_recurrence() {
        let first = day(2026, 1, 1);
        assert_eq!(expand("FREQ=DAILY;COUNT=3", first, 0, day(2027, 1, 1), 100).unwrap().len(), 3);
        let until = format_ical_time(day(2026, 1, 5), true);
        assert_eq!(expand(&format!("FREQ=DAILY;UNTIL={until}"), first, 0, day(2027, 1, 1), 100).unwrap().len(), 5);
    }

    #[test]
    fn monthly_on_the_31st_skips_short_months() {
        let first = day(2026, 1, 31);
        let starts = expand("FREQ=MONTHLY", first, 0, day(2026, 6, 1), 100).unwrap();
        let months: Vec<u32> = starts.iter().map(|s| civil_from_days(s / 86_400).1).collect();
        assert_eq!(months, vec![1, 3, 5]);
    }

    #[test]
    fn events_expand_inside_the_range_and_show_in_order() {
        let store = Store::open_in_memory().unwrap();
        let standup = NewCalendarEvent {
            calendar: "local".into(),
            uid: "s".into(),
            title: "Standup".into(),
            notes: String::new(),
            location: String::new(),
            starts_at: day(2026, 11, 2) + 9 * 3600,
            ends_at: day(2026, 11, 2) + 9 * 3600 + 900,
            all_day: false,
            rrule: Some("FREQ=DAILY;COUNT=10".into()),
            source: "local".into(),
            href: None,
            etag: None,
        };
        store.save_calendar_event(&standup, 1).unwrap();
        let dentist = NewCalendarEvent { uid: "d".into(), title: "Dentist".into(), rrule: None, starts_at: day(2026, 11, 4) + 3600, ends_at: day(2026, 11, 4) + 7200, ..standup.clone() };
        store.save_calendar_event(&dentist, 1).unwrap();
        let found = store.calendar_occurrences(day(2026, 11, 3), day(2026, 11, 5)).unwrap();
        let titles: Vec<&str> = found.iter().map(|o| o.title.as_str()).collect();
        assert_eq!(titles, vec!["Standup", "Dentist", "Standup"]);
    }

    #[test]
    fn saving_the_same_uid_replaces_and_a_bad_rule_is_refused() {
        let store = Store::open_in_memory().unwrap();
        let mut event = NewCalendarEvent {
            calendar: "local".into(), uid: "u".into(), title: "A".into(), notes: String::new(), location: String::new(),
            starts_at: 10, ends_at: 20, all_day: false, rrule: None, source: "local".into(), href: None, etag: None,
        };
        let first = store.save_calendar_event(&event, 1).unwrap();
        event.title = "B".into();
        assert_eq!(store.save_calendar_event(&event, 2).unwrap(), first);
        assert_eq!(store.calendar_event(first).unwrap().unwrap().title, "B");
        event.rrule = Some("FREQ=HOURLY".into());
        assert!(store.save_calendar_event(&event, 3).is_err());
    }

    #[test]
    fn ical_times_parse_and_format() {
        assert_eq!(parse_ical_time("19700101T000001Z"), Some((1, false)));
        assert_eq!(parse_ical_time("20260230"), None);
        assert_eq!(format_ical_time(day(2026, 11, 7) + 3661, false), "20261107T010101Z");
    }
}
