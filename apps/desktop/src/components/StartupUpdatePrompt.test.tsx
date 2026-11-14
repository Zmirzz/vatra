import { StrictMode } from "react";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, test, vi } from "vitest";
import { setLanguage } from "../copy";
import { initialUpdateState, useUpdates } from "../updates";
import { StartupUpdatePrompt } from "./StartupUpdatePrompt";
import { UpdateStatus } from "./UpdateStatus";

const mocks = vi.hoisted(() => ({ desktop: true, invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => mocks.desktop, invoke: mocks.invoke }));
beforeEach(() => {
  setLanguage("en");
  mocks.desktop = true;
  mocks.invoke.mockReset();
  useUpdates.setState(initialUpdateState());
});

test("startup checks once in StrictMode and never installs before the user chooses", async () => {
  mocks.invoke.mockResolvedValue({ configured: true, available: "1.1.2", error: null });
  render(<StrictMode><StartupUpdatePrompt /></StrictMode>);
  const dialog = await screen.findByRole("alertdialog", { name: "Update available" });
  expect(within(dialog).getByText("Vatra 1.1.2 is ready to install.")).toBeTruthy();
  expect(mocks.invoke.mock.calls).toEqual([["update_status"]]);
  await userEvent.click(within(dialog).getByRole("button", { name: "Later" }));
  expect(screen.queryByRole("alertdialog")).toBeNull();
  expect(mocks.invoke).not.toHaveBeenCalledWith("install_update");
});

test.each([
  { configured: true, available: null, error: null },
  { configured: false, available: null, error: null },
  { configured: true, available: null, error: "Offline" },
])("startup stays quiet when no update can be offered: %j", async status => {
  mocks.invoke.mockResolvedValue(status);
  render(<StartupUpdatePrompt />);
  await waitFor(() => expect(useUpdates.getState().checking).toBe(false));
  expect(screen.queryByRole("alertdialog")).toBeNull();
  expect(mocks.invoke).toHaveBeenCalledTimes(1);
});

test("a rejected startup check is silent and browser previews make no desktop calls", async () => {
  mocks.invoke.mockRejectedValue(new Error("Offline"));
  const view = render(<StartupUpdatePrompt />);
  await waitFor(() => expect(useUpdates.getState().status?.error).toContain("Offline"));
  expect(screen.queryByRole("alertdialog")).toBeNull();
  view.unmount();
  mocks.desktop = false;
  mocks.invoke.mockClear();
  useUpdates.setState(initialUpdateState());
  render(<StartupUpdatePrompt />);
  expect(mocks.invoke).not.toHaveBeenCalled();
});

test("defers the popup behind another app dialog and Settings shares the startup result", async () => {
  mocks.invoke.mockResolvedValue({ configured: true, available: "1.1.2", error: null });
  const view = render(<StartupUpdatePrompt suspended />);
  await waitFor(() => expect(useUpdates.getState().status?.available).toBe("1.1.2"));
  expect(screen.queryByRole("alertdialog")).toBeNull();
  view.rerender(<><StartupUpdatePrompt /><UpdateStatus /></>);
  await screen.findByRole("alertdialog");
  expect(mocks.invoke).toHaveBeenCalledTimes(1);
});

test("install failures stay in the popup for retry and installation cannot be started twice", async () => {
  let fail!: (error: Error) => void;
  mocks.invoke.mockResolvedValueOnce({ configured: true, available: "1.1.2", error: null })
    .mockImplementationOnce(() => new Promise((_, reject) => { fail = reject; }))
    .mockResolvedValueOnce(undefined);
  render(<StartupUpdatePrompt />);
  const dialog = await screen.findByRole("alertdialog");
  await userEvent.click(within(dialog).getByRole("button", { name: "Install update" }));
  expect((within(dialog).getByRole("button", { name: "Installing update…" }) as HTMLButtonElement).disabled).toBe(true);
  expect((within(dialog).getByRole("button", { name: "Later" }) as HTMLButtonElement).disabled).toBe(true);
  await userEvent.keyboard("{Escape}");
  expect(screen.queryByRole("alertdialog")).toBeTruthy();
  await act(async () => fail(new Error("Download failed")));
  expect(await screen.findByRole("alert")).toHaveProperty("textContent", "Error: Download failed");
  await userEvent.click(within(dialog).getByRole("button", { name: "Install update" }));
  await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  expect(mocks.invoke.mock.calls.filter(([command]) => command === "install_update")).toHaveLength(2);
});

test("keyboard focus stays in the popup and Escape returns focus without installing", async () => {
  mocks.invoke.mockResolvedValue({ configured: true, available: "1.1.2", error: null });
  render(<><button>Existing control</button><StartupUpdatePrompt suspended /></>);
  const previous = screen.getByRole("button", { name: "Existing control" });
  previous.focus();
  useUpdates.setState({ status: { configured: true, available: "1.1.2", error: null } });
  const popup = render(<StartupUpdatePrompt />);
  const dialog = await screen.findByRole("alertdialog");
  const later = within(dialog).getByRole("button", { name: "Later" });
  expect(document.activeElement).toBe(later);
  await userEvent.tab({ shift: true });
  expect(document.activeElement).toBe(within(dialog).getByRole("button", { name: "Install update" }));
  await userEvent.tab();
  expect(document.activeElement).toBe(later);
  await userEvent.keyboard("{Escape}");
  expect(document.activeElement).toBe(previous);
  expect(mocks.invoke).not.toHaveBeenCalledWith("install_update");
  popup.unmount();
});
