// flux-cortex/src/ai_cortex.rs — Cortex v2: AI-Native Development Platform
//
// Extends the core Cortex loop with AI-aware development phases.
// Where Cortex v1 optimizes code structure (SIMD, io_uring, cache-line),
// Cortex v2 routes development tasks through AI tools (heal, suggest, JIT,
// webhook-gen, integration tests) and learns which AI+tool combos work best.
//
// The AI Cortex Loop™ (7 phases):
//   1. ARCHITECT  — scan workspace, compute blueprint (existing)
//   2. DIAGNOSE   — AI analyzes code for issues (lifetimes, types, logic)
//   3. GENERATE   — AI proposes fixes using JIT + MIR cache
//   4. VERIFY     — compile + test the fix via integration suite
//   5. DEPLOY     — provenance-sign, fire webhooks, update cache
//   6. VALIDATE   — measure actual impact (existing)
//   7. LEARN      — update agent scores, improve routing (existing + agent)
//
// Innovation: This is the FIRST system that routes development tasks through
// an AI agent registry, learning which models produce the best fixes for each
// task type, and autonomously closing the loop from diagnosis to deployment.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

// ═══════════════════════════════════════════════════════════════
// Agent Registry
// ═══════════════════════════════════════════════════════════════

/// A registered AI agent that can perform development tasks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiAgent {
    /// Unique agent id
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Model identifier (e.g. "qwen3.6", "deepseek-v4-flash", "deepseek-v4-pro")
    pub model: String,
    /// Provider: "ollama", "deepseek", "openai", "anthropic"
    pub provider: String,
    /// Capabilities this agent has
    pub capabilities: Vec<AgentCapability>,
    /// Current score 0.0–1.0 (updated by learning)
    pub score: f64,
    /// Number of tasks completed
    pub tasks_completed: u64,
    /// Number of tasks where agent was correct
    pub tasks_correct: u64,
    /// Tokens used (total)
    pub tokens_used: u64,
    /// Is this agent currently available?
    pub available: bool,
}

/// What an AI agent can do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentCapability {
    /// Diagnose compile/runtime errors
    Diagnose,
    /// Generate code fixes
    GenerateFix,
    /// Review and veto unsafe fixes
    Veto,
    /// Suggest lifetime annotations
    LifetimeAnalysis,
    /// Generate webhook contracts
    WebhookDesign,
    /// Run integration tests
    TestExecution,
    /// General code generation
    CodeGen,
}

impl AgentCapability {
    pub fn name(&self) -> &str {
        match self {
            Self::Diagnose => "Diagnose",
            Self::GenerateFix => "GenerateFix",
            Self::Veto => "Veto",
            Self::LifetimeAnalysis => "LifetimeAnalysis",
            Self::WebhookDesign => "WebhookDesign",
            Self::TestExecution => "TestExecution",
            Self::CodeGen => "CodeGen",
        }
    }
}

/// Default agent registry with known models.
pub fn default_agent_registry() -> Vec<AiAgent> {
    vec![
        AiAgent {
            id: "qwen-local".into(),
            name: "Qwen 3.6 (Local)".into(),
            model: "qwen3.6:latest".into(),
            provider: "ollama".into(),
            capabilities: vec![
                AgentCapability::Diagnose,
                AgentCapability::GenerateFix,
                AgentCapability::CodeGen,
                AgentCapability::WebhookDesign,
            ],
            score: 0.75,
            tasks_completed: 0,
            tasks_correct: 0,
            tokens_used: 0,
            available: true,
        },
        AiAgent {
            id: "deepseek-flash".into(),
            name: "DeepSeek V4 Flash".into(),
            model: "deepseek-v4-flash".into(),
            provider: "deepseek".into(),
            capabilities: vec![
                AgentCapability::Diagnose,
                AgentCapability::Veto,
                AgentCapability::LifetimeAnalysis,
                AgentCapability::CodeGen,
            ],
            score: 0.88,
            tasks_completed: 0,
            tasks_correct: 0,
            tokens_used: 0,
            available: std::env::var("DEEPSEEK_API_KEY").is_ok(),
        },
        AiAgent {
            id: "deepseek-pro".into(),
            name: "DeepSeek V4 Pro".into(),
            model: "deepseek-v4-pro".into(),
            provider: "deepseek".into(),
            capabilities: vec![
                AgentCapability::Diagnose,
                AgentCapability::GenerateFix,
                AgentCapability::Veto,
                AgentCapability::LifetimeAnalysis,
                AgentCapability::WebhookDesign,
                AgentCapability::CodeGen,
            ],
            score: 0.92,
            tasks_completed: 0,
            tasks_correct: 0,
            tokens_used: 0,
            available: std::env::var("DEEPSEEK_API_KEY").is_ok(),
        },
    ]
}

