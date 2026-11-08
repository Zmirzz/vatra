use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub prompt: String,
    /// Folder name under `parity/fixtures`.
    pub fixture: String,
    /// Exact commands the runner may auto-approve, e.g. "node test.js".
    #[serde(default)]
    pub allowed_commands: Vec<String>,
    #[serde(default)]
    pub needs_mcp: bool,
    /// Skill name the prompt invokes.
    #[serde(default)]
    pub skill: Option<String>,
    pub checks: Vec<Check>,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Check {
    FileContains {
        path: String,
        text: String,
    },
    FileNotContains {
        path: String,
        text: String,
    },
    /// Same bytes as in the fixture.
    FileUnchanged {
        path: String,
    },
    /// Run in the workdir after the agent finished; exit 0 passes.
    CommandPasses {
        command: String,
    },
    /// A ToolCall named one of these, or ending in "__" + name (MCP), anywhere including Nested.
    ToolUsed {
        any_of: Vec<String>,
    },
    /// Case-insensitive, in the concatenated AssistantText.
    AnswerContains {
        text: String,
    },
}

/// Loads every `*.toml` task in `dir`, sorted by file name. Errors name the bad file.
pub fn load_tasks(dir: &Path) -> Result<Vec<Task>, String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().is_some_and(|ext| ext == "toml") {
            paths.push(path);
        }
    }
    paths.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

    paths
        .iter()
        .map(|path| {
            let text =
                std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let task =
                toml::from_str::<Task>(&text).map_err(|e| format!("{}: {e}", path.display()))?;
            if task.fixture.is_empty()
                || task.fixture.contains([':', '\\'])
                || Path::new(&task.fixture)
                    .components()
                    .any(|part| !matches!(part, Component::Normal(_)))
            {
                return Err(format!(
                    "{}: fixture must be a relative path inside parity/fixtures",
                    path.display()
                ));
            }
            Ok(task)
        })
        .collect()
}
