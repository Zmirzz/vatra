import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, test } from "vitest";
import { App } from "../App";
import { setLanguage } from "../copy";
import { ClientProvider } from "../rpc/context";
import { FakeService } from "../rpc/fake";
import { initialState, useAppStore } from "../state/store";
import { SettingsPage } from "./SettingsPage";
import { ModelsPage } from "./ModelsPage";
import { AgentForm } from "./AgentForm";
import { Onboarding } from "./Onboarding";
import { ChatgptAccount } from "./ChatgptAccount";

let service: FakeService;
beforeEach(() => { setLanguage("en"); useAppStore.setState(initialState()); service = new FakeService(); service.open();
  Object.assign(service.results, { "agents.configured": { agents: [], summarizer: null }, "agents.providers": [], "agents.detect": { acpScripts: [] }, "models.suggest": [], "hardware.get": { cpu: {}, ram: { total: 0, available: 0 }, gpus: [] }, "llama.status": { installed: false, installCommand: "winget install ggml.llamacpp" }, "models.installed": [], "models.labels": [], "onboarding.state": { done: false, sources: false } });
});

test("Agents is reachable from Settings and setup can be reopened", async () => {
  render(<ClientProvider client={service.client}><SettingsPage onClose={() => undefined} /></ClientProvider>);
  await userEvent.click(screen.getByRole("button", { name: "Agents" }));
  expect(await screen.findByRole("button", { name: "Add an agent" })).toBeTruthy();
});

test("setup opens for a first empty agent list and every step can be skipped", async () => {
  useAppStore.setState({ agentsLoaded: true });
  render(<ClientProvider client={service.client}><App /></ClientProvider>);
  const dialog = await screen.findByRole("dialog", { name: "Let's set Vatra up" });
  await userEvent.click(within(dialog).getByRole("button", { name: "Continue" }));
  expect(within(dialog).getByRole("heading", { name: "Choose a model" })).toBeTruthy();
  await userEvent.click(within(dialog).getByRole("button", { name: "Skip" }));
  await userEvent.click(within(dialog).getByRole("button", { name: "Skip" }));
  await userEvent.click(within(dialog).getByRole("button", { name: "Skip" }));
  expect(service.calls("onboarding.done")).toEqual([{}]);
});

test("Models offers installation but waits for confirmation", async () => {
  service.results["deps.status"] = { winget: true, packages: [{ id: "ggml.llamacpp", name: "llama.cpp", purpose: "localModels", command: "winget install --id ggml.llamacpp", installed: false }] };
  render(<ClientProvider client={service.client}><ModelsPage /></ClientProvider>);
  await userEvent.click(await screen.findByRole("button", { name: "Install llama.cpp" }));
  const dialog = await screen.findByRole("dialog", { name: "Install llama.cpp?" });
  expect(service.calls("deps.install")).toEqual([]);
  expect(within(dialog).getByText(/license/)).toBeTruthy();
  await userEvent.click(within(dialog).getByRole("button", { name: "Install" }));
  expect(service.calls("deps.install")).toEqual([{ id: "ggml.llamacpp", consent: true }]);
});

test("API task runs require cost confirmation and show the last result", async () => {
  service.results["models.labels"] = [{ agent: "api", label: "notChecked", reasons: [], estimated: true, results: null }];
  service.results["parity.results"] = [{ agent: "api", model: "model-x", build: "test", startedAt: 1700000000, results: Array.from({ length: 6 }, (_, i) => ({ taskId: String(i), outcome: { passed: true, failures: [] }, metrics: {} })) }];
  render(<ClientProvider client={service.client}><ModelsPage /></ClientProvider>);
  expect(await screen.findByText(/6 \/ 6.*model-x/)).toBeTruthy();
  await userEvent.click(screen.getByRole("button", { name: "Run six tasks" }));
  expect(service.calls("parity.run")).toEqual([]);
  const dialog = screen.getByRole("alertdialog");
  expect(within(dialog).getByText(/estimate.*unavailable/i)).toBeTruthy();
  await userEvent.click(within(dialog).getByRole("button", { name: "Continue" }));
  expect(service.calls("parity.run")).toEqual([{ agent: "api", consentCost: true }]);
});

