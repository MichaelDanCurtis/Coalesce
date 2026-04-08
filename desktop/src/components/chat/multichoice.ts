// Heuristic multi-choice detection.
//
// Scans trailing lines of assistant text for numbered (`1. foo`, `2) bar`)
// or bulleted (`- foo`, `* bar`) lists and returns the options if the tail
// of the message is "mostly" such a list. Designed to run cheaply inside
// parseBlocks — no allocation when nothing matches.

export interface ChoiceDetection {
  question: string;
  options: string[];
  /// Character offset in the original text where the choice list starts.
  /// parseBlocks uses this to keep any preceding prose as a text block.
  start: number;
}

const NUMBERED = /^\s*([1-9])[.)]\s+(.+?)\s*$/;
const BULLETED = /^\s*[-*]\s+(.+?)\s*$/;

/// Detect a trailing choice list. Returns null when nothing qualifies.
///
/// Rules:
/// - At least 2 consecutive matching lines at the tail of the text.
/// - All tail lines must use the same marker family (numbered OR bulleted).
/// - At most 6 options (anything bigger is probably a real list of facts).
/// - Each option label must be <= 160 chars (longer → treat as prose).
export function detectChoices(text: string): ChoiceDetection | null {
  if (!text) return null;
  const lines = text.split("\n");

  // Walk from the end, collecting contiguous matching lines.
  let i = lines.length - 1;
  // Skip pure-whitespace trailing lines.
  while (i >= 0 && lines[i].trim() === "") i--;
  if (i < 0) return null;

  type Kind = "num" | "bul";
  let kind: Kind | null = null;
  const collected: Array<{ line: number; label: string }> = [];

  while (i >= 0) {
    const line = lines[i];
    if (line.trim() === "") break;
    const numMatch = NUMBERED.exec(line);
    const bulMatch = BULLETED.exec(line);
    if (numMatch && (kind === null || kind === "num")) {
      kind = "num";
      collected.push({ line: i, label: numMatch[2] });
      i--;
      continue;
    }
    if (bulMatch && (kind === null || kind === "bul")) {
      kind = "bul";
      collected.push({ line: i, label: bulMatch[1] });
      i--;
      continue;
    }
    break;
  }

  if (collected.length < 2 || collected.length > 6) return null;
  if (collected.some((c) => c.label.length > 160)) return null;

  collected.reverse();
  const firstLine = collected[0].line;
  // Compute character offset of firstLine.
  let start = 0;
  for (let k = 0; k < firstLine; k++) start += lines[k].length + 1;

  const question = lines.slice(0, firstLine).join("\n").trim();

  return {
    question,
    options: collected.map((c) => c.label),
    start,
  };
}

/// Parse a respond_with_choices tool-call args payload. Returns the
/// structured choice or null if the payload is malformed.
export function parseRespondWithChoicesArgs(
  argsJson: string,
): { question: string; options: string[] } | null {
  try {
    const parsed = JSON.parse(argsJson);
    if (
      parsed &&
      typeof parsed === "object" &&
      typeof parsed.question === "string" &&
      Array.isArray(parsed.options) &&
      parsed.options.every((o: unknown) => typeof o === "string")
    ) {
      return { question: parsed.question, options: parsed.options };
    }
  } catch {
    /* fallthrough */
  }
  return null;
}
