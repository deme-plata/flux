//! flux-gpu-market — the rental-market engine for the flux GPU gateway: "vast.ai, but the API works".
//!
//! Pure logic the MCP/gateway calls into. Every rule here is a lesson paid for in real $ today:
//! - **Fit gate** — a box that can't hold the model scores 0 (the Blackwell/unsupported-GPU = $0 useful
//!   work lesson). Cheapest is worthless if it can't run the job.
//! - **Reliability² ÷ price** — wedged/dud hosts cost more than money; reliability is squared so a
//!   0.998 box beats a cheaper 0.97 one. (run #1 died on box wedges.)
//! - **Burn projection** — a $/hr reads as $/day · $/mo, so an orphaned box can't quietly drain the
//!   wallet ($8.98 → "6 hrs left" happened today).
//! - **Budget guard** — a hard ceiling on total live $/hr before a rental is allowed.
//! - **Idle autostop** — GPU at 0% past a grace window → reap it.
//! - **Box registry** — claim before create, release before destroy → teardowns never collide
//!   (the run-#1 teardown-collision that killed a sibling's box).
//!
//! No HTTP, no provider lock-in: the gateway feeds offers/telemetry in and acts on the decisions out.

use std::collections::BTreeMap;

/// What a workload needs from a box.
#[derive(Clone, Copy, Debug)]
pub struct Need {
    pub min_vram_gb: u32,
    pub min_disk_gb: u32,
    /// Minimum down-link so a big model pull isn't glacial. 0 = don't care.
    pub min_down_mbps: u32,
}

/// A provider offer (vast ask, etc.), normalized.
#[derive(Clone, Debug)]
pub struct Offer {
    pub id: u64,
    pub gpu: String,
    pub vram_gb: u32,
    pub disk_gb: u32,
    pub dph: f64,        // gateway $/hr (already +10% if you marked it up upstream)
    pub reliability: f64, // 0..1
    pub down_mbps: u32,
    pub verified: bool,
}

impl Offer {
    /// Does this box satisfy the need at all? If not, it's worth zero regardless of price.
    pub fn fits(&self, n: &Need) -> bool {
        self.vram_gb >= n.min_vram_gb && self.disk_gb >= n.min_disk_gb && self.down_mbps >= n.min_down_mbps
    }
    /// Rank score: reliability² × verified × link, per dollar. 0 if it doesn't fit.
    pub fn score(&self, n: &Need) -> f64 {
        if !self.fits(n) || self.dph <= 0.0 {
            return 0.0;
        }
        let link = if self.down_mbps >= 400 { 1.0 } else { 0.6 }; // slow link → long model pull
        let trust = if self.verified { 1.0 } else { 0.9 };
        // A rental that FAILS (prob 1-reliability) wastes the whole provisioning +
        // pipeline hour, not just its dph — so charge that disruption against the
        // EFFECTIVE cost. Dividing by raw dph let a cheap dud beat a reliable box
        // (0.90-rel $0.10 scored 8.1 vs 0.998-rel $0.16 at 6.2). Lived 2026-06-03:
        // every cheap unverified box that snapshot/CDI-failed cost ~an hour, exactly
        // the "cheap unreliable" the market must NOT recommend. Now reliability wins.
        const DISRUPTION_PER_FAIL: f64 = 1.0; // $-equiv of a wasted provisioning+pipeline hour
        let effective_cost = self.dph + (1.0 - self.reliability) * DISRUPTION_PER_FAIL;
        (self.reliability * self.reliability * link * trust) / effective_cost
    }
}

/// Best offers for a need, highest score first; unfit offers dropped.
pub fn rank<'a>(offers: &'a [Offer], need: &Need) -> Vec<&'a Offer> {
    let mut v: Vec<&Offer> = offers.iter().filter(|o| o.score(need) > 0.0).collect();
    v.sort_by(|a, b| b.score(need).partial_cmp(&a.score(need)).unwrap_or(std::cmp::Ordering::Equal));
    v
}

