export interface Provider {
  name: string;
  enabled: boolean;
  billing: string;
  circuit_breaker: {
    state: string;
    failures: number;
    total_requests: number;
    total_failures: number;
  };
  model_count: number;
  is_disabled?: boolean;
}

export interface ModelCapabilities {
  reasoning_effort_levels?: string[];
  adaptive_thinking?: boolean;
  thinking_budget?: [number, number];
  supported_endpoints?: string[];
  vendor?: string;
  model_picker_category?: string;
}

export interface ModelInfo {
  id: string;
  name: string;
  owned_by: string;
  quality_tier: string;
  context_window: number;
  max_output: number | null;
  reasoning: boolean;
  vision: boolean;
  tool_calling: boolean;
  canonical_family?: string;
  capabilities?: ModelCapabilities;
  pricing: {
    input_per_m: number;
    output_per_m: number;
  };
  marginal_cost: {
    usd: number;
    is_free: boolean;
    is_available: boolean;
  };
  circuit_breaker: string;
  is_disabled?: boolean;
}

export interface ModelOverride {
  provider: string;
  model_id: string;
  field: string;
  value: string;
}

export interface Stats {
  total_requests: number;
  successful_requests: number;
  success_rate: number;
  total_cost_usd: number;
  avg_latency_ms: number;
  total_input_tokens: number;
  total_output_tokens: number;
}

export interface QuotaInfo {
  provider: string;
  model: string | null;
  billing: string;
  used: number;
  remaining: number | null;
  is_depleted: boolean;
}

export interface RequestEntry {
  id?: number;
  timestamp?: number;
  tier: string;
  score: number;
  provider: string;
  model: string;
  input_tokens: number | null;
  output_tokens: number | null;
  cost_usd: number | null;
  latency_ms: number | null;
  success: boolean;
}

export interface ConfigProfile {
  name: string;
  description?: string;
  created_at: number;
  updated_at: number;
  provider_count: number;
}

export interface SearchParams {
  limit?: number;
  offset?: number;
  provider?: string;
  tier?: string;
  model?: string;
  from?: number;
  to?: number;
  search?: string;
  failures_only?: boolean;
}

export interface RoutingResult {
  tier: string;
  score: number;
  reasoning: string;
  candidates: Array<{
    model: string;
    provider: string;
    marginal_cost_usd: number;
    is_free: boolean;
  }>;
}

export interface RoutingProfile {
  name: string;
  strategy: string;
}
