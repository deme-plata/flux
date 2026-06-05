# Variational Flow (VF) — A Development Methodology for the Agentic Money / Post-Quantum Era

**Author:** rocky-sigil
**Date:** 2026-05-30
**Status:** v0 — derived from one swarm session, awaits 5-release validation
**For:** any Flux substrate project (SIGIL, Quillon, future chains) using fluxc + agentic settlement

---

## One-line definition

> *Development converges toward a stationary action $\delta\mathcal{S} = 0$ through pull-based lane claims, settled cryptographically, witnessed by quorum, validated in multiverse before mainline.*

## Why VarFlow exists

Every prior methodology assumes humans with bandwidth caps, hierarchical orgs with PMs/EMs/ICs, fiat payroll, single-timeline development, and crypto as a peripheral feature. The agentic money / post-quantum era inverts each assumption:

- AI agents fork freely; 17 can collaborate where humans need a hierarchy
- Settlement is automatic on-chain; payroll is anachronistic
- Multiverse simulation (flux-chronos) is a development primitive
- Cryptographic provenance (`.proof`, SQIsign-L5, hardware attestation) ships with every artifact
- The master equation says SIGIL IS a variational principle — VarFlow makes the *development of* SIGIL the same shape

## The 7 axioms

### 1. The Action IS the contract
Each release has an action functional $\mathcal{S}$. Lanes are field variations. Settlement is the saddle-point check. Code converges toward $\delta\mathcal{S} = 0$.

*Replaces:* requirements docs, user stories.

### 2. Proof IS provenance
Every artifact carries a `.proof` bundle binding it to {agent, source-tree, hardware (when attested)}. No artifact ships without provenance. Verification is a three-check ritual: BLAKE3 + SQIsign signature + source tree match.

*Replaces:* trust-based code review, "tested on my machine."

### 3. Lanes are bounties
Pull-based, not assigned. Each lane has a measurement gate (the criterion), an appetite (max QUG/effort), and wave-dependency declarations. Settlement reflects realized value.

*Replaces:* top-down task assignment, story-point estimation.

### 4. The bottleneck moves; relocate it
Every release explicitly names its current constraint (via measurement, not opinion). The release scope IS the relocation of that constraint. Stargate 500M → "crypto is the wall" → P7 exists for that reason.

*Replaces:* "velocity" metrics, sprint goals.

### 5. Multiverse before mainline
Every consensus-critical change runs in flux-chronos across N seeded scenarios first. Only universal survivors merge to mainline. Adversarial scenarios are part of the merge gate.

*Replaces:* staging environments, integration testing as separate phase.

### 6. Honest pretend gates honest ship
Every spec doc has a "what's still pretend" section. Every release README has an "IS NOT" section. "Tested + working" claims forbidden until soak gate passes. The template structurally prevents aspirational readme syndrome.

*Replaces:* "definition of done" by committee.

### 7. Quorum replaces hierarchy
Decisions need M-of-N agent attestation (signed). No PM, no EM, no Tech Lead — just staked witnesses. Disputes resolve via flux-court when it lands.

*Replaces:* org charts, approval workflows.

---

## Transient functions (not permanent roles)

VarFlow has no permanent roles. Per-lane, functions rotate:

| Function | Responsibility |
|---|---|
| **Lane Author** | Drafts spec doc, sets gate, sizes appetite |
| **Lane Claimant** | Pulls lane, ships the work, settles |
| **Witness** | Staked attestor; verifies claim before settlement releases |
| **Operator** | Human who sizes releases + approves cross-prototype directives |
| **Architect** | Master-equation framing + `flux_architect_predict` + composition graph reasoning |

The same agent can be all five across different lanes in the same day. No identity locks to function.

## Practices (the actual rituals)

### Action Drift Check
At session start: read activity log + inbox + lane catalog. The lane catalog IS the standup. No daily synchronization; async by default.

### Variational Review
Before any code change: ask "which term in $\mathcal{S}$ does this touch?" If none, the change is orthogonal to consensus. If one, it composes with that term's existing math (use it). If two+, the change may be invasive; reconsider scope.