/// The real commitment of a $/hr rate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Burn {
    pub hour: f64,
    pub day: f64,
    pub month: f64,
}
pub fn burn(dph: f64) -> Burn {
    Burn { hour: dph, day: dph * 24.0, month: dph * 24.0 * 30.0 }
}

/// A hard ceiling on total live spend.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    pub ceiling_dph: f64,
}
impl Budget {
    /// May we add a `new_dph` rental on top of `current_total` without breaching the ceiling?
    pub fn can_add(&self, current_total_dph: f64, new_dph: f64) -> bool {
        current_total_dph + new_dph <= self.ceiling_dph + f64::EPSILON
    }
    /// Hours of runway left at the current burn given a wallet `balance`.
    pub fn hours_left(&self, balance: f64, current_total_dph: f64) -> f64 {
        if current_total_dph <= 0.0 { f64::INFINITY } else { balance / current_total_dph }
    }
}

/// Idle-autostop decision: reap a box whose GPU has been cold past a grace window.
/// `gpu_util_pct` now · `idle_min` minutes the GPU has been below 1% · `uptime_min` since boot.
pub fn should_autostop(gpu_util_pct: f64, idle_min: u32, grace_min: u32, idle_threshold_min: u32, uptime_min: u32) -> bool {
    uptime_min >= grace_min && gpu_util_pct < 1.0 && idle_min >= idle_threshold_min
}

/// Who owns which instance — claim before create, release before destroy, so two agents never tear
/// down the same box (the run-#1 collision).
#[derive(Default)]
pub struct Registry {
    owners: BTreeMap<u64, String>,
}
#[derive(Debug, PartialEq, Eq)]
pub enum RegErr {
    AlreadyOwned(String),
    NotOwner(String),
}
impl Registry {
    pub fn new() -> Self { Self::default() }

    /// Claim an instance. OK if unowned or already yours; rejects if another agent owns it.
    pub fn claim(&mut self, id: u64, owner: &str) -> Result<(), RegErr> {
        match self.owners.get(&id) {
            Some(o) if o != owner => Err(RegErr::AlreadyOwned(o.clone())),
            _ => { self.owners.insert(id, owner.to_string()); Ok(()) }
        }
    }
    /// May `who` destroy `id`? True if they own it or it's unclaimed; false if someone else owns it.
    pub fn can_destroy(&self, id: u64, who: &str) -> bool {
        self.owners.get(&id).map(|o| o == who).unwrap_or(true)
    }
    /// Release ownership (after a destroy). Only the owner may release.
    pub fn release(&mut self, id: u64, who: &str) -> Result<(), RegErr> {
        match self.owners.get(&id) {
            Some(o) if o != who => Err(RegErr::NotOwner(o.clone())),
            _ => { self.owners.remove(&id); Ok(()) }
        }
    }
    pub fn owner(&self, id: u64) -> Option<&String> { self.owners.get(&id) }
}

/// A propose-only GPU-rental recommendation — the agentic decision that composes EVERY market
/// lesson into ONE call: fit-gate → [`rank`] → budget-breach guard → [`burn`] → runway. The spend
/// discipline is baked into the TYPE: if the best fit breaches the budget ceiling, `offer_id` is
/// `None` — an agent literally cannot get a create-id for an over-budget box. `propose_only` is
/// always true: the operator/gateway triggers the actual `create_instance`; this never rents.
#[derive(Clone, Debug)]
pub struct Recommendation {
    /// The offer to propose — `None` if nothing fits the need OR the best fit breaches the budget.
    pub offer_id: Option<u64>,
    pub gpu: String,
    pub dph: f64,
    pub burn: Burn,
    pub score: f64,
    pub fits_budget: bool,
    pub hours_runway: f64,
    pub reason: String,
    /// Always true — the agent proposes, the operator confirms. (Never auto-rent.)
    pub propose_only: bool,
}

