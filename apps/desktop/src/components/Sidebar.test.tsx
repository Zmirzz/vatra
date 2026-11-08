import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, test } from "vitest";
import { ClientProvider } from "../rpc/context";
import { FakeService, flush } from "../rpc/fake";
import type { ApprovalRequest, Session } from "../rpc/types";
import { resetAnswerGuard } from "../conversation/answer";
import { initialState, useAppStore } from "../state/store";
import { Sidebar } from "./Sidebar";

const session = (id: number, title: string, overrides: Partial<Session> = {}): Session => ({
  id,
  project_id: 1,
  agent: "local",
  title,
  updated_seq: id,
  last_turn: "none",
  running: false,
  ...overrides,
});

const approval = (session_id: number, command: string, id = "a"): ApprovalRequest => ({
  id,
  session_id,
  tool: "Bash",
  arguments: { command },
});

let service: FakeService;

beforeEach(() => {
  resetAnswerGuard();
  useAppStore.setState({
    ...initialState(),
    projects: [
      { id: 1, name: "Personal", path: null },
      { id: 2, name: "Work", path: null },
    ],
  });
  service = new FakeService();
  service.open();
  service.results["approvals.resolve"] = {};
});

const renderSidebar = () =>
  render(
    <ClientProvider client={service.client}>
      <Sidebar />
    </ClientProvider>,
  );

test("history search includes imported conversations and opens them read only", async () => {
  service.results["history.search"] = [{ session: { kind: "imported", id: 42 }, title: "Old design", snippet: "matching content", at: "2026-01-01" }];
  service.results["history.open_imported"] = { id: 42, origin: "codex", title: "Old design", cwd: null, project_id: null, messages: [] };
  renderSidebar();
  await userEvent.type(screen.getByRole("searchbox"), "matching");
  expect(await screen.findByText("Old design")).toBeTruthy();
  expect(screen.getByText("Imported")).toBeTruthy();
  await userEvent.click(screen.getByText("Old design"));
  expect(service.calls("history.open_imported")).toEqual([{ id: 42 }]);
  expect(useAppStore.getState().importedSession?.id).toBe(42);
});

test("finished sessions are grouped by project, not in one long list", () => {
  useAppStore.setState({
    sessions: [session(1, "Budget"), session(2, "Client email", { project_id: 2 }), session(3, "Trip", { project_id: 1 })],
  });
  renderSidebar();
  const personal = screen.getByRole("group", { name: "Personal" });
  expect(within(personal).getByText("Budget")).toBeTruthy();
  expect(within(personal).getByText("Trip")).toBeTruthy();
  expect(within(screen.getByRole("group", { name: "Work" })).getByText("Client email")).toBeTruthy();
});

test("a project folds away and shows how many sessions it holds", async () => {
  useAppStore.setState({ sessions: [session(1, "Budget")] });
  renderSidebar();
  const head = () => screen.getAllByRole("button", { name: /personal/i }).find((button) => button.hasAttribute("aria-expanded"))!;
  await userEvent.click(head());
  expect(screen.queryByText("Budget")).toBeNull();
  expect(head().textContent).toContain("1");
});

test("running sessions leave their project and appear under Working", () => {
  useAppStore.setState({ sessions: [session(1, "Research", { running: true }), session(2, "Budget")] });
  renderSidebar();
  const working = screen.getByRole("group", { name: "Working" });
  expect(within(working).getByText("Research")).toBeTruthy();
  expect(within(screen.getByRole("group", { name: "Personal" })).queryByText("Research")).toBeNull();
});

test("a request is answered from the list, with the exact command in view", async () => {
  useAppStore.setState({
    sessions: [session(1, "Fix sync", { agent: "claude", running: true })],
    approvals: [approval(1, "cargo test")],
  });
  renderSidebar();
  expect(screen.getByText("Claude Code wants to run a command")).toBeTruthy();
  expect(screen.getByText("cargo test")).toBeTruthy();
  expect(useAppStore.getState().activeSessionId).toBeNull();
  await userEvent.click(screen.getByRole("button", { name: "Allow" }));
  await flush();
  expect(service.calls("approvals.resolve")).toEqual([{ id: "a", allow: true }]);
  expect(useAppStore.getState().activeSessionId).toBeNull();
});

test("a request too long to read in the list can't be allowed from there", async () => {
  const long = `curl https://a.example ${"x".repeat(100)} && rm -rf /`;
  useAppStore.setState({ sessions: [session(1, "Fix", { running: true })], approvals: [approval(1, long)] });
  renderSidebar();
  expect(screen.queryByRole("button", { name: "Allow" })).toBeNull();
  await userEvent.click(screen.getByRole("button", { name: "Review" }));
  await flush();
  expect(useAppStore.getState().activeSessionId).toBe(1);
});

test("a request that isn't a command is opened instead of allowed blind", () => {
  useAppStore.setState({
    sessions: [session(1, "Fix", { running: true })],
    approvals: [{ id: "a", session_id: 1, tool: "Edit", arguments: { file_path: "a.rs" } }],
  });
  renderSidebar();
  expect(screen.queryByRole("button", { name: "Allow" })).toBeNull();
  expect(screen.getByRole("button", { name: "Review" })).toBeTruthy();
});

