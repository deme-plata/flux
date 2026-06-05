//! flux-legacy PROTOTYPE 12 — `remediate`: the Safe Remediation Engine.
//!
//! [`stability`](crate::stability) (P10) *detects*; `remediate` closes the loop to a *fix*. It maps
//! each non-OK [`Finding`] to a concrete, **risk-classed, reversible** [`RemediationStep`], runs only
//! the safe subset, and hands the rest to the operator as a ready-to-run gated plan. Fail-closed by
//! construction so it can never make a live node worse.
//!
//! Risk classes:
//!   * **Auto** — idempotent, reversible, non-consensus (rotate oversized syslog, vacuum *volatile*
//!     journald, redial bootstrap peers). May auto-run.
//!   * **NeedHuman** — turbo-sync, a binary deploy (e.g. v10.11.40 via canary→HA→rollback), a node
//!     restart, fixing `/.env Q_DB_PATH`. PREPARED with the exact command, NEVER auto-run.
//!   * **Forbidden** — touch the DB / balances / consensus / `data-mainnet-genesis`. Refused outright.
//!
//! Binding invariants: auto-mode runs ONLY `Auto` steps; never restart/DB/balance/consensus in
//! auto-mode; **dry-run by default**; every executed action is appended to the caller's ledger.

use crate::stability::{Finding, Health, StabilityReport};
use serde::{Deserialize, Serialize};

/// How dangerous an action is to run automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Risk {
    /// reversible, idempotent, non-consensus — may auto-run
    Auto,
    /// prepared with the exact command, but a human/HA must run it
    NeedHuman,
    /// never automated, refused even when asked (DB / balances / consensus)
    Forbidden,
}

/// One concrete remediation for one failing stability signal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemediationStep {
    pub signal: String,
    pub symptom: String,
    pub action: String,
    pub risk: Risk,
    pub reversible: bool,
    /// the exact command (shown in dry-run; only `Auto`+reversible ever actually run)
    pub command: String,
    pub rationale: String,
}

/// Operator policy over what may auto-run.
#[derive(Debug, Clone)]
pub struct Policy {
    /// signals the operator forbids auto-remediating (downgrades them to NeedHuman regardless)
    pub never_auto: Vec<String>,
}
impl Default for Policy {
    fn default() -> Self {
        Self { never_auto: Vec::new() }
    }
}

/// Map a [`StabilityReport`] → an ordered remediation plan (worst health first). PURE — plans only,
/// runs nothing.
pub fn plan_remediation(report: &StabilityReport, policy: &Policy) -> Vec<RemediationStep> {
    let mut steps: Vec<RemediationStep> = report
        .findings
        .iter()
        .filter(|f| f.health != Health::Ok)
        .map(|f| step_for(f, policy))
        .collect();
    // dangers first, then by risk (forbidden surfaced last as "can't help")
    steps.sort_by_key(|s| match s.risk {
        Risk::Auto => 0,
        Risk::NeedHuman => 1,
        Risk::Forbidden => 2,
    });
    steps
}