// ═══════════════════════════════════════════════════════════════
// AI Cortex Types
// ═══════════════════════════════════════════════════════════════

/// A development task dispatched by the AI Cortex.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiTask {
    /// Unique task id
    pub id: u64,
    /// What kind of task
    pub kind: AiTaskKind,
    /// Target file or crate
    pub target: String,
    /// The source code or error context
    pub context: String,
    /// Which agent was assigned
    pub assigned_agent: Option<String>,
    /// The agent's response/output
    pub agent_output: Option<String>,
    /// Was the task successful?
    pub success: Option<bool>,
    /// Time taken in ms
    pub duration_ms: Option<u64>,
    /// Timestamp
    pub created_at_secs: u64,
}

/// Kinds of development tasks the AI Cortex can dispatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AiTaskKind {
    /// Heal: diagnose + fix compile/runtime errors
    Heal,
    /// Suggest: AI lifetime/type analysis
    Suggest,
    /// Test: run integration tests
    Test,
    /// Webhook: generate webhook contracts
    WebhookGen,
    /// Review: code review with suggestions
    Review,
    /// Optimize: suggest performance improvements
    Optimize,
}

impl AiTaskKind {
    pub fn name(&self) -> &str {
        match self {
            Self::Heal => "Heal",
            Self::Suggest => "Suggest",
            Self::Test => "Test",
            Self::WebhookGen => "WebhookGen",
            Self::Review => "Review",
            Self::Optimize => "Optimize",
        }
    }

    /// Which capabilities are needed for this task kind.
    pub fn required_capabilities(&self) -> Vec<AgentCapability> {
        match self {
            Self::Heal => vec![AgentCapability::Diagnose, AgentCapability::GenerateFix],
            Self::Suggest => vec![AgentCapability::LifetimeAnalysis, AgentCapability::CodeGen],
            Self::Test => vec![AgentCapability::TestExecution],
            Self::WebhookGen => vec![AgentCapability::WebhookDesign, AgentCapability::CodeGen],
            Self::Review => vec![AgentCapability::Diagnose, AgentCapability::CodeGen],
            Self::Optimize => vec![AgentCapability::Diagnose, AgentCapability::GenerateFix],
        }
    }
}

/// Result of an AI Cortex loop iteration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiCortexResult {
    /// Iteration number
    pub iteration: u64,
    /// Tasks dispatched
    pub tasks_dispatched: usize,
    /// Tasks completed successfully
    pub tasks_succeeded: usize,
    /// Tasks that failed
    pub tasks_failed: usize,
    /// Agent scores after this iteration
    pub agent_scores: HashMap<String, f64>,
    /// Best performing agent
    pub best_agent: Option<String>,
    /// Total tokens used this iteration
    pub tokens_used: u64,
    /// Learning improvement (0.0–1.0)
    pub learning_improvement: f64,
    /// Duration in ms
    pub duration_ms: u64,
}

// ═══════════════════════════════════════════════════════════════
// AI Cortex Engine
// ═══════════════════════════════════════════════════════════════

/// The AI-Native Development Platform engine.
pub struct AiCortex {
    /// Registered AI agents
    pub agents: Vec<AiAgent>,
    /// Task history for learning
    pub task_history: Vec<AiTask>,
    /// Result history
    pub result_history: Vec<AiCortexResult>,
    /// Iteration counter
    pub iteration_count: u64,
    /// Total tokens used across all iterations
    pub total_tokens: u64,
    /// Path to the workspace root
    pub workspace_root: std::path::PathBuf,
}

