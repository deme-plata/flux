// flux_tune — Skill Loadout System: equip different "builds" for your compiler
//
// Like equipping boots for speed or armor for protection in a game,
// flux_tune lets you redistribute scoring weights across SAP, X-Algo,
// and Q-Spec dimensions to optimize for different playstyles.
//
// Presets (game items):
//   ⚡ SPEED_BOOTS     — max iteration speed, lower safety (prototyping)
//   🛡️ TITAN_ARMOR     — max correctness, slower but bulletproof (production)
//   🔭 EXPLORER_LENS   — max diversity, explore unconventional fixes (innovation)
//   🎯 PRECISION_SCOPE — max accuracy, self-calibrating predictions (debugging)
//   ⚖️ BALANCED_BLADE   — even distribution (default)
//
// Each preset redistributes the 1.0 total weight across 5 dimensions.

use std::fs;
use std::path::PathBuf;

// ── Weight Configuration ──

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TuneWeights {
    /// Dimension 1 weight
    pub w1: f64,
    /// Dimension 2 weight
    pub w2: f64,
    /// Dimension 3 weight
    pub w3: f64,
    /// Dimension 4 weight
    pub w4: f64,
    /// Dimension 5 weight
    pub w5: f64,
}

impl TuneWeights {
    pub fn new(w1: f64, w2: f64, w3: f64, w4: f64, w5: f64) -> Self {
        let total = w1 + w2 + w3 + w4 + w5;
        if (total - 1.0).abs() > 0.01 {
            // Normalize
            TuneWeights { w1: w1/total, w2: w2/total, w3: w3/total, w4: w4/total, w5: w5/total }
        } else {
            TuneWeights { w1, w2, w3, w4, w5 }
        }
    }

    pub fn as_array(&self) -> [f64; 5] {
        [self.w1, self.w2, self.w3, self.w4, self.w5]
    }
}

impl Default for TuneWeights {
    fn default() -> Self {
        TuneWeights::new(0.35, 0.25, 0.20, 0.10, 0.10)
    }
}

// ── Presets ──

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TunePreset {
    pub name: &'static str,
    pub emoji: &'static str,
    pub description: &'static str,
    pub sap_weights: TuneWeights,    // Contribution, Latency, Stake, Accuracy, Uptime
    pub xalgo_weights: TuneWeights,  // SourceDelta, CacheAffinity, DepGraph, HistAccuracy, PeerConsensus
    pub qspec_weights: TuneWeights,  // CompileSuccess, TestPass, IntentFidelity, PerfDelta, Safety
    pub personality: &'static str,   // flavor text
}

pub fn presets() -> Vec<TunePreset> {
    vec![
    TunePreset {
        name: "SPEED_BOOTS",
        emoji: "⚡",
        description: "Maximum iteration speed. Source delta and compile success dominate. Lower safety, higher velocity. Ideal for prototyping and rapid experimentation.",
        sap_weights: TuneWeights::new(0.40, 0.30, 0.10, 0.10, 0.10),
        xalgo_weights: TuneWeights::new(0.50, 0.30, 0.10, 0.05, 0.05),
        qspec_weights: TuneWeights::new(0.45, 0.25, 0.15, 0.10, 0.05),
        personality: "You feel a surge of speed. Builds complete 40% faster. Safety is for the slow.",
    },
    TunePreset {
        name: "TITAN_ARMOR",
        emoji: "🛡️",
        description: "Bulletproof correctness. Safety score, test pass rate, and accuracy dominate. Slower builds but near-zero regression risk. Ideal for production releases.",
        sap_weights: TuneWeights::new(0.15, 0.15, 0.20, 0.30, 0.20),
        xalgo_weights: TuneWeights::new(0.10, 0.15, 0.15, 0.40, 0.20),
        qspec_weights: TuneWeights::new(0.20, 0.30, 0.10, 0.10, 0.30),
        personality: "Fortress mode engaged. Every change is triple-checked. Nothing gets past.",
    },
    TunePreset {
        name: "EXPLORER_LENS",
        emoji: "🔭",
        description: "Maximum diversity. Peer consensus and intent fidelity dominate. Favors unconventional fixes over safe ones. Ideal for AI-driven innovation and exploring paradigm shifts.",
        sap_weights: TuneWeights::new(0.15, 0.15, 0.15, 0.15, 0.40),
        xalgo_weights: TuneWeights::new(0.15, 0.15, 0.15, 0.15, 0.40),
        qspec_weights: TuneWeights::new(0.15, 0.15, 0.35, 0.15, 0.20),
        personality: "Your vision expands. You see paths others cannot. Unconventional solutions emerge.",
    },
    TunePreset {
        name: "PRECISION_SCOPE",
        emoji: "🎯",
        description: "Maximum prediction accuracy. Historical accuracy and test pass rate dominate. Self-calibrating predictions converge faster. Ideal for debugging and performance tuning.",
        sap_weights: TuneWeights::new(0.20, 0.20, 0.15, 0.35, 0.10),
        xalgo_weights: TuneWeights::new(0.15, 0.20, 0.10, 0.45, 0.10),
        qspec_weights: TuneWeights::new(0.25, 0.35, 0.15, 0.15, 0.10),
        personality: "Every millisecond accounted for. Predictions converge to truth. Debugging becomes surgical.",
    },
    TunePreset {
        name: "BALANCED_BLADE",
        emoji: "⚖️",
        description: "Even distribution across all dimensions. No trade-offs. The default loadout for general development.",
        sap_weights: TuneWeights::new(0.20, 0.20, 0.20, 0.20, 0.20),
        xalgo_weights: TuneWeights::new(0.20, 0.20, 0.20, 0.20, 0.20),
        qspec_weights: TuneWeights::new(0.20, 0.20, 0.20, 0.20, 0.20),
        personality: "Perfect balance. No weaknesses, no extremes. The blade finds its mark.",
    },
]
}

