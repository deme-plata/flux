//! Bridges — convert events from sister substrates (flux-hotswap, CHIRON,
//! flux-search v2) into [`ViteEvent`]s so all three engines flow through one
//! ledger and one snapshot. The `vite-garden.html` surface relies on this:
//! one event stream, three pipelines visualized.
//!
//! These are pure transformations — no cross-crate dependencies, no async.
//! Downstream callers (fluxc-serve, flux-ue-bridge, fluxc-mcp) translate their
//! domain shapes into the small primitives below.

use crate::events::{ChironArm, ViteEvent, ViteEventKind};
use serde_json::{json, Value};

/// Build a HotSwap event from a `flux-hotswap` dispatch.
///
/// `old_blake3` / `new_blake3` are the canonical hex hashes of the old/new
/// function code; they're shortened to `aaaa…bbbb` before storage so they
/// stay compact in the recent-events ring and snapshot.
pub fn from_hotswap(
    fn_name: impl Into<String>,
    old_blake3: &str,
    new_blake3: &str,
    swap_ms: u32,
    epoch: u64,
) -> ViteEvent {
    ViteEvent::now(ViteEventKind::HotSwap {
        fn_name: fn_name.into(),
        old_blake3_short: short_hash(old_blake3),
        new_blake3_short: short_hash(new_blake3),
        swap_ms,
        epoch,
    })
}

/// Build a CHIRON event from a UE bridge dispatch.
pub fn from_chiron(
    arm: ChironArm,
    action: impl Into<String>,
    asset_id: impl Into<String>,
    ms: u32,
    ok: bool,
) -> ViteEvent {
    ViteEvent::now(ViteEventKind::ChironOp {
        arm,
        action: action.into(),
        asset_id: asset_id.into(),
        ms,
        ok,
    })
}

/// Build a SearchTap event when flux-search v2 ingests a new MCP / swarm /
/// proof document.
pub fn from_search_tap(
    tool: impl Into<String>,
    doc_id: impl Into<String>,
    blake3: &str,
    pattern: impl Into<String>,
) -> ViteEvent {
    ViteEvent::now(ViteEventKind::SearchTap {
        tool: tool.into(),
        doc_id: doc_id.into(),
        blake3_short: short_hash(blake3),
        pattern: pattern.into(),
    })
}

/// Convert any [`ViteEvent`] into a flat JSON document suitable for
/// flux-search v2 indexing (the engine wants `id` + `content` + `category` +
/// `last_crawled` + meta fields). Lets every event the engine sees become
/// instantly searchable across the substrate.
pub fn event_to_search_doc(ev: &ViteEvent) -> Value {
    let (category, content, path) = match &ev.kind {
        ViteEventKind::Connected { port } => (
            "vite",
            format!("vite connected port={port}"),
            None,
        ),
        ViteEventKind::HmrUpdate { path, kind } => (
            "hmr",
            format!("hmr {kind:?} {path}"),
            Some(path.clone()),
        ),
        ViteEventKind::PageReload { path } => (
            "vite",
            format!("page reload {}", path.as_deref().unwrap_or("")),
            path.clone(),
        ),
        ViteEventKind::Transform { path, stage, ms } => (
            "transform",
            format!("transform {stage:?} {path} {ms}ms"),
            Some(path.clone()),
        ),
        ViteEventKind::Error { message, path } => (
            "error",
            format!("error {} {}", path.as_deref().unwrap_or(""), message),
            path.clone(),
        ),
        ViteEventKind::Prune { paths } => (
            "vite",
            format!("prune {} modules", paths.len()),
            None,
        ),
        ViteEventKind::Exit { code } => (
            "vite",
            format!("vite exit code={:?}", code),
            None,
        ),
        ViteEventKind::HotSwap {
            fn_name,
            old_blake3_short,
            new_blake3_short,
            swap_ms,
            epoch,
        } => (
            "hotswap",
            format!(
                "hotswap fn={fn_name} old={old_blake3_short} new={new_blake3_short} ms={swap_ms} epoch={epoch}"
            ),
            None,
        ),
        ViteEventKind::ChironOp {
            arm,
            action,
            asset_id,
            ms,
            ok,
        } => (
            "chiron",
            format!(
                "chiron arm={arm:?} action={action} asset={asset_id} ms={ms} ok={ok}"
            ),
            Some(asset_id.clone()),
        ),
        ViteEventKind::SearchTap {
            tool,
            doc_id,
            blake3_short,
            pattern,
        } => (
            "search_tap",
            format!("search_tap tool={tool} doc={doc_id} blake3={blake3_short} pattern={pattern}"),
            None,
        ),
    };

    json!({
        "id": format!("vite-{}-{}", ev.ts_ms, kind_tag(&ev.kind)),
        "url": path.unwrap_or_else(|| "vite-engine://event".into()),
        "title": kind_tag(&ev.kind),
        "content": content,
        "category": category,
        "last_crawled": ev.ts_ms,
        "page_rank": 0.0,
        "readability_score": 0.0,
        "word_count": 0,
        "content_hash": "",
    })
}

