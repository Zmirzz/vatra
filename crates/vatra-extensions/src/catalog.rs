use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::types::ToSql;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use vatra_core::{
    McpServerConfig, PluginContext, Rule, Rules, SecretError, SecretRef, SecretStore,
};

use crate::bang::{BangForm, bang_spans};
use crate::hash::{content_hash, files};
use crate::{FormatError, LiteralSecret, ManifestKind, Skill, parse_skill, read_plugin};

/// How long a write waits for the other connection to the same file, which is the one the service's store uses.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS extensions (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        kind TEXT NOT NULL,
        name TEXT NOT NULL,
        origin TEXT NOT NULL,
        scope TEXT NOT NULL,
        plugin TEXT NOT NULL DEFAULT '',
        content_hash TEXT NOT NULL,
        content_dir TEXT NOT NULL,
        enabled INTEGER NOT NULL DEFAULT 1,
        runs_code TEXT NOT NULL DEFAULT '[]',
        approved_hash TEXT,
        details TEXT NOT NULL DEFAULT '{}',
        source_hash TEXT NOT NULL DEFAULT '',
        held_for_secret INTEGER NOT NULL DEFAULT 0,
        UNIQUE (origin, kind, name, plugin, scope)
    );
    CREATE TABLE IF NOT EXISTS extension_uses (
        name TEXT PRIMARY KEY,
        uses INTEGER NOT NULL DEFAULT 0
    );
";

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("file error: {0}")]
    Io(#[from] io::Error),
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Format(#[from] FormatError),
    #[error(transparent)]
    Secret(#[from] SecretError),
    #[error("no extension with id {0}")]
    UnknownEntry(i64),
    #[error("{0}")]
    Invalid(String),
    #[error("the catalog holds a damaged row: {0}")]
    Corrupt(String),
}

/// Where an extension came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    Claude,
    Codex,
    Project,
    Manual,
}

impl Origin {
    pub const ALL: [Origin; 4] = [
        Origin::Claude,
        Origin::Codex,
        Origin::Project,
        Origin::Manual,
    ];

    fn as_str(self) -> &'static str {
        match self {
            Origin::Claude => "claude",
            Origin::Codex => "codex",
            Origin::Project => "project",
            Origin::Manual => "manual",
        }
    }

    fn parse(text: &str) -> Option<Origin> {
        Origin::ALL
            .into_iter()
            .find(|origin| origin.as_str() == text)
    }
}

/// What kind of element a catalog entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElementKind {
    Plugin,
    Skill,
    Command,
    McpServer,
    Instructions,
    Memory,
    /// Permission rules from a settings file. Their details hold the rules; there is no stored copy.
    Permission,
}

impl ElementKind {
    pub const ALL: [ElementKind; 7] = [
        ElementKind::Plugin,
        ElementKind::Skill,
        ElementKind::Command,
        ElementKind::McpServer,
        ElementKind::Instructions,
        ElementKind::Memory,
        ElementKind::Permission,
    ];

    fn as_str(self) -> &'static str {
        match self {
            ElementKind::Plugin => "plugin",
            ElementKind::Skill => "skill",
            ElementKind::Command => "command",
            ElementKind::McpServer => "mcp_server",
            ElementKind::Instructions => "instructions",
            ElementKind::Memory => "memory",
            ElementKind::Permission => "permission",
        }
    }

    fn parse(text: &str) -> Option<ElementKind> {
        ElementKind::ALL
            .into_iter()
            .find(|kind| kind.as_str() == text)
    }
}

/// Where an extension is active: everywhere, or in one project.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Global,
    Project(i64),
}

impl Scope {
    fn key(self) -> String {
        match self {
            Scope::Global => "global".into(),
            Scope::Project(id) => format!("project:{id}"),
        }
    }

    fn parse(text: &str) -> Option<Scope> {
        match text.strip_prefix("project:") {
            Some(id) => id.parse().ok().map(Scope::Project),
            None => (text == "global").then_some(Scope::Global),
        }
    }
}

/// A command a skill or an MCP server runs on this machine, shown before the user approves it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunsCode {
    pub what: String,
    pub command: String,
}

/// One extension in the catalog. Its content is a copy under `content_dir`, which is named by the hash of the
/// content, so a changed extension gets a new folder and the old one stays until it is pruned.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub id: i64,
    pub kind: ElementKind,
    pub name: String,
    pub origin: Origin,
    pub scope: Scope,
    pub content_hash: String,
    pub content_dir: PathBuf,
    pub enabled: bool,
    pub runs_code: Vec<RunsCode>,
    /// The hash of the content the user approved the code of. Code runs only while it matches `content_hash`.
    pub approved_hash: Option<String>,
    /// The plugin this entry belongs to, if any. A plugin's parts share its content folder.
    pub plugin: Option<String>,
    pub details: Value,
}

/// What `put` did with an entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PutOutcome {
    Added,
    Unchanged,
    Changed,
}

/// What adding a folder did: one outcome per entry it gave, the problems its files had, and the names of the entries
/// that run code and are not approved, which stay out of the active set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AddReport {
    pub outcomes: Vec<PutOutcome>,
    pub problems: Vec<String>,
    pub unapproved: Vec<String>,
}

/// The approvals in effect: for each entry, the hash of the content whose code the user approved.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApprovedSet {
    approvals: HashMap<i64, String>,
}

impl ApprovedSet {
    pub fn contains(&self, entry_id: i64, hash: &str) -> bool {
        self.approvals
            .get(&entry_id)
            .is_some_and(|approved| approved == hash)
    }
}

/// How many times each skill or command was run, by the name the person types. It orders the skill listing, so the
/// most used skills keep their descriptions when the listing is cut down.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UsageCounts {
    counts: HashMap<String, u64>,
}

impl UsageCounts {
    pub fn from_pairs(pairs: impl IntoIterator<Item = (String, u64)>) -> UsageCounts {
        UsageCounts {
            counts: pairs.into_iter().collect(),
        }
    }

