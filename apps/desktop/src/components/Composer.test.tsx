import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, test, vi } from "vitest";
import styles from "../styles.css?raw";
import { ClientProvider } from "../rpc/context";
import { FakeService, flush } from "../rpc/fake";
import type { Session } from "../rpc/types";
import { resetAnswerGuard } from "../conversation/answer";
import { initialState, useAppStore } from "../state/store";
import { Composer } from "./Composer";

const session = (id: number, overrides: Partial<Session> = {}): Session => ({
  id,
  project_id: 1,
  agent: "local",
  title: "s",
  updated_seq: id,
  last_turn: "none",
  running: false,
  ...overrides,
});

let service: FakeService;

const ui = (sessionId: number) => (
  <ClientProvider client={service.client}>
    <Composer sessionId={sessionId} />
  </ClientProvider>
);

const box = () => screen.getByRole("textbox", { name: /reply or ask/i }) as HTMLTextAreaElement;

beforeEach(() => {
  resetAnswerGuard();
  useAppStore.setState({ ...initialState(), sessions: [session(1), session(2)], agents: ["local", "claude"] });
  service = new FakeService();
  service.open();
  service.results["turn.start"] = {};
  service.results["approvals.resolve"] = {};
});

test("enter sends, shift+enter adds a line and a draft survives switching sessions", async () => {
  const { rerender } = render(ui(1));
  await userEvent.type(box(), "a{Shift>}{Enter}{/Shift}b");
  expect(box().value).toBe("a\nb");
  rerender(ui(2));
  expect(box().value).toBe("");
  rerender(ui(1));
  expect(box().value).toBe("a\nb");
  await userEvent.type(box(), "{Enter}");
  await flush();
  expect(service.calls("turn.start")[0]).toEqual({ session_id: 1, prompt: "a\nb", mode: "supervised" });
  expect(box().value).toBe("");
});

test("an empty message isn't sent", async () => {
  render(ui(1));
  await userEvent.type(box(), "   {Enter}");
  expect(service.calls("turn.start")).toHaveLength(0);
});

test("while running, send becomes stop and the draft is kept", async () => {
  useAppStore.setState({ sessions: [session(1, { running: true })] });
  render(ui(1));
  await userEvent.type(box(), "for later{Enter}");
  expect(service.calls("turn.start")).toHaveLength(0);
  await userEvent.click(screen.getByRole("button", { name: "Stop" }));
  await flush();
  expect(service.calls("turn.stop")).toEqual([{ session_id: 1 }]);
  expect(box().value).toBe("for later");
});

test("a failed send keeps the text and shows the error", async () => {
  service.failures["turn.start"] = { code: 1001, message: "this session is busy" };
  render(ui(1));
  await userEvent.type(box(), "x{Enter}");
  await flush();
  expect(box().value).toBe("x");
  expect((await screen.findByRole("alert")).textContent).toContain("busy");
});

test("new sessions ask first and the chosen mode is used", async () => {
  render(ui(1));
  await userEvent.click(screen.getByRole("button", { name: "Permissions: Ask first" }));
  await userEvent.click(screen.getByRole("menuitemradio", { name: "Full access" }));
  await userEvent.type(box(), "x{Enter}");
  await flush();
  expect(service.calls("turn.start")[0].mode).toBe("full_access");
});

test("a pending request sits right above the box", async () => {
  useAppStore.setState({
    sessions: [session(1, { running: true })],
    approvals: [{ id: "a", session_id: 1, tool: "Bash", arguments: { command: "cargo test" } }],
  });
  render(ui(1));
  await userEvent.click(screen.getByRole("button", { name: "Allow" }));
  await flush();
  expect(service.calls("approvals.resolve")).toEqual([{ id: "a", allow: true }]);
});