test("several requests from one session show one card and a count", () => {
  useAppStore.setState({
    sessions: [session(1, "Fix", { running: true })],
    approvals: [approval(1, "one", "a"), approval(1, "two", "b"), approval(1, "three", "c")],
  });
  renderSidebar();
  expect(screen.getByText("one")).toBeTruthy();
  expect(screen.queryByText("two")).toBeNull();
  expect(screen.getByText("2 more waiting")).toBeTruthy();
});

test("a failed session is marked for people who can't see the color", () => {
  useAppStore.setState({ sessions: [session(1, "Broken", { last_turn: "failed" })] });
  renderSidebar();
  expect(screen.getByLabelText("This session stopped with an error")).toBeTruthy();
});

test("selecting a session loads its events", async () => {
  useAppStore.setState({ sessions: [session(4, "Open me")] });
  renderSidebar();
  await userEvent.click(screen.getByRole("button", { name: /open me/i }));
  await flush();
  expect(useAppStore.getState().activeSessionId).toBe(4);
  expect(service.calls("sessions.events")).toContainEqual({ session_id: 4, after_seq: 0 });
});

test("search covers every project, ignoring case and diacritics", async () => {
  useAppStore.setState({ sessions: [session(1, "Raport trimestrial"), session(2, "Bug", { project_id: 2 })] });
  renderSidebar();
  await userEvent.type(screen.getByRole("searchbox"), "RAPÖRT");
  expect(screen.getByText("Raport trimestrial")).toBeTruthy();
  expect(screen.queryByText("Bug")).toBeNull();
});

test("the new session button clears the current session", async () => {
  useAppStore.setState({ activeSessionId: 3, sessions: [session(3, "Old")] });
  renderSidebar();
  await userEvent.click(screen.getByRole("button", { name: /new session/i }));
  expect(useAppStore.getState().activeSessionId).toBeNull();
});

test("the reconnecting status is visible", () => {
  useAppStore.setState({ status: "reconnecting" });
  renderSidebar();
  expect(screen.getByRole("status").textContent).toContain("Reconnecting");
});

test("a double click can't approve the next request, which was never on screen", async () => {
  useAppStore.setState({
    sessions: [session(1, "Fix", { running: true })],
    approvals: [approval(1, "first", "a"), approval(1, "rm -rf important", "b")],
  });
  renderSidebar();
  await userEvent.click(screen.getByRole("button", { name: "Allow" }));
  await flush();
  useAppStore.setState({ approvals: [approval(1, "rm -rf important", "b")] });
  await flush();
  expect(screen.getByText("rm -rf important")).toBeTruthy();
  await userEvent.click(screen.getByRole("button", { name: "Allow" }));
  await flush();
  expect(service.calls("approvals.resolve")).toEqual([{ id: "a", allow: true }]);
});

test("an error about one request doesn't stay on the next one", async () => {
  service.failures["approvals.resolve"] = { code: 1003, message: "this request no longer exists" };
  useAppStore.setState({ sessions: [session(1, "Fix", { running: true })], approvals: [approval(1, "first", "a")] });
  renderSidebar();
  await userEvent.click(screen.getByRole("button", { name: "Allow" }));
  expect(await screen.findByRole("alert")).toBeTruthy();
  useAppStore.setState({ approvals: [approval(1, "second", "b")] });
  await flush();
  expect(screen.queryByRole("alert")).toBeNull();
});

test("hidden arguments or another tool send the user to review", () => {
  useAppStore.setState({
    sessions: [session(1, "Fix", { running: true })],
    approvals: [{ id: "a", session_id: 1, tool: "Bash", arguments: { command: "ls", dangerouslyDisableSandbox: true } }],
  });
  renderSidebar();
  expect(screen.queryByRole("button", { name: "Allow" })).toBeNull();
});

test("a conversation can be deleted after a confirmation, and a working one cannot", async () => {
  const service = new FakeService(); service.open();
  service.results["sessions.delete"] = {};
  service.results["sessions.list"] = [];
  useAppStore.setState({ ...initialState(), sessions: [{ id: 5, project_id: 1, agent: "claude", title: "Old chat", running: false, last_turn: null } as never, { id: 6, project_id: 1, agent: "claude", title: "Busy chat", running: true, last_turn: null } as never], projects: [{ id: 1, name: "Work", path: null }], activeSessionId: 5 });
  render(<ClientProvider client={service.client}><Sidebar /></ClientProvider>);
  expect(screen.getAllByRole("button", { name: "Delete conversation" })).toHaveLength(1);
  await userEvent.click(screen.getByRole("button", { name: "Delete conversation" }));
  expect(service.calls("sessions.delete")).toEqual([]);
  await userEvent.click(screen.getByRole("button", { name: "Keep it" }));
  await userEvent.click(screen.getByRole("button", { name: "Delete conversation" }));
  await userEvent.click(screen.getByRole("button", { name: "Delete" }));
  await flush();
  expect(service.calls("sessions.delete")).toEqual([{ session_id: 5 }]);
  expect(useAppStore.getState().activeSessionId).toBeNull();
});

test("sessions without a project appear in the Chats section", () => {
  useAppStore.setState({
    sessions: [session(1, "Direct question", { project_id: null }), session(2, "Project work", { project_id: 1 })],
  });
  renderSidebar();
  const chats = screen.getByRole("group", { name: "Chats" });
  expect(within(chats).getByText("Direct question")).toBeTruthy();
  const personal = screen.getByRole("group", { name: "Personal" });
  expect(within(personal).getByText("Project work")).toBeTruthy();
});
