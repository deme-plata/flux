//! # GoalLoop — webhook-native `/loop` + `/goal` with a `#!/sigil` shebang
//!
//! A **newly-invented** subsystem that ports Claude Code's own `/loop` and
//! `/goal` ergonomics into the webhook-mcp event bus, plus a shebang-style
//! interpreter directive so a loop/goal body can be routed through SIGIL.
//!
//! ## The three primitives
//!
//! 1. **Loop function** (`LoopSpec`) — mirrors Claude Code `/loop`: run a body
//!    on a fixed interval *or* self-paced (the body decides the next delay).
//!    Each tick emits a [`WebhookEvent`] onto the bus, which the existing
//!    [`crate::McpDispatcher`] turns into an MCP tool call. No new transport.
//!
//! 2. **Goal / measure function** (`GoalSpec`) — mirrors `/goal`: a loop with a
//!    *measurable* target. Each iteration runs a **measure** body (an MCP tool
//!    whose JSON output is reduced to one `f64` via a dotted path), compares it
//!    to `target` under `Comparator`, and **auto-stops** when the goal is met or
//!    the metric plateaus for `plateau_patience` ticks. This is the "consensus"
//!    of `/goal` — converge then halt, instead of looping forever.
//!
//! 3. **`#!/sigil` shebang** — a body string may begin with a shebang line that
//!    picks the *executor*, exactly like a script interpreter directive:
//!
//!    ```text
//!    #!/sigil   flux_sigil_txn_send {"to":"...","amount":1}   ← settle on SIGIL
//!    #!/mcp     flux_cortex_loop {"preset":"MaxPerf"}         ← any MCP tool
//!    #!/loop    <bare event_type>                             ← re-emit on bus
//!    #!/goal    <goal_id>                                     ← tick a goal
//!    ```
//!
//!    A `#!/sigil` body is the headline: the loop/goal step is recorded on the
//!    SIGIL chain (provenance + settlement) rather than being a fire-and-forget
//!    MCP call. Every loop iteration thus leaves an auditable on-chain trail.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio::sync::broadcast;
use tracing::{info, warn};

use crate::types::{WebhookEvent, now_ms_pub as now_ms};

/// Which interpreter runs a loop/goal body — chosen by the `#!/…` shebang line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Executor {
    /// `#!/sigil` — settle/record the step on the SIGIL chain (provenance).
    Sigil,
    /// `#!/mcp` — call an arbitrary MCP tool via the dispatcher.
    Mcp,
    /// `#!/loop` — re-emit a bare event onto the bus (chain loops together).
    Loop,
    /// `#!/goal` — tick a registered goal by id.
    Goal,
}

/// A parsed shebang body: `#!/<executor>  <tool/target>  <json-args?>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shebang {
    pub executor: Executor,
    /// MCP tool name, event type, or goal id depending on `executor`.
    pub target: String,
    /// JSON arguments passed to the tool (empty object if none).
    pub args: serde_json::Value,
}

impl Shebang {
    /// Parse a body. A body WITHOUT a leading `#!` defaults to `#!/mcp` with the
    /// first whitespace token as the tool name — so plain bodies still work.
    pub fn parse(body: &str) -> Result<Self> {
        let body = body.trim();
        let first_line = body.lines().next().unwrap_or("").trim();

        let (exec_tok, rest) = if let Some(stripped) = first_line.strip_prefix("#!/") {
            // shebang form: "#!/sigil  tool {json}"
            let mut parts = stripped.splitn(2, char::is_whitespace);
            let exec = parts.next().unwrap_or("").to_string();
            (exec, parts.next().unwrap_or("").trim().to_string())
        } else {
            // bare form → treat as #!/mcp
            ("mcp".to_string(), first_line.to_string())
        };

        let executor = match exec_tok.as_str() {
            "sigil" => Executor::Sigil,
            "mcp" => Executor::Mcp,
            "loop" => Executor::Loop,
            "goal" => Executor::Goal,
            other => return Err(anyhow!("unknown shebang executor: #!/{}", other)),
        };

        // rest = "<target> <maybe-json>"
        let mut rp = rest.splitn(2, char::is_whitespace);
        let target = rp.next().unwrap_or("").to_string();
        if target.is_empty() {
            return Err(anyhow!("shebang body missing target after #!/{:?}", executor));
        }
        let args = match rp.next().map(str::trim).filter(|s| !s.is_empty()) {
            Some(json) => serde_json::from_str(json)
                .map_err(|e| anyhow!("invalid shebang JSON args: {}", e))?,
            None => serde_json::json!({}),
        };

        Ok(Shebang { executor, target, args })
    }

