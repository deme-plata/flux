//! flux-bbq — **the agentic grill.** 🔥
//!
//! Put LLM/agent jobs (`Skewer`s) on the `Pit`; they cook on a GPU endpoint and
//! you collect them (`Cooked`) when done. The Pit enforces a **concurrency cap**
//! so a single box never gets overloaded — the lesson paid for in blood when 4
//! heavy jobs hit one A100 at once and every one died with `TimeoutError`. The
//! default is **serial** (cap = 1): safe first, turn the heat up deliberately.
//!
//! The grill function is injectable, so the scheduler is testable with zero
//! network; `ollama_grill` is the batteries-included caller.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// One job on the grill.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skewer {
    pub id: String,
    pub model: String,
    pub prompt: String,
}

impl Skewer {
    pub fn new(id: impl Into<String>, model: impl Into<String>, prompt: impl Into<String>) -> Self {
        Skewer { id: id.into(), model: model.into(), prompt: prompt.into() }
    }
}

/// A cooked result (always returned in the same order skewers went on).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cooked {
    pub id: String,
    pub output: String,
    pub ms: u128,
    pub ok: bool,
    pub error: String,
}

/// The grill. `max_concurrent` is the heat — how many skewers cook at once.
#[derive(Debug, Clone)]
pub struct Pit {
    pub endpoint: String,
    pub max_concurrent: usize,
}

impl Pit {
    /// Default: **serial** (one skewer at a time). The safe heat.
    pub fn new(endpoint: impl Into<String>) -> Self {
        Pit { endpoint: endpoint.into(), max_concurrent: 1 }
    }

    /// Turn the heat up — cook up to `n` skewers at once. Clamped to ≥1.
    pub fn with_heat(mut self, n: usize) -> Self {
        self.max_concurrent = n.max(1);
        self
    }

    /// Cook all skewers under the concurrency cap, results in input order.
    /// `grill` is the per-skewer call (inject `ollama_grill(&self.endpoint)` for
    /// real inference, or a stub in tests).
    pub fn cook<F>(&self, skewers: &[Skewer], grill: F) -> Vec<Cooked>
    where
        F: Fn(&Skewer) -> Result<String, String> + Sync,
    {
        let n = self.max_concurrent.max(1);
        let mut out: Vec<Option<Cooked>> = (0..skewers.len()).map(|_| None).collect();
        for start in (0..skewers.len()).step_by(n) {
            let end = (start + n).min(skewers.len());
            std::thread::scope(|s| {
                let handles: Vec<_> = (start..end)
                    .map(|i| {
                        let sk = &skewers[i];
                        let g = &grill;
                        s.spawn(move || cook_one(sk, g))
                    })
                    .collect();
                for (i, h) in (start..end).zip(handles) {
                    out[i] = Some(h.join().unwrap_or_else(|_| Cooked {
                        id: skewers[i].id.clone(),
                        output: String::new(),
                        ms: 0,
                        ok: false,
                        error: "grill thread panicked".into(),
                    }));
                }
            });
        }
        out.into_iter().map(|o| o.expect("every slot cooked")).collect()
    }
}

fn cook_one<F>(sk: &Skewer, grill: &F) -> Cooked
where
    F: Fn(&Skewer) -> Result<String, String>,
{
    let t0 = Instant::now();
    let r = grill(sk);
    let ms = t0.elapsed().as_millis();
    match r {
        Ok(output) => Cooked { id: sk.id.clone(), output, ms, ok: true, error: String::new() },
        Err(error) => Cooked { id: sk.id.clone(), output: String::new(), ms, ok: false, error },
    }
}

/// Batteries-included grill function: call an Ollama `/api/generate` endpoint.
/// `endpoint` is e.g. `http://108.143.3.52:16083`.
pub fn ollama_grill(endpoint: &str) -> impl Fn(&Skewer) -> Result<String, String> + Sync {
    let ep = endpoint.trim_end_matches('/').to_string();
    move |sk: &Skewer| {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(180))
            .build()
            .map_err(|e| e.to_string())?;
        let body = serde_json::json!({ "model": sk.model, "prompt": sk.prompt, "stream": false });
        let v: serde_json::Value = client
            .post(format!("{ep}/api/generate"))
            .json(&body)
            .send()
            .map_err(|e| e.to_string())?
            .json()
            .map_err(|e| e.to_string())?;
        v.get("response")
            .and_then(|r| r.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| {
                v.get("error").and_then(|e| e.as_str()).unwrap_or("no response field").to_string()
            })
    }
}

