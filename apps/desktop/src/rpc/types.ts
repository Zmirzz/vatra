export type PermissionMode = "supervised" | "accept_edits" | "full_access";

export type LastTurn = "none" | "completed" | "failed" | "unfinished";

export type Project = { id: number; name: string; path: string | null };

export type Verdict = "works" | { limited: string } | { needs_input: string } | { needs_approval: string } | { unavailable: string };
export type Route = "engine" | "claude" | "acp";
export type RouteVerdicts = [Route, Verdict][];
export type SecretName = { server: string; key: string };
export type RunsCode = { what: string; command: string };
export type ImportReportItem = { kind: string; name: string; origin: string; routes: RouteVerdicts; warning?: string | null };
export type ScanItem = { key: string; source: string; project: number | null; project_known: boolean; status: "new" | "changed" | "unchanged" | null; secrets: SecretName[]; runs_code: RunsCode[]; report: ImportReportItem };
export type ScanView = { items: ScanItem[]; alternatives?: ScanItem[]; conflicts: { name: string; kind: string; kept: { source: string }; dropped: { source: string }[] }[]; problems: { source: string; message: string }[] };
export type ImportItemChoice = { key: string; include: boolean; approve_code: boolean; secrets: Record<string, boolean>; allow_rules: boolean };
export type HistoryProject = { key: string; name: string; path: string | null; conversations: number; last_active: string; known: boolean };
export type MarketListing = { id: string; name: string; needs: "nothing" | "account" | "token"; token: { getUrl: string } | null; runsProgram: boolean; connected: boolean };
export type ImportOutcome = { results: { key: string; name: string; project: number | null; state: { state: string; reason?: string; message?: string } }[] };
export type CatalogEntry = { id: number; name: string; kind: string; origin: string; scope: string | { project: number }; content_hash: string; approved_hash: string | null; enabled: boolean; runs_code: RunsCode[]; plugin: string | null; details: Record<string, unknown>; routes?: RouteVerdicts; secrets?: SecretName[] };
export type HistoryHit = { session: { kind: "vatra" | "imported"; id: number }; title: string; snippet: string; at: string };
export type ImportedConversation = { id: number; origin: string; title: string; cwd: string | null; project_id: number | null; messages: { role: string; text: string; at: string }[] };

/** One suggestion for `/`, `$` or `@`. `insert` replaces the typed trigger and query. */
export type CompletionItem = {
  label: string;
  insert: string;
  detail: string;
  kind: "skill" | "command" | "prompt" | "file" | "resource";
};

export type Session = {
  options?: TurnOptions;
  extension_route?: Route;
  id: number;
  project_id: number | null;
  agent: string;
  title: string;
  updated_seq: number;
  last_turn: LastTurn;
  running: boolean;
};

/** Agent events. New kinds may arrive before the app knows them, hence the open `type`. */
export type AgentEvent =
  | { type: "user_message"; text: string }
  | { type: "assistant_text"; text: string }
  /** Streamed text of a running turn. It has no seq and is never journaled. */
  | { type: "assistant_delta"; text: string }
  | { type: "tool_call"; id: string; name: string; arguments: unknown }
  | { type: "tool_result"; id: string; output: string; is_error: boolean }
  | { type: "tool_denied"; id: string; name: string }
  | { type: "turn_completed"; agent: string }
  | { type: "turn_failed"; agent: string; message: string }
  | { type: "handoff_started"; from_session: number; to_agent: string; brief: HandoffBrief }
  | { type: "handoff_result"; to_session: number; text: string }
  | { type: "unmapped"; source: string; raw: string }
  | { type: "rule_added"; label: string; text: string }
  | { type: "skill_loaded"; name: string; text: string }
  /** Deferred MCP tools that ToolSearch found. They join the tool list for the next steps, with no row shown. */
  | { type: "tools_loaded"; names: string[] }
  | { type: "background_result"; id: string; text: string }
  | { type: "compacted"; summary: string }
  | { type: "native_session"; id: string }
  | { type: "warning"; source: string; message: string }
  | { type: "nested"; parent_id: string; event: AgentEvent }
  | { type: "subagent_started"; id: string; description: string }
  | { type: "subagent_finished"; id: string; status: string }
  | { type: "hook"; name: string; phase: string; output: string | null }
  | { type: "usage"; input_tokens: number; output_tokens: number; cost_usd: number | null }
  | { type: "rate_limit"; status: string; resets_at: number | null }
  | { type: "catalog"; skills: number; agents: number; plugins: number; mcp_servers: number };

export type HandoffBrief = {
  summary: string;
  files: string[];
  decisions: string[];
  remaining: string[];
  degraded: boolean;
};

export type StoredEvent = { seq: number; session_id: number; event: AgentEvent };

export type ApprovalRequest = { id: string; session_id: number; tool: string; arguments: unknown };

/** `questions` is the list the agent sent to AskUserQuestion, passed on unchanged. */
export type QuestionRequest = { id: string; session_id: number; questions: unknown };

export type QuestionItem = {
  question: string;
  header?: string;
  options?: { label: string; description?: string }[];
  multiSelect?: boolean;
};

export type AgentKind = "model" | "claude" | "acp";
export type AgentView = { name: string; kind: AgentKind; source: "app" | "file"; provider: string | null; model: string | null; hasKey: boolean };
export type Provider = { id: string; name: string; baseUrl: string | null; driver: string; modelsUrl: string | null; keyUrl: string | null };
export type ProviderModel = { id: string; name: string; tools: boolean; free: boolean; contextLength: number | null };
export type DetectedAgents = { claude: string | null; codex: string | null; node: string | null; acpScripts: [string, string][] };
export type AgentForm =
  | { kind: "model"; name: string; provider: string; baseUrl?: string; model: string; key?: string }
  | { kind: "claude"; binary: string }
  | { kind: "acp"; name: string; node: string; script?: string; args?: string[]; startupTimeoutSec?: number; turnTimeoutSec?: number; subscription: boolean; managedAccount?: boolean; codexBinary?: string | null };
