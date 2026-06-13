//! Flux-native virtualization control-plane primitives.
//!
//! `flux-visor` is not a hypervisor. The host still runs KVM/QEMU/libvirt,
//! Firecracker, or another proven isolation backend. This crate is the Flux
//! layer above that: product plans, capacity accounting, tenant/name hygiene,
//! and dry-run backend actions that can be reviewed before any executor touches
//! a real machine.

#![warn(missing_docs)]

pub mod cortex_bridge;
pub mod executor;
pub mod heartbeat;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Decimal terabyte to average megabit/second over a 30-day month.
///
/// This is useful because hosting plans are often sold as "100 TB on 1 Gbit";
/// the sustained average is far below a saturated port.
pub fn traffic_tb_month_to_mbps(tb: u32) -> f64 {
    let bits = tb as f64 * 1_000_000_000_000.0 * 8.0;
    bits / (30.0 * 24.0 * 60.0 * 60.0) / 1_000_000.0
}

/// FluxVisor control topics carried over `flux-p2p`.
///
/// These are deliberately separate from compile/SIGIL topics so a host fleet can
/// route capacity/provisioning messages without confusing them with chain data.
pub const FLUXVISOR_P2P_TOPICS: &[&str] = &[
    "/fluxvisor/1/host-heartbeat",
    "/fluxvisor/1/capacity",
    "/fluxvisor/1/provision-plan",
    "/fluxvisor/1/provision-result",
    "/fluxvisor/1/abuse-event",
];

/// A validated tenant identifier.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TenantId(String);

impl TenantId {
    /// Validate and build a tenant id.
    pub fn new(value: impl Into<String>) -> Result<Self, FluxVisorError> {
        let value = value.into();
        validate_slug("tenant", &value, 3, 48)?;
        Ok(Self(value))
    }

    /// Borrow the tenant id string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A validated VM name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct VmName(String);

impl VmName {
    /// Validate and build a VM name.
    pub fn new(value: impl Into<String>) -> Result<Self, FluxVisorError> {
        let value = value.into();
        validate_slug("vm_name", &value, 3, 64)?;
        Ok(Self(value))
    }

    /// Borrow the VM name string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn validate_slug(field: &'static str, value: &str, min: usize, max: usize) -> Result<(), FluxVisorError> {
    if value.len() < min || value.len() > max {
        return Err(FluxVisorError::InvalidIdentifier {
            field,
            value: value.to_string(),
            reason: format!("length must be {min}..={max} bytes"),
        });
    }
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return Err(FluxVisorError::InvalidIdentifier {
            field,
            value: value.to_string(),
            reason: "empty".to_string(),
        });
    };
    if !first.is_ascii_alphanumeric() {
        return Err(FluxVisorError::InvalidIdentifier {
            field,
            value: value.to_string(),
            reason: "must start with an ASCII letter or digit".to_string(),
        });
    }
    if !value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err(FluxVisorError::InvalidIdentifier {
            field,
            value: value.to_string(),
            reason: "only ASCII letters, digits, '-' and '_' are allowed".to_string(),
        });
    }
    Ok(())
}

/// The isolation backend the host executor will target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Backend {
    /// KVM via libvirt domain definitions.
    LibvirtKvm,
    /// Firecracker microVMs.
    Firecracker,
    /// Direct QEMU command generation for lab hosts.
    QemuDirect,
}

/// Host bridge / egress shape for customer VMs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkProfile {
    /// Linux bridge name that guests attach to.
    pub bridge: String,
    /// Public uplink device, used by the eventual executor for shaping rules.
    pub uplink: String,
    /// Maximum physical or contracted port speed in megabit/second.
    pub port_mbps: u32,
}

impl NetworkProfile {
    /// Build a network profile.
    pub fn new(bridge: &str, uplink: &str, port_mbps: u32) -> Result<Self, FluxVisorError> {
        validate_slug("bridge", bridge, 2, 32)?;
        validate_slug("uplink", uplink, 2, 32)?;
        if port_mbps == 0 {
            return Err(FluxVisorError::InvalidCapacity("port_mbps must be > 0"));
        }
        Ok(Self {
            bridge: bridge.to_string(),
            uplink: uplink.to_string(),
            port_mbps,
        })
    }
}

/// Role of a physical host inside the FluxVisor P2P mesh.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HostRole {
    /// Control-plane seed and operator API host.
    Seed,
    /// General VM worker host.
    Worker,
    /// Storage-heavy host.
    Storage,
    /// GPU or accelerator host.
    Gpu,
}

/// Public P2P identity for one physical host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct P2pHostNode {
    /// Stable host id, e.g. `epsilon-host-01`.
    pub host_id: String,
    /// Host role in the fleet.
    pub role: HostRole,
    /// Public or private libp2p multiaddr. Should include `/p2p/<PeerId>` once
    /// the host identity is known.
    pub advertise_addr: String,
    /// Listen port on the host.
    pub listen_port: u16,
    /// Whether other nodes should use this node as a bootstrap peer.
    pub bootstrap: bool,
}

impl P2pHostNode {
    /// Build and validate a P2P host node.
    pub fn new(
        host_id: &str,
        role: HostRole,
        advertise_addr: &str,
        listen_port: u16,
        bootstrap: bool,
    ) -> Result<Self, FluxVisorError> {
        validate_slug("host_id", host_id, 3, 64)?;
        validate_multiaddr(advertise_addr, bootstrap)?;
        if listen_port == 0 {
            return Err(FluxVisorError::InvalidP2pConfig("listen_port must be > 0".to_string()));
        }
        Ok(Self {
            host_id: host_id.to_string(),
            role,
            advertise_addr: advertise_addr.to_string(),
            listen_port,
            bootstrap,
        })
    }