### Multiverse Pre-Ship
For consensus-critical changes: `flux_chronos_run` N seeded scenarios including adversarial ones (Byzantine validators, network partitions, replay attacks). Failure in ANY scenario halts the ship. Pass in ALL → eligible to merge.

### Quorum Settlement
For each lane completion: M witnesses sign over (claim_id, deliverable_hash). Settlement releases when threshold met. Honor system today; cryptographic via `flux_quorum_sign` after SWARM S6 ships.

### Honest Checklist (per artifact)
Four questions, every spec:
1. **What's the measurement gate?** (specific, falsifiable)
2. **What's still pretend?** (explicit list of unverified assumptions)
3. **What composition edges?** (which other lanes/crates does this touch)
4. **What's the rollback path?** (how do we undo this if soak fails)

### Soak Gate
24h continuous production observation before any release promotes from RC to stable. If any metric fails: REVERT, root-cause, ship a fix release.

---

## Operational discipline — the token-economy (DeepSeek-era lineage)

The 7 axioms are the WHAT. The token-economy is the HOW. Inherited from the DeepSeek session (the rocky lineage's predecessor on Flux) that proved you can run a 17-agent swarm without context blowups if you respect a small discipline. Without these, even good methodology burns tokens at the wrong layer.

### The inner loop is `flux_iterate`, not cold tool calls

The canonical inner-dev loop is **one** `flux_iterate` MCP call, NOT a sequence of `flux_combo` → `flux_qspec` → `flux_combo` → `flux_qspec`. The wrapper handles error → fix-suggestion → recompile → measure as one unit. Cold composition costs ~3× the tokens of `flux_iterate` for the same outcome.

### `flux_qspec` runs on error, not preemptively

Don't ask `flux_qspec` "what would help?" before there's an error. Run `flux_combo` first; let the error trigger `flux_qspec`. Pre-emptive specs over green code are pure noise.

### Phrasal verbs > tool-by-tool composition

Five composite tools (`flux_combo`, `flux_quickcast`, `flux_ult`, `flux_fullcheck`, `flux_quickstart`) bundle 3-5 primitive operations each. One phrasal-verb call replaces a chain of primitive calls + the LLM "thinking" between them. Token savings: 40-70% per inner-dev iteration vs primitive sequencing.

| When you'd reach for | Use instead |
|---|---|
| `flux_compile` + `flux_test` + `flux_predict` | `flux_combo` (1 call, all three) |
| Quick "is this even green?" check | `flux_quickcast` (compile-only, no tests) |
| Pre-merge final gate | `flux_ult` (compile + tests + audit + predict) |
| Pre-ship health snapshot | `flux_fullcheck` (full workspace + soak signal) |
| New scaffold cold-build | `flux_quickstart` (sets up FLUXFOOD + first compile) |

### `flux_batch_compile` for variants, not loops

When iterating across 2+ crates: one `flux_batch_compile packages=[...]` call, not N sequential `flux_combo` calls. Parallel under the hood + cache-aware. Linear-in-N token cost collapses to constant.

### Cache-aware compiles save the most tokens

`fluxc` content-hash cache (rocky-sigil-78, 6× speedup verified): same source content + same flags = identical cache key = NO recompile, NO log output, NO LLM context spent on parsing build noise. Honor the cache by ensuring `Cargo.lock` is committed + workspace.dependencies are harmonized (FLUXFOOD lever 1). Breaking the cache un-shielded costs 10× the tokens of a green incremental.

### Webhook feedback > polling

Per `[[feedback_flux_swarm_use_webhooks]]`: register `flux_webhook_register` at session start so your file-claims, build events, and swarm activity arrive asynchronously in your context. NEVER `flux_swarm_status` in a loop waiting for state to change. Token cost of polling = O(checks × tokens-per-status); cost of webhooks = O(events × tokens-per-event), almost always much cheaper.

### X-Algo predict + 1-shot feedback

Per `[[feedback_flux_predict_before_build]]`:
- `flux_architect_predict` ONCE before scaffolding (predict architectural cost)
- `flux_heatmap` ONCE before kicking off a multi-crate cold compile (catch the long-tail)
- `flux_predict` ONCE per batch (forecasts the build envelope)
- `flux_feedback` ONCE after with actual outcome → calibrates X-Algo for next prediction

Don't run predict in a loop. The model updates on feedback, not on speculation.

### Link-liberally over re-explain

Memory entries reference each other via `[[name]]` cross-references. So do broadcast messages (cite by msg# rather than re-summarizing). So do spec docs (cite `[[VARIATIONAL_FLOW_v0.md#axiom-5]]` instead of re-stating the axiom). A 5-character link replaces 50-500 tokens of restatement. The cost of writing a link is fixed; the cost of re-explaining grows with the explanation's depth.

### Honest pretend = token-economy too

Don't fabricate confidence to fill space. If you're unsure, write *"I'm not sure — could be X or Y; flux_qspec will tell us"*. That sentence is ~15 tokens. A speculative paragraph claiming false certainty is 200+ tokens AND wastes the human reader's verification time. Honest-pretend saves tokens on both sides of the conversation.

### `flux_ai_audit` is the merge gate, not a per-iteration check

Run `flux_ai_audit` once before settlement, not inside the inner-dev loop. It's an O(workspace) operation; inside the loop it's pure cost. Outside the loop (at the gate) it's the right tool.

### Honest activity tail > full status dumps

`flux_activity_tail limit=10` returns 10 events. `flux_swarm_status` returns everything (17 agents, all claims, all completions). Use activity_tail in the inner loop; use status only when you actively need the full picture.

### Soak monitoring without burning context

For R9-style 24h soak gates: a small monitoring script reads logs + posts to swarm broadcast on threshold violation. NEVER run a 24h monitoring loop inside the agent context — that's 24h of polling tokens. Push to background; consume only the violation events.

### Summary table — token-economy axioms (call them T1-T10)

| ID | Practice | Token savings |
|---|---|---|
| **T1** | `flux_iterate` for inner loops, not primitive chains | ~3× |
| **T2** | `flux_qspec` on error only, never preemptively | unbounded for no-error paths |
| **T3** | Phrasal verbs over primitive composition | 40-70% per iteration |
| **T4** | `flux_batch_compile` for parallel variants | O(N) → O(1) |
| **T5** | Honor content-hash cache; never break gratuitously | 10× on incremental |
| **T6** | Webhooks > polling | O(checks) → O(events) |
| **T7** | Predict ONCE before, feedback ONCE after | unbounded for prediction loops |
| **T8** | Link-liberally with `[[name]]` cross-references | 50-500 tokens per link |
| **T9** | Honest-pretend over speculative confidence | 150-200 tokens per fabricated paragraph |
| **T10** | `flux_ai_audit` at the gate, not in the loop | O(workspace) per skipped iteration |

These are operational, not axiomatic. They sit BELOW the 7 axioms — you can violate them and still ship under VarFlow; you'll just burn more tokens. But every burned token is paid for by someone (the agent's session-token budget, the swarm's settlement pool, the human's reading time). T1-T10 minimize the bill.

## Comparison to other methodologies

| Aspect | Waterfall | Scrum | Kanban | Bazaar (OSS) | **VarFlow** |
|---|---|---|---|---|---|
| Planning horizon | Months | 2 weeks | Continuous | None | Per-release (~3-7 days) |
| Roles | Hierarchical | PO + SM + Team | Limited | None | **Transient functions** |
| Task source | Top-down spec | PO backlog | Pull catalog | Anyone's TODO | **Pull catalog + appetite** |
| Settlement | Salary | Salary | Salary | Volunteer/reputation | **On-chain QUG** |
| Quality gate | UAT | Acceptance criteria | DoD | Maintainer review | **Soak + quorum + multiverse** |
| Failure | Big-bang reroll | Reprioritize | Stop-the-line | Fork the project | **Revert + root-cause + fix release** |
| Coordination | Meetings | Standups | WIP limits | Mailing lists | **Broadcasts + threads** |
| Provenance | Audit logs | Commit msg | Commit msg | Commit msg | **`.proof` + SQIsign** |

VarFlow inherits:
- **Pull-based flow** from Kanban
- **Theory of Constraints framing** from Goldratt
- **Bazaar dynamics** from open-source / ESR
- **Continuous deployment** from DevOps
- **Empirical loops** from XP/TDD
- **Token-economy discipline** from the DeepSeek-era rocky lineage (T1-T10 above)

VarFlow rejects:
- Daily standups (broadcasts replace)
- Sprint planning (lane catalog replaces)
- Story points (QUG market-prices)
- Retrospectives (honest-pretend section + memory entries replace)
- "Works on my machine" (provenance prevents)

## Falsifiable predictions

VarFlow predicts five measurable outcomes; each can falsify the methodology if it doesn't hold over ≥5 releases:

1. **Release cadence ≥3/month** after v0.0.6 (vs ~1/month today)
2. **Median lane completion <15 min** (vs ~30 min today)
3. **Attribution disputes approach zero** with quorum-attestation
4. **Multiverse pre-ship catches ≥80%** of consensus bugs (vs ~40% via informal review today)
5. **Operator time per release <30 min** (vs ~2h today, after cut-release.sh + auto-quorum-sign land)

If any fails after fair trial: revisit the relevant axiom.

## Honest section — what's still pretend about VarFlow

This is v0, derived from one session. Aspirational pieces:

- **Quorum attestation primitive doesn't exist yet** — SWARM S6 (`flux_quorum_sign`) must ship. Honor system today.
- **Multiverse pre-ship is partial** — flux-chronos runs scenarios but no MCP gate that blocks merge on simulation failure. Needs `flux_chronos_gate` to ship.
- **Auto-broadcast on claim** — SWARM S4 must ship. Today manual.
- **Cross-session attribution** — wallet-level works but "rocky vs rocky-sigil vs rocky-updater" disambiguation still requires reading swarm log. Quorum-attestation will tighten this.
- **Falsifiable predictions are guesses** — calibrated on one day of data. Real validation needs ≥5 releases.

If any prove intractable: VarFlow either evolves or retires. Methodologies are themselves variational — they minimize an action over their own usefulness.

## Composition with active work

| VarFlow rule | Active lane that implements it |
|---|---|
| Quorum replaces hierarchy | SWARM S6 + flux update-v1 #57 flux-quorum |
| Auto-broadcast on claim | SWARM S4 |
| Lane catalog (Axiom 3 in MCP form) | SWARM S2 |
| Multiverse pre-ship gate | **SWARM S8 `flux_chronos_gate`** (opened 2026-05-30) |
| Honest pretend templates | sigil/docs/honest-release-readme-template.md (shipped R5) |
| `.proof` provenance | shipping in v0.0.5 R3 |
| Tools advise methodology | **SWARM S9 `flux-varflow-advisor`** (opened 2026-05-30) — Generation 6 enabler |
| Methodology learns from its advisory log + session prefetch | **SWARM S10 `flux-session-prefetch + advisor-learn`** (opened 2026-05-30) — Generation 7 enabler |
| Multi-modal substrate (video as a first-class output) | **SWARM-V1 `flux-video-gen`** (opened 2026-05-30) — Generation 9 enabler · 1000× cheaper than Runway via Vast.ai + open-source models · introduces "probabilistic provenance" alongside the existing deterministic provenance |

VarFlow doesn't require new infrastructure beyond what's already being built. The methodology emerges naturally from the substrate work.

## How to start using VarFlow today

For agents:
1. Read this doc
2. Each lane you author or claim: apply the Honest Checklist
3. Each release you write: name the bottleneck explicitly
4. Each soak gate: don't promote until 24h passes
5. Each settlement: try to attest at least one other agent's claim (build the witness habit)

For Viktor (operator):
1. Size releases by appetite (3-7 days)
2. Approve cross-prototype directives (USDS, P7, etc.) at broadcast level
3. Don't approve individual lanes — let the swarm pull

For the substrate (Flux/SIGIL evolution):
1. Ship SWARM S1-S7 to make the methodology runnable
2. Add `flux_chronos_gate` for Axiom 5
3. After 5 releases, check the falsifiable predictions
4. Revise axioms based on outcomes

---

## Naming history

The name evolved during one session:
- "Bazaar Mesh" — emphasized the social model
- "Action-Pull Development" — too verbose
- "Variational Bazaar" — too academic
- **"Variational Flow"** ✓ — short, evocative, both rigorous (variational principle) and accessible (flow = Kanban echo)

VF for short.

---

— rocky-sigil, 2026-05-30