/// How many cooked ok vs failed (for a one-line report).
pub fn tally(cooked: &[Cooked]) -> (usize, usize) {
    let ok = cooked.iter().filter(|c| c.ok).count();
    (ok, cooked.len() - ok)
}

/// A **swarm of grills** — many boxes joined into one pit-line. Skewers are
/// fanned across the boxes (round-robin) and cooked in parallel; each box still
/// respects its own heat cap, so adding boxes adds throughput without ever
/// overloading any single one. This is "join the swarm": N cheap boxes sharing
/// the load instead of one mega-box.
///
/// (Note: this parallelises *independent jobs* across boxes. Serving ONE model
/// bigger than a single box's VRAM — e.g. R1-671B across 8 GPUs — is a different
/// thing: tensor/pipeline parallelism inside one logical endpoint, which the
/// serve layer sets up; the Swarm then load-balances requests to it.)
#[derive(Debug, Clone)]
pub struct Swarm {
    pub pits: Vec<Pit>,
}

impl Swarm {
    /// Join these box endpoints into a swarm (each serial by default).
    pub fn join(endpoints: &[&str]) -> Self {
        Swarm { pits: endpoints.iter().map(|e| Pit::new(*e)).collect() }
    }

    /// Set every box's heat (per-box concurrency cap).
    pub fn with_heat(mut self, n: usize) -> Self {
        for p in &mut self.pits {
            p.max_concurrent = n.max(1);
        }
        self
    }