test("choosing another agent hands the work over and opens the new session", async () => {
  localStorage.setItem("vatra.subscription-notice", "accepted");
  service.results["handoff.start"] = { session_id: 9 };
  service.results["sessions.list"] = [session(1), session(9, { agent: "claude" })];
  render(ui(1));
  await userEvent.click(screen.getByRole("button", { name: "Model: local" }));
  await userEvent.click(screen.getByRole("option", { name: "Claude Code" }));
  await userEvent.type(screen.getByRole("textbox", { name: /what should it do/i }), "continue");
  await userEvent.click(screen.getByRole("button", { name: "Hand over" }));
  await flush();
  expect(service.calls("handoff.start")[0]).toEqual({ from: 1, to: "claude", task: "continue", mode: "supervised" });
  expect(useAppStore.getState().activeSessionId).toBe(9);
  expect(useAppStore.getState().sessions).toHaveLength(2);
});

test("handing over to claude asks for the subscription notice first", async () => {
  render(ui(1));
  await userEvent.click(screen.getByRole("button", { name: "Model: local" }));
  await userEvent.click(screen.getByRole("option", { name: "Claude Code" }));
  await userEvent.type(screen.getByRole("textbox", { name: /what should it do/i }), "continue");
  await userEvent.click(screen.getByRole("button", { name: "Hand over" }));
  expect(screen.getAllByRole("dialog").at(-1)?.textContent).toContain("your account");
  expect(service.calls("handoff.start")).toHaveLength(0);
});

test("a handover needs a task", async () => {
  render(ui(1));
  await userEvent.click(screen.getByRole("button", { name: "Model: local" }));
  await userEvent.click(screen.getByRole("option", { name: "Claude Code" }));
  await userEvent.click(screen.getByRole("button", { name: "Hand over" }));
  expect(screen.getByRole("alert").textContent).toContain("what it should do");
});

test("escape closes a menu", async () => {
  render(ui(1));
  await userEvent.click(screen.getByRole("button", { name: "Permissions: Ask first" }));
  expect(screen.getByRole("menu")).toBeTruthy();
  await userEvent.keyboard("{Escape}");
  expect(screen.queryByRole("menu")).toBeNull();
});

const completions = [
  { label: "resumat", insert: "/resumat", detail: "Rezumă", kind: "skill" },
  { label: "reparat", insert: "/reparat", detail: "Repară", kind: "skill" },
];

/** Sets the text the way a composition does: the field gets the value and an input event flagged as composing. */
const composingInput = (text: string) => {
  const element = box();
  Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set?.call(element, text);
  element.dispatchEvent(new InputEvent("input", { bubbles: true, isComposing: true }));
};

test("filters_while_typing", async () => {
  service.results["composer.complete"] = completions;
  render(ui(1));
  await userEvent.type(box(), "/re");
  await screen.findByRole("option", { name: /resumat/ });
  await waitFor(() => expect(service.calls("composer.complete").at(-1)).toEqual({ project_id: 1, trigger: "/", query: "re" }));
});

test("arrows_move_enter_picks", async () => {
  service.results["composer.complete"] = completions;
  render(ui(1));
  await userEvent.type(box(), "/");
  await screen.findByRole("option", { name: /resumat/ });
  await userEvent.keyboard("{ArrowDown}");
  expect(screen.getByRole("option", { name: /reparat/ }).getAttribute("aria-selected")).toBe("true");
  await userEvent.keyboard("{Enter}");
  expect(box().value).toBe("/reparat ");
  expect(screen.queryByRole("listbox")).toBeNull();
});

test("escape_closes_keeps_text", async () => {
  service.results["composer.complete"] = completions;
  render(ui(1));
  await userEvent.type(box(), "/re");
  await screen.findByRole("option", { name: /resumat/ });
  await userEvent.keyboard("{Escape}");
  expect(screen.queryByRole("listbox")).toBeNull();
  expect(box().value).toBe("/re");
});

test("escape_reopens_on_a_new_trigger", async () => {
  service.results["composer.complete"] = completions;
  render(ui(1));
  await userEvent.type(box(), "/re");
  await screen.findByRole("option", { name: /resumat/ });
  await userEvent.keyboard("{Escape}");
  expect(screen.queryByRole("listbox")).toBeNull();
  await userEvent.type(box(), " @");
  await screen.findByRole("option", { name: /resumat/ });
  expect(box().value).toBe("/re @");
});

