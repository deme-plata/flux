//! In-memory state container — accumulates events into a snapshot suitable
//! for the vite-garden surface.

use crate::events::{ChironArm, HmrKind, TransformStage, ViteEvent, ViteEventKind};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

const HMR_WINDOW_SECS: u64 = 60;
const MAX_RECENT_EVENTS: usize = 128;
const TRANSFORM_SAMPLES: usize = 64;

/// Accumulating state — updated by `apply()` on each event.
#[derive(Debug, Clone, Default)]
pub struct ViteState {
    pub port: Option<u16>,
    pub last_event_ms: u64,
    pub hmr_total: u64,
    pub page_reload_total: u64,
    pub error_total: u64,
    pub exit_code: Option<i32>,
    // ── v0.2 sister-engine counters ──
    pub hot_swap_total: u64,
    pub hot_swap_ok: u64,
    pub last_hot_swap_ms: Option<u64>,
    pub chiron_op_total: u64,
    pub chiron_op_ok: u64,
    pub search_tap_total: u64,
    // Recent HMR timestamps (capped) used to compute rolling rate.
    hmr_timestamps: VecDeque<u64>,
    // Per-path HMR counts.
    hmr_by_path: HashMap<String, u64>,
    // Recent transform-time samples (capped).
    transform_samples: VecDeque<u32>,
    // Recent hot-swap timings (capped).
    hot_swap_ms_samples: VecDeque<u32>,
    // Per-arm CHIRON counts.
    chiron_by_arm: HashMap<String, u64>,
    // Per-pattern flux-search tap counts.
    search_taps_by_pattern: HashMap<String, u64>,
    // Recent events for the UI ribbon (capped).
    recent: VecDeque<ViteEvent>,
}

impl ViteState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply(&mut self, ev: &ViteEvent) {
        self.last_event_ms = ev.ts_ms;
        if self.recent.len() >= MAX_RECENT_EVENTS {
            self.recent.pop_front();
        }
        self.recent.push_back(ev.clone());

