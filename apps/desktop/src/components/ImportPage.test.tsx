import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, test } from "vitest";
import { ClientProvider } from "../rpc/context";
import { FakeService, flush } from "../rpc/fake";
import { setLanguage } from "../copy";
import { initialState, useAppStore } from "../state/store";
import { ImportPage } from "./ImportPage";

let service: FakeService;
const item = (key: string, kind = "skill", more = {}) => ({ key, source: `C:/source/${key}`, project: null, project_known: true, status: "new", secrets: [], runs_code: [], report: { kind, name: key, origin: "codex", routes: [["engine", "works"], ["claude", { limited: "Not verified" }], ["acp", { unavailable: "Not supported" }]] }, ...more });
const project = (key: string, conversations: number, more = {}) => ({ key, name: key, path: `C:/${key}`, conversations, last_active: "2026-10-05T10:00:00Z", known: false, ...more });
const setup = async (items: unknown[] = [], history: unknown[] = []) => {
  service.results["import.scan"] = { items, conflicts: [], problems: [] };
  service.results["import.history"] = history;
  render(<ClientProvider client={service.client}><ImportPage /></ClientProvider>);
  await screen.findByRole("tab", { name: /Projects and conversations/ });
};
beforeEach(() => { setLanguage("en"); useAppStore.setState(initialState()); service = new FakeService(); service.open(); service.results["import.apply"] = { results: [] }; });

test("it looks by itself and offers only projects, connections and skills", async () => {
  await setup([item("design"), item("files", "mcp_server"), item("addon", "plugin"), item("rules", "instructions"), item("old", "hook", { status: null })]);
  expect(service.calls("import.scan")).toEqual([{}]);
  expect(service.calls("import.history")).toEqual([{}]);
  expect(screen.getAllByRole("tab").map((tab) => tab.textContent?.replace(/\d+$/, "").trim())).toEqual(["Projects and conversations", "Connections", "Skills"]);
  await userEvent.click(screen.getByRole("tab", { name: /Skills/ }));
  expect(screen.getByRole("checkbox", { name: "design" })).toBeTruthy();
  for (const hidden of ["addon", "rules", "old", "files"]) expect(screen.queryByRole("checkbox", { name: hidden })).toBeNull();
  await userEvent.click(screen.getByRole("tab", { name: /Connections/ }));
  expect(screen.getByRole("checkbox", { name: "files" })).toBeTruthy();
});

test("nothing is picked at first, and what is not shown is never brought over", async () => {
  await setup([item("design"), item("addon", "plugin"), item("rules", "instructions")]);
  await userEvent.click(screen.getByRole("tab", { name: /Skills/ }));
  expect((screen.getByRole("checkbox", { name: "design" }) as HTMLInputElement).checked).toBe(false);
  expect((screen.getByRole("button", { name: "Pick something to bring over" }) as HTMLButtonElement).disabled).toBe(true);
  await userEvent.click(screen.getByRole("checkbox", { name: "design" }));
  await userEvent.click(screen.getByRole("button", { name: "Bring over 1 skill" }));
  const sent = service.calls("import.apply")[0] as { items: { key: string; include: boolean }[] };
  expect(sent.items.filter((entry) => entry.include).map((entry) => entry.key)).toEqual(["design"]);
  expect(sent.items.map((entry) => entry.key).sort()).toEqual(["addon", "design", "rules"]);
});

test("whole projects come with their conversations", async () => {
  await setup([], [project("shop", 41), project("notes", 1)]);
  expect(screen.getByText(/41 conversations/)).toBeTruthy();
  await userEvent.click(screen.getByRole("checkbox", { name: "shop" }));
  expect(screen.getByRole("button", { name: "Bring over 1 project (41 conversations)" })).toBeTruthy();
  await userEvent.click(screen.getByRole("button", { name: "Select all" }));
  await userEvent.click(screen.getByRole("button", { name: "Bring over 2 projects (42 conversations)" }));
  expect(service.calls("import.apply")[0]).toMatchObject({ include_history: false, history_projects: expect.arrayContaining(["shop", "notes"]) });
});

test("a connection that needs a key brings it only when asked", async () => {
  await setup([item("remote", "mcp_server", { secrets: [{ server: "remote", key: "API_TOKEN" }] })]);
  await userEvent.click(screen.getByRole("tab", { name: /Connections/ }));
  await userEvent.click(screen.getByRole("checkbox", { name: "remote" }));
  expect(screen.getByText("Without it, the connection waits until you add the key.")).toBeTruthy();
  expect(screen.queryByDisplayValue(/secret/i)).toBeNull();
  await userEvent.click(screen.getByRole("button", { name: "Bring over 1 connection" }));
  expect(service.calls("import.apply")[0]).toMatchObject({ items: [{ key: "remote", include: true, secrets: {} }] });
  await userEvent.click(await screen.findByRole("button", { name: "Look again" }));
  await userEvent.click(await screen.findByRole("tab", { name: /Connections/ }));
  await userEvent.click(screen.getByRole("checkbox", { name: "remote" }));
  await userEvent.click(screen.getByRole("checkbox", { name: /Bring its saved key too/ }));
  await userEvent.click(screen.getByRole("button", { name: "Bring over 1 connection" }));
  expect(service.calls("import.apply")[1]).toMatchObject({ items: [{ key: "remote", include: true, secrets: { API_TOKEN: true } }] });
});

