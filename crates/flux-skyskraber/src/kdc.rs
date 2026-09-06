//! # K_DC — the Kristensen Datacenter Coordination Gauge
//!
//! A datacenter is not built by making every part fast. It is built by making the *one
//! part that binds* fast, and that part moves. This module measures where it is, predicts
//! when the building becomes real, and turns "I reduced the coordination stress" into a
//! number a node can actually produce — the way a miner produces hashes per second.
//!
//! ## The observation this is built on
//!
//! The datacenter whitepaper's schedule model is
//!
//! ```text
//! T = max(site + construction, grid, procurement) + commissioning
//! ```
//!
//! and `max` is the whole story. Measured in that model: give the robots 100 % of the
//! robot-suitable civil work at five times human speed, and construction collapses from
//! 20 months to 4 — an enormous local win that moves the completion date by **two
//! months**, because a 30-month grid connection still decides it. Delete the grid queue
//! and those same robots suddenly save 14 months, and then procurement is the wall.
//!
//! > **Optimisation does not remove constraints. It reveals the next one.**
//!
//! That is not a disappointment, it is the measurement. A gauge that scores local effort
//! would have called the robots a triumph. This one does not, and that is the point.
//!
//! ## The gauge
//!
//! State is a vector of constraints, each with what is REQUIRED, what is AVAILABLE, and
//! how long it still needs:
//!
//! ```text
//! ΔH_DC = Σ pᵢ · (1 − readinessᵢ)      how far the system is from itself
//! Δs_DC = −Σ pᵢ ln pᵢ                   how FRAGMENTED the reason is
//! K_DC  = 2π √( ΔH_DC · Δs_DC · τ/τ₀ )  dimensionless, engineering scale
//! ```
//!
//! ### Where `pᵢ` comes from, and why it is not a guess
//!
//! `pᵢ` is "the weight that constraint *i* is the next thing to bind". Asserting those
//! numbers by hand would make the entropy a matter of opinion. They are **derived from
//! slack** instead: each branch has a finish time `Tᵢ`, the project finishes at
//! `T = max Tᵢ`, and `slackᵢ = T − Tᵢ` is how far branch *i* is from deciding the date.
//!
//! ```text
//! pᵢ ∝ exp(−slackᵢ / θ)
//! ```
//!
//! A branch with zero slack is binding and dominates the distribution. A branch two years
//! clear of the critical path contributes almost nothing. So the entropy says exactly what
//! it should: **low when one constraint obviously decides everything ("it is the
//! transformer"), high when five things could each be the one that delays opening.**
//!
//! `θ` is the softness — how many months of slack still counts as "could plausibly become
//! the binding one". It is a stated parameter, not a fitted one.
//!
//! ### This is NOT the quantum K
//!
//! No `ħ`. A construction project is not an isolated quantum system and pretending
//! otherwise would be dressing an engineering number in physics it has not earned. The
//! quantum `K_eff` for a real qubit inside the finished building is a different quantity
//! that may live in the same system under its own name. The two are kept apart on purpose.
//!
//! ## The productive unit: coordination work
//!
//! Hash rate answers "how fast is this machine doing the thing that matters". The
//! analogue here is the rate at which a node **collapses** coordination stress:
//!
//! ```text
//! Ẇ = −dK/dt        K-units per hour — the "MH/s" of building a datacenter
//! ```
//!
//! And because `K` is computed through `max`, this unit is honest by construction:
//! **work on a branch that is not binding produces ΔK ≈ 0.** A node cannot farm it by
//! being busy. It has to move the thing that is actually in the way. That property is
//! what makes it worth paying attention to, and it is also why it must never be wired
//! straight to a payout without attestation — see [`KReceipt::gameable`].

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Reference period τ₀. The gauge is dimensionless; this is what "one unit of elapsed
/// time" means for the scale being measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scale {
    /// Live operations: τ₀ = 1 minute.
    Operations,
    /// Commissioning and fit-out: τ₀ = 1 day.
    Commissioning,
    /// Construction: τ₀ = 1 month.
    Construction,
}

impl Scale {
    pub fn tau0_hours(&self) -> f64 {
        match self {
            Scale::Operations => 1.0 / 60.0,
            Scale::Commissioning => 24.0,
            Scale::Construction => 730.0, // 1 month
        }
    }
}

/// Which branch of the schedule a constraint sits on. `max` is taken ACROSS branches;
/// within a branch, work is sequential and adds up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Branch {
    /// Site selection, permits, civil works — the branch robots can actually touch.
    SiteAndConstruction,
    /// The grid connection. Usually the one that decides the date, and the one nobody
    /// on site can speed up.
    Grid,
    /// Transformers, switchgear, servers — lead times, not labour.
    Procurement,
    /// Runs after the others, never in parallel with them.
    Commissioning,
}

/// One measurable constraint. `required` and `available` are in whatever unit suits it
/// (MW, %, racks); only their RATIO is used, so units never have to agree between rows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Constraint {
    pub name: String,
    pub branch: Branch,
    pub required: f64,
    pub available: f64,
    /// Months of work still remaining on this constraint.
    pub months_remaining: f64,
    /// How much this constraint is worth relative to the others on its branch. A cooling
    /// system half-built matters more than a fence half-built.
    pub weight: f64,
}

impl Constraint {
    pub fn new(name: &str, branch: Branch, required: f64, available: f64, months_remaining: f64) -> Self {
        Constraint {
            name: name.to_string(),
            branch,
            required,
            available,
            months_remaining,
            weight: 1.0,
        }
    }

    pub fn with_weight(mut self, w: f64) -> Self {
        self.weight = w;
        self
    }