        match &ev.kind {
            ViteEventKind::Connected { port } => {
                self.port = Some(*port);
            }
            ViteEventKind::HmrUpdate { path, .. } => {
                self.hmr_total += 1;
                self.hmr_timestamps.push_back(ev.ts_ms);
                self.trim_hmr_window(ev.ts_ms);
                *self.hmr_by_path.entry(path.clone()).or_default() += 1;
            }
            ViteEventKind::PageReload { .. } => {
                self.page_reload_total += 1;
            }
            ViteEventKind::Transform { ms, .. } => {
                if self.transform_samples.len() >= TRANSFORM_SAMPLES {
                    self.transform_samples.pop_front();
                }
                self.transform_samples.push_back(*ms);
            }
            ViteEventKind::Error { .. } => {
                self.error_total += 1;
            }
            ViteEventKind::Prune { .. } => {}
            ViteEventKind::Exit { code } => {
                self.exit_code = *code;
            }
            ViteEventKind::HotSwap { swap_ms, .. } => {
                self.hot_swap_total += 1;
                self.hot_swap_ok += 1;
                if self.hot_swap_ms_samples.len() >= TRANSFORM_SAMPLES {
                    self.hot_swap_ms_samples.pop_front();
                }
                self.hot_swap_ms_samples.push_back(*swap_ms);
                self.last_hot_swap_ms = Some(ev.ts_ms);
            }
            ViteEventKind::ChironOp { arm, ok, .. } => {
                self.chiron_op_total += 1;
                if *ok {
                    self.chiron_op_ok += 1;
                }
                let key = match arm {
                    ChironArm::Mesh => "mesh",
                    ChironArm::Rig => "rig",
                    ChironArm::Anim => "anim",
                    ChironArm::Eye => "eye",
                };
                *self.chiron_by_arm.entry(key.to_string()).or_default() += 1;
            }
            ViteEventKind::SearchTap { pattern, .. } => {
                self.search_tap_total += 1;
                *self.search_taps_by_pattern.entry(pattern.clone()).or_default() += 1;
            }
        }
    }

    fn trim_hmr_window(&mut self, now_ms: u64) {
        let cutoff = now_ms.saturating_sub(HMR_WINDOW_SECS * 1000);
        while let Some(&front) = self.hmr_timestamps.front() {
            if front < cutoff {
                self.hmr_timestamps.pop_front();
            } else {
                break;
            }
        }
    }

    pub fn snapshot(&self) -> ViteSnapshot {
        ViteSnapshot {
            port: self.port,
            last_event_ms: self.last_event_ms,
            hmr_total: self.hmr_total,
            page_reload_total: self.page_reload_total,
            error_total: self.error_total,
            exit_code: self.exit_code,
            hmr_rate_60s: self.hmr_rate_60s(),
            transform_p50_ms: self.transform_percentile(0.5),
            transform_p99_ms: self.transform_percentile(0.99),
            top_paths: self.top_hmr_paths(8),
            recent_kinds: self
                .recent
                .iter()
                .rev()
                .take(60)
                .filter_map(|e| match &e.kind {
                    ViteEventKind::HmrUpdate { kind, .. } => Some(*kind),
                    _ => None,
                })
                .collect(),
            hot_swap_total: self.hot_swap_total,
            hot_swap_ok: self.hot_swap_ok,
            hot_swap_p50_ms: self.hot_swap_percentile(0.5),
            hot_swap_p99_ms: self.hot_swap_percentile(0.99),
            last_hot_swap_ms: self.last_hot_swap_ms,
            chiron_op_total: self.chiron_op_total,
            chiron_op_ok: self.chiron_op_ok,
            chiron_by_arm: top_n_map(&self.chiron_by_arm, 4),
            search_tap_total: self.search_tap_total,
            search_taps_top_patterns: top_n_map(&self.search_taps_by_pattern, 6),
            score: self.score(),
        }
    }

    fn hot_swap_percentile(&self, p: f32) -> Option<u32> {
        if self.hot_swap_ms_samples.is_empty() {
            return None;
        }
        let mut v: Vec<u32> = self.hot_swap_ms_samples.iter().copied().collect();
        v.sort_unstable();
        let idx = ((v.len() as f32 - 1.0) * p).round() as usize;
        Some(v[idx])
    }

    fn hmr_rate_60s(&self) -> f32 {
        self.hmr_timestamps.len() as f32 / HMR_WINDOW_SECS as f32
    }

    fn transform_percentile(&self, p: f32) -> Option<u32> {
        if self.transform_samples.is_empty() {
            return None;
        }
        let mut v: Vec<u32> = self.transform_samples.iter().copied().collect();
        v.sort_unstable();
        let idx = ((v.len() as f32 - 1.0) * p).round() as usize;
        Some(v[idx])
    }

    fn top_hmr_paths(&self, n: usize) -> Vec<PathHits> {
        let mut v: Vec<PathHits> = self
            .hmr_by_path
            .iter()
            .map(|(p, c)| PathHits {
                path: p.clone(),
                hits: *c,
            })
            .collect();
        v.sort_by(|a, b| b.hits.cmp(&a.hits));
        v.truncate(n);
        v
    }
}

fn top_n_map(m: &HashMap<String, u64>, n: usize) -> Vec<PathHits> {
    let mut v: Vec<PathHits> = m
        .iter()
        .map(|(k, c)| PathHits {
            path: k.clone(),
            hits: *c,
        })
        .collect();
        v.sort_by(|a, b| b.hits.cmp(&a.hits));
        v.truncate(n);
        v
}

impl ViteState {

