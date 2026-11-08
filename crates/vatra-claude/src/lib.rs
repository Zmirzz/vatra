mod adapter;
mod generated;
mod parse;

pub use adapter::{ClaudeAgent, ClaudeApprovals, ClaudeConfig, claude_args, find_on_path};
pub use generated::{
    ClaudeExtras, GeneratedDir, ProxySettings, generate_session_dir, translate_prompt,
};
pub use parse::{Parsed, parse_line};

mod options;
pub use options::probe_models;
