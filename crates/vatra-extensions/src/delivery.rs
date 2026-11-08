use serde::Serialize;

use crate::UnsupportedComponent;
use crate::catalog::{CatalogEntry, ElementKind, Origin};

/// Where an element would run: the engine of Vatra, the Claude Code adapter, or an ACP agent such as Codex.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    Engine,
    Claude,
    Acp,
}

/// What a route does with an element. `Limited` and `Unavailable` carry the reason, in plain words.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Works,
    Limited(String),
    NeedsInput(String),
    /// Waits for the user to approve the code the item runs before it is checked.
    NeedsApproval(String),
    Unavailable(String),
}

const UNVERIFIED_ON_CLAUDE: &str = "not yet verified on Claude Code";
const UNVERIFIED_ON_ACP: &str = "not yet verified on this agent";

/// What `route` does with `entry`. A plugin is only a container, so its parts carry the verdicts.
pub fn verdict(entry: &CatalogEntry, route: Route) -> Verdict {
    match (route, entry.kind) {
        (_, ElementKind::Plugin) | (Route::Engine, _) => Verdict::Works,
        (Route::Claude, ElementKind::Skill | ElementKind::Command) if entry.origin != Origin::Claude => {
            Verdict::Limited(UNVERIFIED_ON_CLAUDE.into())
        }
        (Route::Claude, _) => Verdict::Works,
        (Route::Acp, _) => Verdict::Limited(UNVERIFIED_ON_ACP.into()),
    }
}

/// What every route does with a part of a plugin that Vatra doesn't run, such as hooks or account connectors.
pub fn unsupported_verdict(component: &UnsupportedComponent) -> Verdict {
    Verdict::Unavailable(component.reason.clone())
}