export type SaveResult = { restartNeeded: boolean; runningTurns: number };
export type DepPackage = { id: string; name: string; provides: string[]; purpose: "localModels" | "extensions"; installed: boolean; command: string };
export type ParityRun = { agent: string; native: string | null; model: string; build: string; startedAt: number; results: { taskId: string; outcome: { passed: boolean; failures: string[] }; metrics: { toolCalls: number; toolErrors: number; seconds: number; inputTokens: number; outputTokens: number; costUsd: number | null } }[] };

export type Notification =
  | { method:"workspace_changed"; params:{project_id:number;item_ids:number[]} }
  | { method:"workspace_proposals_changed"; params:{project_id:number;target_id:number|null} }
  | { method:"reminders_changed"; params:{project_id:number} }
  | { method:"schedule_changed"; params:Record<string,never> }
  | { method:"calendar_changed"; params:Record<string,never> }
  | { method:"research_changed"; params:{id:number} }
  | { method: "parity_progress"; params: { agent: string; task: string; done: number; total: number } }
  | {method:"chatgpt_login";params:{ok:boolean;message:string}}
  | { method: "import_progress"; params: { done: number; total: number; label: string } }
  | { method: "event"; params: StoredEvent }
  | { method: "turn_state"; params: { session_id: number; running: boolean } }
  | { method: "approval"; params: ApprovalRequest }
  | { method: "approval_resolved"; params: { id: string } }
  | { method: "question"; params: QuestionRequest }
  | { method: "question_resolved"; params: { id: string } }
  /** The event at `seq` was too big to send; it is fetched with sessions.events. */
  | { method: "event_too_large"; params: { session_id: number; seq: number } }
  | { method: "problem"; params: { session_id: number; message: string } }
  /** The address of an MCP login, which the app opens in the browser. */
  | { method: "open_url"; params: { url: string } }
  /** The result of an MCP login that was started with mcp.login. */
  | { method: "mcp_login"; params: { server: string; ok: boolean; message: string } }
  | { method: "model_download"; params: ModelDownloadNotice }
  | { method: "model_check"; params: { agent: string; step: CheckStep; outcome: StepOutcome | null } }
  | { method: "deps_install"; params: { id: string; state: "started" | "progress" | "done" | "failed"; line: string | null } }
  | { method: "model_server"; params: { slug: string; state: "starting" | "ready" | "stopped" | "failed"; detail: string | null } };

export type Label = "complete" | "partial" | "chatOnly" | "notChecked";
export type LabelReason = "toolsUnreliable" | "mcpFailed" | "contextSmall" | "noTools" | "slow" | "templateHasNoTools" | "noChatTemplate" | "notEnoughMemory";
export type Fit = "gpu" | "split" | "cpuOnly" | "tooLarge";
export type Task = "everyday" | "coding" | "longDocuments" | "vision";
export type CheckStep = "load" | "speed" | "tools" | "mcp" | "done";
export type StepOutcome = "passed" | { failed: { reason: string } } | { skipped: { reason: string } };
export type Estimate = { needed: number; weights: number; kv: number; overhead: number; ctx: number; fit: Fit; notes: string[] };
export type Gpu = { name: string; backend: string | null; total: number; free: number | null; source: "llama" | "system"; integrated: boolean };
export type HardwareReport = { cpu: { name: string | null; cores: number | null; threads: number | null }; ram: { total: number; available: number }; gpus: Gpu[]; diskFree: number | null };
export type LlamaStatus = { installed: true; install: { server: string; build: number | null; caps: { fit: boolean } } } | { installed: false; installCommand: string };
export type CheckResults = { speed: { promptTps: number | null; genTps: number | null; seconds: number } | null; tools: { passed: number; of: number }; toolFailures: string[] };
export type LabelView = { agent: string; label: Label; reasons: LabelReason[]; estimated: boolean; checkedAt: number | null; llamaBuild: number | null; stale: boolean; results: CheckResults | null };
export type InstalledModel = { slug: string; agent: string; repo: string | null; variant: string | null; sizeBytes: number; running: boolean; missing: boolean };
export type RepoSummary = { id: string; downloads: number; likes: number; gated: boolean };
export type VariantView = { name: string; size: number; mmproj: { path: string; size: number }[]; estimate: Estimate; lowQuality: boolean };
export type RepoView = { repo: string; gated: boolean; ctx: number; variants: VariantView[]; label: Label; reasons: LabelReason[] };
export type Suggestion = { family: string; repo: string; variant: string; size: number; estimate: Estimate; alternatives: [string, string][]; unlockVramGib: number | null; lowQuality: boolean; note: string };
export type ModelDownloadNotice = { state: "progress" | "done" | "failed" | "cancelled"; repo: string; variant: string; progress?: { bytes: number; total: number; file: string }; message?: string; slug?: string };

export type ConnectionStatus = "connecting" | "connected" | "reconnecting";

export const ERROR_BUSY = 1001;

export type TurnOptions = { model?: string; effort?: string };
export type ModelChoice = { id: string; name: string; efforts?: { id: string; name: string }[] };
export type AgentOptions = { models: ModelChoice[]; current_model?: string | null; efforts: { id: string; name: string }[]; current_effort?: string | null };
