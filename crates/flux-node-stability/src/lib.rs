//! flux-node-stability — pure control primitives for native graph nodes.
//!
//! ## Why this exists
//!
//! q-api-server's `memory_backpressure_active()` (block_producer.rs) is a
//! single hard latch: production runs when anon-RSS ≤ `Q_MAX_PRODUCE_RSS_GB`
//! and stops the instant it exceeds it. With one threshold the node *flaps* —
//! RSS sits right at the ceiling, so each sample flips pause↔resume, which is
//! the documented OOM death-loop oscillation (RSS spirals while production
//! restarts every few minutes).
//!
//! The fix control theory has known for a century: **hysteresis**. Pause at a
//! HIGH watermark; resume only after RSS falls under a strictly LOWER one; and
//! refuse to switch faster than a minimum dwell. The node then makes ONE clean
//! pause, lets DAG cleanup / allocator decay actually catch up, and makes ONE
//! clean resume — no flapping.
//!
//! This crate is pure (no I/O, no allocation in the hot path) so it verifies
//! instantly with `flux_combo` and can be unit-tested offline, then grafted
//! into the node's produce loop. The node keeps owning the RSS *sampling*
//! (`/proc/self/status`); this owns the *decision*.

/// Whether block production should run right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProduceState {
    /// Produce blocks normally.
    Run,
    /// Memory backpressure active — pause production so cleanup catches up.
    Paused,
}

/// A state change the controller just made — returned by
/// [`BackpressureController::observe_event`] so the node logs exactly once per
/// transition (not once per sample), which is what keeps the journal readable
/// while still capturing every pause/resume for post-mortem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transition {
    /// Just crossed the high watermark → production paused.
    Paused,
    /// Just dropped under the low watermark → production resumed.
    Resumed,
}

/// Hysteresis backpressure controller. Construct once; feed it monotonic
/// timestamps + RSS samples via [`observe`](Self::observe); it returns the
/// current [`ProduceState`]. Replaces the single-threshold latch.
#[derive(Clone, Debug)]
pub struct BackpressureController {
    /// Pause when RSS rises above this (GB).
    high_gb: f64,
    /// Resume only when RSS falls below this (GB). MUST be < `high_gb`.
    low_gb: f64,
    /// Don't flip state more often than this many seconds, even if the
    /// threshold says to — gives cleanup a guaranteed window to work.
    min_dwell_s: f64,
    state: ProduceState,
    /// Timestamp (seconds, monotonic) of the last state change.
    last_change_s: f64,
    /// How many times production has been paused (observability).
    pauses: u64,
    /// When the current pause began, if paused — used to accumulate paused time.
    pause_enter_s: Option<f64>,
    /// Cumulative seconds spent paused across all completed pauses.
    paused_accum_s: f64,
}

impl BackpressureController {
    /// New controller. `high_gb` is the pause ceiling (e.g. the old
    /// `Q_MAX_PRODUCE_RSS_GB`); `low_gb` is the resume floor; `min_dwell_s` is
    /// the anti-flap window. If `low_gb >= high_gb` it is clamped to a sane
    /// band (`high_gb` minus a 2 GB margin, floored at 0) so a misconfig can
    /// never collapse the hysteresis back into a single-threshold latch.
    pub fn new(high_gb: f64, low_gb: f64, min_dwell_s: f64) -> Self {
        let low_gb = if low_gb >= high_gb {
            (high_gb - 2.0).max(0.0)
        } else {
            low_gb
        };
        Self {
            high_gb,
            low_gb,
            min_dwell_s: min_dwell_s.max(0.0),
            state: ProduceState::Run,
            last_change_s: f64::NEG_INFINITY,
            pauses: 0,
            pause_enter_s: None,
            paused_accum_s: 0.0,
        }
    }

    /// Sensible default for Epsilon: pause at 30 GB, resume under 24 GB, dwell
    /// 45 s. Matches the documented "legit baseline 24–28 G < latch 30" geometry
    /// (the latch must never sit below ~30 G or production stalls forever).
    pub fn epsilon_default() -> Self {
        Self::new(30.0, 24.0, 45.0)
    }

