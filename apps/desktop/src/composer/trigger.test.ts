import { describe, expect, test } from "vitest";
import { findTrigger } from "./trigger";

describe("findTrigger", () => {
  test("slash_only_at_start_of_message", () => {
    expect(findTrigger("/res", 4)).toEqual({ trigger: "/", query: "res", start: 0 });
    expect(findTrigger("ajută /res", 10)).toBeNull();
    expect(findTrigger("\n/res", 5)).toBeNull();
  });

  test("dollar_after_whitespace", () => {
    expect(findTrigger("folosește $sk", 13)).toEqual({ trigger: "$", query: "sk", start: 10 });
    expect(findTrigger("$sk", 3)).toEqual({ trigger: "$", query: "sk", start: 0 });
    expect(findTrigger("a$sk", 4)).toBeNull();
  });

  test("at_after_whitespace", () => {
    expect(findTrigger("citește @src/ma", 15)).toEqual({ trigger: "@", query: "src/ma", start: 8 });
    expect(findTrigger("@", 1)).toEqual({ trigger: "@", query: "", start: 0 });
  });

  test("email_address_is_not_a_trigger", () => {
    expect(findTrigger("scrie la andrei@example.com", 26)).toBeNull();
    expect(findTrigger("andrei@ex", 9)).toBeNull();
  });

  test("caret_moved_away_closes", () => {
    // The caret sits after a word that has no trigger, so the list closes even though a trigger is earlier.
    expect(findTrigger("@fisier nou", 11)).toBeNull();
    // The caret moved before the trigger itself, so nothing is active.
    expect(findTrigger("@fisier", 0)).toBeNull();
  });

  test("a query ends at the caret, not at the end of the text", () => {
    expect(findTrigger("@fis tot", 4)).toEqual({ trigger: "@", query: "fis", start: 0 });
  });
});
