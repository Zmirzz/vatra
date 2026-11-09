import { render, screen } from "@testing-library/react";
import { beforeEach, expect, test } from "vitest";
import { ClientProvider } from "../rpc/context";
import { FakeService } from "../rpc/fake";
import { setLanguage } from "../copy";
import { initialState, useAppStore } from "../state/store";
import { LimitsPage } from "./LimitsPage";

let service: FakeService;
beforeEach(() => { setLanguage("en"); useAppStore.setState(initialState()); service = new FakeService(); service.open(); });

test("shows what is left and says Claude updates when a prompt is sent", async () => {
  const claude = { id: "claude", name: "Claude", refreshable: false, updated: 1_800_000_000, windows: [{ key: "five_hour", left: 0.28, resets_at: 4_000_000_000 }, { key: "seven_day", left: 0.91, resets_at: 4_000_000_000 }] };
  service.results["limits.get"] = { providers: [claude] };
  render(<ClientProvider client={service.client}><LimitsPage /></ClientProvider>);
  expect(await screen.findByText("Session (5 hours)")).toBeTruthy();
  expect(screen.getByText("28%")).toBeTruthy();
  expect(screen.getByText("91%")).toBeTruthy();
  expect(screen.getByText(/each time you send a prompt to Claude/)).toBeTruthy();
  expect(screen.getByRole("progressbar", { name: "Weekly" }).getAttribute("aria-valuenow")).toBe("91");
});

test("Claude without a reading yet says to send a prompt", async () => {
  service.results["limits.get"] = { providers: [{ id: "claude", name: "Claude", refreshable: false, updated: null, windows: [] }] };
  render(<ClientProvider client={service.client}><LimitsPage /></ClientProvider>);
  expect(await screen.findByText(/Send a prompt to Claude/)).toBeTruthy();
});