// ── Active Loadout ──

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ActiveTune {
    pub preset_name: String,
    pub sap: TuneWeights,
    pub xalgo: TuneWeights,
    pub qspec: TuneWeights,
    pub applied_at: u64,
}

impl Default for ActiveTune {
    fn default() -> Self {
        let preset = &&presets()[4]; // BALANCED_BLADE
        ActiveTune {
            preset_name: preset.name.to_string(),
            sap: preset.sap_weights.clone(),
            xalgo: preset.xalgo_weights.clone(),
            qspec: preset.qspec_weights.clone(),
            applied_at: now_secs(),
        }
    }
}

// ── Persistence ──

fn tune_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".flux").join("tune.json")
}

pub fn load_tune() -> ActiveTune {
    let path = tune_path();
    if path.exists() {
        fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        ActiveTune::default()
    }
}

pub fn save_tune(tune: &ActiveTune) -> Result<(), String> {
    let path = tune_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir: {}", e))?;
    }
    let json = serde_json::to_string_pretty(tune).map_err(|e| format!("json: {}", e))?;
    fs::write(&path, json).map_err(|e| format!("write: {}", e))
}

// ── Apply Preset ──

pub fn apply_preset(name: &str) -> Result<ActiveTune, String> {
    let all_presets = presets();
    let preset = all_presets.iter()
        .find(|p| p.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| format!("Unknown preset '{}'. Available: {}", name,
            all_presets.iter().map(|p| p.name).collect::<Vec<_>>().join(", ")))?;

    let tune = ActiveTune {
        preset_name: preset.name.to_string(),
        sap: preset.sap_weights.clone(),
        xalgo: preset.xalgo_weights.clone(),
        qspec: preset.qspec_weights.clone(),
        applied_at: now_secs(),
    };

    save_tune(&tune)?;
    Ok(tune)
}

pub fn apply_custom(sap: [f64; 5], xalgo: [f64; 5], qspec: [f64; 5]) -> Result<ActiveTune, String> {
    let tune = ActiveTune {
        preset_name: "CUSTOM".to_string(),
        sap: TuneWeights::new(sap[0], sap[1], sap[2], sap[3], sap[4]),
        xalgo: TuneWeights::new(xalgo[0], xalgo[1], xalgo[2], xalgo[3], xalgo[4]),
        qspec: TuneWeights::new(qspec[0], qspec[1], qspec[2], qspec[3], qspec[4]),
        applied_at: now_secs(),
    };
    save_tune(&tune)?;
    Ok(tune)
}

// ── Current SAP / X-Algo / Q-Spec Weights ──

pub fn current_sap_weights() -> [f64; 5] {
    load_tune().sap.as_array()
}

pub fn current_xalgo_weights() -> [f64; 5] {
    load_tune().xalgo.as_array()
}

pub fn current_qspec_weights() -> [f64; 5] {
    load_tune().qspec.as_array()
}

// ── Formatting ──

