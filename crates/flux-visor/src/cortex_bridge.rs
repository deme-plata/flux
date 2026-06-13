//! FluxVisor ↔ Flux Cortex platform-dev bridge.
//!
//! This module maps FluxVisor *hosting* development tasks onto the Flux Cortex
//! AI-native development vocabulary (`flux_cortex::ai_cortex::AiTaskKind`) so the
//! follow-up board (executor design, heartbeat daemon, security runbook,
//! billing/abuse webhooks) can be reasoned about with the same engine that
//! optimizes the rest of the Flux tree.
//!
//! ## What this bridge deliberately does NOT do
//!
//! It produces **reviewable, deterministic plans only**. It never:
//!
//! - dispatches an AI agent,
//! - reads or writes host files,
//! - mutates a host, firewall, or service,
//! - touches `virsh`, `qemu`, or libvirt.
//!
//! Every plan it emits is [`DispatchMode::DryRun`] and carries [`PlanGuards`]
//! that assert no mutation. A real (`Live`) dispatch lane is reserved in the
//! type system but is intentionally not constructible here — enabling it is a
//! separate, operator-approved step. This keeps "use Cortex for hosting work"
//! safe to call while the first paid host does not yet exist.

use flux_cortex::ai_cortex::{AgentCapability, AiTaskKind};
use serde::{Deserialize, Serialize};

/// A FluxVisor hosting development task — the work items from the FluxVisor
/// follow-up board (swarm message `#220`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum HostingTask {
    /// Design the executor boundary (`DryRunExecutor` trait + future
    /// `LibvirtExecutor`). Design/critique only; no `virsh`/`qemu`.
    ExecutorDesign,
    /// Design worker capacity heartbeats over the FluxVisor capacity topics.
    HeartbeatDaemon,
    /// Threat-model the first paid alpha host (admin exposure, VM isolation,
    /// bridges, firewall, abuse desk, backups, IPv4 scarcity, resale rules).
    SecurityRunbook,
    /// Design webhook/billing/abuse hooks for provisioning + abuse events.
    BillingWebhook,
}

impl HostingTask {
    /// The four follow-up board tasks, in board order.
    pub fn board() -> &'static [HostingTask] {
        &[
            HostingTask::ExecutorDesign,
            HostingTask::HeartbeatDaemon,
            HostingTask::SecurityRunbook,
            HostingTask::BillingWebhook,
        ]
    }

    /// Stable slug used as the swarm/task identifier for this work item.
    pub fn slug(&self) -> &'static str {
        match self {
            HostingTask::ExecutorDesign => "fluxvisor-libvirt-executor-dryrun",
            HostingTask::HeartbeatDaemon => "fluxvisor-p2p-heartbeat-daemon",
            HostingTask::SecurityRunbook => "fluxhost-alpha-security-runbook",
            HostingTask::BillingWebhook => "fluxvisor-billing-abuse-webhooks",
        }
    }

    /// Human-readable title.
    pub fn title(&self) -> &'static str {
        match self {
            HostingTask::ExecutorDesign => "FluxVisor executor boundary (dry-run)",
            HostingTask::HeartbeatDaemon => "FluxVisor P2P capacity-heartbeat daemon",
            HostingTask::SecurityRunbook => "FluxHost alpha security runbook",
            HostingTask::BillingWebhook => "FluxVisor billing/abuse webhook contracts",
        }
    }

    /// The file or crate a Cortex task would read as context. These are the
    /// artifacts the work centers on — read-only inputs, never write targets.
    pub fn target(&self) -> &'static str {
        match self {
            HostingTask::ExecutorDesign => "crates/flux-visor/src/lib.rs",
            HostingTask::HeartbeatDaemon => "crates/flux-visor/src/lib.rs",
            HostingTask::SecurityRunbook => "docs/FLUXHOST_ALPHA_SECURITY.md",
            HostingTask::BillingWebhook => "crates/flux-visor/src/lib.rs",
        }
    }

    /// Why this task maps to the Cortex modes it does.
    pub fn rationale(&self) -> &'static str {
        match self {
            HostingTask::ExecutorDesign => {
                "Critique the executor trait shape (Review) and stress its \
                 per-action cost/ordering (Optimize) without running any backend."
            }
            HostingTask::HeartbeatDaemon => {
                "Validate topic naming + NetworkConfig fit against flux-p2p \
                 (Test) and review the daemon's failure modes (Review); no \
                 service is started."
            }
            HostingTask::SecurityRunbook => {
                "A threat model is a structured review of the alpha host's \
                 attack surface — maps cleanly onto Review."
            }
            HostingTask::BillingWebhook => {
                "Provisioning/abuse hooks are inbound webhook contracts — the \
                 native fit for WebhookGen."
            }
        }
    }

    /// Map this hosting task onto one or more Cortex development modes.
    ///
    /// Mappings (from the handoff): executor design → `Review` + `Optimize`;
    /// heartbeat daemon → `Test` + `Review`; security runbook → `Review`;
    /// billing/abuse webhooks → `WebhookGen`.
    pub fn cortex_modes(&self) -> Vec<AiTaskKind> {
        match self {
            HostingTask::ExecutorDesign => vec![AiTaskKind::Review, AiTaskKind::Optimize],
            HostingTask::HeartbeatDaemon => vec![AiTaskKind::Test, AiTaskKind::Review],
            HostingTask::SecurityRunbook => vec![AiTaskKind::Review],
            HostingTask::BillingWebhook => vec![AiTaskKind::WebhookGen],
        }
    }

    /// Build the reviewable, dry-run plan for this task.
    pub fn plan(&self) -> PlatformTaskPlan {
        let modes = self.cortex_modes();
        let required_capabilities = union_capabilities(&modes);
        PlatformTaskPlan {
            task: *self,
            slug: self.slug().to_string(),
            title: self.title().to_string(),
            target: self.target().to_string(),
            cortex_modes: modes,
            required_capabilities,
            rationale: self.rationale().to_string(),
            dispatch: DispatchMode::DryRun,
            guards: PlanGuards::dry_run(),
        }
    }
}

