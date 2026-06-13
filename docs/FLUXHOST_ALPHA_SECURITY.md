# FluxHost alpha — security runbook (board T3)

Threat model and go-live gate for the first paid FluxHost alpha. The machine-
checkable form of this checklist lives in `crates/flux-visor/src/security.rs`
(`alpha_security_checklist()`), so the claims here are backed by tests, not prose
alone. `ready_for_paid_customers()` returns **false** while any control is
`NotYetImplemented` — and it currently does. **We are not cleared to sell.**

This runbook was sharpened by a second-mind review (DeepSeek-V4-pro, 2026-06-13);
the controls it surfaced (CPU side-channels, disk encryption, IMDS SSRF, noisy-
neighbor QoS, live-migration trust, patch cadence, cloud-init secrets, audit
trail, TOCTOU reservation lock) are folded into the checklist below.

## Threat actors

- **Malicious tenant** — a paying customer who attacks the host, the fleet, or
  neighbouring tenants from inside their own VM.
- **Internet attacker** — unauthenticated, probing exposed ports / panels.
- **Compromised guest** — a tenant VM taken over and used as a pivot.
- **Abusive tenant** — uses the VM for spam/DoS/illegal content (not a breach,
  but a takedown/liability problem).

## What FluxVisor enforces in code today

These are upheld by the type system or a test (`status = EnforcedInCode`):

- **No live executor.** Only `DryRunExecutor` ships; there is no `HostExecutor`
  impl. Provisioning renders commands, runs nothing. `ExecutionReport::is_dry_run()`.
- **Injection-safe identifiers.** `validate_slug` restricts tenant/VM/bridge ids
  to `[A-Za-z0-9_-]` *before* any action is built, so path-traversal and shell
  metacharacters cannot reach a rendered command. (A live executor must still use
  an argv vector — `Command::args` — never `sh -c`.)
- **Verified base image.** `ImageSpec` rejects a non-64-hex BLAKE3 digest; disks
  are per-VM overlays off a pinned backing file.
- **IPv4 rationing.** The capacity ledger caps IPv4 allocation; `cold-storage`
  defaults to IPv6-only (`ipv4=0`).

## What is operator policy (must be configured before the first customer)

`status = PolicyRequired` — FluxVisor cannot enforce these from the control
plane; the host operator must:

- keep the admin plane (deploy panel, q-flux admin routes, node API :8080)
  unreachable from guests and the public internet without auth;
- keep fleet SSH trust off the guest bridge;
- isolate tenants at L2 (per-tenant VLAN/bridge or ebtables) and anti-spoof egress;
- enable CPU side-channel mitigations (microcode, L1TF/MDS, SMT policy, vCPU pin);
- encrypt guest disks per tenant and wipe-on-delete the HDD pool;
- enforce noisy-neighbor QoS with cgroup CPU/blkio/memory limits (the no-oversell
  ledger is admission control only — the host must enforce it);
- keep cloud-init/config-drive secret-free; inject ephemeral per-instance tokens;
- scope any metadata endpoint per tenant (prefer config-drive over network IMDS);
- state guest data durability honestly (the 4×22 TB pool is not a backup of itself).

## Go-live blockers (must be built — host MUST NOT take paid customers)

`status = NotYetImplemented`. Each is a hard gate:

| id | gate |
|---|---|
| `host-residency` | Customer VMs must **not** co-reside with the production chain node. The first paid host is a fresh dedicated server; Epsilon stays seed/control only. |
| `default-deny-firewall` | A default-deny nftables ruleset (mgmt ports never exposed) must be authored and verified. No firewall automation exists yet. |
| `abuse-desk` | The `/fluxvisor/1/abuse-event` topic is reserved but has no handler or desk. Need a monitored contact + suspend/takedown procedure. |
| `provider-resale-terms` | Confirm the upstream provider's ToS permits VM resale on the leased box. Assume forbidden until read. |
| `reservation-lock-toctou` | `plan_vm` reserves on a *cloned* ledger; nothing holds the reservation to provision time. Need a soft reservation (TTL) + seq-CAS allocation against the worker's heartbeat seq. |
| `hypervisor-patch-cadence` | No patch/reboot orchestration. Define a <24h critical-CVE policy with tenant draining. |
| `host-action-audit-trail` | No immutable audit log of console/snapshot/migration/host-command actions. |

## Live-executor risks (the gap between "renders virsh" and "runs virsh")

When the operator-gated `LibvirtExecutor` is eventually built, these will bite:

- **No transactional semantics.** A mid-sequence failure (disk created, define
  fails) leaks an orphaned resource. The executor needs rollback/cleanup, not a
  linear script.
- **TOCTOU overcommit** (see `reservation-lock-toctou`).
- **Argv hygiene.** Render is for review; execution must use `Command::args`,
  never a shell string — even though identifiers are already slug-validated.

## Fleet-routing safety (capacity heartbeats → provisioning)

Before a seed routes provisioning on `CapacityHeartbeat` gossip, close:

- **Staleness** → soft reservation with TTL; pin before provision, release if unused.
- **Forgery** → mutually authenticated heartbeats (mTLS) + validate reports against
  known-hardware inventory / attestation; reject unsigned gossip.
- **Double-book** → optimistic locking: allocation request carries the last-seen
  `seq`; the worker allocates only if `seq` still matches.
- **Partition** → leased heartbeats with expiry; a worker that loses the seed
  stops accepting new allocations after a grace period; epoch-fence on reconnect.
- **Replay** → per-heartbeat nonce + sliding-window dedup.
- **Backpressure** → add a pressure score (load / steal time); the seed avoids
  routing to pressured workers.

## Verdict for the current target (Server Epsilon)

Epsilon is **not eligible** as a paid host: it runs the production chain node
(`host-residency` blocker) and is OOM-cycling (2026-06-13). It may serve only as
the seed/control node. The first paid FluxHost server must be fresh, joined as a
worker via a generated `HostJoinPlan`, publishing capacity heartbeats, with the
seven `NotYetImplemented` gates closed first.