    /// Graft-friendly constructor: each parameter is the parsed value of the
    /// node's existing ops tunable, or `None` to take the Epsilon default for
    /// that one field. The node side stays a thin one-liner:
    ///
    /// ```ignore
    /// let high = std::env::var("Q_MAX_PRODUCE_RSS_GB").ok().and_then(|s| s.parse().ok());
    /// let low  = std::env::var("Q_RESUME_PRODUCE_RSS_GB").ok().and_then(|s| s.parse().ok());
    /// let dwell = std::env::var("Q_BACKPRESSURE_DWELL_S").ok().and_then(|s| s.parse().ok());
    /// let ctl = BackpressureController::from_config(high, low, dwell);
    /// ```
    ///
    /// Pure (reads no env itself) so it unit-tests without process-global
    /// state. `Q_MAX_PRODUCE_RSS_GB` keeps its existing meaning = the pause
    /// ceiling, so the old single tunable carries straight over; `low`/`dwell`
    /// are the new knobs that turn the latch into hysteresis. The `low < high`
    /// invariant is still enforced by [`new`](Self::new)'s clamp.
    pub fn from_config(high_gb: Option<f64>, low_gb: Option<f64>, dwell_s: Option<f64>) -> Self {
        let d = Self::epsilon_default();
        Self::new(
            high_gb.unwrap_or(d.high_gb),
            // Default the resume floor to 6 GB under whatever ceiling is set,
            // so overriding ONLY the ceiling still yields a real hysteresis band.
            low_gb.unwrap_or_else(|| (high_gb.unwrap_or(d.high_gb) - 6.0).max(0.0)),
            dwell_s.unwrap_or(d.min_dwell_s),
        )
    }

    pub fn state(&self) -> ProduceState {
        self.state
    }

    /// True iff production should pause right now.
    pub fn is_paused(&self) -> bool {
        self.state == ProduceState::Paused
    }

    /// Core step: apply one observation, returning the transition that just
    /// happened (if any) and updating observability counters. Both `observe`
    /// and `observe_event` go through here so the logic exists once.
    fn step(&mut self, now_s: f64, rss_gb: f64) -> Option<Transition> {
        let dwell_ok = now_s - self.last_change_s >= self.min_dwell_s;
        match self.state {
            ProduceState::Run => {
                if rss_gb > self.high_gb && dwell_ok {
                    self.state = ProduceState::Paused;
                    self.last_change_s = now_s;
                    self.pauses += 1;
                    self.pause_enter_s = Some(now_s);
                    return Some(Transition::Paused);
                }
            }
            ProduceState::Paused => {
                if rss_gb < self.low_gb && dwell_ok {
                    self.state = ProduceState::Run;
                    self.last_change_s = now_s;
                    if let Some(entered) = self.pause_enter_s.take() {
                        self.paused_accum_s += now_s - entered;
                    }
                    return Some(Transition::Resumed);
                }
            }
        }
        None
    }

    /// Feed one observation: `now_s` is a monotonic clock in seconds (e.g.
    /// `Instant`-derived), `rss_gb` the process anon-RSS. Returns the resulting
    /// state. Transitions only happen when BOTH the watermark is crossed AND
    /// the min-dwell has elapsed since the last change.
    pub fn observe(&mut self, now_s: f64, rss_gb: f64) -> ProduceState {
        self.step(now_s, rss_gb);
        self.state
    }

    /// Like [`observe`](Self::observe) but returns `Some(Transition)` ONLY on
    /// the sample where the state actually changed, else `None`. Lets the node
    /// emit a single `warn!`/`info!` per pause/resume instead of spamming the
    /// journal every sample (the readability fix the hard latch lacked).
    pub fn observe_event(&mut self, now_s: f64, rss_gb: f64) -> Option<Transition> {
        self.step(now_s, rss_gb)
    }

    /// Number of times production has been paused since construction.
    pub fn pause_count(&self) -> u64 {
        self.pauses
    }

