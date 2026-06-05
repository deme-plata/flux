//! flux-render — real-time GPU render-spec + millisecond inspection of native Flux builds.
//!
//! The pipeline this crate owns:
//! ```text
//!   webhook event ──▶ Inspector.ingest ──▶ per-AREA ms state
//!                                              │
//!                          ┌───────────────────┼────────────────────┐
//!                          ▼                                          ▼
//!                  render_spec() -> RenderSpec            corrections() -> Vec<CorrectionRequest>
//!                  (tiles the GPU draws:                  (each errored area -> an AI
//!                   color=status, height=ms, heat)         error-correction MCP combo)
//! ```
//! "GPU rendering of native flux code in areas" = the [`RenderSpec`] of [`Tile`]s
//! (one per code area/region) the GPU frontend rasterizes. "ms inspection through
//! webhooks" = [`InspectEvent`]s (fed from a webhook) carry monotonic millisecond
//! timestamps. "error correction in AI parallel MCP combos" = [`corrections`]
//! turns every errored area into a [`CorrectionRequest`] naming the combo to run.
//!
//! Pure data + logic (no GPU/IO here) so it's deterministic and unit-tested; the
//! actual GPU draw + webhook transport + MCP dispatch are thin shells over this.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Compile/build status of one area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AreaStatus {
    Idle,
    Compiling,
    Ok,
    Error,
}

/// A code "area" — a region/crate/function the build touches. The unit the GPU
/// renders one tile for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Area {
    pub id: String,
    pub label: String,
    pub status: AreaStatus,
    /// Wall-clock ms the last (or in-flight) compile of this area has taken.
    pub last_ms: u64,
    /// Monotonic ms timestamp the current phase started (for in-flight duration).
    pub started_ms: u64,
    /// Latest error text, if status == Error.
    pub error: Option<String>,
    /// How many times this area has errored — drives the "heat".
    pub error_count: u32,
}

/// A webhook-delivered inspection event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectEvent {
    pub area: String,
    #[serde(default)]
    pub label: String,
    pub kind: EventKind,
    /// Monotonic millisecond timestamp from the build host.
    pub ms: u64,
    #[serde(default)]
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    Start,
    Ok,
    Error,
}

/// One render tile the GPU draws for an area.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tile {
    pub area: String,
    pub label: String,
    /// `#rrggbb` by status — green ok, amber compiling, red error, grey idle.
    pub color: String,
    /// Bar height ∝ last_ms (log-scaled, 0.0–1.0) — taller = slower.
    pub height: f32,
    /// Heat 0.0–1.0 ∝ error_count — redder/hotter the more it has failed.
    pub heat: f32,
}

/// The full scene the GPU frontend rasterizes each frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenderSpec {
    pub tiles: Vec<Tile>,
    pub total_ms: u64,
    pub error_count: usize,
    pub compiling: usize,
}

/// An AI error-correction request for one errored area — the parallel MCP combo
/// to run against it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorrectionRequest {
    pub area: String,
    pub error: String,
    /// The ordered MCP combo a corrector agent runs (verify-zero loop).
    pub combo: Vec<String>,
}

fn status_color(s: AreaStatus) -> &'static str {
    match s {
        AreaStatus::Idle => "#3a3350",
        AreaStatus::Compiling => "#f5c542",
        AreaStatus::Ok => "#a3be8c",
        AreaStatus::Error => "#e63946",
    }
}

/// log-scaled height: 0ms→0.0, ~16s+→1.0. Keeps fast areas readable next to slow.
fn ms_height(ms: u64) -> f32 {
    if ms == 0 {
        return 0.0;
    }
    let h = (ms as f32).ln() / (16_000f32).ln();
    h.clamp(0.0, 1.0)
}

/// Live inspection model. Feed it events; ask it for a render spec + corrections.
#[derive(Debug, Default, Clone)]
pub struct Inspector {
    areas: BTreeMap<String, Area>,
}

impl Inspector {
    pub fn new() -> Self {
        Self { areas: BTreeMap::new() }
    }

    /// Apply one webhook event. Idempotent per (area, ms) within a phase.
    pub fn ingest(&mut self, ev: &InspectEvent) {
        let area = self.areas.entry(ev.area.clone()).or_insert_with(|| Area {
            id: ev.area.clone(),
            label: if ev.label.is_empty() { ev.area.clone() } else { ev.label.clone() },
            status: AreaStatus::Idle,
            last_ms: 0,
            started_ms: 0,
            error: None,
            error_count: 0,
        });
        if !ev.label.is_empty() {
            area.label = ev.label.clone();
        }
        match ev.kind {
            EventKind::Start => {
                area.status = AreaStatus::Compiling;
                area.started_ms = ev.ms;
                area.error = None;
            }
            EventKind::Ok => {
                area.status = AreaStatus::Ok;
                area.last_ms = ev.ms.saturating_sub(area.started_ms);
            }
            EventKind::Error => {
                area.status = AreaStatus::Error;
                area.last_ms = ev.ms.saturating_sub(area.started_ms);
                area.error = Some(if ev.detail.is_empty() { "build error".into() } else { ev.detail.clone() });
                area.error_count += 1;
            }
        }
    }

