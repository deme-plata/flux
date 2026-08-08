//! Dependency-inversion hooks — lower layers emit, upper layers wire.
//!
//! `fluxc-core` (phase3 native compiles, etc.) wants to fire webhooks, but the
//! webhook machinery lives ABOVE it in `fluxc-webhooks` after the split. The
//! binary (or MCP server) wires the sink once at startup; until then dispatch
//! is a silent no-op — same observable behavior as an empty webhook config.

use std::sync::OnceLock;

pub type WebhookSink = fn(&str, serde_json::Value);

static SINK: OnceLock<WebhookSink> = OnceLock::new();

/// Wire the process-wide webhook sink (first caller wins; later calls no-op).
pub fn set_webhook_sink(sink: WebhookSink) {
    let _ = SINK.set(sink);
}

/// Fire a webhook event through the wired sink, if any.
pub fn dispatch_webhook(event: &str, data: serde_json::Value) {
    if let Some(sink) = SINK.get() {
        sink(event, data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static FIRED: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn unwired_is_noop_then_wired_fires() {
        dispatch_webhook("before-wiring", serde_json::json!({}));
        set_webhook_sink(|_, _| {
            FIRED.fetch_add(1, Ordering::SeqCst);
        });
        dispatch_webhook("after-wiring", serde_json::json!({"x": 1}));
        assert_eq!(FIRED.load(Ordering::SeqCst), 1);
    }
}

/// Build-prediction summary — the three numbers combo engines consume.
#[derive(Clone, Copy, Default, Debug)]
pub struct BuildPrediction {
    pub predicted_ms: u64,
    pub predicted_cache_rate: f64,
    pub confidence: f64,
}

pub type Predictor = fn(&str, bool) -> BuildPrediction;

static PREDICTOR: OnceLock<Predictor> = OnceLock::new();

/// Wire the process-wide build predictor (first caller wins).
pub fn set_predictor(p: Predictor) {
    let _ = PREDICTOR.set(p);
}

/// Predict a build; zeros when no predictor is wired (bin/MCP wire at boot).
pub fn predict_build(pkg: &str, release: bool) -> BuildPrediction {
    PREDICTOR.get().map(|p| p(pkg, release)).unwrap_or_default()
}