    /// Lower the shebang into a [`WebhookEvent`] for the existing dispatcher.
    /// `#!/sigil` is tagged so the dispatcher routes it to the SIGIL settle tool.
    pub fn to_event(&self, loop_id: &str, iteration: u64) -> WebhookEvent {
        let (event_type, mcp_tool) = match self.executor {
            // SIGIL settle tool — provenance/settlement for this loop step.
            Executor::Sigil => ("sigil_loop_step", Some("flux_sigil_txn_send")),
            Executor::Mcp => ("goalloop_step", Some(self.target.as_str())),
            Executor::Loop => (self.target.as_str(), None),
            Executor::Goal => ("goal_tick", Some("flux_goal_consensus")),
        };

        let mut payload = self.args.clone();
        // annotate every step so on-chain/audit trail is self-describing
        if let serde_json::Value::Object(ref mut m) = payload {
            m.insert("__loop_id".into(), serde_json::json!(loop_id));
            m.insert("__iteration".into(), serde_json::json!(iteration));
            m.insert("__executor".into(), serde_json::json!(format!("{:?}", self.executor)));
            if self.executor != Executor::Mcp {
                m.insert("__target".into(), serde_json::json!(self.target));
            }
        }

        let mut ev = WebhookEvent::new(event_type, "goalloop", payload);
        ev.target_mcp_tool = mcp_tool.map(|s| s.to_string());
        ev.priority = 7; // above normal: loop/goal steps are intentional work
        ev
    }
}

/// Pacing for a loop — fixed cadence or self-paced (Claude `/loop` dynamic mode).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Pace {
    /// Fixed interval between ticks.
    Interval { secs: u64 },
    /// Self-paced: the body returns the next delay (clamped to [min,max]).
    SelfPaced { min_secs: u64, max_secs: u64 },
}

/// A registered loop — the webhook analog of Claude Code `/loop <prompt>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopSpec {
    pub id: String,
    /// Body string (may carry a `#!/…` shebang). Re-parsed each tick.
    pub body: String,
    pub pace: Pace,
    /// Stop after this many iterations (None = until cancelled / goal met).
    pub max_iterations: Option<u64>,
    pub enabled: bool,
    pub iterations_done: u64,
    /// Optional goal id that, once satisfied, halts this loop.
    pub bound_goal: Option<String>,
}

/// Numeric comparison for goal satisfaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Comparator {
    Ge, // metric >= target
    Le, // metric <= target
    Eq, // |metric - target| <= epsilon
}

/// A measurable goal — the webhook analog of Claude Code `/goal`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalSpec {
    pub id: String,
    /// Human description of the goal ("SAP score >= 0.8 on tunneling peers").
    pub description: String,
    /// Measure body: an MCP/sigil tool call whose output is read for the metric.
    pub measure_body: String,
    /// Dotted path into the measure tool's JSON output, e.g. "result.sap.score".
    pub metric_path: String,
    pub target: f64,
    pub comparator: Comparator,
    pub epsilon: f64,
    /// Stop early if the metric does not improve for this many measurements.
    pub plateau_patience: u32,
    // ── live state ──
    pub last_metric: Option<f64>,
    pub best_metric: Option<f64>,
    pub plateau_count: u32,
    pub satisfied: bool,
    pub measurements: u64,
}

impl GoalSpec {
    /// Check satisfaction of `metric` against the target; updates plateau state.
    /// Returns `true` when the goal is met.
    pub fn evaluate(&mut self, metric: f64) -> bool {
        self.measurements += 1;
        let met = match self.comparator {
            Comparator::Ge => metric >= self.target,
            Comparator::Le => metric <= self.target,
            Comparator::Eq => (metric - self.target).abs() <= self.epsilon,
        };

        // plateau tracking — "improvement" depends on comparator direction
        let improved = match (self.best_metric, self.comparator) {
            (None, _) => true,
            (Some(b), Comparator::Ge) => metric > b + self.epsilon,
            (Some(b), Comparator::Le) => metric < b - self.epsilon,
            (Some(b), Comparator::Eq) => (metric - self.target).abs() < (b - self.target).abs(),
        };
        if improved {
            self.best_metric = Some(metric);
            self.plateau_count = 0;
        } else {
            self.plateau_count += 1;
        }

        self.last_metric = Some(metric);
        self.satisfied = met;
        met
    }

