import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, test } from "vitest";
import { ClientProvider } from "../rpc/context";
import { FakeService } from "../rpc/fake";
import { defaults, updatePreferences } from "../settings/preferences";
import { AgentForm } from "./AgentForm";

beforeEach(() => updatePreferences(defaults()));

test("a generic ACP command retains every argument and timeout when edited", async () => {
  const service = new FakeService(); service.open();
  service.results["agents.detect"] = { claude: null, codex: null, node: null, acpScripts: [] };
  service.results["agents.save"] = { restartNeeded: true, runningTurns: 0 };
  render(<ClientProvider client={service.client}><AgentForm kinds={["acp"]} initial={{ kind: "acp", name: "custom", node: "C:/Tools/agent.exe", args: ["--acp", "work profile"], startupTimeoutSec: 45, turnTimeoutSec: 1800, subscription: false }} onSaved={() => undefined} /></ClientProvider>);
  await userEvent.click(screen.getByRole("button", { name: "Save agent" }));
  expect(service.calls("agents.save")[0]).toMatchObject({ node: "C:/Tools/agent.exe", args: ["--acp", "work profile"], startupTimeoutSec: 45, turnTimeoutSec: 1800 });
  expect(service.calls("agents.save")[0]).not.toHaveProperty("script");
});

test("manual ACP arguments must be a JSON string array", async () => {
  const service = new FakeService(); service.open();
  service.results["agents.detect"] = { claude: null, codex: null, node: null, acpScripts: [] };
  render(<ClientProvider client={service.client}><AgentForm kinds={["acp"]} initial={{ kind: "acp", name: "custom", node: "agent.exe", args: [], subscription: false }} onSaved={() => undefined} /></ClientProvider>);
  const argumentsField = screen.getByRole("textbox", { name: "Arguments (JSON array)" });
  await userEvent.clear(argumentsField); await userEvent.type(argumentsField, "bad JSON");
  expect(screen.getByRole("button", { name: "Save agent" }).hasAttribute("disabled")).toBe(true);
  expect(service.calls("agents.save")).toHaveLength(0);
});
