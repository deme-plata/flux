//! fear_greed.rs — harness FEAR as a contrarian buy signal.
//!
//! Why people love horror: controlled fear in a safe frame is a thrill. Markets
//! run the same wiring — the dip *feels* like horror, but the crowd's terror is
//! the entry. We read the **Crypto Fear & Greed Index** (alternative.me, free, no
//! key: 0 = extreme fear → 100 = extreme greed) and turn it into a DCA multiplier:
//! buy HARDER when others are fearful, ease off when they're greedy. Carl-Runefelt
//! / Buffett: "be greedy when others are fearful." Propose-only — it sizes the
//! *suggested* buy, never auto-spends.

use serde::Deserialize;
use std::time::Duration;

/// The five canonical bands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sentiment {
    ExtremeFear,  // 0–24   → the horror dip; buy hardest
    Fear,         // 25–44
    Neutral,      // 45–55
    Greed,        // 56–74
    ExtremeGreed, // 75–100 → euphoria; ease off
}

impl Sentiment {
    pub fn from_value(v: u8) -> Sentiment {
        match v {
            0..=24 => Sentiment::ExtremeFear,
            25..=44 => Sentiment::Fear,
            45..=55 => Sentiment::Neutral,
            56..=74 => Sentiment::Greed,
            _ => Sentiment::ExtremeGreed,
        }
    }
    /// Contrarian DCA multiplier: scale the base DCA buy by sentiment.
    /// Extreme fear → 2.0× (back up the truck), extreme greed → 0.5× (sip).
    pub fn dca_multiplier(self) -> f64 {
        match self {
            Sentiment::ExtremeFear => 2.0,
            Sentiment::Fear => 1.5,
            Sentiment::Neutral => 1.0,
            Sentiment::Greed => 0.75,
            Sentiment::ExtremeGreed => 0.5,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Sentiment::ExtremeFear => "Extreme Fear",
            Sentiment::Fear => "Fear",
            Sentiment::Neutral => "Neutral",
            Sentiment::Greed => "Greed",
            Sentiment::ExtremeGreed => "Extreme Greed",
        }
    }
}

/// One reading of the index.
#[derive(Debug, Clone)]
pub struct FearGreed {
    pub value: u8,
    pub sentiment: Sentiment,
}
impl FearGreed {
    /// Contrarian sizing: base DCA × the sentiment multiplier.
    pub fn sized_dca(&self, base_usds: f64) -> f64 {
        base_usds * self.sentiment.dca_multiplier()
    }
}

#[derive(Deserialize)]
struct FngResp {
    data: Vec<FngEntry>,
}
#[derive(Deserialize)]
struct FngEntry {
    value: String, // the API returns the number as a string
}

/// Fetch the live Crypto Fear & Greed Index. No API key needed.
pub fn fetch() -> Result<FearGreed, String> {
    let r: FngResp = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent("flux-market/0.1")
        .build()
        .map_err(|e| e.to_string())?
        .get("https://api.alternative.me/fng/?limit=1")
        .send()
        .map_err(|e| format!("connect alternative.me: {e}"))?
        .error_for_status()
        .map_err(|e| format!("http: {e}"))?
        .json()
        .map_err(|e| format!("decode: {e}"))?;
    let v: u8 = r.data.first().ok_or("empty fng data")?.value.trim().parse().map_err(|_| "bad fng value")?;
    Ok(FearGreed { value: v, sentiment: Sentiment::from_value(v) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_classify_correctly() {
        assert_eq!(Sentiment::from_value(10), Sentiment::ExtremeFear);
        assert_eq!(Sentiment::from_value(50), Sentiment::Neutral);
        assert_eq!(Sentiment::from_value(90), Sentiment::ExtremeGreed);
    }

    #[test]
    fn fear_buys_harder_greed_eases() {
        // contrarian: more fear → bigger multiplier
        assert!(Sentiment::ExtremeFear.dca_multiplier() > Sentiment::Neutral.dca_multiplier());
        assert!(Sentiment::ExtremeGreed.dca_multiplier() < Sentiment::Neutral.dca_multiplier());
        let fear = FearGreed { value: 12, sentiment: Sentiment::ExtremeFear };
        assert_eq!(fear.sized_dca(100.0), 200.0); // back up the truck on the horror dip
        let greed = FearGreed { value: 88, sentiment: Sentiment::ExtremeGreed };
        assert_eq!(greed.sized_dca(100.0), 50.0); // sip at euphoria
    }
}
