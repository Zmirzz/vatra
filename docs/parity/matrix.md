# Fixed task results

This covers only the listed tasks and does not establish general agent equivalence. Times are Unix seconds (UTC).

| Agent / model | Build | Date (UTC) | Coverage | Passed |
|---|---|---|---|---|
| local:qwen3-4b-instruct-2507-q4_k_m / local:qwen3-4b-instruct-2507-q4_k_m | llama.cpp 12019 | 1793736301 | 6 tasks | 3 |

Task details for local:qwen3-4b-instruct-2507-q4_k_m / local:qwen3-4b-instruct-2507-q4_k_m (1793736301)

- 01-fix-test: passed

- 02-rename: failed: `node check.js` still fails: at Module._load (node:internal/modules/cjs/loader:1443:12) at wrapModuleLoad (node:internal/modules/cjs/loader:261:19) at Module.executeUserEntryPoint [as runMain] (node:internal/modules/run_main:154:5) at node:internal/main/run_main_module:33:47 Node.js v24.21.0; api.js still contains "getUser"; cli.js still contains "getUser"

- 03-mcp: failed: result.txt is missing

- 04-skill: passed

- 05-search: passed

- 06-multi-step: failed: none of these tools was used: TodoWrite, update_plan

## Ce înseamnă rezultatul

Rulare reală, locală și gratuită, pe 2026-11-03, cu modelul Qwen3 4B Instruct 2507 (Q4_K_M) pe llama.cpp 12019, GPU NVIDIA RTX 5060. Coloana „Agent / model” repetă numele agentului local; modelul este cel din rândul de mai sus. Rezultatul este 3 din 6 sarcini și descrie acest model pe aceste sarcini, nu paritate cu Claude Code sau Codex. Testele cu binare false verifică runner-ul și evaluatorul și nu intră în această matrice.

Cum se repetă:

```powershell
vatra parity run --agent local:<slug>
vatra parity export --output docs/parity/matrix.md
```

Pentru API se cere `--consent-cost`. Comparația nativă Claude cere model explicit și acord separat; Codex nativ și ACP nu sunt rulate (vezi raportul D).
