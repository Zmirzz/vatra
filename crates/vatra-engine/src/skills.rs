//! Skills and commands on the engine: the listing the model sees, the names a prompt invokes, and what an
//! invocation does to the turn.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use vatra_core::{Event, Rule};
use vatra_extensions::{Catalog, Skill, SubstituteInput, UsageCounts, substitute};

use crate::Approver;
use crate::inject::{COMMAND_TIMEOUT, ShellCtx, run_injections};
use crate::instructions::{PathRule, covers};
use crate::model::ContextWindow;
use crate::permissions::PermissionPolicy;
use crate::subagents::{GENERAL_PURPOSE, SubagentRequest, SubagentRunner};
use crate::tools::ToolContext;

/// The listing's budget is this share of the window, in tokens, times four characters a token. It never goes
/// below this many characters.
const LISTING_SHARE_PERCENT: u64 = 1;
const LISTING_MIN_CHARS: usize = 2_000;
const CHARS_PER_TOKEN: u64 = 4;

/// The skills and commands of one turn, and the state the session keeps for them.
#[derive(Default)]
pub struct EngineExtensions {
    /// The skills and commands that apply: the enabled ones, with their code approved.
    pub skills: Vec<Skill>,
    /// What the session keeps between its turns: the commands approved, and the skills run so far.
    pub session: Arc<SkillSession>,
    /// How often each skill ran, by the name the person types. The listing keeps the most used descriptions.
    pub usage: UsageCounts,
    /// Where the runs are counted. Without one they are not kept.
    pub catalog: Option<Arc<Catalog>>,
    /// What the catalog left out, shown as warnings at the start of each turn.
    pub problems: Vec<String>,
    /// Runs the skills that ask for a fork, as subagents of the turn. None when the turn has none.
    pub subagents: Option<Arc<SubagentRunner>>,
    /// The turn's permissions, given to the extensions when the engine is built.
    gate: Mutex<Option<Gate>>,
}

struct Gate {
    policy: Arc<PermissionPolicy>,
    approver: Arc<dyn Approver>,
}

impl EngineExtensions {
    /// Gives the skills the rules and the approver of the engine they run in.
    pub(crate) fn bind(&self, policy: Arc<PermissionPolicy>, approver: Arc<dyn Approver>) {
        *lock(&self.gate) = Some(Gate { policy, approver });
    }

    fn gate(&self) -> Option<(Arc<PermissionPolicy>, Arc<dyn Approver>)> {
        lock(&self.gate)
            .as_ref()
            .map(|gate| (Arc::clone(&gate.policy), Arc::clone(&gate.approver)))
    }

    /// The skill or command the model names. Skills come before commands of the same name.
    pub(crate) fn find(&self, name: &str) -> Option<&Skill> {
        self.named(name, |_| true)
    }

    fn named(&self, name: &str, keep: impl Fn(&Skill) -> bool) -> Option<&Skill> {
        let found = |command: bool| {
            self.skills
                .iter()
                .find(|skill| skill.is_command == command && keep(skill) && names(skill, name))
        };
        found(false).or_else(|| found(true))
    }
}

/// What a session keeps between turns, and what a turn starts with.
#[derive(Default)]
pub struct SkillSession {
    /// The id of the session, for the `${CLAUDE_SESSION_ID}` placeholder.
    pub(crate) id: String,
    pub(crate) state: Mutex<TurnState>,
    /// The commands the person has approved in this session. Each one is asked for once.
    pub(crate) approved: Mutex<HashSet<String>>,
}

#[derive(Default)]
pub(crate) struct TurnState {
    /// The skills run in this turn, or earlier in the session when they are in the history: their instructions.
    /// One entry per skill, the latest run.
    invoked: Vec<Invoked>,
    /// The effort the last skill with one asked for. It applies to the model's requests of the turn.
    effort: Option<String>,
    /// The tools a skill of this turn turned off, by name.
    disallowed: Vec<String>,
}

impl SkillSession {
    pub fn new(id: impl Into<String>) -> SkillSession {
        SkillSession {
            id: id.into(),
            state: Mutex::default(),
            approved: Mutex::default(),
        }
    }

    /// Starts a turn: the skills run earlier are read back from the history, and the turn's effort and tools
    /// start clear.
    pub(crate) fn start_turn(&self, history: &[Event]) {
        *lock(&self.state) = TurnState {
            invoked: invoked_in(history),
            ..Default::default()
        };
    }

