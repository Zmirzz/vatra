import { isTauri } from "@tauri-apps/api/core";
import { beforeEach, expect, test, vi } from "vitest";
import { copy } from "../copy";
import { FakeService, flush } from "../rpc/fake";
import type { Session, StoredEvent } from "../rpc/types";
vi.mock("@tauri-apps/api/core", () => ({ isTauri: vi.fn(() => false) }));
import { applyNotification, attachClient, initialState, mergeEvents, sessionGroups, useAppStore, type AppState } from "./store";

const event = (seq: number, session_id = 1): StoredEvent => ({ seq, session_id, event: { type: "assistant_text", text: "x" } });

const session = (id: number, overrides: Partial<Session> = {}): Session => ({
  id,
  project_id: 1,
  agent: "local",
  title: `s${id}`,
  updated_seq: id,
  last_turn: "none",
  running: false,
  ...overrides,
});

const state = (overrides: Partial<AppState> = {}): AppState => ({ ...initialState(), ...overrides });

beforeEach(() => useAppStore.setState(initialState()));

test("mergeEvents deduplicates by seq and keeps order", () => {
  expect(mergeEvents([event(1), event(2)], [event(2), event(3)]).map((x) => x.seq)).toEqual([1, 2, 3]);
  expect(mergeEvents([event(3)], [event(1)]).map((x) => x.seq)).toEqual([1, 3]);
});

test("a session waiting for approval is grouped above running ones", () => {
  const groups = sessionGroups(
    state({
      sessions: [session(1, { running: true }), session(2, { running: true }), session(3)],
      approvals: [{ id: "a", session_id: 2, tool: "Bash", arguments: {} }],
    }),
  );
  expect(groups.waiting.map((s) => s.id)).toEqual([2]);
  expect(groups.working.map((s) => s.id)).toEqual([1]);
  expect(groups.done.map((s) => s.id)).toEqual([3]);
});

test("each group shows the most recently active session first", () => {
  const groups = sessionGroups(state({ sessions: [session(1, { updated_seq: 5 }), session(2, { updated_seq: 9 })] }));
  expect(groups.done.map((s) => s.id)).toEqual([2, 1]);
});

test("an event updates the session it belongs to", () => {
  const next = applyNotification(state({ sessions: [session(1, { title: "New session" })] }), {
    method: "event",
    params: { seq: 50, session_id: 1, event: { type: "user_message", text: "prima cerere" } },
  });
  expect(next.eventsBySession?.[1]).toHaveLength(1);
  expect(next.sessions?.[0].updated_seq).toBe(50);
  expect(next.sessions?.[0].title).toBe("prima cerere");
});

test("a failed turn is remembered on the session", () => {
  const next = applyNotification(state({ sessions: [session(1)] }), {
    method: "event",
    params: { seq: 2, session_id: 1, event: { type: "turn_failed", agent: "local", message: "x" } },
  });
  expect(next.sessions?.[0].last_turn).toBe("failed");
});

test("turn state and approvals follow the notifications", () => {
  let current = state({ sessions: [session(1)] });
  current = { ...current, ...applyNotification(current, { method: "turn_state", params: { session_id: 1, running: true } }) };
  expect(current.sessions[0].running).toBe(true);
  const request = { id: "a", session_id: 1, tool: "Bash", arguments: {} };
  current = { ...current, ...applyNotification(current, { method: "approval", params: request }) };
  current = { ...current, ...applyNotification(current, { method: "approval", params: request }) };
  expect(current.approvals).toHaveLength(1);
  current = { ...current, ...applyNotification(current, { method: "approval_resolved", params: { id: "a" } }) };
  expect(current.approvals).toEqual([]);
});

test("an event for a session the app doesn't know yet is kept", () => {
  const next = applyNotification(state(), { method: "event", params: event(1, 99) });
  expect(next.eventsBySession?.[99]).toHaveLength(1);
});

test("unknown notifications and event types are ignored without throwing", () => {
  expect(() => applyNotification(state(), { method: "nou", params: {} } as never)).not.toThrow();
  const next = applyNotification(state({ sessions: [session(1)] }), {
    method: "event",
    params: { seq: 3, session_id: 1, event: { type: "tip_nou" } as never },
  });
  expect(next.eventsBySession?.[1]).toHaveLength(1);
});

test("appending in order keeps earlier events without reordering", () => {
  const merged = mergeEvents([event(1), event(2)], [event(3)]);
  expect(merged.map((x) => x.seq)).toEqual([1, 2, 3]);
  const same = [event(1)];
  expect(mergeEvents(same, [])).toBe(same);
});

