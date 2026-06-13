//! The FluxVisor executor boundary.
//!
//! A [`ProvisionPlan`] (already capacity-checked and operator-reviewed) is a
//! *description* of work. An **executor** is the thing that turns each
//! [`BackendAction`] into a backend-specific operation.
//!
//! This module ships only the **dry-run** half of that boundary. A
//! [`DryRunExecutor`] *renders* the commands a real backend would run — as
//! plain strings, marked [`StepStatus::Planned`] — and runs **nothing**. The
//! default [`DryRunExecutor::render_plan`] is execution-free by construction:
//! it only maps actions through [`DryRunExecutor::render_action`], which returns
//! data.
//!
//! The live counterpart — a `LibvirtExecutor` that actually invokes `virsh` /
//! `qemu-img` — is deliberately **not** in this crate yet. It is a separate,
//! operator-gated boundary (a future `HostExecutor` trait carrying a privilege
//! token). Until that lands and the first paid host exists, FluxVisor can render
//! exactly what it would do without any risk of doing it.
//!
//! Safety note: every identifier embedded in a rendered command (VM name,
//! tenant, bridge) has already passed `validate_slug` upstream, so the rendered
//! command lines are free of shell-injection metacharacters by construction.

use crate::{Backend, BackendAction, ProvisionPlan};
use serde::{Deserialize, Serialize};

/// Filesystem roots a libvirt executor would use. Rendered into commands only.
const DISK_DIR: &str = "/var/lib/flux-visor/disks";
const IMAGE_DIR: &str = "/var/lib/flux-visor/images";
const SEED_DIR: &str = "/var/lib/flux-visor/seeds";
const DOMAIN_DIR: &str = "/var/lib/flux-visor/domains";

/// Status of a single rendered step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStatus {
    /// Rendered but not run. The only status a dry-run executor produces.
    Planned,
    /// Run successfully by a live executor. Never produced here.
    Executed,
    /// Attempted by a live executor and failed. Never produced here.
    Failed,
}

/// One backend action, rendered into the command(s) it would run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionStep {
    /// Stable label for the action, e.g. `create-disk`.
    pub action: String,
    /// Execution status. `Planned` for everything a dry-run executor emits.
    pub status: StepStatus,
    /// The command line(s) a live executor would run, in order.
    pub rendered: Vec<String>,
    /// Whether the live version of this step mutates host state.
    pub mutating: bool,
}

/// The result of rendering (or, in future, executing) a whole plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionReport {
    /// Host the plan targets.
    pub host: String,
    /// Backend the executor renders for.
    pub backend: Backend,
    /// Whether any step actually ran. Always `false` for a dry-run.
    pub executed: bool,
    /// Rendered steps, in plan order.
    pub steps: Vec<ExecutionStep>,
    /// Operator warnings carried over from the plan.
    pub warnings: Vec<String>,
}

impl ExecutionReport {
    /// True iff this report provably ran nothing: no step executed and every
    /// step is still `Planned`.
    pub fn is_dry_run(&self) -> bool {
        !self.executed && self.steps.iter().all(|s| s.status == StepStatus::Planned)
    }

    /// Flatten the rendered command lines into a single reviewable script, one
    /// step per block with a `# <action>` header comment.
    pub fn rendered_script(&self) -> String {
        let mut out = String::new();
        for step in &self.steps {
            out.push_str("# ");
            out.push_str(&step.action);
            out.push('\n');
            for line in &step.rendered {
                out.push_str(line);
                out.push('\n');
            }
        }
        out
    }
}

/// Render a reviewed [`ProvisionPlan`] into the backend operations it implies,
/// **without running them**.
///
/// Implementors provide [`backend`](Self::backend) and
/// [`render_action`](Self::render_action). The provided
/// [`render_plan`](Self::render_plan) maps every action through
/// `render_action`, so an implementor cannot accidentally introduce a side
/// effect through the plan path — it only ever produces data.
pub trait DryRunExecutor {
    /// Backend this executor renders for.
    fn backend(&self) -> Backend;

