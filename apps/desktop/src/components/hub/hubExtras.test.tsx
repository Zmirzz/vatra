import {beforeEach, describe, expect, it, vi} from "vitest";
import {act, render, screen, waitFor, within} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {FakeService} from "../../rpc/fake";
import {ClientProvider} from "../../rpc/context";
import {initialState, useAppStore} from "../../state/store";
import {ToastHost, toast, useToasts} from "../../hub/toast";
import {CommandPalette} from "../CommandPalette";
import {AccountsPage} from "./AccountsPage";
import {ResearchPage} from "./ResearchPage";
import {SchedulePage} from "./SchedulePage";
import {StatusPage} from "./StatusPage";

function setup() {
  const fake = new FakeService();
  fake.open();
  return fake;
}
const view = (fake: FakeService, node: React.ReactNode) => render(<ClientProvider client={fake.client}>{node}<ToastHost /></ClientProvider>);

const finished = {
  id: 3, question: "Cat costa abonamentul?", depth: "standard", state: "done", step: "", plan: ["pret abonament"],
  sources: [
    {n: 1, url: "https://a.example/x", title: "Pagina A", status: "read", kind: "web", round: 1},
    {n: 2, url: "https://b.example/paper.pdf", title: "Raport", status: "read", kind: "pdf", round: 2},
    {n: 3, url: "https://c.example", title: "Gol", status: "unreadable", kind: "web", round: 1, reason: "it needs JavaScript to show its content."},
  ],
  report: "Costa 49 lei [1] si 39 lei [2].\n\nSources\n[1] Pagina A - https://a.example/x\n", followups: [], error: null, created_at: 1, finished_at: 2,
  stats: {rounds: 2, sources: {read: 2, not_relevant: 0, unreadable: 1}, hosts: 2, kinds: {web: 1, pdf: 1}, small_model: false, partial: false, search_errors: ["brave: The search service says there were too many requests."]},
  options: {},
};

beforeEach(() => {
  useToasts.setState({toasts: []});
  useAppStore.setState({...initialState(), agents: ["alfa", "beta"], projects: [{id: 1, name: "Casa", path: null}]});
});