test("saving a model clears its password field and editing preserves an existing key", async () => {
  service.results["agents.providers"] = [{ id: "openrouter", name: "OpenRouter", baseUrl: "https://openrouter.ai/api/v1", driver: "openai" }];
  service.results["agents.save"] = { restartNeeded: true, runningTurns: 0 };
  render(<ClientProvider client={service.client}><AgentForm initial={{ kind: "model", name: "test", provider: "openrouter", model: "model-x" }} onSaved={() => undefined} /></ClientProvider>);
  const input = await screen.findByLabelText("Key");
  expect((input as HTMLInputElement).type).toBe("password");
  await userEvent.type(input, "synthetic-test-key");
  await userEvent.click(screen.getByRole("button", { name: "Save agent" }));
  expect((input as HTMLInputElement).value).toBe("");
  await userEvent.click(screen.getByRole("button", { name: "Save agent" }));
  expect(service.calls("agents.save")[1]).not.toHaveProperty("key");
});

test("setup includes Import only when sources exist and opens a conversation with the chosen agent", async () => {
  useAppStore.setState({ agents: ["local:one", "local:two"], projects: [{ id: 4, name: "Work", path: null }] });
  service.results["sessions.create"] = { id: 8 };
  render(<ClientProvider client={service.client}><Onboarding sources onDone={() => undefined} /></ClientProvider>);
  await userEvent.click(screen.getByRole("button", { name: "Continue" }));
  expect(screen.getByRole("heading", { name: "Bring your tools over" })).toBeTruthy();
  await userEvent.click(screen.getByRole("button", { name: "Skip" }));
  await userEvent.click(screen.getByRole("button", { name: "Skip" }));
  await userEvent.click(screen.getByRole("button", { name: "Skip" }));
  await userEvent.selectOptions(screen.getByRole("combobox", { name: "Agents" }), "local:two");
  await userEvent.click(screen.getByRole("button", { name: "Start a conversation" }));
  expect(service.calls("sessions.create")).toEqual([{ project_id: 4, agent: "local:two" }]);
  expect(useAppStore.getState().activeSessionId).toBe(8);
  expect(service.calls("turn.start")).toEqual([]);
});

test("native comparison uses an explicit model and waits for separate cost consent", async () => {
  service.results["models.labels"] = [{ agent: "local:one", label: "notChecked", reasons: [], estimated: true, results: null }];
  render(<ClientProvider client={service.client}><ModelsPage /></ClientProvider>);
  await userEvent.click(await screen.findByText("Compare with the installed app"));
  await userEvent.type(screen.getByRole("textbox", { name: "Model" }), "same-model");
  await userEvent.click(screen.getByRole("button", { name: "Compare" }));
  expect(service.calls("parity.native")).toEqual([]);
  const dialog = screen.getByRole("alertdialog");
  await userEvent.click(within(dialog).getByRole("button", { name: "Continue" }));
  expect(service.calls("parity.native")).toEqual([{ kind: "claude", model: "same-model", consentCost: true }]);
});

test("account selection sends only the account id", async () => {
  service.results["chatgpt.status"] = { connected: true, sharing: true, account: "first@example.test" };
  service.results["chatgpt.accounts"] = [{ id: "one", account: "first@example.test", active: true }, { id: "two", account: "second@example.test", active: false }];
  render(<ClientProvider client={service.client}><ChatgptAccount /></ClientProvider>);
  await userEvent.selectOptions(await screen.findByRole("combobox", { name: "Active account" }), "two");
  expect(service.calls("chatgpt.select")).toEqual([{ id: "two" }]);
});

test("Hugging Face token is hidden and cleared after storage", async () => {
  service.results["hf.status"] = { hasToken: false };
  service.results["hf.set_token"] = { hasToken: true };
  render(<ClientProvider client={service.client}><ModelsPage /></ClientProvider>);
  await userEvent.click(screen.getByText("Hugging Face token"));
  const input = screen.getByLabelText("Hugging Face token");
  expect((input as HTMLInputElement).type).toBe("password");
  await userEvent.type(input, "hf_synthetic");
  await userEvent.click(screen.getByRole("button", { name: "Save token" }));
  expect((input as HTMLInputElement).value).toBe("");
  expect(service.calls("hf.set_token")).toEqual([{ token: "hf_synthetic" }]);
});

