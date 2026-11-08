import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, test, vi } from "vitest";
import { setLanguage } from "../copy";
import { ClientProvider } from "../rpc/context";
import { FakeService } from "../rpc/fake";
import { initialState, useAppStore } from "../state/store";
import { ModelsPage } from "./ModelsPage";
import { UpdateStatus } from "./UpdateStatus";

const mocks = vi.hoisted(() => ({ desktop: true, invoke: vi.fn(), open: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => mocks.desktop, invoke: mocks.invoke }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: mocks.open }));
let service: FakeService;
beforeEach(() => {
  setLanguage("en"); mocks.desktop = true; mocks.invoke.mockReset(); mocks.open.mockReset();
  useAppStore.setState(initialState()); service = new FakeService(); service.open();
  Object.assign(service.results, { "models.installed": [], "models.labels": [], "hardware.get": { cpu: {}, ram: { total: 0, available: 0 }, gpus: [] }, "llama.status": { installed: true, install: { build: 1, caps: {} } } });
});
test("the desktop file picker preserves spaces and diacritics; browser keeps the text field", async () => {
  mocks.open.mockResolvedValue("C:\\Users\\Răzvan Test\\model.gguf");
  const view = render(<ClientProvider client={service.client}><ModelsPage /></ClientProvider>);
  await userEvent.click(await screen.findByRole("button", { name: "Choose a GGUF file" }));
  expect(mocks.open).toHaveBeenCalledWith({ multiple: false, filters: [{ name: "GGUF", extensions: ["gguf"] }] });
  expect((screen.getByRole("textbox", { name: "Add a GGUF file" }) as HTMLInputElement).value).toBe("C:\\Users\\Răzvan Test\\model.gguf");
  await userEvent.click(screen.getByRole("button", { name: "Add" }));
  expect(service.calls("models.add_path")).toEqual([{ path: "C:\\Users\\Răzvan Test\\model.gguf" }]);
  view.unmount(); mocks.desktop = false;
  render(<ClientProvider client={service.client}><ModelsPage /></ClientProvider>);
  expect(screen.queryByRole("button", { name: "Choose a GGUF file" })).toBeNull();
  expect(screen.getByRole("textbox", { name: "Add a GGUF file" })).toBeTruthy();
});
test("updates are clearly disabled without an endpoint", async () => {
  mocks.invoke.mockResolvedValue({ configured: false, available: null, error: null });
  render(<UpdateStatus />);
  expect(await screen.findByText("Updates aren't set up yet.")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Install update" })).toBeNull();
});
test("an available update installs only after confirmation", async () => {
  mocks.invoke.mockResolvedValue({ configured: true, available: "0.2.0", error: null });
  render(<UpdateStatus />);
  await userEvent.click(await screen.findByRole("button", { name: "Install update" }));
  expect(mocks.invoke).not.toHaveBeenCalledWith("install_update");
  const dialog = screen.getByRole("alertdialog");
  await userEvent.click(within(dialog).getByRole("button", { name: "Continue" }));
  expect(mocks.invoke).toHaveBeenCalledWith("install_update");
});