test("clearing_the_box_ends_a_dismissal", async () => {
  service.results["composer.complete"] = completions;
  render(ui(1));
  await userEvent.type(box(), "/re");
  await screen.findByRole("option", { name: /resumat/ });
  await userEvent.keyboard("{Escape}");
  expect(screen.queryByRole("listbox")).toBeNull();
  await userEvent.clear(box());
  await userEvent.type(box(), "/");
  await screen.findByRole("option", { name: /resumat/ });
});

test("no_send_while_list_open", async () => {
  service.results["composer.complete"] = completions;
  render(ui(1));
  await userEvent.type(box(), "/re");
  await screen.findByRole("option", { name: /resumat/ });
  await userEvent.keyboard("{Enter}");
  await flush();
  expect(service.calls("turn.start")).toHaveLength(0);
});

test("composing_ime_does_not_send_or_open", async () => {
  service.results["composer.complete"] = completions;
  render(ui(1));
  composingInput("/re");
  await flush();
  expect(screen.queryByRole("listbox")).toBeNull();
  fireEvent.keyDown(box(), { key: "Enter", isComposing: true });
  await flush();
  expect(service.calls("turn.start")).toHaveLength(0);
  expect(service.calls("composer.complete")).toHaveLength(0);
});

test("enter_while_running_explains", async () => {
  useAppStore.setState({ sessions: [session(1, { running: true })] });
  render(ui(1));
  await userEvent.type(box(), "după{Enter}");
  expect(screen.getByRole("status").textContent).toContain("running");
  expect(service.calls("turn.start")).toHaveLength(0);
  expect(box().value).toBe("după");
});

test("double_enter_sends_once", async () => {
  render(ui(1));
  await userEvent.type(box(), "x");
  fireEvent.keyDown(box(), { key: "Enter" });
  fireEvent.keyDown(box(), { key: "Enter" });
  await flush();
  expect(service.calls("turn.start")).toHaveLength(1);
});

test("sent_prompt_clears_pending", async () => {
  render(ui(1));
  await userEvent.type(box(), "a{Enter}");
  await flush();
  expect(box().value).toBe("");
  await userEvent.type(box(), "b{Enter}");
  await flush();
  expect(service.calls("turn.start").map((call) => call.prompt)).toEqual(["a", "b"]);
});

test("field_remeasures_on_width_change", async () => {
  let heightNow = 40;
  let onResize: ((entries: { contentRect: { width: number } }[]) => void) | undefined;
  vi.stubGlobal(
    "ResizeObserver",
    class {
      constructor(callback: (entries: { contentRect: { width: number } }[]) => void) {
        onResize = callback;
      }
      observe() {}
      disconnect() {}
    },
  );
  render(ui(1));
  Object.defineProperty(box(), "scrollHeight", { configurable: true, get: () => heightNow });
  act(() => onResize?.([{ contentRect: { width: 300 } }]));
  expect(box().style.height).toBe("40px");
  heightNow = 80;
  act(() => onResize?.([{ contentRect: { width: 320 } }]));
  expect(box().style.height).toBe("80px");
  vi.unstubAllGlobals();
});

test("visible_focus_on_field", () => {
  const rule = /\.prompt:focus-within\s*\{([^}]*)\}/.exec(styles)?.[1] ?? "";
  expect(rule).toMatch(/border-color/);
});

test("menu_arrow_keys_and_focus", async () => {
  render(ui(1));
  const chip = screen.getByRole("button", { name: "Permissions: Ask first" });
  await userEvent.click(chip);
  expect(document.activeElement).toBe(screen.getByRole("menuitemradio", { name: "Ask first" }));
  await userEvent.keyboard("{ArrowDown}");
  expect(document.activeElement).toBe(screen.getByRole("menuitemradio", { name: "Edit files on its own" }));
  await userEvent.keyboard("{ArrowUp}{ArrowUp}");
  expect(document.activeElement).toBe(screen.getByRole("menuitemradio", { name: "Full access" }));
  await userEvent.keyboard("{Enter}");
  expect(screen.queryByRole("menu")).toBeNull();
  expect(document.activeElement).toBe(screen.getByRole("button", { name: "Permissions: Full access" }));
});