    /// Total seconds spent paused, including the in-progress pause measured up
    /// to `now_s`. The headline node-health metric: "how much production time
    /// did backpressure cost us?"
    pub fn total_paused_s(&self, now_s: f64) -> f64 {
        let in_progress = self.pause_enter_s.map_or(0.0, |entered| (now_s - entered).max(0.0));
        self.paused_accum_s + in_progress
    }
}

/// Crash-loop governor. The OOM death-loop didn't only spiral memory — it
/// also restarted the node every 1–5 minutes, and each cold start re-loads the
/// DAG/working set, which *feeds* the next OOM. A node (or its supervisor
/// shim) records each start here; the governor reports whether starts are
/// happening too fast and how long to back off before the next one, so a
/// genuinely-broken node settles instead of hammering restart.
///
/// Pure + deterministic (caller supplies monotonic timestamps), so it unit
/// tests offline. Pairs with [`BackpressureController`]: one paces production,
/// the other paces restarts.
#[derive(Clone, Debug)]
pub struct RestartGovernor {
    /// Count restarts within this trailing window (seconds).
    window_s: f64,
    /// At or above this many restarts in the window ⇒ crash-looping.
    max_in_window: u32,
    /// Backoff for the first over-threshold restart (seconds).
    base_backoff_s: f64,
    /// Backoff ceiling (seconds).
    max_backoff_s: f64,
    /// Restart timestamps (monotonic seconds), pruned to the window.
    restarts: std::collections::VecDeque<f64>,
}

impl RestartGovernor {
    pub fn new(window_s: f64, max_in_window: u32, base_backoff_s: f64, max_backoff_s: f64) -> Self {
        Self {
            window_s: window_s.max(0.0),
            max_in_window: max_in_window.max(1),
            base_backoff_s: base_backoff_s.max(0.0),
            max_backoff_s: max_backoff_s.max(0.0),
            restarts: std::collections::VecDeque::new(),
        }
    }

    /// Node default: more than 5 restarts in 5 minutes is a loop; back off
    /// 2 s, doubling, capped at 60 s.
    pub fn node_default() -> Self {
        Self::new(300.0, 5, 2.0, 60.0)
    }

    /// Record a restart at `now_s` and prune anything older than the window.
    pub fn record_restart(&mut self, now_s: f64) {
        self.restarts.push_back(now_s);
        self.prune(now_s);
    }

    fn prune(&mut self, now_s: f64) {
        let cutoff = now_s - self.window_s;
        while self.restarts.front().is_some_and(|&t| t < cutoff) {
            self.restarts.pop_front();
        }
    }

    /// Restarts within the trailing window as of `now_s`.
    pub fn restarts_in_window(&mut self, now_s: f64) -> u32 {
        self.prune(now_s);
        self.restarts.len() as u32
    }

    /// True when restarts in the window have hit the crash-loop threshold.
    pub fn is_crash_looping(&mut self, now_s: f64) -> bool {
        self.restarts_in_window(now_s) >= self.max_in_window
    }

    /// Seconds to wait before the next restart: `0` while under threshold,
    /// then `base * 2^(over)` capped at `max_backoff_s`, where `over` is how
    /// many restarts beyond the threshold have occurred. Exponential so a
    /// persistent loop quickly stretches to the ceiling instead of hammering.
    pub fn backoff_s(&mut self, now_s: f64) -> f64 {
        let count = self.restarts_in_window(now_s);
        if count < self.max_in_window {
            return 0.0;
        }
        let over = count - self.max_in_window; // 0 on the first over-threshold restart
        let factor = 2f64.powi(over as i32);
        (self.base_backoff_s * factor).min(self.max_backoff_s)
    }
}

/// Parse anonymous-RSS in GB from the text of `/proc/<pid>/status`. Pure, so
/// the parsing is unit-tested with a fixture rather than depending on live
/// process memory. Mirrors q-api-server's existing `current_anon_rss_gb` line
/// scan exactly, so moving the sampler into this crate is a behaviour-neutral
/// swap. Returns `None` if no `RssAnon:` line is present.
pub fn parse_rss_anon_gb(status_text: &str) -> Option<f64> {
    for line in status_text.lines() {
        if let Some(rest) = line.strip_prefix("RssAnon:") {
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kb as f64 / 1_048_576.0);
        }
    }
    None
}

