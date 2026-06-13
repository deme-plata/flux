# FluxVisor next-session handoff

Date: 2026-06-13
Agent: vicarious-codex (P2P primitives) → rocky (Cortex bridge)
Repo: `/home/storage/deepseek-codewhale/flux`

## Update 2026-06-13 (rocky) — the cortex/platform bridge landed

The "Next safe step" below (`fluxvisor-cortex-platform-bridge`) is **done**.

New file: `crates/flux-visor/src/cortex_bridge.rs` (+ `pub mod cortex_bridge;` in
`lib.rs`, + `flux-cortex` path dep in `Cargo.toml`).

It maps the follow-up board onto the **real** `flux_cortex::ai_cortex::AiTaskKind`:

- `HostingTask::ExecutorDesign`  → `Review` + `Optimize`
- `HostingTask::HeartbeatDaemon` → `Test` + `Review`
- `HostingTask::SecurityRunbook` → `Review`
- `HostingTask::BillingWebhook`  → `WebhookGen`

`followup_board()` returns reviewable `PlatformTaskPlan`s. Safety is enforced by
construction: every plan is `DispatchMode::DryRun` with `PlanGuards` proving
`mutates_files=false, mutates_host=false, starts_services=false,
operator_approval_required=true`. A `Live` dispatch variant exists in the type
system but the bridge never constructs it — enabling it is a separate
operator-approved lane. No agent is dispatched; no clock/rng/env is read, so
routing is deterministic.

Verification: `flux_combo flux-visor` → **green, 19/19** (9 original + 10 bridge).
Provenance: `flux-rev` full id
`9b25ad0e2bb0721214a670579959222eba03cae87227bd78450755a75391d22e`.
Swarm: `rocky-289` settled 0.50 QUG.

⚠️ Still **uncommitted** — `crates/flux-visor/` + this doc are untracked. A
path-scoped `git add crates/flux-visor docs/FLUXVISOR_NEXT_SESSION_HANDOFF.md`
rescues it when the operator wants it committed.

### Update 2026-06-13 (rocky) — cold-storage + Epsilon profile + T1 executor

- **Cold-storage plan + Epsilon cost model:** added a disk-dense `cold-storage`
  plan, `HostProfile::max_units`/`max_monthly_revenue_cents`, and
  `epsilon_host_profile()`. Test `epsilon_cold_storage_clears_the_lease` proves
  9×€30=€270 (disk-bound) clears the €220 lease while the storage VPS (€98,
  traffic-bound) does not. Commit `de098992`.
- **Board T1 DONE — `DryRunExecutor`:** new `crates/flux-visor/src/executor.rs`.
  Trait `DryRunExecutor` + `LibvirtDryRun` renders the `qemu-img`/`cloud-localds`/
  `virt-install`/`virsh`/`tc` commands a libvirt executor would run, as
  `ExecutionStep`s marked `StepStatus::Planned`. `render_plan` is execution-free
  by construction (`executed=false`, all steps `Planned`); `report.is_dry_run()`
  proves it. **Runs nothing** — no `virsh`/`qemu`. The live `LibvirtExecutor` is
  intentionally a separate, operator-gated boundary (future `HostExecutor` trait
  + privilege token), not in this crate.

- **Board T2 DONE — capacity-heartbeat daemon:** new
  `crates/flux-visor/src/heartbeat.rs`. `HeartbeatDaemon::tick(ledger, uptime,
  healthy)` returns a `HostHeartbeat` (on `/fluxvisor/1/host-heartbeat`) + a
  `CapacityHeartbeat` (on `/fluxvisor/1/capacity`, carrying total/used/remaining
  `ResourceSet`s from the ledger). Pure: monotonic `seq`, `uptime_secs` passed in
  — no wall clock. Publishing is the injected `HeartbeatSink` boundary; the only
  sink shipped is the inert `RecordingSink` (no network). **No transport is
  started** — the live flux-p2p sink is the operator-gated step. Test
  `heartbeat_topics_present_in_generated_network_config` proves the daemon's
  topics are in the `NetworkConfig` a worker gets from
  `FluxP2pCluster::network_config_for` (the "NetworkConfig fit" gate). Added
  `serde_json` dep + `FluxVisorError::Serialization`.

`flux_combo flux-visor` → **green, 35/35**.

Remaining board: **T3** security runbook (`docs/FLUXHOST_ALPHA_SECURITY.md`),
billing/abuse `WebhookGen`. Plus two still-unbuilt niceties: a read-only Cortex
*preview* routing each `PlatformTaskPlan`'s modes through `AiCortex::route_task`,
and a fleet-side `CapacityHeartbeat` aggregator that picks the worker with the
most `remaining` for a given plan (the publish side now exists; the consume/route
side is the natural next step).

Next after this (pre-T1, now also done): wire dry-run plans into a Cortex
*preview* (route each plan's modes through `AiCortex::route_task` to show which
agent *would* take it — read-only, still no dispatch).

## Current objective

Build a Flux-native hosting control plane that can scale horizontally across
multiple bare-metal servers. The direction is not "clone all of Proxmox." The
direction is:

1. keep KVM/QEMU/libvirt or Firecracker as the isolation backend
2. let Flux own plans, capacity, P2P host discovery, dry-run provisioning, and
   eventually billing/abuse hooks
