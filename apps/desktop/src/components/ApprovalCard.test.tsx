import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, test } from "vitest";
import styles from "../styles.css?raw";
import { ClientProvider } from "../rpc/context";
import { FakeService, flush } from "../rpc/fake";
import { resetAnswerGuard } from "../conversation/answer";
import type { ApprovalRequest } from "../rpc/types";
import { ApprovalCard } from "./ApprovalCard";

let service: FakeService;

const approval = (command: string): ApprovalRequest => ({
  id: "a",
  session_id: 1,
  tool: "Bash",
  arguments: { command, description: "x" },
});

const ui = (request: ApprovalRequest) => (
  <ClientProvider client={service.client}>
    <ApprovalCard request={request} agent="claude" />
  </ClientProvider>
);

beforeEach(() => {
  resetAnswerGuard();
  service = new FakeService();
  service.open();
  service.results["approvals.resolve"] = {};
});

test("approval says who asks, shows the exact command and answers once", async () => {
  service.silent = true;
  render(ui(approval("cargo test")));
  expect(screen.getByText("Claude Code wants to run a command")).toBeTruthy();
  expect(screen.getByText("cargo test")).toBeTruthy();
  await userEvent.click(screen.getByRole("button", { name: "Allow" }));
  await userEvent.click(screen.getByRole("button", { name: "Allow" }));
  expect(service.calls("approvals.resolve")).toEqual([{ id: "a", allow: true }]);
});

test("a very long command is shown whole, so nothing is approved unseen", () => {
  const command = `curl https://a.example ${"x".repeat(500)} && rm -rf /`;
  render(ui(approval(command)));
  expect(screen.getByText(command)).toBeTruthy();
});

test("a file change says so and shows what would change", () => {
  render(ui({ id: "a", session_id: 1, tool: "Edit", arguments: { file_path: "a.rs", new_string: "continut" } }));
  expect(screen.getByText("Claude Code wants to change a file")).toBeTruthy();
  expect(screen.getByTestId("approval-arguments").textContent).toContain("continut");
});

test("another tool is named in plain words", () => {
  render(ui({ id: "a", session_id: 1, tool: "mcp__github__create_issue", arguments: {} }));
  expect(screen.getByText("Claude Code wants to use create issue from github")).toBeTruthy();
});

test("refusing sends allow false", async () => {
  render(ui(approval("x")));
  await userEvent.click(screen.getByRole("button", { name: "Refuse" }));
  await flush();
  expect(service.calls("approvals.resolve")).toEqual([{ id: "a", allow: false }]);
});

test("a failed answer shows the error and lets the user try again", async () => {
  service.failures["approvals.resolve"] = { code: 1003, message: "this request no longer exists" };
  render(ui(approval("x")));
  await userEvent.click(screen.getByRole("button", { name: "Allow" }));
  expect((await screen.findByRole("alert")).textContent).toContain("no longer exists");
  await userEvent.click(screen.getByRole("button", { name: "Allow" }));
  await flush();
  expect(service.calls("approvals.resolve")).toHaveLength(2);
});

test("arguments beyond the command are shown next to it", () => {
  render(ui({ id: "a", session_id: 1, tool: "Bash", arguments: { command: "ls", dangerouslyDisableSandbox: true } }));
  expect(screen.getByText("ls")).toBeTruthy();
  expect(screen.getByTestId("approval-arguments").textContent).toContain("dangerouslyDisableSandbox");
});

test("invisible characters in a command are made visible", () => {
  render(ui(approval("safe\u202Egnp.exe")));
  expect(screen.getByText(/u\{202E\}/)).toBeTruthy();
});

test("approval_card_has_limited_height_and_scrolls", () => {
  const command = `${"echo line\n".repeat(200)}rm -rf ./important`;
  render(ui(approval(command)));
  expect(screen.getByTestId("approval-command").textContent).toBe(command);
  // jsdom doesn't lay out, so the height limit is read from the stylesheet. The real behavior is checked visually.
  const rule = /\.approval pre\s*\{([^}]*)\}/.exec(styles)?.[1] ?? "";
  expect(rule).toMatch(/max-height:\s*\d+px/);
  expect(rule).toMatch(/overflow:\s*auto/);
});