    /// Whether the goal has plateaued (no improvement for `plateau_patience`).
    pub fn plateaued(&self) -> bool {
        self.plateau_count >= self.plateau_patience
    }
}

/// Extract a number from a dotted path in a JSON value (`a.b.c`).
/// Accepts JSON numbers, or numeric strings.
pub fn extract_metric(v: &serde_json::Value, path: &str) -> Option<f64> {
    let mut cur = v;
    for seg in path.split('.').filter(|s| !s.is_empty()) {
        cur = match cur {
            serde_json::Value::Object(m) => m.get(seg)?,
            serde_json::Value::Array(a) => a.get(seg.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    match cur {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// The engine that owns all loops + goals and drives them over the event bus.
pub struct GoalLoopEngine {
    event_tx: broadcast::Sender<WebhookEvent>,
    loops: Arc<RwLock<HashMap<String, LoopSpec>>>,
    goals: Arc<RwLock<HashMap<String, GoalSpec>>>,
}

impl GoalLoopEngine {
    pub fn new(event_tx: broadcast::Sender<WebhookEvent>) -> Self {
        Self {
            event_tx,
            loops: Arc::new(RwLock::new(HashMap::new())),
            goals: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register (or replace) a loop. Validates the shebang body up front.
    pub async fn register_loop(&self, mut spec: LoopSpec) -> Result<()> {
        Shebang::parse(&spec.body)?; // fail fast on bad bodies
        spec.iterations_done = 0;
        info!("➿ loop '{}' registered ({:?})", spec.id, spec.pace);
        self.loops.write().await.insert(spec.id.clone(), spec.clone());
        if spec.enabled {
            self.spawn_loop(spec).await;
        }
        Ok(())
    }

    /// Register a measurable goal.
    pub async fn register_goal(&self, mut spec: GoalSpec) -> Result<()> {
        Shebang::parse(&spec.measure_body)?;
        spec.satisfied = false;
        spec.measurements = 0;
        spec.plateau_count = 0;
        info!("🎯 goal '{}' registered: {}", spec.id, spec.description);
        self.goals.write().await.insert(spec.id.clone(), spec);
        Ok(())
    }

    /// Cancel a loop.
    pub async fn cancel_loop(&self, id: &str) {
        if let Some(l) = self.loops.write().await.get_mut(id) {
            l.enabled = false;
            info!("⏹ loop '{}' cancelled", id);
        }
    }

    /// Snapshot all loop + goal state (for `flux_webhook_loop_status`).
    pub async fn status(&self) -> serde_json::Value {
        let loops = self.loops.read().await;
        let goals = self.goals.read().await;
        serde_json::json!({
            "loops": loops.values().collect::<Vec<_>>(),
            "goals": goals.values().collect::<Vec<_>>(),
        })
    }

    /// Spawn the driver task for a single loop.
    async fn spawn_loop(&self, spec: LoopSpec) {
        let event_tx = self.event_tx.clone();
        let loops = self.loops.clone();
        let goals = self.goals.clone();

        tokio::spawn(async move {
            let id = spec.id.clone();
            loop {
                // re-read live state (allows cancel / goal-halt mid-flight)
                let current = { loops.read().await.get(&id).cloned() };
                let Some(cur) = current else { break };
                if !cur.enabled {
                    break;
                }
                if let Some(max) = cur.max_iterations {
                    if cur.iterations_done >= max {
                        info!("➿ loop '{}' hit max_iterations {}", id, max);
                        break;
                    }
                }

                // goal-halt: if bound to a goal that is satisfied, stop.
                if let Some(ref gid) = cur.bound_goal {
                    if goals.read().await.get(gid).map(|g| g.satisfied).unwrap_or(false) {
                        info!("➿ loop '{}' halted: bound goal '{}' satisfied", id, gid);
                        break;
                    }
                }

                // tick: parse shebang, emit the step event onto the bus
                match Shebang::parse(&cur.body) {
                    Ok(sb) => {
                        let ev = sb.to_event(&id, cur.iterations_done + 1);
                        if event_tx.send(ev).is_err() {
                            warn!("➿ loop '{}' tick lost (no subscribers)", id);
                        }
                    }
                    Err(e) => {
                        warn!("➿ loop '{}' bad body, stopping: {}", id, e);
                        break;
                    }
                }

                // advance + emit a heartbeat event for outbound/audit
                let next_delay = {
                    let mut w = loops.write().await;
                    if let Some(l) = w.get_mut(&id) {
                        l.iterations_done += 1;
                    }
                    match &cur.pace {
                        Pace::Interval { secs } => *secs,
                        // self-paced: default to mid-range; a body can later
                        // feed a measured delay back via register_loop().
                        Pace::SelfPaced { min_secs, max_secs } => {
                            (min_secs + max_secs) / 2
                        }
                    }
                };

                let _ = event_tx.send(WebhookEvent::new(
                    "loop_heartbeat",
                    "goalloop",
                    serde_json::json!({ "loop_id": id, "next_delay_secs": next_delay, "ts": now_ms() }),
                ));

                tokio::time::sleep(Duration::from_secs(next_delay.max(1))).await;
            }
        });
    }

    /// Feed a freshly-measured value into a goal; returns (satisfied, plateaued).
    /// The dispatcher calls this when a `goal_tick` measure result comes back.
    pub async fn record_measurement(&self, goal_id: &str, raw_output: &serde_json::Value) -> Result<(bool, bool)> {
        let mut goals = self.goals.write().await;
        let g = goals.get_mut(goal_id).ok_or_else(|| anyhow!("no such goal {}", goal_id))?;
        let metric = extract_metric(raw_output, &g.metric_path)
            .ok_or_else(|| anyhow!("metric path '{}' not found in measure output", g.metric_path))?;
        let met = g.evaluate(metric);
        let plateau = g.plateaued();
        info!(
            "🎯 goal '{}' measured {} (target {} {:?}) → met={} plateau={}",
            goal_id, metric, g.target, g.comparator, met, plateau
        );
        Ok((met, plateau))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sigil_shebang() {
        let sb = Shebang::parse("#!/sigil flux_sigil_txn_send {\"to\":\"qnk1\",\"amount\":1}").unwrap();
        assert_eq!(sb.executor, Executor::Sigil);
        assert_eq!(sb.target, "flux_sigil_txn_send");
        assert_eq!(sb.args["amount"], 1);
    }

    #[test]
    fn parse_bare_body_defaults_to_mcp() {
        let sb = Shebang::parse("flux_cortex_loop {\"preset\":\"MaxPerf\"}").unwrap();
        assert_eq!(sb.executor, Executor::Mcp);
        assert_eq!(sb.target, "flux_cortex_loop");
    }

    #[test]
    fn sigil_lowers_to_settle_tool() {
        let sb = Shebang::parse("#!/sigil flux_sigil_txn_send {}").unwrap();
        let ev = sb.to_event("loop-1", 3);
        assert_eq!(ev.target_mcp_tool.as_deref(), Some("flux_sigil_txn_send"));
        assert_eq!(ev.payload["__iteration"], 3);
        assert_eq!(ev.event_type, "sigil_loop_step");
    }

    #[test]
    fn goal_ge_satisfaction_and_plateau() {
        let mut g = GoalSpec {
            id: "sap".into(),
            description: "SAP >= 0.8".into(),
            measure_body: "#!/mcp flux_sap_status {}".into(),
            metric_path: "sap.score".into(),
            target: 0.8,
            comparator: Comparator::Ge,
            epsilon: 1e-6,
            plateau_patience: 2,
            last_metric: None,
            best_metric: None,
            plateau_count: 0,
            satisfied: false,
            measurements: 0,
        };
        assert!(!g.evaluate(0.5));
        assert!(!g.evaluate(0.5)); // no improvement
        assert!(g.evaluate(0.85)); // met
        assert!(g.satisfied);
    }

    #[test]
    fn extract_nested_metric() {
        let v = serde_json::json!({"result": {"sap": {"score": 0.91}}});
        assert_eq!(extract_metric(&v, "result.sap.score"), Some(0.91));
        assert_eq!(extract_metric(&v, "result.missing"), None);
    }
}