    pub(crate) fn invoked_texts(&self) -> Vec<String> {
        lock(&self.state).invoked.iter().map(|invoked| invoked.text.clone()).collect()
    }

    pub(crate) fn effort(&self) -> Option<String> {
        lock(&self.state).effort.clone()
    }

    pub(crate) fn disallowed(&self) -> Vec<String> {
        lock(&self.state).disallowed.clone()
    }

    fn record(&self, skill: &Skill, text: &str) {
        let mut state = lock(&self.state);
        remember(&mut state.invoked, &skill.qualified_name(), text);
        if let Some(effort) = &skill.effort {
            state.effort = Some(effort.clone());
        }
        // Only the bare names are kept: a name turns the whole tool off, an entry with a pattern is a rule.
        state.disallowed.extend(
            skill
                .disallowed_tools
                .iter()
                .filter(|entry| !entry.contains('('))
                .cloned(),
        );
    }
}

/// Keeps one entry per skill, the latest: an earlier run of the same skill is dropped, and this one goes last.
fn remember(invoked: &mut Vec<Invoked>, name: &str, text: &str) {
    invoked.retain(|earlier| earlier.name != name);
    invoked.push(Invoked {
        name: name.into(),
        text: text.into(),
    });
}

/// The skills run in a history: each one that was journaled as loaded, and each Skill call that succeeded.
fn invoked_in(history: &[Event]) -> Vec<Invoked> {
    let results: HashMap<&str, (bool, &str)> = history
        .iter()
        .filter_map(|event| match event {
            Event::ToolResult { id, output, is_error } => Some((id.as_str(), (*is_error, output.as_str()))),
            _ => None,
        })
        .collect();
    let mut invoked = vec![];
    for event in history {
        match event {
            Event::SkillLoaded { name, text } => remember(&mut invoked, name, text),
            Event::ToolCall { id, name, arguments } if name == "Skill" => {
                if let Some((false, text)) = results.get(id.as_str()).copied() {
                    let skill = arguments["skill"].as_str().unwrap_or_default();
                    remember(&mut invoked, skill, text);
                }
            }
            _ => {}
        }
    }
    invoked
}

/// A skill run in the session: its name, and the instructions the model got.
struct Invoked {
    name: String,
    text: String,
}

/// The listing of the skills the model may run: one line each, with a description while the budget allows. The
/// most used skills keep theirs. Every name is listed, however small the budget.
pub fn skills_listing(skills: &[Skill], window: ContextWindow, usage: &UsageCounts) -> String {
    listing_of(skills.iter(), window, usage)
}

