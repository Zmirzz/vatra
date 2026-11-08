# Vatra agent platform: implementation and evidence ledger

This is one delivery covering the complete user request of 2026-11-07. The goal
remains active until all requirements are implemented and verified. Entries below
are current code inspection, not proof of live provider compatibility.

## Initial audit and shared architecture

The clean `main` working tree uses Tauri 2, React 19/TypeScript and a Rust workspace.
The desktop talks to the authenticated loopback WebSocket JSON-RPC service.
`vatra-core::Store` owns SQLite conversation events, project associations, workspace
documents, versions, proposals, preferences and audit records. Preserve those IDs
and tables. Existing document viewers and artifact panels must be reused.

| Responsibility | Existing implementation | Decision |
| --- | --- | --- |
| Provider registry and saved configuration | `vatra-service/config.rs`, `setup.rs`, `agents.rs`, app agent rows | Extend existing registry; do not add another provider store |
| Native inference | `vatra-models` process/model manager; `vatra-engine` model drivers | Reuse llama.cpp lifecycle and existing model picker |
| ACP runtime | `vatra-acp`: stdio JSON-RPC v1, capabilities, session load/resume, streaming, permission mapping, cancellation, model/options probes | Extend this runtime, never create an agent-specific second ACP client |
| Claude | `vatra-claude`, approval bridge and `AgentConfig::Claude` | Migration still required; preserve working route until supported replacement and rollback are verified |
| Codex | `AgentConfig::Acp`, `chatgpt` OAuth, official-binary launch bridge | Already ACP; audit against maintained adapter and preserve state/authentication |
| MCP | Official Rust `rmcp` SDK; engine MCP hubs/pool, service proxy, OAuth and catalog | Reuse stdio/Streamable HTTP, resource/prompt interfaces, secrets and project isolation |
| Skills/extensions | `vatra-extensions` frontmatter, parser, catalog, import; engine skill loader and gated expansion | Extend catalog/discovery/install UI and standard validation |
| Tools/orchestration | `ToolRegistry`, `EngineAgent`, schema/policy/approver, bounded turns, tool search, compaction, event stream | Register integrations here; keep orchestration deterministic where reliability matters |
| Accounts and secrets | `SecretStore`/OS keyring; ChatGPT and MCP auth; connector registry and SQLite connector rows | Extend existing connector registry and keychain references; never return tokens in RPC/model context |
| Permissions/audit | Core rules, engine policy, service approvals and audit persistence | Route installation/external writes through existing authorization; external text cannot grant permission |
| Web | Existing Brave WebSearch and WebFetch | Extend search provider abstraction and reuse extraction/security; interactive browser remains to implement |
| Calendar/email/research | Connector framework exists but ships no connector kinds; workspace reminders are not calendar integration | Implement actual provider adapters and sequential research; do not relabel reminders or a prompt as completed integrations |
| Settings | Horizontal Settings pages, persisted frontend preferences, agent/extension/model management | Replace navigation with grouped vertical sidebar, reuse working content and add real pages only |

All external services remain optional. MCP configuration is passed to ACP agents
only through negotiated supported mechanisms. Agent sessions remain agent-owned;
changing agents does not imply transferable internal state. Process cleanup,
projectless conversation isolation and secret filtering must remain intact.

## Protocol sources inspected live on 2026-11-07

- ACP official index: <https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json>.
  Envelope `version: "1.0.0"`, `agents`; each entry includes version, authors and
  distribution methods such as platform-keyed `binary`, `npx`, `uvx`.
- MCP official API: <https://registry.modelcontextprotocol.io/v0.1/servers?limit=1>.
  Envelope `servers: [{server, _meta}]`, `metadata.nextCursor` and `count`.
  Server entries contain packages/remotes and installation configuration metadata.
- <https://github.com/agentclientprotocol/registry/blob/main/README.md>
- <https://github.com/agentclientprotocol/claude-agent-acp/blob/main/README.md>:
  the current adapter describes itself as using the official Claude Agent SDK.
  This alone does not prove the required unmodified CLI/subscription migration.
- <https://github.com/agentclientprotocol/codex-acp/blob/main/README.md>:
  adapter supports ChatGPT/API key/custom gateway authentication and `CODEX_PATH`.
- <https://github.com/pingdotgg/t3code/blob/main/docs/user/providers-acp.md>:
  distinguishes ACP v1 from v2 preview and manages exact registry distributions.
  Vatra currently negotiates v1; registry presence is not a compatibility verdict.

