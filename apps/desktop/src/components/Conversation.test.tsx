import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, test } from "vitest";
import type { WorkItem } from "../conversation/group";
import type { Session, StoredEvent } from "../rpc/types";
import { initialState, useAppStore } from "../state/store";
import { Conversation } from "./Conversation";
import { Markdown } from "./Markdown";
import { ToolRow } from "./ToolRow";
import {ClientProvider} from "../rpc/context";
import {FakeService} from "../rpc/fake";
function ConnectedConversation(){const service=new FakeService();service.open();return <ClientProvider client={service.client}><Conversation/></ClientProvider>;}

const toolItem = (output: string | undefined, overrides: Partial<WorkItem> = {}): WorkItem => ({
  id: "1",
  name: "Bash",
  arguments: { command: "ls" },
  output,
  isError: false,
  denied: false,
  interrupted: false,
  children: [],
  texts: [],
  ...overrides,
});

const session: Session = { id: 1, project_id: 1, agent: "local", title: "s", updated_seq: 1, last_turn: "none", running: false };
const events = (list: StoredEvent["event"][]) => ({
  activeSessionId: 1,
  sessions: [session],
  eventsBySession: { 1: list.map((event, index) => ({ seq: index + 1, session_id: 1, event })) },
});

beforeEach(() => useAppStore.setState(initialState()));

test("a session summarizes limits only from enabled extensions for its route and project", async () => {
  const entry = { id: 2, name: "review", kind: "skill", origin: "codex", scope: "global", content_hash: "hash", approved_hash: null, enabled: true, runs_code: [], plugin: null, details: {}, routes: [["engine", "works"], ["acp", { limited: "Needs setup" }]] } as const;
  useAppStore.setState({ ...events([]), sessions: [{ ...session, extension_route: "acp" }], catalog: [JSON.parse(JSON.stringify(entry)), { ...JSON.parse(JSON.stringify(entry)), id: 3, name: "wrong project", scope: { project: 999 } }, { ...JSON.parse(JSON.stringify(entry)), id: 4, name: "disabled", enabled: false }] });
  render(<ConnectedConversation />);
  expect(screen.getAllByRole("status")).toHaveLength(1);
  await userEvent.click(screen.getByRole("button", { name: "Details" }));
  expect(screen.getByText(/review: Limited: Needs setup/)).toBeTruthy();
  expect(screen.queryByText(/wrong project:/)).toBeNull();
  expect(screen.queryByText(/disabled:/)).toBeNull();
});

test("markdown with raw html renders as text and runs nothing", () => {
  render(<Markdown text={'<img src=x onerror="window.pwned=1"><script>window.pwned=1</script>'} />);
  expect((window as unknown as { pwned?: number }).pwned).toBeUndefined();
  expect(document.querySelector("img, script")).toBeNull();
});

test("javascript links are not clickable", () => {
  render(<Markdown text="[x](javascript:alert(1))" />);
  expect(screen.getByText("x").closest("a")?.getAttribute("href") ?? "").not.toContain("javascript:");
});

test("normal links open outside the app", () => {
  render(<Markdown text="[docs](https://example.com)" />);
  const link = screen.getByText("docs").closest("a");
  expect(link?.getAttribute("href")).toBe("https://example.com");
  expect(link?.getAttribute("rel")).toContain("noreferrer");
});

test("markdown images don't load anything from the network", () => {
  render(<Markdown text="![data](https://evil.example/?x=secret)" />);
  expect(document.querySelector("img")).toBeNull();
  expect(screen.getByText("data").closest("a")?.getAttribute("href")).toContain("evil.example");
});

test("a tool is described in plain words", () => {
  render(<ToolRow item={toolItem("ok")} />);
  expect(screen.getByText("Ran a command")).toBeTruthy();
  expect(screen.queryByText("Bash")).toBeNull();
});

test("a huge tool result shows a preview until expanded", async () => {
  render(<ToolRow item={toolItem("a".repeat(1_000_000))} />);
  expect(screen.queryByTestId("tool-output")).toBeNull();
  await userEvent.click(screen.getByRole("button", { name: /details/i }));
  expect(screen.getByTestId("tool-output").textContent!.length).toBeLessThan(20_100);
  await userEvent.click(screen.getByRole("button", { name: /show everything/i }));
  expect(screen.getByTestId("tool-output").textContent!.length).toBe(1_000_000);
});

test("a refused tool is marked", () => {
  render(<ToolRow item={toolItem(undefined, { denied: true })} />);
  expect(screen.getByText("Refused")).toBeTruthy();
});

test("the conversation shows messages and folds the work away", () => {
  useAppStore.setState(
    events([
      { type: "user_message", text: "request" },
      { type: "tool_call", id: "1", name: "Bash", arguments: { command: "cargo test" } },
      { type: "tool_result", id: "1", output: "ok", is_error: false },
      { type: "assistant_text", text: "**done**" },
      { type: "turn_failed", agent: "a", message: "it broke" },
    ]),
  );
  render(<ConnectedConversation />);
  expect(screen.getByText("request")).toBeTruthy();
  expect(screen.getByText("done").tagName).toBe("STRONG");
  expect(screen.getByText(/Worked through 1 step/)).toBeTruthy();
  expect(screen.queryByText("cargo test")).toBeNull();
  expect(screen.getByRole("alert").textContent).toContain("it broke");
});

