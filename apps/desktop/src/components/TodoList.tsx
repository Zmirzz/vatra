import { Circle, CircleCheck, CircleDot } from "lucide-react";
import { copy } from "../copy";

export type TodoEntry = { text: string; status: string };

const STATUS_ICONS = { completed: CircleCheck, in_progress: CircleDot } as const;

/** TodoWrite sends `todos` and update_plan sends `plan`. A running item shows its present-tense form. */
export function todosFrom(args: unknown): TodoEntry[] {
  if (args === null || typeof args !== "object") {
    return [];
  }
  const record = args as { todos?: unknown; plan?: unknown };
  const list = Array.isArray(record.todos) ? record.todos : Array.isArray(record.plan) ? record.plan : [];
  return list.flatMap((entry): TodoEntry[] => {
    if (entry === null || typeof entry !== "object") {
      return [];
    }
    const item = entry as Record<string, unknown>;
    const status = typeof item.status === "string" ? item.status : "pending";
    const wording = status === "in_progress" && typeof item.activeForm === "string" ? item.activeForm : (item.content ?? item.step);
    return typeof wording === "string" ? [{ text: wording, status }] : [];
  });
}

export function TodoList({ todos }: { todos: TodoEntry[] }) {
  return (
    <ul className="todo-list" aria-label={copy.taskList}>
      {todos.map((todo, index) => {
        const Icon = STATUS_ICONS[todo.status as keyof typeof STATUS_ICONS] ?? Circle;
        return (
          <li key={index} className="todo" data-status={todo.status}>
            <Icon size={14} aria-hidden="true" />
            <span>{todo.text}</span>
          </li>
        );
      })}
    </ul>
  );
}
