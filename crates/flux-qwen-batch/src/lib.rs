// flux-qwen-batch — drive qwen3.6 (served on a Vast A100 via ollama) with BIG PARALLEL
// batches. ollama defaults to NUM_PARALLEL=1 (one small package at a time); this fans a
// batch of prompts across N workers in one sweep — the same "make the packages bigger →
// more efficient" lever we used on the job-index fetch, now on the model.
//
// FLUXFOOD discipline: std + serde_json only. No tokio, no reqwest — concurrency is
// std::thread::scope, HTTP is a 30-line raw TcpStream POST. Compiles in a blink, links small.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;

/// Anything that can answer one prompt — real ollama, or a test mock.
pub trait Transport: Sync {
    fn call(&self, prompt: &str) -> Result<String, String>;
}

/// The outcome of running a whole batch.
#[derive(Debug, Clone, Serialize)]
pub struct BatchResult {
    /// Responses in INPUT order (a worker pool that preserves indexing).
    pub responses: Vec<Result<String, String>>,
    pub total: usize,
    pub oks: usize,
    pub errs: usize,
    pub wall_ms: u128,
    /// prompts completed per second across the batch.
    pub throughput_per_s: f64,
}

/// Fans a batch of prompts across `parallel` workers, preserving input order.
pub struct BatchRunner {
    pub parallel: usize,
}

impl BatchRunner {
    pub fn new(parallel: usize) -> Self {
        Self { parallel: parallel.max(1) }
    }

    pub fn run<T: Transport>(&self, t: &T, prompts: &[String]) -> BatchResult {
        let n = prompts.len();
        if n == 0 {
            return BatchResult { responses: vec![], total: 0, oks: 0, errs: 0, wall_ms: 0, throughput_per_s: 0.0 };
        }
        let par = self.parallel.min(n);
        let next = AtomicUsize::new(0);
        let slots: Vec<Mutex<Option<Result<String, String>>>> = (0..n).map(|_| Mutex::new(None)).collect();
        let t0 = Instant::now();
        thread::scope(|s| {
            for _ in 0..par {
                s.spawn(|| loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    if i >= n {
                        break;
                    }
                    let r = t.call(&prompts[i]);
                    *slots[i].lock().unwrap() = Some(r);
                });
            }
        });
        let wall = t0.elapsed().as_millis();
        let responses: Vec<Result<String, String>> =
            slots.into_iter().map(|m| m.into_inner().unwrap().unwrap_or_else(|| Err("no result".into()))).collect();
        let oks = responses.iter().filter(|r| r.is_ok()).count();
        let secs = (wall as f64) / 1000.0;
        BatchResult {
            total: n,
            oks,
            errs: n - oks,
            wall_ms: wall,
            throughput_per_s: if secs > 0.0 { n as f64 / secs } else { n as f64 },
            responses,
        }
    }
}

/// The real transport: ollama `/api/generate` over a raw TCP HTTP/1.1 POST (no HTTP crate).
pub struct OllamaTransport {
    pub host: String,
    pub port: u16,
    pub model: String,
    pub num_predict: u32,
    pub timeout: Duration,
}

impl OllamaTransport {
    pub fn new(host: &str, port: u16, model: &str) -> Self {
        Self { host: host.to_string(), port, model: model.to_string(), num_predict: 64, timeout: Duration::from_secs(120) }
    }
}

impl Transport for OllamaTransport {
    fn call(&self, prompt: &str) -> Result<String, String> {
        let body = serde_json::json!({
            "model": self.model,
            "prompt": prompt,
            "stream": false,
            "options": { "num_predict": self.num_predict }
        })
        .to_string();
        let mut s = TcpStream::connect((self.host.as_str(), self.port)).map_err(|e| format!("connect: {e}"))?;
        s.set_read_timeout(Some(self.timeout)).ok();
        s.set_write_timeout(Some(self.timeout)).ok();
        let req = format!(
            "POST /api/generate HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            self.host,
            body.len(),
            body
        );
        s.write_all(req.as_bytes()).map_err(|e| format!("write: {e}"))?;
        let mut raw = String::new();
        s.read_to_string(&mut raw).map_err(|e| format!("read: {e}"))?;
        let body = raw.splitn(2, "\r\n\r\n").nth(1).ok_or_else(|| "no http body".to_string())?;
        let v: serde_json::Value = serde_json::from_str(body.trim()).map_err(|e| format!("json: {e} · body={}", &body.chars().take(80).collect::<String>()))?;
        Ok(v.get("response").and_then(|x| x.as_str()).unwrap_or("").trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic mock: sleeps `delay`, errors on prompts containing "BAD".
    struct Mock {
        delay: Duration,
    }
    impl Transport for Mock {
        fn call(&self, prompt: &str) -> Result<String, String> {
            thread::sleep(self.delay);
            if prompt.contains("BAD") {
                Err(format!("rejected: {prompt}"))
            } else {
                Ok(format!("ok:{prompt}"))
            }
        }
    }

    fn prompts(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("case {i}")).collect()
    }

    #[test]
    fn preserves_input_order() {
        let r = BatchRunner::new(4).run(&Mock { delay: Duration::from_millis(1) }, &prompts(8));
        for (i, resp) in r.responses.iter().enumerate() {
            assert_eq!(resp.as_ref().unwrap(), &format!("ok:case {i}"));
        }
        assert_eq!(r.oks, 8);
        assert_eq!(r.errs, 0);
    }

    #[test]
    fn parallel_beats_sequential() {
        let m = Mock { delay: Duration::from_millis(40) };
        let seq = BatchRunner::new(1).run(&m, &prompts(6));
        let par = BatchRunner::new(6).run(&m, &prompts(6));
        // 6 × 40ms sequential ≈ 240ms; 6-wide ≈ 40ms → parallel must be much faster
        assert!(par.wall_ms * 2 < seq.wall_ms, "par {}ms should be < half of seq {}ms", par.wall_ms, seq.wall_ms);
        assert!(par.throughput_per_s > seq.throughput_per_s);
    }

    #[test]
    fn collects_errors_without_aborting_batch() {
        let mut ps = prompts(5);
        ps[2] = "BAD case".into();
        let r = BatchRunner::new(3).run(&Mock { delay: Duration::from_millis(1) }, &ps);
        assert_eq!(r.total, 5);
        assert_eq!(r.errs, 1);
        assert_eq!(r.oks, 4);
        assert!(r.responses[2].is_err());
        assert!(r.responses[0].is_ok());
    }

    #[test]
    fn empty_batch_is_safe() {
        let r = BatchRunner::new(4).run(&Mock { delay: Duration::from_millis(1) }, &[]);
        assert_eq!(r.total, 0);
        assert_eq!(r.throughput_per_s, 0.0);
    }
}