## Completion evidence still required

- ACP marketplace installation/update/remove, manual configurations, capability
  negotiation including auth/terminal/usage, Claude migration, real two-turn
  Claude/Codex sessions, permissions, cancellation and rollback.
- MCP registry installation/import/configuration, lifecycle management, tool
  toggles, resources/prompts, stdio and remote execution verification.
- Skills ZIP/git installation, integration of previews into the shared side panel,
  broader live desktop verification and remaining source/path defense evidence.
  Local discovery/install/management and existing progressive runtime are covered below.
- Google/Microsoft OAuth account setup and calendar/email APIs, ICS,
  calendar/inbox/composer interfaces and confirmed external writes.
- Configurable real search, isolated interactive browser and side-panel activity.
- Persistent sequential Quick/Standard/Deep research with follow-up retrieval,
  evidence/citations/contradictions, cancellation/resume and MD/PDF/DOCX artifacts.
- Complete functional Settings navigation/search/accessibility/persistence,
  command access and reused side-panel views.
- Actual cross-service workflows, security/lifecycle/resource checks, regression
  tests, desktop application verification and exact live-auth limitations.

No live agent inference, external-account authentication, mail sending, calendar
write, package installation or provider credential extraction has been performed
for this stage. Synthetic tests must be reported separately from live evidence.

## Implemented so far (stage remains active)

- Added the read-only `registry.acp` and `registry.mcp` service methods, backed by
  fixed official HTTPS endpoints. ACP format validation, distribution availability
  by host platform, search, five-minute index caching and MCP cursor pagination
  preserve source distribution/configuration metadata. Requests have timeouts,
  response-size limits and no redirect following. Unsupported envelopes fail
  visibly. Discovery never launches commands or claims verified interoperability.
- Added real ACP/MCP discovery pages within Settings. Entries expose publisher,
  repository, version and complete installation requirements. Links permit HTTPS
  only; remote metadata is rendered as text. No package installation action is
  claimed implemented yet.
- Replaced horizontal Settings navigation with grouped vertical navigation,
  independent scrolling, page titles, indexed search, responsive sizing and
  keyboard focus containment/restoration. Applied the user's supplied reference:
  discreet icons, quiet selection and compact rows using Vatra theme tokens.
  Working preferences, agents, imports, extensions, accounts and model controls
  are reused. Additional nonfunctional pages were not added.
- Extended the existing ACP registration form to native executable commands,
  arbitrary string-array arguments and validated startup/turn timeouts. Legacy
  Node/script forms still deserialize. Editing now preserves the full argument
  vector and timeouts: the old `agents.form` response kept only `args.first()`.
  Configuration continues to use the existing agent store and generic runtime.
  `.cmd`/`.bat` shims and invalid/NUL/oversized arguments are refused before save.
- Added a functional Skills Manager backed by the existing extension catalog and
  engine skill loader: configurable global/project source directories, discovery,
  strict Agent Skills YAML metadata validation, resource inventory, duplicate
  diagnostics, reload, inspect, local install/update, enable/disable and uninstall.
  Sources are read-only references; installations are content-addressed copies.
  Metadata discovery omits instruction bodies. Model disclosure/invocation continues
  through the existing bounded skill listing and `Skill` tool.
- Local skill installation requires the reviewed source hash and, on replacement,
  the installed hash. The catalog verifies the source again before its checked
  copy. Executable content remains unapproved; source scripts never run at install.
  Audit records contain the action and content hash, not instruction bodies.
  Uninstall removes the standalone catalog row and retains original/hashed files.
- Source scanning bounds file size, resource bytes/count/depth and directory
  entries, skips linked resources, reports invalid skills and refuses linked
  SKILL.md inspection. Slow scans run outside the RPC loop. A failed scan leaves
  source management usable. Desktop open commands validate configured source
  membership using the profile's read-only database and allow only the skill
  directory or SKILL.md; no general webview file-opening permission was added.

Observed validation:

- `cargo test -p vatra-service --test registry live_official -- --ignored`:
  **1 live test passed** against the official ACP index and two MCP pages.
  No login, inference or package installation occurred.
- Registry parser fixtures: **3 passed**. HTTP boundary tests: **3 passed**
  (redirect refusal, oversized/invalid JSON response, invalid request bounds).
- `cargo test -p vatra-service --test configured_agents`: **17 passed**.
- RPC save/edit/save round-trip regression: **1 passed** using a real local RPC
  connection and synthetic executable configuration; it did not execute an agent.