    /// Flux P2P listen address for this host.
    pub fn listen_addr(&self) -> String {
        format!("/ip4/0.0.0.0/tcp/{}", self.listen_port)
    }
}

fn validate_multiaddr(addr: &str, require_peer_id: bool) -> Result<(), FluxVisorError> {
    if !(addr.starts_with("/ip4/") || addr.starts_with("/ip6/") || addr.starts_with("/dns4/") || addr.starts_with("/dns6/")) {
        return Err(FluxVisorError::InvalidP2pConfig(format!(
            "multiaddr `{addr}` must start with /ip4, /ip6, /dns4, or /dns6"
        )));
    }
    if !addr.contains("/tcp/") {
        return Err(FluxVisorError::InvalidP2pConfig(format!(
            "multiaddr `{addr}` must include /tcp/<port>"
        )));
    }
    if require_peer_id && !addr.contains("/p2p/") {
        return Err(FluxVisorError::InvalidP2pConfig(format!(
            "bootstrap multiaddr `{addr}` must include /p2p/<PeerId>"
        )));
    }
    Ok(())
}

/// A FluxVisor host mesh carried by `flux-p2p`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FluxP2pCluster {
    /// Human-readable cluster id.
    pub cluster_id: String,
    /// Hosts participating in the mesh.
    pub nodes: Vec<P2pHostNode>,
    /// Extra topics beyond [`FLUXVISOR_P2P_TOPICS`].
    pub extra_topics: Vec<String>,
}

impl FluxP2pCluster {
    /// Build and validate a cluster.
    pub fn new(cluster_id: &str, nodes: Vec<P2pHostNode>) -> Result<Self, FluxVisorError> {
        validate_slug("cluster_id", cluster_id, 3, 48)?;
        if nodes.is_empty() {
            return Err(FluxVisorError::InvalidP2pConfig("cluster must contain at least one node".to_string()));
        }
        let mut seen = std::collections::BTreeSet::new();
        for node in &nodes {
            if !seen.insert(node.host_id.clone()) {
                return Err(FluxVisorError::InvalidP2pConfig(format!(
                    "duplicate host_id `{}`",
                    node.host_id
                )));
            }
        }
        if !nodes.iter().any(|n| n.bootstrap) {
            return Err(FluxVisorError::InvalidP2pConfig("cluster needs at least one bootstrap node".to_string()));
        }
        Ok(Self {
            cluster_id: cluster_id.to_string(),
            nodes,
            extra_topics: Vec::new(),
        })
    }

    /// Add an extra gossipsub topic to every generated config.
    pub fn with_topic(mut self, topic: &str) -> Result<Self, FluxVisorError> {
        if !topic.starts_with('/') || topic.len() < 4 {
            return Err(FluxVisorError::InvalidP2pConfig(format!("invalid topic `{topic}`")));
        }
        self.extra_topics.push(topic.to_string());
        Ok(self)
    }

    /// Return bootstrap multiaddrs.
    pub fn bootstrap_peers(&self) -> Vec<String> {
        self.nodes
            .iter()
            .filter(|n| n.bootstrap)
            .map(|n| n.advertise_addr.clone())
            .collect()
    }

    /// Generate a `flux-p2p` network config for one host.
    pub fn network_config_for(&self, host_id: &str) -> Result<flux_p2p::NetworkConfig, FluxVisorError> {
        let node = self
            .nodes
            .iter()
            .find(|n| n.host_id == host_id)
            .ok_or_else(|| FluxVisorError::UnknownHost(host_id.to_string()))?;

        let mut topics: Vec<String> = FLUXVISOR_P2P_TOPICS.iter().map(|t| t.to_string()).collect();
        topics.extend(self.extra_topics.clone());

        let bootstrap_peers = self
            .nodes
            .iter()
            .filter(|n| n.host_id != node.host_id && n.bootstrap)
            .map(|n| n.advertise_addr.clone())
            .collect();

        Ok(flux_p2p::NetworkConfig {
            node_id: format!("{}:{}", self.cluster_id, node.host_id),
            listen_addr: node.listen_addr(),
            bootstrap_peers,
            dagknight_enabled: true,
            sap_enabled: true,
            x_algo_enabled: true,
            entanglement_enabled: true,
            gossipsub_topics: topics,
        })
    }

    /// Build a join plan for a fresh server entering the mesh.
    pub fn join_plan_for(&self, host_id: &str) -> Result<HostJoinPlan, FluxVisorError> {
        let node = self
            .nodes
            .iter()
            .find(|n| n.host_id == host_id)
            .ok_or_else(|| FluxVisorError::UnknownHost(host_id.to_string()))?;
        let config = self.network_config_for(host_id)?;
        Ok(HostJoinPlan {
            host_id: node.host_id.clone(),
            actions: vec![
                HostJoinAction::InstallFluxP2pService,
                HostJoinAction::WriteNetworkConfig { config },
                HostJoinAction::OpenTcpPort { port: node.listen_port },
                HostJoinAction::StartFluxP2pService,
                HostJoinAction::PublishCapacityHeartbeat,
            ],
        })
    }
}

/// Operator-reviewed actions for joining a host to the P2P control plane.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostJoinPlan {
    /// Host being joined.
    pub host_id: String,
    /// Actions to execute, in order.
    pub actions: Vec<HostJoinAction>,
}

