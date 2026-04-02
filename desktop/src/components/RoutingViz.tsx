import { useMemo } from "react";

export interface RoutingPath {
  tier: string;
  score: number;
  provider: string;
  model: string;
  attempt: number;
  costUsd: number;
  latencyMs: number;
  inputTokens?: number;
  outputTokens?: number;
  error?: boolean;
  candidates?: Array<{
    provider: string;
    model: string;
    marginalCost: number;
    selected: boolean;
    skipped?: string;
  }>;
}

const TIER_COLORS: Record<string, string> = {
  Simple: "#38bdf8", SIMPLE: "#38bdf8",
  Medium: "#fbbf24", MEDIUM: "#fbbf24",
  Complex: "#a78bfa", COMPLEX: "#a78bfa",
  Reasoning: "#f472b6", REASONING: "#f472b6",
};

const NODE_WIDTH = 150;
const NODE_HEIGHT = 48;
const ARROW_LEN = 28;
const TOP_PAD = 16;

interface NodeDef {
  label: string;
  sublabel: string;
  color: string;
}

export default function RoutingViz({ path }: { path: RoutingPath | null }) {
  const nodes = useMemo<NodeDef[]>(() => {
    if (!path) return [];
    const tierColor = TIER_COLORS[path.tier] || "#94a3b8";
    const tierName = path.tier || "Unknown";
    const providerName = path.provider || "unknown";
    const modelName = path.model || "unknown";
    const costLabel = path.costUsd > 0 ? `$${path.costUsd.toFixed(4)}` : "FREE";

    return [
      { label: "Request", sublabel: path.inputTokens ? `${path.inputTokens} tokens` : "incoming", color: "#64748b" },
      { label: "Complexity", sublabel: `score ${path.score.toFixed(3)}`, color: "#8b5cf6" },
      { label: "Tier", sublabel: tierName, color: tierColor },
      { label: "Cost", sublabel: costLabel, color: "#10b981" },
      { label: "Provider", sublabel: providerName, color: "#f59e0b" },
      { label: "Model", sublabel: truncate(modelName, 20), color: tierColor },
      { label: path.error ? "Failed" : "Response", sublabel: `${path.latencyMs}ms`, color: path.error ? "#ef4444" : "#22c55e" },
    ];
  }, [path]);

  if (!path || nodes.length === 0) {
    return (
      <div className="flex items-center justify-center h-full text-secondary text-xs">
        Send a message to see routing
      </div>
    );
  }

  const totalHeight = TOP_PAD + nodes.length * (NODE_HEIGHT + ARROW_LEN) - ARROW_LEN + TOP_PAD;
  const svgWidth = NODE_WIDTH + 40;

  return (
    <div className="flex flex-col h-full overflow-y-auto">
      <h3 className="text-xs font-semibold text-secondary uppercase tracking-wider px-3 py-3">
        Routing Path
      </h3>
      <svg width="100%" viewBox={`0 0 ${svgWidth} ${totalHeight}`} className="flex-1">
        <defs>
          <marker id="arrowhead" markerWidth="7" markerHeight="5" refX="7" refY="2.5" orient="auto">
            <polygon points="0 0, 7 2.5, 0 5" fill="#475569" />
          </marker>
        </defs>

        {nodes.map((node, i) => {
          const x = (svgWidth - NODE_WIDTH) / 2;
          const y = TOP_PAD + i * (NODE_HEIGHT + ARROW_LEN);
          const cx = x + NODE_WIDTH / 2;

          return (
            <g key={i}>
              {i > 0 && (
                <line x1={cx} y1={y - ARROW_LEN + 4} x2={cx} y2={y - 3}
                  stroke="#475569" strokeWidth={1.5} markerEnd="url(#arrowhead)" />
              )}
              <rect x={x} y={y} width={NODE_WIDTH} height={NODE_HEIGHT}
                rx={10} fill={`${node.color}12`} stroke={node.color} strokeWidth={1.5} />
              {/* Step label (small, top) */}
              <text x={cx} y={y + 18} fill={node.color} fontSize={11} fontWeight={600}
                fontFamily="system-ui, -apple-system, sans-serif" textAnchor="middle">
                {node.label}
              </text>
              {/* Decision value (larger, bottom) */}
              <text x={cx} y={y + 35} fill="#cbd5e1" fontSize={12} fontWeight={500}
                fontFamily="system-ui, -apple-system, sans-serif" textAnchor="middle">
                {node.sublabel}
              </text>
            </g>
          );
        })}
      </svg>

      <div className="px-3 py-3 border-t border-themed text-xs space-y-1.5">
        <div className="flex justify-between">
          <span className="text-secondary">Cost</span>
          <span className={path.costUsd === 0 ? "text-emerald-400 font-medium" : "text-primary"}>
            {path.costUsd > 0 ? `$${path.costUsd.toFixed(4)}` : "FREE"}
          </span>
        </div>
        <div className="flex justify-between">
          <span className="text-secondary">Latency</span>
          <span className="text-primary">{path.latencyMs}ms</span>
        </div>
        {path.attempt > 1 && (
          <div className="flex justify-between">
            <span className="text-secondary">Fallbacks</span>
            <span className="text-amber-400">{path.attempt - 1}</span>
          </div>
        )}
      </div>
    </div>
  );
}

function truncate(s: string, max: number): string {
  return s.length > max ? s.slice(0, max - 1) + "\u2026" : s;
}