    /// 0.0 = nothing available, 1.0 = fully satisfied. Clamped: an over-provisioned
    /// constraint is satisfied, not extra-satisfied, and must not offset a starved one.
    pub fn readiness(&self) -> f64 {
        if self.required <= 0.0 {
            return 1.0;
        }
        (self.available / self.required).clamp(0.0, 1.0)
    }

    pub fn deficit(&self) -> f64 {
        1.0 - self.readiness()
    }
}

/// The whole site, as a set of constraints.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SiteState {
    pub constraints: Vec<Constraint>,
}

impl SiteState {
    pub fn new(constraints: Vec<Constraint>) -> Self {
        SiteState { constraints }
    }

    /// Months remaining on each branch: sequential within a branch, so they sum.
    pub fn branch_months(&self) -> BTreeMap<Branch, f64> {
        let mut m = BTreeMap::new();
        for c in &self.constraints {
            *m.entry(c.branch).or_insert(0.0) += c.months_remaining;
        }
        m
    }

    /// The schedule model, and the prediction that comes out of it.
    ///
    /// `T = max(site+construction, grid, procurement) + commissioning`. The binding branch
    /// is the `argmax` — the one whose finish date IS the project's finish date.
    pub fn predict(&self) -> Prediction {
        let bm = self.branch_months();
        let commissioning = *bm.get(&Branch::Commissioning).unwrap_or(&0.0);
        let parallel: Vec<(Branch, f64)> = [Branch::SiteAndConstruction, Branch::Grid, Branch::Procurement]
            .iter()
            .map(|b| (*b, *bm.get(b).unwrap_or(&0.0)))
            .collect();
        let critical = parallel
            .iter()
            .cloned()
            .fold((Branch::SiteAndConstruction, f64::NEG_INFINITY), |a, b| if b.1 > a.1 { b } else { a });
        let months = critical.1.max(0.0) + commissioning;
        // Second place: how much room there is before the bottleneck CHANGES. A one-month
        // gap means shaving the critical path buys one month and then the next wall
        // arrives, which is the whole lesson of the robot experiment.
        let mut sorted: Vec<f64> = parallel.iter().map(|p| p.1).collect();
        sorted.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        let headroom = if sorted.len() >= 2 { (sorted[0] - sorted[1]).max(0.0) } else { 0.0 };
        Prediction {
            months_to_ready: months,
            binding_branch: critical.0,
            binding_months: critical.1.max(0.0),
            commissioning_months: commissioning,
            headroom_months: headroom,
            branch_months: parallel.into_iter().collect(),
        }
    }

    /// Slack per constraint: how many months this constraint's BRANCH is from being the
    /// one that decides the date. Zero slack = on the critical path.
    fn slacks(&self) -> Vec<f64> {
        let p = self.predict();
        let bm = self.branch_months();
        self.constraints
            .iter()
            .map(|c| {
                if c.branch == Branch::Commissioning {
                    // Commissioning is always on the critical path — it runs after
                    // everything and nothing overlaps it.
                    0.0
                } else {
                    (p.binding_months - bm.get(&c.branch).copied().unwrap_or(0.0)).max(0.0)
                }
            })
            .collect()
    }

    /// `pᵢ ∝ exp(−slackᵢ/θ) · weightᵢ · deficitᵢ`, normalised.
    ///
    /// Three factors, each earning its place: slack decides which branch can bind,
    /// weight says how much the constraint matters, and **deficit excludes what is already
    /// done** — a finished constraint cannot become the next bottleneck, and letting it
    /// carry probability mass would inflate the entropy with settled questions.
    pub fn bottleneck_weights(&self, theta_months: f64) -> Vec<f64> {
        let theta = if theta_months > 0.0 { theta_months } else { 1.0 };
        let raw: Vec<f64> = self
            .slacks()
            .iter()
            .zip(&self.constraints)
            .map(|(s, c)| (-s / theta).exp() * c.weight.max(0.0) * c.deficit())
            .collect();
        let total: f64 = raw.iter().sum();
        if total <= 0.0 {
            // Everything is done, or nothing can bind. A degenerate distribution, and the
            // honest answer is a flat one over nothing — entropy 0, handled below.
            return vec![0.0; raw.len()];
        }
        raw.into_iter().map(|r| r / total).collect()
    }

    /// The gauge. `elapsed_hours` is the observation window τ.
    pub fn gauge(&self, scale: Scale, elapsed_hours: f64, theta_months: f64) -> KGauge {
        let p = self.bottleneck_weights(theta_months);
        let dh: f64 = p.iter().zip(&self.constraints).map(|(pi, c)| pi * c.deficit()).sum();
        let ds: f64 = p.iter().filter(|x| **x > 0.0).map(|pi| -pi * pi.ln()).sum();
        let tau_ratio = if scale.tau0_hours() > 0.0 { elapsed_hours / scale.tau0_hours() } else { 0.0 };
        let k = 2.0 * std::f64::consts::PI * (dh * ds * tau_ratio).max(0.0).sqrt();
        let pred = self.predict();
        // The single most useful line: WHICH constraint is currently most likely to be
        // the next wall, by the same weights the entropy is computed from.
        let top = p
            .iter()
            .enumerate()
            .fold((0usize, f64::NEG_INFINITY), |a, (i, v)| if *v > a.1 { (i, *v) } else { a });
        KGauge {
            k,
            delta_h: dh,
            delta_s: ds,
            tau_ratio,
            scale,
            binding_constraint: self.constraints.get(top.0).map(|c| c.name.clone()).unwrap_or_default(),
            binding_weight: if top.1.is_finite() { top.1 } else { 0.0 },
            prediction: pred,
        }
    }
}