test("opening the work shows each step", async () => {
  useAppStore.setState(events([{ type: "tool_call", id: "1", name: "Bash", arguments: { command: "cargo test" } }]));
  render(<ConnectedConversation />);
  await userEvent.click(screen.getByRole("button", { name: /step/i }));
  expect(screen.getByText("cargo test")).toBeTruthy();
});

test("while the agent runs, the stitched mark says it's working", () => {
  useAppStore.setState({ ...events([{ type: "user_message", text: "x" }]), sessions: [{ ...session, running: true }] });
  render(<ConnectedConversation />);
  const status = screen.getByRole("status");
  expect(status.textContent).toContain("Working");
  expect(status.querySelector("svg.mark")).toBeTruthy();
});

test("work still running says so without claiming it's finished", () => {
  useAppStore.setState(events([{ type: "tool_call", id: "1", name: "Bash", arguments: {} }]));
  render(<ConnectedConversation />);
  const head = screen.getByRole("button", { name: /step/i }).textContent ?? "";
  expect(head).toContain("Working through");
  expect(head).not.toContain("Worked");
});

test("a stopped turn no longer says it's working", () => {
  useAppStore.setState(
    events([
      { type: "tool_call", id: "1", name: "Bash", arguments: {} },
      { type: "turn_failed", agent: "a", message: "stopped" },
    ]),
  );
  render(<ConnectedConversation />);
  expect(screen.getByRole("button", { name: /step/i }).textContent).toContain("Worked");
});

test("a degraded handover tells the user", () => {
  const brief = { summary: "s", files: ["a.rs"], decisions: [], remaining: [], degraded: true };
  useAppStore.setState(events([{ type: "handoff_started", from_session: 2, to_agent: "claude", brief }]));
  render(<ConnectedConversation />);
  expect(screen.getByText(/latest messages/)).toBeTruthy();
  expect(screen.getByText(/Claude Code/)).toBeTruthy();
});

test("usage counts stay out of the conversation, in the technical details", () => {
  useAppStore.setState(events([{ type: "usage", input_tokens: 123, output_tokens: 45, cost_usd: null }]));
  render(<ConnectedConversation />);
  expect(screen.getByText("Technical details")).toBeTruthy();
});

test("huge technical details, arguments and helper text are capped", async () => {
  const big = "b".repeat(1_000_000);
  useAppStore.setState(
    events([
      { type: "unmapped", source: "x", raw: big },
      { type: "user_message", text: "m" },
      { type: "tool_call", id: "1", name: "Agent", arguments: { text: big } },
      { type: "nested", parent_id: "1", event: { type: "assistant_text", text: big } },
    ]),
  );
  render(<ConnectedConversation />);
  await userEvent.click(screen.getByRole("button", { name: /step/i }));
  await userEvent.click(screen.getByRole("button", { name: /details agent/i }));
  expect(document.body.textContent!.length).toBeLessThan(100_000);
});

test("an agent waiting for an answer isn't described as working", () => {
  useAppStore.setState({
    ...events([{ type: "user_message", text: "x" }]),
    sessions: [{ ...session, running: true }],
    approvals: [{ id: "a", session_id: 1, tool: "Bash", arguments: {} }],
  });
  render(<ConnectedConversation />);
  expect(screen.queryByRole("status")).toBeNull();
});

test("compacted, background results and new rules are quiet lines", () => {
  useAppStore.setState(
    events([
      { type: "compacted", summary: "rezumat" },
      { type: "background_result", id: "b1", text: "build ok" },
      { type: "rule_added", label: "src/*.rs", text: "Use snake case." },
    ]),
  );
  render(<ConnectedConversation />);
  expect(screen.getByText("Older messages were summarized to make room.")).toBeTruthy();
  expect(screen.getByText("A background command finished.")).toBeTruthy();
  expect(screen.getByText("A rule for src/*.rs was added.")).toBeTruthy();
  expect(screen.queryByText("Technical details")).toBeNull();
});

test("tools loaded by ToolSearch show no row", () => {
  useAppStore.setState(events([{ type: "tools_loaded", names: ["mcp__demo__filler_042"] }]));
  const { container } = render(<ConnectedConversation />);
  expect(screen.queryByText("Technical details")).toBeNull();
  expect(container.textContent).not.toContain("filler_042");
});

test("streamed text shows while running and runs no html", () => {
  useAppStore.setState({
    ...events([{ type: "user_message", text: "x" }]),
    sessions: [{ ...session, running: true }],
    streaming: { 1: "Începe <img src=x onerror=\"window.pwned=1\"><script>window.pwned=1</script>" },
  });
  render(<ConnectedConversation />);
  expect(screen.getByText(/Începe/)).toBeTruthy();
  expect((window as unknown as { pwned?: number }).pwned).toBeUndefined();
  expect(document.querySelector("img, script")).toBeNull();
});

test("a problem from the service is shown in the conversation", () => {
  useAppStore.setState({ ...events([{ type: "user_message", text: "x" }]), problems: { 1: ["Disk full"] } });
  render(<ConnectedConversation />);
  expect(screen.getByText("Disk full")).toBeTruthy();
});