    pub fn area(&self, id: &str) -> Option<&Area> {
        self.areas.get(id)
    }

    pub fn len(&self) -> usize {
        self.areas.len()
    }

    pub fn is_empty(&self) -> bool {
        self.areas.is_empty()
    }

    /// The scene the GPU draws — one tile per area.
    pub fn render_spec(&self) -> RenderSpec {
        let max_err = self.areas.values().map(|a| a.error_count).max().unwrap_or(0).max(1) as f32;
        let tiles: Vec<Tile> = self
            .areas
            .values()
            .map(|a| Tile {
                area: a.id.clone(),
                label: a.label.clone(),
                color: status_color(a.status).to_string(),
                height: ms_height(a.last_ms),
                heat: a.error_count as f32 / max_err,
            })
            .collect();
        RenderSpec {
            total_ms: self.areas.values().map(|a| a.last_ms).sum(),
            error_count: self.areas.values().filter(|a| a.status == AreaStatus::Error).count(),
            compiling: self.areas.values().filter(|a| a.status == AreaStatus::Compiling).count(),
            tiles,
        }
    }

    /// One correction request per currently-errored area — the parallel MCP combo
    /// a corrector agent runs. The combo escalates: compile+test, native fix,
    /// quantum-spec repair, then re-verify zero.
    pub fn corrections(&self) -> Vec<CorrectionRequest> {
        self.areas
            .values()
            .filter(|a| a.status == AreaStatus::Error)
            .map(|a| CorrectionRequest {
                area: a.id.clone(),
                error: a.error.clone().unwrap_or_default(),
                combo: vec![
                    "flux_combo".into(),
                    "flux_fix".into(),
                    "flux_qspec".into(),
                    "flux_combo".into(), // re-verify zero
                ],
            })
            .collect()
    }

    /// Render spec as JSON for the GPU frontend / a webhook payload.
    pub fn render_json(&self) -> String {
        serde_json::to_string(&self.render_spec()).unwrap_or_else(|_| "{}".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(area: &str, kind: EventKind, ms: u64) -> InspectEvent {
        InspectEvent { area: area.into(), label: String::new(), kind, ms, detail: String::new() }
    }

    #[test]
    fn start_then_ok_tracks_ms_and_colors_green() {
        let mut ins = Inspector::new();
        ins.ingest(&ev("fluxc-core", EventKind::Start, 1000));
        ins.ingest(&ev("fluxc-core", EventKind::Ok, 1850));
        let a = ins.area("fluxc-core").unwrap();
        assert_eq!(a.status, AreaStatus::Ok);
        assert_eq!(a.last_ms, 850);
        let spec = ins.render_spec();
        assert_eq!(spec.tiles.len(), 1);
        assert_eq!(spec.tiles[0].color, "#a3be8c"); // green
        assert_eq!(spec.error_count, 0);
        assert!(spec.tiles[0].height > 0.0 && spec.tiles[0].height < 1.0);
    }

    #[test]
    fn error_event_produces_red_tile_and_a_correction_combo() {
        let mut ins = Inspector::new();
        ins.ingest(&ev("flux-backend", EventKind::Start, 0));
        ins.ingest(&InspectEvent {
            area: "flux-backend".into(),
            label: "flux-backend".into(),
            kind: EventKind::Error,
            ms: 1200,
            detail: "E0599: no method `lower`".into(),
        });
        let spec = ins.render_spec();
        assert_eq!(spec.error_count, 1);
        assert_eq!(spec.tiles[0].color, "#e63946"); // red
        let cors = ins.corrections();
        assert_eq!(cors.len(), 1);
        assert_eq!(cors[0].area, "flux-backend");
        assert!(cors[0].error.contains("E0599"));
        // the parallel MCP combo: compile -> fix -> qspec -> re-verify
        assert_eq!(cors[0].combo, vec!["flux_combo", "flux_fix", "flux_qspec", "flux_combo"]);
    }

    #[test]
    fn heat_scales_with_repeated_failures() {
        let mut ins = Inspector::new();
        for area in ["a", "b"] {
            ins.ingest(&ev(area, EventKind::Start, 0));
            ins.ingest(&InspectEvent { area: area.into(), label: String::new(), kind: EventKind::Error, ms: 100, detail: "x".into() });
        }
        // fail "a" twice more → hotter than "b"
        for _ in 0..2 {
            ins.ingest(&ev("a", EventKind::Start, 0));
            ins.ingest(&InspectEvent { area: "a".into(), label: String::new(), kind: EventKind::Error, ms: 100, detail: "x".into() });
        }
        let spec = ins.render_spec();
        let heat = |id: &str| spec.tiles.iter().find(|t| t.area == id).unwrap().heat;
        assert_eq!(heat("a"), 1.0); // hottest
        assert!(heat("b") < heat("a"));
    }

    #[test]
    fn render_json_roundtrips() {
        let mut ins = Inspector::new();
        ins.ingest(&ev("x", EventKind::Start, 0));
        ins.ingest(&ev("x", EventKind::Ok, 500));
        let j = ins.render_json();
        let back: RenderSpec = serde_json::from_str(&j).unwrap();
        assert_eq!(back.tiles.len(), 1);
        assert_eq!(back.total_ms, 500);
    }
}
