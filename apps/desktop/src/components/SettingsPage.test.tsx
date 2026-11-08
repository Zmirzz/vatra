import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, test } from "vitest";
import { App } from "../App";
import { ClientProvider } from "../rpc/context";
import { FakeService, flush } from "../rpc/fake";
import { defaults, loadPreferences, updatePreferences, usePreferences } from "../settings/preferences";
import { initialState, useAppStore } from "../state/store";
import { Composer } from "./Composer";
import { SettingsPage } from "./SettingsPage";

beforeEach(() => {
  updatePreferences(defaults());
  useAppStore.setState(initialState());
});

test("the app starts in English", () => {
  expect(defaults().language).toBe("en");
  render(<SettingsPage onClose={() => undefined} />);
  expect(screen.getByRole("heading", { name: "Settings" })).toBeTruthy();
});

test("settings search finds pages by the controls they contain", async () => {
  render(<SettingsPage onClose={() => undefined} />);
  await userEvent.type(screen.getByRole("searchbox", { name: "Search settings" }), "shortcut");
  expect(screen.getByRole("button", { name: "Appearance" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Import" })).toBeNull();
  await userEvent.clear(screen.getByRole("searchbox", { name: "Search settings" }));
  expect(screen.getByRole("button", { name: "Import" })).toBeTruthy();
});

test("settings navigation remembers a selected page while keeping content accessible", async () => {
  const service = new FakeService(); service.open();
  const view = render(<ClientProvider client={service.client}><SettingsPage onClose={() => undefined} /></ClientProvider>);
  await userEvent.click(screen.getByRole("button", { name: "Import" }));
  expect(useAppStore.getState().settingsPage).toBe("import");
  expect(screen.getByRole("button", { name: "Import" }).getAttribute("aria-current")).toBe("page");
  view.unmount();
  render(<ClientProvider client={service.client}><SettingsPage onClose={() => undefined} /></ClientProvider>);
  expect(await screen.findByRole("heading", { name: "Bring over what you already have" })).toBeTruthy();
});

test("settings opens the import and extension pages", async () => {
  const service = new FakeService(); service.open();
  render(<ClientProvider client={service.client}><SettingsPage onClose={() => undefined} /></ClientProvider>);
  await userEvent.click(screen.getByRole("button", { name: "Import" }));
  expect(await screen.findByRole("heading", { name: "Bring over what you already have" })).toBeTruthy();
  await userEvent.click(screen.getByRole("button", { name: "Extensions" }));
  expect(await screen.findByText(/No extensions yet/)).toBeTruthy();
});

test("opening an imported conversation in the app never renders the composer", () => {
  const service = new FakeService(); service.open();
  useAppStore.setState({ importedSession: { id: 20, title: "Imported task", origin: "claude", cwd: null, project_id: null, messages: [] } });
  render(<ClientProvider client={service.client}><App /></ClientProvider>);
  expect(screen.getByText(/read.only/i)).toBeTruthy();
  expect(screen.queryByRole("textbox")).toBeNull();
  expect(screen.queryByRole("button", { name: "Send" })).toBeNull();
});

test("the sidebar can be reopened while reading an imported conversation", async () => {
  const service = new FakeService(); service.open();
  useAppStore.setState({ importedSession: { id: 20, title: "Imported task", origin: "claude", cwd: null, project_id: null, messages: [] } });
  render(<ClientProvider client={service.client}><App /></ClientProvider>);
  await userEvent.click(screen.getByRole("button", { name: "Hide the list" }));
  await userEvent.click(screen.getByRole("button", { name: "Show the list" }));
  expect(screen.getByRole("searchbox")).toBeTruthy();
});

test("choosing a language changes the whole app at once and is remembered", async () => {
  const service = new FakeService();
  service.open();
  useAppStore.setState({ agents: ["local"] });
  render(
    <ClientProvider client={service.client}>
      <App />
    </ClientProvider>,
  );
  expect(screen.getByText("What are we working on?")).toBeTruthy();
  await userEvent.click(screen.getByRole("button", { name: "Settings" }));
  await userEvent.selectOptions(screen.getByRole("combobox", { name: "Language" }), "ro");
  expect(screen.getByRole("heading", { name: "Setări" })).toBeTruthy();
  expect(screen.getByText("Cu ce începem azi?")).toBeTruthy();
  expect(document.documentElement.lang).toBe("ro");
  expect(loadPreferences().language).toBe("ro");
});

test("changing a setting applies and saves it at once", async () => {
  render(<SettingsPage onClose={() => undefined} />);
  await userEvent.selectOptions(screen.getByRole("combobox", { name: "Mode" }), "dark");
  expect(document.documentElement.dataset.mode).toBe("dark");
  expect(loadPreferences().mode).toBe("dark");
});

test("recording a free shortcut saves it", async () => {
  render(<SettingsPage onClose={() => undefined} />);
  const field = screen.getByRole("button", { name: /new session/i });
  await userEvent.click(field);
  fireEvent.keyDown(field, { key: "y", ctrlKey: true, shiftKey: true });
  expect(loadPreferences().keybindings["session.new"]).toBe("mod+shift+y");
});

test("recording a taken shortcut warns and changes nothing", async () => {
  render(<SettingsPage onClose={() => undefined} />);
  const field = screen.getByRole("button", { name: /new session/i });
  await userEvent.click(field);
  fireEvent.keyDown(field, { key: ",", ctrlKey: true });
  expect(screen.getByRole("alert").textContent).toContain("Settings");
  expect(loadPreferences().keybindings["session.new"]).toBe("mod+n");
});

test("escape cancels recording, not the whole page", async () => {
  let closed = false;
  render(<SettingsPage onClose={() => { closed = true; }} />);
  const field = screen.getByRole("button", { name: /new session/i });
  await userEvent.click(field);
  fireEvent.keyDown(field, { key: "Escape" });
  expect(closed).toBe(false);
  expect(field.textContent).toContain("Ctrl N");
});

test("escape closes the page", async () => {
  let closed = false;
  render(<SettingsPage onClose={() => { closed = true; }} />);
  await userEvent.keyboard("{Escape}");
  expect(closed).toBe(true);
});

test("settings keeps keyboard focus in the dialog and restores it on close", async () => {
  const trigger = document.createElement("button");
  document.body.append(trigger); trigger.focus();
  const view = render(<SettingsPage onClose={() => undefined} />);
  const close = screen.getByRole("button", { name: "Close" });
  const last = screen.getByRole("button", { name: /reset to defaults/i });
  last.focus(); await userEvent.tab();
  expect(document.activeElement).toBe(close);
  await userEvent.tab({ shift: true });
  expect(document.activeElement).toBe(last);
  view.unmount();
  expect(document.activeElement).toBe(trigger);
  trigger.remove();
});

test("resetting restores the defaults", async () => {
  updatePreferences({ mode: "dark", fontScale: 1.2 });
  render(<SettingsPage onClose={() => undefined} />);
  await userEvent.click(screen.getByRole("button", { name: /reset to defaults/i }));
  expect(usePreferences.getState().preferences.mode).toBe("system");
});

test("send shortcut setting changes the composer", async () => {
  updatePreferences({ sendShortcut: "mod+enter" });
  useAppStore.setState({
    sessions: [{ id: 1, project_id: 1, agent: "local", title: "s", updated_seq: 1, last_turn: "none", running: false }],
  });
  const service = new FakeService();
  service.open();
  service.results["turn.start"] = {};
  render(
    <ClientProvider client={service.client}>
      <Composer sessionId={1} />
    </ClientProvider>,
  );
  const box = screen.getByRole("textbox") as HTMLTextAreaElement;
  await userEvent.type(box, "a{Enter}b");
  expect(service.calls("turn.start")).toHaveLength(0);
  expect(box.value).toBe("a\nb");
  await userEvent.type(box, "{Control>}{Enter}{/Control}");
  await flush();
  expect(service.calls("turn.start")).toHaveLength(1);
});

test("the service can be restarted from settings", async () => {
  let restarted = false;
  render(<SettingsPage onClose={() => undefined} onRestartService={() => { restarted = true; }} />);
  await userEvent.click(screen.getByRole("button", { name: /restart the service/i }));
  expect(restarted).toBe(true);
});

test("a collapsed list still shows that something is waiting", async () => {
  const service = new FakeService();
  service.open();
  useAppStore.setState({
    agents: ["local"],
    sessions: [{ id: 1, project_id: 1, agent: "local", title: "s", updated_seq: 1, last_turn: "none", running: true }],
    approvals: [{ id: "a", session_id: 1, tool: "Bash", arguments: {} }],
  });
  render(
    <ClientProvider client={service.client}>
      <App />
    </ClientProvider>,
  );
  await userEvent.click(screen.getByRole("button", { name: "Hide the list" }));
  expect(screen.getByRole("button", { name: "Show the list, 1 waiting for you" })).toBeTruthy();
});