/// A host-level action for a future privileged executor.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum HostJoinAction {
    /// Install or verify the flux-p2p service binary.
    InstallFluxP2pService,
    /// Write a `flux_p2p::NetworkConfig`.
    WriteNetworkConfig {
        /// Generated network config.
        config: flux_p2p::NetworkConfig,
    },
    /// Open the host firewall for the P2P listen port.
    OpenTcpPort {
        /// TCP port.
        port: u16,
    },
    /// Start or restart the service.
    StartFluxP2pService,
    /// Publish initial capacity heartbeat on `/fluxvisor/1/capacity`.
    PublishCapacityHeartbeat,
}

/// Raw sellable capacity for one physical host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostCapacity {
    /// Hardware threads or vCPU slots.
    pub vcpu_threads: u16,
    /// Host RAM reserved for guests, in MiB.
    pub ram_mib: u64,
    /// Guest disk pool capacity, in GiB.
    pub disk_gib: u64,
    /// Monthly traffic budget this host may sell, in decimal TB.
    pub monthly_traffic_tb: u32,
    /// Routable IPv4 addresses available for guests.
    pub ipv4: u16,
    /// Routed IPv6 prefixes available for guests.
    pub ipv6_prefixes: u16,
}

impl HostCapacity {
    /// Return a zero-capacity host.
    pub fn zero() -> Self {
        Self {
            vcpu_threads: 0,
            ram_mib: 0,
            disk_gib: 0,
            monthly_traffic_tb: 0,
            ipv4: 0,
            ipv6_prefixes: 0,
        }
    }
}

/// How much oversell the host allows.
///
/// Alpha default is intentionally boring: no oversell. CPU can be raised later
/// once real load data exists; RAM and disk should stay conservative for paid
/// customers unless ballooning/thin-provisioning is explicitly part of the
/// product.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvercommitPolicy {
    /// CPU sellable ratio in per-mille. `1000` means 1.0x, `2000` means 2.0x.
    pub cpu_per_mille: u16,
    /// RAM sellable ratio in per-mille.
    pub ram_per_mille: u16,
    /// Disk sellable ratio in per-mille.
    pub disk_per_mille: u16,
}

impl Default for OvercommitPolicy {
    fn default() -> Self {
        Self {
            cpu_per_mille: 1000,
            ram_per_mille: 1000,
            disk_per_mille: 1000,
        }
    }
}

impl OvercommitPolicy {
    /// Build a policy and reject impossible ratios.
    pub fn new(cpu_per_mille: u16, ram_per_mille: u16, disk_per_mille: u16) -> Result<Self, FluxVisorError> {
        for (field, value) in [
            ("cpu_per_mille", cpu_per_mille),
            ("ram_per_mille", ram_per_mille),
            ("disk_per_mille", disk_per_mille),
        ] {
            if value < 1000 {
                return Err(FluxVisorError::InvalidOvercommit { field, value });
            }
        }
        Ok(Self {
            cpu_per_mille,
            ram_per_mille,
            disk_per_mille,
        })
    }
}

/// A physical host that can run customer VMs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostProfile {
    /// Host identifier.
    pub name: String,
    /// Guest backend.
    pub backend: Backend,
    /// Sellable capacity.
    pub capacity: HostCapacity,
    /// Oversell policy.
    pub overcommit: OvercommitPolicy,
    /// Network shape.
    pub network: NetworkProfile,
}

impl HostProfile {
    /// Build and validate a host profile.
    pub fn new(
        name: &str,
        backend: Backend,
        capacity: HostCapacity,
        overcommit: OvercommitPolicy,
        network: NetworkProfile,
    ) -> Result<Self, FluxVisorError> {
        validate_slug("host", name, 3, 64)?;
        if capacity.vcpu_threads == 0 || capacity.ram_mib == 0 || capacity.disk_gib == 0 {
            return Err(FluxVisorError::InvalidCapacity("host must have non-zero cpu, ram, and disk"));
        }
        Ok(Self {
            name: name.to_string(),
            backend,
            capacity,
            overcommit,
            network,
        })
    }

    /// Capacity after overcommit policy.
    pub fn sellable_capacity(&self) -> HostCapacity {
        HostCapacity {
            vcpu_threads: scale_u16(self.capacity.vcpu_threads, self.overcommit.cpu_per_mille),
            ram_mib: scale_u64(self.capacity.ram_mib, self.overcommit.ram_per_mille),
            disk_gib: scale_u64(self.capacity.disk_gib, self.overcommit.disk_per_mille),
            monthly_traffic_tb: self.capacity.monthly_traffic_tb,
            ipv4: self.capacity.ipv4,
            ipv6_prefixes: self.capacity.ipv6_prefixes,
        }
    }

    /// Maximum number of identical `plan` units this host can sell, bounded by
    /// the scarcest resource after the overcommit policy. A resource the plan
    /// does not consume (e.g. a cold-storage box with no public IPv4) is not a
    /// constraint. Returns 0 only for a degenerate plan that consumes nothing.
    pub fn max_units(&self, plan: &VmPlan) -> u32 {
        let cap = self.sellable_capacity();
        let r = &plan.resources;
        [
            units_for(r.vcpu as u64, cap.vcpu_threads as u64),
            units_for(r.ram_mib, cap.ram_mib),
            units_for(r.disk_gib, cap.disk_gib),
            units_for(r.monthly_traffic_tb as u64, cap.monthly_traffic_tb as u64),
            units_for(r.ipv4 as u64, cap.ipv4 as u64),
            units_for(r.ipv6_prefixes as u64, cap.ipv6_prefixes as u64),
        ]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(0)
    }

