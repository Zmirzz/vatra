import { expect, test } from "vitest";
import { agentLabel, commandFitsInline, projectTint, toolLabel, toolWords, visibleText } from "./labels";

test("common tools have plain names", () => {
  expect(toolLabel("Bash")).toBe("Ran a command");
  expect(toolLabel("Read")).toBe("Read a file");
  expect(toolLabel("Edit")).toBe("Changed a file");
  expect(toolLabel("Grep")).toBe("Searched");
  expect(toolLabel("WebFetch")).toBe("Searched the web");
});

test("mcp tools read as words from their source", () => {
  expect(toolWords("mcp__github__create_issue")).toEqual({ name: "create issue", source: "github" });
  expect(toolLabel("mcp__demo__echo")).toBe("echo from demo");
});

test("an unknown tool keeps its own name", () => {
  expect(toolLabel("Something")).toBe("Something");
});

test("agents have readable names", () => {
  expect(agentLabel("claude")).toBe("Claude Code");
  expect(agentLabel("openrouter")).toBe("openrouter");
});

test("a project keeps its tint", () => {
  expect(projectTint(3)).toBe(projectTint(3));
  expect(projectTint(1)).not.toBe(projectTint(2));
});

test("only a plain, fully visible command can be allowed from the list", () => {
  expect(commandFitsInline("Bash", { command: "cargo test", description: "x", timeout: 5 })).toBe(true);
  expect(commandFitsInline("mcp__x__run", { command: "ls" })).toBe(false);
  expect(commandFitsInline("Bash", { command: "ls", run_in_background: true })).toBe(false);
  expect(commandFitsInline("Bash", { command: "x".repeat(81) })).toBe(false);
  expect(commandFitsInline("Bash", { command: "a\nb" })).toBe(false);
  expect(commandFitsInline("Bash", { command: "a\tb" })).toBe(false);
  expect(commandFitsInline("Bash", { command: "a  b" })).toBe(false);
  expect(commandFitsInline("Bash", { command: "safe\u202Egnp.exe" })).toBe(false);
  expect(commandFitsInline("Bash", { command: "a\u200Bb" })).toBe(false);
});

test("invisible characters become visible", () => {
  expect(visibleText("a\u202Eb")).toBe("a\\u{202E}b");
  expect(visibleText("line\nnext")).toBe("line\nnext");
});
