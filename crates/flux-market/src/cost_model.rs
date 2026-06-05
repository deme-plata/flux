//! cost_model.rs — is self-hosting Qwen3.6 cheaper than the DeepSeek API?
//!
//! The honest answer is "it depends on UTILIZATION" — and this models the
//! crossover. Single-stream self-host is *more* expensive than an API; BATCHED
//! self-host (a busy GPU) is far cheaper. The flux-api gateway resells the Vast
//! box with a +10% red line and still beats DeepSeek at scale, because it
//! aggregates many users onto one batched GPU.

/// DeepSeek-V3 list pricing (USD per 1M tokens), conservative.
pub const DEEPSEEK_IN_PER_MTOK: f64 = 0.27;
pub const DEEPSEEK_OUT_PER_MTOK: f64 = 1.10;
/// Flux gateway red line over raw Vast cost.
pub const FLUX_MARGIN: f64 = 1.10;

/// DeepSeek API cost for a workload (USD).
pub fn deepseek_cost(in_mtok: f64, out_mtok: f64) -> f64 {
    in_mtok * DEEPSEEK_IN_PER_MTOK + out_mtok * DEEPSEEK_OUT_PER_MTOK
}

/// Self-hosted $/1M tokens = box $/hr ÷ (tok/s × 3600 / 1e6).
/// `tok_per_s` is AGGREGATE throughput (batched serving is the whole game:
/// single-stream ~40 tok/s; vLLM continuous-batching on an A100 ~1500-3000).
pub fn selfhost_per_mtok(box_per_hr: f64, tok_per_s: f64) -> f64 {
    let mtok_per_hr = tok_per_s * 3600.0 / 1e6;
    box_per_hr / mtok_per_hr
}

/// What a flux-api user pays per 1M tokens (self-host + gateway margin).
pub fn flux_gateway_per_mtok(box_per_hr: f64, tok_per_s: f64) -> f64 {
    selfhost_per_mtok(box_per_hr, tok_per_s) * FLUX_MARGIN
}

/// A 24/7 box's monthly cost (730 hr).
pub fn monthly_box_cost(box_per_hr: f64) -> f64 { box_per_hr * 730.0 }

/// Break-even monthly token volume (Mtok): above this, a 24/7 self-host box is
/// cheaper than paying the DeepSeek API at `blended_api_per_mtok`.
pub fn breakeven_mtok(box_per_hr: f64, blended_api_per_mtok: f64) -> f64 {
    monthly_box_cost(box_per_hr) / blended_api_per_mtok
}

/// The verdict for a given box + throughput + API blended price.
#[derive(Debug, Clone)]
pub struct CostVerdict {
    pub selfhost_per_mtok: f64,
    pub flux_user_per_mtok: f64,
    pub deepseek_blended_per_mtok: f64,
    pub flux_cheaper_per_token: bool,
    pub breakeven_mtok_per_month: f64,
}
pub fn verdict(box_per_hr: f64, tok_per_s: f64) -> CostVerdict {
    let blended = (DEEPSEEK_IN_PER_MTOK + DEEPSEEK_OUT_PER_MTOK) / 2.0; // ~0.685
    let flux = flux_gateway_per_mtok(box_per_hr, tok_per_s);
    CostVerdict {
        selfhost_per_mtok: selfhost_per_mtok(box_per_hr, tok_per_s),
        flux_user_per_mtok: flux,
        deepseek_blended_per_mtok: blended,
        flux_cheaper_per_token: flux < blended,
        breakeven_mtok_per_month: breakeven_mtok(box_per_hr, blended),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_stream_selfhost_loses_to_api() {
        // 40 tok/s on a $1/hr box → ~$6.94/Mtok, way above DeepSeek ~$0.69 blended
        let c = selfhost_per_mtok(1.0, 40.0);
        assert!(c > 5.0, "single-stream self-host should be expensive, got {c}");
    }

    #[test]
    fn batched_selfhost_beats_api() {
        // 2000 tok/s (vLLM batched) on $1/hr → ~$0.139/Mtok, beats DeepSeek
        let v = verdict(1.0, 2000.0);
        assert!(v.flux_cheaper_per_token, "batched self-host (+margin) should beat API: flux={} ds={}",
            v.flux_user_per_mtok, v.deepseek_blended_per_mtok);
        assert!(v.selfhost_per_mtok < 0.2);
    }

    #[test]
    fn breakeven_is_a_real_volume() {
        // $1/hr box = $730/mo; at blended $0.685/Mtok → ~1066 Mtok/mo to break even
        let be = breakeven_mtok(1.0, 0.685);
        assert!(be > 900.0 && be < 1200.0, "breakeven ~1066 Mtok/mo, got {be}");
    }

    #[test]
    fn flux_gateway_keeps_its_margin() {
        let s = selfhost_per_mtok(1.0, 2000.0);
        let f = flux_gateway_per_mtok(1.0, 2000.0);
        assert!((f / s - 1.10).abs() < 1e-9, "gateway should be +10%");
    }
}