    pub fn get(&self, name: &str) -> u64 {
        self.counts.get(name).copied().unwrap_or(0)
    }
}

/// The extensions that apply to one project, read into the parts the engine uses. `problems` names what was left out
/// and why: a row that can't be read, a stored copy that no longer matches its hash, or a part that is missing.
#[derive(Debug, Default)]
pub struct ActiveExtensions {
    pub skills: Vec<Skill>,
    pub mcp_servers: Vec<McpServerConfig>,
    pub instructions: Vec<PathBuf>,
    pub memory: Vec<PathBuf>,
    pub rules: Rules,
    pub approved: ApprovedSet,
    pub problems: Vec<String>,
}

/// The extensions of the service, kept in the same database file as its store.
pub struct Catalog {
    connection: Mutex<Connection>,
    data_dir: PathBuf,
}

impl Catalog {
    /// The catalog of a database, read only: nothing is created or changed. A dry run uses it.
    pub fn open_read_only(db: &Path, data_dir: &Path) -> Result<Catalog, CatalogError> {
        let connection = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        connection.busy_timeout(BUSY_TIMEOUT)?;
        Ok(Catalog {
            connection: Mutex::new(connection),
            data_dir: data_dir.to_path_buf(),
        })
    }

    pub fn open(db: &Path, data_dir: &Path) -> Result<Catalog, CatalogError> {
        let connection = Connection::open(db)?;
        connection.busy_timeout(BUSY_TIMEOUT)?;
        connection.execute_batch(SCHEMA)?;
        migrate(&connection)?;
        Ok(Catalog {
            connection: Mutex::new(connection),
            data_dir: data_dir.to_path_buf(),
        })
    }

    fn connection(&self) -> MutexGuard<'_, Connection> {
        self.connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The entries. A row that can't be read is left out here, and `active_for` reports it.
    pub fn list(&self) -> Result<Vec<CatalogEntry>, CatalogError> {
        Ok(self.scan()?.0)
    }

