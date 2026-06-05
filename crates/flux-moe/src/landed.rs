//! landed.rs — the auto-integration **landing zone**.
//!
//! `flux-moe pipeline --land …/landed.rs` appends here ONLY code that passed the 2-of-2 gate
//! (rustc compile + DeepSeek judge) AND survived the post-write whole-crate verify. A failed verify
//! rolls back, so this file always compiles as part of the crate. Items below were written by the
//! pipeline itself — qwen3.6 drafted, the gate proved, the pipeline landed.

pub fn clamp01(x: f32) -> f32 {
    x.max(0.0).min(1.0)
}
