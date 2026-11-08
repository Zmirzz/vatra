import { create } from "zustand";
import { copy } from "../copy";
import type { RpcClient } from "../rpc/client";
import { openLoginAddress } from "./openLogin";
import type { ApprovalRequest, CatalogEntry, ConnectionStatus, ImportedConversation, Notification, PermissionMode, Project, QuestionRequest, Session, StoredEvent } from "../rpc/types";

const UNTITLED = "New session";
const TITLE_MAX_CHARS = 60;
const NO_PROBLEMS: string[] = [];

export type AppState = {
  subscriptionAgents:string[];
  importedSession: ImportedConversation | null;
  /** A Settings page another screen asked to open, like Models from the start screen. */
  settingsPage: "general" | "agents" | "import" | "extensions" | "account" | "models" | "market" | "acp-market" | "mcp-market" | "mcp-installed" | "skills" | "accounts" | "status" | null;
  setupRequested: boolean;
  preferredAgent: string | null;
  openingImportedId: number | null;
  catalog: CatalogEntry[];
  projects: Project[];
  sessions: Session[];
  agents: string[];
  /** False until the service first answers, so an empty list isn't taken for "no agents". */
  agentsLoaded: boolean;
  eventsBySession: Record<number, StoredEvent[]>;
  /** Sessions whose full history was fetched. Live events alone don't make a session loaded. */
  loadedSessions: number[];
  approvals: ApprovalRequest[];
  questions: QuestionRequest[];
  /** Text streamed by a turn that hasn't been journaled yet, per session. */
  streaming: Record<number, string>;
  /** Service messages about a session. They aren't journaled, so they last until a reload. */
  problems: Record<number, string[]>;
  activeSessionId: number | null;
  status: ConnectionStatus;
  drafts: Record<number, string>;
  modes: Record<number, PermissionMode>;
  turnOptions: Record<number, import("../rpc/types").TurnOptions>;
};

export const initialState = (): AppState => ({
  subscriptionAgents:["claude"],
  importedSession: null,
  settingsPage: null,
  setupRequested: false,
  preferredAgent: null,
  openingImportedId: null,
  catalog: [],
  projects: [],
  sessions: [],
  agents: [],
  agentsLoaded: false,
  eventsBySession: {},
  loadedSessions: [],
  approvals: [],
  questions: [],
  streaming: {},
  problems: {},
  activeSessionId: null,
  status: "connecting",
  drafts: {},
  modes: {},
  turnOptions: {},
});

export const useAppStore = create<AppState>(() => initialState());

/** Events arrive from two sources during a reconnect, so `seq` decides what's new. */
export function mergeEvents(existing: StoredEvent[], incoming: StoredEvent[]): StoredEvent[] {
  if (incoming.length === 0) {
    return existing;
  }
  const last = existing.at(-1)?.seq ?? 0;
  if (incoming.every((event, index) => event.seq > (incoming[index - 1]?.seq ?? last))) {
    return [...existing, ...incoming];
  }
  const bySeq = new Map(existing.map((event) => [event.seq, event]));
  incoming.forEach((event) => bySeq.set(event.seq, event));
  return [...bySeq.values()].sort((a, b) => a.seq - b.seq);
}

export function sessionGroups(state: Pick<AppState, "sessions" | "approvals" | "questions">) {
  const waitingIds = new Set([...state.approvals, ...state.questions].map((request) => request.session_id));
  const sorted = [...state.sessions].sort((a, b) => b.updated_seq - a.updated_seq);
  return {
    waiting: sorted.filter((session) => waitingIds.has(session.id)),
    working: sorted.filter((session) => session.running && !waitingIds.has(session.id)),
    done: sorted.filter((session) => !session.running && !waitingIds.has(session.id)),
  };
}

