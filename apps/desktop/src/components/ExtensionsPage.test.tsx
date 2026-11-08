import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, test } from "vitest";
import { ClientProvider } from "../rpc/context";
import { FakeService } from "../rpc/fake";
import { setLanguage } from "../copy";
import { initialState, useAppStore } from "../state/store";
import { ExtensionsPage } from "./ExtensionsPage";

let service: FakeService;
const base = { id: 2, name: "files", kind: "mcp_server", origin: "codex", scope: "global", content_hash: "new-hash", approved_hash: null, enabled: true, runs_code: [{ what: "start", command: "node server.js" }], plugin: null, details: {}, routes: [["engine", { needs_approval: "Review commands" }], ["claude", "works"], ["acp", { limited: "Not verified" }]] };
function setup(entry = base) { service.results["catalog.list"] = [entry]; render(<ClientProvider client={service.client}><ExtensionsPage /></ClientProvider>); }
beforeEach(() => { setLanguage("en"); useAppStore.setState(initialState()); service = new FakeService(); service.open(); });

test("toggle persists enabled state, details start collapsed, changed code needs approval", async () => {
  setup();
  await screen.findByText("files");
  expect((screen.getByText("Details").closest("details") as HTMLDetailsElement).open).toBe(false);
  expect(screen.getByText("Review the commands before enabling this tool.")).toBeTruthy();
  await userEvent.click(screen.getByRole("checkbox", { name: "files" }));
  expect(service.calls("catalog.set_enabled")).toEqual([{ id: 2, enabled: false }]);
  await userEvent.click(screen.getByText("Details"));
  expect(screen.getByText("node server.js")).toBeTruthy();
  await userEvent.click(screen.getByRole("button", { name: "Approve these commands" }));
  expect(service.calls("catalog.approve")).toEqual([{ id: 2, expected_hash: "new-hash" }]);
});

test("missing values use a password field and clear it after saving", async () => {
  setup({ ...base, runs_code: [], secrets: [{ server: "files", key: "TOKEN" }] } as typeof base);
  await screen.findByText("files");
  await userEvent.click(screen.getByText("Details"));
  const input = screen.getByLabelText("TOKEN") as HTMLInputElement;
  expect(input.type).toBe("password");
  await userEvent.type(input, "private-value");
  await userEvent.click(screen.getByRole("button", { name: "Save value" }));
  expect(service.calls("catalog.set_secret")).toEqual([{ id: 2, key: "TOKEN", value: "private-value" }]);
  expect(input.value).toBe("");
});

test("sign in starts the connected-tool login", async () => {
  setup({ ...base, runs_code: [], details: { oauth: {} } });
  await screen.findByText("files");
  await userEvent.click(screen.getByRole("button", { name: "Sign in" }));
  expect(service.calls("mcp.login")).toEqual([{ server: "files" }]);
});

test("a failed toggle preserves the shown value and explains the error", async () => {
  setup(); service.failures["catalog.set_enabled"] = { code: 1, message: "Cannot update catalog" };
  await screen.findByText("files");
  await userEvent.click(screen.getByRole("checkbox", { name: "files" }));
  expect((await screen.findByRole("alert")).textContent).toContain("Cannot update catalog");
  expect((screen.getByRole("checkbox", { name: "files" }) as HTMLInputElement).checked).toBe(true);
});

test("unlinked memory stays off until the user links it to a Vatra project", async () => {
  useAppStore.setState({ projects: [{ id: 7, name: "My work", path: "C:/work" }] });
  setup({ ...base, kind: "memory", enabled: false, runs_code: [], details: { native_project: "C:/old-work" } });
  await screen.findByText("files");
  expect((screen.getByRole("checkbox", { name: "files" }) as HTMLInputElement).disabled).toBe(true);
  expect(screen.getByText(/Choose a project before using this memory/)).toBeTruthy();
  await userEvent.selectOptions(screen.getByRole("combobox", { name: "Link to a project" }), "7");
  expect(service.calls("catalog.set_project")).toEqual([{ id: 2, project_id: 7 }]);
  expect(service.calls("catalog.set_enabled")).toHaveLength(0);
});
