# FluxVisor alpha

FluxVisor is the Flux-native control plane for a tiny hosting alpha. It is not
trying to reimplement KVM, QEMU, libvirt, or Firecracker. Those are the
isolation backends. FluxVisor owns the sellable layer:

- product plans
- host capacity accounting
- VM request validation
- dry-run provisioning actions
- traffic-quota math
- future billing and abuse hooks

## First product shape

Start invite-only with one fresh dedicated server, not the current Epsilon
development host.

Suggested alpha plans:

| plan | vCPU | RAM | disk | traffic | €/mo |
|---|---:|---:|---:|---:|---:|
| small | 2 | 4 GiB | 120 GiB | 5 TB/mo | 12 |
| builder | 4 | 12 GiB | 350 GiB | 12 TB/mo | 35 |
| storage | 2 | 4 GiB | 1.5 TiB | 30 TB/mo | 49 |
| cold-storage | 1 | 1 GiB | 8 TiB | 2 TB/mo | 30 |

The alpha default is no RAM or disk oversell. CPU oversell should stay disabled
until the host has real utilization data.

### Why cold-storage exists — making an expensive box pay for itself

The compute plans (small/builder/storage) are bound by **RAM and traffic**, not
disk. On a storage-heavy box that strands the asset. Worked example —
Server Epsilon (the €220/month lease: 48 threads, 64 GB RAM, ~88 TB HDD,
100 TB/mo) modelled by `epsilon_host_profile()` with the node co-resident:

| plan | max units | bound by | revenue/mo |
|---|---:|---|---:|
| storage (VPS) | 2 | traffic | €98 |
| **cold-storage** | **9** | **disk** | **€270** |

The compute-oriented `storage` VPS alone (€98) does **not** cover the €220 lease.
`cold-storage` — disk-dense, RAM/traffic-light — is bound by disk instead, fills
the 88 TB, and clears the lease with margin. `HostProfile::max_units(plan)` /
`max_monthly_revenue_cents(plan)` compute this; the
`epsilon_cold_storage_clears_the_lease` test asserts it (all dry-run; nothing is
run on the box).

## Why dry-run first

The first safe milestone is not "start a VM." It is "prove we know what we
would start." A dry-run plan lets the operator review:

- requested tenant and VM identifiers
- selected base image digest
- reserved CPU/RAM/disk/IP/traffic
- backend actions in order
- warning flags, such as a plan implying more average bandwidth than the port

Only after that is boring and tested should a privileged host executor be added.

## Bandwidth reality

`100 TB/month` is about `309 Mbit/s` sustained average over a 30-day month.
A `1 Gbit/s` port can burst higher, but selling "1 Gbit" does not mean the plan
can sustain a full gigabit forever unless the traffic quota and upstream bill
are priced for it.

## Backend lane

The natural first executor is libvirt/KVM:

1. clone or create a qcow2 disk from a verified cloud image
2. write a cloud-init seed
3. define the VM
4. attach it to a controlled bridge
5. apply traffic shaping
6. start the VM

Firecracker is attractive later for dense single-purpose microVMs. Direct QEMU
is useful for labs, not for a paid alpha.

## Horizontal scaling with flux-p2p

FluxVisor now has a typed host mesh model. Each physical server gets a
`flux_p2p::NetworkConfig` generated from a `FluxP2pCluster`, subscribes to the
FluxVisor control topics, and publishes capacity heartbeats.

Control topics:

- `/fluxvisor/1/host-heartbeat`
- `/fluxvisor/1/capacity`
- `/fluxvisor/1/provision-plan`
- `/fluxvisor/1/provision-result`
- `/fluxvisor/1/abuse-event`

Bootstrap nodes must advertise full libp2p multiaddrs including
`/p2p/<PeerId>`. A bare `/ip4/.../tcp/9003` address may open a socket but is not
enough for stable peer discovery.

First safe rollout:

1. keep Epsilon as the seed/control node
2. add one fresh worker server with its own FluxP2P identity
3. publish capacity heartbeats only
4. verify the control plane sees the worker
5. enable dry-run provisioning plans for that worker
6. only then connect a privileged libvirt/KVM executor

This makes "add more servers" a repeatable host-join plan instead of a manual
SSH ritual.
