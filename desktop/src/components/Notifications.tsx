import { useState, useEffect, useCallback, createContext, useContext, type ReactNode } from "react";

interface Toast {
  id: number;
  message: string;
  type: "info" | "success" | "warning" | "error";
  timestamp: number;
}

interface NotificationContextValue {
  toasts: Toast[];
  addToast: (message: string, type?: Toast["type"]) => void;
  dismissToast: (id: number) => void;
}

const NotificationContext = createContext<NotificationContextValue>({
  toasts: [],
  addToast: () => {},
  dismissToast: () => {},
});

export const useNotifications = () => useContext(NotificationContext);

let nextId = 1;

export function NotificationProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);

  const addToast = useCallback((message: string, type: Toast["type"] = "info") => {
    const id = nextId++;
    setToasts((prev) => [...prev, { id, message, type, timestamp: Date.now() }].slice(-5));
    // Auto-dismiss after 6s
    setTimeout(() => {
      setToasts((prev) => prev.filter((t) => t.id !== id));
    }, 6000);
  }, []);

  const dismissToast = useCallback((id: number) => {
    setToasts((prev) => prev.filter((t) => t.id !== id));
  }, []);

  // Subscribe to SSE events for automatic notifications
  useEffect(() => {
    let es: EventSource | null = null;
    let reconnectTimer: ReturnType<typeof setTimeout>;

    const connect = () => {
      es = new EventSource("http://127.0.0.1:8402/api/v1/events");
      es.onmessage = (event) => {
        try {
          const data = JSON.parse(event.data);
          switch (data.type) {
            case "budget_alert":
              addToast(
                `Budget alert: ${data.threshold_pct}% spent ($${data.spent_usd?.toFixed(2)} / $${data.limit_usd?.toFixed(2)})`,
                data.threshold_pct >= 100 ? "error" : data.threshold_pct >= 80 ? "warning" : "info"
              );
              break;
            case "provider_status":
              if (data.state === "open") {
                addToast(`Provider "${data.provider}" circuit breaker opened (${data.failures} failures)`, "warning");
              } else if (data.state === "closed" && data.failures === 0) {
                addToast(`Provider "${data.provider}" recovered`, "success");
              }
              break;
          }
        } catch {}
      };
      es.onerror = () => {
        es?.close();
        reconnectTimer = setTimeout(connect, 5000);
      };
    };

    connect();
    return () => {
      es?.close();
      clearTimeout(reconnectTimer);
    };
  }, [addToast]);

  return (
    <NotificationContext.Provider value={{ toasts, addToast, dismissToast }}>
      {children}
      {/* Toast container */}
      {toasts.length > 0 && (
        <div className="fixed bottom-4 right-4 z-50 flex flex-col gap-2 max-w-sm">
          {toasts.map((toast) => (
            <div
              key={toast.id}
              onClick={() => dismissToast(toast.id)}
              className={`cursor-pointer rounded-lg border px-4 py-3 text-sm shadow-lg backdrop-blur-sm transition-all animate-in slide-in-from-right ${
                toast.type === "error"
                  ? "border-red-500/30 bg-red-500/10 text-red-300"
                  : toast.type === "warning"
                  ? "border-amber-500/30 bg-amber-500/10 text-amber-300"
                  : toast.type === "success"
                  ? "border-emerald-500/30 bg-emerald-500/10 text-emerald-300"
                  : "border-brand-500/30 bg-brand-500/10 text-brand-300"
              }`}
            >
              <div className="flex items-start gap-2">
                <span className="text-base leading-none mt-0.5">
                  {toast.type === "error" ? "\u26D4" : toast.type === "warning" ? "\u26A0" : toast.type === "success" ? "\u2705" : "\u2139"}
                </span>
                <span>{toast.message}</span>
              </div>
            </div>
          ))}
        </div>
      )}
    </NotificationContext.Provider>
  );
}
