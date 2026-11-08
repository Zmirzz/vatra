import {beforeEach, describe, expect, it, vi} from "vitest";
import {render, screen, waitFor} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {FakeService} from "../../rpc/fake";
import {ClientProvider} from "../../rpc/context";
import {useHubFocus} from "../../hub/focus";
import {initialState, useAppStore} from "../../state/store";
import {useToasts, ToastHost} from "../../hub/toast";
import {CommandPalette} from "../CommandPalette";
import {AccountsPage} from "./AccountsPage";

function setup() {
  const fake = new FakeService();
  fake.open();
  return fake;
}
const view = (fake: FakeService, node: React.ReactNode) => render(<ClientProvider client={fake.client}>{node}<ToastHost /></ClientProvider>);

beforeEach(() => {
  useToasts.setState({toasts: []});
  useHubFocus.setState({researchId: null});
  useAppStore.setState({...initialState(), agents: ["alfa"], projects: [{id: 1, name: "Casa", path: null}]});
});

describe("search by meaning", () => {
  it("adds what the service finds by meaning to the palette, and opens a note from it", async () => {
    const fake = setup();
    fake.results["semantic.status"] = {configured: true, model: "nomic", sources: 3, pending: 0};
    fake.results["semantic.search"] = [
      {kind: "item", ref_id: 42, project_id: 1, title: "Retete", snippet: "Supa de pui cu taietei", score: 0.8},
      {kind: "research", ref_id: 9, project_id: null, title: "Cat costa chiria?", snippet: "Chiria medie este 500 euro", score: 0.6},
    ];
    const closed = vi.fn();
    view(fake, <CommandPalette onClose={closed} />);
    await waitFor(() => expect(fake.calls("semantic.status")).toHaveLength(1));
    await userEvent.type(screen.getByRole("combobox"), "ce gatesc");
    await waitFor(() => expect(fake.calls("semantic.search").at(-1)).toMatchObject({query: "ce gatesc"}));
    const found = await screen.findByRole("option", {name: /Retete/});
    expect(found.textContent).toContain("Supa de pui cu taietei");
    expect(screen.getByText("Found by meaning")).toBeTruthy();
    await userEvent.click(screen.getByRole("option", {name: /Cat costa chiria/}));
    expect(closed).toHaveBeenCalled();
    await waitFor(() => expect(useHubFocus.getState().researchId).toBe(9));
  });

  it("does not look by meaning without an embedding account", async () => {
    const fake = setup();
    fake.results["semantic.status"] = {configured: false};
    view(fake, <CommandPalette onClose={() => undefined} />);
    await waitFor(() => expect(fake.calls("semantic.status")).toHaveLength(1));
    await userEvent.type(screen.getByRole("combobox"), "ce gatesc");
    await new Promise((resolve) => setTimeout(resolve, 450));
    expect(fake.calls("semantic.search")).toHaveLength(0);
  });
});

describe("accounts for memory and browser", () => {
  function accounts(fake: FakeService) {
    fake.results["connectors.list"] = [];
    fake.results["research.settings"] = {duckduckgo: true, browser: false, browser_found: true, browser_path: "C:/edge.exe"};
    fake.results["semantic.status"] = {configured: false};
  }

  it("saves an embedding account with the suggested local server and model", async () => {
    const fake = setup();
    accounts(fake);
    view(fake, <AccountsPage />);
    await userEvent.click(await screen.findByRole("button", {name: /Search by meaning/}));
    expect((screen.getByLabelText("Server address") as HTMLInputElement).value).toBe("http://localhost:11434/v1");
    expect((screen.getByLabelText("Embedding model") as HTMLInputElement).value).toBe("nomic-embed-text");
    await userEvent.click(screen.getByRole("button", {name: "Save"}));
    await waitFor(() => expect(fake.calls("connectors.save")).toHaveLength(1));
    expect(fake.calls("connectors.save")[0]).toMatchObject({kind: "embeddings", settings: {base_url: "http://localhost:11434/v1", model: "nomic-embed-text"}, secrets: {}});
  });

  it("shows how much is indexed and starts over on request", async () => {
    const fake = setup();
    accounts(fake);
    fake.results["semantic.status"] = {configured: true, model: "nomic-embed-text", sources: 12, chunks: 40, pending: 2};
    view(fake, <AccountsPage />);
    expect(await screen.findByText(/12 items indexed · 2 waiting · Model: nomic-embed-text/)).toBeTruthy();
    await userEvent.click(screen.getByRole("button", {name: "Index everything again"}));
    await waitFor(() => expect(fake.calls("semantic.reindex")).toHaveLength(1));
    expect(await screen.findByText("Indexing again in the background.")).toBeTruthy();
  });

  it("keeps drawing pages in a browser off until it is turned on, and says when there is no browser", async () => {
    const fake = setup();
    accounts(fake);
    view(fake, <AccountsPage />);
    const box = await screen.findByLabelText("Open such pages in a browser");
    await waitFor(() => expect((box as HTMLInputElement).disabled).toBe(false));
    expect((box as HTMLInputElement).checked).toBe(false);
    await userEvent.click(box);
    await waitFor(() => expect(fake.calls("research.settings")).toContainEqual({browser: true}));
  });

  it("disables the browser option when none was found", async () => {
    const fake = setup();
    accounts(fake);
    fake.results["research.settings"] = {duckduckgo: true, browser: false, browser_found: false, browser_path: null};
    view(fake, <AccountsPage />);
    expect(await screen.findByText("No Edge or Chrome was found on this computer.")).toBeTruthy();
    expect((screen.getByLabelText("Open such pages in a browser") as HTMLInputElement).disabled).toBe(true);
  });
});