/// Sample this process's anonymous RSS in GB from `/proc/self/status` (Linux).
/// `None` off-Linux or on read/parse failure — callers treat that as "no
/// backpressure signal available", never as 0 GB. This is the one impure
/// function in the crate; the decision logic above it stays pure.
pub fn sample_rss_gb() -> Option<f64> {
    let text = std::fs::read_to_string("/proc/self/status").ok()?;
    parse_rss_anon_gb(&text)
}

/// Coarse alert level derived from a [`HealthSnapshot`] — the single signal a
/// monitor pages on, so alerting logic lives here (tested) instead of being
/// re-derived ad-hoc at every call site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealthLevel {
    /// Producing normally, not restarting too fast.
    Ok,
    /// Memory backpressure has paused production — expected + self-correcting,
    /// but worth surfacing.
    Degraded,
    /// Crash-looping — needs attention; the node can't stay up.
    Critical,
}

impl HealthLevel {
    /// The HTTP status a load-balancer `/health` probe should return for this
    /// level. The ops-critical nuance: a memory pause (Degraded) keeps `200` —
    /// block production is paused but the API still serves reads, so draining
    /// the node would needlessly remove a working endpoint. Only a crash-loop
    /// (Critical) returns `503` to drain traffic off a node that can't stay up.
    pub fn as_http_status(self) -> u16 {
        match self {
            HealthLevel::Ok | HealthLevel::Degraded => 200,
            HealthLevel::Critical => 503,
        }
    }

    /// Whether an LB should keep routing traffic here (true for Ok/Degraded).
    pub fn should_serve_traffic(self) -> bool {
        self.as_http_status() == 200
    }
}

/// A point-in-time read of node stability health — what the node would expose
/// on `/status` or log periodically. Plain numbers so it serializes trivially
/// without forcing a serde dep into this pure crate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HealthSnapshot {
    /// Should production run or is it paused under memory backpressure?
    pub produce_state: ProduceState,
    /// Times production has paused since start.
    pub pauses: u64,
    /// Cumulative seconds spent paused (incl. an in-progress pause).
    pub total_paused_s: f64,
    /// Restarts within the governor's trailing window.
    pub restarts_in_window: u32,
    /// Is the node restarting too fast?
    pub crash_looping: bool,
    /// Seconds to wait before the next restart (0 if not looping).
    pub restart_backoff_s: f64,
}

impl HealthSnapshot {
    /// Classify into a single alert level. Crash-looping dominates (can't stay
    /// up); a memory pause alone is Degraded (expected, self-correcting);
    /// otherwise Ok.
    pub fn level(&self) -> HealthLevel {
        if self.crash_looping {
            HealthLevel::Critical
        } else if self.produce_state == ProduceState::Paused {
            HealthLevel::Degraded
        } else {
            HealthLevel::Ok
        }
    }

    /// True iff fully healthy (producing, not crash-looping).
    pub fn is_healthy(&self) -> bool {
        self.level() == HealthLevel::Ok
    }

    /// `"OK"` / `"DEGRADED"` / `"CRITICAL"` for the current level.
    pub fn level_str(&self) -> &'static str {
        match self.level() {
            HealthLevel::Ok => "OK",
            HealthLevel::Degraded => "DEGRADED",
            HealthLevel::Critical => "CRITICAL",
        }
    }

    /// Compact one-line status for a periodic `info!`/`warn!`, e.g.
    /// `"stability=DEGRADED produce=Paused pauses=1(12.3s) restarts=2/win backoff=0.0s"`.
    pub fn summary_line(&self) -> String {
        let produce = match self.produce_state {
            ProduceState::Run => "Run",
            ProduceState::Paused => "Paused",
        };
        format!(
            "stability={} produce={produce} pauses={}({:.1}s) restarts={}/win backoff={:.1}s",
            self.level_str(),
            self.pauses,
            self.total_paused_s,
            self.restarts_in_window,
            self.restart_backoff_s
        )
    }

    /// Minimal JSON object for the node's `/status` endpoint. Hand-rolled so
    /// this stays a zero-dependency crate (no serde). All fields are numbers,
    /// bools, or fixed enum strings, so no escaping is required.
    pub fn to_json(&self) -> String {
        let produce = match self.produce_state {
            ProduceState::Run => "Run",
            ProduceState::Paused => "Paused",
        };
        format!(
            "{{\"level\":\"{}\",\"produce_state\":\"{}\",\"pauses\":{},\
             \"total_paused_s\":{:.3},\"restarts_in_window\":{},\
             \"crash_looping\":{},\"restart_backoff_s\":{:.3}}}",
            self.level_str(),
            produce,
            self.pauses,
            self.total_paused_s,
            self.restarts_in_window,
            self.crash_looping,
            self.restart_backoff_s
        )
    }
}

