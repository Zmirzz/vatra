import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, test, vi } from "vitest";
import { ClientProvider } from "../rpc/context";
import { FakeService, flush } from "../rpc/fake";
import { setLanguage } from "../copy";
import { initialState, useAppStore } from "../state/store";
import { AddModelDialog } from "./AddModelDialog";
import { MarketPage } from "./MarketPage";
import { ModelPicker } from "./ModelPicker";

let service: FakeService;
const configured = [
  { name: "claude", kind: "claude", source: "app", provider: null, model: null, hasKey: false },
  { name: "codex", kind: "acp", source: "app", provider: null, model: null, hasKey: false },
  { name: "openrouter", kind: "model", source: "app", provider: "openrouter", model: "qwen/qwen3", hasKey: true },
];
const providers = [{ id: "openrouter", name: "OpenRouter", baseUrl: "https://openrouter.ai/api/v1", driver: "openai", modelsUrl: "https://openrouter.ai/api/v1/models", keyUrl: "https://openrouter.ai/keys" }];
const ui = (node: React.ReactNode) => render(<ClientProvider client={service.client}>{node}</ClientProvider>);
beforeEach(() => {
  setLanguage("en");
  useAppStore.setState({ ...initialState(), agents: ["claude", "codex", "openrouter", "local:qwen"] });
  service = new FakeService(); service.open();
  service.results["agents.configured"] = { agents: configured, summarizer: null };
  service.results["agents.providers"] = providers;
});

test("the list is grouped in words a person knows, and the chosen one is marked", async () => {
  ui(<ModelPicker value="claude" onSelect={() => undefined} />);
  await userEvent.click(screen.getByRole("button", { name: "Model: Claude Code" }));
  await flush();
  const groups = screen.getAllByRole("group").map((group) => group.getAttribute("aria-label"));
  expect(groups).toEqual(["Claude account", "ChatGPT account", "On this computer", "OpenRouter"]);
  expect(screen.getByRole("option", { name: /Claude Code/ }).getAttribute("aria-selected")).toBe("true");
  expect(screen.getByRole("option", { name: "qwen" })).toBeTruthy();
});

test("search narrows the list and Enter picks the highlighted model", async () => {
  const onSelect = vi.fn();
  ui(<ModelPicker value="claude" onSelect={onSelect} />);
  await userEvent.click(screen.getByRole("button", { name: /Model:/ }));
  await flush();
  await userEvent.type(screen.getByRole("combobox"), "qwen");
  expect(screen.getAllByRole("option")).toHaveLength(2);
  await userEvent.keyboard("{ArrowDown}{Enter}");
  expect(onSelect).toHaveBeenCalledWith("openrouter");
  expect(screen.queryByRole("listbox")).toBeNull();
});

test("the side bar shows one kind of model at a time", async () => {
  ui(<ModelPicker value="claude" onSelect={() => undefined} />);
  await userEvent.click(screen.getByRole("button", { name: /Model:/ }));
  await flush();
  await userEvent.click(screen.getByRole("button", { name: "On this computer" }));
  expect(screen.getAllByRole("option")).toHaveLength(1);
  await userEvent.click(screen.getByRole("button", { name: "On this computer" }));
  expect(screen.getAllByRole("option").length).toBeGreaterThan(1);
});

test("Escape closes the list", async () => {
  ui(<ModelPicker value="claude" onSelect={() => undefined} />);
  await userEvent.click(screen.getByRole("button", { name: /Model:/ }));
  await userEvent.keyboard("{Escape}");
  expect(screen.queryByRole("listbox")).toBeNull();
});

test("with no model yet the button goes straight to adding one", async () => {
  useAppStore.setState({ agents: [] });
  ui(<ModelPicker value="" onSelect={() => undefined} />);
  await userEvent.click(screen.getByRole("button", { name: "Model: Add a model" }));
  expect(screen.getByRole("dialog", { name: "What do you want to add?" })).toBeTruthy();
});