describe("research", () => {
  function research(fake: FakeService) {
    fake.results["research.ready"] = {ready: true, providers: ["brave", "duckduckgo"]};
    fake.results["research.profile"] = {tier: "small", small_model: true, reason: "This computer fits small models, so research runs in light mode."};
    fake.results["research.list"] = [{id: 3, question: finished.question, depth: "standard", state: "done", step: "", created_at: 1, finished_at: 2}];
    fake.results["research.get"] = finished;
  }

  it("asks the clarifying questions first and starts with the answers", async () => {
    const fake = setup();
    research(fake);
    fake.results["research.clarify"] = {questions: ["For which country?", "For personal or company use?"]};
    fake.results["research.start"] = {id: 3};
    view(fake, <ResearchPage onOpenSettings={() => undefined} />);
    await userEvent.type(await screen.findByLabelText("What do you want to find out?"), "abonament");
    await userEvent.click(screen.getByRole("button", {name: "Start research"}));
    expect(await screen.findByText("A few questions first")).toBeTruthy();
    expect(fake.calls("research.start")).toHaveLength(0);
    await userEvent.type(screen.getByLabelText("For which country?"), "Romania");
    await userEvent.click(screen.getByRole("button", {name: "Start with these answers"}));
    await waitFor(() => expect(fake.calls("research.start")).toHaveLength(1));
    expect(fake.calls("research.start")[0]).toMatchObject({question: "abonament", context: "For which country? Romania"});
  });

  it("can skip the questions, and light mode is suggested for a small computer", async () => {
    const fake = setup();
    research(fake);
    fake.results["research.clarify"] = {questions: ["Which period?"]};
    fake.results["research.start"] = {id: 3};
    view(fake, <ResearchPage onOpenSettings={() => undefined} />);
    expect(await screen.findByText(/Suggested for this computer/)).toBeTruthy();
    expect((screen.getByLabelText(/Light mode for small models/) as HTMLInputElement).checked).toBe(true);
    expect(screen.getByText("Searching with Brave Search, DuckDuckGo")).toBeTruthy();
    await userEvent.type(screen.getByLabelText("What do you want to find out?"), "abonament");
    await userEvent.click(screen.getByRole("button", {name: "Start research"}));
    await userEvent.click(await screen.findByRole("button", {name: "Skip and start"}));
    await waitFor(() => expect(fake.calls("research.start")[0]).toMatchObject({small_model: true, context: ""}));
  });

  it("shows how the research went, the kind and round of each source, and why a page could not be read", async () => {
    const fake = setup();
    research(fake);
    view(fake, <ResearchPage onOpenSettings={() => undefined} />);
    await userEvent.click(await screen.findByRole("button", {name: /Cat costa abonamentul/}));
    const stats = await screen.findByRole("list", {name: "How this went"});
    expect(within(stats).getByText("2 rounds")).toBeTruthy();
    expect(within(stats).getByText("2 sources read")).toBeTruthy();
    expect(within(stats).getByText("1 PDF")).toBeTruthy();
    expect(screen.getByText(/PDF · Round 2/)).toBeTruthy();
    expect(screen.getByText("it needs JavaScript to show its content.")).toBeTruthy();
    expect(screen.getByText("Why searches failed")).toBeTruthy();
  });

  it("marks a partial report", async () => {
    const fake = setup();
    research(fake);
    fake.results["research.get"] = {...finished, stats: {...finished.stats, partial: true}, error: "The full report couldn't be written."};
    view(fake, <ResearchPage onOpenSettings={() => undefined} />);
    await userEvent.click(await screen.findByRole("button", {name: /Cat costa abonamentul/}));
    expect(await screen.findByText(/The report couldn't be written in full/)).toBeTruthy();
  });

  it("exports, saves as a document, copies and discusses, each with a message", async () => {
    const fake = setup();
    research(fake);
    fake.results["research.export"] = {text: "# x", filename: "cat.md", mime: "text/markdown"};
    fake.results["research.save_document"] = {item_id: 9};
    fake.results["research.discuss"] = {session_id: 41};
    fake.results["sessions.list"] = [];
    const created = vi.fn(() => "blob:x");
    URL.createObjectURL = created as never;
    URL.revokeObjectURL = (() => undefined) as never;
    const write = vi.fn(async () => undefined);
    Object.defineProperty(navigator, "clipboard", {value: {writeText: write}, configurable: true});
    view(fake, <ResearchPage onOpenSettings={() => undefined} />);
    await userEvent.click(await screen.findByRole("button", {name: /Cat costa abonamentul/}));

    await userEvent.click(await screen.findByRole("button", {name: "Copy"}));
    await waitFor(() => expect(write).toHaveBeenCalledWith(finished.report));

    await userEvent.click(screen.getByRole("button", {name: "Export"}));
    await userEvent.click(screen.getByRole("button", {name: "Markdown (.md)"}));
    await waitFor(() => expect(fake.calls("research.export")).toEqual([{id: 3, format: "markdown"}]));
    expect(created).toHaveBeenCalled();

    await userEvent.click(screen.getByRole("button", {name: "Save as a document"}));
    await userEvent.click(screen.getByRole("button", {name: "Save"}));
    await waitFor(() => expect(fake.calls("research.save_document")).toEqual([{id: 3, project_id: 1}]));
    expect(await screen.findByText("Saved in Documents.")).toBeTruthy();

    await userEvent.click(screen.getByRole("button", {name: "Discuss"}));
    await userEvent.click(screen.getAllByRole("button", {name: "Discuss"}).at(-1)!);
    await waitFor(() => expect(fake.calls("research.discuss")).toEqual([{id: 3, agent: "alfa"}]));
    await waitFor(() => expect(useAppStore.getState().activeSessionId).toBe(41));
  });
});

describe("scheduling a research", () => {
  it("saves a research task with its depth and light mode, and links a finished run to its report", async () => {
    const fake = setup();
    fake.results["schedule.list"] = {
      tasks: [{id: 5, name: "Preturi", prompt: "Cat costa?", agent: "", project_id: null, schedule: {kind: "daily", hour: 8, minute: 0, tz_offset_min: 120}, enabled: true, next_run_at: 1_900_000_000, last_run_at: null, created_at: 1, kind: "research", options: {depth: "deep"}}],
      runs: [{id: 1, task_id: 5, scheduled_for: 1, started_at: 1_800_000_000, finished_at: 1_800_000_100, state: "done", session_id: null, summary: "Costa 49 lei.", research_id: 77}],
    };
    view(fake, <SchedulePage />);
    expect(await screen.findByText(/Research · deep/)).toBeTruthy();
    await userEvent.click(screen.getByRole("button", {name: "Open the report"}));

    await userEvent.click(screen.getByRole("button", {name: "New scheduled task"}));
    await userEvent.click(screen.getByRole("radio", {name: "A research"}));
    await userEvent.type(screen.getByLabelText(/^Name$/), "Zilnic");
    await userEvent.type(screen.getByLabelText("What to research"), "Pretul benzinei");
    await userEvent.selectOptions(screen.getByLabelText("Depth"), "quick");
    await userEvent.click(screen.getByLabelText("Light mode for small models"));
    await userEvent.click(screen.getByRole("button", {name: "Save"}));
    await waitFor(() => expect(fake.calls("schedule.save")).toHaveLength(1));
    expect(fake.calls("schedule.save")[0]).toMatchObject({kind: "research", options: {depth: "quick", small_model: true}, prompt: "Pretul benzinei"});
  });
});

describe("accounts", () => {
  it("asks for an address, not a key, for SearXNG and saves the provider in the settings", async () => {
    const fake = setup();
    fake.results["connectors.list"] = [];
    fake.results["research.settings"] = {duckduckgo: true};
    view(fake, <AccountsPage />);
    await userEvent.click(await screen.findByRole("button", {name: /Web search \(Brave\)/}));
    expect(screen.getByLabelText("API key")).toBeTruthy();
    await userEvent.selectOptions(screen.getByLabelText("Search service"), "searxng");
    expect(screen.queryByLabelText("API key")).toBeNull();
    await userEvent.type(screen.getByLabelText("Server address"), "http://192.168.1.5:8080");
    await userEvent.click(screen.getByRole("button", {name: "Save"}));
    await waitFor(() => expect(fake.calls("connectors.save")).toHaveLength(1));
    expect(fake.calls("connectors.save")[0]).toMatchObject({kind: "websearch", settings: {provider: "searxng", url: "http://192.168.1.5:8080"}, secrets: {}});
  });

  it("adds phone notifications with the topic, and turns DuckDuckGo off", async () => {
    const fake = setup();
    fake.results["connectors.list"] = [];
    fake.results["research.settings"] = {duckduckgo: true};
    view(fake, <AccountsPage />);
    const box = await screen.findByLabelText("Use DuckDuckGo");
    await waitFor(() => expect((box as HTMLInputElement).disabled).toBe(false));
    await userEvent.click(box);
    await waitFor(() => expect(fake.calls("research.settings")).toContainEqual({duckduckgo: false}));
    await userEvent.click(screen.getByRole("button", {name: /Phone notifications \(ntfy\)/}));
    await userEvent.type(screen.getByLabelText("Topic"), "vatra-example-1");
    await userEvent.click(screen.getByRole("button", {name: "Save"}));
    await waitFor(() => expect(fake.calls("connectors.save")).toHaveLength(1));
    expect(fake.calls("connectors.save")[0]).toMatchObject({kind: "ntfy", settings: {topic: "vatra-example-1"}});
  });
});

describe("status", () => {
  it("lists what works and what failed with the reason, and checks a part again", async () => {
    const fake = setup();
    fake.results["health.status"] = {
      down: 1,
      items: [
        {id: "connector.email-1", group: "accounts", label: "Personal", kind: "Email", state: "down", detail: "The mail server refused the user name or password.", at: 1_800_000_000, retry: {method: "connectors.check", params: {id: "email-1"}}},
        {id: "search.duckduckgo", group: "search", label: "DuckDuckGo", kind: "Web search", state: "ok", detail: "", at: 1_800_000_000, retry: null},
      ],
    };
    view(fake, <StatusPage />);
    expect(await screen.findByText("1 thing needs attention.")).toBeTruthy();
    expect(screen.getByText("The mail server refused the user name or password.")).toBeTruthy();
    expect(screen.getByText(/Web search · Working/)).toBeTruthy();
    fake.results["health.status"] = {down: 0, items: []};
    await userEvent.click(screen.getByRole("button", {name: "Check again"}));
    await waitFor(() => expect(fake.calls("connectors.check")).toEqual([{id: "email-1"}]));
    expect(await screen.findByText("Everything that was used is working.")).toBeTruthy();
  });
});

describe("command palette", () => {
  function open(fake: FakeService) {
    const closed = vi.fn();
    view(fake, <CommandPalette onClose={closed} />);
    return closed;
  }

  it("finds a page, a conversation and a project, and opens one with Enter", async () => {
    const fake = setup();
    useAppStore.setState({sessions: [{id: 7, title: "Plan de vacanta", agent: "alfa", project_id: null, last_turn: "none", running: false} as never], projects: [{id: 1, name: "Casa", path: null}]});
    const closed = open(fake);
    const input = screen.getByRole("combobox");
    expect(document.activeElement).toBe(input);
    await userEvent.type(input, "resea");
    const options = screen.getAllByRole("option");
    expect(options[0].textContent).toContain("Research");
    await userEvent.clear(input);
    await userEvent.type(input, "vacanta");
    expect(screen.getByRole("option").textContent).toContain("Plan de vacanta");
    await userEvent.clear(input);
    await userEvent.type(input, "zzzz");
    expect(screen.getByText("Nothing matches.")).toBeTruthy();
    await userEvent.clear(input);
    await userEvent.type(input, "tool store{Enter}");
    expect(closed).toHaveBeenCalled();
    expect(useAppStore.getState().settingsPage).toBe("mcp-market");
  });

  it("moves with the arrow keys and closes with Escape", async () => {
    const fake = setup();
    const closed = open(fake);
    const input = screen.getByRole("combobox");
    await userEvent.type(input, "{ArrowDown}");
    const options = screen.getAllByRole("option");
    expect(options[1].getAttribute("aria-selected")).toBe("true");
    await userEvent.type(input, "{Escape}");
    expect(closed).toHaveBeenCalled();
  });
});

describe("toasts", () => {
  it("shows a message that leaves by itself, and an error that can be dismissed", async () => {
    vi.useFakeTimers();
    render(<ToastHost />);
    act(() => { toast("Saved in Documents."); toast("Something failed", "error"); });
    expect(screen.getByRole("status").textContent).toContain("Saved in Documents.");
    expect(screen.getByRole("alert").textContent).toContain("Something failed");
    act(() => { vi.advanceTimersByTime(4000); });
    expect(screen.queryByText("Saved in Documents.")).toBeNull();
    expect(screen.getByText("Something failed")).toBeTruthy();
    vi.useRealTimers();
    await userEvent.click(screen.getByRole("button", {name: "Dismiss"}));
    expect(screen.queryByText("Something failed")).toBeNull();
  });
});
