//! FluxHost alpha security gates (board T3).
//!
//! The companion to `docs/FLUXHOST_ALPHA_SECURITY.md`. The prose threat model
//! lives in the doc; this module encodes the **go-live checklist** as data so the
//! runbook is honest by construction: each control records whether it is
//! enforced by FluxVisor code today, is operator policy that must be configured
//! first, or is a known gap that has not been built.
//!
//! [`ready_for_paid_customers`] is `false` while any control is
//! [`ControlStatus::NotYetImplemented`] — and the alpha checklist deliberately
//! still has blockers, so the test `current_alpha_is_not_ready` asserts we are
//! not yet allowed to sell. That is the point: the gate is closed until the work
//! is done.

/// Where a control sits relative to running code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlStatus {
    /// Enforced by FluxVisor code today (a test or the type system upholds it).
    EnforcedInCode,
    /// The operator must configure this before the first paid customer; FluxVisor
    /// cannot enforce it from the control plane.
    PolicyRequired,
    /// A known gap. A host MUST NOT take paid customers while any of these apply.
    NotYetImplemented,
}

/// The threat-model areas from the FluxVisor follow-up board.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlArea {
    /// Operator/admin surface (deploy panel, q-flux admin routes, node API).
    AdminExposure,
    /// Guest isolation under the hypervisor.
    VmIsolation,
    /// Whether customer VMs share a box with production services.
    HostResidency,
    /// Guest L2/L3 networking and inter-tenant reachability.
    Networking,
    /// Host firewall posture.
    Firewall,
    /// Abuse reporting and takedown.
    AbuseDesk,
    /// Data durability and backups.
    Backups,
    /// IPv4 address scarcity.
    Ipv4Scarcity,
    /// Upstream provider resale / sub-letting terms.
    ProviderResale,
    /// Capacity correctness between plan time and provision time.
    Capacity,
    /// Immutable audit logging of host-level actions.
    Audit,
    /// Hypervisor/host vulnerability patching.
    Patching,
}

/// One go-live security control.
#[derive(Clone, Copy, Debug)]
pub struct SecurityControl {
    /// Stable id, e.g. `host-residency`.
    pub id: &'static str,
    /// Threat area.
    pub area: ControlArea,
    /// What must be true.
    pub statement: &'static str,
    /// Where it sits relative to code today.
    pub status: ControlStatus,
    /// The concrete mitigation / what to do.
    pub mitigation: &'static str,
}