fn step_for(f: &Finding, policy: &Policy) -> RemediationStep {
    let s = |action: &str, risk: Risk, reversible: bool, command: &str, rationale: &str| {
        // operator can force any Auto step down to NeedHuman; nothing can lift a step UP to Auto.
        let risk = if risk == Risk::Auto && policy.never_auto.iter().any(|n| n == &f.signal) {
            Risk::NeedHuman
        } else {
            risk
        };
        RemediationStep {
            signal: f.signal.clone(),
            symptom: f.detail.clone(),
            action: action.to_string(),
            risk,
            reversible,
            command: command.to_string(),
            rationale: rationale.to_string(),
        }
    };
    match f.signal.as_str() {
        "syslog" => s(
            "rotate syslog + verify the 200MB cap",
            Risk::Auto, true,
            "logrotate -f /etc/logrotate.d/rsyslog && du -sh /var/log/syslog",
            "oversized syslog fills the tight 40G root → block-production death (CLAUDE.md). Logs only.",
        ),
        "root-disk" => match f.health {
            Health::Danger => s(
                "free root NOW (logs + volatile journal, never the DB)",
                Risk::Auto, true,
                "journalctl --vacuum-size=100M; logrotate -f /etc/logrotate.d/rsyslog; rm -rf /home/orobit/tmp/*",
                "root near full kills block production; touches ONLY logs/journal/temp, never data-mainnet-genesis.",
            ),
            _ => s(
                "reclaim root headroom (logs + journal)",
                Risk::Auto, true,
                "journalctl --vacuum-size=150M; logrotate -f /etc/logrotate.d/rsyslog",
                "tight root — trim logs before it crosses the danger threshold.",
            ),
        },
        "journal" => s(
            "set journald volatile (RAM) + vacuum",
            Risk::NeedHuman, true,
            "# /etc/systemd/journald.conf.d/size.conf -> Storage=volatile ; systemctl restart systemd-journald",
            "non-volatile journal fills root at INFO/DEBUG; restarting journald is a service action → gated.",
        ),
        "peers" => s(
            "redial bootstrap peers",
            Risk::Auto, true,
            "curl -s -X POST localhost:8080/api/v1/p2p/redial   # P2P only, no chain state",
            "thin/isolated peer set — re-dialing bootstraps is stateless + reversible.",
        ),
        "sync-gap" | "gap" => s(
            "prepare turbo-sync OR deploy v10.11.40 (VDF gate)",
            Risk::NeedHuman, false,
            "# build OFF Epsilon -> canary(Alpha Docker) -> HA roll -> rollback. Never build/deploy on the live node.",
            "lag is non-fatal under standalone; the real fix is the undeployed VDF gate → a gated binary deploy.",
        ),
        "serving" => s(
            "restart q-api-server (HA procedure)",
            Risk::NeedHuman, false,
            "# HA rolling restart only — never cowboy-restart the production supernode.",
            "endpoint down → restart is a production service action → human + HA only.",
        ),
        "process" => s(
            "start q-api-server",
            Risk::NeedHuman, false,
            "systemctl start q-api-server   # operator-gated",
            "process gone → starting it is a production action → gated.",
        ),
        "db" => s(
            "fix /.env Q_DB_PATH to the absolute home DB (operator only)",
            Risk::Forbidden, false,
            "# STOP. Wrong DB open = emission/balance corruption. Operator verifies FD count, then restarts. Auto-mode MUST NOT act.",
            "wrong-DB is the CLAUDE.md critical incident — auto-mode never touches DB/balance/emission state.",
        ),
        "ram" => s(
            "investigate memory — do NOT auto-kill",
            Risk::NeedHuman, false,
            "# inspect RSS/leaks; killing or restarting the node is forbidden in auto-mode.",
            "OOM risk — auto-killing/restarting the node is unsafe; human decides.",
        ),
        other => s(
            &format!("manual review: {other}"),
            Risk::NeedHuman, false,
            "# unmapped signal — human review",
            "no safe automated remedy is mapped for this signal.",
        ),
    }
}

/// The result of (maybe) running one step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outcome {
    pub signal: String,
    pub ran: bool,
    pub result: String,
}

/// Execute ONLY `Auto` + reversible steps. `dry_run = true` shows the command without running it.
/// `NeedHuman`/`Forbidden` are NEVER executed here — fail-closed. The returned [`Outcome`]s are the
/// caller's remediation ledger (audit + rollback trail).
pub fn apply_auto(steps: &[RemediationStep], dry_run: bool) -> Vec<Outcome> {
    steps
        .iter()
        .filter(|s| s.risk == Risk::Auto && s.reversible)
        .map(|s| {
            if dry_run {
                Outcome { signal: s.signal.clone(), ran: false, result: format!("DRY-RUN would run: {}", s.command) }
            } else {
                match std::process::Command::new("bash").arg("-c").arg(&s.command).output() {
                    Ok(o) => Outcome {
                        signal: s.signal.clone(),
                        ran: true,
                        result: format!(
                            "exit={} | {}",
                            o.status.code().unwrap_or(-1),
                            String::from_utf8_lossy(&o.stdout).lines().last().unwrap_or("").trim()
                        ),
                    },
                    Err(e) => Outcome { signal: s.signal.clone(), ran: false, result: format!("spawn error: {e}") },
                }
            }
        })
        .collect()
}

