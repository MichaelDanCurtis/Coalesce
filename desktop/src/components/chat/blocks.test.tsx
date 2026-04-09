// Phase 19.7 — parseBlocks spec.
//
// There is no configured test runner in desktop/ at the time of
// writing (no vitest, no jest). This file uses a tiny home-grown
// harness so it typechecks under the existing tsconfig and can be
// executed directly with:
//
//   cd desktop && npx tsx src/components/chat/blocks.test.tsx
//
// If a runner is added later, swap `test`/`assertEq` for its
// primitives. The assertions themselves are the spec.

import { parseBlocks, canonicalToLocalBlocks, type Block } from "./blocks";
import type { CanonicalBlock } from "../../types/rosetta";

type TestFn = () => void;
const cases: Array<{ name: string; fn: TestFn }> = [];
function test(name: string, fn: TestFn) {
  cases.push({ name, fn });
}
function assertTrue(cond: unknown, msg: string): asserts cond {
  if (!cond) throw new Error("assertion failed: " + msg);
}
function assertEq<T>(actual: T, expected: T, msg: string) {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  if (a !== e) throw new Error(`${msg}: expected ${e}, got ${a}`);
}

function kinds(bs: Block[]): string[] {
  return bs.map((b) => b.kind);
}

test("parseBlocks returns a single text block for plain prose", () => {
  const bs = parseBlocks("Hello, world. This is a plain sentence.");
  assertEq(kinds(bs), ["text"], "plain prose is one text block");
  assertTrue(
    (bs[0] as { kind: "text"; text: string }).text.startsWith("Hello"),
    "text preserved",
  );
});

test("parseBlocks extracts <think> tags into a thinking block", () => {
  const bs = parseBlocks(
    "<think>weighing options carefully</think>\nHere is the answer.",
  );
  const ks = kinds(bs);
  assertTrue(ks.includes("thinking"), "has thinking");
  assertTrue(ks.includes("text"), "has trailing text");
  const thinking = bs.find((b) => b.kind === "thinking") as Extract<Block, { kind: "thinking" }>;
  assertEq(thinking.text, "weighing options carefully", "thinking text preserved");
});

test("parseBlocks extracts fenced tool_call blocks", () => {
  const content = [
    "Let me check the weather.",
    "```tool_call",
    JSON.stringify({ name: "get_weather", arguments: { city: "Tokyo" } }),
    "```",
  ].join("\n");
  const bs = parseBlocks(content);
  const tc = bs.find((b) => b.kind === "tool_call") as Extract<Block, { kind: "tool_call" }>;
  assertTrue(tc, "tool_call block should be present");
  assertEq(tc.name, "get_weather", "tool name");
  assertTrue(tc.args.includes("Tokyo"), "tool args include city");
});

test("parseBlocks detects trailing multi-choice lists", () => {
  const content = [
    "Which language should you pick?",
    "",
    "1. Python",
    "2. Rust",
    "3. Go",
  ].join("\n");
  const bs = parseBlocks(content);
  const choice = bs.find((b) => b.kind === "choices") as Extract<Block, { kind: "choices" }>;
  assertTrue(choice, "choices block present");
  assertEq(choice.source, "heuristic", "heuristic source");
  assertEq(choice.options, ["Python", "Rust", "Go"], "numbered options");
  assertTrue(bs.some((b) => b.kind === "text"), "question kept as text");
});

test("parseBlocks passes through bulleted multi-choice", () => {
  const content = "Options:\n- red\n- green\n- blue";
  const bs = parseBlocks(content);
  const choice = bs.find((b) => b.kind === "choices") as Extract<Block, { kind: "choices" }>;
  assertTrue(choice, "bulleted choices detected");
  assertEq(choice.options, ["red", "green", "blue"], "bulleted options");
});

test("parseBlocks converts respond_with_choices tool-call into choices block", () => {
  const content = [
    "```tool_call",
    JSON.stringify({
      name: "respond_with_choices",
      arguments: { question: "pick one", options: ["a", "b"] },
    }),
    "```",
  ].join("\n");
  const bs = parseBlocks(content);
  const choice = bs.find((b) => b.kind === "choices") as Extract<Block, { kind: "choices" }>;
  assertTrue(choice, "tool-call converted to choices");
  assertEq(choice.source, "tool", "tool source");
  assertEq(choice.question, "pick one", "tool question");
  assertEq(choice.options, ["a", "b"], "tool options");
});

test("canonicalToLocalBlocks aggregates streamed tool-call deltas by id", () => {
  const canonical: CanonicalBlock[] = [
    { kind: "thinking", text: "considering tools" },
    { kind: "tool_call_start", id: "call_1", name: "get_weather" },
    { kind: "tool_call_delta", id: "call_1", arguments_delta: '{"city":' },
    { kind: "tool_call_delta", id: "call_1", arguments_delta: '"Tokyo"}' },
    { kind: "tool_call_end", id: "call_1" },
    { kind: "text", text: "Here's the forecast." },
  ];
  const bs = canonicalToLocalBlocks(canonical);
  assertEq(kinds(bs), ["thinking", "tool_call", "text"], "3 aggregated blocks");
  const tc = bs[1] as Extract<Block, { kind: "tool_call" }>;
  assertEq(tc.name, "get_weather", "tool name preserved");
  assertEq(tc.id, "call_1", "tool id preserved");
  // tool_call_end pretty-prints the accumulated JSON.
  const parsed = JSON.parse(tc.args);
  assertEq(parsed.city, "Tokyo", "args accumulated across deltas");
});

test("canonicalToLocalBlocks maps fully-formed tool_call and respond_with_choices", () => {
  const canonical: CanonicalBlock[] = [
    {
      kind: "tool_call",
      id: "c1",
      name: "respond_with_choices",
      arguments: { question: "pick", options: ["x", "y"] },
    },
  ];
  const bs = canonicalToLocalBlocks(canonical);
  assertEq(kinds(bs), ["choices"], "tool-call becomes choices block");
  const ch = bs[0] as Extract<Block, { kind: "choices" }>;
  assertEq(ch.source, "tool", "tool source");
  assertEq(ch.options, ["x", "y"], "options preserved");
});

// Runner — executes when file is loaded directly via tsx.
let failed = 0;
for (const c of cases) {
  try {
    c.fn();
    // eslint-disable-next-line no-console
    console.log("  ok  " + c.name);
  } catch (err) {
    failed++;
    // eslint-disable-next-line no-console
    console.error("  FAIL " + c.name + "\n    " + (err as Error).message);
  }
}
if (failed > 0) {
  // eslint-disable-next-line no-console
  console.error(`${failed} test(s) failed`);
}
