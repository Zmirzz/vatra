import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, test } from "vitest";
import { ClientProvider } from "../rpc/context";
import { FakeService, flush } from "../rpc/fake";
import { initialState, useAppStore } from "../state/store";
import { Home } from "./Home";

let service: FakeService;

const renderIt = () =>
  render(
    <ClientProvider client={service.client}>
      <Home />
    </ClientProvider>,
  );

const box = () => screen.getByRole("textbox", { name: /describe what you need/i });

beforeEach(() => {
  useAppStore.setState({
    ...initialState(),
    agents: ["local", "claude"],
    projects: [{ id: 3, name: "Work", path: "E:/code" }],
  });
  service = new FakeService();
  service.open();
  service.results["sessions.create"] = { id: 8 };
  service.results["turn.start"] = {};
  service.results["projects.create"] = { id: 4, name: "Personal", path: null };
});

test("typing and sending starts the work, with no form in the way", async () => {
  renderIt();
  expect(screen.queryByRole("textbox", { name: /name/i })).toBeNull();
  await userEvent.type(box(), "Plan my week{Enter}");
  await flush();
  expect(service.calls("sessions.create")).toEqual([{ project_id: null, agent: "local" }]);
  expect(service.calls("turn.start")).toEqual([{ session_id: 8, prompt: "Plan my week", mode: "supervised" }]);
  expect(useAppStore.getState().activeSessionId).toBe(8);
});

test("starts directly as a chat without requiring a project", async () => {
  useAppStore.setState({ projects: [] });
  renderIt();
  await userEvent.type(box(), "Hello{Enter}");
  await flush();
  expect(service.calls("projects.create")).toEqual([]);
  expect(service.calls("sessions.create")).toEqual([{ project_id: null, agent: "local" }]);
});

test("project, agent and permissions are chosen in the box", async () => {
  useAppStore.setState({ projects: [{ id: 3, name: "Work", path: null }, { id: 5, name: "Home", path: null }] });
  renderIt();
  await userEvent.click(screen.getByRole("button", { name: /project: no project/i }));
  await userEvent.click(screen.getByRole("menuitemradio", { name: "Home" }));
  await userEvent.click(screen.getByRole("button", { name: /permissions: ask first/i }));
  await userEvent.click(screen.getByRole("menuitemradio", { name: "Full access" }));
  await userEvent.type(box(), "Go{Enter}");
  await flush();
  expect(service.calls("sessions.create")[0].project_id).toBe(5);
  expect(service.calls("turn.start")[0].mode).toBe("full_access");
  expect(useAppStore.getState().modes[8]).toBe("full_access");
});

test("a new project can be added from the box, with or without a folder", async () => {
  renderIt();
  await userEvent.click(screen.getByRole("button", { name: /project: no project/i }));
  await userEvent.click(screen.getByRole("menuitem", { name: /new project/i }));
  await userEvent.click(screen.getByRole("button", { name: "Create" }));
  expect(screen.getByRole("alert").textContent).toContain("name");
  await userEvent.type(screen.getByRole("textbox", { name: /^name$/i }), "Personal");
  await userEvent.click(screen.getByRole("button", { name: "Create" }));
  await flush();
  expect(service.calls("projects.create")).toEqual([{ name: "Personal", path: null }]);
  expect(screen.getByRole("button", { name: /project: personal/i })).toBeTruthy();
});

test("claude cannot start until the subscription notice is accepted", async () => {
  renderIt();
  await userEvent.click(screen.getByRole("button", { name: /model: local/i }));
  await userEvent.click(screen.getByRole("option", { name: "Claude Code" }));
  await userEvent.type(box(), "Fix it{Enter}");
  expect(screen.getByRole("dialog").textContent).toContain("your account");
  expect(service.calls("sessions.create")).toHaveLength(0);

  await userEvent.click(screen.getByRole("button", { name: /got it/i }));
  await flush();
  expect(service.calls("sessions.create")).toEqual([{ project_id: null, agent: "claude" }]);
  expect(service.calls("turn.start")[0].prompt).toBe("Fix it");
});

test("the notice is asked only once", async () => {
  localStorage.setItem("vatra.subscription-notice", "accepted");
  useAppStore.setState({ agents: ["claude"] });
  renderIt();
  await userEvent.type(box(), "Go{Enter}");
  await flush();
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(service.calls("sessions.create")).toHaveLength(1);
});

test("configured ACP subscription agent asks for its own notice", async () => {
  useAppStore.setState({agents:["codex"],subscriptionAgents:["codex"]});
  renderIt();
  await userEvent.type(box(),"Fix it{Enter}");
  expect(screen.getByRole("dialog").textContent).toContain("This agent uses your account");
  expect(service.calls("sessions.create")).toHaveLength(0);
});

test("a failed start keeps the text and shows why", async () => {
  service.failures["sessions.create"] = { code: 1000, message: "the disk is full" };
  renderIt();
  await userEvent.type(box(), "Keep me{Enter}");
  await flush();
  expect((await screen.findByRole("alert")).textContent).toContain("disk is full");
  expect((box() as HTMLTextAreaElement).value).toBe("Keep me");
});

test("without agents it explains what to do", () => {
  useAppStore.setState({ agents: [], agentsLoaded: true });
  renderIt();
  expect(screen.getByRole("button", { name: "Set up Vatra" })).toBeTruthy();
});

test("retrying after the turn failed to start reuses the session", async () => {
  service.failures["turn.start"] = { code: 1000, message: "try again" };
  renderIt();
  await userEvent.type(box(), "Once{Enter}");
  await flush();
  expect(await screen.findByRole("alert")).toBeTruthy();
  await userEvent.type(box(), "{Enter}");
  await flush();
  expect(service.calls("sessions.create")).toHaveLength(1);
  expect(service.calls("turn.start")).toHaveLength(2);
});

test("a refresh failing after the turn started isn't reported as a failed start", async () => {
  service.failures["sessions.list"] = { code: 0, message: "connection lost" };
  renderIt();
  await userEvent.type(box(), "Once{Enter}");
  await flush();
  await flush();
  expect(screen.queryByRole("alert")).toBeNull();
  expect((box() as HTMLTextAreaElement).value).toBe("");
  expect(useAppStore.getState().activeSessionId).toBe(8);
});

test("before the first answer from the service it doesn't claim there are no agents", () => {
  useAppStore.setState({ agents: [], agentsLoaded: false });
  renderIt();
  expect(screen.queryByText(/vatra\.toml/)).toBeNull();
  expect(screen.getByRole("status").textContent).toContain("Connecting");
});
