export type Schedule =
  | {kind: "once"; at: number}
  | {kind: "interval"; every_secs: number}
  | {kind: "daily"; hour: number; minute: number; tz_offset_min: number}
  | {kind: "weekly"; days: number[]; hour: number; minute: number; tz_offset_min: number};
export type ScheduledTask = {id: number; name: string; prompt: string; agent: string; project_id: number | null; schedule: Schedule; enabled: boolean; next_run_at: number | null; last_run_at: number | null; created_at: number; kind: "agent" | "research"; options: {depth?: string; small_model?: boolean}};
export type ScheduledRun = {id: number; task_id: number; scheduled_for: number; started_at: number; finished_at: number | null; state: "running" | "done" | "failed" | "interrupted"; session_id: number | null; summary: string | null; research_id: number | null};
export type Occurrence = {event_id: number; title: string; location: string; notes: string; calendar: string; starts_at: number; ends_at: number; all_day: boolean; recurring: boolean};
export type CalendarEvent = {id: number; calendar: string; uid: string; title: string; notes: string; location: string; starts_at: number; ends_at: number; all_day: boolean; rrule: string | null; source: string};
export type Account = {id: string; name: string; address?: string};
export type MailHeader = {uid: number; key: string; from: string; subject: string; date: string; seen: boolean; flagged: boolean; labels: string[]; triage: string | null; summary: string | null; category?: string};
export type MailAttachment = {name: string; type: string; size: number; preview: string | null};
export type MailMessage = {uid: number; key: string; from: string; to: string; subject: string; date: string; text: string; html?: string; attachments?: MailAttachment[]; category?: string; message_id: string; untrusted: true; labels: string[]; triage: string | null; summary: string | null};
export type Draft = {id: number; account: string; in_reply_to: string | null; to: string[]; subject: string; body: string; state: "draft" | "sending" | "sent" | "failed"; error: string | null; fingerprint: string; updated_at: number};
export type ResearchSource = {n: number; url: string; title: string; status: "read" | "not relevant" | "unreadable"; kind?: "web" | "pdf" | "video" | "paper"; round?: number; notes?: string; reason?: string};
export type ResearchStats = {rounds?: number; queries?: number; providers?: Record<string, number>; search_errors?: string[]; kinds?: Record<string, number>; hosts?: number; sources?: {read: number; not_relevant: number; unreadable: number}; partial?: boolean; small_model?: boolean};
export type ResearchRun = {id: number; question: string; depth: string; state: "planning" | "searching" | "reading" | "writing" | "done" | "failed" | "cancelled"; step: string; plan: string[]; sources: ResearchSource[]; report: string | null; followups: {question: string; answer: string}[]; error: string | null; created_at: number; finished_at: number | null; stats: ResearchStats; options: {small_model?: boolean; context?: string}};
export type ResearchSummary = Pick<ResearchRun, "id" | "question" | "depth" | "state" | "step" | "created_at" | "finished_at">;
export type CompareAnswer = {label: string; text: string | null; error: string | null; agent?: string};
export type CompareRun = {id: number; prompt: string; answers: CompareAnswer[]; vote: string | null; created_at: number};
export type ConnectorView = {id: string; kind: string; name: string; settings: Record<string, unknown>; hasSecret: Record<string, boolean>};
export type HealthItem = {id: string; group: "accounts" | "search" | "models"; label: string; kind: string; state: "ok" | "down" | "unknown"; detail: string; at: number | null; retry: {method: string; params: Record<string, unknown>} | null};
export type HealthStatus = {items: HealthItem[]; down: number};
export type ResearchProfile = {tier: "small" | "medium" | "large" | "unknown"; small_model: boolean; reason: string};