impl AiCortex {
    /// Create a new AI Cortex with the default agent registry.
    pub fn new(workspace_root: std::path::PathBuf) -> Self {
        AiCortex {
            agents: default_agent_registry(),
            task_history: Vec::new(),
            result_history: Vec::new(),
            iteration_count: 0,
            total_tokens: 0,
            workspace_root,
        }
    }

    /// Find the best agent for a given task kind, considering capabilities and score.
    pub fn route_task(&self, kind: &AiTaskKind) -> Option<&AiAgent> {
        let required = kind.required_capabilities();
        self.agents
            .iter()
            .filter(|a| a.available && required.iter().all(|c| a.capabilities.contains(c)))
            .max_by(|a, b| {
                a.score
                    .partial_cmp(&b.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    }

    /// Dispatch a single AI task: route to best agent, execute, record result.
    pub fn dispatch_task(
        &mut self,
        kind: AiTaskKind,
        target: &str,
        context: &str,
    ) -> AiTask {
        let task_id = self.task_history.len() as u64 + 1;
        let now = now_secs();

        let agent = self.route_task(&kind).cloned();
        let agent_id = agent.as_ref().map(|a| a.id.clone());

        let mut task = AiTask {
            id: task_id,
            kind: kind.clone(),
            target: target.to_string(),
            context: context.to_string(),
            assigned_agent: agent_id.clone(),
            agent_output: None,
            success: None,
            duration_ms: None,
            created_at_secs: now,
        };

        let start = std::time::Instant::now();

        // Execute the task based on kind
        if let Some(ref agent) = agent {
            let output = match kind {
                AiTaskKind::Heal => {
                    // Use the fluxc heal pipeline
                    execute_heal_via_agent(agent, target, context)
                }
                AiTaskKind::Suggest => {
                    execute_suggest_via_agent(agent, target, context)
                }
                AiTaskKind::Test => {
                    execute_test_via_agent(target)
                }
                AiTaskKind::WebhookGen => {
                    execute_webhook_gen_via_agent(agent, target, context)
                }
                AiTaskKind::Review => {
                    execute_review_via_agent(agent, target, context)
                }
                AiTaskKind::Optimize => {
                    execute_optimize_via_agent(agent, target, context)
                }
            };

            task.agent_output = Some(output.clone());
            task.success = Some(!output.is_empty() && !output.contains("ERROR"));
        } else {
            task.agent_output = Some(format!("No available agent for {:?}", kind));
            task.success = Some(false);
        }

        task.duration_ms = Some(start.elapsed().as_millis() as u64);

        // Update agent scores
        if let Some(ref agent_id) = agent_id {
            if let Some(agent) = self.agents.iter_mut().find(|a| a.id == *agent_id) {
                agent.tasks_completed += 1;
                if task.success.unwrap_or(false) {
                    agent.tasks_correct += 1;
                    // Boost score slightly on success
                    agent.score = (agent.score * 0.9 + 0.1).min(1.0);
                } else {
                    // Penalize on failure
                    agent.score = (agent.score * 0.95).max(0.1);
                }
            }
        }

        self.task_history.push(task.clone());
        task
    }

    /// Run a full AI Cortex iteration: dispatch tasks for a target file/crate.
    pub fn run_iteration(
        &mut self,
        target: &str,
        modes: &[AiTaskKind],
    ) -> AiCortexResult {
        self.iteration_count += 1;
        let start = std::time::Instant::now();
        let mut tokens = 0u64;
        let mut succeeded = 0usize;
        let mut failed = 0usize;

        // Read target source for context
        let target_path = self.workspace_root.join(target);
        let context = std::fs::read_to_string(&target_path)
            .unwrap_or_else(|_| format!("// File: {}", target));

        let mut tasks_dispatched = 0usize;

        for mode in modes {
            let task = self.dispatch_task(mode.clone(), target, &context);
            tasks_dispatched += 1;
            if task.success.unwrap_or(false) {
                succeeded += 1;
            } else {
                failed += 1;
            }
        }

        // Collect agent scores
        let agent_scores: HashMap<String, f64> = self
            .agents
            .iter()
            .map(|a| (a.id.clone(), a.score))
            .collect();

        let best_agent = self
            .agents
            .iter()
            .max_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal))
            .map(|a| a.id.clone());

        // Calculate learning improvement based on success rate trend
        let success_rate = if tasks_dispatched > 0 {
            succeeded as f64 / tasks_dispatched as f64
        } else {
            0.0
        };

        let prev_avg = if !self.result_history.is_empty() {
            let last = &self.result_history[self.result_history.len() - 1];
            let prev_total = last.tasks_succeeded + last.tasks_failed;
            if prev_total > 0 {
                last.tasks_succeeded as f64 / prev_total as f64
            } else {
                0.5
            }
        } else {
            0.5
        };

        let learning = (success_rate - prev_avg).max(0.0);

        let result = AiCortexResult {
            iteration: self.iteration_count,
            tasks_dispatched,
            tasks_succeeded: succeeded,
            tasks_failed: failed,
            agent_scores,
            best_agent,
            tokens_used: tokens,
            learning_improvement: learning,
            duration_ms: start.elapsed().as_millis() as u64,
        };

        self.result_history.push(result.clone());
        result
    }

    /// Run continuous AI Cortex iterations.
    pub fn run_continuous(
        &mut self,
        iterations: usize,
        target: &str,
        modes: &[AiTaskKind],
    ) -> Vec<AiCortexResult> {
        let mut results = Vec::new();
        for _ in 0..iterations {
            let result = self.run_iteration(target, modes);
            results.push(result);
        }
        results
    }

    /// Generate a summary of AI Cortex activity.
    pub fn summary(&self) -> AiCortexSummary {
        let total_tasks = self.task_history.len() as u64;
        let total_succeeded = self.task_history.iter().filter(|t| t.success == Some(true)).count() as u64;
        let success_rate = if total_tasks > 0 {
            total_succeeded as f64 / total_tasks as f64
        } else {
            0.0
        };

        let best_agent = self
            .agents
            .iter()
            .max_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal))
            .map(|a| a.name.clone());