function updateFromEvent(session: Session, stored: StoredEvent): Session {
  const event = stored.event;
  const next = { ...session, updated_seq: Math.max(session.updated_seq, stored.seq) };
  if (event.type === "user_message") {
    next.last_turn = "unfinished";
    if (session.title === UNTITLED) {
      next.title = [...event.text].slice(0, TITLE_MAX_CHARS).join("");
    }
  } else if (event.type === "turn_completed") {
    next.last_turn = "completed";
  } else if (event.type === "turn_failed") {
    next.last_turn = "failed";
  }
  return next;
}

/** The final text replaces what was streamed, and a finished turn leaves nothing half-written behind. */
const ENDS_STREAMING = ["assistant_text", "turn_completed", "turn_failed"];

function withoutStreaming(streaming: Record<number, string>, sessionId: number): Record<number, string> {
  const { [sessionId]: _dropped, ...rest } = streaming;
  return rest;
}

export function applyNotification(state: AppState, notification: Notification): Partial<AppState> {
  switch (notification.method) {
    case "event": {
      const stored = notification.params;
      if (stored.event.type === "assistant_delta") {
        // Transient: it has no seq, so it never touches the journal or the session order.
        return { streaming: { ...state.streaming, [stored.session_id]: (state.streaming[stored.session_id] ?? "") + stored.event.text } };
      }
      const existing = state.eventsBySession[stored.session_id] ?? [];
      return {
        eventsBySession: { ...state.eventsBySession, [stored.session_id]: mergeEvents(existing, [stored]) },
        sessions: state.sessions.map((session) => (session.id === stored.session_id ? updateFromEvent(session, stored) : session)),
        ...(ENDS_STREAMING.includes(stored.event.type) ? { streaming: withoutStreaming(state.streaming, stored.session_id) } : {}),
      };
    }
    case "turn_state": {
      const { session_id, running } = notification.params;
      return {
        sessions: state.sessions.map((session) => (session.id === session_id ? { ...session, running } : session)),
        ...(running ? {} : { streaming: withoutStreaming(state.streaming, session_id) }),
      };
    }
    case "approval": {
      const others = state.approvals.filter((approval) => approval.id !== notification.params.id);
      return { approvals: [...others, notification.params] };
    }
    case "approval_resolved":
      return { approvals: state.approvals.filter((approval) => approval.id !== notification.params.id) };
    case "question": {
      const others = state.questions.filter((request) => request.id !== notification.params.id);
      return { questions: [...others, notification.params] };
    }
    case "question_resolved":
      return { questions: state.questions.filter((request) => request.id !== notification.params.id) };
    case "problem": {
      const { session_id, message } = notification.params;
      const listed = state.problems[session_id] ?? NO_PROBLEMS;
      if (listed.includes(message)) {
        return {};
      }
      return { problems: { ...state.problems, [session_id]: [...listed, message] } };
    }
    default:
      return {};
  }
}

async function loadEvents(client: RpcClient, sessionId: number, afterSeq: number): Promise<void> {
  const events = await client.call<StoredEvent[]>("sessions.events", { session_id: sessionId, after_seq: afterSeq });
  // Once the journal holds the final text, the streamed copy of it is dropped so it isn't shown twice.
  const ended = events.some((stored) => ENDS_STREAMING.includes(stored.event.type));
  useAppStore.setState((state) => ({
    eventsBySession: {
      ...state.eventsBySession,
      [sessionId]: mergeEvents(state.eventsBySession[sessionId] ?? [], events),
    },
    loadedSessions: state.loadedSessions.includes(sessionId) ? state.loadedSessions : [...state.loadedSessions, sessionId],
    ...(ended ? { streaming: withoutStreaming(state.streaming, sessionId) } : {}),
  }));
}

type MissingEvent = { session_id: number; seq: number };

