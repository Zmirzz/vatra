import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, test } from "vitest";
import { ClientProvider } from "../rpc/context";
import { FakeService } from "../rpc/fake";
import { setLanguage } from "../copy";
import { initialState, useAppStore } from "../state/store";
import { ModelsPage } from "./ModelsPage";

const GIB = 1024 ** 3;
let service: FakeService;
const hardware = { cpu: { name: "CPU", cores: 10, threads: 16 }, ram: { total: 32 * GIB, available: 16 * GIB }, gpus: [{ name: "RTX 5060", backend: "Vulkan0", total: 8 * GIB, free: 7 * GIB, source: "llama", integrated: false }], diskFree: 100 * GIB };
const local = { slug: "qwen3-4b-q4_k_m", agent: "local:qwen3-4b-q4_k_m", repo: "unsloth/Qwen3-4B-Instruct-2507-GGUF", variant: "Q4_K_M", sizeBytes: 2.3 * GIB, running: false, missing: false };
const estimate = (fit: string) => ({ needed: 9 * GIB, weights: 5 * GIB, kv: 3 * GIB, overhead: GIB, ctx: 32768, fit, notes: [] });

function setup(results: Record<string, unknown> = {}) {
  Object.assign(service.results, {
    "hardware.get": hardware,
    "llama.status": { installed: true, install: { server: "llama-server.exe", build: 12019, caps: { fit: true } } },
    "models.installed": [local],
    "models.labels": [{ agent: local.agent, label: "complete", reasons: [], estimated: true, checkedAt: null, llamaBuild: null, stale: false, results: null }],
    ...results,
  });
  render(<ClientProvider client={service.client}><ModelsPage /></ClientProvider>);
}

beforeEach(() => { setLanguage("en"); useAppStore.setState(initialState()); service = new FakeService(); service.open(); });

test("shows the estimate word without a check", async () => {
  setup();
  const card = (await screen.findByText(local.agent)).closest("article") as HTMLElement;
  expect(within(card).getByText("Complete")).toBeTruthy();
  expect(within(card).getByText("Estimate")).toBeTruthy();
  expect(within(card).getByText(/Should handle tools, MCP and agents/)).toBeTruthy();
  expect(within(card).queryByText("Tools, MCP and agents work.")).toBeNull();
  expect(within(card).queryByText(/Short check/)).toBeNull();
  expect(await screen.findByText(/llama.cpp build 12019/)).toBeTruthy();
});

test("measured label shows date, build, reasons and what the check didn't prove", async () => {
  setup({ "models.labels": [{ agent: local.agent, label: "partial", reasons: ["toolsUnreliable", "slow"], estimated: false, checkedAt: 1_700_000_000, llamaBuild: 12019, stale: true, results: { speed: { promptTps: 500, genTps: 3.2, seconds: 4 }, tools: { passed: 1, of: 2 }, toolFailures: [] } }] });
  const card = (await screen.findByText(local.agent)).closest("article") as HTMLElement;
  expect(within(card).getByText("Partial")).toBeTruthy();
  expect(within(card).queryByText("Estimate")).toBeNull();
  expect(within(card).getByText(/with llama.cpp build 12019/)).toBeTruthy();
  expect(within(card).getByText(/one of two tries failed/)).toBeTruthy();
  expect(within(card).getByText("Short check: one tool, one MCP call. Long tasks, subagents and skills weren't tested.")).toBeTruthy();
  expect(within(card).getByText(/Check again/)).toBeTruthy();
  expect(within(card).getByText("3.2 tokens a second")).toBeTruthy();
});

test("download starts directly, no confirmation dialog in the way", async () => {
  setup({ "models.suggest": [{ family: "Qwen3 4B", repo: "unsloth/Qwen3-4B-Instruct-2507-GGUF", variant: "Q4_K_M", size: 2.5 * GIB, estimate: estimate("split"), alternatives: [], unlockVramGib: 12, lowQuality: false, note: "Small and quick." }] });
  await userEvent.click(await screen.findByRole("button", { name: "Show suggestions" }));
  expect(await screen.findByText(/Splits between GPU and RAM/)).toBeTruthy();
  expect(screen.getByText(/With 12 GB of graphics memory/)).toBeTruthy();
  await userEvent.click(screen.getByRole("button", { name: "Download" }));
  expect(service.calls("models.download")).toEqual([{ repo: "unsloth/Qwen3-4B-Instruct-2507-GGUF", variant: "Q4_K_M", mmproj: false }]);
  expect(screen.queryByRole("alertdialog")).toBeNull();
});

test("a variant that is too large can still be downloaded, with a warning", async () => {
  setup({
    "models.search": [{ id: "big/Model-GGUF", downloads: 1200, likes: 3, gated: false }],
    "models.files": { repo: "big/Model-GGUF", gated: false, ctx: 32768, label: "complete", reasons: ["notEnoughMemory"], variants: [{ name: "Q8_0", size: 40 * GIB, mmproj: [], estimate: estimate("tooLarge"), lowQuality: false }] },
  });
  await userEvent.type(await screen.findByRole("textbox", { name: "Search Hugging Face" }), "big");
  await userEvent.click(screen.getByRole("button", { name: "Search" }));
  await userEvent.click(await screen.findByRole("button", { name: /big\/Model-GGUF/ }));
  expect(await screen.findByText(/Probably too large for this computer. You can still try it./)).toBeTruthy();
  expect((screen.getByRole("button", { name: "Download" }) as HTMLButtonElement).disabled).toBe(false);
});

test("checking an API model asks about cost first", async () => {
  setup({ "models.installed": [], "models.labels": [{ agent: "api", label: "notChecked", reasons: [], estimated: true, checkedAt: null, llamaBuild: null, stale: false, results: null }] });
  await userEvent.click(await screen.findByRole("button", { name: "Check" }));
  expect(service.calls("models.check")).toEqual([]);
  const dialog = screen.getByRole("alertdialog", { name: "This check may cost money" });
  expect(within(dialog).getByText(/may charge/)).toBeTruthy();
  await userEvent.click(within(dialog).getByRole("button", { name: "Continue" }));
  expect(service.calls("models.check")).toEqual([{ agent: "api", consentCost: true }]);
});

test("a local check runs without a cost question", async () => {
  setup();
  await userEvent.click(await screen.findByRole("button", { name: "Check" }));
  expect(service.calls("models.check")).toEqual([{ agent: local.agent, consentCost: false }]);
});

test("missing llama.cpp shows the winget command", async () => {
  setup({ "llama.status": { installed: false, installCommand: "winget install ggml.llamacpp" } });
  expect(await screen.findByText("winget install ggml.llamacpp")).toBeTruthy();
});

test("Romanian labels", async () => {
  setLanguage("ro");
  setup();
  expect(await screen.findByText("Complet")).toBeTruthy();
  expect(screen.getByText("Estimare")).toBeTruthy();
});
