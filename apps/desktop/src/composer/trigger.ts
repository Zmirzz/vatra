export type Trigger = "/" | "$" | "@";

export type ActiveTrigger = { trigger: Trigger; query: string; start: number };

const TRIGGERS: readonly string[] = ["/", "$", "@"];

/**
 * The completion that the caret is in, if any. The word before the caret must begin with a trigger, and that
 * trigger must follow whitespace (or the start of the text). `/` also needs to be the first thing in the message,
 * so a path or an address in the middle of a sentence never opens the list.
 */
export function findTrigger(text: string, caret: number): ActiveTrigger | null {
  const before = text.slice(0, caret);
  const start = Math.max(before.search(/\s\S*$/) + 1, 0);
  const word = before.slice(start);
  const trigger = word.charAt(0);
  if (!TRIGGERS.includes(trigger)) {
    return null;
  }
  if (trigger === "/" && start !== 0) {
    return null;
  }
  return { trigger: trigger as Trigger, query: word.slice(1), start };
}