3. only add a privileged executor after dry-run plans and host heartbeats are
   boring and tested

## What landed

New files:

- `crates/flux-visor/Cargo.toml`
- `crates/flux-visor/src/lib.rs`
- `docs/FLUXVISOR_ALPHA.md`

`flux-visor` currently provides:

- alpha product catalog: `small`, `builder`, `storage`
- host capacity accounting: vCPU, RAM, disk, IPv4, IPv6, monthly traffic
- honest bandwidth math: `100 TB/month ~= 309 Mbit/s` average over 30 days
- tenant and VM identifier validation before backend actions exist
- dry-run VM provisioning actions:
  - create disk
  - write cloud-init
  - define VM
  - attach bridge
  - apply traffic policy
  - start VM
- capacity ledger with overcapacity rejection
- FluxP2P horizontal-scaling primitives:
  - `FLUXVISOR_P2P_TOPICS`
  - `HostRole`
  - `P2pHostNode`
  - `FluxP2pCluster`
  - `HostJoinPlan`
  - `HostJoinAction`
- per-host `flux_p2p::NetworkConfig` generation
- host-join plan actions:
  - install/verify FluxP2P service
  - write network config
  - open TCP listen port
  - start service
  - publish capacity heartbeat

Important guard: bootstrap nodes must advertise full libp2p multiaddrs with
`/p2p/<PeerId>`. Bare `/ip4/.../tcp/...` addresses are rejected for bootstrap
nodes because earlier SIGIL mesh work showed they can open sockets while still
failing stable peer registration.

## Verification

Passed on Epsilon:

```text
./target/debug/fluxc build --package flux-visor
./target/debug/fluxc test flux-visor
```

Latest result: `9 passed`.

Dependency warnings appeared in existing crates (`flux-cache`, `flux-sync`,
`flux-ai`, `flux-architect`, `flux-cortex`) but not as FluxVisor failures.

## Swarm state

Completed:

- `vicarious-codex-288` — FluxVisor FluxP2P horizontal-scaling primitives,
  settled for `0.50 QUG`.

Released:

- `vicarious-codex-289` — initially claimed for Cortex/platform-dev integration,
  but no code was implemented before this handoff. Released with no payment so
  the next session or another agent can claim cleanly.

Messages:

- `#220` — follow-up task board opened
- `#221` — FluxVisor/P2P completion broadcast

Open follow-up board from `#220`:

- `T1 fluxvisor-libvirt-executor-dryrun`
  - Design executor boundary only.
  - Add/critique trait shape for `DryRunExecutor` and future `LibvirtExecutor`.
  - Do not run `virsh`, `qemu`, or mutate the host.
- `T2 fluxvisor-p2p-heartbeat-daemon`
  - Design worker capacity heartbeats over `/fluxvisor/1/capacity` and
    `/fluxvisor/1/host-heartbeat`.
  - Verify topic naming and `NetworkConfig` fit existing `flux-p2p`.
  - Do not start/restart services.
- `T3 fluxhost-alpha-security-runbook`
  - Threat model first paid alpha: admin exposure, VM isolation, bridges,
    firewall, abuse desk, backups, IPv4 scarcity, provider resale rules.
  - Suggested deliverable: `docs/FLUXHOST_ALPHA_SECURITY.md`.

## Current repo dirtiness

FluxVisor-owned untracked files:

- `crates/flux-visor/`
- `docs/FLUXVISOR_ALPHA.md`
- this handoff document

Other dirty files existed before/around this work and should not be reverted
without checking ownership:

- `Cargo.lock`
- `Cargo.toml`
- `RELEASES.md`
- `crates/fluxc-core/src/distributed.rs`
- `crates/fluxc-core/src/heatmap.rs`
- `crates/fluxc-core/src/release_audit.rs`
- `docs/RELEASE_0.36.1_PLAN.md`
- `docs/flux-hunyuan3d-pipeline.md`
- `sagas/`

## Next safe step

Recommended first task next session:

`fluxvisor-cortex-platform-bridge`

Goal: use Flux Cortex and the AI-native platform-dev vocabulary without giving
it host privileges yet.

Suggested implementation:

1. Add a FluxVisor "platform task" adapter that maps hosting tasks onto
   `flux_cortex::ai_cortex::AiTaskKind`.
2. Proposed mappings:
   - executor design -> `Review` or `Optimize`
   - heartbeat daemon -> `Test` plus `Review`
   - security runbook -> `Review`
   - webhook/billing hooks -> `WebhookGen`
3. Return reviewable task plans only. Do not dispatch real agents that mutate
   files or hosts until the operator approves that lane.
4. Add tests proving routing stays dry-run and deterministic.

Then build/test:

```text
./target/debug/fluxc build --package flux-visor
./target/debug/fluxc test flux-visor
```

## Do not do yet

- Do not provision real customer VMs.
- Do not expose a libvirt/Proxmox/admin panel publicly.
- Do not start or restart FluxP2P services.
- Do not mutate firewall rules.
- Do not sell public plans from the current Epsilon dev/prod host.

The next production-like machine should be a fresh dedicated server, joined as a
worker through a generated `HostJoinPlan`, publishing capacity heartbeats before
any privileged executor is enabled.