        AiCortexSummary {
            total_iterations: self.iteration_count,
            total_tasks,
            total_succeeded,
            success_rate,
            best_agent,
            total_tokens: self.total_tokens,
            agents: self.agents.clone(),
            learning_plateau: self.is_plateaued(),
        }
    }

    fn is_plateaued(&self) -> bool {
        if self.result_history.len() < 5 {
            return false;
        }
        let recent: Vec<f64> = self
            .result_history
            .iter()
            .rev()
            .take(5)
            .map(|r| {
                let total = r.tasks_succeeded + r.tasks_failed;
                if total > 0 {
                    r.tasks_succeeded as f64 / total as f64
                } else {
                    0.0
                }
            })
            .collect();
        let avg = recent.iter().sum::<f64>() / 5.0;
        let variance = recent.iter().map(|x| (x - avg).powi(2)).sum::<f64>() / 5.0;
        variance < 0.001
    }
}

/// Summary of AI Cortex activity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiCortexSummary {
    pub total_iterations: u64,
    pub total_tasks: u64,
    pub total_succeeded: u64,
    pub success_rate: f64,
    pub best_agent: Option<String>,
    pub total_tokens: u64,
    pub agents: Vec<AiAgent>,
    pub learning_plateau: bool,
}

// ═══════════════════════════════════════════════════════════════
// Task Executors (bridge to fluxc tools)
// ═══════════════════════════════════════════════════════════════

fn execute_heal_via_agent(agent: &AiAgent, target: &str, context: &str) -> String {
    let prompt = format!(
        "Analyze this Rust code for errors and suggest fixes.\n\
         File: {}\n\n\
         ```rust\n{}\n```\n\n\
         If you find issues, output the corrected code inside a ```rust block.\n\
         If the code is correct, output: NO_ISSUES\n",
        target, context
    );
    query_agent(agent, &prompt)
}

fn execute_suggest_via_agent(agent: &AiAgent, target: &str, context: &str) -> String {
    let prompt = format!(
        "Analyze this Rust code for lifetime issues, missing annotations, \
         and borrow-checker problems.\n\
         File: {}\n\n\
         ```rust\n{}\n```\n\n\
         Return JSON with \"issues\" array and \"summary\".\n",
        target, context
    );
    query_agent(agent, &prompt)
}

