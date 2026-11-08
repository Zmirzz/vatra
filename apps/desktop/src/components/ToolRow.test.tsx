import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";
import type { WorkItem } from "../conversation/group";
import { ToolRow } from "./ToolRow";

const item = (name: string, args: unknown, output?: string): WorkItem => ({
  id: "1",
  name,
  arguments: args,
  output,
  isError: false,
  denied: false,
  interrupted: false,
  children: [],
  texts: [],
});

test("an edit shows a colored diff of the change", async () => {
  const { container } = render(
    <ToolRow item={item("Edit", { file_path: "a.rs", old_string: "let x = 1;", new_string: "let x = 2;" }, "ok")} />,
  );
  await userEvent.click(screen.getByRole("button", { name: /details/i }));
  expect(container.querySelector(".diff-remove")?.textContent).toContain("let x = 1;");
  expect(container.querySelector(".diff-add")?.textContent).toContain("let x = 2;");
});

test("a shell alias shows its command and output like Bash", async () => {
  render(<ToolRow item={item("exec_command", { command: "ls -la", description: "list files" }, "a.txt")} />);
  expect(screen.getByText("Ran a command")).toBeTruthy();
  await userEvent.click(screen.getByRole("button", { name: /details/i }));
  expect(screen.getByText("ls -la", { selector: "pre" })).toBeTruthy();
  expect(screen.getByTestId("tool-output").textContent).toBe("a.txt");
});

test("a read names its file in the summary", () => {
  render(<ToolRow item={item("Read", { file_path: "src/main.rs" }, "fn main() {}")} />);
  expect(screen.getByText("Read a file")).toBeTruthy();
  expect(screen.getByText("src/main.rs")).toBeTruthy();
});

test("a todo list shows each item with its status", async () => {
  render(
    <ToolRow
      item={item("TodoWrite", {
        todos: [
          { content: "Read the spec", status: "completed", activeForm: "Reading the spec" },
          { content: "Write the code", status: "in_progress", activeForm: "Writing the code" },
          { content: "Run tests", status: "pending", activeForm: "Running tests" },
        ],
      })}
    />,
  );
  await userEvent.click(screen.getByRole("button", { name: /details/i }));
  expect(screen.getByText("Read the spec")).toBeTruthy();
  expect(screen.getByText("Writing the code")).toBeTruthy();
  expect(screen.getByText("Run tests")).toBeTruthy();
  expect(screen.getByRole("list", { name: "Task list" }).querySelectorAll("li")).toHaveLength(3);
});

test("arguments the edit view doesn't show stay visible in the details", async () => {
  render(<ToolRow item={item("Edit", { file_path: "a.rs", old_string: "x", new_string: "y", replace_all: true }, "ok")} />);
  await userEvent.click(screen.getByRole("button", { name: /details/i }));
  expect(screen.getByTestId("tool-arguments").textContent).toContain("replace_all");
  expect(screen.getByTestId("tool-arguments").textContent).not.toContain("new_string");
});

test("an update_plan alias shows its steps as a checklist", async () => {
  render(<ToolRow item={item("update_plan", { plan: [{ step: "Find the bug", status: "completed" }] })} />);
  await userEvent.click(screen.getByRole("button", { name: /details/i }));
  expect(screen.getByText("Find the bug")).toBeTruthy();
});
