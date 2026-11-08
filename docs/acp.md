# ACP agents in Vatra

Vatra can act as an **Agent Client Protocol v1 client**, alongside its existing engine and Claude Code adapter. ACP connects a client to an external coding agent; it is not a model API, MCP replacement, or automatic rate-limit bypass. The external agent chooses its models, owns its tools and authentication, and must already be installed.

The shared ACP route implements Plan B Task 28. No real subscription agent was launched during development. Agent-specific subscription setup remains gated by the author's Task 29 authentication decision.

## Configure

Configure an independently installed ACP executable:

```toml
summarizer = "local"

[agents.local]
kind = "model"
base_url = "http://127.0.0.1:8080/v1"
model = "local"

[agents.external]
kind = "acp"
binary = 'C:\absolute\path\to\agent.exe'
args = ['--acp'] # Use the arguments documented by the installed agent.
startup_timeout_sec = 30
turn_timeout_sec = 900
```

Agent names are arbitrary. A model agent is still required as the summarizer. Both timeout settings accept 1–86400 seconds. The service builds agents lazily: listing configured agents does not start their executables. Existing configurations remain valid.

`kind = "claude"` remains the subscription route through Claude Code. `claude-agent-acp`, built on the Agent SDK, is API-key only, so it is not a subscription route and doesn't replace `kind = "claude"`.

On Windows, npm `.cmd` and `.bat` shims are refused. Use an absolute `node.exe` path as `binary` and the installed package's absolute JavaScript entry point in `args`, followed by its ACP arguments. No package is downloaded or installed automatically. Keep credentials out of arguments and TOML. Sign in with the external agent's own supported workflow before running it.

```powershell
cargo run -p vatra-cli -- --config .\vatra-acp.toml --db .\vatra-acp.db run --project acp-demo --agent external "Inspect this project and describe it"
```

The existing agent list, CLI turn runner, journal, and handoff paths consume the adapter through `vatra_core::Agent`. The service uses the project's absolute directory, or an isolated workspace for a project without a folder. The CLI uses its current working directory.

## Implemented boundary

- Newline-delimited UTF-8 JSON-RPC over stdio, ACP major version 1. A different negotiated version fails clearly; v2 draft semantics are not mixed into v1.
- `initialize`, `session/new`, text-only `session/prompt`, and streamed `session/update`. Assistant text becomes transient deltas and durable text; tool calls and partial tool results become journal events. Standard `plan` entries use the existing task view, and diff content uses the existing edit view. Unknown updates, including unverified usage fields, are retained as `Unmapped`, except user-history and reasoning chunks.
- Capability-gated `session/resume` takes priority over `session/load`. Replay is suppressed to avoid duplicating the journal. Without either capability, the adapter starts a new session with a bounded transcript and latest compaction summary; it warns when replacing an existing native session. Resume/load failures are reported rather than silently starting over. Context added by Vatra since the last turn is included on resume.
- Vatra's approved MCP servers are passed to session creation/loading. Stdio servers are supported; HTTP servers require the agent's `mcpCapabilities.http`. Unsupported entries emit warnings naming the server and reason, with no secret values. OAuth and per-server execution context go through the Vatra stdio MCP proxy. The factory resolves secret references and avoids repeating servers native to that agent. Protocol environment/header values use named-value arrays, and are never emitted as journal events by the client.
- Foreign skill listings, project instructions and memory are added to the first prompt of a new native session. They are omitted after successful resume/load. Explicit `/name` and `$name` use Vatra's shared skill resolver and placeholder expansion; unknown slash commands reach the agent unchanged. Command-bearing skills require Vatra's gated expansion hook. The latest `available_commands_update` is held in the shared session command cache for composer completion.
- `session/request_permission` uses the existing approver with cached tool details, including commands, paths and content. It selects only `allow_once` or `reject_once`; persistent grants are never selected. No suitable option means `cancelled`. Permission checks map each edit location to the existing engine policy: Accept edits allows edits inside the project; missing or outside locations require approval. Imported deny rules still win over permission modes, including Full access.
- Stopping a turn, startup failure, timeout or protocol failure triggers bounded cleanup of the process tree. Once a session is known, cancellation sends `session/cancel`; a pending permission receives a `cancelled` response first. A fresh process is used per turn and terminated after completion.
- Messages are bounded at 1 MiB, cached tool state at 16 MiB / 4096 calls. Startup and whole-turn deadlines prevent hangs. The whole-turn deadline doesn't count time spent waiting for a permission decision. Assistant text streamed before a timeout, failure or stop is still journaled, and a cleanup problem is reported as a warning without replacing the turn's result. Invalid stdout, EOF, unsupported stop reasons and malformed responses fail clearly. Raw remote error messages and stderr are not copied into the journal.
- `VATRA_API_KEY` and variables configured for Vatra model/search keys are removed from the subprocess environment. Other environment variables remain available for the external agent's own setup. Vatra does not read or copy its credential files.