    /// Monthly revenue in euro cents from selling [`HostProfile::max_units`] of
    /// `plan` on this host at full occupancy.
    pub fn max_monthly_revenue_cents(&self, plan: &VmPlan) -> u64 {
        self.max_units(plan) as u64 * plan.monthly_price_eur_cents as u64
    }
}

fn scale_u16(value: u16, per_mille: u16) -> u16 {
    ((value as u32 * per_mille as u32) / 1000).min(u16::MAX as u32) as u16
}

fn scale_u64(value: u64, per_mille: u16) -> u64 {
    value.saturating_mul(per_mille as u64) / 1000
}

/// Units of a per-unit demand that fit into an available amount. `None` when the
/// plan does not consume this resource (per-unit 0), so it is not a constraint.
fn units_for(per_unit: u64, available: u64) -> Option<u32> {
    if per_unit == 0 {
        None
    } else {
        Some((available / per_unit) as u32)
    }
}

/// Resources consumed by a plan or reservation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceSet {
    /// vCPU slots.
    pub vcpu: u16,
    /// RAM in MiB.
    pub ram_mib: u64,
    /// Disk in GiB.
    pub disk_gib: u64,
    /// Monthly traffic in decimal TB.
    pub monthly_traffic_tb: u32,
    /// IPv4 addresses.
    pub ipv4: u16,
    /// IPv6 prefixes.
    pub ipv6_prefixes: u16,
}

impl ResourceSet {
    /// Add another resource set.
    pub fn saturating_add(self, rhs: Self) -> Self {
        Self {
            vcpu: self.vcpu.saturating_add(rhs.vcpu),
            ram_mib: self.ram_mib.saturating_add(rhs.ram_mib),
            disk_gib: self.disk_gib.saturating_add(rhs.disk_gib),
            monthly_traffic_tb: self.monthly_traffic_tb.saturating_add(rhs.monthly_traffic_tb),
            ipv4: self.ipv4.saturating_add(rhs.ipv4),
            ipv6_prefixes: self.ipv6_prefixes.saturating_add(rhs.ipv6_prefixes),
        }
    }
}

/// A public product plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmPlan {
    /// Plan slug.
    pub name: String,
    /// Human-readable label.
    pub label: String,
    /// Resources reserved by this plan.
    pub resources: ResourceSet,
    /// Monthly price in euro cents.
    pub monthly_price_eur_cents: u32,
}

impl VmPlan {
    /// Build a plan.
    pub fn new(
        name: &str,
        label: &str,
        resources: ResourceSet,
        monthly_price_eur_cents: u32,
    ) -> Result<Self, FluxVisorError> {
        validate_slug("plan", name, 2, 48)?;
        if resources.vcpu == 0 || resources.ram_mib == 0 || resources.disk_gib == 0 {
            return Err(FluxVisorError::InvalidCapacity("plan must have non-zero cpu, ram, and disk"));
        }
        Ok(Self {
            name: name.to_string(),
            label: label.to_string(),
            resources,
            monthly_price_eur_cents,
        })
    }
}

/// A small invite-only FluxHost alpha catalog.
pub fn fluxhost_alpha_plans() -> Vec<VmPlan> {
    vec![
        VmPlan::new(
            "small",
            "Small VPS",
            ResourceSet {
                vcpu: 2,
                ram_mib: 4 * 1024,
                disk_gib: 120,
                monthly_traffic_tb: 5,
                ipv4: 1,
                ipv6_prefixes: 1,
            },
            1_200,
        )
        .expect("static plan"),
        VmPlan::new(
            "builder",
            "Builder VPS",
            ResourceSet {
                vcpu: 4,
                ram_mib: 12 * 1024,
                disk_gib: 350,
                monthly_traffic_tb: 12,
                ipv4: 1,
                ipv6_prefixes: 1,
            },
            3_500,
        )
        .expect("static plan"),
        VmPlan::new(
            "storage",
            "Storage/Bandwidth Box",
            ResourceSet {
                vcpu: 2,
                ram_mib: 4 * 1024,
                disk_gib: 1_500,
                monthly_traffic_tb: 30,
                ipv4: 1,
                ipv6_prefixes: 1,
            },
            4_900,
        )
        .expect("static plan"),
        // Disk-dense, RAM- and traffic-light. This is the plan that actually
        // monetizes a storage box like Epsilon (88 TB of HDD): customers park
        // terabytes but rarely move them, so RAM and egress stay cheap.
        VmPlan::new(
            "cold-storage",
            "Cold Storage / Backup Target",
            ResourceSet {
                vcpu: 1,
                ram_mib: 1024,
                disk_gib: 8 * 1024,
                monthly_traffic_tb: 2,
                ipv4: 0,
                ipv6_prefixes: 1,
            },
            3_000,
        )
        .expect("static plan"),
    ]
}

