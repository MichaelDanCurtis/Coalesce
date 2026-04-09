// Canonical Rosetta response wire types — mirror of
// crates/coalesce-core/src/rosetta/response.rs.
//
// Emitted per-chunk by the proxy SSE forwarder under the sibling
// key `x_coalesce` on OpenAI-compat delta frames (see Ticket B2).
// Do NOT confuse with `x_coalesce_route`, which is a one-shot
// head-of-stream routing metadata frame (see Ticket B3).

export type CanonicalBlock =
  | { kind: "text"; text: string }
  | { kind: "thinking"; text: string; signature?: string }
  | { kind: "tool_call_start"; id: string; name: string }
  | { kind: "tool_call_delta"; id: string; arguments_delta: string }
  | { kind: "tool_call_end"; id: string }
  | { kind: "tool_call"; id: string; name: string; arguments: unknown }
  | { kind: "tool_result"; tool_call_id: string; content: string; is_error?: boolean }
  | {
      kind: "citation";
      source: string;
      text?: string;
      start?: number;
      end?: number;
    }
  | { kind: "image"; media_type: string; data?: string; url?: string };

export type FinishReason =
  | "stop"
  | "length"
  | "tool_calls"
  | "content_filter"
  | "error";

export interface CanonicalUsage {
  prompt_tokens?: number;
  completion_tokens?: number;
  cache_read_input_tokens?: number;
  cache_creation_input_tokens?: number;
}

export interface CanonicalStreamDelta {
  blocks?: CanonicalBlock[];
  finish_reason?: FinishReason;
  usage?: CanonicalUsage;
}
