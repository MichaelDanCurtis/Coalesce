// Slash command parser for the Chat input.
//
// Commands are recognized when the input begins with `/`. Parsing returns
// a discriminated action the Chat component can dispatch against its own
// state. Unknown commands pass through as a plain message.

export type SlashAction =
  | { kind: "none"; text: string }
  | { kind: "model"; model: string }
  | { kind: "system"; prompt: string }
  | { kind: "clear" }
  | { kind: "branch" }
  | { kind: "regen" }
  | { kind: "unknown"; name: string };

export const SLASH_COMMANDS: Array<{ name: string; desc: string }> = [
  { name: "/model <id>", desc: "Switch the active model for this conversation" },
  { name: "/system <prompt>", desc: "Set the system prompt" },
  { name: "/clear", desc: "Clear messages in the current conversation" },
  { name: "/branch", desc: "Fork the current conversation" },
  { name: "/regen", desc: "Regenerate the last assistant response" },
];

export function parseSlash(raw: string): SlashAction {
  const trimmed = raw.trim();
  if (!trimmed.startsWith("/")) return { kind: "none", text: raw };

  const match = /^\/(\w+)(?:\s+([\s\S]*))?$/.exec(trimmed);
  if (!match) return { kind: "none", text: raw };
  const cmd = match[1].toLowerCase();
  const arg = (match[2] ?? "").trim();

  switch (cmd) {
    case "model":
      if (!arg) return { kind: "unknown", name: "/model requires an argument" };
      return { kind: "model", model: arg };
    case "system":
      return { kind: "system", prompt: arg };
    case "clear":
      return { kind: "clear" };
    case "branch":
      return { kind: "branch" };
    case "regen":
    case "regenerate":
      return { kind: "regen" };
    default:
      return { kind: "unknown", name: cmd };
  }
}