/// The prediction: when the building becomes real, and what is deciding that.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Prediction {
    pub months_to_ready: f64,
    pub binding_branch: Branch,
    pub binding_months: f64,
    pub commissioning_months: f64,
    /// Months the critical branch leads the runner-up by. Small headroom means shortening
    /// the critical path buys almost nothing before the next wall arrives.
    pub headroom_months: f64,
    pub branch_months: BTreeMap<Branch, f64>,
}

impl Prediction {
    /// What shortening the binding branch by `months` would ACTUALLY buy — capped by the
    /// runner-up, because past that point the bottleneck changes and the date stops moving.
    ///
    /// This is the robot experiment in one function: pass a huge number and the answer is
    /// still only `headroom_months`.
    pub fn gain_from_shortening_critical(&self, months: f64) -> f64 {
        months.max(0.0).min(self.headroom_months)
    }
}

/// The gauge reading at one moment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KGauge {
    pub k: f64,
    pub delta_h: f64,
    pub delta_s: f64,
    pub tau_ratio: f64,
    pub scale: Scale,
    /// The constraint most likely to be the next wall, by the derived weights.
    pub binding_constraint: String,
    pub binding_weight: f64,
    pub prediction: Prediction,
}

impl KGauge {
    /// One line a human can act on.
    pub fn headline(&self) -> String {
        format!(
            "K_DC {:.2} · ΔH {:.3} · Δs {:.3} · ready in {:.1} months · bottleneck: {} ({:.0}% of the weight) · shortening it buys at most {:.1} months",
            self.k,
            self.delta_h,
            self.delta_s,
            self.prediction.months_to_ready,
            self.binding_constraint,
            self.binding_weight * 100.0,
            self.prediction.headroom_months,
        )
    }
}

/// Coordination work: what a node produced between two readings.
///
/// ## The bug this type exists to not have
///
/// The first version defined work as `−ΔK` alone, and its own tests caught it inside a
/// day. Slip the grid connection from 30 months to 44 — an unambiguous disaster — and K
/// went DOWN, from 6.74 to 5.53. Nothing was wrong with the arithmetic. K carries the
/// entropy factor `Δs`, and a slip that makes ONE constraint obviously dominant *reduces
/// the confusion about why the project is stuck*. Less confusion, lower K. The gauge was
/// rewarding a catastrophe for being legible.
///
/// So the productive unit is anchored where it cannot be gamed by clarifying: **months
/// pulled off the completion date**.
///
/// ```text
/// Ẇ = Δmonths_to_ready / hours     the "MH/s" of building a datacenter
/// ```
///
/// That number comes through `max(...)`, so effort on a branch that is not binding moves
/// it by zero — a node cannot farm it by being busy — and a slip is unambiguously
/// negative. `ΔK` is kept beside it as what it actually measures: whether the REASON the
/// system is stuck got clearer or more fragmented. Two different questions, two fields,
/// neither pretending to be the other.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CoordinationWork {
    pub k_before: f64,
    pub k_after: f64,
    /// Change in the gauge. **Not the work done** — positive here can mean "the cause got
    /// clearer", including when it got clearer by getting worse. Read it with `delta_months`.
    pub delta_k: f64,
    pub months_before: f64,
    pub months_after: f64,
    /// Months pulled off the completion date. THIS is the work. Negative is a slip.
    pub delta_months: f64,
    pub hours: f64,
    /// Months per hour — the hash-rate analogue, and the only field fit to be paid on.
    pub rate: f64,
    /// Whether the bottleneck moved to a different constraint during the window. When it
    /// does, the *kind* of work that pays changes, which is the actionable event.
    pub bottleneck_moved: bool,
    pub bottleneck_before: String,
    pub bottleneck_after: String,
}

pub fn coordination_work(before: &KGauge, after: &KGauge, hours: f64) -> CoordinationWork {
    let dk = before.k - after.k;
    let mb = before.prediction.months_to_ready;
    let ma = after.prediction.months_to_ready;
    let dm = mb - ma;
    CoordinationWork {
        k_before: before.k,
        k_after: after.k,
        delta_k: dk,
        months_before: mb,
        months_after: ma,
        delta_months: dm,
        hours,
        rate: if hours > 0.0 { dm / hours } else { 0.0 },
        bottleneck_moved: before.binding_constraint != after.binding_constraint,
        bottleneck_before: before.binding_constraint.clone(),
        bottleneck_after: after.binding_constraint.clone(),
    }
}

/// A content-addressed record of one piece of coordination work.
///
/// Content-addressed so it can live in a decentralised store and be referred to by id
/// rather than by location — the same discipline the datacenter's job receipts already use.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KReceipt {
    pub version: u8,
    /// BLAKE3 of the canonical body. This is the id.
    pub id: String,
    pub job: String,
    pub work: CoordinationWork,
    pub gauge_after: KGauge,
    /// Energy actually spent, when the caller measured it. `None` is not zero.
    pub kwh: Option<f64>,
    pub at_unix_ms: u128,
    /// Always false, and there is no code path that sets it true. See [`Self::gameable`].
    pub settled_on_chain: bool,
}

impl KReceipt {
    pub fn new(job: &str, work: CoordinationWork, gauge_after: KGauge, kwh: Option<f64>) -> Self {
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let body = format!(
            "kdc/v1|{job}|{:.9}|{:.9}|{:.9}|{:.9}|{:.6}|{}|{at}",
            work.k_before, work.k_after, work.months_before, work.months_after,
            work.hours, gauge_after.binding_constraint
        );
        KReceipt {
            version: 1,
            id: blake3::hash(body.as_bytes()).to_hex().to_string(),
            job: job.to_string(),
            work,
            gauge_after,
            kwh,
            at_unix_ms: at,
            settled_on_chain: false,
        }
    }