    /// Fan skewers across the boxes and cook them all. `grill` is called with the
    /// (endpoint, skewer) so it knows which box is cooking. Results come back in
    /// input order. Boxes run in parallel; each box honours its own heat.
    pub fn cook<F>(&self, skewers: &[Skewer], grill: F) -> Vec<Cooked>
    where
        F: Fn(&str, &Skewer) -> Result<String, String> + Sync,
    {
        let np = self.pits.len().max(1);
        if self.pits.is_empty() {
            return Vec::new();
        }
        let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); np];
        for i in 0..skewers.len() {
            buckets[i % np].push(i);
        }
        let mut out: Vec<Option<Cooked>> = (0..skewers.len()).map(|_| None).collect();
        std::thread::scope(|s| {
            let handles: Vec<_> = self
                .pits
                .iter()
                .enumerate()
                .map(|(pi, pit)| {
                    let idxs = &buckets[pi];
                    let all = skewers;
                    let g = &grill;
                    let ep = pit.endpoint.clone();
                    s.spawn(move || {
                        let sub: Vec<Skewer> = idxs.iter().map(|&i| all[i].clone()).collect();
                        let cooked = pit.cook(&sub, |sk| g(&ep, sk));
                        idxs.iter().cloned().zip(cooked).collect::<Vec<(usize, Cooked)>>()
                    })
                })
                .collect();
            for h in handles {
                for (i, c) in h.join().unwrap_or_default() {
                    out[i] = Some(c);
                }
            }
        });
        out.into_iter().map(|o| o.expect("every slot cooked")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn skewers(n: usize) -> Vec<Skewer> {
        (0..n).map(|i| Skewer::new(format!("s{i}"), "stub", format!("prompt {i}"))).collect()
    }

    #[test]
    fn order_is_preserved_and_all_cook() {
        let pit = Pit::new("http://x").with_heat(4);
        let out = pit.cook(&skewers(10), |sk| Ok(format!("done:{}", sk.id)));
        assert_eq!(out.len(), 10);
        for (i, c) in out.iter().enumerate() {
            assert_eq!(c.id, format!("s{i}"));
            assert_eq!(c.output, format!("done:s{i}"));
            assert!(c.ok);
        }
    }

    #[test]
    fn concurrency_cap_is_respected() {
        // the whole point: never exceed max_concurrent simultaneous skewers.
        let cur = AtomicUsize::new(0);
        let max_seen = AtomicUsize::new(0);
        let pit = Pit::new("http://x").with_heat(3);
        let _ = pit.cook(&skewers(12), |sk| {
            let c = cur.fetch_add(1, Ordering::SeqCst) + 1;
            max_seen.fetch_max(c, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(15));
            cur.fetch_sub(1, Ordering::SeqCst);
            Ok(sk.id.clone())
        });
        assert!(max_seen.load(Ordering::SeqCst) <= 3, "exceeded heat cap: {}", max_seen.load(Ordering::SeqCst));
    }

    #[test]
    fn serial_by_default_never_parallel() {
        let cur = AtomicUsize::new(0);
        let max_seen = AtomicUsize::new(0);
        let pit = Pit::new("http://x"); // default heat = 1
        let _ = pit.cook(&skewers(5), |sk| {
            let c = cur.fetch_add(1, Ordering::SeqCst) + 1;
            max_seen.fetch_max(c, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(5));
            cur.fetch_sub(1, Ordering::SeqCst);
            Ok(sk.id.clone())
        });
        assert_eq!(max_seen.load(Ordering::SeqCst), 1, "default pit must be serial");
    }

    #[test]
    fn errors_are_captured_not_fatal() {
        let pit = Pit::new("http://x").with_heat(2);
        let out = pit.cook(&skewers(4), |sk| {
            if sk.id == "s2" { Err("burnt".into()) } else { Ok("ok".into()) }
        });
        let (ok, fail) = tally(&out);
        assert_eq!((ok, fail), (3, 1));
        let burnt = out.iter().find(|c| c.id == "s2").unwrap();
        assert!(!burnt.ok);
        assert_eq!(burnt.error, "burnt");
    }

    #[test]
    fn empty_pit_cooks_nothing() {
        let pit = Pit::new("http://x");
        assert!(pit.cook(&[], |_| Ok(String::new())).is_empty());
    }

    #[test]
    fn swarm_fans_across_boxes_in_order() {
        let swarm = Swarm::join(&["http://box-a", "http://box-b", "http://box-c"]);
        let out = swarm.cook(&skewers(9), |endpoint, sk| Ok(format!("{endpoint}|{}", sk.id)));
        assert_eq!(out.len(), 9);
        // results in input order
        for (i, c) in out.iter().enumerate() {
            assert_eq!(c.id, format!("s{i}"));
            assert!(c.ok);
        }
        // round-robin: s0→box-a, s1→box-b, s2→box-c, s3→box-a ...
        assert!(out[0].output.starts_with("http://box-a|"));
        assert!(out[1].output.starts_with("http://box-b|"));
        assert!(out[2].output.starts_with("http://box-c|"));
        assert!(out[3].output.starts_with("http://box-a|"));
    }

    #[test]
    fn swarm_each_box_keeps_its_own_heat_cap() {
        // 3 boxes × heat 2 = at most 6 concurrent overall, ≤2 per box.
        let cur = AtomicUsize::new(0);
        let max_seen = AtomicUsize::new(0);
        let swarm = Swarm::join(&["a", "b", "c"]).with_heat(2);
        let _ = swarm.cook(&skewers(30), |_ep, sk| {
            let c = cur.fetch_add(1, Ordering::SeqCst) + 1;
            max_seen.fetch_max(c, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(8));
            cur.fetch_sub(1, Ordering::SeqCst);
            Ok(sk.id.clone())
        });
        // 3 boxes × heat 2 → never more than 6 at once
        assert!(max_seen.load(Ordering::SeqCst) <= 6, "swarm exceeded total cap: {}", max_seen.load(Ordering::SeqCst));
    }

    #[test]
    fn empty_swarm_cooks_nothing() {
        let swarm = Swarm { pits: vec![] };
        assert!(swarm.cook(&skewers(3), |_, _| Ok(String::new())).is_empty());
    }
}