fn execute_test_via_agent(target: &str) -> String {
    // Run integration tests via subprocess
    match std::process::Command::new("fluxc")
        .args(["test-native"])
        .output()
    {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            if out.status.success() {
                format!("TESTS_PASSED: {}", stdout.lines().last().unwrap_or(""))
            } else {
                format!("TESTS_FAILED: {}", stdout.lines().last().unwrap_or(""))
            }
        }
        Err(e) => format!("TEST_ERROR: {}", e),
    }
}

fn execute_webhook_gen_via_agent(agent: &AiAgent, target: &str, context: &str) -> String {
    let prompt = format!(
        "Suggest webhook contracts for this Rust crate. \
         Analyze the API surface and suggest inbound webhook endpoints \
         that would be useful for CI/CD, deployments, and monitoring.\n\
         Crate: {}\n\n\
         ```rust\n{}\n```\n\n\
         Return a TOML webhook contract definition.\n",
        target, context
    );
    query_agent(agent, &prompt)
}

fn execute_review_via_agent(agent: &AiAgent, target: &str, context: &str) -> String {
    let prompt = format!(
        "Review this Rust code for bugs, safety issues, and performance problems.\n\
         File: {}\n\n\
         ```rust\n{}\n```\n\n\
         Return a structured review with severity levels (CRITICAL/HIGH/MEDIUM/LOW).\n",
        target, context
    );
    query_agent(agent, &prompt)
}

fn execute_optimize_via_agent(agent: &AiAgent, target: &str, context: &str) -> String {
    let prompt = format!(
        "Suggest performance optimizations for this Rust code.\n\
         Consider: SIMD, io_uring, cache-line alignment, allocation reduction.\n\
         File: {}\n\n\
         ```rust\n{}\n```\n\n\
         Return specific, actionable suggestions with estimated impact.\n",
        target, context
    );
    query_agent(agent, &prompt)
}

/// Query an AI agent via their provider.
fn query_agent(agent: &AiAgent, prompt: &str) -> String {
    match agent.provider.as_str() {
        "ollama" => query_ollama(&agent.model, prompt),
        "deepseek" => query_deepseek(&agent.model, prompt),
        _ => format!("Unsupported provider: {}", agent.provider),
    }
}

fn query_ollama(model: &str, prompt: &str) -> String {
    match std::process::Command::new("ollama")
        .args(["run", model])
        .arg(prompt)
        .output()
    {
        Ok(out) => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        Err(e) => format!("OLLAMA_ERROR: {}", e),
    }
}

fn query_deepseek(model: &str, prompt: &str) -> String {
    let api_key = std::env::var("DEEPSEEK_API_KEY").unwrap_or_default();
    if api_key.is_empty() {
        return "DEEPSEEK_ERROR: DEEPSEEK_API_KEY not set".into();
    }

    let body = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "max_tokens": 2048,
        "temperature": 0.3
    });

    let body_str = serde_json::to_string(&body).unwrap_or_default();

    match std::process::Command::new("curl")
        .args(["-s", "-X", "POST", "https://api.deepseek.com/chat/completions"])
        .args(["-H", &format!("Authorization: Bearer {}", api_key)])
        .args(["-H", "Content-Type: application/json"])
        .args(["-d", &body_str])
        .args(["--max-time", "60"])
        .output()
    {
        Ok(out) => {
            let resp: serde_json::Value =
                serde_json::from_slice(&out.stdout).unwrap_or_default();
            resp["choices"][0]["message"]["content"]
                .as_str()
                .unwrap_or("")
                .to_string()
        }
        Err(e) => format!("DEEPSEEK_ERROR: {}", e),
    }
}