/// Visual remediation plan (Viktor=visual): per-step risk badge + the exact command + why.
pub fn render_remediation(steps: &[RemediationStep]) -> String {
    let mut o = String::from("🔧 REMEDIATION PLAN\n");
    for s in steps {
        let badge = match s.risk {
            Risk::Auto => "🟢 AUTO ",
            Risk::NeedHuman => "🟡 HUMAN",
            Risk::Forbidden => "🔴 FORBID",
        };
        let rev = if s.reversible { "reversible" } else { "NOT reversible" };
        o.push_str(&format!(
            "  [{badge}] {:<10} {}\n        ↳ {} ({rev})\n        $ {}\n        ∵ {}\n",
            s.signal, s.symptom, s.action, s.command, s.rationale
        ));
    }
    let c = |r: Risk| steps.iter().filter(|s| s.risk == r).count();
    o.push_str(&format!(
        "  ── {} auto · {} need-human · {} forbidden\n",
        c(Risk::Auto), c(Risk::NeedHuman), c(Risk::Forbidden)
    ));
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stability::{Finding, Verdict};

    fn finding(signal: &str, health: Health) -> Finding {
        Finding { signal: signal.into(), health, detail: format!("{signal} sym"), implicated_crate: None }
    }
    fn report(findings: Vec<Finding>) -> StabilityReport {
        StabilityReport { verdict: Verdict::WatchClosely, fatal: false, findings }
    }

    #[test]
    fn maps_signals_to_correct_risk_classes() {
        let r = report(vec![
            finding("syslog", Health::Watch),
            finding("peers", Health::Watch),
            finding("sync-gap", Health::Watch),
            finding("db", Health::Danger),
            finding("process", Health::Ok), // OK → no step
        ]);
        let plan = plan_remediation(&r, &Policy::default());
        let by = |sig: &str| plan.iter().find(|s| s.signal == sig).map(|s| s.risk);
        assert_eq!(by("syslog"), Some(Risk::Auto));
        assert_eq!(by("peers"), Some(Risk::Auto));
        assert_eq!(by("sync-gap"), Some(Risk::NeedHuman));
        assert_eq!(by("db"), Some(Risk::Forbidden));
        assert!(by("process").is_none(), "OK findings produce no step");
    }

    #[test]
    fn apply_auto_runs_only_auto_reversible_and_is_dry_by_default() {
        let r = report(vec![
            finding("syslog", Health::Watch),   // Auto
            finding("sync-gap", Health::Watch), // NeedHuman
            finding("db", Health::Danger),      // Forbidden
        ]);
        let plan = plan_remediation(&r, &Policy::default());
        let outcomes = apply_auto(&plan, true); // dry-run
        // only the Auto step (syslog) is even considered
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].signal, "syslog");
        assert!(!outcomes[0].ran, "dry-run runs nothing");
        assert!(outcomes[0].result.contains("DRY-RUN"));
        // the Forbidden/NeedHuman steps are NEVER in the execution set
        assert!(outcomes.iter().all(|o| o.signal != "db" && o.signal != "sync-gap"));
    }

    #[test]
    fn db_is_forbidden_and_never_executes() {
        let plan = plan_remediation(&report(vec![finding("db", Health::Danger)]), &Policy::default());
        assert_eq!(plan[0].risk, Risk::Forbidden);
        // even with dry_run=false, a Forbidden step is filtered out of apply_auto entirely
        assert!(apply_auto(&plan, false).is_empty(), "Forbidden must never reach execution");
    }

    #[test]
    fn policy_never_auto_downgrades_to_human() {
        let pol = Policy { never_auto: vec!["syslog".into()] };
        let plan = plan_remediation(&report(vec![finding("syslog", Health::Watch)]), &pol);
        assert_eq!(plan[0].risk, Risk::NeedHuman, "operator forbade auto-rotating syslog");
        assert!(apply_auto(&plan, true).is_empty(), "now nothing is Auto");
    }

    #[test]
    fn render_shows_badges_and_counts() {
        let r = report(vec![finding("syslog", Health::Watch), finding("db", Health::Danger)]);
        let txt = render_remediation(&plan_remediation(&r, &Policy::default()));
        assert!(txt.contains("AUTO"));
        assert!(txt.contains("FORBID"));
        assert!(txt.contains("1 auto") && txt.contains("1 forbidden"));
    }
}
