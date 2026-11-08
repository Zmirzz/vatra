import { render, screen } from "@testing-library/react";
import { expect, test } from "vitest";
import { ImportedSession } from "./ImportedSession";
test("imported conversations display their source and messages without an editable reply", () => {
  render(<ImportedSession session={{ id: 9, origin: "codex", title: "Old task", cwd: "C:/project", project_id: null, messages: [{ role: "user", text: "Find the issue", at: "2026-01-01" }, { role: "assistant", text: "Here is the answer", at: "2026-01-01" }] }} />);
  expect(screen.getByText(/read.only/i)).toBeTruthy();
  expect(screen.getByText("Find the issue")).toBeTruthy();
  expect(screen.getByText("Here is the answer")).toBeTruthy();
  expect(screen.queryByRole("textbox")).toBeNull();
  expect(screen.queryByRole("button", { name: "Send" })).toBeNull();
});
