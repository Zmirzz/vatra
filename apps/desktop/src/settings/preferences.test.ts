import { expect, test, vi } from "vitest";
import { applyPreferences, defaults, loadPreferences, savePreferences, toggleMode, updatePreferences, usePreferences } from "./preferences";
import { REQUIRED_TOKENS, themes } from "./themes";

function luminance(hex: string): number {
  const channel = (start: number) => {
    const value = parseInt(hex.slice(start, start + 2), 16) / 255;
    return value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5);
}

function contrast(a: string, b: string): number {
  const [light, dark] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (light + 0.05) / (dark + 0.05);
}

test("corrupt or missing storage falls back to defaults", () => {
  expect(loadPreferences().mode).toBe("system");
  localStorage.setItem("vatra.preferences", "{nu e json");
  expect(loadPreferences().mode).toBe("system");
});

test("storage that throws doesn't break the app", () => {
  const get = vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
    throw new Error("blocat");
  });
  const set = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
    throw new Error("blocat");
  });
  expect(loadPreferences().mode).toBe("system");
  expect(() => savePreferences(defaults())).not.toThrow();
  get.mockRestore();
  set.mockRestore();
});

test("old preferences keep working when new fields appear", () => {
  localStorage.setItem("vatra.preferences", JSON.stringify({ mode: "dark" }));
  expect(loadPreferences()).toMatchObject({ mode: "dark", density: "comfortable" });
  expect(loadPreferences().keybindings["session.new"]).toBe("mod+n");
});

test("invalid stored values are replaced by defaults", () => {
  localStorage.setItem("vatra.preferences", JSON.stringify({ mode: "neon", lightTheme: "lipsa", fontScale: 99, density: 3 }));
  const preferences = loadPreferences();
  expect(preferences.mode).toBe("system");
  expect(preferences.lightTheme).toBe(defaults().lightTheme);
  expect(preferences.fontScale).toBe(1);
  expect(preferences.density).toBe("comfortable");
});

test("the language defaults to English and an unknown one falls back", () => {
  expect(defaults().language).toBe("en");
  localStorage.setItem("vatra.preferences", JSON.stringify({ language: "klingon" }));
  expect(loadPreferences().language).toBe("en");
  applyPreferences({ ...defaults(), language: "ro" }, document.documentElement, true);
  expect(document.documentElement.lang).toBe("ro");
  applyPreferences(defaults(), document.documentElement, true);
});

test("preferences survive a save and load", () => {
  savePreferences({ ...defaults(), mode: "light", accent: "#112233" });
  expect(loadPreferences()).toMatchObject({ mode: "light", accent: "#112233" });
});

test("each mode uses its own theme and follows the system when asked", () => {
  const prefs = { ...defaults(), mode: "system" as const, lightTheme: "paper" as const, darkTheme: "carbon" as const };
  applyPreferences(prefs, document.documentElement, true);
  expect(document.documentElement.dataset.theme).toBe("carbon");
  expect(document.documentElement.dataset.mode).toBe("dark");
  applyPreferences(prefs, document.documentElement, false);
  expect(document.documentElement.dataset.theme).toBe("paper");
  applyPreferences({ ...prefs, mode: "dark" }, document.documentElement, false);
  expect(document.documentElement.dataset.theme).toBe("carbon");
});

test("a valid accent replaces the theme's and an invalid one is ignored", () => {
  applyPreferences({ ...defaults(), accent: "#123456" }, document.documentElement, true);
  expect(document.documentElement.style.getPropertyValue("--accent")).toBe("#123456");
  applyPreferences({ ...defaults(), accent: "url(http://x)" }, document.documentElement, true);
  expect(document.documentElement.style.getPropertyValue("--accent")).not.toContain("url");
});

test("text on the accent stays readable whichever accent is chosen", () => {
  applyPreferences({ ...defaults(), accent: "#ffffff" }, document.documentElement, true);
  const on = document.documentElement.style.getPropertyValue("--on-accent");
  expect(contrast(on, "#ffffff")).toBeGreaterThanOrEqual(4.5);
  applyPreferences({ ...defaults(), accent: "#000000" }, document.documentElement, true);
  const onDark = document.documentElement.style.getPropertyValue("--on-accent");
  expect(contrast(onDark, "#000000")).toBeGreaterThanOrEqual(4.5);
});

test("font scale and density reach the page", () => {
  applyPreferences({ ...defaults(), fontScale: 1.2, density: "compact" }, document.documentElement, true);
  expect(document.documentElement.style.getPropertyValue("--font-scale")).toBe("1.2");
  expect(document.documentElement.dataset.density).toBe("compact");
});

test("every theme defines every required token with readable text", () => {
  for (const theme of Object.values(themes)) {
    for (const palette of [theme.light, theme.dark]) {
      expect(Object.keys(palette)).toEqual(expect.arrayContaining([...REQUIRED_TOKENS]));
      for (const surface of ["--surface-0", "--surface-1", "--surface-2"] as const) {
        expect(contrast(palette["--text-primary"], palette[surface])).toBeGreaterThanOrEqual(4.5);
        expect(contrast(palette["--text-secondary"], palette[surface])).toBeGreaterThanOrEqual(4.5);
      }
      expect(contrast(palette["--on-accent"], palette["--accent"])).toBeGreaterThanOrEqual(4.5);
      expect(contrast(palette["--warning"], palette["--bg-warning"])).toBeGreaterThanOrEqual(4.5);
      expect(contrast(palette["--danger"], palette["--bg-danger"])).toBeGreaterThanOrEqual(4.5);
    }
  }
});

test("the quick switch goes straight to the opposite of what's on screen", () => {
  updatePreferences({ mode: "system" });
  toggleMode();
  expect(document.documentElement.dataset.mode).toBe("dark");
  toggleMode();
  expect(usePreferences.getState().preferences.mode).toBe("light");
  updatePreferences(defaults());
});
