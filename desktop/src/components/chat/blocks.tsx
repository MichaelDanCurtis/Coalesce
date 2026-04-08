import { useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { detectChoices, parseRespondWithChoicesArgs } from "./multichoice";

// ─── Block types ─────────────────────────────────────────────
// Targets the canonical Rosetta normalized response shape
// (crates/coalesce-core/src/rosetta/). Today Chat.tsx only
// captures the plain text stream, so most blocks are derived
// heuristically from msg.content. When the streaming layer
// starts forwarding Rosetta blocks directly, swap parseBlocks
// for a pass-through. See TODO(rosetta) markers below.

export type Block =
  | { kind: "text"; text: string }
  | { kind: "thinking"; text: string }
  | { kind: "tool_call"; id?: string; name: string; args: string; resultId?: string }
  | { kind: "tool_result"; callId?: string; text: string }
  | { kind: "citation"; index: number; title?: string; url?: string; text?: string }
  | { kind: "choices"; question: string; options: string[]; source: "heuristic" | "tool" };

// ─── Parsing ─────────────────────────────────────────────────

// TODO(rosetta): when the proxy streams canonical blocks down,
// replace this with a direct map from rosetta::Block → Block.
export function parseBlocks(content: string): Block[] {
  if (!content) return [];
  const blocks: Block[] = [];
  let rest = content;

  // Extract <think>...</think> blocks (many providers: DeepSeek R1,
  // QwQ, Kimi, GLM thinking mode, Gemini thinking summary).
  const thinkRe = /<think(?:ing)?>([\s\S]*?)<\/think(?:ing)?>/gi;
  let lastIndex = 0;
  let match: RegExpExecArray | null;
  const pieces: Array<{ start: number; end: number; block: Block }> = [];
  while ((match = thinkRe.exec(rest)) !== null) {
    pieces.push({
      start: match.index,
      end: match.index + match[0].length,
      block: { kind: "thinking", text: match[1].trim() },
    });
  }

  // Extract fenced ```tool_call blocks — some models emit these
  // as a convention. TODO(rosetta): drop once canonical tool_use
  // blocks flow through the streaming layer.
  const toolRe = /```tool_call\s*\n([\s\S]*?)\n```/gi;
  while ((match = toolRe.exec(rest)) !== null) {
    let name = "tool";
    let args = match[1];
    try {
      const parsed = JSON.parse(match[1]);
      if (parsed && typeof parsed === "object") {
        name = String(parsed.name ?? parsed.tool ?? "tool");
        args = JSON.stringify(parsed.arguments ?? parsed.args ?? parsed.input ?? {}, null, 2);
      }
    } catch {
      /* keep raw */
    }
    pieces.push({
      start: match.index,
      end: match.index + match[0].length,
      block: { kind: "tool_call", name, args },
    });
  }

  pieces.sort((a, b) => a.start - b.start);

  for (const p of pieces) {
    if (p.start > lastIndex) {
      const text = rest.slice(lastIndex, p.start);
      if (text.trim()) blocks.push({ kind: "text", text });
    }
    blocks.push(p.block);
    lastIndex = p.end;
  }
  if (lastIndex < rest.length) {
    const tail = rest.slice(lastIndex);
    if (tail.trim() || blocks.length === 0) blocks.push({ kind: "text", text: tail });
  }

  // Fallback: nothing matched → single text block
  if (blocks.length === 0) blocks.push({ kind: "text", text: rest });

  // ─── Post-processing passes ────────────────────────────────

  // 1. Convert respond_with_choices tool calls into structured choices.
  const withTool: Block[] = blocks.map((b) => {
    if (b.kind === "tool_call" && b.name === "respond_with_choices") {
      const parsed = parseRespondWithChoicesArgs(b.args);
      if (parsed) {
        return {
          kind: "choices" as const,
          question: parsed.question,
          options: parsed.options,
          source: "tool" as const,
        };
      }
    }
    return b;
  });

  // 2. Heuristic: if the final text block ends in a numbered/bulleted
  // list, split it into a (possibly shorter) text block + choices block.
  const finalIdx = withTool.length - 1;
  const last = withTool[finalIdx];
  if (last && last.kind === "text") {
    const detected = detectChoices(last.text);
    if (detected) {
      const prefix = last.text.slice(0, detected.start);
      const result: Block[] = withTool.slice(0, finalIdx);
      if (prefix.trim()) result.push({ kind: "text", text: prefix });
      result.push({
        kind: "choices",
        question: detected.question,
        options: detected.options,
        source: "heuristic",
      });
      return result;
    }
  }

  return withTool;
}

// ─── Renderers ───────────────────────────────────────────────

interface BlockRendererProps {
  blocks: Block[];
  mdComponents: any;
  streaming?: boolean;
  onChoose?: (choice: string) => void;
}

export function BlockRenderer({ blocks, mdComponents, streaming, onChoose }: BlockRendererProps) {
  // Mark each index with a "tool run" id so consecutive tool_call
  // blocks can be rendered side-by-side as a parallel-tool grid.
  const runIds: Array<number | null> = new Array(blocks.length).fill(null);
  const runs: Array<Array<number>> = [];
  for (let i = 0; i < blocks.length; i++) {
    if (blocks[i].kind !== "tool_call") continue;
    if (i > 0 && blocks[i - 1].kind === "tool_call") {
      const id = runIds[i - 1]!;
      runIds[i] = id;
      runs[id].push(i);
    } else {
      const id = runs.length;
      runs.push([i]);
      runIds[i] = id;
    }
  }
  // Only first index of a multi-block run renders; others skip.
  const skipIdx = new Set<number>();
  for (const run of runs) {
    if (run.length > 1) for (let k = 1; k < run.length; k++) skipIdx.add(run[k]);
  }

  return (
    <div className="space-y-1">
      {blocks.map((b, i) => {
        if (skipIdx.has(i)) return null;
        if (b.kind === "tool_call" && runIds[i] !== null) {
          const run = runs[runIds[i]!];
          if (run.length > 1) {
            return (
              <div key={i} className="flex flex-wrap gap-2">
                {run.map((idx) => {
                  const tb = blocks[idx] as Extract<Block, { kind: "tool_call" }>;
                  return (
                    <div key={idx} className="flex-1 min-w-[240px]">
                      <ToolCallBlock name={tb.name} args={tb.args} />
                    </div>
                  );
                })}
              </div>
            );
          }
        }
        switch (b.kind) {
          case "text":
            return (
              <div
                key={i}
                className="prose prose-sm prose-invert max-w-none text-sm leading-relaxed"
              >
                <ReactMarkdown remarkPlugins={[remarkGfm]} components={mdComponents}>
                  {b.text || (streaming && i === blocks.length - 1 ? "▍" : "")}
                </ReactMarkdown>
              </div>
            );
          case "thinking":
            return <ThinkingBlock key={i} text={b.text} />;
          case "tool_call":
            return <ToolCallBlock key={i} name={b.name} args={b.args} />;
          case "tool_result":
            return (
              <div key={i} className="chat-terminal__block chat-terminal__block--tool">
                <div className="text-[10px] uppercase tracking-wider opacity-60 mb-1">
                  tool_result
                </div>
                <pre className="text-xs whitespace-pre-wrap">{b.text}</pre>
              </div>
            );
          case "choices":
            return (
              <div key={i} className="my-2">
                {b.question ? (
                  <div className="text-sm mb-1 opacity-90">{b.question}</div>
                ) : null}
                {b.options.map((opt, j) => (
                  <button
                    key={j}
                    type="button"
                    className="chat-terminal__choice"
                    onClick={() => onChoose?.(opt)}
                    disabled={!onChoose}
                  >
                    {opt}
                  </button>
                ))}
                {b.source === "heuristic" ? (
                  <div className="text-[10px] opacity-40 mt-1">detected</div>
                ) : null}
              </div>
            );
          case "citation":
            return (
              <a
                key={i}
                href={b.url || "#"}
                target="_blank"
                rel="noreferrer"
                className="chat-terminal__block chat-terminal__block--citation inline-block"
              >
                [{b.index}] {b.title || b.url || b.text}
              </a>
            );
          default:
            return null;
        }
      })}
    </div>
  );
}

function ThinkingBlock({ text }: { text: string }) {
  const [open, setOpen] = useState(false);
  const preview = text.length > 120 ? text.slice(0, 120) + "…" : text;
  return (
    <div className="chat-terminal__block chat-terminal__block--thinking">
      <button
        onClick={() => setOpen((v) => !v)}
        className="text-[10px] uppercase tracking-wider opacity-80 hover:opacity-100 flex items-center gap-1"
      >
        <span>{open ? "▼" : "▶"}</span>
        <span>thinking</span>
        <span className="opacity-50">({text.length} chars)</span>
      </button>
      {open ? (
        <pre className="text-xs whitespace-pre-wrap mt-1 opacity-90">{text}</pre>
      ) : (
        <div className="text-xs opacity-60 mt-1 italic truncate">{preview}</div>
      )}
    </div>
  );
}

function ToolCallBlock({ name, args }: { name: string; args: string }) {
  // TODO(rosetta): wire Approve/Deny/Edit to a pending-tool-call
  // queue surfaced from the proxy plugin chain. Buttons are no-ops.
  const noop = () => {
    /* TODO */
  };
  return (
    <div className="chat-terminal__block chat-terminal__block--tool">
      <div className="flex items-center gap-2 mb-1">
        <span className="text-[10px] uppercase tracking-wider opacity-70">tool_call</span>
        <span className="font-semibold text-sm">{name}</span>
      </div>
      <pre className="text-xs whitespace-pre-wrap opacity-90 bg-black/20 p-2 rounded">
        {args}
      </pre>
      <div className="flex gap-2 mt-2">
        <button
          onClick={noop}
          className="text-[10px] px-2 py-0.5 rounded border border-current opacity-70 hover:opacity-100"
          title="TODO: approval flow"
        >
          Approve
        </button>
        <button
          onClick={noop}
          className="text-[10px] px-2 py-0.5 rounded border border-current opacity-70 hover:opacity-100"
          title="TODO: approval flow"
        >
          Deny
        </button>
        <button
          onClick={noop}
          className="text-[10px] px-2 py-0.5 rounded border border-current opacity-70 hover:opacity-100"
          title="TODO: approval flow"
        >
          Edit
        </button>
      </div>
    </div>
  );
}
