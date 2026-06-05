//! flux-policy — the auto-calibrating, multi-engine powerplant for Flux/SIGIL.
//!
//! **The idea (Viktor's metaphor):** one motor that's a Koenigsegg on the road,
//! a jet at altitude, a yacht on long water, and a rocket for the burst — and it
//! *auto-shifts* to the right engine for the regime. Here the "engines" are
//! coherent parameter profiles; the **policy** reads live metrics, selects the
//! engine that fits, and fine-tunes individual knobs on top.
//!
//! **The dual loop (no-recompile AND recompile, together):**
//! - *Without recompiling* — every tunable is an **env var** the runtime already
//!   sources (e.g. `FLUX_GOSSIPSUB_MESH_N_LOW`). The policy emits new env; the
//!   process picks it up next start. Improve in seconds.
//! - *With recompiling* — when a knob doesn't exist yet, you add it (code) and
//!   register it here. Same iterative loop, two speeds.
//!
//! Fed by the flux webhook surface (a build/run event carries [`Metric`]s),
//! exposed via MCP (`flux_policy_calibrate`). SIGIL adopts the registry — the
//! gossipsub knobs already read these env vars (see `flux-p2p/src/swarm.rs`).
//!
//! Seeded with rules learned the hard way this session — e.g. the cross-WAN
//! propagation fix: `blocks_applied == 0` → drop the gossip mesh floor to 1.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A tunable knob: bound to an env var, with a default and a safe range.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Param {
    pub key: String,
    pub env: String,
    pub value: f64,
    pub min: f64,
    pub max: f64,
}
impl Param {
    pub fn new(key: &str, env: &str, default: f64, min: f64, max: f64) -> Self {
        Self { key: key.into(), env: env.into(), value: default, min, max }
    }
    fn clamp(&self, v: f64) -> f64 {
        v.max(self.min).min(self.max)
    }
}

/// A named engine profile — a coherent bundle of knob settings for one regime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Engine {
    pub name: String,
    pub regime: String,
    /// key → value overrides this engine applies when selected.
    pub set: BTreeMap<String, f64>,
}

/// An observation from a webhook / run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metric {
    pub name: String,
    pub value: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Cond { Lt(f64), Le(f64), Gt(f64), Ge(f64), Eq(f64) }
impl Cond {
    fn holds(&self, v: f64) -> bool {
        match *self {
            Cond::Lt(x) => v < x,
            Cond::Le(x) => v <= x,
            Cond::Gt(x) => v > x,
            Cond::Ge(x) => v >= x,
            Cond::Eq(x) => (v - x).abs() < f64::EPSILON,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Action { Set(f64), Scale(f64), SelectEngine(String) }

/// `when metric <cond>` ⇒ `action` (on a param, or select an engine).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub metric: String,
    pub cond: Cond,
    pub target: String, // param key, or "" for SelectEngine
    pub action: Action,
    pub why: String,
}

/// One applied change, for the audit trail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub kind: String, // "param" | "engine"
    pub target: String,
    pub from: String,
    pub to: String,
    pub why: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    pub params: Vec<Param>,
    pub engines: Vec<Engine>,
    pub rules: Vec<Rule>,
    pub active_engine: Option<String>,
}

impl Policy {
    fn param_mut(&mut self, key: &str) -> Option<&mut Param> {
        self.params.iter_mut().find(|p| p.key == key)
    }