    /// SAP-style developer-experience score (0-100). Composite of:
    ///   - HMR rate normalized against a healthy 10/s baseline (weight 25)
    ///   - inverse of error rate (weight 30)
    ///   - inverse of transform p99 normalized against 100ms ceiling (weight 25)
    ///   - hot-swap latency normalized against 10ms ceiling (weight 20, perfect
    ///     when no samples — neutral default for projects without hot-swap yet)
    pub fn score(&self) -> SapScore {
        let hmr = (self.hmr_rate_60s() / 10.0).min(1.0);
        let errs = if self.hmr_total == 0 {
            1.0
        } else {
            1.0 - (self.error_total as f32 / self.hmr_total.max(1) as f32).min(1.0)
        };
        let p99 = self
            .transform_percentile(0.99)
            .map(|v| 1.0 - (v as f32 / 100.0).min(1.0))
            .unwrap_or(1.0);
        let hot_swap = self
            .hot_swap_percentile(0.5)
            .map(|v| 1.0 - ((v as f32 - 3.0).max(0.0) / 10.0).min(1.0))
            .unwrap_or(1.0);
        let composite = (hmr * 25.0 + errs * 30.0 + p99 * 25.0 + hot_swap * 20.0).clamp(0.0, 100.0);
        SapScore {
            hmr_rate_score: (hmr * 100.0) as u8,
            type_safety_score: (errs * 100.0) as u8,
            transform_score: (p99 * 100.0) as u8,
            hot_swap_score: (hot_swap * 100.0) as u8,
            composite: composite as u8,
        }
    }

    /// Borrow the recent-events ring (cheapest UI source).
    pub fn recent_events(&self) -> impl Iterator<Item = &ViteEvent> {
        self.recent.iter()
    }
}

/// A serializable point-in-time view of the engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViteSnapshot {
    pub port: Option<u16>,
    pub last_event_ms: u64,
    pub hmr_total: u64,
    pub page_reload_total: u64,
    pub error_total: u64,
    pub exit_code: Option<i32>,
    pub hmr_rate_60s: f32,
    pub transform_p50_ms: Option<u32>,
    pub transform_p99_ms: Option<u32>,
    pub top_paths: Vec<PathHits>,
    pub recent_kinds: Vec<HmrKind>,
    pub hot_swap_total: u64,
    pub hot_swap_ok: u64,
    pub hot_swap_p50_ms: Option<u32>,
    pub hot_swap_p99_ms: Option<u32>,
    pub last_hot_swap_ms: Option<u64>,
    pub chiron_op_total: u64,
    pub chiron_op_ok: u64,
    pub chiron_by_arm: Vec<PathHits>,
    pub search_tap_total: u64,
    pub search_taps_top_patterns: Vec<PathHits>,
    pub score: SapScore,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathHits {
    pub path: String,
    pub hits: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SapScore {
    pub hmr_rate_score: u8,
    pub type_safety_score: u8,
    pub transform_score: u8,
    pub hot_swap_score: u8,
    pub composite: u8,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{ViteEvent, ViteEventKind};

    #[test]
    fn apply_hmr_increments_total_and_rate() {
        let mut s = ViteState::new();
        for i in 0..5 {
            let ev = ViteEvent {
                ts_ms: 1_000_000_000_000 + i * 1000,
                kind: ViteEventKind::HmrUpdate {
                    path: format!("/src/App.tsx"),
                    kind: HmrKind::Js,
                },
            };
            s.apply(&ev);
        }
        assert_eq!(s.hmr_total, 5);
        let snap = s.snapshot();
        assert!(snap.hmr_rate_60s > 0.0);
        assert_eq!(snap.top_paths.len(), 1);
        assert_eq!(snap.top_paths[0].hits, 5);
    }

    #[test]
    fn transform_percentiles_sane() {
        let mut s = ViteState::new();
        for ms in [4, 6, 8, 10, 12, 14, 50, 80] {
            s.apply(&ViteEvent {
                ts_ms: 0,
                kind: ViteEventKind::Transform {
                    path: "x.tsx".into(),
                    stage: TransformStage::Swc,
                    ms,
                },
            });
        }
        let snap = s.snapshot();
        assert!(snap.transform_p50_ms.unwrap() >= 10);
        assert!(snap.transform_p99_ms.unwrap() >= 50);
    }

    #[test]
    fn score_in_bounds() {
        let mut s = ViteState::new();
        for _ in 0..10 {
            s.apply(&ViteEvent {
                ts_ms: 0,
                kind: ViteEventKind::HmrUpdate {
                    path: "a".into(),
                    kind: HmrKind::Js,
                },
            });
        }
        let snap = s.snapshot();
        assert!(snap.score.composite <= 100);
    }
}