/// Whether a plan is a proposal only or an approved live dispatch.
///
/// The bridge only ever emits [`DispatchMode::DryRun`]. `Live` exists so callers
/// can pattern-match exhaustively and so a future operator-approved lane has a
/// place to land, but [`HostingTask::plan`] never constructs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DispatchMode {
    /// Reviewable proposal. No agent runs, nothing is mutated.
    DryRun,
    /// Operator-approved live dispatch. Not produced by this bridge.
    Live,
}

/// Safety guards attached to every plan. The bridge asserts all-false mutation
/// and operator-approval-required, so a reviewer can trust a plan at a glance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanGuards {
    /// Does executing this plan write files? (Always false here.)
    pub mutates_files: bool,
    /// Does executing this plan mutate a host? (Always false here.)
    pub mutates_host: bool,
    /// Does executing this plan start/restart services? (Always false here.)
    pub starts_services: bool,
    /// Must an operator approve before any live dispatch? (Always true here.)
    pub operator_approval_required: bool,
}

impl PlanGuards {
    /// The only guard set this bridge produces: nothing mutated, approval gated.
    pub fn dry_run() -> Self {
        Self {
            mutates_files: false,
            mutates_host: false,
            starts_services: false,
            operator_approval_required: true,
        }
    }

    /// True iff this plan is provably side-effect free.
    pub fn is_inert(&self) -> bool {
        !self.mutates_files && !self.mutates_host && !self.starts_services
    }
}

/// A reviewable mapping of a FluxVisor hosting task onto Cortex modes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformTaskPlan {
    /// The hosting task this plan addresses.
    pub task: HostingTask,
    /// Stable slug / swarm task id.
    pub slug: String,
    /// Human-readable title.
    pub title: String,
    /// Read-only context artifact (file or crate).
    pub target: String,
    /// Cortex development modes this task routes to, in order.
    pub cortex_modes: Vec<AiTaskKind>,
    /// Deduplicated union of capabilities the modes require.
    pub required_capabilities: Vec<AgentCapability>,
    /// Why the mapping is what it is.
    pub rationale: String,
    /// Always [`DispatchMode::DryRun`] from this bridge.
    pub dispatch: DispatchMode,
    /// Safety guards.
    pub guards: PlanGuards,
}

impl PlatformTaskPlan {
    /// True iff this plan is safe to hand to a reviewer with no risk of side
    /// effects: dry-run dispatch and inert guards.
    pub fn is_dry_run(&self) -> bool {
        matches!(self.dispatch, DispatchMode::DryRun) && self.guards.is_inert()
    }
}