/// Guest-sellable [`HostProfile`] for Server Epsilon (the €220/month lease)
/// while the Quillon node co-resides on the box.
///
/// Raw hardware: 2× Xeon Gold 5118 (48 threads), 64 GB RAM, 2 TB NVMe + 4× 22 TB
/// HDD (~88 TB), 100 TB/month on a 1 Gbit port. The numbers below are what is
/// left for guests after reserving headroom for the node and OS:
///
/// - ~8 threads + ~38 GB RAM held for the q-api-server memory latch + OS,
/// - the NVMe kept for the node's chain DB (guest pool lives on the HDDs),
/// - ~30 TB/month of traffic reserved for node sync / P2P.
///
/// The point this profile makes: on Epsilon **disk is abundant and RAM/traffic
/// are scarce**, so the disk-dense `cold-storage` plan is what clears the lease.
/// See the `epsilon_cold_storage_clears_the_lease` test.
pub fn epsilon_host_profile() -> HostProfile {
    HostProfile::new(
        "epsilon-host-01",
        Backend::LibvirtKvm,
        HostCapacity {
            vcpu_threads: 40,
            ram_mib: 24 * 1024,
            disk_gib: 80_000,
            monthly_traffic_tb: 70,
            ipv4: 4,
            ipv6_prefixes: 16,
        },
        OvercommitPolicy::default(),
        NetworkProfile::new("br0", "eno1", 1_000).expect("static network profile"),
    )
    .expect("static epsilon profile")
}

/// Boot image metadata accepted by the planner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageSpec {
    /// Image family, for executor routing.
    pub family: ImageFamily,
    /// Version string or release codename.
    pub version: String,
    /// BLAKE3 digest of the base image.
    pub blake3_hex: String,
}

impl ImageSpec {
    /// Build and validate an image spec.
    pub fn new(family: ImageFamily, version: &str, blake3_hex: &str) -> Result<Self, FluxVisorError> {
        validate_slug("image_version", version, 1, 32)?;
        if blake3_hex.len() != 64 || !blake3_hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(FluxVisorError::InvalidImageDigest(blake3_hex.to_string()));
        }
        Ok(Self {
            family,
            version: version.to_string(),
            blake3_hex: blake3_hex.to_ascii_lowercase(),
        })
    }
}

/// Supported base image families.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageFamily {
    /// Debian cloud image.
    Debian,
    /// Ubuntu cloud image.
    Ubuntu,
    /// Alpine cloud image.
    Alpine,
    /// NixOS image.
    NixOs,
}

/// A customer's requested VM.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmRequest {
    /// Tenant id.
    pub tenant: TenantId,
    /// VM name.
    pub vm_name: VmName,
    /// Requested plan slug.
    pub plan: String,
    /// Boot image.
    pub image: ImageSpec,
    /// Optional public SSH key to inject through cloud-init.
    pub ssh_public_key: Option<String>,
}

impl VmRequest {
    /// Build and validate a VM request.
    pub fn new(
        tenant: TenantId,
        vm_name: VmName,
        plan: &str,
        image: ImageSpec,
        ssh_public_key: Option<String>,
    ) -> Result<Self, FluxVisorError> {
        validate_slug("plan", plan, 2, 48)?;
        if let Some(key) = ssh_public_key.as_deref() {
            validate_ssh_public_key(key)?;
        }
        Ok(Self {
            tenant,
            vm_name,
            plan: plan.to_string(),
            image,
            ssh_public_key,
        })
    }
}

fn validate_ssh_public_key(key: &str) -> Result<(), FluxVisorError> {
    let mut parts = key.split_whitespace();
    let Some(kind) = parts.next() else {
        return Err(FluxVisorError::InvalidSshKey);
    };
    let Some(material) = parts.next() else {
        return Err(FluxVisorError::InvalidSshKey);
    };
    if !matches!(kind, "ssh-ed25519" | "ssh-rsa" | "ecdsa-sha2-nistp256") {
        return Err(FluxVisorError::InvalidSshKey);
    }
    if material.len() < 32 || material.bytes().any(|b| b.is_ascii_control()) {
        return Err(FluxVisorError::InvalidSshKey);
    }
    Ok(())
}

/// A capacity reservation for one VM.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reservation {
    /// Tenant id.
    pub tenant: TenantId,
    /// VM name.
    pub vm_name: VmName,
    /// Plan slug.
    pub plan: String,
    /// Reserved resources.
    pub resources: ResourceSet,
}

impl Reservation {
    /// Unique reservation key.
    pub fn key(&self) -> String {
        format!("{}/{}", self.tenant.as_str(), self.vm_name.as_str())
    }
}

/// In-memory host capacity ledger.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapacityLedger {
    /// Host profile.
    pub host: HostProfile,
    /// Current reservations keyed by `tenant/vm`.
    pub reservations: BTreeMap<String, Reservation>,
}

impl CapacityLedger {
    /// Empty ledger for a host.
    pub fn new(host: HostProfile) -> Self {
        Self {
            host,
            reservations: BTreeMap::new(),
        }
    }

    /// Total reserved resources.
    pub fn used(&self) -> ResourceSet {
        self.reservations
            .values()
            .fold(ResourceSet::default(), |acc, r| acc.saturating_add(r.resources))
    }

    /// Remaining sellable capacity.
    pub fn remaining(&self) -> ResourceSet {
        let cap = self.host.sellable_capacity();
        let used = self.used();
        ResourceSet {
            vcpu: cap.vcpu_threads.saturating_sub(used.vcpu),
            ram_mib: cap.ram_mib.saturating_sub(used.ram_mib),
            disk_gib: cap.disk_gib.saturating_sub(used.disk_gib),
            monthly_traffic_tb: cap.monthly_traffic_tb.saturating_sub(used.monthly_traffic_tb),
            ipv4: cap.ipv4.saturating_sub(used.ipv4),
            ipv6_prefixes: cap.ipv6_prefixes.saturating_sub(used.ipv6_prefixes),
        }
    }