    /// Render a single backend action into a reviewable step. Must not run it.
    fn render_action(&self, action: &BackendAction) -> ExecutionStep;

    /// Render an entire provisioning plan into a reviewable report. Provided:
    /// execution-free by construction.
    fn render_plan(&self, plan: &ProvisionPlan) -> ExecutionReport {
        ExecutionReport {
            host: plan.host.clone(),
            backend: self.backend(),
            executed: false,
            steps: plan.actions.iter().map(|a| self.render_action(a)).collect(),
            warnings: plan.warnings.clone(),
        }
    }
}

/// Dry-run executor for the libvirt/KVM backend.
///
/// Renders the `qemu-img` / `cloud-localds` / `virt-install` / `virsh` / `tc`
/// commands a `LibvirtExecutor` would run. Stateless.
#[derive(Clone, Copy, Debug, Default)]
pub struct LibvirtDryRun;

impl LibvirtDryRun {
    /// Build a libvirt dry-run executor.
    pub fn new() -> Self {
        Self
    }
}

impl DryRunExecutor for LibvirtDryRun {
    fn backend(&self) -> Backend {
        Backend::LibvirtKvm
    }

    fn render_action(&self, action: &BackendAction) -> ExecutionStep {
        match action {
            BackendAction::CreateDisk {
                vm,
                disk_gib,
                image_blake3,
            } => ExecutionStep {
                action: "create-disk".to_string(),
                status: StepStatus::Planned,
                rendered: vec![format!(
                    "qemu-img create -f qcow2 -b {IMAGE_DIR}/{image_blake3}.qcow2 \
                     -F qcow2 {DISK_DIR}/{}.qcow2 {disk_gib}G",
                    vm.as_str()
                )],
                mutating: true,
            },
            BackendAction::WriteCloudInit {
                vm,
                tenant,
                ssh_public_key,
            } => {
                let vm = vm.as_str();
                let key_note = if ssh_public_key.is_some() { "present" } else { "none" };
                ExecutionStep {
                    action: "write-cloud-init".to_string(),
                    status: StepStatus::Planned,
                    rendered: vec![
                        format!(
                            "install -m 0600 /dev/stdin {SEED_DIR}/{vm}-user-data.yaml  \
                             # tenant={}, ssh_key={key_note}",
                            tenant.as_str()
                        ),
                        format!(
                            "cloud-localds {SEED_DIR}/{vm}-seed.iso \
                             {SEED_DIR}/{vm}-user-data.yaml"
                        ),
                    ],
                    mutating: true,
                }
            }
            BackendAction::DefineVm {
                vm,
                vcpu,
                ram_mib,
                backend,
            } => {
                let vm = vm.as_str();
                let _ = backend; // libvirt renderer always targets LibvirtKvm
                ExecutionStep {
                    action: "define-vm".to_string(),
                    status: StepStatus::Planned,
                    rendered: vec![
                        format!(
                            "virt-install --import --name {vm} --vcpus {vcpu} \
                             --memory {ram_mib} \
                             --disk path={DISK_DIR}/{vm}.qcow2,format=qcow2,bus=virtio \
                             --disk path={SEED_DIR}/{vm}-seed.iso,device=cdrom \
                             --os-variant generic --graphics none --noautoconsole \
                             --print-xml > {DOMAIN_DIR}/{vm}.xml"
                        ),
                        format!("virsh define {DOMAIN_DIR}/{vm}.xml"),
                    ],
                    mutating: true,
                }
            }
            BackendAction::AttachBridge { vm, bridge } => ExecutionStep {
                action: "attach-bridge".to_string(),
                status: StepStatus::Planned,
                // --config = persistent only; the VM is not running yet.
                rendered: vec![format!(
                    "virsh attach-interface --domain {} --type bridge \
                     --source {bridge} --model virtio --config",
                    vm.as_str()
                )],
                mutating: true,
            },
            BackendAction::ApplyTrafficPolicy {
                vm,
                monthly_traffic_tb,
                avg_mbps,
            } => {
                let vm = vm.as_str();
                ExecutionStep {
                    action: "apply-traffic-policy".to_string(),
                    status: StepStatus::Planned,
                    rendered: vec![
                        format!(
                            "tc qdisc replace dev vnet-{vm} root tbf \
                             rate {avg_mbps}mbit burst 32kbit latency 50ms"
                        ),
                        format!(
                            "# monthly quota {monthly_traffic_tb} TB enforced by \
                             accounting, not tc (tc shapes rate only)"
                        ),
                    ],
                    mutating: true,
                }
            }
            BackendAction::StartVm { vm } => ExecutionStep {
                action: "start-vm".to_string(),
                status: StepStatus::Planned,
                rendered: vec![format!("virsh start {}", vm.as_str())],
                mutating: true,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        epsilon_host_profile, fluxhost_alpha_plans, plan_vm, CapacityLedger, ImageFamily,
        ImageSpec, TenantId, VmName, VmRequest,
    };

    fn cold_storage_plan() -> ProvisionPlan {
        let ledger = CapacityLedger::new(epsilon_host_profile());
        let request = VmRequest::new(
            TenantId::new("viktor").unwrap(),
            VmName::new("backup-01").unwrap(),
            "cold-storage",
            ImageSpec::new(ImageFamily::Debian, "12", &"a".repeat(64)).unwrap(),
            None,
        )
        .unwrap();
        plan_vm(&ledger, &fluxhost_alpha_plans(), request).unwrap()
    }

    #[test]
    fn renders_every_action_in_order() {
        let report = LibvirtDryRun::new().render_plan(&cold_storage_plan());
        assert_eq!(report.backend, Backend::LibvirtKvm);
        assert_eq!(report.steps.len(), 6);
        let labels: Vec<&str> = report.steps.iter().map(|s| s.action.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                "create-disk",
                "write-cloud-init",
                "define-vm",
                "attach-bridge",
                "apply-traffic-policy",
                "start-vm",
            ]
        );
    }

    #[test]
    fn report_is_provably_inert() {
        let report = LibvirtDryRun::new().render_plan(&cold_storage_plan());
        assert!(report.is_dry_run());
        assert!(!report.executed);
        assert!(report
            .steps
            .iter()
            .all(|s| s.status == StepStatus::Planned));
    }

    #[test]
    fn create_disk_uses_backing_image_and_size() {
        let report = LibvirtDryRun::new().render_plan(&cold_storage_plan());
        let line = &report.steps[0].rendered[0];
        assert!(line.contains("qemu-img create"));
        // cold-storage is 8 TiB = 8192 GiB.
        assert!(line.contains("8192G"), "disk size missing: {line}");
        // backing file is the verified image digest (64 'a's).
        assert!(line.contains(&"a".repeat(64)), "backing image missing: {line}");
    }

    #[test]
    fn start_is_last_and_renders_virsh_start() {
        let report = LibvirtDryRun::new().render_plan(&cold_storage_plan());
        let last = report.steps.last().unwrap();
        assert_eq!(last.action, "start-vm");
        assert_eq!(last.rendered, vec!["virsh start backup-01".to_string()]);
    }

    #[test]
    fn rendered_script_is_reviewable_and_complete() {
        let script = LibvirtDryRun::new()
            .render_plan(&cold_storage_plan())
            .rendered_script();
        assert!(script.contains("# create-disk"));
        assert!(script.contains("virt-install --import --name backup-01"));
        assert!(script.contains("virsh define"));
        assert!(script.contains("virsh start backup-01"));
    }

    #[test]
    fn every_libvirt_step_is_marked_mutating() {
        // The renderer is honest that each step's *live* form mutates the host,
        // even though rendering itself does not.
        let report = LibvirtDryRun::new().render_plan(&cold_storage_plan());
        assert!(report.steps.iter().all(|s| s.mutating));
    }
}
