import { useState, useEffect, useRef, useCallback } from "react";

interface EventEntry {
  id: number;
  type: string;
  timestamp: string;
  data: Record<string, unknown>;
}

const EVENT_COLORS: Record<string, string> = {
  request: "text-sky-400",
  response: "text-emerald-400",
  error: "text-red-400",
  routing: "text-violet-400",
  fallback: "text-amber-400",
  quota: "text-orange-400",
  circuit_breaker: "text-rose-400",
};

const EVENT_BG: Record<string, string> = {
  request: "bg-sky-500/10",
  response: "bg-emerald-500/10",
  error: "bg-red-500/10",
  routing: "bg-violet-500/10",
  fallback: "bg-amber-500/10",
  quota: "bg-orange-500/10",
  circuit_breaker: "bg-rose-500/10",
};

export default function EventStream() {
  const [events, setEvents] = useState<EventEntry[]>([]);
  const [connected, setConnected] = useState(false);
  const [paused, setPaused] = useState(false);
  const idCounter = useRef(0);
  const eventSourceRef = useRef<EventSource | null>(null);

  const connect = useCallback(() => {
    if (eventSourceRef.current) {
      eventSourceRef.current.close();
    }

    const es = new EventSource("http://127.0.0.1:8402/api/v1/events");
    eventSourceRef.current = es;

    es.onopen = () => setConnected(true);

    es.onmessage = (event) => {
      try {
        const parsed = JSON.parse(event.data);
        const entry: EventEntry = {
          id: ++idCounter.current,
          type: parsed.type ?? "unknown",
          timestamp: parsed.timestamp ?? new Date().toISOString(),
          data: parsed,
        };
        setEvents((prev) => [entry, ...prev].slice(0, 100));
      } catch {
        // skip malformed events
      }
    };

    es.onerror = () => {
      setConnected(false);
      es.close();
      // auto-reconnect after 3s
      setTimeout(connect, 3000);
    };
  }, []);

  useEffect(() => {
    connect();
    return () => {
      eventSourceRef.current?.close();
    };
  }, [connect]);

  const clearEvents = () => setEvents([]);

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <h2 className="text-xl font-semibold">Live Events</h2>
        <div className="flex items-center gap-3">
          <div className="flex items-center gap-2">
            <div
              className={`h-2.5 w-2.5 rounded-full ${
                connected ? "bg-emerald-500" : "bg-red-500"
              }`}
            />
            <span className="text-xs text-secondary">
              {connected ? "Connected" : "Disconnected"}
            </span>
          </div>
          <button
            onClick={() => setPaused((p) => !p)}
            className="text-xs px-3 py-1 rounded border border-themed hover:bg-hover transition-colors"
          >
            {paused ? "Resume" : "Pause"}
          </button>
          <button
            onClick={clearEvents}
            className="text-xs px-3 py-1 rounded border border-themed hover:bg-hover transition-colors"
          >
            Clear
          </button>
        </div>
      </div>

      {events.length === 0 ? (
        <div className="card text-center py-12 text-secondary">
          <p>Waiting for events...</p>
          <p className="text-xs mt-2">
            Events will appear here as requests flow through the proxy.
          </p>
        </div>
      ) : (
        <div className="space-y-1.5">
          {events.map((evt) => {
            const color = EVENT_COLORS[evt.type] ?? "text-secondary";
            const bg = EVENT_BG[evt.type] ?? "bg-tertiary";
            const time = new Date(evt.timestamp).toLocaleTimeString();
            return (
              <div
                key={evt.id}
                className={`card flex items-start gap-3 py-2 px-3 ${bg}`}
              >
                <span
                  className={`text-xs font-mono font-bold uppercase min-w-[90px] ${color}`}
                >
                  {evt.type}
                </span>
                <span className="text-xs text-secondary min-w-[70px]">
                  {time}
                </span>
                <span className="text-xs text-primary truncate flex-1">
                  {summarizeEvent(evt)}
                </span>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

function summarizeEvent(evt: EventEntry): string {
  const d = evt.data;
  if (d.provider && d.model) return `${d.provider}/${d.model}`;
  if (d.message) return String(d.message);
  if (d.tier) return `Tier: ${d.tier}`;
  const keys = Object.keys(d).filter((k) => k !== "type" && k !== "timestamp");
  if (keys.length === 0) return "-";
  return keys.map((k) => `${k}=${JSON.stringify(d[k])}`).join(" ");
}
