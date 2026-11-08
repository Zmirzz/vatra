# Vatra

A desktop assistant that brings your AI tools together in one calm place. Use Claude Code, Codex, any
OpenAI-compatible service, GitHub Copilot or a model running on your own computer, with documents, tasks,
a calendar, email, cited web research and side-by-side model comparison next to your conversations.

Built for ordinary people as much as programmers: plain words, sensible defaults, details only when you ask.

## Install (Windows)

Download `Vatra_<version>_x64-setup.exe` from the [latest release](../../releases/latest) and run it. The
installer is not code-signed yet, so Windows may show a SmartScreen warning. Updates arrive inside the app.

## What it does

- **Conversations** with Claude Code, Codex, API models, GitHub Copilot and local models (llama.cpp, Ollama, vLLM, LM Studio).
- **Web search in chat** and **Deep Research** with checked, linked sources.
- **Documents, tasks, calendar and email** your assistants can read and propose changes to; nothing is sent or changed without your confirmation.
- **Bring your tools over** from Claude Code and Codex: plugins, MCP servers, skills and history.
- **Compare models** side by side with names hidden until you choose.
- English and Romanian.

## Build from source

Requirements: Rust (stable), Node.js 20+, and on Windows the MSVC build tools.

```powershell
cargo test --workspace
cd apps/desktop
npm install
npm run build:desktop   # NSIS installer under target/release/bundle/nsis/
```

For a development run, start the service with `cargo run -p vatra-cli -- serve` and the UI with `npm run dev`
in `apps/desktop`. Copy `vatra.example.toml` to `vatra.toml` to configure agents by hand. See `docs/` for the
ACP notes, the security model and the release process.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