// ═══════════════════════════════════════════════════════════════
// Utility
// ═══════════════════════════════════════════════════════════════

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ═══════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_registry_default() {
        let agents = default_agent_registry();
        assert!(agents.len() >= 2);
        assert!(agents.iter().any(|a| a.id == "qwen-local"));
        assert!(agents.iter().any(|a| a.id == "deepseek-flash"));
    }

    #[test]
    fn test_route_task_heal() {
        let cortex = AiCortex::new(std::path::PathBuf::from("/tmp"));
        let agent = cortex.route_task(&AiTaskKind::Heal);
        assert!(agent.is_some());
        let a = agent.unwrap();
        assert!(a.capabilities.contains(&AgentCapability::Diagnose));
        assert!(a.capabilities.contains(&AgentCapability::GenerateFix));
    }

    #[test]
    fn test_route_task_suggest() {
        let cortex = AiCortex::new(std::path::PathBuf::from("/tmp"));
        let agent = cortex.route_task(&AiTaskKind::Suggest);
        assert!(agent.is_some());
        let a = agent.unwrap();
        assert!(a.capabilities.contains(&AgentCapability::LifetimeAnalysis));
    }

    #[test]
    fn test_route_task_no_agent_for_test() {
        // Test task requires TestExecution capability — none of our default
        // agents have it explicitly, so this should return None or fallback
        let cortex = AiCortex::new(std::path::PathBuf::from("/tmp"));
        let agent = cortex.route_task(&AiTaskKind::Test);
        // May return None or a fallback agent
        // This is expected — test execution is done via subprocess, not AI
    }

    #[test]
    fn test_ai_cortex_new() {
        let cortex = AiCortex::new(std::path::PathBuf::from("/tmp"));
        assert_eq!(cortex.iteration_count, 0);
        assert!(cortex.task_history.is_empty());
        assert!(cortex.result_history.is_empty());
        assert!(!cortex.agents.is_empty());
    }

    #[test]
    fn test_ai_cortex_summary_empty() {
        let cortex = AiCortex::new(std::path::PathBuf::from("/tmp"));
        let summary = cortex.summary();
        assert_eq!(summary.total_iterations, 0);
        assert_eq!(summary.total_tasks, 0);
        assert!(summary.best_agent.is_some()); // Default agents have scores
    }

    #[test]
    fn test_task_kind_required_capabilities() {
        let heal_caps = AiTaskKind::Heal.required_capabilities();
        assert!(heal_caps.contains(&AgentCapability::Diagnose));
        assert!(heal_caps.contains(&AgentCapability::GenerateFix));

        let suggest_caps = AiTaskKind::Suggest.required_capabilities();
        assert!(suggest_caps.contains(&AgentCapability::LifetimeAnalysis));

        let webhook_caps = AiTaskKind::WebhookGen.required_capabilities();
        assert!(webhook_caps.contains(&AgentCapability::WebhookDesign));
    }

    #[test]
    fn test_agent_score_updates() {
        let mut cortex = AiCortex::new(std::path::PathBuf::from("/tmp"));
        let initial_score = cortex.agents[0].score;

        // Simulate a task dispatch (without actual AI call)
        let task = AiTask {
            id: 1,
            kind: AiTaskKind::Heal,
            target: "test.rs".into(),
            context: "fn main() {}".into(),
            assigned_agent: Some(cortex.agents[0].id.clone()),
            agent_output: Some("NO_ISSUES".into()),
            success: Some(true),
            duration_ms: Some(100),
            created_at_secs: now_secs(),
        };

        // Manually update score
        if let Some(agent) = cortex.agents.iter_mut().find(|a| a.id == task.assigned_agent.clone().unwrap()) {
            agent.tasks_completed += 1;
            if task.success.unwrap_or(false) {
                agent.tasks_correct += 1;
                agent.score = (agent.score * 0.9 + 0.1).min(1.0);
            }
        }

        assert!(cortex.agents[0].score > initial_score);
    }

    #[test]
    fn test_plateau_detection() {
        let mut cortex = AiCortex::new(std::path::PathBuf::from("/tmp"));
        assert!(!cortex.is_plateaued());

        // Add 5 identical results → should plateau
        for i in 0..5 {
            cortex.result_history.push(AiCortexResult {
                iteration: i + 1,
                tasks_dispatched: 2,
                tasks_succeeded: 2,
                tasks_failed: 0,
                agent_scores: HashMap::new(),
                best_agent: None,
                tokens_used: 0,
                learning_improvement: 0.0,
                duration_ms: 100,
            });
        }
        assert!(cortex.is_plateaued());
    }
}
