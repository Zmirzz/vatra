import {beforeEach, describe, expect, it} from "vitest";
import {render, screen, waitFor, within} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {FakeService} from "../../rpc/fake";
import {ClientProvider} from "../../rpc/context";
import {aliasOf, fillValues, installPlan, prettyName, prettyPublisher, requiredValues, type RegistryEntry} from "../../hub/mcpMarket";
import {initialState, useAppStore} from "../../state/store";
import {McpMarketPage} from "./McpMarketPage";
import {ProjectRemover} from "../ProjectRemover";

const online: RegistryEntry = {
  id: "io.github.acme/weather-server", name: "io.github.acme/weather-server", description: "Forecasts for any city.", version: "1.2.0", publishers: ["io.github.acme"],
  repository: null, platformAvailable: true,
  details: {remotes: [{type: "streamable-http", url: "https://mcp.acme.dev/mcp"}]},
};
const local: RegistryEntry = {
  id: "com.stripe/payments", name: "Stripe", description: "Look up payments.", version: "0.4.0", publishers: ["com.stripe"], repository: null, platformAvailable: true,
  details: {packages: [{registryType: "npm", identifier: "@stripe/mcp", version: "0.4.0", transport: {type: "stdio"}, environmentVariables: [{name: "STRIPE_API_KEY", isRequired: true}]}]},
};
const manual: RegistryEntry = {
  id: "org/odd", name: "Odd one", description: "Needs docker.", version: "1", publishers: ["org"], repository: null, platformAvailable: true,
  details: {packages: [{registryType: "oci", identifier: "x", version: "1", transport: {type: "stdio"}}]},
};

function setup() {
  const fake = new FakeService();
  fake.open();
  fake.results["registry.mcp"] = {entries: [online, local, manual], nextCursor: null, source: "registry"};
  fake.results["catalog.list"] = [];
  return fake;
}
const view = (fake: FakeService, node: React.ReactNode) => render(<ClientProvider client={fake.client}>{node}</ClientProvider>);

beforeEach(() => useAppStore.setState({...initialState(), projects: [{id: 1, name: "Casa", path: null}]}));

describe("registry helpers", () => {
  it("name things in plain words and pick the simplest way to install", () => {
    expect(prettyName(online)).toBe("Weather server");
    expect(prettyName(local)).toBe("Stripe");
    expect(prettyPublisher(online)).toBe("acme");
    expect(prettyPublisher(local)).toBe("stripe.com");
    expect(aliasOf(online)).toBe("weather-server");
    expect(installPlan(online)).toMatchObject({method: "remote", target: "https://mcp.acme.dev/mcp"});
    expect(installPlan(local)).toMatchObject({method: "package", target: "@stripe/mcp@0.4.0"});
    expect(installPlan(manual)).toBeNull();
  });

  it("finds the values to ask for and puts them in place as plain strings", () => {
    const document = '{"mcpServers":{"x":{"env":{"A_KEY":"${A_KEY}","B":"fixed"},"headers":{"Authorization":"${TOKEN}"}}}}';
    expect(requiredValues(document)).toEqual(["A_KEY", "TOKEN"]);
    const filled = fillValues(document, {A_KEY: 'he said "hi"', TOKEN: "t"});
    expect(JSON.parse(filled).mcpServers.x.env.A_KEY).toBe('he said "hi"');
    expect(JSON.parse(filled).mcpServers.x.headers.Authorization).toBe("t");
  });
});

