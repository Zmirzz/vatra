import { en, type Strings } from "./i18n/en";
import { ro } from "./i18n/ro";

export const LANGUAGES = [
  { id: "en", label: "English" },
  { id: "ro", label: "Română" },
] as const;

export type Language = (typeof LANGUAGES)[number]["id"];

const STRINGS: Record<Language, Strings> = { en, ro };

let current: Language = "en";

export function setLanguage(language: Language): void {
  current = language;
}

/** Always reads the chosen language, so components only need to render again when it changes. */
export const copy = new Proxy({} as Strings, {
  get: (_, key) => STRINGS[current][key as keyof Strings],
});