    /// Why this is telemetry and not yet money, said in the type itself.
    ///
    /// ΔK is computed from state the reporting node also supplies. A node that wants a
    /// large ΔK can simply report a worse state first. Nothing here attests that the
    /// "before" reading was honest, so paying per ΔK would be paying for a number the
    /// claimant controls both ends of. Making it payable needs the before-state to be
    /// independently witnessed — which is a protocol, not a constant.
    pub const fn gameable(&self) -> &'static str {
        "ΔK is computed from state the reporting node supplies on both sides; without an \
         independent witness of the BEFORE reading this is telemetry, not a claim to payment"
    }
}

/// A decentralised, content-addressed store for K-receipts and for whatever else needs
/// durable space beside them — flux-moe's session memory in particular.
///
/// Deliberately generous: this is meant to be the campus's long memory, not a cache. Each
/// object is a file named by its BLAKE3 id under a namespace directory, so two writers can
/// never collide on a name and an identical object written twice costs nothing.
#[derive(Clone, Debug)]
pub struct KMemory {
    root: PathBuf,
    quota_bytes: u64,
}

/// Namespaces inside the store. Separate directories so a quota sweep can reason about
/// them independently, and so flux-moe's memory is never mixed with consensus telemetry.
pub const NS_RECEIPTS: &str = "receipts";
pub const NS_GAUGE: &str = "gauge";
/// Reserved for flux-moe: session memory, corpora, distilled artefacts.
pub const NS_MOE: &str = "moe";

impl KMemory {
    /// 64 GiB by default — the point of "masses of decentralised space" is that a node
    /// does not have to think about it. Override for a small box.
    pub fn open(root: impl AsRef<Path>) -> std::io::Result<Self> {
        Self::open_with_quota(root, 64 * 1024 * 1024 * 1024)
    }

    pub fn open_with_quota(root: impl AsRef<Path>, quota_bytes: u64) -> std::io::Result<Self> {
        let root = root.as_ref().to_path_buf();
        for ns in [NS_RECEIPTS, NS_GAUGE, NS_MOE] {
            std::fs::create_dir_all(root.join(ns))?;
        }
        Ok(KMemory { root, quota_bytes })
    }

    fn path(&self, ns: &str, id: &str) -> PathBuf {
        // Two-character fan-out: a flat directory with a million entries is a directory
        // no filesystem enjoys.
        let (a, b) = id.split_at(2.min(id.len()));
        self.root.join(ns).join(a).join(b)
    }