    /// Try to reserve capacity.
    pub fn reserve(&mut self, reservation: Reservation) -> Result<(), FluxVisorError> {
        let key = reservation.key();
        if self.reservations.contains_key(&key) {
            return Err(FluxVisorError::DuplicateReservation(key));
        }
        ensure_fits(&self.host.sellable_capacity(), self.used().saturating_add(reservation.resources))?;
        self.reservations.insert(key, reservation);
        Ok(())
    }
}

fn ensure_fits(cap: &HostCapacity, used: ResourceSet) -> Result<(), FluxVisorError> {
    let checks = [
        ("vcpu", used.vcpu as u64, cap.vcpu_threads as u64),
        ("ram_mib", used.ram_mib, cap.ram_mib),
        ("disk_gib", used.disk_gib, cap.disk_gib),
        (
            "monthly_traffic_tb",
            used.monthly_traffic_tb as u64,
            cap.monthly_traffic_tb as u64,
        ),
        ("ipv4", used.ipv4 as u64, cap.ipv4 as u64),
        ("ipv6_prefixes", used.ipv6_prefixes as u64, cap.ipv6_prefixes as u64),
    ];
    for (resource, requested, available) in checks {
        if requested > available {
            return Err(FluxVisorError::CapacityExceeded {
                resource,
                requested,
                available,
            });
        }
    }
    Ok(())
}

/// One backend action a future executor can perform.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendAction {
    /// Create or clone a guest disk.
    CreateDisk {
        /// VM name.
        vm: VmName,
        /// Disk size in GiB.
        disk_gib: u64,
        /// Base image digest.
        image_blake3: String,
    },
    /// Create a cloud-init seed volume.
    WriteCloudInit {
        /// VM name.
        vm: VmName,
        /// Tenant id.
        tenant: TenantId,
        /// SSH key, if configured.
        ssh_public_key: Option<String>,
    },
    /// Define the VM in the selected backend.
    DefineVm {
        /// VM name.
        vm: VmName,
        /// vCPU count.
        vcpu: u16,
        /// RAM in MiB.
        ram_mib: u64,
        /// Backend.
        backend: Backend,
    },
    /// Attach VM to a host bridge.
    AttachBridge {
        /// VM name.
        vm: VmName,
        /// Bridge name.
        bridge: String,
    },
    /// Apply a traffic quota/shape.
    ApplyTrafficPolicy {
        /// VM name.
        vm: VmName,
        /// Monthly traffic in TB.
        monthly_traffic_tb: u32,
        /// Average rate implied by the traffic plan.
        avg_mbps: u32,
    },
    /// Start the VM.
    StartVm {
        /// VM name.
        vm: VmName,
    },
}

/// A dry-run provisioning plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvisionPlan {
    /// Host name selected for the VM.
    pub host: String,
    /// Reservation that should be committed if execution succeeds.
    pub reservation: Reservation,
    /// Backend actions, in order.
    pub actions: Vec<BackendAction>,
    /// Human-readable warnings for the operator.
    pub warnings: Vec<String>,
}

/// Build a dry-run provisioning plan without mutating the host.
pub fn plan_vm(
    ledger: &CapacityLedger,
    catalog: &[VmPlan],
    request: VmRequest,
) -> Result<ProvisionPlan, FluxVisorError> {
    let plan = catalog
        .iter()
        .find(|p| p.name == request.plan)
        .ok_or_else(|| FluxVisorError::UnknownPlan(request.plan.clone()))?;

    let reservation = Reservation {
        tenant: request.tenant.clone(),
        vm_name: request.vm_name.clone(),
        plan: plan.name.clone(),
        resources: plan.resources,
    };

    let mut simulated = ledger.clone();
    simulated.reserve(reservation.clone())?;

    let avg_mbps = traffic_tb_month_to_mbps(plan.resources.monthly_traffic_tb)
        .ceil()
        .max(1.0) as u32;
    let mut warnings = Vec::new();
    if avg_mbps > ledger.host.network.port_mbps {
        warnings.push(format!(
            "plan average {} Mbps exceeds host port {} Mbps",
            avg_mbps, ledger.host.network.port_mbps
        ));
    }
    if matches!(ledger.host.backend, Backend::Firecracker) && plan.resources.disk_gib > 2048 {
        warnings.push("large storage plans on Firecracker should use a separate block backend".to_string());
    }

    let actions = vec![
        BackendAction::CreateDisk {
            vm: request.vm_name.clone(),
            disk_gib: plan.resources.disk_gib,
            image_blake3: request.image.blake3_hex.clone(),
        },
        BackendAction::WriteCloudInit {
            vm: request.vm_name.clone(),
            tenant: request.tenant.clone(),
            ssh_public_key: request.ssh_public_key.clone(),
        },
        BackendAction::DefineVm {
            vm: request.vm_name.clone(),
            vcpu: plan.resources.vcpu,
            ram_mib: plan.resources.ram_mib,
            backend: ledger.host.backend,
        },
        BackendAction::AttachBridge {
            vm: request.vm_name.clone(),
            bridge: ledger.host.network.bridge.clone(),
        },
        BackendAction::ApplyTrafficPolicy {
            vm: request.vm_name.clone(),
            monthly_traffic_tb: plan.resources.monthly_traffic_tb,
            avg_mbps,
        },
        BackendAction::StartVm {
            vm: request.vm_name.clone(),
        },
    ];

    Ok(ProvisionPlan {
        host: ledger.host.name.clone(),
        reservation,
        actions,
        warnings,
    })
}