/// Recommend the best box for `need` given live `offers`, the `current_total_dph` already burning,
/// the `budget` ceiling, and the wallet `balance`. Propose-only by construction — over-budget or
/// no-fit yields `offer_id: None` so no agent can create an instance it shouldn't.
pub fn recommend(offers: &[Offer], need: &Need, current_total_dph: f64, budget: &Budget, balance: f64) -> Recommendation {
    match rank(offers, need).first().copied() {
        None => Recommendation {
            offer_id: None, gpu: String::new(), dph: 0.0, burn: burn(0.0), score: 0.0,
            fits_budget: false, hours_runway: budget.hours_left(balance, current_total_dph),
            reason: "no offer fits the need (vram/disk/down-link) — nothing worth renting".into(),
            propose_only: true,
        },
        Some(best) => {
            let dph = best.dph;
            let fits_budget = budget.can_add(current_total_dph, dph);
            let runway = budget.hours_left(balance, current_total_dph + dph);
            let score = best.score(need);
            if !fits_budget {
                return Recommendation {
                    offer_id: None, gpu: best.gpu.clone(), dph, burn: burn(dph), score,
                    fits_budget: false, hours_runway: runway, propose_only: true,
                    reason: format!(
                        "best fit {} (${:.3}/hr) would breach the budget ceiling ${:.3}/hr (current ${:.3}) — BLOCKED, no offer_id",
                        best.gpu, dph, budget.ceiling_dph, current_total_dph),
                };
            }
            Recommendation {
                offer_id: Some(best.id), gpu: best.gpu.clone(), dph, burn: burn(dph), score,
                fits_budget: true, hours_runway: runway, propose_only: true,
                reason: format!(
                    "propose {} · rel {:.3} · ${:.3}/hr · score {:.2} · fits budget · ~{:.1}h runway — OPERATOR confirms create_instance({})",
                    best.gpu, best.reliability, dph, score, runway, best.id),
            }
        }
    }
}

/// A propose-only multi-box FLEET plan — the agentic decision for "spin a test fabric": one capable
/// reliable LEAD (via [`recommend`]) + the cheapest fitting FOLLOWERS packed under the budget
/// ceiling. Maximizes box-count per remaining dollar without ever breaching the budget. Like
/// [`recommend`], it's propose-only and budget-safe by construction.
#[derive(Clone, Debug)]
pub struct FleetPlan {
    /// Lead box id — `None` if no affordable lead fits.
    pub lead: Option<u64>,
    /// Cheapest fitting follower ids, packed within the remaining budget (≤ `max_followers`).
    pub followers: Vec<u64>,
    pub total_dph: f64,
    pub burn: Burn,
    pub hours_runway: f64,
    pub reason: String,
    pub propose_only: bool,
}

/// Minimum reliability for a FOLLOWER — cheap-but-flaky followers waste the fabric run (the same
/// disruption lesson `score()` encodes, applied to the cheap tier).
pub const FLEET_FOLLOWER_MIN_RELIABILITY: f64 = 0.9;