test("subscription agents offer native comparison while isolated engine task runs stay unavailable", async () => {
  useAppStore.setState({ agents: ["claude"] });
  render(<ClientProvider client={service.client}><ModelsPage /></ClientProvider>);
  const card = (await screen.findByText("claude")).closest("article") as HTMLElement;
  expect(within(card).queryByRole("button", { name: "Run six tasks" })).toBeNull();
  expect(service.calls("parity.run")).toEqual([]);
  expect(within(card).getByText(/Isolated task runs are available only for local and API model agents/)).toBeTruthy();
  await userEvent.click(within(card).getByText("Compare with the installed app"));
  expect(within(card).getByRole("button", { name: "Compare" })).toBeTruthy();
  const codex = within(card).getByRole("option", { name: "Codex" }) as HTMLOptionElement;
  expect(codex.disabled).toBe(true);
  expect(within(card).getByText(/Codex comparison is unavailable/)).toBeTruthy();
});

test("configured file agents are read-only and a saved change warns about active turns", async () => {
  service.results["agents.configured"] = { agents: [{ name: "from-file", kind: "model", source: "file", model: "model-x" }, { name: "from-app", kind: "model", source: "app", model: "model-y" }], summarizer: null };
  service.results["agents.remove"] = { restartNeeded: true, runningTurns: 2 };
  render(<ClientProvider client={service.client}><SettingsPage onClose={() => undefined} /></ClientProvider>);
  await userEvent.click(screen.getByRole("button", { name: "Agents" }));
  const fileItem = (await screen.findByText("from-file", { selector: "b" })).closest("li") as HTMLElement;
  expect(within(fileItem).getByText("From vatra.toml")).toBeTruthy();
  expect(within(fileItem).queryByRole("button", { name: "Change" })).toBeNull();
  expect(within(fileItem).queryByRole("button", { name: "Remove" })).toBeNull();
  await userEvent.click(screen.getByRole("button", { name: "Remove" }));
  await userEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Remove" }));
  expect(await screen.findByText(/2 conversation is still working/)).toBeTruthy();
  expect(service.calls("service.shutdown")).toEqual([]);
});

test("the model step offers llama installation even if suggestions could not load", async () => {
  render(<ClientProvider client={service.client}><Onboarding sources={false} onDone={() => undefined} /></ClientProvider>);
  await userEvent.click(screen.getByRole("button", { name: "Continue" }));
  expect(await screen.findByRole("button", { name: "Install llama.cpp" })).toBeTruthy();
});

test("Codex keeps the person's own login unless the ChatGPT account of Vatra is chosen", async () => {
  service.results["agents.detect"] = { claude: null, codex: "C:/bin/codex.exe", node: "C:/node/node.exe", acpScripts: [["codex-acp", "C:/npm/codex-acp.js"]] };
  service.results["agents.save"] = { restartNeeded: true, runningTurns: 0 };
  render(<ClientProvider client={service.client}><AgentForm kinds={["acp"]} onSaved={() => undefined} /></ClientProvider>);
  expect(await screen.findByText(/uses the account you're already signed in to/)).toBeTruthy();
  await userEvent.click(screen.getByRole("button", { name: "Save agent" }));
  expect(service.calls("agents.save")[0]).toMatchObject({ kind: "acp", managedAccount: false, codexBinary: null });
  await userEvent.click(screen.getByRole("checkbox", { name: "Use the ChatGPT account connected in Vatra" }));
  expect(screen.getByText(/not on the one you're signed in to in Codex/)).toBeTruthy();
  await userEvent.click(screen.getByRole("button", { name: "Save agent" }));
  expect(service.calls("agents.save")[1]).toMatchObject({ managedAccount: true, subscription: true, codexBinary: "C:/bin/codex.exe" });
});

test("the managed ChatGPT account cannot be saved when Codex isn't found", async () => {
  service.results["agents.detect"] = { claude: null, codex: null, node: "C:/node/node.exe", acpScripts: [["codex-acp", "C:/npm/codex-acp.js"]] };
  render(<ClientProvider client={service.client}><AgentForm kinds={["acp"]} onSaved={() => undefined} /></ClientProvider>);
  await userEvent.click(await screen.findByRole("checkbox", { name: "Use the ChatGPT account connected in Vatra" }));
  expect(screen.getByText(/Codex wasn't found on this computer/)).toBeTruthy();
  expect((screen.getByRole("button", { name: "Save agent" }) as HTMLButtonElement).disabled).toBe(true);
});