describe("MCP marketplace", () => {
  it("shows cards in plain words and hides what cannot be installed in one step", async () => {
    const fake = setup();
    view(fake, <McpMarketPage />);
    expect(await screen.findByText("Weather server")).toBeTruthy();
    expect(screen.getByText("Stripe")).toBeTruthy();
    expect(screen.queryByText("Odd one")).toBeNull();
    expect(screen.getAllByRole("button", {name: "Install"})).toHaveLength(2);
    expect(screen.getByText("Needs a key")).toBeTruthy();
    await userEvent.click(screen.getByLabelText(/Only tools Vatra can install/));
    expect(await screen.findByText("Odd one")).toBeTruthy();
    expect(screen.getByText("Set up by hand")).toBeTruthy();
  });

  it("searches by itself while typing", async () => {
    const fake = setup();
    view(fake, <McpMarketPage />);
    await screen.findByText("Weather server");
    await userEvent.type(screen.getByLabelText("Search tools"), "stripe");
    await waitFor(() => expect(fake.calls("registry.mcp").some((c) => c.query === "stripe")).toBe(true));
  });

  it("installs an online tool in one step: review, install, turn on, check", async () => {
    const fake = setup();
    fake.results["mcp.registry_config"] = {document: '{"mcpServers":{"weather-server":{"type":"http","url":"https://mcp.acme.dev/mcp"}}}'};
    fake.results["mcp.preview"] = {servers: [{plan: {config: {name: "weather-server", url: "https://mcp.acme.dev/mcp"}, reviewHash: "h1"}, existing: null}]};
    fake.results["mcp.install"] = {id: 7, hash: "c1"};
    fake.results["mcp.test"] = {connected: true, tools: [{name: "a"}, {name: "b"}]};
    view(fake, <McpMarketPage />);
    await screen.findByText("Weather server");
    await userEvent.click(screen.getAllByRole("button", {name: "Install"})[0]);
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByText(/connects your agents to mcp.acme.dev/)).toBeTruthy();
    fake.results["catalog.list"] = [{id: 7, kind: "mcp_server", name: "weather-server", runs_code: [], approved_hash: null, content_hash: "c1", enabled: false}];
    await userEvent.click(within(dialog).getByRole("button", {name: "Install and turn on"}));
    expect(await within(dialog).findByText("Ready. It offers 2 actions.")).toBeTruthy();
    expect(fake.calls("mcp.install")[0]).toMatchObject({reviewHash: "h1", source: {name: "io.github.acme/weather-server", method: "remote"}});
    expect(fake.calls("catalog.set_enabled")).toEqual([{id: 7, enabled: true}]);
    expect(fake.calls("catalog.approve")).toHaveLength(0);
  });

  it("asks for a required key, stores it only through the install, and approves a program it runs", async () => {
    const fake = setup();
    fake.results["mcp.registry_config"] = {document: '{"mcpServers":{"payments":{"command":"npx","args":["-y","@stripe/mcp@0.4.0"],"env":{"STRIPE_API_KEY":"${STRIPE_API_KEY}"}}}}'};
    fake.results["mcp.preview"] = {servers: [{plan: {config: {name: "payments", command: "npx"}, reviewHash: "h2"}, existing: null}]};
    fake.results["mcp.install"] = {id: 9, hash: "c9"};
    fake.results["mcp.test"] = {connected: false, error: "npx was not found"};
    view(fake, <McpMarketPage />);
    await screen.findByText("Stripe");
    await userEvent.click(screen.getAllByRole("button", {name: "Install"})[1]);
    const dialog = await screen.findByRole("dialog");
    const install = within(dialog).getByRole("button", {name: "Install and turn on"}) as HTMLButtonElement;
    expect(install.disabled).toBe(true);
    await userEvent.type(await within(dialog).findByLabelText("Stripe api key"), "sk_live_secret");
    fake.results["catalog.list"] = [{id: 9, kind: "mcp_server", name: "payments", runs_code: ["command"], approved_hash: null, content_hash: "c9", enabled: false}];
    await userEvent.click(install);
    expect(await within(dialog).findByText(/Installed and turned on, but it didn't answer: npx was not found/)).toBeTruthy();
    const sent = fake.calls("mcp.preview")[0] as {document: string};
    expect(JSON.parse(sent.document).mcpServers.payments.env.STRIPE_API_KEY).toBe("sk_live_secret");
    expect(fake.calls("catalog.approve")).toEqual([{id: 9, expected_hash: "c9"}]);
  });

  it("says what went wrong when the install fails and lets the person try again", async () => {
    const fake = setup();
    fake.results["mcp.registry_config"] = {document: '{"mcpServers":{"weather-server":{"type":"http","url":"https://mcp.acme.dev/mcp"}}}'};
    fake.failures["mcp.preview"] = {code: 1001, message: "Server already exists."};
    view(fake, <McpMarketPage />);
    await screen.findByText("Weather server");
    await userEvent.click(screen.getAllByRole("button", {name: "Install"})[0]);
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(await within(dialog).findByRole("button", {name: "Install and turn on"}));
    expect(await within(dialog).findByText(/The install didn't finish: Server already exists/)).toBeTruthy();
    await userEvent.click(within(dialog).getByRole("button", {name: "Try again"}));
    expect(within(dialog).getByRole("button", {name: "Install and turn on"})).toBeTruthy();
  });

  it("marks what is already installed", async () => {
    const fake = setup();
    fake.results["catalog.list"] = [{id: 1, kind: "mcp_server", name: "weather-server", runs_code: [], approved_hash: null, content_hash: "x", enabled: true}];
    view(fake, <McpMarketPage />);
    expect(await screen.findByText("Installed")).toBeTruthy();
    expect(screen.getByRole("button", {name: "Manage"})).toBeTruthy();
  });
});

describe("deleting a project", () => {
  it("asks first, names what goes, and removes the project from the app", async () => {
    const fake = new FakeService();
    fake.open();
    fake.results["sessions.list"] = [];
    view(fake, <ProjectRemover project={{id: 1, name: "Casa"}} conversations={3}><button type="button">Casa</button></ProjectRemover>);
    await userEvent.click(screen.getByRole("button", {name: "Delete project: Casa"}));
    const dialog = screen.getByRole("alertdialog");
    expect(within(dialog).getByText(/3 conversation\(s\), documents, notes and tasks are deleted too/)).toBeTruthy();
    expect(within(dialog).getByText(/Files and folders on your computer are not touched/)).toBeTruthy();
    expect(fake.calls("projects.delete")).toHaveLength(0);
    await userEvent.click(within(dialog).getByRole("button", {name: "Delete project"}));
    await waitFor(() => expect(fake.calls("projects.delete")).toEqual([{id: 1}]));
    await waitFor(() => expect(useAppStore.getState().projects).toEqual([]));
  });

  it("can be cancelled without deleting anything", async () => {
    const fake = new FakeService();
    fake.open();
    view(fake, <ProjectRemover project={{id: 1, name: "Casa"}} conversations={0}><button type="button">Casa</button></ProjectRemover>);
    await userEvent.click(screen.getByRole("button", {name: "Delete project: Casa"}));
    await userEvent.click(screen.getByRole("button", {name: "Cancel"}));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(fake.calls("projects.delete")).toHaveLength(0);
  });
});