/// Plan a cost-optimal fabric: best reliable LEAD for `lead_need`, then cheapest fitting FOLLOWERS
/// for `follower_need` greedily packed under `budget` (≤ `max_followers`). Propose-only; never
/// breaches the ceiling, never adds an unreliable follower, never auto-rents.
pub fn plan_fleet(offers: &[Offer], lead_need: &Need, follower_need: &Need, budget: &Budget, balance: f64, max_followers: usize) -> FleetPlan {
    let rec = recommend(offers, lead_need, 0.0, budget, balance);
    let lead_id = match rec.offer_id {
        Some(id) => id,
        None => return FleetPlan {
            lead: None, followers: Vec::new(), total_dph: 0.0, burn: burn(0.0),
            hours_runway: budget.hours_left(balance, 0.0), propose_only: true,
            reason: format!("no affordable lead — {}", rec.reason),
        },
    };
    let mut total = rec.dph;
    let mut followers: Vec<u64> = Vec::new();
    // cheapest-first fitting + reliable-enough followers (the lead excluded), greedy under budget
    let mut cands: Vec<&Offer> = offers.iter()
        .filter(|o| o.id != lead_id && o.fits(follower_need) && o.reliability >= FLEET_FOLLOWER_MIN_RELIABILITY)
        .collect();
    cands.sort_by(|a, b| a.dph.partial_cmp(&b.dph).unwrap_or(std::cmp::Ordering::Equal));
    for o in cands {
        if followers.len() >= max_followers { break; }
        if budget.can_add(total, o.dph) {
            total += o.dph;
            followers.push(o.id);
        }
    }
    let nf = followers.len();
    let runway = budget.hours_left(balance, total);
    FleetPlan {
        lead: Some(lead_id), followers, total_dph: total, burn: burn(total),
        hours_runway: runway, propose_only: true,
        reason: format!(
            "propose fabric: lead {} + {} follower(s) · ${:.3}/hr total · ~{:.1}h runway — OPERATOR confirms each create",
            lead_id, nf, total, runway),
    }
}

/// Live telemetry for one box in a RUNNING fleet — the runtime companion to a cold [`Offer`].
/// `is_lead` boxes coordinate the fabric and are never auto-folded (reaping the lead kills the run).
#[derive(Clone, Debug)]
pub struct FleetBox {
    pub id: u64,
    pub dph: f64,
    pub gpu_util_pct: f64,
    pub idle_min: u32,
    pub uptime_min: u32,
    pub is_lead: bool,
}

/// The runtime fold decision: which idle followers to reap + the burn/runway AFTER folding.
/// The whole point is `hours_runway_after > before` — trimming cold boxes buys the live ones time.
#[derive(Clone, Debug)]
pub struct FleetFold {
    /// Follower ids to autostop (cold past the grace/idle window) — never the lead.
    pub reap: Vec<u64>,
    /// Ids kept running (lead + busy followers + still-in-grace).
    pub keep: Vec<u64>,
    pub dph_before: f64,
    pub dph_after: f64,
    pub burn_after: Burn,
    pub hours_runway_after: f64,
    pub saved_dph: f64,
    pub reason: String,
    /// Always true — the agent proposes the reap list, the operator/gateway issues the stops.
    pub propose_only: bool,
}

