# ACP interoperability validation

Date: 2026-11-01. Platform: Windows. Baseline: `5d916c5`.

This is a separate ACP v1 client addition. The client was committed as `89fedc5`. Plan B Task 11 is committed at `5d916c5` and has passed its Claude Code review. No Plan B checkboxes, authentication decisions, native agent installations or user configurations were changed. The review fixes for this client are committed in `fix(acp): keep partial text, pause the deadline for approvals, prove key removal`.

## Results

| Check | Observed result |
|---|---|
| `cargo test --workspace` | 392 passed, 0 failed, 0 ignored, at `89fedc5`; run outside the restricted sandbox; not re-run for the review fixes |
| `npx tsc --noEmit` in `apps/desktop` | Passed |
| `npx vitest run` in `apps/desktop` | 160 passed across 15 files |
| ACP adapter integration tests | 17 passed after the review fixes, including two turns through the core journal |
| Service agent/configuration tests | 13 passed after the review fixes, including ACP working-folder and hidden-variable tests |
| `cargo clippy --workspace --all-targets -- -D warnings` | Passed after the review fixes |
| CLI smoke with `fake-acp.exe` | Two successful turns using the real CLI, config parser, factory and SQLite journal; `session/new` followed by `session/load` |
| `git diff --check` | Passed |

The first sandboxed workspace run stopped in the existing Claude fixture's `stopping_a_turn_kills_the_whole_process_tree` test, at its initial liveness assertion. Direct diagnostics showed `tasklist` returning “The user name or password is incorrect” and `ping` returning “Unable to contact IP driver. General failure.” The complete workspace passed outside that sandbox without changing the Claude fixture or adapter.

The ACP cancellation test was strengthened to hold Windows process handles, prove both parent and child are initially alive, and wait for both handles to signal exit. It does not infer success from missing `tasklist` output. It passes in the restricted sandbox as well. Rust reported filesystem hard-link cache warnings and copied cache files instead; these were not test failures. Clippy passed after correcting the new configuration validation's nested `if`.

## What the synthetic peer verifies

- Initialization precedes session setup and prompts; only protocol major 1 is accepted, and no optional client capabilities are claimed.
- Prompts preserve newlines and diacritics. Assistant deltas and final text are mapped; unrelated session updates cannot enter the active turn.
- Partial tool updates retain earlier output; unknown live updates are preserved. Unsupported filesystem requests receive `-32601`.
- Native resume suppresses replayed history and retains new handoff context. Agents without load support receive bounded history and the latest compaction summary in a new session.
- Permissions receive cached command details; allow/deny select one-time options, and persistent-only grants aren't selected. Cancellation answers an outstanding permission before notifying the session.
- Missing executables, incompatible versions, invalid JSON, oversized frames, crashes, authentication-required errors, refusal, startup timeout and whole-turn timeout fail clearly.
- The subprocess cannot see `VATRA_API_KEY` or explicitly hidden environment names. Remote error text isn't echoed into failures. No credential file is read by the adapter.
- Factory construction doesn't start an executable; agent names are arbitrary; timeouts are validated; an ACP agent cannot silently omit configured Vatra MCP servers.

## Reproducible evidence

Local, ignored artifacts from this run:

- `target/acp-workspace-tests.log`: complete successful workspace run.
- `target/acp-clippy.log`: successful Clippy run.
- `target/acp-cli-smoke-20261101/`: synthetic TOML, SQLite database, two turn logs and JSON-RPC trace. The configured summarizer endpoint was unused; the peer never contacted a model.

The CLI trace has exactly: `initialize`, `session/new`, `session/prompt`, `initialize`, `session/load`, `session/prompt`. The second turn ends with `{"type":"turn_completed","agent":"external"}`.

## Boundaries for the next review

See [the ACP guide](../../acp.md) for configuration, official protocol sources and supported capabilities. The new crate is `crates/vatra-acp`; existing runtime changes are confined to service configuration/factory wiring, the example config and the workspace lockfile.

Real Gemini/Codex/Claude compatibility, account login, subscriptions, desktop interaction, imported extensions and Vatra-managed MCP forwarding were not exercised. No real agent was started, and no rate-limit workaround or automatic provider switch was added. The new adapter is an ACP client, not an ACP server exposing Vatra to other clients.

Plan B Task 11 has passed its review and is not part of this change. This change is not a substitute for the Codex decision gate in Task 28. A future real-agent check should explicitly validate the installed executable and arguments, permission defaults, two-turn resume, handoff and stop behavior before claiming compatibility with that agent.
