import { create } from "zustand";
import { LANGUAGES, setLanguage, type Language } from "../copy";
import { defaultBindings, type Keybindings } from "./keybindings";
import { themes, type ThemeId } from "./themes";

const STORAGE_KEY = "vatra.preferences";
const VALID_ACCENT = /^#[0-9a-fA-F]{6}$/;
const MIN_FONT_SCALE = 0.85;
const MAX_FONT_SCALE = 1.4;
const MODES = ["system", "light", "dark"] as const;
const DENSITIES = ["compact", "comfortable"] as const;
const SEND_SHORTCUTS = ["enter", "mod+enter"] as const;

export type Preferences = {
  language: Language;
  mode: (typeof MODES)[number];
  lightTheme: ThemeId;
  darkTheme: ThemeId;
  accent: string | null;
  fontScale: number;
  density: (typeof DENSITIES)[number];
  sendShortcut: (typeof SEND_SHORTCUTS)[number];
  keybindings: Keybindings;
};

export const defaults = (): Preferences => ({
  language: "en",
  mode: "system",
  lightTheme: "vatra",
  darkTheme: "vatra",
  accent: null,
  fontScale: 1,
  density: "comfortable",
  sendShortcut: "enter",
  keybindings: defaultBindings(),
});

const oneOf = <T>(value: unknown, allowed: readonly T[], fallback: T): T =>
  allowed.includes(value as T) ? (value as T) : fallback;

/** Storage can hold anything from an older version, so every field is checked on the way in. */
function sanitize(stored: Partial<Record<keyof Preferences, unknown>>): Preferences {
  const base = defaults();
  const themeIds = Object.keys(themes) as ThemeId[];
  const scale = typeof stored.fontScale === "number" && stored.fontScale >= MIN_FONT_SCALE && stored.fontScale <= MAX_FONT_SCALE;
  const bindings = (stored.keybindings ?? {}) as Partial<Keybindings>;
  return {
    language: oneOf(stored.language, LANGUAGES.map((language) => language.id), base.language),
    mode: oneOf(stored.mode, MODES, base.mode),
    lightTheme: oneOf(stored.lightTheme, themeIds, base.lightTheme),
    darkTheme: oneOf(stored.darkTheme, themeIds, base.darkTheme),
    accent: typeof stored.accent === "string" && VALID_ACCENT.test(stored.accent) ? stored.accent : null,
    fontScale: scale ? (stored.fontScale as number) : base.fontScale,
    density: oneOf(stored.density, DENSITIES, base.density),
    sendShortcut: oneOf(stored.sendShortcut, SEND_SHORTCUTS, base.sendShortcut),
    keybindings: { ...base.keybindings, ...bindings },
  };
}

export function loadPreferences(): Preferences {
  try {
    return sanitize(JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "{}"));
  } catch {
    return defaults();
  }
}

export function savePreferences(preferences: Preferences): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(preferences));
  } catch {
    // Without storage the choice still applies until the app closes.
  }
}

function readableOn(accent: string): string {
  const channel = (start: number) => {
    const value = parseInt(accent.slice(start, start + 2), 16) / 255;
    return value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  };
  const luminance = 0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5);
  return luminance > 0.179 ? "#000000" : "#ffffff";
}

export function applyPreferences(preferences: Preferences, root: HTMLElement, systemDark: boolean): void {
  setLanguage(preferences.language);
  root.lang = preferences.language;
  const dark = preferences.mode === "dark" || (preferences.mode === "system" && systemDark);
  const themeId = dark ? preferences.darkTheme : preferences.lightTheme;
  const palette = dark ? themes[themeId].dark : themes[themeId].light;
  for (const [token, value] of Object.entries(palette)) {
    root.style.setProperty(token, value);
  }
  if (preferences.accent !== null && VALID_ACCENT.test(preferences.accent)) {
    root.style.setProperty("--accent", preferences.accent);
    root.style.setProperty("--on-accent", readableOn(preferences.accent));
  }
  root.style.setProperty("--font-scale", String(preferences.fontScale));
  root.style.colorScheme = dark ? "dark" : "light";
  root.dataset.theme = themeId;
  root.dataset.mode = dark ? "dark" : "light";
  root.dataset.density = preferences.density;
}

export function systemPrefersDark(): boolean {
  return typeof window.matchMedia === "function" && window.matchMedia("(prefers-color-scheme: dark)").matches;
}

export const usePreferences = create<{ preferences: Preferences }>(() => ({ preferences: loadPreferences() }));

export function updatePreferences(change: Partial<Preferences>): void {
  const preferences = { ...usePreferences.getState().preferences, ...change };
  savePreferences(preferences);
  applyPreferences(preferences, document.documentElement, systemPrefersDark());
  usePreferences.setState({ preferences });
}

export function resetPreferences(): void {
  updatePreferences(defaults());
}

export function toggleMode(): void {
  const { mode } = usePreferences.getState().preferences;
  const dark = mode === "dark" || (mode === "system" && systemPrefersDark());
  updatePreferences({ mode: dark ? "light" : "dark" });
}