test("the list offers adding, and adding Claude is one click", async () => {
  service.results["agents.detect"] = { claude: "C:/claude.exe", codex: null, node: null, acpScripts: [] };
  service.results["agents.save"] = { restartNeeded: true, runningTurns: 0 };
  useAppStore.setState({ agents: ["openrouter"] });
  ui(<ModelPicker value="openrouter" onSelect={() => undefined} />);
  await userEvent.click(screen.getByRole("button", { name: /Model:/ }));
  await userEvent.click(screen.getByRole("button", { name: "Add a model or account" }));
  const dialog = screen.getByRole("dialog", { name: "What do you want to add?" });
  await userEvent.click(within(dialog).getByRole("button", { name: /My Claude or ChatGPT account/ }));
  await flush();
  expect(within(dialog).getByText(/Claude Code is on this computer/)).toBeTruthy();
  expect(within(dialog).getByText(/Codex isn't installed/)).toBeTruthy();
  await userEvent.click(within(dialog).getAllByRole("button", { name: "Add" })[0]);
  await flush();
  expect(service.calls("agents.save")).toEqual([{ kind: "claude", binary: "C:/claude.exe" }]);
  expect(await screen.findByText(/Restart Vatra's service/)).toBeTruthy();
});

test("a service with a key picks a model and keeps the address under Details", async () => {
  service.results["agents.provider_models"] = [
    { id: "paid/one", name: "Paid", tools: true, free: false, contextLength: null },
    { id: "free/two", name: "Free", tools: true, free: true, contextLength: null },
    { id: "no/tools", name: "None", tools: false, free: true, contextLength: null },
  ];
  service.results["agents.save"] = { restartNeeded: true, runningTurns: 0 };
  ui(<AddModelDialog onClose={() => undefined} />);
  await userEvent.click(screen.getByRole("button", { name: /A service with a key/ }));
  await flush();
  expect((screen.getByLabelText("Model") as HTMLInputElement).value).toBe("free/two");
  expect((screen.getByLabelText("Key") as HTMLInputElement).type).toBe("password");
  await userEvent.type(screen.getByLabelText("Key"), "synthetic-key");
  await userEvent.click(screen.getByRole("button", { name: "Add model" }));
  await flush();
  expect(service.calls("agents.save")).toEqual([{ kind: "model", name: "openrouter", provider: "openrouter", baseUrl: undefined, model: "free/two", key: "synthetic-key" }]);
});

test("GitHub Copilot signs in with a code, then lists its models and saves the token as the key", async () => {
  service.results["agents.providers"] = [...providers, { id: "copilot", name: "GitHub Copilot", baseUrl: "https://api.githubcopilot.com", driver: "copilot", modelsUrl: null, keyUrl: null }];
  service.results["copilot.start"] = { deviceCode: "dev", userCode: "ABCD-1234", verificationUri: "https://github.com/login/device", interval: 1, expiresIn: 900 };
  service.results["copilot.poll"] = { state: "done", token: "gho_synthetic" };
  service.results["copilot.models"] = [{ id: "gpt-4.1", name: "GPT-4.1", tools: true, free: false, contextLength: null }];
  service.results["agents.provider_models"] = [];
  service.results["agents.save"] = { restartNeeded: true, runningTurns: 0 };
  vi.spyOn(window, "open").mockReturnValue(null);
  ui(<AddModelDialog onClose={() => undefined} />);
  await userEvent.click(screen.getByRole("button", { name: /A service with a key/ }));
  await flush();
  await userEvent.selectOptions(screen.getByLabelText("Service"), "copilot");
  expect(screen.queryByLabelText("Key")).toBeNull();
  await userEvent.click(screen.getByRole("button", { name: "Sign in with GitHub" }));
  expect(await screen.findByText("ABCD-1234")).toBeTruthy();
  expect(await screen.findByText("Signed in to GitHub.", undefined, { timeout: 3000 })).toBeTruthy();
  await waitFor(() => expect((screen.getByLabelText("Model") as HTMLInputElement).value).toBe("gpt-4.1"));
  await userEvent.click(screen.getByRole("button", { name: "Add model" }));
  await flush();
  expect(service.calls("agents.save")).toEqual([{ kind: "model", name: "copilot", provider: "copilot", baseUrl: undefined, model: "gpt-4.1", key: "gho_synthetic" }]);
});

test("the local and import cards open their own pages", async () => {
  ui(<AddModelDialog onClose={() => undefined} />);
  await userEvent.click(screen.getByRole("button", { name: /Bring over from Claude Code or Codex/ }));
  expect(useAppStore.getState().settingsPage).toBe("import");
});

const listing = (id: string, needs: string, more = {}) => ({ id, name: id, needs, token: null, runsProgram: false, connected: false, ...more });

test("the market adds a connection with one click and refreshes the list", async () => {
  service.results["market.list"] = [listing("context7", "nothing"), listing("notion", "account")];
  service.results["market.install"] = { name: "context7", login: false };
  ui(<MarketPage />);
  expect(await screen.findByText("Up-to-date documentation for coding libraries.")).toBeTruthy();
  service.results["market.list"] = [listing("context7", "nothing", { connected: true }), listing("notion", "account")];
  await userEvent.click(screen.getAllByRole("button", { name: "Add" })[0]);
  await flush();
  expect(service.calls("market.install")).toEqual([{ id: "context7", token: undefined }]);
  expect(await screen.findByText("Added")).toBeTruthy();
});

test("a connection that needs a token asks for it, and never shows it again", async () => {
  service.results["market.list"] = [listing("github", "token", { token: { getUrl: "https://github.com/settings/tokens" } })];
  service.results["market.install"] = { name: "github", login: false };
  ui(<MarketPage />);
  await userEvent.click(await screen.findByRole("button", { name: "Add" }));
  const field = screen.getByLabelText("Access token") as HTMLInputElement;
  expect(field.type).toBe("password");
  expect(screen.getByRole("link", { name: "Get a token" }).getAttribute("href")).toBe("https://github.com/settings/tokens");
  expect((screen.getByRole("button", { name: "Add" }) as HTMLButtonElement).disabled).toBe(true);
  await userEvent.type(field, "ghp_synthetic");
  await userEvent.click(screen.getByRole("button", { name: "Add" }));
  await flush();
  expect(service.calls("market.install")).toEqual([{ id: "github", token: "ghp_synthetic" }]);
  expect(screen.queryByDisplayValue("ghp_synthetic")).toBeNull();
});

test("an account connection offers sign-in once added, and search filters", async () => {
  service.results["market.list"] = [listing("notion", "account", { connected: true }), listing("sentry", "account")];
  service.results["mcp.login"] = {};
  ui(<MarketPage />);
  await userEvent.click(await screen.findByRole("button", { name: "Sign in" }));
  expect(service.calls("mcp.login")).toEqual([{ server: "notion" }]);
  expect(await screen.findByText(/Sign-in opened in your browser/)).toBeTruthy();
  await userEvent.type(screen.getByRole("searchbox", { name: "Search connections" }), "zzz");
  expect(screen.getByText("No connection matches.")).toBeTruthy();
});

test("a failed install says so", async () => {
  service.results["market.list"] = [listing("context7", "nothing")];
  ui(<MarketPage />);
  service.failures["market.install"] = { code: -32603, message: "the catalog is locked" };
  await userEvent.click(await screen.findByRole("button", { name: "Add" }));
  expect((await screen.findByRole("alert")).textContent).toContain("the catalog is locked");
});

test("a provider exposes models and effort without changing agent", async () => {
  service.results["agents.options"] = { models: [{ id: "new/model", name: "New Model", efforts: [{ id: "high", name: "High" }] }], efforts: [] };
  const onOptionsChange = vi.fn();
  const onSelect = vi.fn();
  ui(<ModelPicker value="openrouter" onSelect={onSelect} onOptionsChange={onOptionsChange} />);
  await userEvent.click(screen.getByRole("button", { name: /Model:/ }));
  await userEvent.click(within(await screen.findByRole("group", { name: "OpenRouter" })).getByRole("option", { name: /New Model/ }));
  expect(onOptionsChange).toHaveBeenLastCalledWith({ model: "new/model" });
  expect(screen.queryByText("new/model")).toBeNull();
  await userEvent.selectOptions(screen.getByRole("combobox", { name: "Thinking effort" }), "high");
  expect(onOptionsChange).toHaveBeenLastCalledWith({ model: "new/model", effort: "high" });
  expect(screen.getByRole("button", { name: /Model: New Model/ }).textContent).toContain(" · High");
  await userEvent.selectOptions(screen.getByRole("combobox", { name: "Thinking effort" }), "");
  expect(onOptionsChange).toHaveBeenLastCalledWith({ model: "new/model" });
  expect(onSelect).not.toHaveBeenCalled();
});

test("the portal keeps its search inside the viewport and dismisses outside", async () => {
  const { container } = ui(<ModelPicker value="claude" onSelect={() => undefined} />);
  const trigger = screen.getByRole("button", { name: /Model:/ });
  vi.spyOn(trigger, "getBoundingClientRect").mockReturnValue({ top: 700, bottom: 730, left: 500, right: 650 } as DOMRect);
  await userEvent.click(trigger);
  const dialog = screen.getByRole("dialog", { name: "Model" });
  expect(container.contains(dialog)).toBe(false);
  expect(dialog.style.bottom).not.toBe("");
  await userEvent.click(screen.getByRole("combobox"));
  expect(screen.getByRole("dialog", { name: "Model" })).toBeTruthy();
  await userEvent.keyboard("{Escape}");
  expect(screen.queryByRole("dialog", { name: "Model" })).toBeNull();
  expect(document.activeElement).toBe(trigger);
  await userEvent.click(trigger);
  await userEvent.click(document.body);
  expect(screen.queryByRole("dialog", { name: "Model" })).toBeNull();
});

test("ACP agents other than Codex keep their own group", async () => {
  useAppStore.setState({ agents: ["antigravity"] });
  service.results["agents.configured"] = { agents: [{ name: "antigravity", kind: "acp" }] };
  ui(<ModelPicker value="antigravity" onSelect={() => undefined} />);
  await userEvent.click(screen.getByRole("button", { name: /Model:/ }));
  await flush();
  expect(screen.queryByRole("group", { name: "ChatGPT account" })).toBeNull();
  expect(screen.getByRole("group", { name: /antigravity/i })).toBeTruthy();
});
