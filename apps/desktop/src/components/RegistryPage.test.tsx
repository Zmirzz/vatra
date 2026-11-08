import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, test } from "vitest";
import { ClientProvider } from "../rpc/context";
import { FakeService } from "../rpc/fake";
import { defaults, updatePreferences } from "../settings/preferences";
import { RegistryPage } from "./RegistryPage";

beforeEach(() => updatePreferences(defaults()));

test("ACP discovery shows actual distributions without claiming tested compatibility", async () => {
  const service = new FakeService(); service.open();
  service.results["registry.acp"] = { entries: [{ id: "example", name: "Example ACP", description: "From registry", version: "1", publishers: ["Publisher"], repository: "javascript:alert(1)", platformAvailable: true, details: { distribution: { npx: { package: "example@1" } } } }], nextCursor: null, source: "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json" };
  render(<ClientProvider client={service.client}><RegistryPage ecosystem="acp" /></ClientProvider>);
  expect(await screen.findByText("Example ACP")).toBeTruthy();
  expect(screen.getByText("Publisher")).toBeTruthy();
  expect(screen.queryByRole("link", { name: "Repository" })).toBeNull();
  await userEvent.click(screen.getByText("Details"));
  expect(screen.getByText(/example@1/)).toBeTruthy();
  expect(service.calls("registry.acp")).toHaveLength(1);
  expect(service.calls("agents.save")).toHaveLength(0);
});

test("MCP search resets pagination and load more retains results", async () => {
  const service = new FakeService(); service.open();
  const entry = (id: string) => ({ id, name: id, description: "Server", version: "1", publishers: [], platformAvailable: true, details: {} });
  service.results["registry.mcp"] = { entries: [entry("first")], nextCursor: "cursor", source: "https://registry.modelcontextprotocol.io" };
  render(<ClientProvider client={service.client}><RegistryPage ecosystem="mcp" /></ClientProvider>);
  expect(await screen.findByText("first")).toBeTruthy();
  service.results["registry.mcp"] = { entries: [entry("second")], nextCursor: null, source: "https://registry.modelcontextprotocol.io" };
  await userEvent.click(screen.getByRole("button", { name: "Load more" }));
  expect(await screen.findByText("second")).toBeTruthy();
  expect(screen.getByText("first")).toBeTruthy();
  await userEvent.type(screen.getByRole("searchbox", { name: "Search registry" }), "calendar");
  await userEvent.click(screen.getByRole("button", { name: "Search" }));
  expect(service.calls("registry.mcp").at(-1)).toMatchObject({ query: "calendar" });
  expect(service.calls("registry.mcp").at(-1)).not.toHaveProperty("cursor");
  expect(screen.queryByText("first")).toBeNull();
});
