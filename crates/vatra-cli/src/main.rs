mod models;
mod parity;

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use async_trait::async_trait;
use clap::{Parser, Subcommand};
use serde_json::Value;
use vatra_cli::config::Config;
use vatra_core::{
    Agent, KeyringBackend, KeyringStore, SecretStore, Store, StoredEvent, handoff, run_turn,
};
use vatra_engine::{AllowAll, Approver, NoQuestions};
use vatra_extensions::{Catalog, Origin, PutOutcome, Scope};
use vatra_import::{
    GuardedFs, ImportChoice, ItemChoice, ScanView, Status, SystemProbe, apply, approvals, plan,
    probe_items, scan_view,
};
use vatra_service::{
    AgentFactory, BuildRequest, ConfigAgents, PermissionMode, Runtime, default_data_dir,
    project_folders, run_bridge_scoped, run_mcp_proxy, scan_folders,
};

#[derive(Parser)]
#[command(name = "vatra", about = "One place for all your AI agents")]
struct Cli {
    #[arg(long, global = true, default_value = "vatra.toml")]
    config: PathBuf,
    /// The database. Without it, the app's database in `%APPDATA%\Vatra`.
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the fixed tasks or export their measured results.
    Parity {
        #[command(subcommand)]
        command: ParityCommand,
    },
    /// Show the CPU, memory, GPUs and the llama.cpp this computer has.
    Hardware {
        #[arg(long)]
        json: bool,
    },
    /// Find and inspect local models.
    Models {
        #[command(subcommand)]
        command: ModelsCommand,
    },
    /// The MCP server the model check calls.
    #[command(hide = true)]
    McpProbe,
    #[command(hide = true)]
    FixtureMcp,
    /// Installer-only helper: no process kill, no implicit service start.
    #[command(hide = true)]
    ServiceShutdownOwned { #[arg(long)] data_dir:PathBuf, #[arg(long)] expected_exe:PathBuf },
    #[command(hide = true)]
    ServiceCheckUnlocked { #[arg(long)] exe:PathBuf },
    /// Permission hook for a disposable native task; never touches native user configuration.
    #[command(hide = true)]
    ParityGuard { #[arg(long)] policy:PathBuf },
    /// Internal SIWC launch bridge used by the unmodified ACP adapter.
    #[command(hide = true)]
    AppServer {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// MCP transport for a server whose OAuth login stays in Vatra.
    #[command(hide = true)]
    McpProxy {
        #[arg(long)]
        server: String,
        #[arg(long)]
        url: String,
        #[arg(long)]
        token_file: PathBuf,
        #[arg(long)]
        project: i64,
        #[arg(long)]
        snapshot: Option<String>,
    },
    /// Run a turn in a new or existing session.
    Run {
        #[arg(long)]
        project: String,
        #[arg(long)]
        agent: String,
        #[arg(long)]
        session: Option<i64>,
        /// Approve catalog tools automatically.
        #[arg(long)]
        yes: bool,
        prompt: String,
    },
    /// Hand a session's context to another agent.
    Handoff {
        #[arg(long)]
        from: i64,
        #[arg(long)]
        to: String,
        #[arg(long)]
        yes: bool,
        task: String,
    },
    /// Show a session's events.
    Show { session: i64 },
    /// Start the local service the desktop app uses.
    Serve {
        /// Default: %APPDATA%\\Vatra.
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// Plugins and skills in the extension catalog.
    Extensions {
        #[command(subcommand)]
        command: ExtensionsCommand,
    },
    /// Bring in what Claude Code and Codex keep on this machine. Without `--dry-run`, the items are copied with their
    /// code off and their secrets not stored.
    Import {
        /// List what would be imported, and write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Include past conversation text in Vatra's searchable history.
        #[arg(long)]
        history: bool,
    },
    /// MCP server Claude Code uses for approvals. Started by the service.
    #[command(hide = true)]
    ApprovalBridge {
        #[arg(long)]
        url: String,
        #[arg(long)]
        token_file: PathBuf,
        #[arg(long)]
        session: i64,
        #[arg(long)]
        workspace_scope: Option<String>,
    },
}

#[derive(Subcommand)]
enum ParityCommand {
    Run { #[arg(long)] agent:String, #[arg(long)] task:Option<String>, #[arg(long)] consent_cost:bool, #[arg(long, default_value_t = 1)] runs:u32 },
    Export { #[arg(long)] agent:Option<String>, #[arg(long)] output:Option<PathBuf> },
    Native { #[arg(long)] kind:String, #[arg(long)] model:String, #[arg(long)] task:Option<String>, #[arg(long)] consent_cost:bool },
}

#[derive(Subcommand)]
enum ModelsCommand {
    /// Read a model's header from Hugging Face, without downloading it, and estimate it for this computer.
    Inspect {
        /// Like `unsloth/Qwen3-4B-Instruct-2507-GGUF`.
        repo: String,
        /// Like `Q4_K_M`.
        variant: String,
        /// The context to estimate for. Defaults to 32768, or less when the model supports less.
        #[arg(long)]
        ctx: Option<u32>,
        #[arg(long, hide = true)]
        hf_url: Option<String>,
    },
}

/// The `[local]` section of the configuration, or its defaults when the file is missing or invalid.
fn local_config(path: &std::path::Path) -> vatra_service::config::LocalConfig {
    vatra_service::config::Config::load(path).map(|config| config.local).unwrap_or_default()
}

#[derive(Subcommand)]
enum ExtensionsCommand {
    /// Add a plugin or skill folder, as added by hand.
    Add {
        folder: PathBuf,
        /// Approve the code the folder runs. Without it, the code stays off.
        #[arg(long)]
        approve: bool,
    },
}

struct TerminalApprover;

#[async_trait]
impl Approver for TerminalApprover {
    async fn approve(&self, name: &str, arguments: &Value) -> bool {
        if !std::io::stdin().is_terminal() {
            return false;
        }
        let question = format!("Allow {name} with {arguments}? [y/N] ");
        tokio::task::spawn_blocking(move || {
            eprint!("{question}");
            let mut answer = String::new();
            std::io::stdin().read_line(&mut answer).is_ok()
                && answer.trim().eq_ignore_ascii_case("y")
        })
        .await
        .unwrap_or(false)
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match execute(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

async fn execute(cli: Cli) -> Result<(), String> {
    let Cli {
        config,
        db,
        command,
    } = cli;
    match command {
        Command::ServiceCheckUnlocked { exe } => vatra_cli::service_control::ensure_executable_unlocked(&exe),
        Command::ServiceShutdownOwned { data_dir, expected_exe } => {
            let outcome=vatra_cli::service_control::shutdown_owned_service(&data_dir,&expected_exe).await?;
            // The installer runs this from a helper copy, but the app runs it from the
            // installed vatra.exe itself, which can never open its own image for writing.
            let is_self=match (std::env::current_exe(),std::fs::canonicalize(&expected_exe)) {
                (Ok(running),Ok(expected)) => match std::fs::canonicalize(&running) {
                    Ok(running)=>running.to_string_lossy().eq_ignore_ascii_case(&expected.to_string_lossy()),
                    Err(_)=>false,
                },
                _=>false,
            };
            if !is_self { vatra_cli::service_control::ensure_executable_unlocked(&expected_exe)?; }
            println!("{outcome:?}");Ok(())
        }
        Command::Parity { command } => {
            let (db_path,data_dir)=database(db)?;
            if db_path != data_dir.join("vatra.db") { return Err("Parity uses the service database; --db must name vatra.db in its data directory".into()); }
            let (method,params,output,runs)=match command {
                ParityCommand::Run { agent,task,consent_cost,runs } => {
                    if !(1..=10).contains(&runs) { return Err(format!("--runs must be from 1 to 10, not {runs}")); }
                    ("parity.run",serde_json::json!({"agent":agent,"task":task,"consentCost":consent_cost}),None,runs)
                }
                ParityCommand::Native { kind,model,task,consent_cost } => ("parity.native",serde_json::json!({"kind":kind,"model":model,"task":task,"consentCost":consent_cost}),None,1),
                ParityCommand::Export { agent,output } => ("parity.export",serde_json::json!({"agent":agent}),output,1),
            };
            let results=parity::call(&data_dir,&config,method,params,runs).await?;
            if method=="parity.export" {
                let markdown=results[0].as_str().ok_or("Invalid export response")?;
                if let Some(path)=output { if let Some(parent)=path.parent().filter(|p|!p.as_os_str().is_empty()) { std::fs::create_dir_all(parent).map_err(text)?; } std::fs::write(path,markdown).map_err(text)?; }
                else { println!("{markdown}"); }
            } else if runs==1 { println!("{}",serde_json::to_string_pretty(&results[0]).map_err(text)?); }
            else { println!("{}",serde_json::to_string_pretty(&results).map_err(text)?); }
            Ok(())
        }
        Command::AppServer { args } => vatra_service::chatgpt::run_app_server(&args).await,
        Command::McpProbe => vatra_service::models::probe::serve().await,
        Command::FixtureMcp => vatra_service::models::probe::serve_fixture().await,
        Command::ParityGuard { policy } => {
            use std::io::Read;
            #[derive(serde::Deserialize)] struct Policy { root:PathBuf,commands:Vec<String> }
            let policy:Policy=serde_json::from_slice(&std::fs::read(policy).map_err(text)?).map_err(text)?;
            let mut input=String::new();
            std::io::stdin().take(262145).read_to_string(&mut input).map_err(text)?;
            if input.len()>262144 { return Err("Hook input too large".into()); }
            let input:Value=serde_json::from_str(&input).map_err(text)?;
            let approver=vatra_service::parity::TaskApprover::new(&policy.root,policy.commands);
            let allowed=approver.approve(input["tool_name"].as_str().unwrap_or(""),&input["tool_input"]).await;
            println!("{}",serde_json::json!({"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":if allowed {"allow"} else {"deny"},"permissionDecisionReason":"Fixed task permission policy"}}));
            Ok(())
        }
        Command::Hardware { json } => {
            let local = local_config(&config);
            models::print_hardware(local.llama_server.as_deref(), json)
        }
        Command::Models {
            command: ModelsCommand::Inspect { repo, variant, ctx, hf_url },
        } => {
            let local = local_config(&config);
            models::inspect(&repo, &variant, ctx, hf_url.as_deref(), local.llama_server.as_deref()).await
        }
        Command::Serve { data_dir } => serve(data_dir).await,
        Command::Extensions {
            command: ExtensionsCommand::Add { folder, approve },
        } => add_extension(db, &folder, approve),
        Command::ApprovalBridge {
            url,
            token_file,
            session,
            workspace_scope,
        } => run_bridge_scoped(&url, &token_file, session, workspace_scope).await,
        Command::McpProxy {
            server,
            url,
            token_file,
            project,
            snapshot,
        } => run_mcp_proxy(&server, &url, &token_file, project, snapshot.as_deref()).await,
        Command::Show { session } => {
            let (db, _) = database(db)?;
            let store = open_store(&db)?;
            for stored in store.events(session).map_err(text)? {
                print_event(&stored);
            }
            Ok(())
        }
        Command::Run {
            project,
            agent,
            session,
            yes,
            prompt,
        } => {
            let (db, data_dir) = database(db)?;
            let catalog = open_catalog(&db, &data_dir)?;
            let store = Arc::new(open_store(&db)?);
            let agents = ConfigAgents::new(Config::load(&config).map_err(text)?)
                .with_catalog(catalog)
                .with_store(Arc::clone(&store));
            let project_id = store.create_project(&project).map_err(text)?;
            let session_id = match session {
                Some(id) => {
                    check_session(&store, id, project_id, &agent)?;
                    id
                }
                None => {
                    check_agent(&agents, &agent)?;
                    store.create_session(project_id, &agent).map_err(text)?
                }
            };
            let agent = build_agent(&agents, &agent, session_id, project_id, yes).await?;
            println!("session: {session_id}");
            run_turn(
                &store,
                agent.as_ref(),
                session_id,
                &prompt,
                &mut print_event,
            )
            .await
            .map_err(text)
        }
        Command::Handoff {
            from,
            to,
            yes,
            task,
        } => {
            let (db, data_dir) = database(db)?;
            let catalog = open_catalog(&db, &data_dir)?;
            let store = Arc::new(open_store(&db)?);
            let agents = ConfigAgents::new(Config::load(&config).map_err(text)?)
                .with_catalog(catalog)
                .with_store(Arc::clone(&store));
            let project_id = store.session_project(from).map_err(text)?.unwrap_or(0);
            let target = build_agent(&agents, &to, from, project_id, yes).await?;
            let summarizer = agents.summarizer();
            let session = handoff(
                &store,
                from,
                target.as_ref(),
                summarizer.as_ref(),
                &task,
                &mut print_event,
            )
            .await
            .map_err(text)?;
            println!("session: {session}");
            Ok(())
        }
        Command::Import { dry_run, history } => {
            let (db, data_dir) = database(db)?;
            import(&db, &data_dir, dry_run, history).await
        }
    }
}

/// The database a command uses, and the folder of its data. Without `--db` it is the app's, so the CLI and the app
/// share one database. A `vatra.db` in the current folder is then not used, and a warning says so.
fn database(db: Option<PathBuf>) -> Result<(PathBuf, PathBuf), String> {
    match db {
        Some(db) => {
            let data_dir = match db.parent() {
                Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
                _ => PathBuf::from("."),
            };
            Ok((db, data_dir))
        }
        None => {
            let data_dir = default_data_dir()?;
            let db = data_dir.join("vatra.db");
            let local = Path::new("vatra.db");
            if local.is_file() && local.canonicalize().ok() != db.canonicalize().ok() {
                eprintln!(
                    "warning: vatra.db in this folder isn't used; pass --db vatra.db to use it"
                );
            }
            Ok((db, data_dir))
        }
    }
}

/// Adds a folder to the catalog as added by hand. The catalog is the one of `database`.
fn add_extension(db: Option<PathBuf>, folder: &Path, approve: bool) -> Result<(), String> {
    let (db, data_dir) = database(db)?;
    let catalog = open_catalog(&db, &data_dir)?;
    println!("catalog: {}", db.display());
    let report = catalog
        .add_folder(folder, Origin::Manual, Scope::Global, approve)
        .map_err(text)?;
    for problem in &report.problems {
        eprintln!("warning: {problem}");
    }
    let count = |wanted: PutOutcome| {
        report
            .outcomes
            .iter()
            .filter(|outcome| **outcome == wanted)
            .count()
    };
    println!(
        "{} entries: {} added, {} changed, {} unchanged",
        report.outcomes.len(),
        count(PutOutcome::Added),
        count(PutOutcome::Changed),
        count(PutOutcome::Unchanged),
    );
    if !report.unapproved.is_empty() {
        println!(
            "code not approved, so it stays off: {}",
            report.unapproved.join(", ")
        );
        println!("add --approve to allow it");
    }
    Ok(())
}

/// Lists what an import would bring and, unless `dry_run`, copies the default choice into the catalog: every item,
/// with its code off, its secrets not stored and its allow rules left out. A dry run writes nothing, so without a
/// database it does not create one.
async fn import(db: &Path, data_dir: &Path, dry_run: bool, history: bool) -> Result<(), String> {
    // A dry run opens an existing database read only, so it changes no byte of it; without one, it opens nothing.
    let (store, catalog) = match (dry_run, db.is_file()) {
        (true, false) => (None, None),
        (true, true) => (
            Some(Store::open_read_only(db).map_err(text)?),
            Some(
                Catalog::open_read_only(db, data_dir)
                    .map(Arc::new)
                    .map_err(text)?,
            ),
        ),
        (false, _) => (Some(open_store(db)?), Some(open_catalog(db, data_dir)?)),
    };
    println!("database: {}", db.display());
    let dirs = vatra_import::SourceDirs::detect();
    let folders = match &store {
        Some(store) => project_folders(store, &dirs)?,
        None => Vec::new(),
    };
    let (scan, projects) = scan_folders(&GuardedFs::new(), &dirs, &folders);
    let planned = plan(&scan);
    let mut probes = probe_items(&SystemProbe::new(), &planned.items).await;
    probes.approved = approvals(&planned, &projects, catalog.as_deref())?;
    let view = scan_view(&planned, &projects, catalog.as_deref(), &probes)?;
    print_view(&view);
    if dry_run {
        println!("dry run: nothing was written");
        return Ok(());
    }

    let catalog = catalog.ok_or("the catalog isn't available")?;
    let secrets: Arc<dyn SecretStore> = Arc::new(KeyringStore::new(KeyringBackend));
    let choice = ImportChoice {
        items: view
            .items
            .iter()
            .map(|item| ItemChoice {
                key: item.key.clone(),
                include: true,
                ..Default::default()
            })
            .collect(),
        projects,
        include_history: history,
        history_projects: vec![],
    };
    let outcome = apply(&scan, &choice, &catalog, secrets.as_ref());
    for result in &outcome.results {
        if let vatra_import::ItemState::Failed { message } = &result.state {
            eprintln!("warning: {} wasn't imported: {message}", result.name);
        }
    }
    let count = |wanted: fn(&vatra_import::ItemState) -> bool| {
        outcome
            .results
            .iter()
            .filter(|result| wanted(&result.state))
            .count()
    };
    println!(
        "imported: {} added, {} changed, {} unchanged, {} skipped, {} failed",
        count(|state| matches!(state, vatra_import::ItemState::Added)),
        count(|state| matches!(state, vatra_import::ItemState::Changed)),
        count(|state| matches!(state, vatra_import::ItemState::Unchanged)),
        count(|state| matches!(state, vatra_import::ItemState::Skipped { .. })),
        count(|state| matches!(state, vatra_import::ItemState::Failed { .. })),
    );
    println!("code stays off until you approve it, and secrets are not stored");
    if choice.include_history {
        let store = store.ok_or("the history database isn't available")?;
        let projects = store.list_projects().map_err(text)?;
        let cancelled = std::sync::atomic::AtomicBool::new(false);
        let mut imported = 0;
        let mut failed = 0;
        for (path, origin) in vatra_import::history_files(&dirs) {
            match vatra_import::import_log(
                &path,
                origin,
                &store,
                &projects,
                &cancelled,
                &mut |_, _| {},
            ) {
                Ok(_) => imported += 1,
                Err(error) => {
                    failed += 1;
                    eprintln!("warning: {} wasn't imported: {error}", path.display());
                }
            }
        }
        println!("past conversations: {imported} imported, {failed} failed");
    }
    Ok(())
}

/// The items of an import, one line each: its status in the catalog, where it comes from, and its name.
fn print_view(view: &ScanView) {
    for problem in &view.problems {
        eprintln!("warning: {}: {}", problem.source.display(), problem.message);
    }
    for item in &view.items {
        let status = match item.status {
            Some(Status::New) => "new",
            Some(Status::Changed) => "changed",
            Some(Status::Unchanged) => "same",
            None => "not kept",
        };
        let project = if item.project_known {
            ""
        } else {
            " (project unknown to Vatra)"
        };
        println!(
            "{status:<9} {:?} {:?} {}{project}",
            item.report.origin, item.report.kind, item.report.name
        );
    }
    println!("{} items", view.items.len());
}

async fn serve(data_dir: Option<PathBuf>) -> Result<(), String> {
    let data_dir = data_dir.map_or_else(default_data_dir, Ok)?;
    let config_path = data_dir.join("vatra.toml");
    let config = if config_path.exists() {
        Config::load(&config_path).map_err(text)?
    } else {
        Config::empty()
    };
    let service = Runtime::start(&data_dir, config).await.map_err(text)?;
    eprintln!("Vatra is running on {}", service.address());
    service.wait().await;
    Ok(())
}

fn text(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn open_store(path: &Path) -> Result<Store, String> {
    ensure_folder(path)?;
    Store::open(path).map_err(text)
}

fn open_catalog(db: &Path, data_dir: &Path) -> Result<Arc<Catalog>, String> {
    std::fs::create_dir_all(data_dir).map_err(text)?;
    Catalog::open(db, data_dir).map(Arc::new).map_err(text)
}

fn ensure_folder(db: &Path) -> Result<(), String> {
    match db.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => {
            std::fs::create_dir_all(parent).map_err(text)
        }
        _ => Ok(()),
    }
}

fn print_event(stored: &StoredEvent) {
    let line = serde_json::to_string(&stored.event)
        .unwrap_or_else(|error| format!("event can't be displayed: {error}"));
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{line}");
}

fn check_session(store: &Store, session: i64, project: i64, agent: &str) -> Result<(), String> {
    if store.session_project(session).map_err(text)? != Some(project) {
        return Err(format!("session {session} belongs to another project"));
    }
    let owner = store.session_agent(session).map_err(text)?;
    if owner != agent {
        return Err(format!(
            "session {session} belongs to agent `{owner}`; for `{agent}` use `vatra handoff`"
        ));
    }
    Ok(())
}

fn check_agent(agents: &ConfigAgents, name: &str) -> Result<(), String> {
    let known = agents.names();
    if known.iter().any(|candidate| candidate == name) {
        Ok(())
    } else {
        Err(format!(
            "unknown agent `{name}`; configured: {}",
            known.join(", ")
        ))
    }
}

async fn build_agent(
    agents: &ConfigAgents,
    name: &str,
    session_id: i64,
    project_id: i64,
    yes: bool,
) -> Result<Arc<dyn Agent>, String> {
    let approver: Arc<dyn Approver> = if yes {
        Arc::new(AllowAll)
    } else {
        Arc::new(TerminalApprover)
    };
    let mode = if yes {
        PermissionMode::FullAccess
    } else {
        PermissionMode::Supervised
    };
    let questioner = Arc::new(NoQuestions);
    agents
        .build(BuildRequest {
            name: name.into(),
            session_id,
            project_id,
            approver,
            questioner,
            mode,
            cwd: None,
            options: Default::default(),
        })
        .await
}