- `cargo clippy -p vatra-service --lib -- -D warnings`: passed.
- `npm run build`: TypeScript and Vite production build passed. Existing bundle
  size/dynamic-import warnings remain; this is not a Tauri installer or GUI test.
- The final full frontend regression run passed **320 tests in 48 files** after
  the custom ACP form, registry pages, reference layout and focus changes.

Still unverified: rendered desktop layout, native installer, actual agents and
provider subscriptions, authentication and all downstream integrations listed in
the completion ledger. Current code must not be described as the completed stage.

## Skills validation added in the continuation

- Live inspection of <https://agentskills.io/specification> confirmed required
  name/description, 64/1024 character limits, lowercase/hyphen name rules,
  500-character compatibility and string-to-string metadata.
- `cargo test -p vatra-service --test skills_manager`: 4 passed, covering source
  immutability, metadata/resource discovery, duplicate/scope diagnostics, size
  limits, reviewed-hash rejection, local installation/update, persistent disable
  state, database reopen, runtime metadata listing/explicit invocation, audit and
  uninstall. Native source target validation rejects unrelated paths.
- `cargo test -p vatra-extensions --test catalog`: 34 passed, including checked
  installation and removal/retention regressions.
- `cargo test -p vatra-extensions --test skill`: 21 passed; legacy parsing remains
  compatible. Strict standard validation is applied to newly managed sources.
- `cargo test -p vatra-engine --test skills`: 37 passed, including progressive
  loading, permissions, scope, cancellation and injected-command behavior.
- Authenticated WebSocket RPC regression passed for source registration,
  metadata discovery, full inspection, reviewed installation and removal.
- Full frontend run: 323 tests in 49 files passed. The final source-opening UI
  adjustment also passed its 3 focused tests and TypeScript checking.
- Production frontend build passed with the previously recorded warnings.
- Final `cargo clippy -p vatra-service --lib -- -D warnings` passed.
- `cargo check -p vatra-desktop --lib` passed including the native source-opening
  command. This does not prove real OS opening or a rendered desktop session.

These are local fixture and integration tests, not live model or OS-opening tests.
ZIP/Git skill installation and shared side-panel previews are still outstanding.

## Update 2026-11-07: workspace services (E3-E6)

Built in this delivery, each with tests against local fake servers: scheduled tasks and calendar (ICS, CalDAV),
email (IMAP/SMTP, tool-less reading, confirmed single send), cited research, blind model comparison, a Telegram
bot, and Ollama / vLLM / LM Studio providers. Settings has an Accounts page for the new connector kinds.

Still open, and why:
- Real two-turn Claude/Codex sessions and the Claude ACP migration: they spend the person's subscription, so they
  need their consent.
- Google and Microsoft OAuth for mail and calendar: needs OAuth client ids registered by the person; mail and
  calendar work today with app passwords.
- Interactive isolated browser and side-panel activity: not started.
- Copilot backend: usage terms unconfirmed.
- No real mail, Telegram or search server was contacted; only fixtures.

## Update 2026-11-07 (later): research v2, status, phone notifications, palette

Ideas taken from reading Odysseus (AGPL: ideas only, no code) and t3code (UX patterns), written from scratch.

- Search: a chain of providers (Brave, Tavily, Serper, your own SearXNG, DuckDuckGo with no key), with query repair
  (cleaner query, looser query on an empty answer, next provider on failure), de-duplication and ranking, plus arXiv
  and OpenAlex for academic questions.
- Pages: PDFs are read, video pages give their transcript, and a page that is empty without JavaScript or that asks
  for a human check is refused as unavailable instead of counting as evidence.
- Research runs in rounds with a check between them, counts how it went (rounds, sources by kind and provider),
  keeps a partial report when the full one can't be written, asks clarifying questions first, has a light mode for
  small models (suggested from the graphics card), can be exported (Markdown, HTML), saved as a document, discussed in
  a conversation, and scheduled.
- Status page with the reason for each failure and a way to check again; ntfy phone notifications; email contacts
  and signature; command palette (Ctrl+Shift+P) and toasts.

Added after: lighter prompts and a core tool set for models with a context window of 16k tokens or less; search by
meaning (an embedding account, background indexing of documents, notes, tasks and research reports, an agent tool
`search_by_meaning`, and results in the command palette); and an opt-in browser (the Edge or Chrome already on the
computer) for pages that only show content through JavaScript.

Not built, by decision: a command-line interface and a webhook to start tasks from scripts.