/// Fold a RUNNING fleet down: reap every idle FOLLOWER past the grace/idle window so the surviving
/// burn buys more runway. This is the runtime sequel to [`plan_fleet`] — provisioning packs boxes
/// IN under a ceiling; folding takes the cold ones OUT to stretch the wallet. Composes
/// [`should_autostop`] (the reap rule, reused verbatim) + [`burn`] + [`Budget::hours_left`] (the
/// runway payoff). The lead is protected; folding never tears down the coordinator. Propose-only:
/// it returns the reap list and the operator/gateway issues the actual stops.
pub fn fold_fleet(boxes: &[FleetBox], grace_min: u32, idle_threshold_min: u32, budget: &Budget, balance: f64) -> FleetFold {
    let dph_before: f64 = boxes.iter().map(|b| b.dph).sum();
    let mut reap = Vec::new();
    let mut keep = Vec::new();
    let mut dph_after = 0.0;
    for b in boxes {
        // the lead is never auto-folded; a follower is reaped iff it's cold past the window
        let cold = !b.is_lead
            && should_autostop(b.gpu_util_pct, b.idle_min, grace_min, idle_threshold_min, b.uptime_min);
        if cold {
            reap.push(b.id);
        } else {
            keep.push(b.id);
            dph_after += b.dph;
        }
    }
    let saved = dph_before - dph_after;
    let runway_after = budget.hours_left(balance, dph_after);
    let reason = if reap.is_empty() {
        format!("nothing to fold — all {} box(es) busy or in grace; burn ${:.3}/hr unchanged", keep.len(), dph_after)
    } else {
        format!(
            "fold {} idle follower(s) → burn ${:.3}→${:.3}/hr (save ${:.3}/hr) · runway ~{:.1}h — OPERATOR confirms each stop",
            reap.len(), dph_before, dph_after, saved, runway_after)
    };
    FleetFold {
        reap, keep, dph_before, dph_after, burn_after: burn(dph_after),
        hours_runway_after: runway_after, saved_dph: saved, reason, propose_only: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn off(id: u64, vram: u32, dph: f64, rel: f64, down: u32, verified: bool) -> Offer {
        Offer { id, gpu: "x".into(), vram_gb: vram, disk_gb: 200, dph, reliability: rel, down_mbps: down, verified }
    }
    fn need70b() -> Need { Need { min_vram_gb: 48, min_disk_gb: 100, min_down_mbps: 400 } }

    #[test]
    fn unfit_box_scores_zero() {
        // a cheap 24GB box can't hold a 70b (needs 48GB) → worth nothing, no matter the price
        let cheap_small = off(1, 24, 0.10, 0.99, 800, true);
        assert_eq!(cheap_small.score(&need70b()), 0.0);
    }

    #[test]
    fn reliability_beats_cheap_unreliable() {
        let need = Need { min_vram_gb: 24, min_disk_gb: 100, min_down_mbps: 400 };
        let cheap_dud = off(1, 24, 0.10, 0.90, 800, true);
        let solid = off(2, 24, 0.16, 0.998, 800, true);
        // reliability² makes the slightly pricier 0.998 box win
        assert!(solid.score(&need) > cheap_dud.score(&need));
    }

    #[test]
    fn rank_drops_unfit_and_orders_by_score() {
        let need = need70b();
        let offers = vec![
            off(1, 24, 0.10, 0.99, 800, true),  // unfit (24<48) → dropped
            off(2, 80, 1.20, 0.97, 800, true),
            off(3, 80, 1.07, 0.994, 800, true), // best: cheaper + more reliable
        ];
        let ranked = rank(&offers, &need);
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].id, 3);
    }

    #[test]
    fn slow_link_is_penalized() {
        let need = Need { min_vram_gb: 24, min_disk_gb: 100, min_down_mbps: 0 };
        let fast = off(1, 24, 0.20, 0.99, 800, true);
        let slow = off(2, 24, 0.20, 0.99, 100, true);
        assert!(fast.score(&need) > slow.score(&need));
    }

    #[test]
    fn burn_projects_hour_day_month() {
        let b = burn(0.75);
        assert_eq!(b.day, 18.0);
        assert_eq!(b.month, 540.0);
    }

    #[test]
    fn budget_guard_blocks_overspend() {
        let bud = Budget { ceiling_dph: 2.0 };
        assert!(bud.can_add(1.0, 0.9));   // 1.9 ≤ 2.0
        assert!(!bud.can_add(1.5, 0.8));  // 2.3 > 2.0
        assert_eq!(bud.hours_left(9.0, 1.5), 6.0); // the $9 → 6hrs scenario
    }

    #[test]
    fn autostop_respects_grace_then_reaps_idle() {
        // still in grace window → keep (it may be pulling a model)
        assert!(!should_autostop(0.0, 20, 30, 10, 5));
        // past grace, GPU cold 12 min → reap
        assert!(should_autostop(0.0, 12, 30, 10, 40));
        // GPU busy → never reap
        assert!(!should_autostop(85.0, 99, 30, 10, 99));
    }

    #[test]
    fn registry_prevents_teardown_collision() {
        let mut r = Registry::new();
        r.claim(100, "infra").unwrap();
        // another agent can't claim or destroy it
        assert_eq!(r.claim(100, "rocky"), Err(RegErr::AlreadyOwned("infra".into())));
        assert!(!r.can_destroy(100, "rocky"));
        assert!(r.can_destroy(100, "infra"));
        // owner releases, then it's free
        r.release(100, "infra").unwrap();
        assert!(r.owner(100).is_none());
        assert!(r.can_destroy(100, "rocky")); // unclaimed → anyone
    }

    #[test]
    fn recommend_proposes_best_affordable_fit() {
        let offers = vec![off(1, 24, 0.10, 0.90, 800, true), off(2, 80, 0.16, 0.998, 800, true)];
        let r = recommend(&offers, &need70b(), 0.0, &Budget { ceiling_dph: 2.0 }, 10.0);
        assert_eq!(r.offer_id, Some(2), "the 80GB reliable box fits the 70b need + budget (24GB doesn't fit)");
        assert!(r.fits_budget && r.propose_only && r.hours_runway > 0.0);
    }

    #[test]
    fn recommend_blocks_budget_breach_with_no_offer_id() {
        let offers = vec![off(2, 80, 1.50, 0.998, 800, true)];
        let r = recommend(&offers, &need70b(), 0.0, &Budget { ceiling_dph: 1.0 }, 100.0);
        assert_eq!(r.offer_id, None, "over-budget → no create-id; the spend-gate is in the type");
        assert!(!r.fits_budget && r.propose_only);
        assert!(r.reason.contains("breach"));
    }

    #[test]
    fn recommend_none_when_nothing_fits() {
        let offers = vec![off(1, 24, 0.10, 0.99, 800, true)]; // 24GB can't hold a 70b (needs 48)
        let r = recommend(&offers, &need70b(), 0.0, &Budget { ceiling_dph: 5.0 }, 100.0);
        assert_eq!(r.offer_id, None);
        assert!(r.reason.contains("no offer fits"));
    }

    #[test]
    fn recommendation_is_always_propose_only() {
        let offers = vec![off(2, 80, 0.16, 0.998, 800, true)];
        let r = recommend(&offers, &need70b(), 0.0, &Budget { ceiling_dph: 5.0 }, 10.0);
        assert!(r.propose_only, "never auto-rent — agent proposes, operator confirms");
    }

    fn follower_need() -> Need { Need { min_vram_gb: 16, min_disk_gb: 50, min_down_mbps: 100 } }

    #[test]
    fn fleet_picks_lead_plus_cheapest_followers_in_budget() {
        let offers = vec![
            off(1, 80, 0.50, 0.998, 800, true), // lead (fits 70b)
            off(2, 24, 0.10, 0.95, 400, true),  // cheap follower
            off(3, 24, 0.08, 0.95, 400, true),  // cheaper follower
            off(4, 24, 0.30, 0.95, 400, true),  // pricier follower
        ];
        let p = plan_fleet(&offers, &need70b(), &follower_need(), &Budget { ceiling_dph: 1.0 }, 50.0, 5);
        assert_eq!(p.lead, Some(1));
        assert!(p.followers.contains(&3) && p.followers.contains(&2), "cheapest followers chosen first");
        assert!(p.total_dph <= 1.0 + f64::EPSILON, "never breaches the ceiling");
        assert!(p.propose_only && p.hours_runway > 0.0);
    }

    #[test]
    fn fleet_respects_follower_cap() {
        let offers = vec![
            off(1, 80, 0.20, 0.998, 800, true),
            off(2, 24, 0.05, 0.95, 400, true),
            off(3, 24, 0.05, 0.95, 400, true),
            off(4, 24, 0.05, 0.95, 400, true),
        ];
        let p = plan_fleet(&offers, &need70b(), &follower_need(), &Budget { ceiling_dph: 10.0 }, 50.0, 2);
        assert_eq!(p.followers.len(), 2, "capped at max_followers even with budget to spare");
    }

    #[test]
    fn fleet_excludes_unreliable_followers() {
        let offers = vec![
            off(1, 80, 0.20, 0.998, 800, true),
            off(2, 24, 0.01, 0.50, 400, true), // dirt cheap but flaky → excluded
        ];
        let p = plan_fleet(&offers, &need70b(), &follower_need(), &Budget { ceiling_dph: 10.0 }, 50.0, 5);
        assert!(p.followers.is_empty(), "a 0.50-reliability follower is never added");
    }

    #[test]
    fn fleet_none_when_no_affordable_lead() {
        let offers = vec![off(1, 80, 5.0, 0.998, 800, true)]; // lead too pricey for the ceiling
        let p = plan_fleet(&offers, &need70b(), &follower_need(), &Budget { ceiling_dph: 1.0 }, 50.0, 5);
        assert_eq!(p.lead, None);
        assert!(p.followers.is_empty() && p.propose_only);
    }

    fn fbox(id: u64, dph: f64, util: f64, idle: u32, uptime: u32, lead: bool) -> FleetBox {
        FleetBox { id, dph, gpu_util_pct: util, idle_min: idle, uptime_min: uptime, is_lead: lead }
    }

    #[test]
    fn fold_reaps_idle_followers_and_extends_runway() {
        let fleet = vec![
            fbox(1, 0.50, 90.0, 0, 120, true),   // lead, busy → keep
            fbox(2, 0.10, 0.0, 30, 120, false),  // follower cold 30min → reap
            fbox(3, 0.10, 75.0, 0, 120, false),  // follower busy → keep
        ];
        let f = fold_fleet(&fleet, 30, 10, &Budget { ceiling_dph: 2.0 }, 9.0);
        assert_eq!(f.reap, vec![2]);
        assert!(f.keep.contains(&1) && f.keep.contains(&3));
        assert!((f.dph_before - 0.70).abs() < 1e-9 && (f.dph_after - 0.60).abs() < 1e-9);
        assert!((f.saved_dph - 0.10).abs() < 1e-9);
        // runway after (9.0/0.60=15h) is longer than before (9.0/0.70≈12.86h) — that's the payoff
        assert!(f.hours_runway_after > 9.0 / f.dph_before);
        assert!(f.propose_only);
    }

    #[test]
    fn fold_never_reaps_the_lead_even_when_idle() {
        // lead is stone-cold past every window, but folding must never tear down the coordinator
        let fleet = vec![fbox(1, 0.50, 0.0, 99, 999, true)];
        let f = fold_fleet(&fleet, 30, 10, &Budget { ceiling_dph: 2.0 }, 9.0);
        assert!(f.reap.is_empty(), "the lead is protected from auto-fold");
        assert_eq!(f.keep, vec![1]);
    }

    #[test]
    fn fold_respects_grace_window() {
        // follower idle but still inside the grace window (uptime < grace) → may be pulling a model, keep
        let fleet = vec![
            fbox(1, 0.50, 90.0, 0, 120, true),
            fbox(2, 0.10, 0.0, 20, 5, false), // uptime 5 < grace 30 → not reaped yet
        ];
        let f = fold_fleet(&fleet, 30, 10, &Budget { ceiling_dph: 2.0 }, 9.0);
        assert!(f.reap.is_empty(), "in-grace follower is kept");
        assert!(f.reason.contains("nothing to fold"));
    }

    #[test]
    fn fold_nothing_when_all_busy_leaves_burn_unchanged() {
        let fleet = vec![
            fbox(1, 0.50, 90.0, 0, 120, true),
            fbox(2, 0.10, 60.0, 0, 120, false),
        ];
        let f = fold_fleet(&fleet, 30, 10, &Budget { ceiling_dph: 2.0 }, 9.0);
        assert!(f.reap.is_empty());
        assert!((f.dph_after - f.dph_before).abs() < 1e-9 && f.saved_dph.abs() < 1e-9);
    }
}