pub fn format_tune(tune: &ActiveTune) -> String {
    let all_presets = presets();
    let preset = all_presets.iter().find(|p| p.name == tune.preset_name);
    let emoji = preset.map(|p| p.emoji).unwrap_or("⚙️");
    let desc = preset.map(|p| p.description).unwrap_or("Custom weight configuration");
    let personality = preset.map(|p| p.personality).unwrap_or("");

    let mut lines = Vec::new();
    lines.push(format!("{} {} Loadout Active", emoji, tune.preset_name));
    lines.push(format!("   {}", desc));
    if !personality.is_empty() {
        lines.push(format!("   \"{}\"", personality));
    }
    lines.push(String::new());

    // SAP weights
    let sap_names = ["Contribution", "Latency", "Stake", "Accuracy", "Uptime"];
    lines.push("   📊 SAP Weights:".into());
    for (i, name) in sap_names.iter().enumerate() {
        let w = [tune.sap.w1, tune.sap.w2, tune.sap.w3, tune.sap.w4, tune.sap.w5][i];
        let bar = "█".repeat((w * 20.0) as usize);
        lines.push(format!("     {} {:>12}: {:.0}% {}", 
            if i == 0 { " " } else { " " },
            name, w * 100.0, bar));
    }

    // X-Algo weights
    let xalgo_names = ["Source Delta", "Cache Affinity", "Dep Graph", "Hist Accuracy", "Peer Consensus"];
    lines.push("   🔮 X-Algo Weights:".into());
    for (i, name) in xalgo_names.iter().enumerate() {
        let w = [tune.xalgo.w1, tune.xalgo.w2, tune.xalgo.w3, tune.xalgo.w4, tune.xalgo.w5][i];
        let bar = "█".repeat((w * 20.0) as usize);
        lines.push(format!("     {} {:>12}: {:.0}% {}", 
            if i == 0 { " " } else { " " },
            name, w * 100.0, bar));
    }

    // Q-Spec weights
    let qspec_names = ["Compile Success", "Test Pass Rate", "Intent Fidelity", "Perf Delta", "Safety Score"];
    lines.push("   ⚛️  Q-Spec Weights:".into());
    for (i, name) in qspec_names.iter().enumerate() {
        let w = [tune.qspec.w1, tune.qspec.w2, tune.qspec.w3, tune.qspec.w4, tune.qspec.w5][i];
        let bar = "█".repeat((w * 20.0) as usize);
        lines.push(format!("     {} {:>12}: {:.0}% {}", 
            if i == 0 { " " } else { " " },
            name, w * 100.0, bar));
    }

    // Speed estimate
    let speed_boost = estimate_speed_boost(tune);
    let safety_boost = estimate_safety_boost(tune);
    lines.push(String::new());
    lines.push(format!("   ⚡ Speed: +{:.0}%  |  🛡️ Safety: +{:.0}%  |  🔭 Innovation: +{:.0}%",
        speed_boost, safety_boost, 100.0 - speed_boost - safety_boost + 50.0));

    lines.join("\n")
}

pub fn format_presets() -> String {
    let mut lines = vec!["🎒 Available Loadouts:".to_string(), String::new()];
    for preset in &presets() {
        lines.push(format!("   {} {}", preset.emoji, preset.name));
        lines.push(format!("      {}", preset.description));
        lines.push(format!("      Effect: SPD +{:.0}% | SAF +{:.0}% | INV +{:.0}%",
            estimate_preset_speed(preset),
            estimate_preset_safety(preset),
            estimate_preset_innovation(preset)));
        lines.push(String::new());
    }
    lines.join("\n")
}

fn estimate_speed_boost(tune: &ActiveTune) -> f64 {
    let speed = tune.xalgo.w1 * 0.5  // source delta drives speed
        + tune.qspec.w1 * 0.3        // compile success = fast
        - tune.qspec.w5 * 0.2;       // safety slows down
    ((speed + 0.1) * 100.0).clamp(0.0, 80.0)
}

fn estimate_safety_boost(tune: &ActiveTune) -> f64 {
    let safety = tune.qspec.w5 * 0.5       // safety score
        + tune.qspec.w2 * 0.3              // test pass rate
        + tune.sap.w4 * 0.2;               // accuracy
    (safety * 100.0).clamp(0.0, 80.0)
}

fn estimate_preset_speed(p: &TunePreset) -> f64 {
    (p.xalgo_weights.w1 * 50.0 + p.qspec_weights.w1 * 30.0 - p.qspec_weights.w5 * 20.0 + 10.0).clamp(0.0, 80.0)
}

fn estimate_preset_safety(p: &TunePreset) -> f64 {
    (p.qspec_weights.w5 * 50.0 + p.qspec_weights.w2 * 30.0 + p.sap_weights.w4 * 20.0).clamp(0.0, 80.0)
}