/// The FluxHost alpha go-live checklist.
///
/// Grounded in the real fleet: Epsilon is the leased node box, it co-resides the
/// production Quillon node, and (as of 2026-06-13) is OOM-cycling — so it is not
/// an eligible paid host. See `docs/FLUXHOST_ALPHA_SECURITY.md` for the full
/// threat model behind each line.
pub fn alpha_security_checklist() -> Vec<SecurityControl> {
    use ControlArea::*;
    use ControlStatus::*;
    vec![
        SecurityControl {
            id: "admin-plane-unreachable-from-guests",
            area: AdminExposure,
            statement: "The deploy/admin panel, q-flux admin routes and the node API (:8080) \
                        are unreachable from customer VMs and from the public internet without auth.",
            status: PolicyRequired,
            mitigation: "Bind admin to loopback/VPN; q-flux vhost ACL; guests on an isolated \
                         bridge with no route to the management plane.",
        },
        SecurityControl {
            id: "host-ssh-trust-not-shared",
            area: AdminExposure,
            statement: "Fleet SSH key trust between hosts is never reachable from a guest.",
            status: PolicyRequired,
            mitigation: "Guests get no route to the host mgmt interface; mgmt SSH bound off the \
                         guest bridge.",
        },
        SecurityControl {
            id: "verified-base-image",
            area: VmIsolation,
            statement: "Guest disks are per-VM qcow2 overlays off a BLAKE3-pinned base image.",
            status: EnforcedInCode,
            mitigation: "ImageSpec rejects a non-64-hex digest; the executor renders a backing-file \
                         overlay, never a shared writable base.",
        },
        SecurityControl {
            id: "no-live-executor",
            area: VmIsolation,
            statement: "No privileged executor that mutates a host exists; provisioning is dry-run \
                        render-only until an operator-gated executor is reviewed.",
            status: EnforcedInCode,
            mitigation: "Only DryRunExecutor ships; ExecutionReport::is_dry_run() holds; there is \
                         no HostExecutor impl.",
        },
        SecurityControl {
            id: "injection-safe-identifiers",
            area: VmIsolation,
            statement: "Tenant/VM/bridge identifiers cannot inject shell metacharacters into \
                        rendered backend commands.",
            status: EnforcedInCode,
            mitigation: "validate_slug restricts identifiers to [A-Za-z0-9_-] before any action is \
                         built.",
        },
        SecurityControl {
            id: "host-residency",
            area: HostResidency,
            statement: "Customer VMs do NOT co-reside with the production Quillon node.",
            status: NotYetImplemented,
            mitigation: "First paid host must be a fresh dedicated server; Epsilon stays \
                         seed/control only. (Epsilon is OOM-cycling and runs the prod node — \
                         categorically not an eligible paid host.)",
        },
        SecurityControl {
            id: "inter-tenant-l2-isolation",
            area: Networking,
            statement: "Guests on the shared bridge cannot reach each other at L2.",
            status: PolicyRequired,
            mitigation: "Per-tenant VLAN/bridge or ebtables isolation; AttachBridge names the \
                         bridge but isolation is host policy.",
        },
        SecurityControl {
            id: "egress-antispoof",
            area: Networking,
            statement: "A guest cannot spoof source IPs or attack the fleet from inside.",
            status: PolicyRequired,
            mitigation: "Anti-spoof + egress filtering on the guest bridge (cf. the fleet's \
                         iptables OUTPUT REJECT discipline).",
        },
        SecurityControl {
            id: "default-deny-firewall",
            area: Firewall,
            statement: "Host firewall default-denies inbound to guests; mgmt ports never exposed.",
            status: NotYetImplemented,
            mitigation: "No firewall automation yet — a default-deny nftables ruleset must be \
                         authored and verified before go-live.",
        },
        SecurityControl {
            id: "abuse-desk",
            area: AbuseDesk,
            statement: "An abuse path exists: a monitored contact plus a documented suspend/takedown \
                        procedure.",
            status: NotYetImplemented,
            mitigation: "The /fluxvisor/1/abuse-event topic is reserved but has no handler or desk; \
                         define an abuse contact + suspension runbook before the first customer.",
        },
        SecurityControl {
            id: "durability-honesty",
            area: Backups,
            statement: "Guest data durability is stated honestly; the HDD pool is not a backup of \
                        itself.",
            status: PolicyRequired,
            mitigation: "State the real redundancy of the 4x22TB pool in plan terms; back up host \
                         config + tenant metadata off-box.",
        },
        SecurityControl {
            id: "ipv4-scarcity",
            area: Ipv4Scarcity,
            statement: "Scarce IPv4 is rationed; IPv6-only is the default where possible.",
            status: EnforcedInCode,
            mitigation: "cold-storage plan allocates ipv4=0; the capacity ledger rejects \
                         over-allocation of the host's IPv4 pool.",
        },
        SecurityControl {
            id: "provider-resale-terms",
            area: ProviderResale,
            statement: "The upstream provider's ToS permits VM resale / sub-letting on the leased box.",
            status: NotYetImplemented,
            mitigation: "Read the EUR220 lease ToS before selling public plans — many budget \
                         providers forbid resale; assume forbidden until confirmed.",
        },
        // --- Folded in from the 2026-06-13 DeepSeek-V4 foundation consult ---
        SecurityControl {
            id: "reservation-lock-toctou",
            area: ControlArea::Capacity,
            statement: "Capacity reserved at plan time is held to provision time; no TOCTOU \
                        overcommit between plan_vm() and a live executor.",
            status: ControlStatus::NotYetImplemented,
            mitigation: "plan_vm reserves on a CLONED ledger; a live path needs a soft reservation \
                         with TTL + a seq-CAS allocation against the worker's heartbeat seq.",
        },
        SecurityControl {
            id: "cpu-side-channels",
            area: ControlArea::VmIsolation,
            statement: "Cross-VM CPU side channels (L1TF/MDS/SMT, speculative exec) are mitigated.",
            status: ControlStatus::PolicyRequired,
            mitigation: "Current microcode, kernel mitigations on, SMT policy decided, vCPU pinning \
                         per tenant for sensitive plans.",
        },
        SecurityControl {
            id: "guest-disk-encryption",
            area: ControlArea::VmIsolation,
            statement: "A guest cannot read another tenant's disk; backing stores are isolated and \
                        encrypted at rest.",
            status: ControlStatus::PolicyRequired,
            mitigation: "Per-tenant LUKS/qcow2 encryption; overlays never share a writable base; \
                         wipe-on-delete for the HDD pool.",
        },
        SecurityControl {
            id: "noisy-neighbor-qos",
            area: ControlArea::VmIsolation,
            statement: "One guest cannot starve others; admission control is backed by host \
                        enforcement.",
            status: ControlStatus::PolicyRequired,
            mitigation: "The no-oversell ledger is admission only — pair it with cgroup CPU/blkio/\
                         memory hard limits + network QoS on the host.",
        },
        SecurityControl {
            id: "cloud-init-secrets",
            area: ControlArea::VmIsolation,
            statement: "Secrets are not stored in cleartext cloud-init/config-drive.",
            status: ControlStatus::PolicyRequired,
            mitigation: "Inject per-instance ephemeral tokens at boot; keep user-data secret-free; \
                         config-drive (not network IMDS) limits SSRF exposure.",
        },
        SecurityControl {
            id: "imds-ssrf-hardening",
            area: ControlArea::Networking,
            statement: "Any metadata endpoint is tenant-scoped and cannot leak host credentials via \
                        SSRF.",
            status: ControlStatus::PolicyRequired,
            mitigation: "Prefer config-drive over a network IMDS; if an IMDS exists, scope per-tenant \
                         + require a token and strip host creds.",
        },
        SecurityControl {
            id: "live-migration-trust",
            area: ControlArea::VmIsolation,
            statement: "Live migration, if offered, authenticates and encrypts guest state in transit.",
            status: ControlStatus::PolicyRequired,
            mitigation: "Alpha disables live migration. If enabled later: TLS + key wrap + \
                         source/destination hypervisor-version attestation.",
        },
        SecurityControl {
            id: "hypervisor-patch-cadence",
            area: ControlArea::Patching,
            statement: "Critical hypervisor/host CVEs are patched on a defined cadence with tenant \
                        draining.",
            status: ControlStatus::NotYetImplemented,
            mitigation: "No patch/reboot orchestration yet — define a <24h critical-patch policy with \
                         host drain before go-live.",
        },
        SecurityControl {
            id: "host-action-audit-trail",
            area: ControlArea::Audit,
            statement: "Host-level actions (console, snapshot, migration, host-command) are written to \
                        an immutable audit log.",
            status: ControlStatus::NotYetImplemented,
            mitigation: "No audit trail yet — ship integrity-checked logs to cold storage with \
                         anomaly detection on privilege escalation.",
        },
    ]
}

