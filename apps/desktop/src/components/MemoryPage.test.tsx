import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, test } from "vitest";
import { ClientProvider } from "../rpc/context";
import { FakeService } from "../rpc/fake";
import { setLanguage } from "../copy";
import { initialState, useAppStore } from "../state/store";
import { MemoryPage } from "./MemoryPage";

let service: FakeService;
const item = (id: number, text: string, category: string, source = "you", pinned = false) => ({ id, text, category, source, pinned, created_at: 1, updated_at: 1 });
const memory = (items: ReturnType<typeof item>[], enabled = true) => ({ enabled, items, limit: 300, max_chars: 400 });
beforeEach(() => { setLanguage("en"); useAppStore.setState(initialState()); service = new FakeService(); service.open(); });

test("lists facts by kind and says who saved each one", async () => {
  service.results["memory.list"] = memory([item(1, "Is called Razvan", "about"), item(2, "Likes short answers", "preferences", "claude")]);
  render(<ClientProvider client={service.client}><MemoryPage /></ClientProvider>);
  expect(await screen.findByText("Is called Razvan")).toBeTruthy();
  expect(screen.getByRole("region", { name: "Things you like" })).toBeTruthy();
  expect(screen.getByText("Added by you")).toBeTruthy();
  expect(screen.getByText("Saved by claude")).toBeTruthy();
});

test("adds a fact with its kind", async () => {
  service.results["memory.list"] = memory([]);
  service.results["memory.add"] = memory([item(3, "Hates long intros", "dislikes")]);
  render(<ClientProvider client={service.client}><MemoryPage /></ClientProvider>);
  expect(await screen.findByText(/Nothing yet/)).toBeTruthy();
  fireEvent.change(screen.getByLabelText("Something worth knowing about you…"), { target: { value: "Hates long intros" } });
  fireEvent.change(screen.getByLabelText("Kind"), { target: { value: "dislikes" } });
  fireEvent.click(screen.getByRole("button", { name: "Add" }));
  await waitFor(() => expect(service.calls("memory.add")[0]).toEqual({ text: "Hates long intros", category: "dislikes" }));
  expect(await screen.findByText("Hates long intros")).toBeTruthy();
});

test("turning memory off and removing a fact reach the service", async () => {
  service.results["memory.list"] = memory([item(1, "Likes tea", "preferences")]);
  service.results["memory.set_enabled"] = memory([item(1, "Likes tea", "preferences")], false);
  service.results["memory.delete"] = memory([]);
  render(<ClientProvider client={service.client}><MemoryPage /></ClientProvider>);
  fireEvent.click(await screen.findByRole("switch"));
  await waitFor(() => expect(service.calls("memory.set_enabled")[0]).toEqual({ enabled: false }));
  fireEvent.click(screen.getByRole("button", { name: "Remove: Likes tea" }));
  await waitFor(() => expect(service.calls("memory.delete")[0]).toEqual({ id: 1 }));
});

test("clearing everything asks first", async () => {
  service.results["memory.list"] = memory([item(1, "Likes tea", "preferences")]);
  service.results["memory.clear"] = memory([]);
  render(<ClientProvider client={service.client}><MemoryPage /></ClientProvider>);
  fireEvent.click(await screen.findByRole("button", { name: "Remove everything" }));
  expect(service.calls("memory.clear").length).toBe(0);
  fireEvent.click(screen.getByRole("button", { name: "Yes, remove all" }));
  await waitFor(() => expect(service.calls("memory.clear").length).toBe(1));
});