/// The single object a node holds: composes the memory-pacing
/// [`BackpressureController`] and the restart-pacing [`RestartGovernor`] so the
/// graft into q-api-server touches ONE type. Feed it RSS samples and restart
/// events; read a [`HealthSnapshot`] for logs / `/status`.
#[derive(Clone, Debug)]
pub struct NodeStabilitySupervisor {
    backpressure: BackpressureController,
    restarts: RestartGovernor,
}

impl NodeStabilitySupervisor {
    pub fn new(backpressure: BackpressureController, restarts: RestartGovernor) -> Self {
        Self { backpressure, restarts }
    }

    /// Epsilon-tuned defaults for both sub-controllers.
    pub fn epsilon_default() -> Self {
        Self::new(BackpressureController::epsilon_default(), RestartGovernor::node_default())
    }

    /// Feed one RSS sample; returns whether production should run now.
    pub fn observe_memory(&mut self, now_s: f64, rss_gb: f64) -> ProduceState {
        self.backpressure.observe(now_s, rss_gb)
    }

    /// Like [`observe_memory`](Self::observe_memory) but yields the transition
    /// (if any) so the node logs once per pause/resume.
    pub fn observe_memory_event(&mut self, now_s: f64, rss_gb: f64) -> Option<Transition> {
        self.backpressure.observe_event(now_s, rss_gb)
    }

    /// Record a node (re)start; the governor uses it for crash-loop backoff.
    pub fn note_restart(&mut self, now_s: f64) {
        self.restarts.record_restart(now_s);
    }

    /// Seconds to wait before the next restart (0 when not crash-looping).
    pub fn restart_backoff_s(&mut self, now_s: f64) -> f64 {
        self.restarts.backoff_s(now_s)
    }