    /// Store bytes under their own BLAKE3 id. Returns the id. Idempotent.
    pub fn put(&self, ns: &str, bytes: &[u8]) -> std::io::Result<String> {
        let id = blake3::hash(bytes).to_hex().to_string();
        let p = self.path(ns, &id);
        if p.exists() {
            return Ok(id); // identical content, already here
        }
        if self.used_bytes() + bytes.len() as u64 > self.quota_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::StorageFull,
                format!("K-memory quota of {} bytes reached", self.quota_bytes),
            ));
        }
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Write-then-rename: a reader never sees a half-written object, and the id is a
        // promise about the content that a torn file would break.
        let tmp = p.with_extension("tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &p)?;
        Ok(id)
    }

    /// Store bytes under a caller-supplied id. Used where the object declares its own
    /// canonical id (a receipt); [`Self::put`] is the plain content-addressed path.
    pub fn put_at(&self, ns: &str, id: &str, bytes: &[u8]) -> std::io::Result<()> {
        let p = self.path(ns, id);
        if p.exists() {
            return Ok(());
        }
        if self.used_bytes() + bytes.len() as u64 > self.quota_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::StorageFull,
                format!("K-memory quota of {} bytes reached", self.quota_bytes),
            ));
        }
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = p.with_extension("tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &p)
    }

    pub fn get(&self, ns: &str, id: &str) -> std::io::Result<Vec<u8>> {
        std::fs::read(self.path(ns, id))
    }

    /// Verify a plain content-addressed object still hashes to its own name. Cheap
    /// integrity, no index needed. NOT valid for receipts — those are keyed by their own
    /// declared id; use [`Self::verify_receipt`].
    pub fn verify(&self, ns: &str, id: &str) -> bool {
        self.get(ns, id).map(|b| blake3::hash(&b).to_hex().to_string() == id).unwrap_or(false)
    }

    /// Store a receipt under the id IT declares, not under the hash of its JSON encoding.
    ///
    /// Those are different strings — the id is a hash of the canonical body, the JSON
    /// carries the id itself and so cannot hash to it. Keying by the JSON hash made the
    /// store's id disagree with the receipt's own, which is exactly the kind of quiet
    /// mismatch that turns "content-addressed" into a claim rather than a property.
    pub fn remember_receipt(&self, r: &KReceipt) -> std::io::Result<String> {
        let bytes = serde_json::to_vec(r).map_err(std::io::Error::other)?;
        self.put_at(NS_RECEIPTS, &r.id, &bytes)?;
        Ok(r.id.clone())
    }

    /// A receipt is intact when the body it declares still hashes to the id it is filed
    /// under — the same check, against the canonical body rather than the JSON.
    pub fn verify_receipt(&self, id: &str) -> bool {
        self.recall_receipt(id).map(|r| r.id == id).unwrap_or(false)
    }

    pub fn recall_receipt(&self, id: &str) -> std::io::Result<KReceipt> {
        let b = self.get(NS_RECEIPTS, id)?;
        serde_json::from_slice(&b).map_err(std::io::Error::other)
    }

    pub fn used_bytes(&self) -> u64 {
        fn walk(p: &Path) -> u64 {
            let Ok(rd) = std::fs::read_dir(p) else { return 0 };
            rd.filter_map(Result::ok)
                .map(|e| match e.file_type() {
                    Ok(t) if t.is_dir() => walk(&e.path()),
                    Ok(_) => e.metadata().map(|m| m.len()).unwrap_or(0),
                    _ => 0,
                })
                .sum()
        }
        walk(&self.root)
    }

    pub fn quota_bytes(&self) -> u64 {
        self.quota_bytes
    }

    /// Total coordination work remembered here — the node's lifetime output, the way a
    /// miner's accepted-share count is its lifetime output.
    pub fn total_work(&self) -> f64 {
        fn walk(p: &Path, acc: &mut f64) {
            let Ok(rd) = std::fs::read_dir(p) else { return };
            for e in rd.filter_map(Result::ok) {
                match e.file_type() {
                    Ok(t) if t.is_dir() => walk(&e.path(), acc),
                    Ok(_) => {
                        if let Ok(b) = std::fs::read(e.path()) {
                            if let Ok(r) = serde_json::from_slice::<KReceipt>(&b) {
                                *acc += r.work.delta_k;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut acc = 0.0;
        walk(&self.root.join(NS_RECEIPTS), &mut acc);
        acc
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The Flux organs: SAP, X-algo, cortex and P2P, wired to the gauge
// ─────────────────────────────────────────────────────────────────────────────
//
// K_DC says WHERE the system is stuck. These say WHO is trustworthy to unstick it, and
// carry the answer to the rest of the campus. They are the crate's existing dependencies
// (`flux_p2p::sap`, `flux_p2p::x_algo`) rather than a private re-implementation, so a
// builder's reputation here is the SAME number the mesh already uses for a peer.

use flux_p2p::sap::{SAPComponents, ScoreTable};
use flux_p2p::x_algo::{CrossScoreTable, PeerId as XPeerId};
use flux_p2p::sap::PeerId as SapPeerId;

/// One agent — a robot fleet, a contractor, a grid operator, another node — working a
/// constraint. Scored by SAP so that "who should take the next job" is a measurement.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Builder {
    pub id: String,
    /// The constraint this agent is working.
    pub constraint: String,
    /// Coordination work it has actually delivered, in K-units.
    pub delivered_k: f64,
    /// Work it committed to deliver over the same window.
    pub committed_k: f64,
    /// Round-trip responsiveness, seconds. Slow coordination IS a constraint.
    pub response_secs: f64,
    /// Windows it showed up for, out of windows it was asked.
    pub windows_present: u64,
    pub windows_total: u64,
    /// Times it reported progress that a later reading contradicted.
    pub contradictions: u64,
}

impl Builder {
    /// Map a builder onto the mesh's own SAP components, so one vocabulary covers a peer
    /// serving blocks and a robot pouring concrete.
    ///
    /// * contribution — delivered vs committed coordination work
    /// * latency      — how fast it answers, on the same decay the mesh uses
    /// * stake        — how much of the campus's remaining work it has taken on
    /// * accuracy     — 1.0 until it reports progress that does not survive the next reading
    /// * uptime       — windows present / windows asked
    pub fn sap_components(&self, campus_committed_k: f64) -> SAPComponents {
        let contribution = if self.committed_k > 0.0 {
            (self.delivered_k / self.committed_k).clamp(0.0, 1.0)
        } else {
            0.0
        };
        // Same shape as the mesh's latency score: full marks under 100 ms, zero past 1 s —
        // scaled here to coordination time, where the useful band is hours not milliseconds.
        let latency = (1.0 - (self.response_secs / 3600.0)).clamp(0.0, 1.0);
        let stake = if campus_committed_k > 0.0 {
            (self.committed_k / campus_committed_k).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let accuracy = 1.0 / (1.0 + self.contradictions as f64);
        let uptime = if self.windows_total > 0 {
            (self.windows_present as f64 / self.windows_total as f64).clamp(0.0, 1.0)
        } else {
            0.0
        };
        SAPComponents { contribution, latency, stake, accuracy, uptime }
    }
}

/// The campus's view of who is building what, scored with the mesh's own algorithms.
pub struct BuilderRegistry {
    pub sap: ScoreTable,
    pub cross: CrossScoreTable,
    builders: Vec<Builder>,
}

impl Default for BuilderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl BuilderRegistry {
    pub fn new() -> Self {
        BuilderRegistry { sap: ScoreTable::new(), cross: CrossScoreTable::new(), builders: Vec::new() }
    }

    /// Record a window: every builder's SAP components, then the X-algo cross-score, then
    /// the correlation between them. `round` is the observation window index.
    pub fn observe(&mut self, builders: Vec<Builder>, round: u64) {
        let committed: f64 = builders.iter().map(|b| b.committed_k).sum();
        for b in &builders {
            self.sap.update(SapPeerId(b.id.clone()), b.sap_components(committed));
            // A builder is "correct" for the round when it delivered what it promised;
            // tx_quality carries how much of the promise landed.
            let ratio = if b.committed_k > 0.0 { (b.delivered_k / b.committed_k).clamp(0.0, 1.0) } else { 0.0 };
            self.cross.record_round(XPeerId(b.id.clone()), round, ratio >= 0.9, ratio);
        }
        // Cross-score against the SAP table — the same correlation the mesh runs, so a
        // builder that scores well on one and badly on the other is visible as a
        // disagreement rather than averaged into the middle.
        self.cross.correlate_with_sap(&self.sap);
        self.builders = builders;
    }

    /// Topology rank fed to X-algo: a builder working the BINDING constraint sits at the
    /// centre of the campus's topology, because it is the one everything else waits on.
    /// This is the whole thesis expressed as a rank — position is decided by the gauge,
    /// not by who is loudest.
    pub fn rank_by_bottleneck(&mut self, gauge: &KGauge) {
        let ranks: std::collections::HashMap<XPeerId, f64> = self
            .builders
            .iter()
            .map(|b| {
                let r = if b.constraint == gauge.binding_constraint { 1.0 } else { 0.15 };
                (XPeerId(b.id.clone()), r)
            })
            .collect();
        self.cross.update_topology(&ranks);
    }

    /// Who should take the next job on the binding constraint: highest SAP score among
    /// builders already on it. `None` when nobody is working the thing that is in the way
    /// — which is itself the most important thing the campus can be told.
    pub fn best_for_bottleneck(&self, gauge: &KGauge) -> Option<(String, f64)> {
        self.builders
            .iter()
            .filter(|b| b.constraint == gauge.binding_constraint)
            .filter_map(|b| self.sap.get(&SapPeerId(b.id.clone())).map(|s| (b.id.clone(), s)))
            .fold(None, |acc: Option<(String, f64)>, x| match acc {
                Some(a) if a.1 >= x.1 => Some(a),
                _ => Some(x),
            })
    }

    pub fn builders(&self) -> &[Builder] {
        &self.builders
    }
}

/// What the cortex decides to do about a reading.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum CortexAction {
    /// Move effort onto the binding constraint, and say what it is worth.
    Reallocate { onto: String, worth_months: f64, best_builder: Option<String> },
    /// The bottleneck is one nobody on site can move — the grid, a lead time. Effort spent
    /// elsewhere buys nothing; say so instead of inventing work.
    WaitOn { constraint: String, months: f64 },
    /// Several constraints are near-tied: reduce the entropy before optimising anything,
    /// because with a fragmented cause you cannot know what to speed up.
    ResolveAmbiguity { candidates: Vec<String>, entropy: f64 },
    /// Nothing to coordinate.
    Ready,
}

/// The building cortex, applied to the gauge.
///
/// Deliberately small and rule-shaped rather than a learned policy: every branch below is
/// a consequence of `T = max(...)` that can be checked by hand, and a decision a human
/// cannot re-derive is a decision they cannot overrule.
pub fn cortex_decide(gauge: &KGauge, reg: &BuilderRegistry, high_entropy: f64) -> CortexAction {
    if gauge.k <= f64::EPSILON && gauge.prediction.months_to_ready <= 0.0 {
        return CortexAction::Ready;
    }
    if gauge.delta_s >= high_entropy {
        let mut candidates: Vec<String> =
            reg.builders().iter().map(|b| b.constraint.clone()).collect();
        candidates.sort();
        candidates.dedup();
        return CortexAction::ResolveAmbiguity { candidates, entropy: gauge.delta_s };
    }
    // Nobody on the binding constraint, or no headroom to gain: waiting is the honest
    // answer. This is the branch that stops the campus buying more robots for a problem
    // that is a queue at the utility.
    let best = reg.best_for_bottleneck(gauge);
    if gauge.prediction.headroom_months <= 0.0 || best.is_none() {
        return CortexAction::WaitOn {
            constraint: gauge.binding_constraint.clone(),
            months: gauge.prediction.binding_months,
        };
    }
    CortexAction::Reallocate {
        onto: gauge.binding_constraint.clone(),
        worth_months: gauge.prediction.headroom_months,
        best_builder: best.map(|b| b.0),
    }
}

/// The gossip topic the campus publishes readings on.
pub const TOPIC_KDC: &str = "/quillon/kdc/1/readings";

/// A reading as it goes over flux-p2p. Content-addressed by the receipt id it carries, so
/// a peer can ask for the detail without the announcement having to contain it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KAnnouncement {
    pub version: u8,
    pub node: String,
    pub k: f64,
    pub delta_h: f64,
    pub delta_s: f64,
    pub months_to_ready: f64,
    pub binding_constraint: String,
    /// The receipt id in the content-addressed store, for whoever wants the detail.
    pub receipt_id: Option<String>,
    pub at_unix_ms: u128,
}

impl KAnnouncement {
    pub fn from_gauge(node: &str, g: &KGauge, receipt_id: Option<String>) -> Self {
        KAnnouncement {
            version: 1,
            node: node.to_string(),
            k: g.k,
            delta_h: g.delta_h,
            delta_s: g.delta_s,
            months_to_ready: g.prediction.months_to_ready,
            binding_constraint: g.binding_constraint.clone(),
            receipt_id,
            at_unix_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0),
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// Decode a peer's announcement. Returns `None` rather than panicking on anything a
    /// hostile peer might send — this is untrusted input off the wire.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let a: KAnnouncement = serde_json::from_slice(bytes).ok()?;
        if a.version != 1 || !a.k.is_finite() || a.k < 0.0 || a.binding_constraint.len() > 128 {
            return None;
        }
        Some(a)
    }
}

/// The campus-wide view: every node's reading, reduced to one answer.
///
/// A single node sees its own site. The campus is only "ready" when the WORST reading says
/// so — taking the best, or the mean, would let one finished building hide an unpowered
/// one next door. `max` again, for the same reason it is in the schedule model.
pub fn campus_view(readings: &[KAnnouncement]) -> Option<KAnnouncement> {
    readings.iter().cloned().fold(None, |acc: Option<KAnnouncement>, r| match acc {
        Some(a) if a.months_to_ready >= r.months_to_ready => Some(a),
        _ => Some(r),
    })
}

/// The emergence boolean. Nothing is the datacenter; the relations are.
pub fn physical_datacenter_commissioned(readings: &[KAnnouncement]) -> bool {
    !readings.is_empty() && readings.iter().all(|r| r.months_to_ready <= 0.0 && r.k <= f64::EPSILON)
}

/// The whitepaper's likely 10 MW case, as a starting state. Every figure here is ASSUMED —
/// they are the paper's own industry-typical lead times, not measurements of any project.
pub fn whitepaper_10mw_likely() -> SiteState {
    SiteState::new(vec![
        Constraint::new("design + build", Branch::SiteAndConstruction, 1.0, 0.0, 32.0).with_weight(1.0),
        Constraint::new("grid connection", Branch::Grid, 10.0, 1.7, 30.0).with_weight(2.0),
        Constraint::new("transformers + switchgear", Branch::Procurement, 1.0, 0.0, 18.0).with_weight(1.5),
        Constraint::new("commissioning", Branch::Commissioning, 1.0, 0.0, 4.0).with_weight(1.0),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn the_prediction_reproduces_the_whitepapers_likely_case() {
        let p = whitepaper_10mw_likely().predict();
        // max(32, 30, 18) + 4 = 36
        assert!(approx(p.months_to_ready, 36.0, 1e-9), "{p:?}");
        assert_eq!(p.binding_branch, Branch::SiteAndConstruction);
        // Construction leads the grid by only two months — which is exactly why the robots
        // in the paper bought two months and no more.
        assert!(approx(p.headroom_months, 2.0, 1e-9), "{p:?}");
    }

    #[test]
    fn robots_buy_two_months_and_then_the_grid_decides() {
        let mut s = whitepaper_10mw_likely();
        // Science-fiction robots: civil work 32 → 16 months.
        s.constraints[0].months_remaining = 16.0;
        let p = s.predict();
        // 36 → 34, not 36 → 20. The grid is now the wall.
        assert!(approx(p.months_to_ready, 34.0, 1e-9), "{p:?}");
        assert_eq!(p.binding_branch, Branch::Grid);
        // And shortening the NEW critical path is worth only the new headroom (30 − 18).
        assert!(approx(p.headroom_months, 12.0, 1e-9), "{p:?}");
    }

    #[test]
    fn shortening_the_critical_path_is_capped_by_the_runner_up() {
        let p = whitepaper_10mw_likely().predict();
        // Ask for a year off construction; the runner-up caps the gain at two months.
        assert!(approx(p.gain_from_shortening_critical(12.0), 2.0, 1e-9));
        assert!(approx(p.gain_from_shortening_critical(0.5), 0.5, 1e-9));
    }

    #[test]
    fn removing_the_grid_queue_lets_the_robots_earn_their_keep() {
        // The paper's counterfactual: delete the grid wait and the same robots save real
        // time — until procurement becomes the next wall.
        let mut s = whitepaper_10mw_likely();
        s.constraints[1].months_remaining = 0.0; // no grid queue
        s.constraints[0].months_remaining = 16.0; // super-robots
        let p = s.predict();
        assert!(approx(p.months_to_ready, 22.0, 1e-9), "{p:?}"); // max(16,0,18)+4 = 22
        assert_eq!(p.binding_branch, Branch::Procurement, "procurement is the next wall");
    }

    #[test]
    fn entropy_is_low_when_one_constraint_obviously_binds() {
        // One constraint far out in front: "it is the transformer", and nothing else.
        let s = SiteState::new(vec![
            Constraint::new("grid", Branch::Grid, 10.0, 1.0, 40.0),
            Constraint::new("build", Branch::SiteAndConstruction, 1.0, 0.9, 2.0),
            Constraint::new("kit", Branch::Procurement, 1.0, 0.9, 1.0),
        ]);
        let g = s.gauge(Scale::Construction, 730.0, 6.0);
        assert!(g.delta_s < 0.35, "one clear bottleneck ⇒ low entropy, got {}", g.delta_s);
        assert_eq!(g.binding_constraint, "grid");
    }

    #[test]
    fn entropy_is_high_when_several_things_could_each_be_the_delay() {
        // Three branches finishing within a month of each other, all still incomplete.
        let s = SiteState::new(vec![
            Constraint::new("grid", Branch::Grid, 10.0, 5.0, 20.0),
            Constraint::new("build", Branch::SiteAndConstruction, 1.0, 0.5, 20.5),
            Constraint::new("kit", Branch::Procurement, 1.0, 0.5, 19.8),
        ]);
        let g = s.gauge(Scale::Construction, 730.0, 6.0);
        assert!(g.delta_s > 0.9, "a fragmented cause ⇒ high entropy, got {}", g.delta_s);
    }

    #[test]
    fn a_finished_constraint_cannot_be_the_next_bottleneck() {
        let s = SiteState::new(vec![
            // Done, and sitting on the critical path — it must still carry no weight.
            Constraint::new("grid", Branch::Grid, 10.0, 10.0, 40.0),
            Constraint::new("build", Branch::SiteAndConstruction, 1.0, 0.2, 10.0),
        ]);
        let w = s.bottleneck_weights(6.0);
        assert!(w[0] < 1e-12, "a satisfied constraint must not hold probability mass: {w:?}");
        assert!(w[1] > 0.99);
    }

    #[test]
    fn an_over_provisioned_constraint_does_not_offset_a_starved_one() {
        let s = SiteState::new(vec![
            Constraint::new("power", Branch::Grid, 10.0, 100.0, 5.0), // 10x over-provisioned
            Constraint::new("cooling", Branch::SiteAndConstruction, 10.0, 1.0, 5.0),
        ]);
        // Readiness clamps at 1.0, so the surplus cannot cancel the deficit.
        assert!(approx(s.constraints[0].readiness(), 1.0, 1e-12));
        let g = s.gauge(Scale::Construction, 730.0, 6.0);
        assert!(g.delta_h > 0.0, "a starved constraint must still show up: {g:?}");
        assert_eq!(g.binding_constraint, "cooling");
    }

    #[test]
    fn k_is_zero_when_the_site_is_finished() {
        let s = SiteState::new(vec![
            Constraint::new("grid", Branch::Grid, 10.0, 10.0, 0.0),
            Constraint::new("build", Branch::SiteAndConstruction, 1.0, 1.0, 0.0),
        ]);
        let g = s.gauge(Scale::Construction, 730.0, 6.0);
        assert!(approx(g.k, 0.0, 1e-12), "nothing left to coordinate ⇒ K = 0, got {}", g.k);
        assert!(approx(g.prediction.months_to_ready, 0.0, 1e-12));
    }

    #[test]
    fn work_on_a_non_binding_branch_produces_almost_no_coordination_work() {
        // THE property that makes this a unit worth having. Pour effort into a branch that
        // is not binding and the gauge barely moves — you cannot farm it by being busy.
        let before = whitepaper_10mw_likely();
        let g0 = before.gauge(Scale::Construction, 730.0, 6.0);

        let mut idle_branch = before.clone();
        idle_branch.constraints[2].months_remaining = 2.0; // procurement 18 → 2, far off critical
        idle_branch.constraints[2].available = 0.95;
        let g1 = idle_branch.gauge(Scale::Construction, 730.0, 6.0);
        let busy = coordination_work(&g0, &g1, 730.0);
        assert!(approx(busy.delta_months, 0.0, 1e-9), "a slack branch moves the date by ZERO: {busy:?}");

        let mut real_branch = before.clone();
        real_branch.constraints[0].months_remaining = 30.0; // 2 months off the CRITICAL path
        real_branch.constraints[0].available = 0.1;
        let g2 = real_branch.gauge(Scale::Construction, 730.0, 6.0);
        let real = coordination_work(&g0, &g2, 730.0);

        assert!(approx(real.delta_months, 2.0, 1e-9), "2 months off the critical path: {real:?}");
        assert!(
            real.rate > busy.rate,
            "moving the binding branch must beat polishing a slack one: real {} vs busy {} months/h",
            real.rate,
            busy.rate
        );
    }

    #[test]
    fn a_slipping_delivery_shows_as_negative_work_not_as_nothing() {
        let s = whitepaper_10mw_likely();
        let g0 = s.gauge(Scale::Construction, 730.0, 6.0);
        let mut worse = s.clone();
        worse.constraints[1].months_remaining = 44.0; // the grid slips by 14 months
        let g1 = worse.gauge(Scale::Construction, 730.0, 6.0);
        let w = coordination_work(&g0, &g1, 730.0);
        // A 14-month slip costs only TWELVE months on the date: construction sat 2 months
        // ahead of the grid, so the runner-up absorbed the first 2. `max` cuts both ways —
        // it caps what optimisation buys AND what a slip costs.
        assert!(approx(w.delta_months, -12.0, 1e-9), "a slip is negative work: {w:?}");
        assert!(w.rate < 0.0);
        // And the bug this whole type exists for: K went DOWN (ΔK positive) while the
        // project got 12 months later, because one dominant constraint is a CLEARER cause.
        // That is why ΔK is not the unit and must never be paid on.
        assert!(w.delta_k > 0.0, "a slip can still lower K — clarity is not progress: {w:?}");
        // The grid was already the heaviest constraint before the slip (weight 2.0), so
        // the bottleneck did not move — it just got worse. Two different events.
        assert!(!w.bottleneck_moved);
        assert_eq!(w.bottleneck_after, "grid connection");
    }

    #[test]
    fn receipts_are_content_addressed_and_never_claim_settlement() {
        let s = whitepaper_10mw_likely();
        let g0 = s.gauge(Scale::Construction, 730.0, 6.0);
        let mut s1 = s.clone();
        s1.constraints[0].months_remaining = 30.0;
        let g1 = s1.gauge(Scale::Construction, 730.0, 6.0);
        let r = KReceipt::new("civil-week-12", coordination_work(&g0, &g1, 168.0), g1, Some(31.2));
        assert_eq!(r.id.len(), 64);
        assert!(!r.settled_on_chain);
        assert!(r.gameable().contains("telemetry"));
    }

    #[test]
    fn memory_remembers_verifies_and_totals() {
        let dir = std::env::temp_dir().join(format!("kdc-mem-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let m = KMemory::open(&dir).expect("open");
        let s = whitepaper_10mw_likely();
        let g0 = s.gauge(Scale::Construction, 730.0, 6.0);
        let mut s1 = s.clone();
        s1.constraints[0].months_remaining = 30.0;
        let g1 = s1.gauge(Scale::Construction, 730.0, 6.0);
        let r = KReceipt::new("week-12", coordination_work(&g0, &g1, 168.0), g1, None);

        let id = m.remember_receipt(&r).expect("store");
        assert_eq!(id, r.id, "the id IS the content hash");
        assert!(m.verify_receipt(&id), "a stored receipt must still declare the id it is filed under");
        assert_eq!(m.recall_receipt(&id).expect("recall").job, "week-12");
        // Writing the identical receipt again is free and does not double-count.
        assert_eq!(m.remember_receipt(&r).expect("again"), id);
        assert!(approx(m.total_work(), r.work.delta_k, 1e-9));

        // flux-moe shares the same store under its own namespace.
        let moe = m.put(NS_MOE, b"a distilled session").expect("moe put");
        assert_eq!(m.get(NS_MOE, &moe).expect("moe get"), b"a distilled session");
        assert!(m.quota_bytes() >= 64 * 1024 * 1024 * 1024, "generous by default");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_quota_refuses_rather_than_filling_the_disk() {
        let dir = std::env::temp_dir().join(format!("kdc-quota-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let m = KMemory::open_with_quota(&dir, 64).expect("open");
        assert!(m.put(NS_MOE, &vec![b'x'; 32]).is_ok());
        assert!(m.put(NS_MOE, &vec![b'y'; 128]).is_err(), "past quota must fail loudly");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
