//! pricing.rs — the bootstrap business model (engine fusion).
//!
//! PHASE 1 (now): the engine is **Claude Code Max — $200/mo Viktor already pays**.
//! Charge the first wave of customers above that → instant margin, zero new infra,
//! zero GPU burn (the boxes that wouldn't boot taught us this).
//! PHASE 2 (later): when monthly LLM volume crosses the self-host break-even
//! (from [`crate::cost_model`]), onboard customers to their OWN served DeepSeek
//! agent — the flux-moe / deepseek-money stack. Reinvest revenue into compute.

/// The Phase-1 engine cost: the Claude Code Max subscription (flat, already paid).
pub const CLAUDE_CODE_MAX_MO: f64 = 200.0;

#[derive(Debug, Clone)]
pub struct Tier {
    pub name: &'static str,
    pub price_mo: f64,
    pub blurb: &'static str,
}

/// The first-wave price sheet (agentic-money + flux-coding service).
pub fn tiers() -> Vec<Tier> {
    vec![
        Tier { name: "Starter", price_mo: 49.0,  blurb: "agentic-money proposals + flux builds, propose-only, e-mail support" },
        Tier { name: "Pro",     price_mo: 199.0, blurb: "live-intel trade proposals, invoice gen, priority, 1 custom skill" },
        Tier { name: "Business",price_mo: 499.0, blurb: "dedicated agent, SIGIL+wallet combos, SLA, onboard-to-own-DeepSeek ready" },
    ]
}

/// Monthly profit given a customer mix (tier, count). Phase-1 cost is the flat
/// Claude Code Max plan — no per-customer LLM cost while we ride the subscription.
pub fn monthly_profit(mix: &[(&str, u32)]) -> f64 {
    let t = tiers();
    let revenue: f64 = mix.iter().map(|(name, n)| {
        t.iter().find(|x| x.name == *name).map(|x| x.price_mo * *n as f64).unwrap_or(0.0)
    }).sum();
    revenue - CLAUDE_CODE_MAX_MO
}

/// How many customers of a tier to cover the $200 engine (break the floor).
pub fn breakeven_customers(tier: &str) -> u32 {
    let p = tiers().into_iter().find(|x| x.name == tier).map(|x| x.price_mo).unwrap_or(f64::INFINITY);
    (CLAUDE_CODE_MAX_MO / p).ceil() as u32
}

/// Phase-2 trigger: flip a customer to their own self-hosted DeepSeek when their
/// monthly token volume exceeds the self-host break-even (cost_model: ~1066 Mtok
/// at a $1/hr box vs DeepSeek API). Below that, the shared Claude-Code engine wins.
pub fn should_onboard_own_deepseek(monthly_mtok: f64) -> bool {
    monthly_mtok > crate::cost_model::breakeven_mtok(1.0, 0.685)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_pro_customers_clear_the_engine() {
        // 2 × $199 = $398 - $200 engine = $198 profit
        assert!((monthly_profit(&[("Pro", 2)]) - 198.0).abs() < 1e-6);
    }

    #[test]
    fn breakeven_is_small() {
        assert_eq!(breakeven_customers("Starter"), 5);  // 5×49=245 ≥ 200
        assert_eq!(breakeven_customers("Pro"), 2);       // 2×199=398 ≥ 200
        assert_eq!(breakeven_customers("Business"), 1);  // 1×499 ≥ 200
    }

    #[test]
    fn phase2_only_for_heavy_users() {
        assert!(!should_onboard_own_deepseek(100.0));   // light → stay on Claude Code engine
        assert!(should_onboard_own_deepseek(2000.0));   // heavy → own DeepSeek pays off
    }

    #[test]
    fn a_modest_book_clears_profit() {
        // 4 Starter + 2 Pro + 1 Business = 196 + 398 + 499 = 1093 - 200 = 893/mo
        assert!((monthly_profit(&[("Starter",4),("Pro",2),("Business",1)]) - 893.0).abs() < 1e-6);
    }
}
