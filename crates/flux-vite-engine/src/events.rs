//! Typed events emitted by the Vite engine.

use serde::{Deserialize, Serialize};

/// One event from the Vite stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViteEvent {
    /// Milliseconds since UNIX epoch.
    pub ts_ms: u64,
    pub kind: ViteEventKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ViteEventKind {
    /// Vite child process connected and is serving.
    Connected { port: u16 },
    /// An HMR update was sent for a file path.
    HmrUpdate { path: String, kind: HmrKind },
    /// A full page reload was triggered (HMR not possible).
    PageReload { path: Option<String> },
    /// Per-file transform timing observed.
    Transform { path: String, stage: TransformStage, ms: u32 },
    /// Vite reported a build/compile error.
    Error { message: String, path: Option<String> },
    /// Vite was about to prune unused modules.
    Prune { paths: Vec<String> },
    /// Vite child exited.
    Exit { code: Option<i32> },

    // ── v0.2 sister-engine variants — flow through the same event ledger ──

    /// fluxc serve hot-swapped a running function via AtomicPtr trampoline.
    /// Emitted by `flux-hotswap` integration in fluxc-serve.
    HotSwap {
        fn_name: String,
        old_blake3_short: String,
        new_blake3_short: String,
        swap_ms: u32,
        epoch: u64,
    },
    /// CHIRON (the three-armed asset surgeon) completed an operation.
    /// Emitted by the UE bridge when a Mesh/Rig/Anim arm finishes.
    ChironOp {
        arm: ChironArm,
        action: String,
        asset_id: String,
        ms: u32,
        ok: bool,
    },
    /// flux-search v2 indexed a substrate event (MCP tool call, swarm
    /// broadcast, settled task). Lets the vite-garden ribbon glow whenever
    /// the search substrate ingests something new.
    SearchTap {
        tool: String,
        doc_id: String,
        blake3_short: String,
        /// `release_cut` | `claim_settlement_loop` | `proof_emission` | ...
        pattern: String,
    },
}

/// Which CHIRON arm acted.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChironArm {
    Mesh,
    Rig,
    Anim,
    /// The chiron-eye camera observer (gold tier in the visualization).
    Eye,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HmrKind {
    /// JavaScript module update.
    Js,
    /// CSS-in-JS or .css update.
    Css,
    /// Static asset replaced (image, font).
    Asset,
    /// Generic (couldn't classify).
    Other,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransformStage {
    /// TypeScript type check pass.
    TsCheck,
    /// JSX to JS compilation.
    Jsx,
    /// SWC compilation.
    Swc,
    /// Asset emit/copy.
    Asset,
}

impl ViteEvent {
    pub fn now(kind: ViteEventKind) -> Self {
        Self { ts_ms: now_ms(), kind }
    }
}

pub(crate) fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Best-effort classifier from a file extension.
pub fn classify_hmr(path: &str) -> HmrKind {
    let lower = path.to_lowercase();
    if lower.ends_with(".css") || lower.ends_with(".scss") || lower.ends_with(".sass") || lower.ends_with(".less") {
        HmrKind::Css
    } else if lower.ends_with(".js")
        || lower.ends_with(".jsx")
        || lower.ends_with(".ts")
        || lower.ends_with(".tsx")
        || lower.ends_with(".vue")
        || lower.ends_with(".mjs")
    {
        HmrKind::Js
    } else if lower.ends_with(".png")
        || lower.ends_with(".svg")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".webp")
        || lower.ends_with(".woff2")
        || lower.ends_with(".woff")
        || lower.ends_with(".ttf")
    {
        HmrKind::Asset
    } else {
        HmrKind::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_basics() {
        assert_eq!(classify_hmr("src/App.tsx"), HmrKind::Js);
        assert_eq!(classify_hmr("src/index.ts"), HmrKind::Js);
        assert_eq!(classify_hmr("src/theme.css"), HmrKind::Css);
        assert_eq!(classify_hmr("public/logo.svg"), HmrKind::Asset);
        assert_eq!(classify_hmr("src/sw"), HmrKind::Other);
    }

    #[test]
    fn event_serializes_round_trip() {
        let ev = ViteEvent::now(ViteEventKind::HmrUpdate {
            path: "/src/App.tsx".into(),
            kind: HmrKind::Js,
        });
        let j = serde_json::to_string(&ev).unwrap();
        let back: ViteEvent = serde_json::from_str(&j).unwrap();
        assert!(matches!(back.kind, ViteEventKind::HmrUpdate { .. }));
    }

    #[test]
    fn hotswap_event_round_trip() {
        let ev = ViteEvent::now(ViteEventKind::HotSwap {
            fn_name: "render_dashboard".into(),
            old_blake3_short: "ab12…ef89".into(),
            new_blake3_short: "cd34…12ab".into(),
            swap_ms: 3,
            epoch: 847,
        });
        let j = serde_json::to_string(&ev).unwrap();
        assert!(j.contains("\"hot_swap\""));
        assert!(j.contains("render_dashboard"));
        let back: ViteEvent = serde_json::from_str(&j).unwrap();
        assert!(matches!(back.kind, ViteEventKind::HotSwap { swap_ms: 3, .. }));
    }

    #[test]
    fn chiron_event_round_trip() {
        let ev = ViteEvent::now(ViteEventKind::ChironOp {
            arm: ChironArm::Mesh,
            action: "retopo_lp".into(),
            asset_id: "SKM_Hero_01".into(),
            ms: 412,
            ok: true,
        });
        let j = serde_json::to_string(&ev).unwrap();
        assert!(j.contains("\"chiron_op\""));
        assert!(j.contains("\"mesh\""));
        let back: ViteEvent = serde_json::from_str(&j).unwrap();
        if let ViteEventKind::ChironOp { arm, ok, .. } = back.kind {
            assert_eq!(arm, ChironArm::Mesh);
            assert!(ok);
        } else {
            panic!("expected ChironOp");
        }
    }

    #[test]
    fn search_tap_event_round_trip() {
        let ev = ViteEvent::now(ViteEventKind::SearchTap {
            tool: "flux_swarm_complete".into(),
            doc_id: "doc-rocky-130".into(),
            blake3_short: "9a8b…c1d2".into(),
            pattern: "claim_settlement_loop".into(),
        });
        let j = serde_json::to_string(&ev).unwrap();
        assert!(j.contains("\"search_tap\""));
        assert!(j.contains("claim_settlement_loop"));
        let back: ViteEvent = serde_json::from_str(&j).unwrap();
        assert!(matches!(back.kind, ViteEventKind::SearchTap { .. }));
    }

    #[test]
    fn chiron_arm_eye_serializes_snake_case() {
        let j = serde_json::to_string(&ChironArm::Eye).unwrap();
        assert_eq!(j, "\"eye\"");
    }
}
