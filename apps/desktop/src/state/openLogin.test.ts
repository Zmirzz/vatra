import { isTauri } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { openLoginAddress } from "./openLogin";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(() => Promise.resolve()) }));

const AUTHORIZATION = "https://auth.example.test/authorize?client_id=abc&state=xyz";

beforeEach(() => {
  vi.mocked(openUrl).mockClear();
});

afterEach(() => {
  vi.restoreAllMocks();
});

test("in the app the login address goes to the system browser through the opener", () => {
  vi.mocked(isTauri).mockReturnValue(true);
  const open = vi.spyOn(window, "open");
  openLoginAddress(AUTHORIZATION);
  expect(openUrl).toHaveBeenCalledWith(AUTHORIZATION);
  expect(open).not.toHaveBeenCalled();
});

test("in the browser the login address opens in a new window without a handle back", () => {
  vi.mocked(isTauri).mockReturnValue(false);
  const open = vi.spyOn(window, "open").mockReturnValue(null);
  openLoginAddress(AUTHORIZATION);
  expect(open).toHaveBeenCalledWith(AUTHORIZATION, "_blank", "noopener,noreferrer");
  expect(openUrl).not.toHaveBeenCalled();
});

test.each(["javascript:alert(1)", "file:///C:/secrets.txt", "vatra://login", "mailto:someone@example.test"])(
  "%s is never opened",
  (url) => {
    vi.mocked(isTauri).mockReturnValue(true);
    const open = vi.spyOn(window, "open");
    openLoginAddress(url);
    expect(openUrl).not.toHaveBeenCalled();
    expect(open).not.toHaveBeenCalled();
  },
);