    /// The entries that can be read, and a problem for each row that can't.
    fn scan(&self) -> Result<(Vec<CatalogEntry>, Vec<String>), CatalogError> {
        let connection = self.connection();
        let mut statement = connection.prepare(
            "SELECT id, kind, name, origin, scope, plugin, content_hash, content_dir, enabled, runs_code,
                    approved_hash, details
             FROM extensions ORDER BY id",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok(Row {
                    id: row.get(0)?,
                    kind: row.get(1)?,
                    name: row.get(2)?,
                    origin: row.get(3)?,
                    scope: row.get(4)?,
                    plugin: row.get(5)?,
                    content_hash: row.get(6)?,
                    content_dir: row.get(7)?,
                    enabled: row.get(8)?,
                    runs_code: row.get(9)?,
                    approved_hash: row.get(10)?,
                    details: row.get(11)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<Row>>>()?;
        let mut entries = Vec::new();
        let mut problems = Vec::new();
        for row in rows {
            let id = row.id;
            match row.into_entry() {
                Ok(entry) => entries.push(entry),
                Err(error) => problems.push(format!("extension {id} can't be read: {error}")),
            }
        }
        Ok((entries, problems))
    }

    fn entry(&self, id: i64) -> Result<CatalogEntry, CatalogError> {
        self.list()?
            .into_iter()
            .find(|entry| entry.id == id)
            .ok_or(CatalogError::UnknownEntry(id))
    }

    /// Stores a copy of `source_dir` for `entry`, unless the same content is already there. The hash of the folder is
    /// computed here, so the entry's own hash is replaced.
    pub fn put(&self, entry: CatalogEntry, source_dir: &Path) -> Result<PutOutcome, CatalogError> {
        let source = content_hash(source_dir)?;
        let root = entry.plugin.clone().unwrap_or_else(|| entry.name.clone());
        let (hash, dir) = self.store_copy(entry.origin, &root, Some(&source), |staging| {
            copy_tree(source_dir, staging)
        })?;
        let (outcome, _) = self.record(entry, hash, dir, source, false)?;
        Ok(outcome)
    }

    /// Stores a copy of `source_dir` for `entry`, whose source hashed to `source_hash` when the import read it. The
    /// copy may differ from the source, as when secret values are taken out of it, so the next import compares the
    /// source hash, not the copy's.
    pub fn put_from(
        &self,
        entry: CatalogEntry,
        source_dir: &Path,
        source_hash: &str,
    ) -> Result<PutOutcome, CatalogError> {
        let root = entry.plugin.clone().unwrap_or_else(|| entry.name.clone());
        let (hash, dir) = self.store_copy(entry.origin, &root, None, |staging| {
            copy_tree(source_dir, staging)
        })?;
        let (outcome, _) = self.record(entry, hash, dir, source_hash.to_string(), false)?;
        Ok(outcome)
    }

    /// Adds the MCP servers of a configuration file, each as an entry with the given scope. Each server is kept as
    /// JSON, so it has no code to approve when it has a URL instead of a command.
    pub fn add_servers(
        &self,
        servers: &[McpServerConfig],
        scope: Scope,
    ) -> Result<Vec<PutOutcome>, CatalogError> {
        servers
            .iter()
            .map(|server| self.add_server(server, Origin::Manual, scope, None, false))
            .collect()
    }

    /// Makes the configuration's servers the catalog's `Manual` servers in `scope`: each is added or updated, and a
    /// server the configuration no longer names leaves the catalog. Servers that belong to a plugin are left alone.
    pub fn sync_servers(
        &self,
        servers: &[McpServerConfig],
        scope: Scope,
    ) -> Result<Vec<PutOutcome>, CatalogError> {
        let outcomes = self.add_servers(servers, scope)?;
        let names: HashSet<&str> = servers.iter().map(|server| server.name.as_str()).collect();
        let stale: Vec<i64> = self
            .list()?
            .iter()
            .filter(|entry| {
                entry.origin == Origin::Manual
                    && entry.kind == ElementKind::McpServer
                    && entry.plugin.is_none()
                    && entry.scope == scope
                    && !names.contains(entry.name.as_str())
            })
            .map(|entry| entry.id)
            .collect();
        let connection = self.connection();
        for id in stale {
            connection.execute("DELETE FROM extensions WHERE id = ?1", [id])?;
        }
        Ok(outcomes)
    }

    /// Adds one MCP server with its origin and scope. The server is kept as JSON, so it has no code to approve when it
    /// has a URL and no headers helper. `source_hash` is what the server's source hashed to, when that differs from the
    /// stored text. With `hold`, a new row is written off and held for a value.
    pub fn add_server(
        &self,
        server: &McpServerConfig,
        origin: Origin,
        scope: Scope,
        source_hash: Option<&str>,
        hold: bool,
    ) -> Result<PutOutcome, CatalogError> {
        server.validate().map_err(CatalogError::Invalid)?;
        let text = serde_json::to_string_pretty(server)?;
        let (hash, dir) = self.store_copy(origin, &server.name, None, |staging| {
            fs::create_dir_all(staging)?;
            fs::write(staging.join("server.json"), &text)
        })?;
        let mut entry = server_entry(server, origin, scope, None)?;
        entry.enabled = !hold;
        let source = source_hash.map_or_else(|| hash.clone(), str::to_string);
        let (outcome, _) = self.record(entry, hash, dir, source, hold)?;
        Ok(outcome)
    }

    /// Stores the permission rules of a settings file as one entry. `details` holds the `allow`, `ask` and `deny`
    /// lists. Nothing is copied, so the entry has no folder and no code to approve.
    pub fn put_rules(
        &self,
        origin: Origin,
        scope: Scope,
        name: &str,
        details: Value,
        source_hash: &str,
    ) -> Result<PutOutcome, CatalogError> {
        let entry = CatalogEntry {
            id: 0,
            kind: ElementKind::Permission,
            name: name.into(),
            origin,
            scope,
            content_hash: String::new(),
            content_dir: PathBuf::new(),
            enabled: true,
            runs_code: Vec::new(),
            approved_hash: None,
            plugin: None,
            details,
        };
        let (outcome, _) = self.record(
            entry,
            source_hash.to_string(),
            PathBuf::new(),
            source_hash.to_string(),
            false,
        )?;
        Ok(outcome)
    }

    /// Stores a command file as a command. `folder` holds the file as its `SKILL.md`. The command's code is approved
    /// only when `approve` is set.
    #[allow(clippy::too_many_arguments)]
    pub fn put_command(
        &self,
        name: &str,
        origin: Origin,
        scope: Scope,
        folder: &Path,
        runs_code: Vec<RunsCode>,
        details: Value,
        approve: bool,
        source_hash: &str,
    ) -> Result<PutOutcome, CatalogError> {
        let executable = !runs_code.is_empty();
        let entry = CatalogEntry {
            id: 0,
            kind: ElementKind::Command,
            name: name.into(),
            origin,
            scope,
            content_hash: String::new(),
            content_dir: PathBuf::new(),
            enabled: true,
            runs_code,
            approved_hash: None,
            plugin: None,
            details,
        };
        let (hash, dir) =
            self.store_copy(origin, name, None, |staging| copy_tree(folder, staging))?;
        let (outcome, id) =
            self.record(entry, hash.clone(), dir, source_hash.to_string(), false)?;
        if approve && executable {
            self.approve(id, &hash)?;
        }
        Ok(outcome)
    }

    /// Holds a server off because it waits for a value. The mark tells a later import that the hold is its own.
    pub fn hold(&self, id: i64) -> Result<(), CatalogError> {
        self.update(
            "UPDATE extensions SET enabled = 0, held_for_secret = 1 WHERE id = ?1",
            params![id],
            id,
        )
    }

    /// Releases a server held for a value: it is on again, and the mark is cleared.
    pub fn release(&self, id: i64) -> Result<(), CatalogError> {
        self.update(
            "UPDATE extensions SET enabled = 1, held_for_secret = 0 WHERE id = ?1",
            params![id],
            id,
        )
    }

    /// Whether a server is held for a value.
    pub fn held_for_secret(&self, id: i64) -> Result<bool, CatalogError> {
        let held: Option<bool> = self
            .connection()
            .query_row(
                "SELECT held_for_secret FROM extensions WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        held.ok_or(CatalogError::UnknownEntry(id))
    }

    /// The source hash of every row, by id: what the source of each row hashed to when the row was last written.
    pub fn source_hashes(&self) -> Result<HashMap<i64, String>, CatalogError> {
        let connection = self.connection();
        // A database from before `source_hash` has nothing imported yet, so no row has a source hash.
        if !has_column(&connection, "source_hash")? {
            return Ok(HashMap::new());
        }
        let mut statement = connection.prepare("SELECT id, source_hash FROM extensions")?;
        let pairs = statement
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<HashMap<i64, String>>>()?;
        Ok(pairs)
    }

    /// Adds a plugin folder or a skill folder, with the given origin and scope. Code the folder runs is approved
    /// only when `approve` is set; the rest is active at once. A plugin's parts that its folder no longer has are
    /// removed from the catalog.
    pub fn add_folder(
        &self,
        folder: &Path,
        origin: Origin,
        scope: Scope,
        approve: bool,
    ) -> Result<AddReport, CatalogError> {
        self.add_folder_as(folder, origin, scope, approve, None)
    }

    /// Like `add_folder`, for a folder that is a copy of a source. `source_hash` is what the source hashed to, when
    /// the copy has differences such as secret values taken out of it.
    pub fn add_folder_as(
        &self,
        folder: &Path,
        origin: Origin,
        scope: Scope,
        approve: bool,
        source_hash: Option<&str>,
    ) -> Result<AddReport, CatalogError> {
        self.add_folder_impl(folder, origin, scope, approve, source_hash, None)
    }

    /// Installs exactly the reviewed content; executable content remains unapproved.
    pub fn add_folder_checked(&self, folder: &Path, origin: Origin, scope: Scope, expected_hash: &str) -> Result<AddReport, CatalogError> {
        self.add_folder_impl(folder, origin, scope, false, None, Some(expected_hash))
    }

    fn add_folder_impl(&self, folder: &Path, origin: Origin, scope: Scope, approve: bool, source_hash: Option<&str>, expected_hash: Option<&str>) -> Result<AddReport, CatalogError> {
        let hash = content_hash(folder)?;
        if expected_hash.is_some_and(|expected| expected != hash) {
            return Err(CatalogError::Invalid("Extension changed since review. Reload and inspect it again.".into()));
        }
        let mut report = AddReport::default();
        let (root, entries, removal) = if folder.join("SKILL.md").is_file() {
            let skill = parse_skill(folder)?;
            (
                skill.name.clone(),
                vec![skill_entry(&skill, origin, scope, None)],
                None,
            )
        } else {
            let bundle = read_plugin(folder)?;
            if bundle.manifest_kind == ManifestKind::None
                && bundle.skills.is_empty()
                && bundle.mcp_servers.is_empty()
            {
                return Err(CatalogError::Invalid(format!(
                    "{} has no plugin, skill or MCP server to add",
                    folder.display()
                )));
            }
            report.problems = bundle.problems.clone();
            report
                .problems
                .extend(bundle.literal_secrets.iter().map(literal_problem));
            let plugin = Some(bundle.name.clone());
            let mut entries = vec![plugin_entry(&bundle, origin, scope)];
            entries.extend(
                bundle
                    .skills
                    .iter()
                    .map(|skill| skill_entry(skill, origin, scope, plugin.clone())),
            );
            for server in &bundle.mcp_servers {
                entries.push(server_entry(server, origin, scope, plugin.clone())?);
            }
            let present: Vec<(ElementKind, String)> = entries[1..]
                .iter()
                .map(|entry| (entry.kind, entry.name.clone()))
                .collect();
            (
                bundle.name.clone(),
                entries,
                Some((bundle.name.clone(), present)),
            )
        };

        let (copied, dir) = self.store_copy(origin, &root, Some(&hash), |staging| {
            copy_tree(folder, staging)
        })?;
        let source = source_hash.map_or_else(|| copied.clone(), str::to_string);
        let mut executable = Vec::new();
        for entry in entries {
            let runs_code = !entry.runs_code.is_empty();
            let name = entry.name.clone();
            let (outcome, id) =
                self.record(entry, copied.clone(), dir.clone(), source.clone(), false)?;
            report.outcomes.push(outcome);
            if runs_code {
                executable.push((name, id));
            }
        }
        if let Some((plugin, present)) = removal {
            self.remove_missing(origin, scope, &plugin, &present)?;
        }
        if approve {
            for (_, id) in &executable {
                self.approve(*id, &copied)?;
            }
        }
        let approved: HashSet<i64> = self
            .list()?
            .iter()
            .filter(|entry| self.is_approved(entry))
            .map(|e| e.id)
            .collect();
        report.unapproved = executable
            .into_iter()
            .filter(|(_, id)| !approved.contains(id))
            .map(|(name, _)| name)
            .collect();
        Ok(report)
    }

    /// Copies content into a staging folder, takes the hash of the copy, and moves the copy under its hash folder. A
    /// folder already there is kept only when it still hashes to the same value; a partial or edited one is replaced.
    /// With `expected`, a copy that hashes to something else is refused, since the source changed while it was copied.
    fn store_copy(
        &self,
        origin: Origin,
        root: &str,
        expected: Option<&str>,
        write: impl FnOnce(&Path) -> io::Result<()>,
    ) -> Result<(String, PathBuf), CatalogError> {
        let staging_root = self.data_dir.join("extensions-staging");
        fs::create_dir_all(&staging_root)?;
        let staging = staging_root.join(unique_name());
        let copied = match write(&staging).and_then(|()| content_hash(&staging)) {
            Ok(hash) => hash,
            Err(error) => {
                let _ = fs::remove_dir_all(&staging);
                return Err(error.into());
            }
        };
        if expected.is_some_and(|expected| expected != copied) {
            let _ = fs::remove_dir_all(&staging);
            return Err(CatalogError::Invalid(format!(
                "{root} changed while it was copied; add it again"
            )));
        }
        let parent = self
            .data_dir
            .join("extensions")
            .join(origin.as_str())
            .join(folder_name(root));
        let dir = parent.join(&copied);
        if dir.is_dir() && content_hash(&dir).is_ok_and(|hash| hash == copied) {
            fs::remove_dir_all(&staging)?;
        } else {
            if dir.exists() {
                fs::remove_dir_all(&dir)?;
            }
            fs::create_dir_all(&parent)?;
            fs::rename(&staging, &dir)?;
        }
        Ok((copied, dir))
    }

    /// Writes the row of an entry whose content is stored in `dir`. A new row takes the entry; an existing row is
    /// updated in place, so its id and its plugin data stay. The check and the write are one transaction.
    fn record(
        &self,
        entry: CatalogEntry,
        hash: String,
        dir: PathBuf,
        source_hash: String,
        held: bool,
    ) -> Result<(PutOutcome, i64), CatalogError> {
        let plugin = entry.plugin.clone().unwrap_or_default();
        let runs_code = serde_json::to_string(&entry.runs_code)?;
        let connection = self.connection();
        let transaction = connection.unchecked_transaction()?;
        let existing: Option<(i64, String, String, String)> = transaction
            .query_row(
                "SELECT id, content_hash, source_hash, details FROM extensions
                 WHERE origin = ?1 AND kind = ?2 AND name = ?3 AND plugin = ?4 AND scope = ?5",
                params![
                    entry.origin.as_str(),
                    entry.kind.as_str(),
                    entry.name,
                    plugin,
                    entry.scope.key()
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let mut stored_details = entry.details.clone();
        if let (ElementKind::McpServer, Some((_, _, _, previous))) = (entry.kind, &existing)
            && let Ok(previous) = serde_json::from_str::<Value>(previous)
        {
            keep_secret_refs(&mut stored_details, &previous);
        }
        let details = serde_json::to_string(&stored_details)?;
        let result = match existing {
            None => {
                transaction.execute(
                    "INSERT INTO extensions
                        (kind, name, origin, scope, plugin, content_hash, content_dir, enabled, runs_code,
                         approved_hash, details, source_hash, held_for_secret)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                    params![
                        entry.kind.as_str(),
                        entry.name,
                        entry.origin.as_str(),
                        entry.scope.key(),
                        plugin,
                        hash,
                        dir.display().to_string(),
                        entry.enabled,
                        runs_code,
                        entry.approved_hash,
                        details,
                        source_hash,
                        held,
                    ],
                )?;
                (PutOutcome::Added, transaction.last_insert_rowid())
            }
            Some((id, previous, previous_source, previous_details))
                if previous == hash
                    && previous_source == source_hash
                    && previous_details == details =>
            {
                (PutOutcome::Unchanged, id)
            }
            Some((id, _, _, _)) => {
                // The enabled flag and the approval stay. An approval names the old hash, so it no longer matches
                // code that changed, and the new code waits for a new approval.
                transaction.execute(
                    "UPDATE extensions SET content_hash = ?1, content_dir = ?2, runs_code = ?3, details = ?4,
                        source_hash = ?5
                     WHERE id = ?6",
                    params![hash, dir.display().to_string(), runs_code, details, source_hash, id],
                )?;
                (PutOutcome::Changed, id)
            }
        };
        transaction.commit()?;
        Ok(result)
    }

    /// Removes the skills, commands and MCP servers of a plugin that `present` does not name. The plugin's folder no
    /// longer has them, so keeping their rows would leave entries with nothing to read.
    pub fn remove_missing(
        &self,
        origin: Origin,
        scope: Scope,
        plugin: &str,
        present: &[(ElementKind, String)],
    ) -> Result<usize, CatalogError> {
        let (entries, _) = self.scan()?;
        let connection = self.connection();
        let mut removed = 0;
        for entry in entries.iter().filter(|entry| {
            entry.origin == origin
                && entry.scope == scope
                && entry.plugin.as_deref() == Some(plugin)
                && matches!(
                    entry.kind,
                    ElementKind::Skill | ElementKind::Command | ElementKind::McpServer
                )
        }) {
            let kept = present
                .iter()
                .any(|(kind, name)| *kind == entry.kind && *name == entry.name);
            if !kept {
                removed +=
                    connection.execute("DELETE FROM extensions WHERE id = ?1", [entry.id])?;
            }
        }
        Ok(removed)
    }

    /// Removes a standalone row, retaining content-addressed copies for rollback.
    /// No source directories or shared plugin files are deleted.
    pub fn remove_entry(&self, id: i64) -> Result<(), CatalogError> {
        let entry = self.entry(id)?;
        if entry.plugin.is_some() || entry.kind == ElementKind::Plugin {
            return Err(CatalogError::Invalid("Manage plugin components through their parent plugin.".into()));
        }
        self.connection().execute("DELETE FROM extensions WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Switches an entry on or off, as the user does. A switch clears the mark of a hold, so the import knows the server
    /// is off by the user's choice and does not turn it back on.
    pub fn set_enabled(&self, id: i64, enabled: bool) -> Result<(), CatalogError> {
        if enabled
            && self.list()?.iter().any(|entry| {
                entry.id == id
                    && entry.kind == ElementKind::Memory
                    && entry.scope == Scope::Global
                    && entry
                        .details
                        .get("native_project")
                        .and_then(Value::as_str)
                        .is_some()
            })
        {
            return Err(CatalogError::Invalid(
                "Link this memory to a project before enabling it.".into(),
            ));
        }
        self.update(
            "UPDATE extensions SET enabled = ?1, held_for_secret = 0 WHERE id = ?2",
            params![enabled, id],
            id,
        )
    }

    /// Links retained, unassigned memory to the project the user selected.
    pub fn set_project(&self, id: i64, project_id: i64) -> Result<(), CatalogError> {
        let entry = self
            .list()?
            .into_iter()
            .find(|entry| entry.id == id)
            .ok_or(CatalogError::UnknownEntry(id))?;
        if entry.kind != ElementKind::Memory || entry.scope != Scope::Global {
            return Err(CatalogError::Invalid(
                "Only unassigned memory can be linked.".into(),
            ));
        }
        self.update(
            "UPDATE extensions SET scope=?1,enabled=1 WHERE id=?2",
            params![Scope::Project(project_id).key(), id],
            id,
        )
    }

    /// Approves the code of an entry. `expected_hash` is the content the user looked at; when the entry has another
    /// content now, the approval is refused, so the user never approves code they did not see.
    pub fn approve(&self, id: i64, expected_hash: &str) -> Result<(), CatalogError> {
        let connection = self.connection();
        let current: Option<String> = connection
            .query_row(
                "SELECT content_hash FROM extensions WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        match current {
            None => Err(CatalogError::UnknownEntry(id)),
            Some(hash) if hash != expected_hash => Err(CatalogError::Invalid(
                "this extension changed since you looked at it; look at it again before approving"
                    .into(),
            )),
            Some(_) => {
                connection.execute(
                    "UPDATE extensions SET approved_hash = ?1 WHERE id = ?2",
                    params![expected_hash, id],
                )?;
                Ok(())
            }
        }
    }

    fn update(&self, sql: &str, params: &[&dyn ToSql], id: i64) -> Result<(), CatalogError> {
        match self.connection().execute(sql, params)? {
            0 => Err(CatalogError::UnknownEntry(id)),
            _ => Ok(()),
        }
    }

    /// Whether an entry may run its code: it has none, or the approval names its current content.
    pub fn is_approved(&self, entry: &CatalogEntry) -> bool {
        entry.runs_code.is_empty()
            || entry.approved_hash.as_deref() == Some(entry.content_hash.as_str())
    }

    /// Stores a secret for an MCP server entry in `secrets`, under a key of the entry, and points the server at it.
    /// The value itself never goes into the catalog.
    pub fn set_secret(
        &self,
        id: i64,
        key: &str,
        value: &str,
        secrets: &dyn SecretStore,
    ) -> Result<(), CatalogError> {
        let entry = self.entry(id)?;
        if entry.kind != ElementKind::McpServer {
            return Err(CatalogError::Invalid(format!(
                "extension {id} isn't an MCP server"
            )));
        }
        if key.trim().is_empty() {
            return Err(CatalogError::Invalid("a secret needs a name".into()));
        }
        let stored = format!("extension:{id}:{key}");
        secrets.set(&stored, value)?;
        let mut server: McpServerConfig = serde_json::from_value(entry.details)?;
        server
            .secrets
            .insert(key.to_string(), SecretRef::Keychain(stored));
        self.connection().execute(
            "UPDATE extensions SET details = ?1 WHERE id = ?2",
            params![serde_json::to_string(&server)?, id],
        )?;
        if self.held_for_secret(id)?
            && server
                .env
                .iter()
                .chain(server.headers.iter())
                .all(|(key, value)| !value.contains("${") || server.secrets.contains_key(key))
        {
            self.release(id)?;
        }
        Ok(())
    }

    /// Counts one more run of a skill or command, by the name the person types.
    pub fn record_use(&self, name: &str) -> Result<(), CatalogError> {
        self.connection().execute(
            "INSERT INTO extension_uses (name, uses) VALUES (?1, 1)
             ON CONFLICT (name) DO UPDATE SET uses = uses + 1",
            [name],
        )?;
        Ok(())
    }

    /// The run counts of every skill and command.
    pub fn usage(&self) -> Result<UsageCounts, CatalogError> {
        let connection = self.connection();
        let mut statement = connection.prepare("SELECT name, uses FROM extension_uses")?;
        let pairs = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<(String, i64)>>>()?;
        Ok(UsageCounts::from_pairs(
            pairs
                .into_iter()
                .map(|(name, uses)| (name, uses.max(0) as u64)),
        ))
    }

    /// The folder for the data a plugin keeps across its versions. It is named by the id of the plugin's entry.
    pub fn plugin_data_dir(&self, id: i64) -> PathBuf {
        self.data_dir.join("extensions-data").join(id.to_string())
    }

    /// The extensions that apply to `project_id`: the enabled ones of every scope and of that project, with code
    /// only when it is approved and its stored copy still hashes to what was approved. Anything left out for another
    /// reason is named in `problems`.
    pub fn active_for(&self, project_id: i64) -> Result<ActiveExtensions, CatalogError> {
        let (entries, unreadable) = self.scan()?;
        let mut active = ActiveExtensions {
            problems: unreadable,
            ..Default::default()
        };
        for entry in entries
            .iter()
            .filter(|entry| entry.approved_hash.as_deref() == Some(entry.content_hash.as_str()))
        {
            active
                .approved
                .approvals
                .insert(entry.id, entry.content_hash.clone());
        }
        let mut bundles: HashMap<PathBuf, Result<Vec<Skill>, String>> = HashMap::new();
        for entry in &entries {
            let applies = entry.enabled
                && match entry.scope {
                    Scope::Global => true,
                    Scope::Project(id) => id == project_id,
                };
            if !applies || plugin_is_off(entry, &entries) || !self.is_approved(entry) {
                continue;
            }
            if !entry.runs_code.is_empty()
                && content_hash(&entry.content_dir).ok().as_deref()
                    != Some(entry.content_hash.as_str())
            {
                active.problems.push(format!(
                    "{}: the stored copy no longer matches its hash, so its code stays off; add the folder again",
                    entry.name
                ));
                continue;
            }
            match entry.kind {
                ElementKind::Plugin => {}
                ElementKind::Skill | ElementKind::Command => {
                    match load_skill(entry, &mut bundles) {
                        Ok(mut skill) => {
                            if let Some(plugin) = &entry.plugin {
                                skill.plugin_root = Some(entry.content_dir.clone());
                                skill.plugin_data =
                                    Some(self.plugin_data_dir(plugin_id(entry, plugin, &entries)));
                            }
                            active.skills.push(skill);
                        }
                        Err(problem) => active.problems.push(problem),
                    }
                }
                ElementKind::McpServer => match self.load_server(entry, &entries) {
                    Ok(server) => active.mcp_servers.push(server),
                    Err(problem) => active.problems.push(problem),
                },
                ElementKind::Permission => match permission_rules(&entry.details) {
                    Ok(rules) => {
                        active.rules.allow.extend(rules.allow);
                        active.rules.ask.extend(rules.ask);
                        active.rules.deny.extend(rules.deny);
                    }
                    Err(problem) => active.problems.push(format!("{}: {problem}", entry.name)),
                },
                ElementKind::Instructions => active.instructions.push(entry_path(entry)),
                ElementKind::Memory => active.memory.push(entry_path(entry)),
            }
        }
        Ok(active)
    }

    fn load_server(
        &self,
        entry: &CatalogEntry,
        entries: &[CatalogEntry],
    ) -> Result<McpServerConfig, String> {
        let mut server: McpServerConfig = serde_json::from_value(entry.details.clone())
            .map_err(|error| format!("{}: {error}", entry.name))?;
        if let Some(plugin) = &entry.plugin {
            server.plugin_context = Some(PluginContext {
                root: entry.content_dir.clone(),
                data: self.plugin_data_dir(plugin_id(entry, plugin, entries)),
                ..Default::default()
            });
        }
        Ok(server)
    }

    /// Removes the stored copies that no row uses any more. The service runs this when it starts. Every row counts,
    /// including one that can't be read, so its copy is never removed.
    pub fn prune(&self) -> Result<usize, CatalogError> {
        let live: HashSet<(String, String, String)> = self
            .stored_dirs()?
            .iter()
            .filter_map(|dir| version_key(dir))
            .collect();
        let mut removed = 0;
        for origin in subfolders(&self.data_dir.join("extensions"))? {
            for name in subfolders(&origin)? {
                for version in subfolders(&name)? {
                    let Some(key) = version_key(&version) else {
                        continue;
                    };
                    if !live.contains(&key) {
                        fs::remove_dir_all(&version)?;
                        removed += 1;
                    }
                }
            }
        }
        Ok(removed)
    }

    /// The content folders of every row, read straight from the table.
    fn stored_dirs(&self) -> Result<Vec<PathBuf>, CatalogError> {
        let connection = self.connection();
        let mut statement = connection.prepare("SELECT content_dir FROM extensions")?;
        let dirs = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(dirs.into_iter().map(PathBuf::from).collect())
    }
}

/// The row of the extensions table, before its columns are parsed.
struct Row {
    id: i64,
    kind: String,
    name: String,
    origin: String,
    scope: String,
    plugin: String,
    content_hash: String,
    content_dir: String,
    enabled: bool,
    runs_code: String,
    approved_hash: Option<String>,
    details: String,
}

impl Row {
    fn into_entry(self) -> Result<CatalogEntry, CatalogError> {
        Ok(CatalogEntry {
            id: self.id,
            kind: ElementKind::parse(&self.kind).ok_or_else(|| corrupt(&self.kind))?,
            name: self.name,
            origin: Origin::parse(&self.origin).ok_or_else(|| corrupt(&self.origin))?,
            scope: Scope::parse(&self.scope).ok_or_else(|| corrupt(&self.scope))?,
            content_hash: self.content_hash,
            content_dir: PathBuf::from(self.content_dir),
            enabled: self.enabled,
            runs_code: serde_json::from_str(&self.runs_code)?,
            approved_hash: self.approved_hash,
            plugin: (!self.plugin.is_empty()).then_some(self.plugin),
            details: serde_json::from_str(&self.details)?,
        })
    }
}

/// Adds the columns that a database made by an earlier version lacks. A row from before `source_hash` existed has
/// its content hash as its source, since the copy was made from the source as it was.
fn migrate(connection: &Connection) -> Result<(), CatalogError> {
    if !has_column(connection, "source_hash")? {
        connection.execute_batch(
            "ALTER TABLE extensions ADD COLUMN source_hash TEXT NOT NULL DEFAULT '';
             UPDATE extensions SET source_hash = content_hash;",
        )?;
    }
    if !has_column(connection, "held_for_secret")? {
        connection.execute_batch(
            "ALTER TABLE extensions ADD COLUMN held_for_secret INTEGER NOT NULL DEFAULT 0;",
        )?;
    }
    Ok(())
}

/// Whether the extensions table has a column.
fn has_column(connection: &Connection, column: &str) -> Result<bool, CatalogError> {
    let mut statement = connection.prepare("PRAGMA table_info(extensions)")?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(names.iter().any(|name| name == column))
}

fn corrupt(text: &str) -> CatalogError {
    CatalogError::Corrupt(format!("unknown value `{text}`"))
}

fn plugin_entry(bundle: &crate::PluginBundle, origin: Origin, scope: Scope) -> CatalogEntry {
    let unsupported: Vec<Value> = bundle
        .unsupported
        .iter()
        .map(|part| json!({ "kind": part.kind, "reason": part.reason }))
        .collect();
    CatalogEntry {
        id: 0,
        kind: ElementKind::Plugin,
        name: bundle.name.clone(),
        origin,
        scope,
        content_hash: String::new(),
        content_dir: PathBuf::new(),
        enabled: true,
        runs_code: Vec::new(),
        approved_hash: None,
        plugin: None,
        details: json!({
            "description": bundle.description,
            "version": bundle.version,
            "author": bundle.author,
            "unsupported": unsupported,
        }),
    }
}

fn skill_entry(
    skill: &Skill,
    origin: Origin,
    scope: Scope,
    plugin: Option<String>,
) -> CatalogEntry {
    CatalogEntry {
        id: 0,
        kind: if skill.is_command {
            ElementKind::Command
        } else {
            ElementKind::Skill
        },
        name: skill.name.clone(),
        origin,
        scope,
        content_hash: String::new(),
        content_dir: PathBuf::new(),
        enabled: true,
        runs_code: skill_commands(&skill.body),
        approved_hash: None,
        plugin,
        details: json!({ "description": skill.description }),
    }
}

fn server_entry(
    server: &McpServerConfig,
    origin: Origin,
    scope: Scope,
    plugin: Option<String>,
) -> Result<CatalogEntry, CatalogError> {
    let mut runs_code = Vec::new();
    if let Some(command) = &server.command {
        let mut line = command.clone();
        for arg in &server.args {
            line.push(' ');
            line.push_str(arg);
        }
        runs_code.push(RunsCode {
            what: "MCP server".into(),
            command: line,
        });
    }
    if let Some(helper) = &server.headers_helper {
        runs_code.push(RunsCode {
            what: "MCP headers helper".into(),
            command: helper.clone(),
        });
    }
    Ok(CatalogEntry {
        id: 0,
        kind: ElementKind::McpServer,
        name: server.name.clone(),
        origin,
        scope,
        content_hash: String::new(),
        content_dir: PathBuf::new(),
        enabled: true,
        runs_code,
        approved_hash: None,
        plugin,
        details: serde_json::to_value(server)?,
    })
}

/// The id of the plugin entry that owns a part, so its data folder is the plugin's. A part whose plugin has no
/// entry keeps its own id.
fn plugin_id(entry: &CatalogEntry, plugin: &str, entries: &[CatalogEntry]) -> i64 {
    entries
        .iter()
        .find(|candidate| {
            candidate.kind == ElementKind::Plugin
                && candidate.origin == entry.origin
                && candidate.scope == entry.scope
                && candidate.name == plugin
        })
        .map_or(entry.id, |owner| owner.id)
}

/// The commands a skill body runs, as the runtime finds them with `bang_spans`.
fn skill_commands(body: &str) -> Vec<RunsCode> {
    bang_spans(body)
        .into_iter()
        .map(|span| RunsCode {
            what: match span.form {
                BangForm::Inline => "inline command".into(),
                BangForm::Fenced => "fenced command".into(),
            },
            command: span.command,
        })
        .collect()
}

/// The skill of an entry, read from its stored copy. A plugin's skills are read from the plugin's folder, and a
/// standalone skill from its own folder, whose name is the hash, so the entry's name is kept.
fn load_skill(
    entry: &CatalogEntry,
    bundles: &mut HashMap<PathBuf, Result<Vec<Skill>, String>>,
) -> Result<Skill, String> {
    let wanted_command = entry.kind == ElementKind::Command;
    let mut skill = match &entry.plugin {
        Some(_) => {
            let skills = bundles.entry(entry.content_dir.clone()).or_insert_with(|| {
                read_plugin(&entry.content_dir)
                    .map(|bundle| bundle.skills)
                    .map_err(|error| error.to_string())
            });
            let skills = skills
                .as_ref()
                .map_err(|error| format!("{}: {error}", entry.name))?;
            skills
                .iter()
                .find(|skill| skill.name == entry.name && skill.is_command == wanted_command)
                .cloned()
                .ok_or_else(|| format!("{} isn't in its stored copy any more", entry.name))?
        }
        None => {
            let mut skill = parse_skill(&entry.content_dir)
                .map_err(|error| format!("{}: {error}", entry.name))?;
            skill.name = entry.name.clone();
            skill.is_command = wanted_command;
            skill
        }
    };
    skill.plugin = entry.plugin.clone();
    Ok(skill)
}

/// A server's keychain references, and the `${KEY}` settings that read them, are stored state and not the source's. A
/// new copy of the server keeps them, so a reimport that doesn't tick a secret again leaves the stored value in place.
fn keep_secret_refs(details: &mut Value, previous: &Value) {
    // Stored credentials belong to the reviewed destination. Reimporting a new
    // command or URL must not silently send old credentials somewhere else.
    if ["url","command","args","cwd","headers_helper"].iter().any(|key|details.get(*key)!=previous.get(*key)) {
        return;
    }
    let Some(stored) = previous.get("secrets").and_then(Value::as_object) else {
        return;
    };
    for (key, reference) in stored {
        let placeholder = format!("${{{key}}}");
        let Some(object) = details.as_object_mut() else {
            return;
        };
        let secrets = object.entry("secrets").or_insert_with(|| json!({}));
        if let Some(secrets) = secrets.as_object_mut() {
            secrets
                .entry(key.clone())
                .or_insert_with(|| reference.clone());
        }
        for field in ["env", "headers"] {
            let kept = previous
                .get(field)
                .and_then(|map| map.get(key))
                .and_then(Value::as_str);
            if kept == Some(placeholder.as_str()) {
                let map = object.entry(field).or_insert_with(|| json!({}));
                if let Some(map) = map.as_object_mut() {
                    map.entry(key.clone())
                        .or_insert_with(|| Value::String(placeholder.clone()));
                }
            }
        }
    }
}

/// The rules of a permission entry: its `allow`, `ask` and `deny` lists, each rule read as settings write it.
fn permission_rules(details: &Value) -> Result<Rules, String> {
    let list = |key: &str| -> Result<Vec<Rule>, String> {
        details
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|text| match text.as_str() {
                Some(text) => Rule::parse(text),
                None => Err(format!("a rule in {key} isn't text")),
            })
            .collect()
    };
    Ok(Rules {
        allow: list("allow")?,
        ask: list("ask")?,
        deny: list("deny")?,
    })
}

/// Instructions and memory are files in the stored copy. Their details name the file, or the folder when they don't.
fn entry_path(entry: &CatalogEntry) -> PathBuf {
    match entry.details.get("path").and_then(Value::as_str) {
        Some(path) => entry.content_dir.join(path),
        None => entry.content_dir.clone(),
    }
}

/// Whether the plugin an entry belongs to is switched off. Its parts are then left out.
fn plugin_is_off(entry: &CatalogEntry, entries: &[CatalogEntry]) -> bool {
    entry.plugin.as_ref().is_some_and(|plugin| {
        entries.iter().any(|candidate| {
            candidate.kind == ElementKind::Plugin
                && candidate.origin == entry.origin
                && candidate.scope == entry.scope
                && candidate.name == *plugin
                && !candidate.enabled
        })
    })
}

fn literal_problem(secret: &LiteralSecret) -> String {
    format!(
        "the literal `{}` of MCP server `{}` isn't kept; set it in Vatra",
        secret.key, secret.server
    )
}

fn copy_tree(source: &Path, target: &Path) -> io::Result<()> {
    fs::create_dir_all(target)?;
    for relative in files(source)? {
        let to = target.join(&relative);
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source.join(&relative), to)?;
    }
    Ok(())
}

/// A name for a staging folder, unique across the processes and the moments of this machine.
fn unique_name() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_nanos());
    format!("{}-{nanos}", std::process::id())
}

/// A folder name that is safe on disk, whatever the plugin or skill is called.
fn folder_name(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe.is_empty() || safe.starts_with('.') {
        format!("_{safe}")
    } else {
        safe
    }
}

/// The origin, name and hash folders of a stored copy. Paths are compared by these names, so a data folder given
/// in another form still finds its copies.
fn version_key(dir: &Path) -> Option<(String, String, String)> {
    let hash = dir.file_name()?.to_string_lossy().into_owned();
    let name = dir.parent()?.file_name()?.to_string_lossy().into_owned();
    let origin = dir
        .parent()?
        .parent()?
        .file_name()?
        .to_string_lossy()
        .into_owned();
    Some((origin, name, hash))
}

fn subfolders(dir: &Path) -> io::Result<Vec<PathBuf>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut found = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            found.push(path);
        }
    }
    Ok(found)
}