/// Build the full FluxVisor follow-up board as reviewable dry-run plans.
///
/// Deterministic: the same board, in the same order, every call. No clock, no
/// randomness, no environment reads, no agent dispatch.
pub fn followup_board() -> Vec<PlatformTaskPlan> {
    HostingTask::board().iter().map(HostingTask::plan).collect()
}

/// Deduplicated union of the capabilities required by a sequence of modes,
/// preserving first-seen order for stable, deterministic output.
fn union_capabilities(modes: &[AiTaskKind]) -> Vec<AgentCapability> {
    let mut out: Vec<AgentCapability> = Vec::new();
    for mode in modes {
        for cap in mode.required_capabilities() {
            if !out.contains(&cap) {
                out.push(cap);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use flux_cortex::ai_cortex::default_agent_registry;

    #[test]
    fn board_has_four_tasks_in_order() {
        let board = followup_board();
        assert_eq!(board.len(), 4);
        assert_eq!(board[0].task, HostingTask::ExecutorDesign);
        assert_eq!(board[1].task, HostingTask::HeartbeatDaemon);
        assert_eq!(board[2].task, HostingTask::SecurityRunbook);
        assert_eq!(board[3].task, HostingTask::BillingWebhook);
    }

    #[test]
    fn executor_design_maps_to_review_and_optimize() {
        let modes = HostingTask::ExecutorDesign.cortex_modes();
        assert_eq!(modes, vec![AiTaskKind::Review, AiTaskKind::Optimize]);
    }

    #[test]
    fn heartbeat_maps_to_test_and_review() {
        let modes = HostingTask::HeartbeatDaemon.cortex_modes();
        assert_eq!(modes, vec![AiTaskKind::Test, AiTaskKind::Review]);
    }

    #[test]
    fn security_runbook_maps_to_review_only() {
        assert_eq!(
            HostingTask::SecurityRunbook.cortex_modes(),
            vec![AiTaskKind::Review]
        );
    }

    #[test]
    fn billing_webhook_maps_to_webhookgen() {
        assert_eq!(
            HostingTask::BillingWebhook.cortex_modes(),
            vec![AiTaskKind::WebhookGen]
        );
    }

    #[test]
    fn every_plan_is_dry_run_and_inert() {
        for plan in followup_board() {
            assert!(plan.is_dry_run(), "{} must be dry-run", plan.slug);
            assert_eq!(plan.dispatch, DispatchMode::DryRun);
            assert!(!plan.guards.mutates_files);
            assert!(!plan.guards.mutates_host);
            assert!(!plan.guards.starts_services);
            assert!(plan.guards.operator_approval_required);
        }
    }

    #[test]
    fn routing_is_deterministic() {
        // Same input → byte-identical plans across calls. No clock/rng/env.
        assert_eq!(followup_board(), followup_board());
    }

    #[test]
    fn required_capabilities_are_deduplicated_union() {
        // Review needs {Diagnose, CodeGen}; Optimize needs {Diagnose, GenerateFix}.
        // Diagnose appears in both but must be listed once, first-seen order.
        let plan = HostingTask::ExecutorDesign.plan();
        assert_eq!(
            plan.required_capabilities,
            vec![
                AgentCapability::Diagnose,
                AgentCapability::CodeGen,
                AgentCapability::GenerateFix,
            ]
        );
    }

    #[test]
    fn slugs_are_unique() {
        let board = followup_board();
        let mut slugs: Vec<&str> = board.iter().map(|p| p.slug.as_str()).collect();
        slugs.sort_unstable();
        slugs.dedup();
        assert_eq!(slugs.len(), board.len());
    }

    #[test]
    fn ai_routable_modes_are_satisfiable_by_the_local_agent() {
        // The always-available local agent (qwen-local) must be able to serve
        // every AI-routable mode the board uses. `Test` is intentionally
        // excluded: test execution runs via subprocess in the cortex engine,
        // not through an AI agent (no default agent has TestExecution).
        let registry = default_agent_registry();
        let local = registry
            .iter()
            .find(|a| a.id == "qwen-local")
            .expect("local agent present");
        assert!(local.available, "local agent must be available");

        for plan in followup_board() {
            for mode in &plan.cortex_modes {
                if *mode == AiTaskKind::Test {
                    continue;
                }
                for cap in mode.required_capabilities() {
                    assert!(
                        local.capabilities.contains(&cap),
                        "local agent missing {cap:?} for mode {mode:?} in {}",
                        plan.slug
                    );
                }
            }
        }
    }
}
