import type { AgentEvent, HandoffBrief, StoredEvent } from "../rpc/types";

const SUMMARY_KEYS = ["command", "file_path", "path", "notebook_path", "query", "pattern", "url", "description"];
const SUMMARY_MAX_CHARS = 120;
const CAP_CHARS = 20_000;

export type WorkItem = {
  id: string;
  name: string;
  arguments: unknown;
  output?: string;
  isError: boolean;
  denied: boolean;
  /** The turn ended before this tool reported a result. */
  interrupted: boolean;
  description?: string;
  status?: string;
  children: WorkItem[];
  texts: string[];
};

export type Block =
  | { kind: "user"; seq: number; text: string }
  | { kind: "assistant"; seq: number; text: string }
  | { kind: "work"; seq: number; items: WorkItem[]; technical: string[] }
  | { kind: "handoff"; seq: number; agent: string; brief: HandoffBrief }
  | { kind: "handoff_result"; seq: number; text: string }
  | { kind: "failure"; seq: number; message: string }
  | { kind: "warning"; seq: number; message: string }
  | { kind: "limit"; seq: number; status: string; resetsAt: number | null }
  /** Events that say something happened without asking anything of the user. */
  | {
      kind: "quiet";
      seq: number;
      event: Extract<AgentEvent, { type: "compacted" | "background_result" | "rule_added" | "skill_loaded" }>;
    }
  | { kind: "technical"; seq: number; lines: string[] };

/** Agent output can be megabytes, and a tree of text nodes that size freezes the window. */
export function capText(text: string): string {
  return text.length > CAP_CHARS ? `${text.slice(0, CAP_CHARS)}…` : text;
}

export function toolSummary(args: unknown): string {
  if (args === null || typeof args !== "object") {
    return "";
  }
  const record = args as Record<string, unknown>;
  const key = SUMMARY_KEYS.find((candidate) => typeof record[candidate] === "string");
  if (!key) {
    return "";
  }
  const text = record[key] as string;
  return text.length > SUMMARY_MAX_CHARS ? `${text.slice(0, SUMMARY_MAX_CHARS)}…` : text;
}

/** Tool calls by id, so a long session doesn't search every block for each result. */
type Grouping = { blocks: Block[]; items: Map<string, WorkItem> };

function newItem(grouping: Grouping, id: string, name: string, args: unknown): WorkItem {
  const item: WorkItem = { id, name, arguments: args, isError: false, denied: false, interrupted: false, children: [], texts: [] };
  grouping.items.set(id, item);
  return item;
}

function openWork({ blocks }: Grouping, seq: number): Extract<Block, { kind: "work" }> {
  const last = blocks.at(-1);
  if (last?.kind === "work") {
    return last;
  }
  const block: Block = { kind: "work", seq, items: [], technical: [] };
  blocks.push(block);
  return block;
}

function addTechnical({ blocks }: Grouping, seq: number, line: string): void {
  const last = blocks.at(-1);
  if (last?.kind === "work") {
    last.technical.push(line);
  } else if (last?.kind === "technical") {
    last.lines.push(line);
  } else {
    blocks.push({ kind: "technical", seq, lines: [line] });
  }
}

function applyNested(grouping: Grouping, seq: number, parentId: string, event: AgentEvent): void {
  let parent = grouping.items.get(parentId);
  if (!parent) {
    parent = newItem(grouping, parentId, "Agent", {});
    openWork(grouping, seq).items.push(parent);
  }
  switch (event.type) {
    case "tool_call":
      parent.children.push(newItem(grouping, event.id, event.name, event.arguments));
      break;
    case "tool_result": {
      const child = grouping.items.get(event.id);
      if (child) {
        child.output = event.output;
        child.isError = event.is_error;
      } else {
        parent.texts.push(event.output);
      }
      break;
    }
    case "assistant_text":
    case "user_message":
      parent.texts.push(event.text);
      break;
    default:
      parent.texts.push(JSON.stringify(event));
  }
}

function markInterrupted(items: Iterable<WorkItem>): void {
  for (const item of items) {
    if (item.output === undefined && !item.denied) {
      item.interrupted = true;
    }
  }
}

function applyEvent(grouping: Grouping, seq: number, event: AgentEvent): void {
  const { blocks, items } = grouping;
  switch (event.type) {
    case "user_message":
      blocks.push({ kind: "user", seq, text: event.text });
      break;
    case "assistant_text":
      blocks.push({ kind: "assistant", seq, text: event.text });
      break;
    case "tool_call":
      openWork(grouping, seq).items.push(newItem(grouping, event.id, event.name, event.arguments));
      break;
    case "tool_result": {
      const item = items.get(event.id);
      if (item) {
        item.output = event.output;
        item.isError = event.is_error;
      } else {
        addTechnical(grouping, seq, JSON.stringify(event));
      }
      break;
    }
    case "tool_denied": {
      const item = items.get(event.id);
      if (item) {
        item.denied = true;
      } else {
        addTechnical(grouping, seq, JSON.stringify(event));
      }
      break;
    }
    case "nested":
      applyNested(grouping, seq, event.parent_id, event.event);
      break;
    case "subagent_started": {
      const item = items.get(event.id);
      if (item) {
        item.description = event.description;
      } else {
        addTechnical(grouping, seq, JSON.stringify(event));
      }
      break;
    }
    case "subagent_finished": {
      const item = items.get(event.id);
      if (item) {
        item.status = event.status;
      } else {
        addTechnical(grouping, seq, JSON.stringify(event));
      }
      break;
    }
    case "turn_failed":
      markInterrupted(items.values());
      blocks.push({ kind: "failure", seq, message: event.message });
      break;
    case "warning":
      blocks.push({ kind: "warning", seq, message: event.message });
      break;
    case "handoff_started":
      blocks.push({ kind: "handoff", seq, agent: event.to_agent, brief: event.brief });
      break;
    case "handoff_result":
      blocks.push({ kind: "handoff_result", seq, text: event.text });
      break;
    case "usage":
      addTechnical(grouping, seq, JSON.stringify(event));
      break;
    case "rate_limit":
      if (event.status === "allowed") {
        addTechnical(grouping, seq, JSON.stringify(event));
      } else {
        blocks.push({ kind: "limit", seq, status: event.status, resetsAt: event.resets_at });
      }
      break;
    case "turn_completed":
      markInterrupted(items.values());
      break;
    case "unmapped":
      addTechnical(grouping, seq, event.raw);
      break;
    // A load changes what the model can call, and the person has nothing to read in it.
    case "tools_loaded":
      break;
    case "compacted":
    case "background_result":
    case "rule_added":
    case "skill_loaded":
      blocks.push({ kind: "quiet", seq, event });
      break;
    default:
      addTechnical(grouping, seq, JSON.stringify(event));
  }
}

export function groupEvents(events: StoredEvent[]): Block[] {
  const grouping: Grouping = { blocks: [], items: new Map() };
  for (const stored of events) {
    applyEvent(grouping, stored.seq, stored.event);
  }
  return grouping.blocks;
}