pub(crate) fn listing_of<'a>(
    skills: impl Iterator<Item = &'a Skill>,
    window: ContextWindow,
    usage: &UsageCounts,
) -> String {
    let shown: Vec<&Skill> = skills.filter(|skill| skill.allow_implicit).collect();
    if shown.is_empty() {
        return String::new();
    }
    let budget = ((window.tokens * CHARS_PER_TOKEN * LISTING_SHARE_PERCENT / 100) as usize).max(LISTING_MIN_CHARS);
    let labels: Vec<String> = shown.iter().map(|skill| skill.qualified_name()).collect();
    let texts: Vec<String> = shown.iter().map(|skill| description_of(skill)).collect();
    // A name line is "- name" and a line with a description adds ": " and the description.
    let mut total: usize = labels.iter().map(|label| 3 + label.chars().count()).sum();
    let mut with_text = vec![false; shown.len()];
    let mut by_use: Vec<usize> = (0..shown.len()).collect();
    by_use.sort_by(|&a, &b| usage.get(&labels[b]).cmp(&usage.get(&labels[a])).then(a.cmp(&b)));
    for index in by_use {
        if texts[index].is_empty() {
            continue;
        }
        let extra = 2 + texts[index].chars().count();
        if total + extra <= budget {
            total += extra;
            with_text[index] = true;
        }
    }
    labels
        .iter()
        .zip(&texts)
        .zip(&with_text)
        .map(|((label, text), &shown_text)| {
            if shown_text {
                format!("- {label}: {text}")
            } else {
                format!("- {label}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The description the listing shows: the description, then the `when_to_use` text.
fn description_of(skill: &Skill) -> String {
    [
        skill.description.as_str(),
        skill.when_to_use.as_deref().unwrap_or_default(),
    ]
    .iter()
    .map(|part| part.trim())
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" ")
}

/// The skill a prompt runs with `/name args` at its start, and the skills it names with `$name` in its text.
#[derive(Default)]
pub struct ResolvedPrompt {
    pub command: Option<Invocation>,
    pub mentions: Vec<Skill>,
}

pub struct Invocation {
    pub skill: Skill,
    pub arguments: String,
}

/// Finds the invocations in a prompt. Only skills and commands the person may invoke count: a `/name` that matches
/// none of them is text for the model, and so is a `$name` that matches no skill.
pub fn resolve_prompt(prompt: &str, skills: &[Skill]) -> ResolvedPrompt {
    let command = slash_command(prompt, skills);
    let mut mentions: Vec<Skill> = vec![];
    for name in dollar_names(prompt) {
        let found = skills
            .iter()
            .find(|skill| !skill.is_command && skill.user_invocable && names(skill, name));
        if let Some(skill) = found.filter(|skill| !mentions.iter().any(|known| names_same(known, skill))) {
            mentions.push(skill.clone());
        }
    }
    ResolvedPrompt { command, mentions }
}

fn slash_command(prompt: &str, skills: &[Skill]) -> Option<Invocation> {
    let rest = prompt.trim_start().strip_prefix('/')?;
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let (name, arguments) = rest.split_at(end);
    if name.is_empty() {
        return None;
    }
    let found: Vec<&Skill> = skills
        .iter()
        .filter(|skill| skill.user_invocable && names(skill, name))
        .collect();
    let skill = found.iter().find(|skill| !skill.is_command).or(found.first())?;
    Some(Invocation {
        skill: (*skill).clone(),
        arguments: arguments.trim().to_string(),
    })
}

/// The names written as `$name` in the text. A `$` in a code span, after a letter, or before a word that looks
/// like an email address is not a name.
fn dollar_names(prompt: &str) -> Vec<&str> {
    let mut found = vec![];
    let mut in_code = false;
    let mut previous: Option<char> = None;
    let chars: Vec<(usize, char)> = prompt.char_indices().collect();
    for (index, &(at, c)) in chars.iter().enumerate() {
        if c == '`' {
            in_code = !in_code;
        } else if c == '$' && !in_code && !previous.is_some_and(|p| p.is_alphanumeric() || "_@/\\$".contains(p)) {
            let start = at + 1;
            let mut end = start;
            for &(next_at, next) in &chars[index + 1..] {
                if !(next.is_alphanumeric() || "-_:".contains(next)) {
                    break;
                }
                end = next_at + next.len_utf8();
            }
            let after = prompt[end..].chars().next();
            let email_like = after.is_some_and(|next| next.is_alphanumeric() || next == '@' || next == '_');
            if end > start && !email_like {
                found.push(&prompt[start..end]);
            }
        }
        previous = Some(c);
    }
    found
}

/// Whether two skills are the same one, by their qualified names.
fn names_same(first: &Skill, second: &Skill) -> bool {
    first.qualified_name() == second.qualified_name()
}

/// Whether a skill has the name, as typed: its plugin name and its own, or just its own for a skill without one.
fn names(skill: &Skill, name: &str) -> bool {
    skill.qualified_name() == name || (skill.plugin.is_none() && skill.name == name)
}

/// Runs a skill or command and returns the text the model gets: its instructions, with the commands run and the
/// placeholders filled in. The run is counted, its tools are turned off for the turn, and its effort applies from
/// the next request. Its `allowed-tools` are granted only when the person started the run (`by_user`): a model
/// call never widens what the person allowed. A skill that asks for a fork answers with its subagent's text instead.
/// `call_id` is the id of the Skill call, or empty when the person started the run. Each failure is returned as the
/// message to show.
pub(crate) async fn invoke(
    extensions: &EngineExtensions,
    skill: &Skill,
    arguments: &str,
    context: &ToolContext,
    by_user: bool,
    call_id: &str,
) -> Result<String, String> {
    let name = skill.qualified_name();
    let Some((policy, approver)) = extensions.gate() else {
        return Err(format!(
            "The skill `{name}` can't run, because this turn has no permissions."
        ));
    };
    if let Some(model) = &skill.model {
        warn(
            context,
            &name,
            format!("The skill `{name}` asks for the model `{model}`, which isn't used yet. The turn keeps its model."),
        );
    }
    let shell = ShellCtx {
        context,
        policy: &policy,
        approver: approver.as_ref(),
        approved: &extensions.session.approved,
        skill: &name,
        timeout: COMMAND_TIMEOUT,
    };
    let body = run_injections(&skill.body, &shell)
        .await
        .map_err(|error| error.to_string())?;
    let text = substitute(
        &body,
        &SubstituteInput {
            arguments,
            argument_names: &skill.arguments,
            skill_dir: &skill.dir,
            session_id: &extensions.session.id,
            plugin_root: skill.plugin_root.as_deref(),
            project_dir: Some(context.project_root.as_path()),
            plugin_data: skill.plugin_data.as_deref(),
            effort: skill.effort.as_deref(),
        },
    );
    if let Some(Err(error)) = extensions.catalog.as_ref().map(|catalog| catalog.record_use(&name)) {
        warn(
            context,
            &name,
            format!("The run of `{name}` couldn't be counted: {error}"),
        );
    }
    extensions.session.record(skill, &text);
    if by_user {
        for entry in &skill.allowed_tools {
            grant(&policy, entry, context, &name);
        }
    }
    for entry in &skill.disallowed_tools {
        deny(&policy, entry, context, &name);
    }
    if skill.context_fork {
        return fork(extensions, skill, text, context, call_id).await;
    }
    Ok(text)
}

/// Runs a skill that asks for a fork: its instructions are the task of a subagent, and the subagent's answer is the
/// text of the run. The subagent works under the grants and denials that the run has just made.
async fn fork(
    extensions: &EngineExtensions,
    skill: &Skill,
    prompt: String,
    context: &ToolContext,
    call_id: &str,
) -> Result<String, String> {
    let name = skill.qualified_name();
    let Some(runner) = &extensions.subagents else {
        return Err(format!("The skill `{name}` runs in a fork, but this turn has no subagents."));
    };
    let request = SubagentRequest {
        prompt,
        description: name.clone(),
        agent_type: skill.agent.clone().unwrap_or_else(|| GENERAL_PURPOSE.into()),
        model: None,
        context: context.clone(),
    };
    // A run the person started has no call to nest under, so its steps nest under the skill's name.
    let parent = if call_id.is_empty() { format!("skill:{name}") } else { call_id.to_string() };
    let output = runner.run(request, &parent).await;
    if !output.is_error {
        return Ok(output.text);
    }
    // A run the person started fails as a step: a warning, and the turn goes on with the failure as the run's text.
    if call_id.is_empty() {
        let message = format!("The skill `{name}` didn't finish: {}", output.text);
        warn(context, &name, message.clone());
        return Ok(message);
    }
    Err(output.text)
}

fn grant(policy: &PermissionPolicy, entry: &str, context: &ToolContext, name: &str) {
    match Rule::parse(entry) {
        Ok(rule) => policy.grant_for_turn(rule),
        Err(message) => warn(
            context,
            name,
            format!("The skill `{name}` allows `{entry}`, which can't be used: {message}"),
        ),
    }
}

fn deny(policy: &PermissionPolicy, entry: &str, context: &ToolContext, name: &str) {
    match Rule::parse(entry) {
        Ok(rule) => policy.deny_for_turn(rule),
        // A tool name the rules don't know is still turned off by name, so only the message is left.
        Err(message) => warn(
            context,
            name,
            format!("The skill `{name}` turns off `{entry}`: {message}"),
        ),
    }
}

fn warn(context: &ToolContext, source: &str, message: String) {
    // A closed sink ends the turn at the next event, so a failed send is not an error here.
    let _ = context.events.send(Event::Warning {
        source: source.into(),
        message,
    });
}

/// The skills the model is shown this step: those without paths, and those whose paths a touched file matches.
pub(crate) fn visible<'a>(skills: &'a [Skill], root: &Path, touched: &[PathBuf]) -> Vec<&'a Skill> {
    skills
        .iter()
        .filter(|skill| {
            skill.paths.is_empty() || {
                let rule = PathRule {
                    label: skill.qualified_name(),
                    patterns: skill.paths.clone(),
                    text: String::new(),
                };
                touched.iter().any(|file| covers(&rule, root, file))
            }
        })
        .collect()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
