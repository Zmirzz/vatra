import { copy } from "../copy";

const AGENT_LABELS: Record<string, string> = { claude: "Claude Code" };
const PROJECT_TINTS = ["#4f46e5", "#0f766e", "#b45309", "#be185d", "#0369a1", "#6d28d9", "#15803d", "#c2410c"];
// Names the engine uses for the same tools. Claude Code sends the canonical names.
const TOOL_ALIASES: Record<string, string> = { shell: "Bash", exec_command: "Bash", update_plan: "TodoWrite" };
const COMMAND_TOOLS = ["Bash", "PowerShell"];
const EDIT_TOOLS = ["Edit", "Write", "NotebookEdit"];
const MAX_INLINE_COMMAND = 80;
const QUIET_ARGUMENTS = ["command", "description", "timeout"];
// Control, zero-width and text-direction characters can make a command look like another.
const HIDDEN_CHARACTERS = /[\p{Cc}\p{Cf}]/gu;

export const agentLabel = (agent: string) => AGENT_LABELS[agent] ?? agent;

export const projectTint = (id: number) => PROJECT_TINTS[id % PROJECT_TINTS.length];

/** The name a tool goes by once aliases are resolved, so one view serves both spellings. */
export function canonicalTool(tool: string): string {
  return TOOL_ALIASES[tool] ?? tool;
}

export function isCommandTool(tool: string): boolean {
  return COMMAND_TOOLS.includes(canonicalTool(tool));
}

export function toolWords(tool: string): { name: string; source: string | null } {
  const match = /^mcp__(.+?)__(.+)$/.exec(tool);
  return match ? { name: match[2].replaceAll("_", " "), source: match[1] } : { name: tool, source: null };
}

function namedTool(tool: string): string {
  const { name, source } = toolWords(tool);
  return copy.tools.other(name, source);
}

export function toolLabel(tool: string): string {
  const name = canonicalTool(tool);
  if (COMMAND_TOOLS.includes(name)) return copy.tools.command;
  if (EDIT_TOOLS.includes(name)) return copy.tools.edit;
  switch (name) {
    case "Read":
      return copy.tools.read;
    case "Grep":
    case "Glob":
      return copy.tools.search;
    case "WebFetch":
    case "WebSearch":
      return copy.tools.web;
    case "Skill":
      return copy.tools.skill;
    case "TaskCreate":
    case "TaskUpdate":
    case "TodoWrite":
      return copy.tools.tasks;
    case "AskUserQuestion":
      return copy.tools.ask;
    case "ToolSearch":
      return copy.tools.prepare;
    case "Agent":
    case "Task":
      return copy.subagent;
    default:
      return namedTool(name);
  }
}

export function approvalHeading(agent: string, tool: string): string {
  const name = agentLabel(agent);
  const canonical = canonicalTool(tool);
  if (COMMAND_TOOLS.includes(canonical)) return copy.approvals.command(name);
  if (EDIT_TOOLS.includes(canonical)) return copy.approvals.edit(name);
  return copy.approvals.tool(name, namedTool(canonical));
}

export function exactCommand(args: unknown): string | null {
  if (args && typeof args === "object" && typeof (args as { command?: unknown }).command === "string") {
    return (args as { command: string }).command;
  }
  return null;
}

/** Only what's fully visible and has no other effect may be allowed from the list. */
export function commandFitsInline(tool: string, args: unknown): boolean {
  const command = exactCommand(args);
  if (!isCommandTool(tool) || command === null || command.length > MAX_INLINE_COMMAND) {
    return false;
  }
  const keys = Object.keys(args as object);
  return keys.every((key) => QUIET_ARGUMENTS.includes(key)) && !/[\p{Cc}\p{Cf}]| {2}/u.test(command);
}

/** Arguments that change what a command does, beyond the text the user reads. */
export function extraArguments(args: unknown): Record<string, unknown> {
  const entries = Object.entries(args as object).filter(([key]) => !QUIET_ARGUMENTS.includes(key));
  return Object.fromEntries(entries);
}

export function visibleText(text: string): string {
  return text.replace(HIDDEN_CHARACTERS, (character) =>
    character === "\n" || character === "\t" ? character : `\\u{${character.codePointAt(0)?.toString(16).toUpperCase()}}`,
  );
}