/// Errors produced by flux-visor.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FluxVisorError {
    /// Identifier failed validation.
    #[error("invalid {field} `{value}`: {reason}")]
    InvalidIdentifier {
        /// Field name.
        field: &'static str,
        /// Rejected value.
        value: String,
        /// Rejection reason.
        reason: String,
    },
    /// Capacity value is invalid.
    #[error("invalid capacity: {0}")]
    InvalidCapacity(&'static str),
    /// Overcommit ratio is invalid.
    #[error("invalid overcommit {field}={value}; must be >= 1000 per-mille")]
    InvalidOvercommit {
        /// Field name.
        field: &'static str,
        /// Rejected value.
        value: u16,
    },
    /// Base image digest is invalid.
    #[error("invalid image BLAKE3 digest `{0}`")]
    InvalidImageDigest(String),
    /// SSH key is malformed or unsupported.
    #[error("invalid ssh public key")]
    InvalidSshKey,
    /// P2P cluster configuration is invalid.
    #[error("invalid p2p config: {0}")]
    InvalidP2pConfig(String),
    /// Requested plan is not in the catalog.
    #[error("unknown plan `{0}`")]
    UnknownPlan(String),
    /// Requested host is not in the cluster.
    #[error("unknown host `{0}`")]
    UnknownHost(String),
    /// Reservation already exists.
    #[error("duplicate reservation `{0}`")]
    DuplicateReservation(String),
    /// Host capacity would be exceeded.
    #[error("capacity exceeded for {resource}: requested {requested}, available {available}")]
    CapacityExceeded {
        /// Resource name.
        resource: &'static str,
        /// Requested amount.
        requested: u64,
        /// Available amount.
        available: u64,
    },
    /// A heartbeat payload could not be serialized for the wire.
    #[error("heartbeat serialization failed: {0}")]
    Serialization(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> HostProfile {
        HostProfile::new(
            "epsilon_host_01",
            Backend::LibvirtKvm,
            HostCapacity {
                vcpu_threads: 32,
                ram_mib: 64 * 1024,
                disk_gib: 4_000,
                monthly_traffic_tb: 100,
                ipv4: 8,
                ipv6_prefixes: 64,
            },
            OvercommitPolicy::default(),
            NetworkProfile::new("br0", "eno1", 1_000).unwrap(),
        )
        .unwrap()
    }

    fn image() -> ImageSpec {
        ImageSpec::new(ImageFamily::Debian, "12", "a".repeat(64).as_str()).unwrap()
    }

    #[test]
    fn traffic_quota_math_is_honest() {
        let mbps = traffic_tb_month_to_mbps(100);
        assert!((mbps - 308.64).abs() < 0.5);
    }

    #[test]
    fn alpha_catalog_has_four_sellable_plans() {
        let plans = fluxhost_alpha_plans();
        assert_eq!(plans.len(), 4);
        assert!(plans.iter().any(|p| p.name == "small"));
        assert!(plans.iter().any(|p| p.name == "builder"));
        assert!(plans.iter().any(|p| p.name == "storage"));
        assert!(plans.iter().any(|p| p.name == "cold-storage"));
    }

    #[test]
    fn epsilon_profile_has_expected_guest_capacity() {
        let host = epsilon_host_profile();
        assert_eq!(host.name, "epsilon-host-01");
        let cap = host.sellable_capacity();
        assert_eq!(cap.disk_gib, 80_000);
        assert_eq!(cap.ram_mib, 24 * 1024);
        assert_eq!(cap.monthly_traffic_tb, 70);
    }

    #[test]
    fn cold_storage_is_disk_bound_on_epsilon() {
        let host = epsilon_host_profile();
        let cold = fluxhost_alpha_plans()
            .into_iter()
            .find(|p| p.name == "cold-storage")
            .unwrap();
        // For a disk-dense plan, disk is the scarce resource — RAM and traffic
        // each allow many more units than disk does.
        let cap = host.sellable_capacity();
        let by_disk = cap.disk_gib / cold.resources.disk_gib;
        let by_ram = cap.ram_mib / cold.resources.ram_mib;
        let by_traffic =
            cap.monthly_traffic_tb as u64 / cold.resources.monthly_traffic_tb as u64;
        assert_eq!(host.max_units(&cold) as u64, by_disk);
        assert!(by_disk < by_ram, "disk must bind before RAM");
        assert!(by_disk < by_traffic, "disk must bind before traffic");
    }

    #[test]
    fn epsilon_cold_storage_clears_the_lease() {
        // The €220/month lease (22_000 euro cents) must be covered by
        // cold-storage at full occupancy, with margin.
        let host = epsilon_host_profile();
        let catalog = fluxhost_alpha_plans();
        let cold = catalog.iter().find(|p| p.name == "cold-storage").unwrap();

        let units = host.max_units(cold);
        let revenue = host.max_monthly_revenue_cents(cold);
        assert!(units >= 8, "expected >= 8 cold-storage units, got {units}");
        assert!(
            revenue >= 22_000,
            "cold-storage revenue {revenue}c must clear the €220 (22000c) lease"
        );

        // The contrast that justifies the new plan: the compute-oriented
        // `storage` VPS alone does NOT cover the lease on this box, because it
        // is RAM/traffic-bound and leaves the 88 TB of disk on the floor.
        let storage = catalog.iter().find(|p| p.name == "storage").unwrap();
        assert!(
            host.max_monthly_revenue_cents(storage) < 22_000,
            "storage VPS alone should not cover the lease — that's why cold-storage exists"
        );
    }

    #[test]
    fn plans_vm_without_mutating_ledger() {
        let ledger = CapacityLedger::new(host());
        let request = VmRequest::new(
            TenantId::new("viktor").unwrap(),
            VmName::new("builder-01").unwrap(),
            "builder",
            image(),
            Some("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGx1eHlmbHV4bWFjaGluZQ== viktor".to_string()),
        )
        .unwrap();

        let plan = plan_vm(&ledger, &fluxhost_alpha_plans(), request).unwrap();

        assert_eq!(ledger.used(), ResourceSet::default());
        assert_eq!(plan.host, "epsilon_host_01");
        assert_eq!(plan.reservation.resources.vcpu, 4);
        assert_eq!(plan.actions.len(), 6);
        assert!(matches!(plan.actions[0], BackendAction::CreateDisk { .. }));
    }

    #[test]
    fn reserve_rejects_ram_overcommit() {
        let mut ledger = CapacityLedger::new(host());
        let catalog = fluxhost_alpha_plans();
        for i in 0..5 {
            let plan = catalog.iter().find(|p| p.name == "builder").unwrap();
            ledger
                .reserve(Reservation {
                    tenant: TenantId::new("team").unwrap(),
                    vm_name: VmName::new(format!("builder-{i}")).unwrap(),
                    plan: plan.name.clone(),
                    resources: plan.resources,
                })
                .unwrap();
        }
        let plan = catalog.iter().find(|p| p.name == "builder").unwrap();
        let err = ledger
            .reserve(Reservation {
                tenant: TenantId::new("team").unwrap(),
                vm_name: VmName::new("builder-6").unwrap(),
                plan: plan.name.clone(),
                resources: plan.resources,
            })
            .unwrap_err();
        assert!(matches!(
            err,
            FluxVisorError::CapacityExceeded {
                resource: "ram_mib",
                ..
            }
        ));
    }

    #[test]
    fn rejects_shell_shaped_names_before_backend_planning() {
        let err = VmName::new("vm;rm-rf").unwrap_err();
        assert!(matches!(err, FluxVisorError::InvalidIdentifier { field: "vm_name", .. }));
    }

    #[test]
    fn unknown_plan_is_rejected() {
        let ledger = CapacityLedger::new(host());
        let request = VmRequest::new(
            TenantId::new("viktor").unwrap(),
            VmName::new("gpu-01").unwrap(),
            "gpu",
            image(),
            None,
        )
        .unwrap();
        assert_eq!(
            plan_vm(&ledger, &fluxhost_alpha_plans(), request).unwrap_err(),
            FluxVisorError::UnknownPlan("gpu".to_string())
        );
    }

    fn cluster() -> FluxP2pCluster {
        FluxP2pCluster::new(
            "fluxhost-alpha",
            vec![
                P2pHostNode::new(
                    "epsilon-host-01",
                    HostRole::Seed,
                    "/ip4/89.149.241.126/tcp/9003/p2p/12D3KooWEpsilonFluxVisorAlphaSeed111111111111",
                    9003,
                    true,
                )
                .unwrap(),
                P2pHostNode::new(
                    "delta-host-01",
                    HostRole::Worker,
                    "/ip4/5.79.79.158/tcp/9003/p2p/12D3KooWDeltaFluxVisorAlphaWorker1111111111111",
                    9003,
                    true,
                )
                .unwrap(),
                P2pHostNode::new(
                    "storage-host-01",
                    HostRole::Storage,
                    "/ip4/10.10.0.12/tcp/9003",
                    9003,
                    false,
                )
                .unwrap(),
            ],
        )
        .unwrap()
        .with_topic("/fluxvisor/1/billing-preview")
        .unwrap()
    }

    #[test]
    fn p2p_bootstrap_nodes_require_peer_ids() {
        let err = P2pHostNode::new(
            "epsilon-host-01",
            HostRole::Seed,
            "/ip4/89.149.241.126/tcp/9003",
            9003,
            true,
        )
        .unwrap_err();
        assert!(matches!(err, FluxVisorError::InvalidP2pConfig(_)));
    }

    #[test]
    fn p2p_network_config_excludes_self_from_bootstrap() {
        let cfg = cluster().network_config_for("epsilon-host-01").unwrap();
        assert_eq!(cfg.node_id, "fluxhost-alpha:epsilon-host-01");
        assert_eq!(cfg.listen_addr, "/ip4/0.0.0.0/tcp/9003");
        assert_eq!(cfg.bootstrap_peers.len(), 1);
        assert!(cfg.bootstrap_peers[0].contains("DeltaFluxVisorAlphaWorker"));
        assert!(cfg.gossipsub_topics.contains(&"/fluxvisor/1/capacity".to_string()));
        assert!(cfg.gossipsub_topics.contains(&"/fluxvisor/1/billing-preview".to_string()));
        assert!(cfg.dagknight_enabled);
        assert!(cfg.sap_enabled);
    }

    #[test]
    fn p2p_join_plan_is_operator_reviewable() {
        let plan = cluster().join_plan_for("storage-host-01").unwrap();
        assert_eq!(plan.host_id, "storage-host-01");
        assert_eq!(plan.actions.len(), 5);
        assert!(matches!(plan.actions[0], HostJoinAction::InstallFluxP2pService));
        match &plan.actions[1] {
            HostJoinAction::WriteNetworkConfig { config } => {
                assert_eq!(config.node_id, "fluxhost-alpha:storage-host-01");
                assert_eq!(config.bootstrap_peers.len(), 2);
            }
            other => panic!("unexpected join action: {other:?}"),
        }
        assert!(matches!(plan.actions[2], HostJoinAction::OpenTcpPort { port: 9003 }));
    }
}