fn estimate_preset_innovation(p: &TunePreset) -> f64 {
    (p.qspec_weights.w3 * 50.0 + p.xalgo_weights.w5 * 30.0 + p.sap_weights.w5 * 20.0).clamp(0.0, 80.0)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ── Auto-Equip: Context-Aware Preset Detection ──
//
// Scans an AI context string for keywords and returns the recommended preset.
// This lets the MCP layer auto-equip the right loadout without manual tuning.
//
// Detection rules:
//   🎯 prototyping keywords → SPEED_BOOTS   (fast iteration)
//   🛡️ production keywords  → TITAN_ARMOR   (bulletproof correctness)
//   🔭 research keywords    → EXPLORER_LENS (unconventional exploration)
//   🎯 debugging keywords   → PRECISION_SCOPE (surgical precision)
//   ⚖️ default              → BALANCED_BLADE

/// Score each keyword category against the context and return the best preset.
pub fn auto_detect_preset(context: &str) -> &'static str {
    let lower = context.to_lowercase();

    // Speed/prototyping keywords
    let speed_score = count_keywords(&lower, &[
        "prototype", "quick", "fast", "iterate", "experiment",
        "sketch", "draft", "rapid", "velocity", "speed", "hack",
    ]);

    // Safety/production keywords
    let safety_score = count_keywords(&lower, &[
        "production", "deploy", "release", "safe", "stable",
        "security", "audit", "verify", "bulletproof", "critical",
        "mission", "guarantee",
    ]);

    // Innovation/exploration keywords
    let explore_score = count_keywords(&lower, &[
        "explore", "innovate", "unconventional", "research", "discover",
        "novel", "paradigm", "frontier", "pioneer", "breakthrough",
        "vision", "quantum",
    ]);

    // Precision/debugging keywords
    let precision_score = count_keywords(&lower, &[
        "debug", "fix", "precise", "accurate", "profile",
        "optimize", "benchmark", "trace", "surgical", "pinpoint",
        "diagnose", "calibrate",
    ]);

    // Find the highest-scoring category
    let scores = [
        ("SPEED_BOOTS", speed_score),
        ("TITAN_ARMOR", safety_score),
        ("EXPLORER_LENS", explore_score),
        ("PRECISION_SCOPE", precision_score),
    ];

    let best = scores.iter()
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .unwrap();

    if best.1 > 0.0 {
        best.0
    } else {
        "BALANCED_BLADE"
    }
}

/// Count how many keywords from the list appear in the context.
fn count_keywords(context: &str, keywords: &[&str]) -> f64 {
    let mut score = 0.0_f64;
    for kw in keywords {
        if context.contains(kw) {
            score += 1.0;
            // Bonus for exact word boundary matches
            for word in context.split_whitespace() {
                if word.trim_matches(|c: char| !c.is_alphanumeric()) == *kw {
                    score += 0.5;
                }
            }
        }
    }
    score
}

/// Auto-apply the best preset based on context keywords.
/// Returns the applied tune and the detection reason.
pub fn auto_equip(context: &str) -> Result<(ActiveTune, String), String> {
    let detected = auto_detect_preset(context);
    let reason = format!(
        "Auto-detected '{}' from context keywords ({} chars analyzed)",
        detected, context.len()
    );
    let tune = apply_preset(detected)?;
    Ok((tune, reason))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_weights() {
        let w = TuneWeights::new(2.0, 2.0, 2.0, 2.0, 2.0);
        let arr = w.as_array();
        let sum: f64 = arr.iter().sum();
        assert!((sum - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_all_presets_sum_to_one() {
        for preset in &presets() {
            let sap_sum: f64 = preset.sap_weights.as_array().iter().sum();
            let xalgo_sum: f64 = preset.xalgo_weights.as_array().iter().sum();
            let qspec_sum: f64 = preset.qspec_weights.as_array().iter().sum();
            assert!((sap_sum - 1.0).abs() < 0.01, "SAP {} != 1.0 for {}", sap_sum, preset.name);
            assert!((xalgo_sum - 1.0).abs() < 0.01, "X-Algo {} != 1.0 for {}", xalgo_sum, preset.name);
            assert!((qspec_sum - 1.0).abs() < 0.01, "Q-Spec {} != 1.0 for {}", qspec_sum, preset.name);
        }
    }

    #[test]
    fn test_apply_preset() {
        let tune = apply_preset("SPEED_BOOTS").unwrap();
        assert_eq!(tune.preset_name, "SPEED_BOOTS");
        assert!(tune.xalgo.w1 > 0.3); // Source delta should be high
    }

    #[test]
    fn test_unknown_preset() {
        assert!(apply_preset("NONEXISTENT").is_err());
    }

    #[test]
    fn test_persistence_roundtrip() {
        // tune.json lives under $HOME/.flux — isolate HOME via the shared lock so a
        // parallel module's HOME swap can't make load_tune() read the wrong store.
        fluxc_util::test_home::with_temp_home("tune_roundtrip", || {
            let _tune = apply_preset("TITAN_ARMOR").unwrap();
            let loaded = load_tune();
            assert_eq!(loaded.preset_name, "TITAN_ARMOR");
        });
    }
}