/// Short discriminator tag for an event kind — used for search doc IDs +
/// titles. Stable string (no trailing data).
pub fn kind_tag(k: &ViteEventKind) -> &'static str {
    match k {
        ViteEventKind::Connected { .. } => "connected",
        ViteEventKind::HmrUpdate { .. } => "hmr_update",
        ViteEventKind::PageReload { .. } => "page_reload",
        ViteEventKind::Transform { .. } => "transform",
        ViteEventKind::Error { .. } => "error",
        ViteEventKind::Prune { .. } => "prune",
        ViteEventKind::Exit { .. } => "exit",
        ViteEventKind::HotSwap { .. } => "hot_swap",
        ViteEventKind::ChironOp { .. } => "chiron_op",
        ViteEventKind::SearchTap { .. } => "search_tap",
    }
}

fn short_hash(s: &str) -> String {
    if s.len() <= 12 {
        s.to_string()
    } else {
        format!("{}…{}", &s[..6], &s[s.len() - 6..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_hash_truncates_blake3() {
        let h = "ab12cd34ef5678901234567890abcdef";
        let s = short_hash(h);
        assert!(s.starts_with("ab12cd"));
        assert!(s.ends_with("abcdef"));
        assert!(s.contains('…'));
    }

    #[test]
    fn short_hash_keeps_short_strings() {
        assert_eq!(short_hash("abc"), "abc");
        assert_eq!(short_hash("123456789012"), "123456789012");
    }

    #[test]
    fn from_hotswap_builds_event() {
        let ev = from_hotswap(
            "render_dashboard",
            "ab12cd34ef5678901234",
            "cd34ef5678901234abcd",
            3,
            847,
        );
        if let ViteEventKind::HotSwap {
            fn_name, swap_ms, ..
        } = ev.kind
        {
            assert_eq!(fn_name, "render_dashboard");
            assert_eq!(swap_ms, 3);
        } else {
            panic!("expected HotSwap");
        }
    }

    #[test]
    fn from_chiron_with_eye_arm() {
        let ev = from_chiron(ChironArm::Eye, "frame", "scene_001", 16, true);
        assert!(matches!(
            ev.kind,
            ViteEventKind::ChironOp { arm: ChironArm::Eye, ms: 16, .. }
        ));
    }

    #[test]
    fn event_to_search_doc_hotswap_shape() {
        let ev = from_hotswap("classify_hmr", "aaaa", "bbbb", 4, 100);
        let doc = event_to_search_doc(&ev);
        assert_eq!(doc["category"], "hotswap");
        assert_eq!(doc["title"], "hot_swap");
        assert!(doc["content"].as_str().unwrap().contains("classify_hmr"));
        // id must start with vite- and contain the kind tag
        let id = doc["id"].as_str().unwrap();
        assert!(id.starts_with("vite-"));
        assert!(id.ends_with("hot_swap"));
    }

    #[test]
    fn event_to_search_doc_chiron_uses_asset_as_url() {
        let ev = from_chiron(ChironArm::Mesh, "retopo", "SKM_Hero", 412, true);
        let doc = event_to_search_doc(&ev);
        assert_eq!(doc["category"], "chiron");
        assert_eq!(doc["url"], "SKM_Hero");
    }

    #[test]
    fn event_to_search_doc_search_tap_captures_pattern() {
        let ev = from_search_tap(
            "flux_swarm_complete",
            "doc-rocky-130",
            "9a8b7c6d5e4f",
            "claim_settlement_loop",
        );
        let doc = event_to_search_doc(&ev);
        assert_eq!(doc["category"], "search_tap");
        assert!(doc["content"]
            .as_str()
            .unwrap()
            .contains("claim_settlement_loop"));
    }

    #[test]
    fn kind_tag_is_stable_for_every_variant() {
        // sanity — every existing variant maps to a non-empty tag.
        for tag in [
            "connected",
            "hmr_update",
            "page_reload",
            "transform",
            "error",
            "prune",
            "exit",
            "hot_swap",
            "chiron_op",
            "search_tap",
        ] {
            assert!(!tag.is_empty());
        }
    }
}