## Limitations and next checks

The client advertises no filesystem, terminal, interactive authentication, elicitation, or extension capabilities. Unsupported agent-to-client requests receive JSON-RPC `-32601`. It sends no authenticate requests, changes no external modes, and doesn't override an agent's own permission policy. An external executable can perform actions itself: approval bridging is not an OS sandbox. Configure only agents you trust and verify their permission defaults separately.

Images, verified usage accounting and remote ACP transports are not implemented. The external agent may still manage its own tools. ACP's permission requests do not guarantee every operation will request approval. The adapter's raw `AcpExtras` boundary expects approved, expanded configuration; callers must resolve secrets, native duplicates and service-backed OAuth before constructing it.

Synthetic fixtures validate the wire flow; they do not establish compatibility with Gemini, Codex, Claude adapters, real authentication, subscription use, or the desktop UI. Extension delivery remains **Limited: not yet verified on this agent**. A separately authorized real-agent smoke test must check the executable/arguments, login, two consecutive turns, permission allow/deny, handoff and cancellation. Keep the Task 29 decision gate for Codex. No fallback to another account or provider happens automatically.

## Protocol references

Implemented against the official ACP v1 documentation inspected on 2026-11-01. This is a minimal client using existing Rust dependencies, without a new SDK dependency or vendored protocol implementation.

- [Initialization and capability negotiation](https://agentclientprotocol.com/protocol/v1/initialization)
- [Session creation and loading](https://agentclientprotocol.com/protocol/v1/session-setup)
- [Prompt lifecycle and cancellation](https://agentclientprotocol.com/protocol/v1/prompt-turn)
- [Tool calls and permission options](https://agentclientprotocol.com/protocol/v1/tool-calls)
- [Stdio framing](https://agentclientprotocol.com/protocol/v1/transports)

Validation commands and observed results are recorded in [the ACP validation report](acp-validation.md).

## Codex with Sign in with ChatGPT

Use the commented Codex configuration in `vatra.example.toml`. Install the ACP adapter and official Codex executable separately, then connect through Settings > ChatGPT account. Vatra generates a persistent installation host ID and uses the official dynamic-registration OAuth flow. Its tokens are stored in the OS keychain. The desktop receives safe account status, never tokens.

`chatgpt_plan = true` requires `subscription = true` and `codex_binary`. Vatra sets CODEX_PATH to its own internal launch bridge for that child only. The bridge starts the configured official Codex executable with the SIWC Responses provider selected before startup account checks. ACCESS_TOKEN is inherited only through the child environment. CODEX_HOME is never set by Vatra; native auth/config files are not copied or modified. ACP authenticate is never sent.

The supplied protocol configuration and synthetic tests do not prove a real account's entitlement or the adapter's behavior against the live provider. Real login/inference is opt-in and was not run during automated validation. One active Vatra registration is supported in B; multi-account selection is deferred. Identity-only authorization does not enable plan inference. If refresh completion becomes uncertain, Vatra requests sign-in instead of replaying a potentially consumed rotating refresh token. Disconnect clears local credentials and attempts remote revocation; the UI explains when remote revocation was not confirmed.