test("delta shows while running and clears when the final text arrives", () => {
  let current = state({ sessions: [session(1, { running: true })] });
  const delta = (text: string) => ({ method: "event" as const, params: { seq: 0, session_id: 1, event: { type: "assistant_delta" as const, text } } });
  current = { ...current, ...applyNotification(current, delta("Sal")) };
  current = { ...current, ...applyNotification(current, delta("ut")) };
  expect(current.streaming[1]).toBe("Salut");
  current = { ...current, ...applyNotification(current, { method: "event", params: { seq: 7, session_id: 1, event: { type: "assistant_text", text: "Salut" } } }) };
  expect(current.streaming[1]).toBeUndefined();
});

test("delta does not touch seq, the journal or the session order", () => {
  const before = state({ sessions: [session(1, { updated_seq: 4 })], eventsBySession: { 1: [event(4)] } });
  const next = applyNotification(before, { method: "event", params: { seq: 0, session_id: 1, event: { type: "assistant_delta", text: "x" } } });
  expect(next.eventsBySession).toBeUndefined();
  expect(next.sessions).toBeUndefined();
  expect(next.streaming?.[1]).toBe("x");
});

test("a problem is listed once per session", () => {
  const notice = { method: "problem" as const, params: { session_id: 1, message: "Disk full" } };
  let current = state();
  current = { ...current, ...applyNotification(current, notice) };
  current = { ...current, ...applyNotification(current, notice) };
  expect(current.problems[1]).toEqual(["Disk full"]);
});

test("questions are added, replaced and resolved", () => {
  const request = { id: "q", session_id: 1, questions: [{ question: "Continui?" }] };
  let current = state();
  current = { ...current, ...applyNotification(current, { method: "question", params: request }) };
  current = { ...current, ...applyNotification(current, { method: "question", params: request }) };
  expect(current.questions).toHaveLength(1);
  current = { ...current, ...applyNotification(current, { method: "question_resolved", params: { id: "q" } }) };
  expect(current.questions).toEqual([]);
});

test("a too large event is fetched from the seq before it and merged", async () => {
  const service = new FakeService();
  service.results["sessions.events"] = [{ seq: 9, session_id: 1, event: { type: "assistant_text", text: "mare" } }];
  attachClient(service.client);
  service.open();
  service.notify("event_too_large", { session_id: 1, seq: 9 });
  await flush();
  expect(service.calls("sessions.events")).toEqual([{ session_id: 1, after_seq: 8 }]);
  expect(useAppStore.getState().eventsBySession[1]?.map((stored) => stored.seq)).toEqual([9]);
});

test("an open_url notification opens the login address", async () => {
  const service = new FakeService();
  const open = vi.spyOn(window, "open").mockReturnValue(null);
  vi.mocked(isTauri).mockReturnValue(false);
  attachClient(service.client);
  service.open();
  service.notify("open_url", { url: "https://auth.example.test/authorize?state=s" });
  await flush();
  expect(open).toHaveBeenCalledWith("https://auth.example.test/authorize?state=s", "_blank", "noopener,noreferrer");
  open.mockRestore();
});

test("reconnecting loads the questions that are still waiting", async () => {
  const service = new FakeService();
  service.results["questions.list"] = [{ id: "q", session_id: 1, questions: [{ question: "Continui?" }] }];
  attachClient(service.client);
  service.open();
  await flush();
  expect(useAppStore.getState().questions.map((request) => request.id)).toEqual(["q"]);
});

test("reconnect_does_not_show_text_twice", async () => {
  const service = new FakeService();
  service.results["sessions.events"] = [{ seq: 2, session_id: 1, event: { type: "assistant_text", text: "Salut" } }];
  attachClient(service.client);
  service.open();
  await flush();
  useAppStore.setState({ loadedSessions: [1], eventsBySession: { 1: [event(1)] }, streaming: { 1: "Sal" } });
  service.drop();
  service.reconnect();
  await flush();
  const state = useAppStore.getState();
  expect(state.streaming).toEqual({});
  expect(state.eventsBySession[1].map((stored) => stored.seq)).toEqual([1, 2]);
});

test("a too large event that can't be fetched is reported and fetched again on reconnect", async () => {
  const service = new FakeService();
  service.failures["sessions.events"] = { code: 1000, message: "boom" };
  attachClient(service.client);
  service.open();
  await flush();
  service.notify("event_too_large", { session_id: 1, seq: 9 });
  await flush();
  expect(useAppStore.getState().problems[1]).toEqual([copy.eventMissing]);
  service.results["sessions.events"] = [{ seq: 9, session_id: 1, event: { type: "assistant_text", text: "mare" } }];
  service.drop();
  service.reconnect();
  await flush();
  expect(service.calls("sessions.events").at(-1)).toEqual({ session_id: 1, after_seq: 8 });
  expect(useAppStore.getState().eventsBySession[1]?.map((stored) => stored.seq)).toEqual([9]);
  expect(useAppStore.getState().problems[1]).toEqual([]);
});
