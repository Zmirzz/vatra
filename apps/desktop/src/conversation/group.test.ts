import { expect, test } from "vitest";
import type { AgentEvent, StoredEvent } from "../rpc/types";
import { groupEvents, toolSummary, type Block } from "./group";

let seq = 0;
const at = (event: AgentEvent): StoredEvent => ({ seq: ++seq, session_id: 1, event });
const user = (text: string) => at({ type: "user_message", text });
const assistant = (text: string) => at({ type: "assistant_text", text });
const call = (id: string, name = "Bash", args: unknown = { command: "ls" }) => at({ type: "tool_call", id, name, arguments: args });
const result = (id: string, output = "gata", is_error = false) => at({ type: "tool_result", id, output, is_error });
const nested = (parent_id: string, stored: StoredEvent) => at({ type: "nested", parent_id, event: stored.event });
const unmapped = (raw: string) => at({ type: "unmapped", source: "claude", raw });

const work = (block: Block) => {
  if (block.kind !== "work") throw new Error(`bloc ${block.kind}`);
  return block;
};

test("tool activity between messages collapses into one work block", () => {
  const blocks = groupEvents([user("a"), call("1"), result("1"), call("2"), result("2"), assistant("b")]);
  expect(blocks.map((b) => b.kind)).toEqual(["user", "work", "assistant"]);
  expect(work(blocks[1]).items).toHaveLength(2);
});

test("a result joins its call even when it arrives after other events", () => {
  const blocks = groupEvents([call("1"), call("2"), result("2", "doi"), result("1", "unu", true)]);
  const [first, second] = work(blocks[0]).items;
  expect(first.output).toBe("unu");
  expect(first.isError).toBe(true);
  expect(second.output).toBe("doi");
});

test("nested subagent events stay under their parent", () => {
  const blocks = groupEvents([
    call("s", "Agent"),
    at({ type: "subagent_started", id: "s", description: "Caută" }),
    nested("s", call("n")),
    nested("s", result("n")),
    nested("s", assistant("gata")),
    at({ type: "subagent_finished", id: "s", status: "completed" }),
    result("s"),
  ]);
  const parent = work(blocks[0]).items[0];
  expect(parent.children).toHaveLength(1);
  expect(parent.description).toBe("Caută");
  expect(parent.status).toBe("completed");
  expect(parent.texts).toEqual(["gata"]);
});

test("nested events from an unknown parent still appear", () => {
  const blocks = groupEvents([nested("lipsa", call("n"))]);
  expect(JSON.stringify(blocks)).toContain('"n"');
});

test("a denied tool is marked", () => {
  const blocks = groupEvents([call("1"), at({ type: "tool_denied", id: "1", name: "Bash" })]);
  expect(work(blocks[0]).items[0].denied).toBe(true);
});

test("unmapped events are kept as technical details, never dropped", () => {
  expect(JSON.stringify(groupEvents([unmapped("x")]))).toContain("x");
  const blocks = groupEvents([call("1"), unmapped("y"), result("1"), assistant("z")]);
  expect(blocks.map((b) => b.kind)).toEqual(["work", "assistant"]);
  expect(JSON.stringify(blocks)).toContain("y");
});

test("an event type the app doesn't know is kept too", () => {
  expect(JSON.stringify(groupEvents([at({ type: "tip_nou", camp: "valoare" } as never)]))).toContain("valoare");
});

test("failures, warnings, handoffs and limits have their own blocks", () => {
  const brief = { summary: "s", files: [], decisions: [], remaining: [], degraded: true };
  const blocks = groupEvents([
    at({ type: "turn_failed", agent: "a", message: "a căzut" }),
    at({ type: "warning", source: "a", message: "mcp" }),
    at({ type: "handoff_started", from_session: 1, to_agent: "claude", brief }),
    at({ type: "handoff_result", to_session: 2, text: "rezultat" }),
    at({ type: "rate_limit", status: "rejected", resets_at: null }),
  ]);
  expect(blocks.map((b) => b.kind)).toEqual(["failure", "warning", "handoff", "handoff_result", "limit"]);
});

test("an allowed rate limit is only a technical detail", () => {
  const blocks = groupEvents([at({ type: "rate_limit", status: "allowed", resets_at: 1 })]);
  expect(blocks[0].kind).toBe("technical");
});

test("usage is a technical detail, not part of the conversation", () => {
  const usage = at({ type: "usage", input_tokens: 5, output_tokens: 7, cost_usd: null });
  expect(groupEvents([usage])[0].kind).toBe("technical");
});

test("the summary shows what the tool acts on", () => {
  expect(toolSummary({ command: "cargo test" })).toBe("cargo test");
  expect(toolSummary({ file_path: "src/a.rs" })).toBe("src/a.rs");
  expect(toolSummary("nu e obiect")).toBe("");
  expect(toolSummary(null)).toBe("");
  expect(toolSummary({ command: "x".repeat(500) }).length).toBeLessThanOrEqual(121);
});

test("tools without a result stop counting as running when the turn ends", () => {
  const blocks = groupEvents([call("1"), nested("1", call("2")), at({ type: "turn_failed", agent: "a", message: "oprit" })]);
  const parent = work(blocks[0]).items[0];
  expect(parent.interrupted).toBe(true);
  expect(parent.children[0].interrupted).toBe(true);
});

test("a long session groups quickly", () => {
  const events: StoredEvent[] = [];
  for (let n = 0; n < 20_000; n++) {
    events.push(call(String(n)), result(String(n)));
  }
  const start = performance.now();
  groupEvents(events);
  expect(performance.now() - start).toBeLessThan(1_000);
});