/// Controls that currently block taking paid customers.
pub fn blocking_gaps() -> Vec<SecurityControl> {
    alpha_security_checklist()
        .into_iter()
        .filter(|c| c.status == ControlStatus::NotYetImplemented)
        .collect()
}

/// Whether a host with this checklist may take paid customers: only if nothing
/// is still [`ControlStatus::NotYetImplemented`].
pub fn ready_for_paid_customers(checklist: &[SecurityControl]) -> bool {
    !checklist
        .iter()
        .any(|c| c.status == ControlStatus::NotYetImplemented)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checklist_covers_every_threat_area() {
        let checklist = alpha_security_checklist();
        for area in [
            ControlArea::AdminExposure,
            ControlArea::VmIsolation,
            ControlArea::HostResidency,
            ControlArea::Networking,
            ControlArea::Firewall,
            ControlArea::AbuseDesk,
            ControlArea::Backups,
            ControlArea::Ipv4Scarcity,
            ControlArea::ProviderResale,
            ControlArea::Capacity,
            ControlArea::Audit,
            ControlArea::Patching,
        ] {
            assert!(
                checklist.iter().any(|c| c.area == area),
                "no control for {area:?}"
            );
        }
    }

    #[test]
    fn every_control_has_statement_and_mitigation() {
        for c in alpha_security_checklist() {
            assert!(!c.id.is_empty());
            assert!(!c.statement.is_empty(), "{} missing statement", c.id);
            assert!(!c.mitigation.is_empty(), "{} missing mitigation", c.id);
        }
    }

    #[test]
    fn control_ids_are_unique() {
        let checklist = alpha_security_checklist();
        let mut ids: Vec<&str> = checklist.iter().map(|c| c.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), checklist.len());
    }

    #[test]
    fn current_alpha_is_not_ready() {
        // Honest gate: there are still NotYetImplemented controls, so the alpha
        // must NOT take paid customers yet.
        let checklist = alpha_security_checklist();
        assert!(!ready_for_paid_customers(&checklist));
        let blockers = blocking_gaps();
        assert!(!blockers.is_empty());
        // host-residency is the headline blocker on the current Epsilon box.
        assert!(blockers.iter().any(|c| c.id == "host-residency"));
    }

    #[test]
    fn dry_run_invariants_are_enforced_in_code() {
        // The controls FluxVisor already upholds must be marked EnforcedInCode.
        let checklist = alpha_security_checklist();
        for id in ["no-live-executor", "injection-safe-identifiers", "verified-base-image"] {
            let c = checklist.iter().find(|c| c.id == id).unwrap();
            assert_eq!(c.status, ControlStatus::EnforcedInCode, "{id} should be enforced");
        }
    }
}
