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
  candidates?: Array<{
    provider: string;
    model: string;
    marginalCost: number;
    selected: boolean;
    skipped?: string; // reason skipped
  }>;
}

const TIER_COLORS: Record<string, string> = {
  Simple: "#38bdf8",    // sky-400
  Medium: "#fbbf24",    // amber-400
  Complex: "#a78bfa",   // violet-400
  Reasoning: "#f472b6", // pink-400
};

const NODE_WIDTH = 140;
const NODE_HEIGHT = 44;
const ARROW_LEN = 24;
const TOP_PAD = 16;

interface NodeDef {
  label: string;
  sublabel?: string;
  color: string;
  active: boolean;
}

export default function RoutingViz({ path }: { path: RoutingPath | null }) {
  const nodes = useMemo<NodeDef[]>(() => {
    if (!path) return [];
    const tierColor = TIER_COLORS[path.tier] || "#94a3b8";
    return [
      {
        label: "Request",
        sublabel: path.inputTokens ? `${path.inputTokens} tokens` : undefined,
        color: "#64748b",
        active: true,
      },
      {
        label: "Scorer",
        sublabel: `score: ${path.score.toFixed(3)}`,
        color: "#8b5cf6",
        active: true,
      },
      {
        label: path.tier,
        sublabel: "tier",
        color: tierColor,
        active: true,
      },
      {
        label: "Economics",
        sublabel: path.costUsd > 0 ? `$${path.costUsd.toFixed(4)}` : "FREE",
        color: "#10b981",
        active: true,
      },
      {
        label: path.provider,
        sublabel: path.attempt > 1 ? `attempt ${path.attempt}` : undefined,
        color: "#f59e0b",
        active: true,
      },
      {
        label: truncate(path.model, 16),
        sublabel: path.latencyMs ? `${path.latencyMs}ms` : undefined,
        color: tierColor,
        active: true,
      },
      {
        label: "Response",
        sublabel: path.outputTokens ? `${path.outputTokens} tokens` : undefined,
        color: "#22c55e",
        active: true,
      },
    ];
  }, [path]);

  if (!path || nodes.length === 0) {
    return (
      <div className="flex items-center justify-center h-full text-secondary text-xs">
        Send a message to see routing
      </div>
    );
  }

  const totalHeight =
    TOP_PAD + nodes.length * (NODE_HEIGHT + ARROW_LEN) - ARROW_LEN + TOP_PAD;
  const svgWidth = NODE_WIDTH + 32;

  return (
    <div className="flex flex-col h-full overflow-y-auto">
      <h3 className="text-xs font-semibold text-secondary uppercase tracking-wider px-2 py-2">
        Routing Path
      </h3>
      <svg
        width="100%"
        viewBox={`0 0 ${svgWidth} ${totalHeight}`}
        className="flex-1"
      >
        <defs>
          <marker
            id="arrowhead"
            markerWidth="8"
            markerHeight="6"
            refX="8"
            refY="3"
            orient="auto"
          >
            <polygon points="0 0, 8 3, 0 6" fill="#64748b" />
          </marker>
          <filter id="glow">
            <feGaussianBlur stdDeviation="2" result="blur" />
            <feMerge>
              <feMergeNode in="blur" />
              <feMergeNode in="SourceGraphic" />
            </feMerge>
          </filter>
        </defs>

        {nodes.map((node, i) => {
          const x = (svgWidth - NODE_WIDTH) / 2;
          const y = TOP_PAD + i * (NODE_HEIGHT + ARROW_LEN);
          const cx = x + NODE_WIDTH / 2;

          return (
            <g key={i}>
              {/* Connecting arrow */}
              {i > 0 && (
                <line
                  x1={cx}
                  y1={y - ARROW_LEN + 2}
                  x2={cx}
                  y2={y - 2}
                  stroke="#64748b"
                  strokeWidth={1.5}
                  markerEnd="url(#arrowhead)"
                  opacity={0.6}
                />
              )}

              {/* Node rectangle */}
              <rect
                x={x}
                y={y}
                width={NODE_WIDTH}
                height={NODE_HEIGHT}
                rx={8}
                fill={node.active ? node.color + "18" : "#1e293b"}
                stroke={node.active ? node.color : "#475569"}
                strokeWidth={node.active ? 1.5 : 1}
                filter={node.active ? "url(#glow)" : undefined}
              />

              {/* Accent bar left — inset to avoid clipping rounded corners */}
              <rect
                x={x + 4}
                y={y + 4}
                width={3}
                height={NODE_HEIGHT - 8}
                rx={1.5}
                fill={node.color}
                opacity={node.active ? 0.8 : 0.3}
              />

              {/* Label */}
              <text
                x={x + 12}
                y={y + (node.sublabel ? 17 : 22)}
                fill={node.active ? node.color : "#94a3b8"}
                fontSize={12}
                fontWeight={600}
                fontFamily="system-ui, sans-serif"
              >
                {node.label}
              </text>

              {/* Sublabel */}
              {node.sublabel && (
                <text
                  x={x + 12}
                  y={y + 33}
                  fill="#94a3b8"
                  fontSize={10}
                  fontFamily="system-ui, sans-serif"
                >
                  {node.sublabel}
                </text>
              )}
            </g>
          );
        })}
      </svg>

      {/* Cost summary */}
      <div className="px-2 py-2 border-t border-themed text-xs space-y-1">
        <div className="flex justify-between">
          <span className="text-secondary">Cost</span>
          <span className={path.costUsd === 0 ? "text-emerald-400" : "text-primary"}>
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
