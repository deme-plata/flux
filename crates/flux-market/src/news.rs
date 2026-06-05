//! news.rs — Google News sentiment for the trading loop (the Runefelt signal).
//!
//! Fetches Google News RSS (free, read-only, no key) for a query, extracts the
//! headlines, and scores a NAIVE LEXICAL sentiment (bull-word vs bear-word
//! counts → −1..+1). Honest: this is a lexicon scorer, NOT an ML model — a fast,
//! transparent sentiment proxy for the arb/DCA loop, not a trading oracle.

use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Sentiment {
    /// −1 (max bearish) … +1 (max bullish).
    pub score: f64,
    pub label: String,
    pub headlines: usize,
    pub bull_hits: u32,
    pub bear_hits: u32,
    /// A few sample headlines (for the report).
    pub sample: Vec<String>,
}

const BULL: &[&str] = &[
    "surge", "rally", "soar", "bull", "ath", "record", "adopt", "gain", "jump", "high",
    "buy", "moon", "breakout", "inflow", "approve", "etf", "boom", "skyrocket", "outperform",
];
const BEAR: &[&str] = &[
    "crash", "plunge", "plummet", "fall", "bear", "sell", "drop", "fear", "ban", "hack",
    "dump", "outflow", "liquidat", "sue", "fraud", "selloff", "tumble", "warn", "risk", "slump",
];

/// PURE lexical scorer — unit-testable without a network.
pub fn score_headlines(headlines: &[String]) -> Sentiment {
    let (mut bull, mut bear) = (0u32, 0u32);
    for h in headlines {
        let l = h.to_lowercase();
        for w in BULL {
            if l.contains(w) {
                bull += 1;
            }
        }
        for w in BEAR {
            if l.contains(w) {
                bear += 1;
            }
        }
    }
    let total = (bull + bear).max(1) as f64;
    let score = (bull as f64 - bear as f64) / total;
    let label = if score > 0.2 { "Bullish 🟢" } else if score < -0.2 { "Bearish 🔴" } else { "Neutral ⚪" };
    Sentiment {
        score,
        label: label.into(),
        headlines: headlines.len(),
        bull_hits: bull,
        bear_hits: bear,
        sample: headlines.iter().take(3).cloned().collect(),
    }
}

/// Extract `<title>` contents from RSS XML (skips the feed-level title).
fn extract_titles(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(s) = rest.find("<title>") {
        let after = &rest[s + 7..];
        if let Some(e) = after.find("</title>") {
            let t = after[..e].replace("<![CDATA[", "").replace("]]>", "").trim().to_string();
            if !t.is_empty() {
                out.push(t);
            }
            rest = &after[e + 8..];
        } else {
            break;
        }
    }
    if !out.is_empty() {
        out.remove(0); // first <title> is the feed name
    }
    out
}

/// Fetch live Google News RSS for `query` and score sentiment.
pub fn fetch_news_sentiment(query: &str) -> Result<Sentiment, String> {
    let q: String = query.bytes().map(|b| if b == b' ' { "%20".to_string() } else { (b as char).to_string() }).collect();
    let url = format!("https://news.google.com/rss/search?q={q}&hl=en-US&gl=US&ceid=US:en");
    let xml = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent("flux-market/0.1 news")
        .build()
        .map_err(|e| e.to_string())?
        .get(&url)
        .send()
        .map_err(|e| format!("connect: {e}"))?
        .error_for_status()
        .map_err(|e| format!("http: {e}"))?
        .text()
        .map_err(|e| format!("body: {e}"))?;
    Ok(score_headlines(&extract_titles(&xml)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bullish_headlines_score_positive() {
        let h = vec![
            "Bitcoin surges to record ATH as ETF inflows soar".to_string(),
            "Major bank adopts BTC, analysts bullish".to_string(),
        ];
        let s = score_headlines(&h);
        assert!(s.score > 0.2, "score {}", s.score);
        assert!(s.label.contains("Bullish"));
    }

    #[test]
    fn bearish_headlines_score_negative() {
        let h = vec![
            "Bitcoin crashes as fear grips market, ETF outflows".to_string(),
            "Exchange hack sparks selloff, regulators warn".to_string(),
        ];
        let s = score_headlines(&h);
        assert!(s.score < -0.2, "score {}", s.score);
        assert!(s.label.contains("Bearish"));
    }

    #[test]
    fn extracts_titles_skipping_feed_title() {
        let xml = "<rss><channel><title>Feed Name</title><item><title>Headline One</title></item><item><title><![CDATA[Headline Two]]></title></item></channel></rss>";
        let t = extract_titles(xml);
        assert_eq!(t, vec!["Headline One".to_string(), "Headline Two".to_string()]);
    }
}
