import { useState, useEffect, useRef, useCallback, useMemo } from "react";
import { Prism as SyntaxHighlighter } from "react-syntax-highlighter";
import { oneDark } from "react-syntax-highlighter/dist/esm/styles/prism";
import RoutingViz, { RoutingPath } from "./RoutingViz";
import { api } from "../api/client";
import { parseBlocks, BlockRenderer } from "./chat/blocks";
import { parseSlash } from "./chat/slashCommands";

// ─── Types ───────────────────────────────────────────────────

interface ChatMessage {
  id: string;
  role: "user" | "assistant" | "system";
  content: string;
  timestamp: number;
  routing?: RoutingPath;
  attachments?: Attachment[];
  rated?: "up" | "down";
}

interface Attachment {
  name: string;
  type: string;
  dataUrl: string; // base64 data URL
  extractedText?: string; // parsed document text (not shown in UI)
  parsing?: boolean; // true while backend is extracting
  error?: string; // extraction error message
}

interface Conversation {
  id: string;
  title: string;
  messages: ChatMessage[];
  model: string;
  systemPrompt: string;
  createdAt: number;
  updatedAt: number;
  // 19.6 branching metadata
  parentId?: string;
  forkedFromMessageId?: string;
}

// ─── 19.6 Send-to-Chat handoff payload (localStorage) ───────
const HANDOFF_KEY = "coalesce_chat_handoff";
interface ChatHandoff {
  prompt: string;
  systemPrompt?: string;
  model?: string;
}

interface ModelOption {
  id: string;
  provider: string;
  qualityTier: string;
  vision: boolean;
  contextWindow?: number;
}

// ─── Constants ───────────────────────────────────────────────

const API_BASE = "http://127.0.0.1:8402";
const STORAGE_KEY = "coalesce_conversations";

// ─── Helpers ─────────────────────────────────────────────────

function generateId(): string {
  return Date.now().toString(36) + Math.random().toString(36).slice(2, 8);
}

function newConversation(model: string): Conversation {
  return {
    id: generateId(),
    title: "New Chat",
    messages: [],
    model,
    systemPrompt: "",
    createdAt: Date.now(),
    updatedAt: Date.now(),
  };
}

function loadConversations(): Conversation[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    return raw ? JSON.parse(raw) : [];
  } catch {
    return [];
  }
}

function saveConversations(convos: Conversation[]) {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(convos));
}

function formatTokens(n: number): string {
  if (n >= 1_000_000) return (n / 1_000_000).toFixed(1) + "M";
  if (n >= 1_000) return (n / 1_000).toFixed(1) + "K";
  return String(n);
}

// ─── Markdown renderer components ────────────────────────────

const mdComponents = {
  code({ className, children, ...props }: any) {
    const match = /language-(\w+)/.exec(className || "");
    const code = String(children).replace(/\n$/, "");
    if (match) {
      return (
        <div className="relative group my-2">
          <button
            className="absolute top-1 right-1 text-xs px-1.5 py-0.5 rounded bg-white/10 opacity-0 group-hover:opacity-100 transition-opacity"
            onClick={() => navigator.clipboard.writeText(code)}
          >
            Copy
          </button>
          <SyntaxHighlighter
            style={oneDark}
            language={match[1]}
            PreTag="div"
            customStyle={{
              margin: 0,
              borderRadius: "0.5rem",
              fontSize: "0.8rem",
            }}
          >
            {code}
          </SyntaxHighlighter>
        </div>
      );
    }
    return (
      <code className="bg-white/10 px-1 py-0.5 rounded text-sm" {...props}>
        {children}
      </code>
    );
  },
  p({ children }: any) {
    return <p className="mb-2 last:mb-0">{children}</p>;
  },
  ul({ children }: any) {
    return <ul className="list-disc pl-5 mb-2 space-y-1">{children}</ul>;
  },
  ol({ children }: any) {
    return <ol className="list-decimal pl-5 mb-2 space-y-1">{children}</ol>;
  },
  table({ children }: any) {
    return (
      <div className="overflow-x-auto my-2">
        <table className="w-full text-sm border-collapse">{children}</table>
      </div>
    );
  },
  th({ children }: any) {
    return (
      <th className="border border-themed px-2 py-1 text-left text-secondary font-medium">
        {children}
      </th>
    );
  },
  td({ children }: any) {
    return <td className="border border-themed px-2 py-1">{children}</td>;
  },
  blockquote({ children }: any) {
    return (
      <blockquote className="border-l-2 border-brand-400 pl-3 my-2 text-secondary italic">
        {children}
      </blockquote>
    );
  },
  img({ src, alt }: any) {
    // Phase 19.5: render generated images inline. Capped to a
    // reasonable max so they don't overflow the chat column.
    return (
      <img
        src={src}
        alt={alt || ""}
        className="my-2 rounded-md border border-themed max-w-full max-h-96 object-contain"
        loading="lazy"
      />
    );
  },
};

// ─── Main Component ──────────────────────────────────────────

