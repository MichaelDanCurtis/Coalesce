# Phase 19 — Terminal-Styled Native Chat

**Goal:** Make the Chat tab the *reason* non-terminal users install Coalesce. Looks like a terminal (cool factor), behaves like a first-class native client that speaks every modern provider feature fluently — tools, thinking, multi-choice, citations, vision, structured output, streaming, branching.

**Non-goals:** Embedded PTY harnesses (Claude Code, Aider, etc). Terminal users don't need a web app.

---

## Architecture principle

Chat targets the **canonical Rosetta shape** (Phase 18.1), not provider-specific payloads. New provider features → teach Rosetta once → Chat picks them up automatically.

Multi-choice and structured prompts are recognized by **two layers**:
1. **Structured layer** — proxy plugin injects a `respond_with_choices` tool the model can call. Universal capability.
2. **Heuristic layer** — regex scan of plain text for numbered/bulleted patterns. Cheap fallback for any model.

---

## Phase 19.1 — Terminal Skin (cosmetic shell)

Reskin existing Chat.tsx so it *looks* like a terminal without changing behavior.

- New `chat-terminal.css` with: monospace font (JetBrains Mono / Fira Code), CRT-ish dark background, scanline overlay (subtle, optional), green/amber accent palette, blinking block cursor on the input
- Message turns prefixed with shell-style prompts: `user@coalesce:~$` and `assistant >`
- Window chrome: fake macOS traffic-light buttons, title bar showing model name + token count
- Theme toggle in Settings: "Terminal" / "Modern" so users who hate it can switch
- Keep all existing functionality intact

**Deliverable:** Chat tab looks like a terminal. Zero functional regressions.

---

## Phase 19.2 — Structured Response Renderer

Replace plain markdown dump with feature-aware turn rendering. Each assistant turn becomes a sequence of *blocks* derived from the Rosetta normalized response.

Block types:
| Block | Source | Render |
|---|---|---|
| `text` | content string | markdown (existing) |
| `thinking` | reasoning_content / thinking blocks / `<think>` | collapsible "Thinking…" accordion |
| `tool_call` | tool_calls / tool_use | card: name + args + Approve/Deny/Edit |
| `tool_result` | tool role messages | card linked to its call |
| `citation` | Anthropic citations / Perplexity sources | footnote chip |
| `code` | fenced markdown | existing syntax highlighter + Run button (sandboxed) |

**Deliverable:** Thinking, tool calls, and citations render natively instead of as raw text.

---

## Phase 19.3 — Multi-Choice Detection

Two strategies, both shipped:

1. **Heuristic detector** — scan trailing assistant text for `^\s*[1-9][.)]\s` and `^\s*[-*]\s` patterns. Render matched items as clickable buttons. Click sends the chosen text as next user message.
2. **Structured tool** — proxy `on_request` plugin injects a `respond_with_choices` tool definition. Model can call it to formally return `{question, options[]}`. Chat detects the tool call and renders polished choice UI.

**Deliverable:** Asking a model "what should I do next?" produces clickable options, regardless of model.

---

## Phase 19.4 — Rich Input

- Drag-drop files (extends existing attachment system to images, PDFs, DOCX — Phase 16 doc parsing already does this)
- Inline image preview before send
- Mic button → Whisper transcription via proxy
- Slash commands in the input: `/model gpt-5`, `/system <prompt>`, `/clear`, `/branch`, `/regen`
- Per-message model override (small picker on each user turn)

**Deliverable:** Input feels like a modern AI client.

---

## Phase 19.5 — Live Provider Features

- **Streaming**: token-by-token render with stop button (existing — verify works in terminal skin)
- **Prompt cache badge**: when Anthropic `cache_control` hits, show "cached ✓" chip on the turn
- **Token meter**: live context-window fill bar in the title chrome
- **Parallel tool calls**: render multiple tool-use cards side-by-side
- **Continue button** on interrupted/partial responses
- **Generated image rendering** for image-output models
- **TTS playback** button on assistant turns

**Deliverable:** Every modern provider knob is visible and usable in the UI.

---

## Phase 19.6 — Branching & Replay

- Fork conversation at any turn → keep both branches navigable in the sidebar
- "Send to Chat" button on any Timeline tab entry (replay a historical request through current model)
- A/B compare: send same prompt to two models, render side-by-side

**Deliverable:** Power-user workflows that no other client offers.

---

## Phase 19.7 — Polish & Ship

- Keyboard shortcuts cheatsheet (`?` to open)
- Empty state with example prompts that showcase features
- Settings → "Terminal theme" preferences (font, scanlines on/off, color scheme: green/amber/white/coalesce-brand)
- Docs update: README screenshot of the new Chat
- E2E test: send message, render thinking, render tool call, click multi-choice button

**Deliverable:** Feature-complete Phase 19. Tag release.

---

## Sequencing

19.1 first (cosmetic, zero risk, instant wow). Then 19.2 + 19.3 in parallel — they're the meat. Then 19.4–19.6 in any order. 19.7 last.
