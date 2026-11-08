export const COMMAND_IDS = ["session.new", "session.search", "settings.open", "theme.cycleMode", "composer.focus", "turn.stop"] as const;

export type CommandId = (typeof COMMAND_IDS)[number];
export type Keybindings = Record<CommandId, string>;

export const defaultBindings = (): Keybindings => ({
  "session.new": "mod+n",
  "session.search": "mod+k",
  "settings.open": "mod+,",
  "theme.cycleMode": "mod+shift+l",
  "composer.focus": "mod+j",
  "turn.stop": "mod+.",
});

const MODIFIER_KEYS = new Set(["Control", "Meta", "Shift", "Alt"]);

/** The combination pressed, or null if it's a bare key that a shortcut shouldn't capture. */
export function comboFromEvent(event: KeyboardEvent): string | null {
  if (MODIFIER_KEYS.has(event.key) || !(event.ctrlKey || event.metaKey)) {
    return null;
  }
  const parts = ["mod"];
  if (event.shiftKey) parts.push("shift");
  if (event.altKey) parts.push("alt");
  parts.push(event.key.toLowerCase());
  return parts.join("+");
}

export function matchCommand(event: KeyboardEvent, bindings: Keybindings): CommandId | null {
  const combo = comboFromEvent(event);
  if (combo === null) {
    return null;
  }
  return COMMAND_IDS.find((command) => bindings[command] === combo) ?? null;
}

export function findConflict(bindings: Keybindings, command: CommandId, combo: string): CommandId | null {
  return COMMAND_IDS.find((candidate) => candidate !== command && bindings[candidate] === combo) ?? null;
}

export function displayCombo(combo: string): string {
  return combo
    .split("+")
    .map((part) => {
      if (part === "mod") return "Ctrl";
      return part.length === 1 ? part.toUpperCase() : part[0].toUpperCase() + part.slice(1);
    })
    .join(" ");
}
