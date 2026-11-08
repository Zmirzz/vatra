import { expect, test } from "vitest";
import { lineDiff } from "./diff";

test("identical text has no changes", () => {
  expect(lineDiff("a\nb", "a\nb")).toEqual([
    { kind: "same", text: "a" },
    { kind: "same", text: "b" },
  ]);
});

test("a replaced line is shown as removed then added", () => {
  expect(lineDiff("a\nold\nc", "a\nnew\nc")).toEqual([
    { kind: "same", text: "a" },
    { kind: "remove", text: "old" },
    { kind: "add", text: "new" },
    { kind: "same", text: "c" },
  ]);
});

test("an empty old text is all additions", () => {
  expect(lineDiff("", "x\ny")).toEqual([
    { kind: "add", text: "x" },
    { kind: "add", text: "y" },
  ]);
});

test("an inserted line keeps its neighbours unchanged", () => {
  expect(lineDiff("1\n3", "1\n2\n3")).toEqual([
    { kind: "same", text: "1" },
    { kind: "add", text: "2" },
    { kind: "same", text: "3" },
  ]);
});

test("a very large edit falls back to removing everything and adding everything", () => {
  const old = Array.from({ length: 3000 }, (_, index) => `old ${index}`).join("\n");
  const next = Array.from({ length: 3000 }, (_, index) => `new ${index}`).join("\n");
  const lines = lineDiff(old, next);
  expect(lines.filter((line) => line.kind === "remove")).toHaveLength(3000);
  expect(lines.filter((line) => line.kind === "add")).toHaveLength(3000);
});