    /// Current health for logging / `/status`.
    pub fn snapshot(&mut self, now_s: f64) -> HealthSnapshot {
        HealthSnapshot {
            produce_state: self.backpressure.state(),
            pauses: self.backpressure.pause_count(),
            total_paused_s: self.backpressure.total_paused_s(now_s),
            restarts_in_window: self.restarts.restarts_in_window(now_s),
            crash_looping: self.restarts.is_crash_looping(now_s),
            restart_backoff_s: self.restarts.backoff_s(now_s),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_is_clamped_below_high_on_misconfig() {
        // low >= high would collapse to a single latch — must be clamped.
        let c = BackpressureController::new(30.0, 35.0, 10.0);
        assert!(c.low_gb < c.high_gb);
        assert_eq!(c.low_gb, 28.0);
    }

    #[test]
    fn pauses_above_high_resumes_below_low() {
        let mut c = BackpressureController::new(30.0, 24.0, 0.0);
        assert_eq!(c.observe(0.0, 25.0), ProduceState::Run); // below ceiling
        assert_eq!(c.observe(1.0, 31.0), ProduceState::Paused); // over high → pause
        // Between low and high while paused → STAY paused (the whole point).
        assert_eq!(c.observe(2.0, 27.0), ProduceState::Paused);
        assert_eq!(c.observe(3.0, 25.0), ProduceState::Paused);
        // Drop under low → resume.
        assert_eq!(c.observe(4.0, 23.0), ProduceState::Run);
    }

    #[test]
    fn no_flap_at_the_ceiling() {
        // RSS parks right at the old single threshold (30). A latch would flip
        // every sample; hysteresis must NOT — it stays Run until truly > high,
        // then stays Paused until truly < low.
        let mut c = BackpressureController::new(30.0, 24.0, 0.0);
        let mut flips = 0;
        let mut prev = c.state();
        for i in 0..50 {
            // oscillate 29.5 ↔ 30.5 around the ceiling
            let rss = if i % 2 == 0 { 29.5 } else { 30.5 };
            let s = c.observe(i as f64, rss);
            if s != prev {
                flips += 1;
                prev = s;
            }
        }
        // One pause at most — never the per-sample flapping of a single latch.
        assert!(flips <= 1, "hysteresis should not flap; got {flips} flips");
    }

    #[test]
    fn min_dwell_blocks_rapid_switching() {
        let mut c = BackpressureController::new(30.0, 24.0, 45.0);
        assert_eq!(c.observe(0.0, 31.0), ProduceState::Paused); // pause @ t=0
        // RSS drops under low almost immediately, but dwell (45s) not elapsed.
        assert_eq!(c.observe(10.0, 20.0), ProduceState::Paused);
        assert_eq!(c.observe(44.0, 20.0), ProduceState::Paused);
        // After the dwell window, the resume is allowed.
        assert_eq!(c.observe(46.0, 20.0), ProduceState::Run);
    }

    #[test]
    fn events_fire_once_per_transition_and_count() {
        let mut c = BackpressureController::new(30.0, 24.0, 0.0);
        assert_eq!(c.observe_event(0.0, 25.0), None); // no change
        assert_eq!(c.observe_event(1.0, 31.0), Some(Transition::Paused));
        assert_eq!(c.observe_event(2.0, 31.0), None); // still paused, no event
        assert_eq!(c.observe_event(3.0, 23.0), Some(Transition::Resumed));
        assert_eq!(c.observe_event(4.0, 23.0), None);
        // Two-edge cycle = exactly one pause counted.
        assert_eq!(c.pause_count(), 1);
    }

    #[test]
    fn total_paused_time_accumulates_including_in_progress() {
        let mut c = BackpressureController::new(30.0, 24.0, 0.0);
        c.observe(10.0, 31.0); // pause at t=10
        c.observe(13.0, 23.0); // resume at t=13 → 3s paused
        assert!((c.total_paused_s(13.0) - 3.0).abs() < 1e-9);
        c.observe(20.0, 31.0); // pause again at t=20
        // 3s completed + (25-20)=5s in progress = 8s.
        assert!((c.total_paused_s(25.0) - 8.0).abs() < 1e-9);
        assert_eq!(c.pause_count(), 2);
    }

    #[test]
    fn epsilon_default_geometry() {
        let c = BackpressureController::epsilon_default();
        assert_eq!((c.high_gb, c.low_gb, c.min_dwell_s), (30.0, 24.0, 45.0));
        assert!(c.low_gb < c.high_gb);
    }

    #[test]
    fn from_config_all_none_is_epsilon_default() {
        let c = BackpressureController::from_config(None, None, None);
        assert_eq!((c.high_gb, c.low_gb, c.min_dwell_s), (30.0, 24.0, 45.0));
    }

    #[test]
    fn from_config_ceiling_only_keeps_a_hysteresis_band() {
        // Operator bumps only Q_MAX_PRODUCE_RSS_GB=38 → resume floor auto-set
        // 6 GB under it (32), never collapsing to a single threshold.
        let c = BackpressureController::from_config(Some(38.0), None, None);
        assert_eq!(c.high_gb, 38.0);
        assert_eq!(c.low_gb, 32.0);
        assert!(c.low_gb < c.high_gb);
        assert_eq!(c.min_dwell_s, 45.0); // dwell still defaulted
    }

    #[test]
    fn from_config_full_override_and_misconfig_clamp() {
        let c = BackpressureController::from_config(Some(40.0), Some(28.0), Some(60.0));
        assert_eq!((c.high_gb, c.low_gb, c.min_dwell_s), (40.0, 28.0, 60.0));
        // low >= high still clamped by new().
        let bad = BackpressureController::from_config(Some(30.0), Some(31.0), Some(10.0));
        assert!(bad.low_gb < bad.high_gb);
    }

    #[test]
    fn governor_window_prunes_old_restarts() {
        let mut g = RestartGovernor::new(300.0, 5, 2.0, 60.0);
        g.record_restart(0.0);
        g.record_restart(100.0);
        g.record_restart(200.0);
        assert_eq!(g.restarts_in_window(250.0), 3);
        // At t=450, the t=0 and t=100 restarts have aged out of the 300s window.
        assert_eq!(g.restarts_in_window(450.0), 1);
    }

    #[test]
    fn governor_detects_crash_loop_at_threshold() {
        let mut g = RestartGovernor::new(300.0, 5, 2.0, 60.0);
        for i in 0..4 {
            g.record_restart(i as f64 * 10.0);
        }
        assert!(!g.is_crash_looping(40.0)); // 4 < 5
        g.record_restart(40.0);
        assert!(g.is_crash_looping(40.0)); // 5 >= 5
    }

    #[test]
    fn governor_backoff_is_exponential_and_capped() {
        let mut g = RestartGovernor::new(300.0, 3, 2.0, 60.0);
        g.record_restart(0.0);
        g.record_restart(1.0);
        assert_eq!(g.backoff_s(1.0), 0.0); // under threshold (2 < 3)
        g.record_restart(2.0); // count 3 == threshold, over=0 → base
        assert_eq!(g.backoff_s(2.0), 2.0);
        g.record_restart(3.0); // count 4, over=1 → base*2
        assert_eq!(g.backoff_s(3.0), 4.0);
        g.record_restart(4.0); // count 5, over=2 → base*4
        assert_eq!(g.backoff_s(4.0), 8.0);
        // Keep looping until the cap pins it at max_backoff_s.
        for t in 5..20 {
            g.record_restart(t as f64);
        }
        assert_eq!(g.backoff_s(19.0), 60.0);
    }

    #[test]
    fn governor_node_default_geometry() {
        let mut g = RestartGovernor::node_default();
        assert_eq!(g.backoff_s(0.0), 0.0); // no restarts yet
        for t in 0..5 {
            g.record_restart(t as f64);
        }
        assert!(g.is_crash_looping(4.0));
        assert_eq!(g.backoff_s(4.0), 2.0); // 5 restarts = threshold, base backoff
    }

    #[test]
    fn supervisor_snapshot_reflects_both_controllers() {
        let mut sup = NodeStabilitySupervisor::epsilon_default();
        // Healthy baseline: producing, no pauses, no restarts.
        let s0 = sup.snapshot(0.0);
        assert_eq!(s0.produce_state, ProduceState::Run);
        assert_eq!(s0.pauses, 0);
        assert_eq!(s0.restarts_in_window, 0);
        assert!(!s0.crash_looping);

        // Drive memory over the 30 GB ceiling → production pauses.
        assert_eq!(sup.observe_memory(100.0, 31.0), ProduceState::Paused);
        // Hammer restarts into a crash loop (>5 in window).
        for t in 100..106 {
            sup.note_restart(t as f64);
        }
        let s1 = sup.snapshot(106.0);
        assert_eq!(s1.produce_state, ProduceState::Paused);
        assert_eq!(s1.pauses, 1);
        assert!(s1.crash_looping);
        assert!(s1.restarts_in_window >= 5);
        assert!(s1.restart_backoff_s > 0.0); // backing off now
    }

    #[test]
    fn supervisor_memory_event_surfaces_transition() {
        let mut sup = NodeStabilitySupervisor::epsilon_default();
        assert_eq!(sup.observe_memory_event(0.0, 25.0), None);
        assert_eq!(sup.observe_memory_event(50.0, 31.0), Some(Transition::Paused));
    }

    #[test]
    fn parse_rss_anon_from_status_fixture() {
        // 2097152 kB = 2 GiB exactly.
        let status = "Name:\tq-api-server\nVmRSS:\t  9999999 kB\nRssAnon:\t  2097152 kB\nRssFile:\t   123 kB\n";
        let gb = parse_rss_anon_gb(status).expect("RssAnon present");
        assert!((gb - 2.0).abs() < 1e-9);
        // No RssAnon line ⇒ None (never a silent 0).
        assert!(parse_rss_anon_gb("Name:\tx\nVmRSS:\t 100 kB\n").is_none());
        // Malformed value ⇒ None.
        assert!(parse_rss_anon_gb("RssAnon:\tNaN kB\n").is_none());
    }

    #[test]
    fn sample_rss_is_some_on_linux() {
        // This crate's CI runs on Linux (Epsilon); the live sampler must read
        // a positive anon-RSS for the current process.
        #[cfg(target_os = "linux")]
        {
            let gb = sample_rss_gb().expect("linux /proc/self/status readable");
            assert!(gb > 0.0 && gb.is_finite());
        }
    }

    #[test]
    fn health_level_classifies_and_summarizes() {
        let mut sup = NodeStabilitySupervisor::epsilon_default();
        // Healthy.
        let s = sup.snapshot(0.0);
        assert_eq!(s.level(), HealthLevel::Ok);
        assert!(s.is_healthy());
        assert!(s.summary_line().contains("stability=OK"));
        assert!(s.summary_line().contains("produce=Run"));

        // Memory pause alone ⇒ Degraded (not Critical).
        sup.observe_memory(100.0, 31.0);
        let s = sup.snapshot(100.0);
        assert_eq!(s.level(), HealthLevel::Degraded);
        assert!(!s.is_healthy());
        assert!(s.summary_line().contains("stability=DEGRADED"));

        // Crash loop ⇒ Critical dominates.
        for t in 100..106 {
            sup.note_restart(t as f64);
        }
        let s = sup.snapshot(106.0);
        assert_eq!(s.level(), HealthLevel::Critical);
        assert!(s.summary_line().contains("stability=CRITICAL"));
    }

    #[test]
    fn health_level_http_status_drains_only_on_critical() {
        // Ok and Degraded keep serving (200); only Critical drains (503).
        assert_eq!(HealthLevel::Ok.as_http_status(), 200);
        assert_eq!(HealthLevel::Degraded.as_http_status(), 200);
        assert_eq!(HealthLevel::Critical.as_http_status(), 503);
        assert!(HealthLevel::Degraded.should_serve_traffic());
        assert!(!HealthLevel::Critical.should_serve_traffic());

        // Drive a supervisor to each level and check the snapshot agrees.
        let mut sup = NodeStabilitySupervisor::epsilon_default();
        assert_eq!(sup.snapshot(0.0).level().as_http_status(), 200); // healthy
        sup.observe_memory(10.0, 31.0); // pause → Degraded, still 200
        assert_eq!(sup.snapshot(10.0).level().as_http_status(), 200);
        for t in 10..16 {
            sup.note_restart(t as f64); // crash loop → Critical, 503
        }
        assert_eq!(sup.snapshot(16.0).level().as_http_status(), 503);
    }

    #[test]
    fn health_to_json_is_well_formed() {
        let mut sup = NodeStabilitySupervisor::epsilon_default();
        sup.observe_memory(10.0, 31.0); // pause
        let json = sup.snapshot(10.0).to_json();
        // Shape checks (no serde dep available to round-trip).
        assert!(json.starts_with('{') && json.ends_with('}'));
        assert_eq!(json.matches('{').count(), json.matches('}').count());
        assert!(json.contains("\"level\":\"DEGRADED\""));
        assert!(json.contains("\"produce_state\":\"Paused\""));
        assert!(json.contains("\"crash_looping\":false"));
        assert!(json.contains("\"pauses\":1"));
        // Every documented key is present.
        for key in [
            "level", "produce_state", "pauses", "total_paused_s",
            "restarts_in_window", "crash_looping", "restart_backoff_s",
        ] {
            assert!(json.contains(&format!("\"{key}\"")), "missing key {key}");
        }
    }
}
