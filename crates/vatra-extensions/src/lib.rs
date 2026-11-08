//! Reads skills and commands in the formats Claude Code and Codex use, and fills in their `$ARGUMENTS` placeholders.

mod bang;
mod catalog;
mod delivery;
mod frontmatter;
mod hash;
mod mcp_file;
mod plugin;
mod skill;
mod substitute;
mod tool_names;

use std::path::PathBuf;

pub use bang::{BangForm, BangSpan, bang_spans};
pub use catalog::{
    ActiveExtensions, AddReport, ApprovedSet, Catalog, CatalogEntry, CatalogError, ElementKind, Origin, PutOutcome,
    RunsCode, Scope, UsageCounts,
};
pub use delivery::{Route, Verdict, unsupported_verdict, verdict};
pub use hash::content_hash;
pub use mcp_file::{LiteralSecret, McpFile, SecretPlace, parse_codex_mcp, parse_mcp_json};
pub use plugin::{ManifestKind, PluginBundle, UnsupportedComponent, UserOption, read_plugin};
pub use skill::{Skill, parse_command, parse_skill};
pub use skill::validate_skill_standard;
pub use substitute::{SubstituteInput, substitute};
pub use tool_names::{canonical_tool, canonical_tools};

/// Why a skill, command or policy file could not be read.
#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    #[error("cannot read {}: {source}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid frontmatter in {}: {message}", path.display())]
    Frontmatter { path: PathBuf, message: String },
    #[error("invalid YAML in {}: {source}", path.display())]
    Yaml {
        path: PathBuf,
        #[source]
        source: serde_yaml_ng::Error,
    },
    #[error("invalid MCP config: {message}")]
    Mcp { message: String },
    #[error("invalid `{field}` in {}: {message}", path.display())]
    Field {
        path: PathBuf,
        field: String,
        message: String,
    },
}