export default function Chat() {
  // State
  const [conversations, setConversations] = useState<Conversation[]>(loadConversations);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [models, setModels] = useState<ModelOption[]>([]);
  const [input, setInput] = useState("");
  const [streaming, setStreaming] = useState(false);
  const [abortController, setAbortController] = useState<AbortController | null>(null);
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [showSettings, setShowSettings] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");
  const [selectedProvider, setSelectedProvider] = useState<string>("any");
  const [selectedModel, setSelectedModel] = useState<string>("auto");
  const [selectedRouting, setSelectedRouting] = useState<RoutingPath | null>(null);
  const [editingMessageId, setEditingMessageId] = useState<string | null>(null);
  const [editContent, setEditContent] = useState("");
  const [sidebarWidth] = useState(240);
  const [vizWidth] = useState(200);

  const chatEndRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  // ─── 19.6 A/B compare mode ────────────────────────────────
  const [abMode, setAbMode] = useState(false);
  const [abModelB, setAbModelB] = useState<string>("auto");
  const [abViewIds, setAbViewIds] = useState<[string, string] | null>(null);

  // ─── 19.4 rich input state ────────────────────────────────
  const [recording, setRecording] = useState(false);
  const [transcribing, setTranscribing] = useState(false);
  const mediaRecorderRef = useRef<MediaRecorder | null>(null);
  const mediaChunksRef = useRef<Blob[]>([]);
  const mediaStreamRef = useRef<MediaStream | null>(null);

  // Derived
  const active = useMemo(
    () => conversations.find((c) => c.id === activeId) || null,
    [conversations, activeId]
  );

  const providers = useMemo(() => {
    const provSet = new Set(models.map((m) => m.provider));
    return Array.from(provSet).sort();
  }, [models]);

  // ─── 19.5 Context window fill ─────────────────────────────
  // Uses the last assistant turn's reported inputTokens as a proxy for
  // current context usage. Falls back to a summed estimate if no routing
  // data is attached yet.
  const contextFill = useMemo(() => {
    if (!active) return null;
    const lastWithTokens = [...active.messages]
      .reverse()
      .find((m) => m.routing?.inputTokens || m.routing?.outputTokens);
    const used =
      (lastWithTokens?.routing?.inputTokens || 0) +
      (lastWithTokens?.routing?.outputTokens || 0);
    const modelOpt = models.find((m) => m.id === (active.model || selectedModel));
    const max = modelOpt?.contextWindow ?? 128000;
    return { used, max };
  }, [active, models, selectedModel]);

  const filteredModels = useMemo(() => {
    if (selectedProvider === "any") return models;
    return models.filter((m) => m.provider === selectedProvider);
  }, [models, selectedProvider]);

  const filteredConversations = useMemo(() => {
    if (!searchQuery) return conversations;
    const q = searchQuery.toLowerCase();
    return conversations.filter(
      (c) =>
        c.title.toLowerCase().includes(q) ||
        c.messages.some((m) => m.content.toLowerCase().includes(q))
    );
  }, [conversations, searchQuery]);

  // ─── Effects ─────────────────────────────────────────────

  // Load models on mount
  useEffect(() => {
    fetch(`${API_BASE}/v1/models`)
      .then((r) => r.json())
      .then((data) => {
        const opts: ModelOption[] = (data.data || []).map((m: any) => ({
          id: m.id,
          provider: m.owned_by,
          qualityTier: m.quality_tier,
          vision: m.vision || false,
          contextWindow: m.context_window ?? m.context_length ?? undefined,
        }));
        // Sort: free first, then by provider, then by name
        opts.sort((a, b) => a.id.localeCompare(b.id));
        setModels(opts);
      })
      .catch(() => {});
  }, []);

  // Persist conversations
  useEffect(() => {
    saveConversations(conversations);
  }, [conversations]);

  // ─── 19.6 Send-to-Chat handoff from Timeline ─────────────
  // Timeline (or any other tab) can drop a payload in localStorage
  // and dispatch `coalesce:chat-handoff`; we pick it up and seed a
  // new conversation.
  useEffect(() => {
    const applyHandoff = () => {
      try {
        const raw = localStorage.getItem(HANDOFF_KEY);
        if (!raw) return;
        localStorage.removeItem(HANDOFF_KEY);
        const payload = JSON.parse(raw) as ChatHandoff;
        const conv = newConversation(payload.model || selectedModel);
        if (payload.systemPrompt) conv.systemPrompt = payload.systemPrompt;
        if (payload.prompt) {
          conv.messages.push({
            id: generateId(),
            role: "user",
            content: payload.prompt,
            timestamp: Date.now(),
          });
          conv.title = payload.prompt.slice(0, 40) || conv.title;
        }
        setConversations((prev) => [conv, ...prev]);
        setActiveId(conv.id);
        if (payload.model) setSelectedModel(payload.model);
      } catch {
        /* ignore malformed payload */
      }
    };
    applyHandoff();
    window.addEventListener("coalesce:chat-handoff", applyHandoff);
    return () => window.removeEventListener("coalesce:chat-handoff", applyHandoff);
  }, [selectedModel]);

  // Scroll to bottom on new messages
  useEffect(() => {
    chatEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [active?.messages.length, streaming]);

  // Auto-resize textarea
  useEffect(() => {
    if (inputRef.current) {
      inputRef.current.style.height = "auto";
      inputRef.current.style.height =
        Math.min(inputRef.current.scrollHeight, 200) + "px";
    }
  }, [input]);

  // Select last routing on active change
  useEffect(() => {
    if (active) {
      const lastAssistant = [...active.messages]
        .reverse()
        .find((m) => m.role === "assistant" && m.routing);
      setSelectedRouting(lastAssistant?.routing || null);
    } else {
      setSelectedRouting(null);
    }
  }, [active?.id, active?.messages.length]);

  // ─── Conversation management ─────────────────────────────

  const updateConversation = useCallback(
    (id: string, updater: (c: Conversation) => Conversation) => {
      setConversations((prev) =>
        prev.map((c) => (c.id === id ? updater(c) : c))
      );
    },
    []
  );

  const createNewChat = useCallback(() => {
    const conv = newConversation(selectedModel);
    setConversations((prev) => [conv, ...prev]);
    setActiveId(conv.id);
    setInput("");
    setAttachments([]);
    setSelectedRouting(null);
    inputRef.current?.focus();
  }, [selectedModel]);

  // ─── 19.6 Fork-from-here ───────────────────────────────
  // Create a new conversation containing messages [0..forkMsgId] and
  // switch to it. Tracks parentId/forkedFromMessageId so the sidebar
  // can badge forked conversations. Used by the inline fork button,
  // the /branch slash command, and the per-turn model override.
  const forkFrom = useCallback(
    (
      sourceConv: Conversation,
      forkMsgId: string | null,
      opts?: { modelOverride?: string; switchTo?: boolean },
    ): Conversation => {
      const idx =
        forkMsgId === null
          ? sourceConv.messages.length - 1
          : sourceConv.messages.findIndex((m) => m.id === forkMsgId);
      const upto = idx >= 0 ? sourceConv.messages.slice(0, idx + 1) : [...sourceConv.messages];
      const fork: Conversation = {
        id: generateId(),
        title: sourceConv.title + " (fork)",
        messages: upto.map((m) => ({ ...m })),
        model: opts?.modelOverride ?? sourceConv.model,
        systemPrompt: sourceConv.systemPrompt,
        createdAt: Date.now(),
        updatedAt: Date.now(),
        parentId: sourceConv.id,
        forkedFromMessageId: forkMsgId ?? undefined,
      };
      setConversations((prev) => [fork, ...prev]);
      if (opts?.switchTo !== false) {
        setActiveId(fork.id);
        if (opts?.modelOverride) setSelectedModel(opts.modelOverride);
      }
      return fork;
    },
    [],
  );

  const deleteConversation = useCallback(
    (id: string) => {
      setConversations((prev) => prev.filter((c) => c.id !== id));
      if (activeId === id) {
        setActiveId(null);
        setSelectedRouting(null);
      }
    },
    [activeId]
  );

  const renameConversation = useCallback(
    (id: string, title: string) => {
      updateConversation(id, (c) => ({ ...c, title }));
    },
    [updateConversation]
  );

  const exportConversation = useCallback(
    (conv: Conversation) => {
      const md = conv.messages
        .map((m) => {
          const prefix = m.role === "user" ? "**You:**" : "**Assistant:**";
          const cost = m.routing
            ? ` *(${m.routing.provider}/${m.routing.model}, $${m.routing.costUsd.toFixed(4)})*`
            : "";
          return `${prefix}${cost}\n\n${m.content}`;
        })
        .join("\n\n---\n\n");

      const blob = new Blob([md], { type: "text/markdown" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = `${conv.title.replace(/[^a-z0-9]/gi, "_")}.md`;
      a.click();
      URL.revokeObjectURL(url);
    },
    []
  );

  // ─── File upload ─────────────────────────────────────────

  const TEXT_EXTS = [".txt", ".md", ".json", ".csv"];
  const PARSE_EXTS = [".pdf", ".docx"];

  const handleFileUpload = useCallback(
    (files: FileList | null) => {
      if (!files) return;
      Array.from(files).forEach((file) => {
        const reader = new FileReader();
        reader.onload = async () => {
          const dataUrl = reader.result as string;
          const ext = "." + file.name.split(".").pop()?.toLowerCase();

          if (file.type.startsWith("image/")) {
            // Images: keep as-is
            setAttachments((prev) => [...prev, { name: file.name, type: file.type, dataUrl }]);
          } else if (TEXT_EXTS.includes(ext)) {
            // Text files: decode client-side
            const b64 = dataUrl.split(",")[1] || "";
            const text = atob(b64);
            setAttachments((prev) => [...prev, { name: file.name, type: file.type, dataUrl, extractedText: text }]);
          } else if (PARSE_EXTS.includes(ext)) {
            // PDFs: send to backend for extraction
            setAttachments((prev) => [...prev, { name: file.name, type: file.type, dataUrl, parsing: true }]);
            try {
              const resp = await fetch(`http://127.0.0.1:8402/api/v1/parse`, {
                method: "POST",
                headers: { "Content-Type": "application/json" },
                body: JSON.stringify({ data: dataUrl, filename: file.name }),
              });
              const result = await resp.json();
              if (result.error) {
                setAttachments((prev) => prev.map((a) => a.name === file.name && a.parsing ? { ...a, parsing: false, error: result.error } : a));
              } else {
                setAttachments((prev) => prev.map((a) => a.name === file.name && a.parsing ? { ...a, parsing: false, extractedText: result.text } : a));
              }
            } catch (e) {
              setAttachments((prev) => prev.map((a) => a.name === file.name && a.parsing ? { ...a, parsing: false, error: "Parse failed" } : a));
            }
          } else {
            // Unsupported — add with error
            setAttachments((prev) => [...prev, { name: file.name, type: file.type, dataUrl, error: "Unsupported format" }]);
          }
        };
        reader.readAsDataURL(file);
      });
    },
    [attachments.length]
  );

  const handlePaste = useCallback(
    (e: React.ClipboardEvent) => {
      const items = e.clipboardData?.items;
      if (!items) return;
      for (const item of Array.from(items)) {
        if (item.type.startsWith("image/")) {
          e.preventDefault();
          const file = item.getAsFile();
          if (file) handleFileUpload([file] as unknown as FileList);
        }
      }
    },
    [handleFileUpload]
  );

  const handleDrop = useCallback(
    (e: React.DragEvent) => {
      e.preventDefault();
      handleFileUpload(e.dataTransfer.files);
    },
    [handleFileUpload]
  );

  const removeAttachment = useCallback((idx: number) => {
    setAttachments((prev) => prev.filter((_, i) => i !== idx));
  }, []);

  // ─── Send message ────────────────────────────────────────

  const sendMessage = useCallback(
    async (overrideContent?: string) => {
      const content = overrideContent || input.trim();
      if (!content && attachments.length === 0) return;
      if (streaming) return;
      if (attachments.some(a => a.parsing)) return;

      let convId = activeId;
      let conv = active;

      // Create new conversation if needed
      if (!conv) {
        const newConv = newConversation(selectedModel);
        setConversations((prev) => [newConv, ...prev]);
        setActiveId(newConv.id);
        convId = newConv.id;
        conv = newConv;
      }

      // Build user message
      const userMsg: ChatMessage = {
        id: generateId(),
        role: "user",
        content,
        timestamp: Date.now(),
        attachments: attachments.length > 0 ? [...attachments] : undefined,
      };

      // Build assistant placeholder
      const assistantMsg: ChatMessage = {
        id: generateId(),
        role: "assistant",
        content: "",
        timestamp: Date.now(),
      };

      // Update conversation with user message + placeholder
      const updatedMessages = [...(conv?.messages || []), userMsg, assistantMsg];
      setConversations((prev) =>
        prev.map((c) =>
          c.id === convId
            ? {
                ...c,
                messages: updatedMessages,
                updatedAt: Date.now(),
                title:
                  c.messages.length === 0
                    ? content.slice(0, 50) + (content.length > 50 ? "…" : "")
                    : c.title,
              }
            : c
        )
      );

      setInput("");
      setAttachments([]);
      setStreaming(true);

      const controller = new AbortController();
      setAbortController(controller);

      try {
        // Build messages array for API
        const apiMessages: any[] = [];

        // System prompt
        const systemPrompt = conv?.systemPrompt;
        if (systemPrompt) {
          apiMessages.push({ role: "system", content: systemPrompt });
        }

        // History + new message
        for (const m of [...(conv?.messages || []), userMsg]) {
          if (m.role === "assistant" && m.content === "") continue;
          if (m.role === "system") continue;

          if (m.attachments && m.attachments.length > 0) {
            const hasImages = m.attachments.some((a) => a.type.startsWith("image/"));
            // Build document text from extracted attachments
            const docTexts: string[] = [];
            for (const att of m.attachments) {
              if (!att.type.startsWith("image/") && att.extractedText) {
                docTexts.push(`\`\`\`${att.name}\n${att.extractedText}\n\`\`\``);
              }
            }
            const fullText = docTexts.length > 0
              ? `${m.content}\n\n${docTexts.join("\n\n")}`
              : m.content;

            if (hasImages) {
              // Vision message — send as Parts array to trigger vision routing
              const contentParts: any[] = [{ type: "text", text: fullText }];
              for (const att of m.attachments) {
                if (att.type.startsWith("image/")) {
                  contentParts.push({ type: "image_url", image_url: { url: att.dataUrl } });
                }
              }
              apiMessages.push({ role: m.role, content: contentParts });
            } else {
              // Document-only — send as plain text (routes to any model)
              apiMessages.push({ role: m.role, content: fullText });
            }
          } else {
            apiMessages.push({ role: m.role, content: m.content });
          }
        }

        const startTime = Date.now();

        const response = await fetch(`${API_BASE}/v1/chat/completions`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            model: selectedModel || conv?.model || "auto",
            messages: apiMessages,
            stream: true,
          }),
          signal: controller.signal,
        });

        // Extract routing headers (may be empty due to CORS; overridden by SSE metadata)
        let routingProvider = response.headers.get("x-coalesce-provider") || "";
        let routingModel = response.headers.get("x-coalesce-model") || "";
        let routingTier = response.headers.get("x-coalesce-tier") || "";
        let routingAttempt = parseInt(response.headers.get("x-coalesce-attempt") || "1");

        if (!response.ok) {
          const err = await response.text();
          const latencyMs = Date.now() - startTime;

          // Try to extract provider from error message (e.g. "Provider error: openrouter - ...")
          let errorProvider = routingProvider || "unknown";
          let errorAttempt = routingAttempt;
          try {
            const errJson = JSON.parse(err);
            const msg = errJson?.error?.message || "";
            const provMatch = msg.match(/Provider error:\s*(\S+)\s*-/);
            if (provMatch) errorProvider = provMatch[1];
            const attemptMatch = msg.match(/after (\d+) attempts/);
            if (attemptMatch) errorAttempt = parseInt(attemptMatch[1]);
          } catch { /* not JSON */ }

          const errorRouting: RoutingPath = {
            tier: routingTier || "Unknown",
            score: 0,
            provider: errorProvider,
            model: routingModel || conv?.model || "auto",
            attempt: errorAttempt,
            costUsd: 0,
            latencyMs,
            inputTokens: 0,
            outputTokens: 0,
            error: true,
          };
          updateConversation(convId!, (c) => ({
            ...c,
            messages: c.messages.map((m) =>
              m.id === assistantMsg.id
                ? { ...m, content: `Error: ${response.status} — ${err}`, routing: errorRouting }
                : m
            ),
          }));
          setSelectedRouting(errorRouting);
          setStreaming(false);
          setAbortController(null);
          return;
        }

        // Stream SSE response
        const reader = response.body?.getReader();
        const decoder = new TextDecoder();
        let fullContent = "";
        let buffer = "";
        let inputTokens = 0;
        let outputTokens = 0;
        let cacheReadTokens = 0;
        let finishReason: string | undefined;
        let costUsd = 0;
        let routingScore = 0;

        while (reader) {
          const { done, value } = await reader.read();
          if (done) break;

          buffer += decoder.decode(value, { stream: true });
          const lines = buffer.split("\n");
          buffer = lines.pop() || "";

          for (const line of lines) {
            if (!line.startsWith("data:")) continue;
            const data = line.slice(line.startsWith("data: ") ? 6 : 5).trim();
            if (data === "[DONE]") continue;

            try {
              const chunk = JSON.parse(data);

              // Extract routing metadata from SSE metadata chunk
              if (chunk.x_coalesce) {
                routingScore = chunk.x_coalesce.score || routingScore;
                costUsd = chunk.x_coalesce.cost_usd || costUsd;
                // Override header values with in-band data (more reliable than CORS headers)
                if (chunk.x_coalesce.tier) routingTier = chunk.x_coalesce.tier;
                if (chunk.x_coalesce.provider) routingProvider = chunk.x_coalesce.provider;
                if (chunk.x_coalesce.model) routingModel = chunk.x_coalesce.model;
                if (chunk.x_coalesce.attempt) routingAttempt = chunk.x_coalesce.attempt;
              }

              if (chunk.usage) {
                inputTokens = chunk.usage.prompt_tokens || 0;
                outputTokens = chunk.usage.completion_tokens || 0;
                // Anthropic (proxied) surfaces cache hits here.
                cacheReadTokens =
                  chunk.usage.cache_read_input_tokens ||
                  chunk.usage.prompt_tokens_details?.cached_tokens ||
                  0;
              }
              if (chunk.choices?.[0]?.finish_reason) {
                finishReason = chunk.choices[0].finish_reason;
              }

              const delta = chunk.choices?.[0]?.delta;
              if (delta?.content) {
                fullContent += delta.content;

                // Update message content
                updateConversation(convId!, (c) => ({
                  ...c,
                  messages: c.messages.map((m) =>
                    m.id === assistantMsg.id
                      ? { ...m, content: fullContent }
                      : m
                  ),
                }));
              }
            } catch {
              // Skip malformed chunks
            }
          }
        }

        const latencyMs = Date.now() - startTime;

        // Build routing path
        const routing: RoutingPath = {
          tier: routingTier || "Unknown",
          score: routingScore,
          provider: routingProvider || "unknown",
          model: routingModel || conv?.model || "auto",
          attempt: routingAttempt,
          costUsd,
          latencyMs,
          inputTokens,
          outputTokens,
          cacheReadTokens,
          finishReason,
        };

        // Final update with routing data
        updateConversation(convId!, (c) => ({
          ...c,
          messages: c.messages.map((m) =>
            m.id === assistantMsg.id
              ? { ...m, content: fullContent, routing, timestamp: Date.now() }
              : m
          ),
          updatedAt: Date.now(),
        }));

        setSelectedRouting(routing);
      } catch (err: any) {
        if (err.name !== "AbortError") {
          updateConversation(convId!, (c) => ({
            ...c,
            messages: c.messages.map((m) =>
              m.id === assistantMsg.id
                ? { ...m, content: `Error: ${err.message}` }
                : m
            ),
          }));
        }
      } finally {
        setStreaming(false);
        setAbortController(null);
      }
    },
    [input, attachments, active, activeId, streaming, updateConversation, selectedModel]
  );

  const stopGeneration = useCallback(() => {
    abortController?.abort();
    setStreaming(false);
    setAbortController(null);
  }, [abortController]);

  // ─── Edit & Resend ──────────────────────────────────────

  const startEdit = useCallback((msg: ChatMessage) => {
    setEditingMessageId(msg.id);
    setEditContent(msg.content);
  }, []);

  const submitEdit = useCallback(
    (msgId: string) => {
      if (!active || !editContent.trim()) return;

      // Find message index, truncate conversation there, and resend
      const idx = active.messages.findIndex((m) => m.id === msgId);
      if (idx === -1) return;

      const trimmed = active.messages.slice(0, idx);
      updateConversation(active.id, (c) => ({
        ...c,
        messages: trimmed,
      }));

      setEditingMessageId(null);
      // Send with edited content after state update
      setTimeout(() => sendMessage(editContent.trim()), 50);
    },
    [active, editContent, updateConversation, sendMessage]
  );

  const regenerate = useCallback(
    (msgId: string) => {
      if (!active) return;

      const idx = active.messages.findIndex((m) => m.id === msgId);
      if (idx === -1) return;

      // Find the user message before this assistant message
      const userMsg = active.messages
        .slice(0, idx)
        .reverse()
        .find((m) => m.role === "user");
      if (!userMsg) return;

      // Truncate from the assistant message onward
      const trimmed = active.messages.slice(0, idx);
      updateConversation(active.id, (c) => ({
        ...c,
        messages: trimmed,
      }));

      setTimeout(() => sendMessage(userMsg.content), 50);
    },
    [active, updateConversation, sendMessage]
  );

  // ─── 19.6 A/B send ───────────────────────────────────────
  // Forks the current conversation (or creates a new one) into two
  // siblings, one per model, and sends the same prompt to both.
  // Runtime-only — not persisted beyond the forks themselves.
  const sendAB = useCallback(
    async (content: string) => {
      if (!content.trim()) return;
      const source = active ?? newConversation(selectedModel);
      if (!active) {
        setConversations((prev) => [source, ...prev]);
      }
      const aFork = forkFrom(source, null, {
        modelOverride: selectedModel,
        switchTo: true,
      });
      updateConversation(aFork.id, (c) => ({ ...c, title: c.title + " [A]" }));
      const bFork = forkFrom(source, null, {
        modelOverride: abModelB,
        switchTo: false,
      });
      updateConversation(bFork.id, (c) => ({ ...c, title: c.title + " [B]" }));
      setAbViewIds([aFork.id, bFork.id]);
      setTimeout(() => sendMessage(content), 0);
      // TODO(19.6): truly parallel send to B. For now we queue the
      // second send after a short delay so it runs against bFork
      // once the first request's state settles.
      setTimeout(() => {
        setActiveId(bFork.id);
        setSelectedModel(abModelB);
        setTimeout(() => sendMessage(content), 0);
      }, 100);
    },
    [active, selectedModel, abModelB, forkFrom, sendMessage, updateConversation],
  );

  // ─── 19.4 Slash command dispatcher ──────────────────────

  const handleSend = useCallback(() => {
    const raw = input;
    if (!raw.trim()) {
      sendMessage();
      return;
    }
    if (abMode) {
      const content = raw;
      setInput("");
      sendAB(content);
      return;
    }
    const action = parseSlash(raw);
    switch (action.kind) {
      case "none":
        sendMessage();
        return;
      case "model": {
        if (!active) {
          setSelectedModel(action.model);
        } else {
          updateConversation(active.id, (c) => ({ ...c, model: action.model }));
        }
        setInput("");
        return;
      }
      case "system": {
        if (active) {
          updateConversation(active.id, (c) => ({ ...c, systemPrompt: action.prompt }));
        }
        setInput("");
        return;
      }
      case "clear": {
        if (active) {
          updateConversation(active.id, (c) => ({ ...c, messages: [] }));
        }
        setInput("");
        return;
      }
      case "branch": {
        if (active) forkFrom(active, null);
        setInput("");
        return;
      }
      case "regen": {
        if (active) {
          const lastAssistant = [...active.messages].reverse().find((m) => m.role === "assistant");
          if (lastAssistant) regenerate(lastAssistant.id);
        }
        setInput("");
        return;
      }
      case "unknown": {
        // Surface as a harmless inline notice in the input.
        setInput(`# unknown command: ${action.name}\n` + raw);
        return;
      }
    }
  }, [input, active, regenerate, sendMessage, updateConversation, forkFrom, abMode, sendAB]);

  // ─── 19.4 Mic / audio transcription ─────────────────────

  const stopRecording = useCallback(() => {
    const rec = mediaRecorderRef.current;
    if (rec && rec.state !== "inactive") rec.stop();
    const stream = mediaStreamRef.current;
    if (stream) stream.getTracks().forEach((t) => t.stop());
    mediaStreamRef.current = null;
    setRecording(false);
  }, []);

  const toggleMic = useCallback(async () => {
    if (recording) {
      stopRecording();
      return;
    }
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      mediaStreamRef.current = stream;
      const rec = new MediaRecorder(stream);
      mediaChunksRef.current = [];
      rec.ondataavailable = (e) => {
        if (e.data.size > 0) mediaChunksRef.current.push(e.data);
      };
      rec.onstop = async () => {
        const blob = new Blob(mediaChunksRef.current, { type: "audio/webm" });
        mediaChunksRef.current = [];
        if (blob.size === 0) return;
        setTranscribing(true);
        try {
          const text = await api.transcribeAudio(blob);
          if (text) {
            setInput((prev) => (prev ? prev + " " + text : text));
          }
        } catch (err) {
          console.error("transcription failed", err);
        } finally {
          setTranscribing(false);
        }
      };
      mediaRecorderRef.current = rec;
      rec.start();
      setRecording(true);
    } catch (err) {
      console.error("mic access denied", err);
      setRecording(false);
    }
  }, [recording, stopRecording]);

  // ─── Key handler ─────────────────────────────────────────

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        handleSend();
      }
    },
    [handleSend]
  );

  // ─── Render ──────────────────────────────────────────────

  return (
    <div
      className="chat-terminal chat-terminal--scanlines flex h-full overflow-hidden flex-col"
      onDrop={handleDrop}
      onDragOver={(e) => e.preventDefault()}
    >
      <div className="chat-terminal__chrome">
        <span className="chat-terminal__dot chat-terminal__dot--red" />
        <span className="chat-terminal__dot chat-terminal__dot--yellow" />
        <span className="chat-terminal__dot chat-terminal__dot--green" />
        <span className="chat-terminal__title">
          coalesce — {active?.model || selectedModel || "chat"}
          {contextFill ? (
            <>
              {" · "}
              <span title={`${contextFill.used} / ${contextFill.max} tokens`}>
                {formatTokens(contextFill.used)}/{formatTokens(contextFill.max)}
                {" "}
                ({Math.min(100, Math.round((contextFill.used / contextFill.max) * 100))}%)
              </span>
            </>
          ) : null}
        </span>
      </div>
      <div className="flex flex-1 overflow-hidden">
      {/* ─── Left: Conversation History ──────────────── */}
      <div
        className="flex flex-col border-r border-themed bg-surface"
        style={{ width: sidebarWidth, minWidth: sidebarWidth }}
      >
        {/* New chat button */}
        <div className="p-2">
          <button
            onClick={createNewChat}
            className="w-full px-3 py-2 text-sm font-medium rounded-lg btn-primary"
          >
            + New Chat
          </button>
        </div>

        {/* Search */}
        <div className="px-2 pb-2">
          <input
            type="text"
            placeholder="Search chats..."
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            className="w-full px-2 py-1.5 text-xs rounded-md bg-surface-alt border border-themed text-primary placeholder:text-secondary focus:outline-none focus:ring-1 focus:ring-brand-500"
          />
        </div>

        {/* Conversation list */}
        <div className="flex-1 overflow-y-auto px-1 space-y-0.5">
          {filteredConversations.map((conv) => (
            <div
              key={conv.id}
              className={`group flex items-center gap-1 px-2 py-2 rounded-md cursor-pointer text-xs transition-colors ${
                conv.id === activeId
                  ? "bg-brand-600/10 text-brand-400"
                  : "text-secondary hover:bg-hover hover:text-primary"
              }`}
              onClick={() => {
                setActiveId(conv.id);
                setSelectedModel(conv.model || "auto");
                const lastRouting = [...conv.messages]
                  .reverse()
                  .find((m) => m.routing)?.routing;
                setSelectedRouting(lastRouting || null);
              }}
            >
              {conv.parentId && (
                <span className="opacity-60" title="Forked conversation">⑂</span>
              )}
              <span className="flex-1 truncate">{conv.title}</span>
              <button
                className="opacity-0 group-hover:opacity-100 p-0.5 hover:text-red-400 transition-opacity"
                onClick={(e) => {
                  e.stopPropagation();
                  deleteConversation(conv.id);
                }}
                title="Delete"
              >
                ×
              </button>
            </div>
          ))}

          {filteredConversations.length === 0 && (
            <p className="text-xs text-secondary px-2 py-4 text-center">
              {searchQuery ? "No matching chats" : "No conversations yet"}
            </p>
          )}
        </div>
      </div>

      {/* ─── Center: Chat Area ───────────────────────── */}
      <div className="flex-1 flex flex-col min-w-0">
        {/* Top bar: provider + model selectors + settings */}
        <div className="flex items-center gap-2 px-4 py-2 border-b border-themed bg-surface">
          {/* Provider dropdown */}
          <select
            value={selectedProvider}
            onChange={(e) => {
              const prov = e.target.value;
              setSelectedProvider(prov);
              setSelectedModel("auto");
              if (active) {
                updateConversation(active.id, (c) => ({
                  ...c,
                  model: "auto",
                }));
              }
            }}
            className="px-2 py-1.5 text-sm rounded-md bg-surface-alt border border-themed text-primary focus:outline-none focus:ring-1 focus:ring-brand-500"
          >
            <option value="any">Any Provider</option>
            {providers.map((p) => (
              <option key={p} value={p}>
                {p}
              </option>
            ))}
          </select>

          {/* Model dropdown (filtered by provider) */}
          <select
            key={`model-select-${selectedProvider}`}
            value={selectedModel}
            onChange={(e) => {
              const model = e.target.value;
              setSelectedModel(model);
              if (active) {
                updateConversation(active.id, (c) => ({
                  ...c,
                  model,
                }));
              }
            }}
            className="flex-1 max-w-xs px-2 py-1.5 text-sm rounded-md bg-surface-alt border border-themed text-primary focus:outline-none focus:ring-1 focus:ring-brand-500"
          >
            <option value="auto">auto (smart routing)</option>
            {filteredModels.map((m) => (
              <option key={`${m.provider}:${m.id}`} value={m.id}>
                {m.id}{selectedProvider === "any" ? ` (${m.provider})` : ""}
              </option>
            ))}
          </select>

          <button
            onClick={() => {
              setAbMode((v) => !v);
              if (abMode) {
                setAbViewIds(null);
              }
            }}
            className={`px-2 py-1.5 text-xs rounded-md border border-themed transition-colors ${
              abMode
                ? "bg-brand-600/20 text-brand-300"
                : "bg-surface-alt text-secondary hover:text-primary"
            }`}
            title="Toggle A/B compare mode"
          >
            A/B
          </button>

          {abMode && (
            <select
              value={abModelB}
              onChange={(e) => setAbModelB(e.target.value)}
              className="max-w-[160px] px-2 py-1.5 text-xs rounded-md bg-surface-alt border border-themed text-primary focus:outline-none focus:ring-1 focus:ring-brand-500"
              title="Model B"
            >
              <option value="auto">B: auto</option>
              {filteredModels.map((m) => (
                <option key={`b:${m.provider}:${m.id}`} value={m.id}>
                  B: {m.id}
                </option>
              ))}
            </select>
          )}

          <button
            onClick={() => {
              if (active) setShowSettings(!showSettings);
            }}
            disabled={!active}
            className={`px-2 py-1.5 text-xs rounded-md bg-surface-alt border border-themed transition-colors ${
              active
                ? "text-secondary hover:text-primary cursor-pointer"
                : "text-secondary/40 cursor-not-allowed"
            }`}
          >
            {showSettings && active ? "Hide Settings" : "Settings"}
          </button>

          {active && (
            <button
              onClick={() => exportConversation(active)}
              className="px-2 py-1.5 text-xs rounded-md bg-surface-alt border border-themed text-secondary hover:text-primary transition-colors"
              title="Export as Markdown"
            >
              Export
            </button>
          )}
        </div>

        {/* Settings panel (collapsible) */}
        {showSettings && active && (
          <div className="px-4 py-3 border-b border-themed bg-surface-alt space-y-2">
            <div>
              <label className="text-xs text-secondary block mb-1">
                System Prompt
              </label>
              <textarea
                value={active.systemPrompt}
                onChange={(e) =>
                  updateConversation(active.id, (c) => ({
                    ...c,
                    systemPrompt: e.target.value,
                  }))
                }
                placeholder="You are a helpful assistant..."
                className="w-full px-2 py-1.5 text-sm rounded-md bg-surface border border-themed text-primary placeholder:text-secondary focus:outline-none focus:ring-1 focus:ring-brand-500 resize-y"
                rows={2}
              />
            </div>
            <div>
              <label className="text-xs text-secondary block mb-1">
                Conversation Title
              </label>
              <input
                type="text"
                value={active.title}
                onChange={(e) =>
                  renameConversation(active.id, e.target.value)
                }
                className="w-full px-2 py-1.5 text-sm rounded-md bg-surface border border-themed text-primary focus:outline-none focus:ring-1 focus:ring-brand-500"
              />
            </div>
          </div>
        )}

        {/* Messages */}
        <div className="flex-1 overflow-y-auto px-4 py-4 space-y-4">
          {!active && (
            <div className="flex flex-col items-center justify-center h-full text-secondary">
              <div className="text-4xl mb-4">💬</div>
              <p className="text-lg font-medium">Welcome to Coalesce Chat</p>
              <p className="text-sm mt-1">
                Start a new conversation or select one from the sidebar
              </p>
              <button
                onClick={createNewChat}
                className="mt-4 px-4 py-2 text-sm rounded-lg btn-primary"
              >
                New Chat
              </button>
            </div>
          )}

          {abViewIds && (
            <div className="flex gap-2 h-full overflow-hidden">
              {abViewIds.map((cid, side) => {
                const conv = conversations.find((c) => c.id === cid);
                if (!conv) return null;
                return (
                  <div
                    key={cid}
                    className={`flex-1 min-w-0 border border-themed rounded-lg p-3 overflow-y-auto ${
                      activeId === cid ? "border-brand-500/60" : ""
                    }`}
                  >
                    <div className="text-[10px] uppercase tracking-wider opacity-60 mb-2 flex items-center gap-2">
                      <span>{side === 0 ? "A" : "B"}</span>
                      <span className="truncate">{conv.model}</span>
                      <button
                        className="ml-auto text-secondary hover:text-primary"
                        onClick={() => setActiveId(cid)}
                        title="Make active"
                      >
                        focus
                      </button>
                    </div>
                    <div className="space-y-3">
                      {conv.messages.map((m) => (
                        <div
                          key={m.id}
                          className={`text-sm ${
                            m.role === "user"
                              ? "text-secondary"
                              : "text-primary"
                          }`}
                        >
                          <div className="text-[10px] opacity-50 mb-1">
                            {m.role === "user" ? "user@coalesce:~$" : "assistant >"}
                          </div>
                          <BlockRenderer
                            blocks={parseBlocks(m.content || "")}
                            mdComponents={mdComponents}
                            streaming={streaming && activeId === cid}
                          />
                        </div>
                      ))}
                    </div>
                  </div>
                );
              })}
            </div>
          )}

          {!abViewIds && active?.messages.map((msg) => (
            <div
              key={msg.id}
              className={`group flex gap-3 ${
                msg.role === "user" ? "justify-end" : "justify-start"
              }`}
            >
              <div
                className={`max-w-[80%] rounded-xl px-4 py-3 ${
                  msg.role === "user"
                    ? "bg-brand-600/20 border border-brand-500/10 text-primary"
                    : "bg-surface-alt border border-themed-faint text-primary"
                }`}
              >
                {/* Attachments */}
                {msg.attachments && msg.attachments.length > 0 && (
                  <div className="flex flex-wrap gap-2 mb-2">
                    {msg.attachments.map((att, i) =>
                      att.type.startsWith("image/") ? (
                        <img
                          key={i}
                          src={att.dataUrl}
                          alt={att.name}
                          className="max-h-40 rounded-lg border border-themed"
                        />
                      ) : (
                        <div
                          key={i}
                          className="inline-flex items-center gap-1.5 text-xs px-2.5 py-1.5 rounded-lg bg-surface border border-themed text-secondary"
                        >
                          <span>📄</span>
                          <span className="font-medium text-primary">{att.name}</span>
                        </div>
                      )
                    )}
                  </div>
                )}

                {/* Content */}
                {editingMessageId === msg.id ? (
                  <div className="space-y-2">
                    <textarea
                      value={editContent}
                      onChange={(e) => setEditContent(e.target.value)}
                      className="w-full px-2 py-1.5 text-sm rounded-md bg-surface border border-themed text-primary focus:outline-none resize-y"
                      rows={3}
                      autoFocus
                    />
                    <div className="flex gap-2">
                      <button
                        onClick={() => submitEdit(msg.id)}
                        className="px-2 py-1 text-xs rounded btn-primary"
                      >
                        Send
                      </button>
                      <button
                        onClick={() => setEditingMessageId(null)}
                        className="px-2 py-1 text-xs rounded bg-surface border border-themed text-secondary"
                      >
                        Cancel
                      </button>
                    </div>
                  </div>
                ) : (
                  <BlockRenderer
                    blocks={parseBlocks(msg.content || (streaming ? "▍" : ""))}
                    mdComponents={mdComponents}
                    streaming={streaming}
                    onChoose={
                      msg.role === "assistant" && !streaming
                        ? (choice) => sendMessage(choice)
                        : undefined
                    }
                  />
                )}

                {/* Message footer: routing badge + actions */}
                <div className="flex items-center gap-2 mt-2 pt-1 border-t border-white/5">
                  {msg.routing && (
                    <button
                      className="flex items-center gap-1 text-[10px] text-secondary hover:text-primary transition-colors"
                      onClick={() => setSelectedRouting(msg.routing!)}
                      title="View routing path"
                    >
                      <span
                        className="inline-block w-2 h-2 rounded-full"
                        style={{
                          backgroundColor:
                            msg.routing.tier === "Simple"
                              ? "#38bdf8"
                              : msg.routing.tier === "Medium"
                              ? "#fbbf24"
                              : msg.routing.tier === "Complex"
                              ? "#a78bfa"
                              : "#f472b6",
                        }}
                      />
                      {msg.routing.provider}/{msg.routing.model}
                      {msg.routing.costUsd > 0
                        ? ` · $${msg.routing.costUsd.toFixed(4)}`
                        : " · FREE"}
                      {msg.routing.inputTokens
                        ? ` · ${formatTokens(msg.routing.inputTokens)}→${formatTokens(
                            msg.routing.outputTokens || 0
                          )}`
                        : ""}
                    </button>
                  )}

                  {msg.routing && (msg.routing.cacheReadTokens ?? 0) > 0 && (
                    <span
                      className="text-[10px] px-1.5 py-0.5 rounded border border-emerald-500/40 text-emerald-400"
                      title={`${msg.routing.cacheReadTokens} prompt tokens served from cache`}
                    >
                      cached ✓
                    </span>
                  )}

                  <div className="flex-1" />

                  {/* Actions */}
                  {msg.role === "user" && !streaming && (
                    <>
                      <button
                        onClick={() => startEdit(msg)}
                        className="text-[10px] text-secondary hover:text-primary transition-colors"
                        title="Edit & resend"
                      >
                        Edit
                      </button>
                      <select
                        value=""
                        onChange={(e) => {
                          const m = e.target.value;
                          if (!m || !active) return;
                          // Fork at the message *before* this user turn
                          // so the fork contains the prior context, then
                          // re-send this content against the new model.
                          const idx = active.messages.findIndex((x) => x.id === msg.id);
                          const priorId =
                            idx > 0 ? active.messages[idx - 1].id : null;
                          forkFrom(active, priorId, { modelOverride: m });
                          const content = msg.content;
                          setTimeout(() => sendMessage(content), 0);
                          e.currentTarget.value = "";
                        }}
                        className="text-[10px] bg-transparent text-secondary hover:text-primary border border-themed rounded px-1 py-0.5"
                        title="Fork and re-send with a different model"
                      >
                        <option value="">⟳ model…</option>
                        {filteredModels.slice(0, 40).map((m) => (
                          <option key={m.id} value={m.id}>
                            {m.provider}/{m.id}
                          </option>
                        ))}
                      </select>
                    </>
                  )}
                  {!streaming && active && (
                    <button
                      onClick={() => forkFrom(active, msg.id)}
                      className="text-[10px] text-secondary hover:text-primary transition-colors"
                      title="Fork conversation from this turn"
                    >
                      ⑂ Fork
                    </button>
                  )}
                  {msg.role === "assistant" &&
                    msg.content &&
                    !streaming && (
                      <>
                        <button
                          onClick={() =>
                            navigator.clipboard.writeText(msg.content)
                          }
                          className="text-[10px] text-secondary hover:text-primary transition-colors"
                          title="Copy response"
                        >
                          Copy
                        </button>
                        <button
                          onClick={() => regenerate(msg.id)}
                          className="text-[10px] text-secondary hover:text-primary transition-colors"
                          title="Regenerate"
                        >
                          Retry
                        </button>
                        {msg.routing?.finishReason === "length" && (
                          <button
                            onClick={() => sendMessage("continue")}
                            className="text-[10px] text-amber-400 hover:text-amber-300 transition-colors"
                            title="Response was truncated — continue generating"
                          >
                            Continue ⏵
                          </button>
                        )}
                        <button
                          onClick={async () => {
                            try {
                              const blob = await api.synthesizeSpeech(msg.content);
                              const url = URL.createObjectURL(blob);
                              const audio = new Audio(url);
                              audio.onended = () => URL.revokeObjectURL(url);
                              await audio.play();
                            } catch (err) {
                              console.error("tts failed", err);
                            }
                          }}
                          className="text-[10px] text-secondary hover:text-primary transition-colors"
                          title="Play with TTS"
                        >
                          ♪
                        </button>
                        {msg.routing && (
                          <>
                            <span className="text-secondary/30">|</span>
                            <button
                              onClick={() => {
                                api.submitFeedback(msg.routing!.provider, msg.routing!.model, 1.0);
                                setConversations((prev: Conversation[]) => prev.map((c: Conversation) => c.id === active?.id ? { ...c, messages: c.messages.map((m: ChatMessage) => m.id === msg.id ? { ...m, rated: "up" } : m) } : c));
                              }}
                              className={`text-[10px] transition-colors ${msg.rated === "up" ? "text-emerald-400" : "text-secondary hover:text-emerald-400"}`}
                              title="Good response"
                            >
                              {msg.rated === "up" ? "\u{1F44D}" : "\u25B2"}
                            </button>
                            <button
                              onClick={() => {
                                api.submitFeedback(msg.routing!.provider, msg.routing!.model, 0.0);
                                setConversations((prev: Conversation[]) => prev.map((c: Conversation) => c.id === active?.id ? { ...c, messages: c.messages.map((m: ChatMessage) => m.id === msg.id ? { ...m, rated: "down" } : m) } : c));
                              }}
                              className={`text-[10px] transition-colors ${msg.rated === "down" ? "text-red-400" : "text-secondary hover:text-red-400"}`}
                              title="Poor response"
                            >
                              {msg.rated === "down" ? "\u{1F44E}" : "\u25BC"}
                            </button>
                          </>
                        )}
                      </>
                    )}
                </div>
              </div>
            </div>
          ))}

          {streaming && (
            <div className="flex items-center gap-2 text-xs text-secondary">
              <span className="animate-pulse">●</span> Generating...
            </div>
          )}

          <div ref={chatEndRef} />
        </div>

        {/* Attachments preview */}
        {attachments.length > 0 && (
          <div className="px-4 py-2 border-t border-themed flex flex-wrap gap-2">
            {attachments.map((att, i) => (
              <div
                key={i}
                className={`flex items-center gap-1 text-xs px-2 py-1 rounded-md border border-themed ${att.error ? "bg-red-500/10 border-red-500/30" : "bg-surface-alt"}`}
              >
                {att.type.startsWith("image/") ? (
                  <img
                    src={att.dataUrl}
                    alt={att.name}
                    className="w-16 h-16 rounded object-cover border border-themed"
                    title={att.name}
                  />
                ) : att.parsing ? (
                  <span className="animate-spin">⏳</span>
                ) : att.error ? (
                  <span title={att.error}>❌</span>
                ) : (
                  <span>📄</span>
                )}
                <span className="max-w-[120px] truncate">{att.name}</span>
                <button
                  onClick={() => removeAttachment(i)}
                  className="text-secondary hover:text-red-400 ml-1"
                >
                  ×
                </button>
              </div>
            ))}
          </div>
        )}

        {/* Input area */}
        <div className="px-4 py-3 border-t border-themed bg-surface">
          <div className="flex items-end gap-2">
            <input
              type="file"
              ref={fileInputRef}
              className="hidden"
              multiple
              accept="image/*,.txt,.pdf,.docx,.md,.json,.csv,.html"
              onChange={(e) => handleFileUpload(e.target.files)}
            />

            <button
              onClick={() => fileInputRef.current?.click()}
              className="px-2 py-2 text-sm rounded-md bg-surface-alt border border-themed text-secondary hover:text-primary transition-colors"
              title="Upload file"
            >
              📎
            </button>

            <button
              onClick={toggleMic}
              disabled={transcribing}
              className={`px-2 py-2 text-sm rounded-md border border-themed transition-colors ${
                recording
                  ? "bg-red-600/80 text-white animate-pulse"
                  : "bg-surface-alt text-secondary hover:text-primary"
              } disabled:opacity-40`}
              title={
                recording
                  ? "Stop recording"
                  : transcribing
                  ? "Transcribing…"
                  : "Record voice message"
              }
            >
              {transcribing ? "⏳" : recording ? "⏹" : "🎤"}
            </button>

            <textarea
              ref={inputRef}
              value={input}
              onChange={(e) => setInput(e.target.value)}
              onKeyDown={handleKeyDown}
              onPaste={handlePaste}
              placeholder={
                active
                  ? "Type a message... (Shift+Enter for newline)"
                  : "Start a new chat..."
              }
              className="flex-1 px-3 py-2 text-sm rounded-lg bg-surface-alt border border-themed text-primary placeholder:text-secondary focus:outline-none focus:ring-1 focus:ring-brand-500 resize-none"
              rows={1}
              disabled={!active && conversations.length > 0 && !activeId}
              onFocus={() => {
                if (!active) createNewChat();
              }}
            />

            {streaming ? (
              <button
                onClick={stopGeneration}
                className="px-4 py-2 text-sm rounded-lg bg-red-600/80 text-white hover:bg-red-600 transition-colors"
              >
                Stop
              </button>
            ) : (
              <button
                onClick={handleSend}
                disabled={(!input.trim() && attachments.length === 0) || attachments.some(a => a.parsing)}
                className="px-4 py-2 text-sm rounded-lg btn-primary disabled:opacity-40 disabled:cursor-not-allowed"
              >
                Send
              </button>
            )}
          </div>
        </div>
      </div>

      {/* ─── Right: Routing Visualization ────────────── */}
      <div
        className="border-l border-themed bg-surface flex flex-col"
        style={{ width: vizWidth, minWidth: vizWidth }}
      >
        <RoutingViz path={selectedRouting} />
      </div>
      </div>
    </div>
  );
}
