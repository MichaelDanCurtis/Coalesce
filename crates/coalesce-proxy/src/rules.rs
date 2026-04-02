//! Auto-failover rules engine — user-defined conditions that trigger actions.

use serde::{Deserialize, Serialize};
use std::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverRule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub condition: RuleCondition,
    pub action: RuleAction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RuleCondition {
    #[serde(rename = "quota_below")]
    QuotaBelow { provider: String, percent: f64 },
    #[serde(rename = "error_rate_above")]
    ErrorRateAbove { provider: String, threshold: f64 },
    #[serde(rename = "latency_above")]
    LatencyAbove { provider: String, ms: u64 },
    #[serde(rename = "budget_above")]
    BudgetAbove { percent: f64 },
    #[serde(rename = "provider_down")]
    ProviderDown { provider: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RuleAction {
    #[serde(rename = "disable_provider")]
    DisableProvider { provider: String },
    #[serde(rename = "prefer_provider")]
    PreferProvider { provider: String, priority: u32 },
    #[serde(rename = "notify")]
    Notify { message: String },
    #[serde(rename = "switch_profile")]
    SwitchProfile { profile: String },
}

pub struct RulesEngine {
    rules: RwLock<Vec<FailoverRule>>,
}

impl RulesEngine {
    pub fn new() -> Self {
        Self {
            rules: RwLock::new(Vec::new()),
        }
    }

    pub fn list(&self) -> Vec<FailoverRule> {
        self.rules.read().unwrap().clone()
    }

    pub fn add(&self, rule: FailoverRule) {
        self.rules.write().unwrap().push(rule);
    }

    pub fn update(&self, id: &str, rule: FailoverRule) -> bool {
        let mut rules = self.rules.write().unwrap();
        if let Some(existing) = rules.iter_mut().find(|r| r.id == id) {
            *existing = rule;
            true
        } else {
            false
        }
    }

    pub fn delete(&self, id: &str) -> bool {
        let mut rules = self.rules.write().unwrap();
        let len_before = rules.len();
        rules.retain(|r| r.id != id);
        rules.len() < len_before
    }

    pub fn toggle(&self, id: &str) -> Option<bool> {
        let mut rules = self.rules.write().unwrap();
        if let Some(rule) = rules.iter_mut().find(|r| r.id == id) {
            rule.enabled = !rule.enabled;
            Some(rule.enabled)
        } else {
            None
        }
    }

    /// Evaluate all enabled rules against current state and return triggered actions.
    pub fn evaluate(&self, ctx: &EvalContext) -> Vec<TriggeredAction> {
        let rules = self.rules.read().unwrap();
        let mut actions = Vec::new();

        for rule in rules.iter().filter(|r| r.enabled) {
            let triggered = match &rule.condition {
                RuleCondition::QuotaBelow { provider, percent } => {
                    ctx.quota_percent.get(provider)
                        .map(|q| *q < *percent)
                        .unwrap_or(false)
                }
                RuleCondition::ErrorRateAbove { provider, threshold } => {
                    ctx.error_rates.get(provider)
                        .map(|r| *r > *threshold)
                        .unwrap_or(false)
                }
                RuleCondition::LatencyAbove { provider, ms } => {
                    ctx.avg_latency_ms.get(provider)
                        .map(|l| *l > *ms)
                        .unwrap_or(false)
                }
                RuleCondition::BudgetAbove { percent } => {
                    ctx.budget_percent > *percent
                }
                RuleCondition::ProviderDown { provider } => {
                    ctx.circuit_open.get(provider).copied().unwrap_or(false)
                }
            };

            if triggered {
                actions.push(TriggeredAction {
                    rule_id: rule.id.clone(),
                    rule_name: rule.name.clone(),
                    action: rule.action.clone(),
                });
            }
        }
        actions
    }
}

/// Context passed to rule evaluation
pub struct EvalContext {
    pub quota_percent: std::collections::HashMap<String, f64>,
    pub error_rates: std::collections::HashMap<String, f64>,
    pub avg_latency_ms: std::collections::HashMap<String, u64>,
    pub budget_percent: f64,
    pub circuit_open: std::collections::HashMap<String, bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TriggeredAction {
    pub rule_id: String,
    pub rule_name: String,
    pub action: RuleAction,
}

/// Preset rules for common scenarios
pub fn preset_rules() -> Vec<FailoverRule> {
    vec![
        FailoverRule {
            id: "preset-copilot-quota".into(),
            name: "Low Copilot quota -> prefer DeepSeek".into(),
            enabled: false,
            condition: RuleCondition::QuotaBelow {
                provider: "copilot".into(),
                percent: 10.0,
            },
            action: RuleAction::PreferProvider {
                provider: "deepseek".into(),
                priority: 5,
            },
        },
        FailoverRule {
            id: "preset-high-latency".into(),
            name: "High latency -> disable provider".into(),
            enabled: false,
            condition: RuleCondition::LatencyAbove {
                provider: "".into(),
                ms: 10000,
            },
            action: RuleAction::Notify {
                message: "Provider latency exceeded 10s".into(),
            },
        },
        FailoverRule {
            id: "preset-budget-warning".into(),
            name: "Budget 80% -> switch to eco profile".into(),
            enabled: false,
            condition: RuleCondition::BudgetAbove { percent: 80.0 },
            action: RuleAction::SwitchProfile {
                profile: "eco".into(),
            },
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_engine() -> RulesEngine {
        let engine = RulesEngine::new();
        engine.add(FailoverRule {
            id: "r1".into(),
            name: "Test Rule 1".into(),
            enabled: true,
            condition: RuleCondition::ErrorRateAbove {
                provider: "openai".into(),
                threshold: 50.0,
            },
            action: RuleAction::DisableProvider {
                provider: "openai".into(),
            },
        });
        engine.add(FailoverRule {
            id: "r2".into(),
            name: "Test Rule 2".into(),
            enabled: false,
            condition: RuleCondition::BudgetAbove { percent: 80.0 },
            action: RuleAction::SwitchProfile {
                profile: "eco".into(),
            },
        });
        engine
    }

    fn empty_ctx() -> EvalContext {
        EvalContext {
            quota_percent: HashMap::new(),
            error_rates: HashMap::new(),
            avg_latency_ms: HashMap::new(),
            budget_percent: 0.0,
            circuit_open: HashMap::new(),
        }
    }

    #[test]
    fn test_crud_list() {
        let engine = make_engine();
        assert_eq!(engine.list().len(), 2);
    }

    #[test]
    fn test_crud_add_and_delete() {
        let engine = RulesEngine::new();
        assert_eq!(engine.list().len(), 0);
        engine.add(FailoverRule {
            id: "x".into(),
            name: "X".into(),
            enabled: true,
            condition: RuleCondition::ProviderDown { provider: "foo".into() },
            action: RuleAction::Notify { message: "down".into() },
        });
        assert_eq!(engine.list().len(), 1);
        assert!(engine.delete("x"));
        assert_eq!(engine.list().len(), 0);
        assert!(!engine.delete("nonexistent"));
    }

    #[test]
    fn test_crud_update() {
        let engine = make_engine();
        let updated = FailoverRule {
            id: "r1".into(),
            name: "Updated".into(),
            enabled: false,
            condition: RuleCondition::BudgetAbove { percent: 90.0 },
            action: RuleAction::Notify { message: "hi".into() },
        };
        assert!(engine.update("r1", updated));
        assert!(!engine.update("nonexistent", FailoverRule {
            id: "z".into(), name: "z".into(), enabled: true,
            condition: RuleCondition::BudgetAbove { percent: 0.0 },
            action: RuleAction::Notify { message: "".into() },
        }));
        let rules = engine.list();
        assert_eq!(rules.iter().find(|r| r.id == "r1").unwrap().name, "Updated");
    }

    #[test]
    fn test_toggle() {
        let engine = make_engine();
        assert_eq!(engine.toggle("r1"), Some(false)); // was true -> false
        assert_eq!(engine.toggle("r1"), Some(true));  // false -> true
        assert_eq!(engine.toggle("missing"), None);
    }

    #[test]
    fn test_evaluate_disabled_rules_skipped() {
        let engine = make_engine();
        // r2 is disabled and budget is 90% (above 80%), but should NOT trigger
        let mut ctx = empty_ctx();
        ctx.budget_percent = 90.0;
        let actions = engine.evaluate(&ctx);
        assert!(actions.iter().all(|a| a.rule_id != "r2"));
    }

    #[test]
    fn test_evaluate_error_rate_above() {
        let engine = make_engine();
        let mut ctx = empty_ctx();
        ctx.error_rates.insert("openai".into(), 75.0);
        let actions = engine.evaluate(&ctx);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].rule_id, "r1");
        assert!(matches!(actions[0].action, RuleAction::DisableProvider { .. }));
    }

    #[test]
    fn test_evaluate_error_rate_below_threshold() {
        let engine = make_engine();
        let mut ctx = empty_ctx();
        ctx.error_rates.insert("openai".into(), 30.0);
        let actions = engine.evaluate(&ctx);
        assert!(actions.is_empty());
    }

    #[test]
    fn test_evaluate_quota_below() {
        let engine = RulesEngine::new();
        engine.add(FailoverRule {
            id: "q1".into(),
            name: "Low quota".into(),
            enabled: true,
            condition: RuleCondition::QuotaBelow { provider: "copilot".into(), percent: 10.0 },
            action: RuleAction::PreferProvider { provider: "deepseek".into(), priority: 5 },
        });
        let mut ctx = empty_ctx();
        ctx.quota_percent.insert("copilot".into(), 5.0);
        let actions = engine.evaluate(&ctx);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0].action, RuleAction::PreferProvider { .. }));
    }

    #[test]
    fn test_evaluate_latency_above() {
        let engine = RulesEngine::new();
        engine.add(FailoverRule {
            id: "l1".into(),
            name: "Slow".into(),
            enabled: true,
            condition: RuleCondition::LatencyAbove { provider: "openai".into(), ms: 5000 },
            action: RuleAction::Notify { message: "slow".into() },
        });
        let mut ctx = empty_ctx();
        ctx.avg_latency_ms.insert("openai".into(), 6000);
        assert_eq!(engine.evaluate(&ctx).len(), 1);

        ctx.avg_latency_ms.insert("openai".into(), 3000);
        assert_eq!(engine.evaluate(&ctx).len(), 0);
    }

    #[test]
    fn test_evaluate_provider_down() {
        let engine = RulesEngine::new();
        engine.add(FailoverRule {
            id: "d1".into(),
            name: "Down".into(),
            enabled: true,
            condition: RuleCondition::ProviderDown { provider: "google".into() },
            action: RuleAction::DisableProvider { provider: "google".into() },
        });
        let mut ctx = empty_ctx();
        ctx.circuit_open.insert("google".into(), true);
        assert_eq!(engine.evaluate(&ctx).len(), 1);

        ctx.circuit_open.insert("google".into(), false);
        assert_eq!(engine.evaluate(&ctx).len(), 0);
    }

    #[test]
    fn test_evaluate_budget_above() {
        let engine = RulesEngine::new();
        engine.add(FailoverRule {
            id: "b1".into(),
            name: "Budget".into(),
            enabled: true,
            condition: RuleCondition::BudgetAbove { percent: 80.0 },
            action: RuleAction::SwitchProfile { profile: "eco".into() },
        });
        let mut ctx = empty_ctx();
        ctx.budget_percent = 85.0;
        assert_eq!(engine.evaluate(&ctx).len(), 1);

        ctx.budget_percent = 50.0;
        assert_eq!(engine.evaluate(&ctx).len(), 0);
    }

    #[test]
    fn test_presets() {
        let presets = preset_rules();
        assert_eq!(presets.len(), 3);
        assert!(presets.iter().all(|r| !r.enabled));
        assert!(presets.iter().all(|r| r.id.starts_with("preset-")));
    }

    #[test]
    fn test_serde_roundtrip() {
        let rule = FailoverRule {
            id: "s1".into(),
            name: "Serde test".into(),
            enabled: true,
            condition: RuleCondition::ErrorRateAbove { provider: "x".into(), threshold: 42.0 },
            action: RuleAction::PreferProvider { provider: "y".into(), priority: 3 },
        };
        let json = serde_json::to_string(&rule).unwrap();
        let parsed: FailoverRule = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "s1");
        assert_eq!(parsed.name, "Serde test");
        assert!(parsed.enabled);
    }
}