/** An event too big for the socket comes back from the seq before it. The session stays as loaded as it was. */
async function loadTooLarge(client: RpcClient, { session_id: sessionId, seq }: MissingEvent): Promise<void> {
  const events = await client.call<StoredEvent[]>("sessions.events", { session_id: sessionId, after_seq: seq - 1 });
  useAppStore.setState((state) => ({
    eventsBySession: {
      ...state.eventsBySession,
      [sessionId]: mergeEvents(state.eventsBySession[sessionId] ?? [], events),
    },
  }));
}

function setMissingProblem(sessionId: number, listed: boolean): void {
  useAppStore.setState((state) => {
    const message = copy.eventMissing;
    if (listed) {
      return applyNotification(state, { method: "problem", params: { session_id: sessionId, message } });
    }
    return { problems: { ...state.problems, [sessionId]: (state.problems[sessionId] ?? []).filter((text) => text !== message) } };
  });
}

/** Everything a client could have missed while disconnected comes back from here. */
async function resync(client: RpcClient, resumeFrom: Map<number, number>): Promise<void> {
  const [projects, sessions, agents, approvals, questions] = await Promise.all([
    client.call<Project[]>("projects.list", {}),
    client.call<Session[]>("sessions.list", {}),
    client.call<string[]>("agents.list", {}),
    client.call<ApprovalRequest[]>("approvals.list", {}),
    client.call<QuestionRequest[]>("questions.list", {}),
  ]);
  // Deltas sent before the drop are gone; the final text comes back from the journal instead.
  useAppStore.setState({ projects, sessions, agents, agentsLoaded: true, approvals, questions, streaming: {} });
  client.call<CatalogEntry[]>("catalog.list", {}).then((catalog) => useAppStore.setState({ catalog })).catch(() => undefined);
  client.call<{name:string;subscription:boolean}[]>("agents.describe",{}).then(items=>{if(Array.isArray(items))useAppStore.setState({subscriptionAgents:items.filter(item=>item.subscription).map(item=>item.name)});}).catch(()=>undefined);
  await Promise.all(useAppStore.getState().loadedSessions.map((sessionId) => loadEvents(client, sessionId, resumeFrom.get(sessionId) ?? 0)));
}

export function attachClient(client: RpcClient): void {
  // Too-big events that couldn't be fetched. They are tried again on the next connection.
  let missing: MissingEvent[] = [];
  const fetchTooLarge = (item: MissingEvent) =>
    loadTooLarge(client, item).then(
      () => setMissingProblem(item.session_id, false),
      () => {
        missing.push(item);
        setMissingProblem(item.session_id, true);
      },
    );
  client.on((notification) => {
    useAppStore.setState((state) => applyNotification(state, notification));
    if (notification.method === "event_too_large") {
      fetchTooLarge(notification.params);
    }
    if (notification.method === "open_url") {
      openLoginAddress(notification.params.url);
    }
  });
  // Taken while disconnected: live events start flowing as soon as the socket opens, so the
  // newest seq at resync time would skip whatever was missed during the outage.
  let resumeFrom = new Map<number, number>();
  client.onStatus((status) => {
    useAppStore.setState({ status });
    if (status === "connected") {
      resync(client, resumeFrom)
        .catch(() => undefined)
        .then(() => {
          const retry = missing;
          missing = [];
          retry.forEach((item) => void fetchTooLarge(item));
        });
    } else {
      const { loadedSessions, eventsBySession } = useAppStore.getState();
      resumeFrom = new Map(loadedSessions.map((id) => [id, eventsBySession[id]?.at(-1)?.seq ?? 0]));
    }
  });
}

export async function refreshSessions(client: RpcClient): Promise<void> {
  const sessions = await client.call<Session[]>("sessions.list", {});
  useAppStore.setState({ sessions });
}

export function openSession(client: RpcClient, sessionId: number): void {
  useAppStore.setState({ activeSessionId: sessionId, importedSession: null, openingImportedId: null });
  if (!useAppStore.getState().loadedSessions.includes(sessionId)) {
    loadEvents(client, sessionId, 0).catch(() => undefined);
  }
}