test("a program is shown and needs approval, and 'select all' skips it", async () => {
  await setup([item("files", "mcp_server", { runs_code: [{ what: "start", command: "node server.js" }] }), item("plain", "mcp_server")]);
  await userEvent.click(screen.getByRole("tab", { name: /Connections/ }));
  await userEvent.click(screen.getByRole("button", { name: "Select all" }));
  expect((screen.getByRole("checkbox", { name: "files" }) as HTMLInputElement).checked).toBe(false);
  expect((screen.getByRole("checkbox", { name: "plain" }) as HTMLInputElement).checked).toBe(true);
  await userEvent.click(screen.getByRole("checkbox", { name: "files" }));
  expect(screen.getByText("It starts a program on your computer. Approve it to bring it over.")).toBeTruthy();
  expect(screen.getByText("node server.js")).toBeTruthy();
  await userEvent.click(screen.getByRole("checkbox", { name: "I approve this program" }));
  await userEvent.click(screen.getByRole("button", { name: "Bring over 2 connections" }));
  expect(service.calls("import.apply")[0]).toMatchObject({ items: expect.arrayContaining([{ key: "files", include: true, approve_code: true, secrets: {}, allow_rules: false }]) });
});

test("progress and failures are explained; unchanged items are hidden", async () => {
  await setup([item("same", "skill", { status: "unchanged" }), item("changed", "skill", { status: "changed" })]);
  service.results["import.apply"] = { results: [{ key: "changed", name: "changed", project: null, state: { state: "failed", message: "Source changed; scan again" } }] };
  await userEvent.click(screen.getByRole("tab", { name: /Skills/ }));
  expect(screen.queryByRole("checkbox", { name: "same" })).toBeNull();
  await userEvent.click(screen.getByRole("checkbox", { name: "changed" }));
  act(() => service.notify("import_progress", { done: 1, total: 2, label: "changed" }));
  expect(screen.getByRole("status").textContent).toContain("1 / 2");
  await userEvent.click(screen.getByRole("button", { name: "Bring over 1 skill" }));
  expect(await screen.findByText("Source changed; scan again")).toBeTruthy();
});

test("a finished import counts conversations", async () => {
  await setup([], [project("shop", 2)]);
  service.results["import.apply"] = { results: [
    { key: "history:a", name: "Past conversation", project: null, state: { state: "added" } },
    { key: "history:b", name: "Past conversation", project: null, state: { state: "added" } },
  ] };
  await userEvent.click(screen.getByRole("checkbox", { name: "shop" }));
  await userEvent.click(screen.getByRole("button", { name: /Bring over/ }));
  expect(await screen.findByText("Brought over 2 conversations.")).toBeTruthy();
});

test("scan errors leave a retryable explanation", async () => {
  service.failures["import.scan"] = { code: 1, message: "Cannot read source" };
  render(<ClientProvider client={service.client}><ImportPage /></ClientProvider>);
  expect((await screen.findByRole("alert")).textContent).toContain("Cannot read source");
});

test("conflicts offer each source; choosing a command copy resets code consent", async () => {
  const kept = item("new-copy", "skill", { source: "C:/new/SKILL.md", report: { ...item("design").report, name: "design" } });
  const alternate = item("old-copy", "skill", { source: "C:/old/SKILL.md", runs_code: [{ what: "expand", command: "git status" }], report: { ...item("design").report, name: "design" } });
  service.results["import.scan"] = { items: [kept], alternatives: [alternate], conflicts: [{ name: "design", kind: "skill", kept: { source: "C:/new/SKILL.md" }, dropped: [{ source: "C:/old/SKILL.md" }] }], problems: [] };
  render(<ClientProvider client={service.client}><ImportPage /></ClientProvider>);
  await userEvent.click(await screen.findByRole("tab", { name: /Skills/ }));
  await userEvent.click(screen.getByText("Details"));
  await userEvent.selectOptions(screen.getByRole("combobox", { name: /choose which copy/i }), "old-copy");
  expect((screen.getByRole("checkbox", { name: "design" }) as HTMLInputElement).checked).toBe(false);
  await userEvent.click(screen.getByRole("checkbox", { name: "design" }));
  expect((screen.getByRole("checkbox", { name: "I approve this program" }) as HTMLInputElement).checked).toBe(false);
  await userEvent.click(screen.getByRole("checkbox", { name: "I approve this program" }));
  await userEvent.click(screen.getByRole("button", { name: "Bring over 1 skill" }));
  expect(service.calls("import.apply")[0]).toMatchObject({ items: [{ key: "old-copy", include: true, approve_code: true }] });
});

test("a completed import refreshes extension availability for the open conversation", async () => {
  await setup([item("design")]);
  service.results["catalog.list"] = [{ id: 8, name: "design", kind: "skill", origin: "codex", enabled: true, scope: "global", content_hash: "hash", approved_hash: null, runs_code: [], plugin: null, details: {}, routes: [["engine", "works"]] }];
  await userEvent.click(screen.getByRole("tab", { name: /Skills/ }));
  await userEvent.click(screen.getByRole("checkbox", { name: "design" }));
  await userEvent.click(screen.getByRole("button", { name: "Bring over 1 skill" }));
  await flush();
  expect(useAppStore.getState().catalog[0]?.name).toBe("design");
});

test("a running import can be cancelled", async () => {
  await setup([item("design")]);
  await userEvent.click(screen.getByRole("tab", { name: /Skills/ }));
  await userEvent.click(screen.getByRole("checkbox", { name: "design" }));
  service.silent = true;
  await userEvent.click(screen.getByRole("button", { name: "Bring over 1 skill" }));
  service.silent = false;
  await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(service.calls("import.cancel")).toEqual([{}]);
});
