import { expect, test } from "vitest";
import { comboFromEvent, defaultBindings, displayCombo, findConflict, matchCommand } from "./keybindings";

const press = (init: KeyboardEventInit) => new KeyboardEvent("keydown", init);

test("mod means ctrl or cmd", () => {
  expect(matchCommand(press({ key: "n", ctrlKey: true }), defaultBindings())).toBe("session.new");
  expect(matchCommand(press({ key: "n", metaKey: true }), defaultBindings())).toBe("session.new");
  expect(matchCommand(press({ key: "n" }), defaultBindings())).toBeNull();
});

test("shift is part of the combination and case doesn't matter", () => {
  expect(matchCommand(press({ key: "L", ctrlKey: true, shiftKey: true }), defaultBindings())).toBe("theme.cycleMode");
  expect(matchCommand(press({ key: "l", ctrlKey: true }), defaultBindings())).not.toBe("theme.cycleMode");
});

test("combinations are recorded from key presses", () => {
  expect(comboFromEvent(press({ key: "K", ctrlKey: true, altKey: true }))).toBe("mod+alt+k");
  expect(comboFromEvent(press({ key: "Control", ctrlKey: true }))).toBeNull();
  expect(comboFromEvent(press({ key: "k" }))).toBeNull();
});

test("a conflicting shortcut is reported instead of saved silently", () => {
  const bindings = defaultBindings();
  expect(findConflict(bindings, "session.new", bindings["settings.open"])).toBe("settings.open");
  expect(findConflict(bindings, "session.new", bindings["session.new"])).toBeNull();
  expect(findConflict(bindings, "session.new", "mod+shift+y")).toBeNull();
});

test("combinations read like keys, not code", () => {
  expect(displayCombo("mod+shift+l")).toBe("Ctrl Shift L");
  expect(displayCombo("mod+,")).toBe("Ctrl ,");
});
