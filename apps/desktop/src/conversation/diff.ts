export type DiffLine = { kind: "same" | "add" | "remove"; text: string };

/** Past this many line pairs, an exact diff costs more than it shows, so the change is shown whole. */
const MAX_TABLE_CELLS = 1_000_000;

const splitLines = (text: string): string[] => (text === "" ? [] : text.split("\n"));

/** A line diff by longest common subsequence. Lines unchanged at either end are kept out of the table. */
export function lineDiff(oldText: string, newText: string): DiffLine[] {
  const before = splitLines(oldText);
  const after = splitLines(newText);
  let start = 0;
  while (start < before.length && start < after.length && before[start] === after[start]) {
    start += 1;
  }
  let endBefore = before.length;
  let endAfter = after.length;
  while (endBefore > start && endAfter > start && before[endBefore - 1] === after[endAfter - 1]) {
    endBefore -= 1;
    endAfter -= 1;
  }
  return [
    ...before.slice(0, start).map((text): DiffLine => ({ kind: "same", text })),
    ...changedLines(before.slice(start, endBefore), after.slice(start, endAfter)),
    ...before.slice(endBefore).map((text): DiffLine => ({ kind: "same", text })),
  ];
}

function changedLines(before: string[], after: string[]): DiffLine[] {
  const rows = before.length;
  const columns = after.length;
  if (rows * columns > MAX_TABLE_CELLS) {
    return [
      ...before.map((text): DiffLine => ({ kind: "remove", text })),
      ...after.map((text): DiffLine => ({ kind: "add", text })),
    ];
  }
  // table[i * (columns + 1) + j] is the length of the common part of before[i..] and after[j..].
  const width = columns + 1;
  const table = new Uint32Array((rows + 1) * width);
  for (let i = rows - 1; i >= 0; i -= 1) {
    for (let j = columns - 1; j >= 0; j -= 1) {
      table[i * width + j] =
        before[i] === after[j] ? table[(i + 1) * width + j + 1] + 1 : Math.max(table[(i + 1) * width + j], table[i * width + j + 1]);
    }
  }
  const lines: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < rows && j < columns) {
    if (before[i] === after[j]) {
      lines.push({ kind: "same", text: before[i] });
      i += 1;
      j += 1;
    } else if (table[(i + 1) * width + j] >= table[i * width + j + 1]) {
      lines.push({ kind: "remove", text: before[i] });
      i += 1;
    } else {
      lines.push({ kind: "add", text: after[j] });
      j += 1;
    }
  }
  before.slice(i).forEach((text) => lines.push({ kind: "remove", text }));
  after.slice(j).forEach((text) => lines.push({ kind: "add", text }));
  return lines;
}