    /// Read metrics → select the engine that fits + fine-tune knobs. Returns the
    /// changes (the audit trail). Idempotent: re-running with the same metrics
    /// yields no further changes.
    pub fn calibrate(&mut self, metrics: &[Metric]) -> Vec<Change> {
        let mut changes = Vec::new();
        let get = |name: &str| metrics.iter().find(|m| m.name == name).map(|m| m.value);

        // pass 1: engine selection (SelectEngine rules)
        let rules = self.rules.clone();
        for r in &rules {
            if let Action::SelectEngine(eng) = &r.action {
                if let Some(v) = get(&r.metric) {
                    if r.cond.holds(v) && self.active_engine.as_deref() != Some(eng) {
                        // apply the engine's bundle
                        let overrides = self.engines.iter().find(|e| &e.name == eng).map(|e| e.set.clone());
                        if let Some(set) = overrides {
                            let prev = self.active_engine.clone().unwrap_or_else(|| "none".into());
                            self.active_engine = Some(eng.clone());
                            changes.push(Change { kind: "engine".into(), target: "engine".into(), from: prev, to: eng.clone(), why: r.why.clone() });
                            for (k, val) in set {
                                if let Some(p) = self.param_mut(&k) {
                                    let nv = p.clamp(val);
                                    if (nv - p.value).abs() > f64::EPSILON {
                                        let from = p.value;
                                        p.value = nv;
                                        changes.push(Change { kind: "param".into(), target: k.clone(), from: format!("{from}"), to: format!("{nv}"), why: format!("engine {eng}") });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        // pass 2: per-param fine-tune (Set / Scale rules)
        for r in &rules {
            let v = match get(&r.metric) { Some(v) if r.cond.holds(v) => v, _ => continue };
            let _ = v;
            let Some(p) = self.param_mut(&r.target) else { continue };
            let nv = match &r.action {
                Action::Set(x) => p.clamp(*x),
                Action::Scale(f) => p.clamp(p.value * f),
                Action::SelectEngine(_) => continue,
            };
            if (nv - p.value).abs() > f64::EPSILON {
                let from = p.value;
                p.value = nv;
                changes.push(Change { kind: "param".into(), target: r.target.clone(), from: format!("{from}"), to: format!("{nv}"), why: r.why.clone() });
            }
        }
        changes
    }

    /// Emit the env the runtime sources — adopt WITHOUT recompiling.
    pub fn to_env(&self) -> String {
        self.params
            .iter()
            .map(|p| format!("export {}={}", p.env, fmt(p.value)))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// JSON for the MCP / webhook surface.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "active_engine": self.active_engine,
            "params": self.params.iter().map(|p| serde_json::json!({"key": p.key, "env": p.env, "value": p.value})).collect::<Vec<_>>(),
        })
    }

    /// The default Flux/SIGIL powerplant: the four engines + this session's rules.
    pub fn standard() -> Self {
        let params = vec![
            Param::new("mesh_n_low", "FLUX_GOSSIPSUB_MESH_N_LOW", 4.0, 1.0, 12.0),
            Param::new("mesh_outbound_min", "FLUX_GOSSIPSUB_MESH_OUTBOUND_MIN", 2.0, 0.0, 4.0),
            Param::new("batch_size", "FLUX_BATCH_SIZE", 128.0, 1.0, 4096.0),
            Param::new("parallelism", "FLUX_PARALLELISM", 8.0, 1.0, 64.0),
            Param::new("producer_run_secs", "SIGIL_PRODUCER_RUN_SECS", 60.0, 10.0, 3600.0),
        ];
        let eng = |n: &str, regime: &str, kv: &[(&str, f64)]| Engine {
            name: n.into(), regime: regime.into(),
            set: kv.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
        };
        let engines = vec![
            // Koenigsegg — peak raw speed, low latency: small batches, fat single-lane
            eng("koenigsegg", "latency-critical", &[("batch_size", 16.0), ("parallelism", 4.0)]),
            // Jet — high-altitude throughput / cross-WAN scale: low mesh floor, big parallel
            eng("jet", "cross-wan-scale", &[("mesh_n_low", 1.0), ("mesh_outbound_min", 1.0), ("parallelism", 32.0), ("producer_run_secs", 200.0)]),
            // Yacht — sustained cruise efficiency: modest everything, long soak
            eng("yacht", "long-soak", &[("batch_size", 64.0), ("parallelism", 8.0), ("producer_run_secs", 1800.0)]),
            // Rocket — burst: everything maxed briefly
            eng("rocket", "burst", &[("batch_size", 2048.0), ("parallelism", 64.0)]),
        ];
        let rules = vec![
            // engine auto-select by regime metric
            Rule { metric: "cross_wan".into(), cond: Cond::Ge(1.0), target: "".into(), action: Action::SelectEngine("jet".into()), why: "cross-WAN regime → jet (low mesh floor + long producer)".into() },
            Rule { metric: "soak".into(), cond: Cond::Ge(1.0), target: "".into(), action: Action::SelectEngine("yacht".into()), why: "long soak → yacht (sustained efficiency)".into() },
            Rule { metric: "latency_critical".into(), cond: Cond::Ge(1.0), target: "".into(), action: Action::SelectEngine("koenigsegg".into()), why: "latency-critical → koenigsegg (small batch, fat lane)".into() },
            Rule { metric: "burst".into(), cond: Cond::Ge(1.0), target: "".into(), action: Action::SelectEngine("rocket".into()), why: "burst → rocket (max throughput briefly)".into() },
            // fine-tune rules (the real lessons)
            Rule { metric: "blocks_applied".into(), cond: Cond::Eq(0.0), target: "mesh_n_low".into(), action: Action::Set(1.0), why: "0 propagation → drop gossip mesh floor so a small mesh publishes (the cross-WAN fix)".into() },
            Rule { metric: "peak_rss_pct".into(), cond: Cond::Gt(85.0), target: "batch_size".into(), action: Action::Scale(0.5), why: "near OOM → halve the batch".into() },
        ];
        Self { params, engines, rules, active_engine: None }
    }
}

fn fmt(v: f64) -> String {
    if v.fract() == 0.0 { format!("{}", v as i64) } else { format!("{v}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn m(n: &str, v: f64) -> Metric { Metric { name: n.into(), value: v } }
    fn pv(p: &Policy, k: &str) -> f64 { p.params.iter().find(|x| x.key == k).unwrap().value }

    #[test]
    fn cross_wan_selects_jet_and_lowers_mesh_floor() {
        let mut p = Policy::standard();
        let ch = p.calibrate(&[m("cross_wan", 1.0)]);
        assert_eq!(p.active_engine.as_deref(), Some("jet"));
        assert_eq!(pv(&p, "mesh_n_low"), 1.0); // jet bundle dropped the floor → THE cross-WAN fix, automatic
        assert_eq!(pv(&p, "producer_run_secs"), 200.0);
        assert!(ch.iter().any(|c| c.kind == "engine" && c.to == "jet"));
    }

    #[test]
    fn zero_propagation_rule_fixes_mesh_floor_directly() {
        let mut p = Policy::standard();
        p.calibrate(&[m("blocks_applied", 0.0)]);
        assert_eq!(pv(&p, "mesh_n_low"), 1.0); // the exact fix this session needed, now automatic
    }

    #[test]
    fn near_oom_halves_batch() {
        let mut p = Policy::standard();
        p.calibrate(&[m("peak_rss_pct", 92.0)]);
        assert_eq!(pv(&p, "batch_size"), 64.0); // 128 → 64
    }

    #[test]
    fn clamps_to_safe_range() {
        let mut p = Policy::standard();
        // rocket maxes parallelism to 64 (the cap), not beyond
        p.calibrate(&[m("burst", 1.0)]);
        assert_eq!(pv(&p, "parallelism"), 64.0);
    }

    #[test]
    fn emits_env_for_no_recompile_adoption() {
        let mut p = Policy::standard();
        p.calibrate(&[m("cross_wan", 1.0)]);
        let env = p.to_env();
        assert!(env.contains("export FLUX_GOSSIPSUB_MESH_N_LOW=1"));
        assert!(env.contains("export SIGIL_PRODUCER_RUN_SECS=200"));
    }

    #[test]
    fn idempotent_no_metrics_no_change() {
        let mut p = Policy::standard();
        assert!(p.calibrate(&[]).is_empty());
    }
}
