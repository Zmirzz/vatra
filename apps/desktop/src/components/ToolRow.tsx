import { ChevronRight } from "lucide-react";
import { useMemo, useState } from "react";
import { lineDiff, type DiffLine } from "../conversation/diff";
import { capText, toolSummary, type WorkItem } from "../conversation/group";
import { canonicalTool, exactCommand, toolLabel } from "../conversation/labels";
import { copy } from "../copy";
import { DiffView } from "./DiffView";
import { questionItems } from "./QuestionCard";
import { TodoList, todosFrom } from "./TodoList";

const PREVIEW_CHARS = 20_000;

const textOf = (value: unknown): string => (typeof value === "string" ? value : "");

function argumentsOf(item: WorkItem): Record<string, unknown> {
  return item.arguments !== null && typeof item.arguments === "object" ? (item.arguments as Record<string, unknown>) : {};
}

/** The change an Edit or Write makes, or null for any other tool. */
function changeOf(item: WorkItem): DiffLine[] | null {
  const args = argumentsOf(item);
  switch (canonicalTool(item.name)) {
    case "Edit":
      return lineDiff(textOf(args.old_string), textOf(args.new_string));
    case "Write":
      return lineDiff("", textOf(args.content));
    default:
      return null;
  }
}

/** Arguments the view above doesn't show, such as replace_all or timeout, so nothing hides behind a summary. */
function ExtraArguments({ args, shown }: { args: Record<string, unknown>; shown: string[] }) {
  const extra = Object.fromEntries(Object.entries(args).filter(([key]) => !shown.includes(key)));
  if (Object.keys(extra).length === 0) {
    return null;
  }
  return <pre data-testid="tool-arguments">{capText(JSON.stringify(extra, null, 2))}</pre>;
}

/** Each tool gets the view that fits its arguments. Anything unknown shows its arguments as they are. */
function ToolDetails({ item, change }: { item: WorkItem; change: DiffLine[] | null }) {
  const args = argumentsOf(item);
  switch (canonicalTool(item.name)) {
    case "Edit":
      return (
        <>
          {change ? <DiffView lines={change} /> : null}
          <ExtraArguments args={args} shown={["file_path", "old_string", "new_string"]} />
        </>
      );
    case "Write":
      return (
        <>
          {change ? <DiffView lines={change} /> : null}
          <ExtraArguments args={args} shown={["file_path", "content"]} />
        </>
      );
    case "Bash":
    case "PowerShell":
      return (
        <>
          <pre>{capText(exactCommand(args) ?? "")}</pre>
          <ExtraArguments args={args} shown={["command", "description"]} />
        </>
      );
    case "TodoWrite":
      return <TodoList todos={todosFrom(args)} />;
    case "AskUserQuestion":
      return (
        <ul className="question-list">
          {questionItems(args.questions).map((question, index) => (
            <li key={index}>{question.question}</li>
          ))}
        </ul>
      );
    default:
      return <pre>{capText(JSON.stringify(item.arguments, null, 2))}</pre>;
  }
}

export function ToolRow({ item }: { item: WorkItem }) {
  const [open, setOpen] = useState(false);
  const [showAll, setShowAll] = useState(false);
  // The diff is only worked out when the row is opened, and not again for each render.
  const change = useMemo(() => (open ? changeOf(item) : null), [item, open]);
  const summary = item.description ?? toolSummary(item.arguments);
  const output = item.output ?? "";
  const truncated = !showAll && output.length > PREVIEW_CHARS;
  const running = item.output === undefined && !item.denied && !item.interrupted;

  return (
    <div className="step" data-state={item.denied || item.isError ? "problem" : running ? "running" : "done"}>
      <button type="button" className="step-head" aria-label={`${copy.details} ${item.name}`} aria-expanded={open} onClick={() => setOpen(!open)}>
        <ChevronRight className="step-chevron" size={12} aria-hidden="true" />
        <span className="step-name">{toolLabel(item.name)}</span>
        <span className="step-summary">{summary}</span>
        {item.denied ? <span className="step-flag">{copy.denied}</span> : null}
      </button>
      {open ? (
        <div className="step-body">
          {item.texts.map((text, index) => (
            <p key={index}>{capText(text)}</p>
          ))}
          {item.children.map((child) => (
            <ToolRow key={child.id} item={child} />
          ))}
          <ToolDetails item={item} change={change} />
          {item.output !== undefined ? <pre data-testid="tool-output">{truncated ? output.slice(0, PREVIEW_CHARS) : output}</pre> : null}
          {truncated ? (
            <button type="button" className="link" onClick={() => setShowAll(true)}>
              {copy.showAll}
            </button>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
