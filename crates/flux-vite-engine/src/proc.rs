//! Spawn a Vite dev server as a child process + stream typed events.

use crate::events::{classify_hmr, now_ms, HmrKind, TransformStage, ViteEvent, ViteEventKind};
use crate::state::ViteState;
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, Mutex};

/// Configuration for spawning a Vite dev server.
#[derive(Debug, Clone)]
pub struct ViteConfig {
    /// Project root (where vite.config.{ts,js} lives).
    pub project_path: PathBuf,
    /// Override binary; default tries `vite` then falls back to `npx vite`.
    pub vite_bin: Option<String>,
    /// Port to bind (passed via --port). None = let Vite pick.
    pub port: Option<u16>,
    /// Extra args to pass to vite after `--port`.
    pub extra_args: Vec<String>,
    /// How many events to buffer in the broadcast channel (default 256).
    pub event_buffer: usize,
}

impl ViteConfig {
    pub fn for_project(project_path: impl Into<PathBuf>) -> Self {
        Self {
            project_path: project_path.into(),
            vite_bin: None,
            port: None,
            extra_args: Vec::new(),
            event_buffer: 256,
        }
    }

    pub fn with_port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }

    pub fn with_bin(mut self, bin: impl Into<String>) -> Self {
        self.vite_bin = Some(bin.into());
        self
    }
}

/// A running Vite dev server with a typed event stream.
pub struct ViteEngine {
    cfg: ViteConfig,
    tx: broadcast::Sender<ViteEvent>,
    state: Arc<Mutex<ViteState>>,
    child: Arc<Mutex<Option<Child>>>,
}

impl ViteEngine {
    /// Spawn Vite + start the event pump. Returns immediately; events arrive
    /// asynchronously on `subscribe()`.
    pub async fn spawn(cfg: ViteConfig) -> Result<Self> {
        let (tx, _) = broadcast::channel(cfg.event_buffer);
        let state = Arc::new(Mutex::new(ViteState::new()));

        let (bin, mut args) = resolve_bin(cfg.vite_bin.as_deref());
        if let Some(port) = cfg.port {
            args.push("--port".into());
            args.push(port.to_string());
        }
        args.extend(cfg.extra_args.iter().cloned());

        let mut cmd = Command::new(&bin);
        cmd.args(&args)
            .current_dir(&cfg.project_path)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd.spawn().with_context(|| {
            format!(
                "failed to spawn `{bin}` in {}",
                cfg.project_path.display()
            )
        })?;

        // Wire up stdout + stderr parsers.
        if let Some(out) = child.stdout.take() {
            let tx_ = tx.clone();
            let state_ = state.clone();
            tokio::spawn(pump_stream(BufReader::new(out), tx_, state_, false));
        }
        if let Some(err) = child.stderr.take() {
            let tx_ = tx.clone();
            let state_ = state.clone();
            tokio::spawn(pump_stream(BufReader::new(err), tx_, state_, true));
        }

        let child_arc = Arc::new(Mutex::new(Some(child)));

        // Wait-on-exit task → emit Exit event.
        let child_for_wait = child_arc.clone();
        let tx_for_wait = tx.clone();
        let state_for_wait = state.clone();
        tokio::spawn(async move {
            let mut guard = child_for_wait.lock().await;
            if let Some(c) = guard.as_mut() {
                let status = c.wait().await.ok();
                let code = status.and_then(|s| s.code());
                let ev = ViteEvent::now(ViteEventKind::Exit { code });
                state_for_wait.lock().await.apply(&ev);
                let _ = tx_for_wait.send(ev);
            }
            *guard = None;
        });

        // Immediate Connected event (best-effort; port may be None until vite logs it).
        let ev = ViteEvent::now(ViteEventKind::Connected {
            port: cfg.port.unwrap_or(5173),
        });
        state.lock().await.apply(&ev);
        let _ = tx.send(ev);

        Ok(Self {
            cfg,
            tx,
            state,
            child: child_arc,
        })
    }

    /// Subscribe to the event stream. Each subscriber gets all events emitted
    /// AFTER they subscribe.
    pub fn subscribe(&self) -> broadcast::Receiver<ViteEvent> {
        self.tx.subscribe()
    }

    /// Snapshot the current state. Cheap clone; safe to call frequently.
    pub async fn snapshot(&self) -> crate::state::ViteSnapshot {
        self.state.lock().await.snapshot()
    }

    /// The project this engine is attached to.
    pub fn project_path(&self) -> &std::path::Path {
        &self.cfg.project_path
    }

    /// Kill the child process. Idempotent.
    pub async fn shutdown(&self) {
        if let Some(c) = self.child.lock().await.as_mut() {
            let _ = c.kill().await;
        }
    }
}

fn resolve_bin(override_bin: Option<&str>) -> (String, Vec<String>) {
    if let Some(b) = override_bin {
        return (b.to_string(), Vec::new());
    }
    // Prefer `vite` on PATH; if it's missing, npx will resolve it.
    // We can't easily detect from inside async code at scale, so default to npx
    // (which is the install-aware path that works regardless of local bins).
    ("npx".into(), vec!["--yes".into(), "vite".into()])
}

async fn pump_stream<R: tokio::io::AsyncRead + Unpin>(
    reader: BufReader<R>,
    tx: broadcast::Sender<ViteEvent>,
    state: Arc<Mutex<ViteState>>,
    is_stderr: bool,
) {
    let mut lines = reader.lines();
    while let Ok(Some(line)) = lines.next_line().await {
        for ev in parse_line(&line, is_stderr) {
            state.lock().await.apply(&ev);
            let _ = tx.send(ev);
        }
    }
}

/// Parse a single line of Vite output into zero or more typed events.
///
/// Vite log shapes (May 2026 baseline; tolerant of minor format drift):
///   `vite v5.2.0  ready in 532 ms`
///   `[vite] hmr update /src/App.tsx`
///   `[vite] page reload /src/main.ts`
///   `2:34:21 PM [vite] hmr update /src/components/Chart.tsx`
///   `[vite:transform] /src/App.tsx 14ms`
///   `error during build: ...`
pub fn parse_line(line: &str, is_stderr: bool) -> Vec<ViteEvent> {
    let mut out = Vec::new();
    let s = strip_timestamp_prefix(line.trim());

    // hmr update
    if let Some(rest) = s.strip_prefix("[vite] hmr update ") {
        let path = rest.trim().to_string();
        let kind = classify_hmr(&path);
        out.push(ViteEvent::now(ViteEventKind::HmrUpdate { path, kind }));
        return out;
    }
    // page reload
    if let Some(rest) = s.strip_prefix("[vite] page reload ") {
        let path = rest.trim();
        out.push(ViteEvent::now(ViteEventKind::PageReload {
            path: if path.is_empty() {
                None
            } else {
                Some(path.to_string())
            },
        }));
        return out;
    }
    if let Some(_rest) = s.strip_prefix("[vite] full reload") {
        out.push(ViteEvent::now(ViteEventKind::PageReload { path: None }));
        return out;
    }
    // transform timing  [vite:transform] PATH Nms  (or  N ms)
    if let Some(rest) = s.strip_prefix("[vite:transform] ") {
        if let Some(ev) = parse_transform_line(rest) {
            out.push(ev);
            return out;
        }
    }
    // ready
    if let Some(rest) = s.strip_prefix("vite v").or_else(|| s.strip_prefix("VITE v")) {
        // Port hidden inside "ready in Nms — Local: http://localhost:PORT/"
        if let Some(port) = parse_port_from_ready(rest) {
            out.push(ViteEvent::now(ViteEventKind::Connected { port }));
            return out;
        }
    }
    if let Some(port) = parse_local_url_port(s) {
        out.push(ViteEvent::now(ViteEventKind::Connected { port }));
        return out;
    }
    // errors
    if is_stderr || s.starts_with("error ") || s.starts_with("Error:") || s.contains("✘ [ERROR]") {
        if !s.is_empty() {
            out.push(ViteEvent::now(ViteEventKind::Error {
                message: s.to_string(),
                path: None,
            }));
            return out;
        }
    }
    out
}

fn strip_timestamp_prefix(s: &str) -> &str {
    // "2:34:21 PM [vite] ..." → "[vite] ..."
    // "14:32:11 [vite] ..."   → "[vite] ..."
    if let Some(rest) = s.split_once(" [vite") {
        if rest.0.chars().all(|c| c.is_ascii_digit() || c == ':' || c == ' ' || c == 'A' || c == 'P' || c == 'M') {
            // Skip the matched timestamp prefix.
            return &s[rest.0.len() + 1..];
        }
    }
    s
}

fn parse_transform_line(rest: &str) -> Option<ViteEvent> {
    // "PATH Nms" or "PATH N ms"
    let lower = rest.to_lowercase();
    let ms_pos = lower.find(" ms").or_else(|| lower.find("ms"))?;
    let (left, _ms_part) = rest.split_at(ms_pos);
    let mut split = left.rsplitn(2, char::is_whitespace);
    let n = split.next()?.trim().parse::<u32>().ok()?;
    let path = split.next()?.trim().to_string();
    Some(ViteEvent::now(ViteEventKind::Transform {
        path,
        stage: TransformStage::Swc, // best-effort; vite's [vite:transform] is post-SWC
        ms: n,
    }))
}

fn parse_port_from_ready(_rest: &str) -> Option<u16> {
    // We rely on the "Local: http://localhost:PORT/" line for the actual port.
    None
}

fn parse_local_url_port(s: &str) -> Option<u16> {
    // "  ➜  Local:   http://localhost:5173/"
    let idx = s.find("localhost:")?;
    let after = &s[idx + "localhost:".len()..];
    let end = after.find(|c: char| !c.is_ascii_digit()).unwrap_or(after.len());
    after[..end].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hmr_update() {
        let ev = parse_line("[vite] hmr update /src/App.tsx", false);
        assert_eq!(ev.len(), 1);
        match &ev[0].kind {
            ViteEventKind::HmrUpdate { path, kind } => {
                assert_eq!(path, "/src/App.tsx");
                assert_eq!(*kind, HmrKind::Js);
            }
            _ => panic!("expected HmrUpdate"),
        }
    }

    #[test]
    fn parse_hmr_with_timestamp_prefix() {
        let ev = parse_line("2:34:21 PM [vite] hmr update /src/theme.css", false);
        assert_eq!(ev.len(), 1);
        match &ev[0].kind {
            ViteEventKind::HmrUpdate { path, kind } => {
                assert_eq!(path, "/src/theme.css");
                assert_eq!(*kind, HmrKind::Css);
            }
            _ => panic!("expected HmrUpdate"),
        }
    }

    #[test]
    fn parse_page_reload() {
        let ev = parse_line("[vite] page reload /src/main.ts", false);
        assert!(matches!(ev[0].kind, ViteEventKind::PageReload { .. }));
    }

    #[test]
    fn parse_transform_inline() {
        let ev = parse_line("[vite:transform] /src/components/Chart.tsx 14ms", false);
        assert_eq!(ev.len(), 1);
        match &ev[0].kind {
            ViteEventKind::Transform { path, ms, .. } => {
                assert_eq!(path, "/src/components/Chart.tsx");
                assert_eq!(*ms, 14);
            }
            _ => panic!("expected Transform"),
        }
    }

    #[test]
    fn parse_local_url() {
        let ev = parse_line("  ➜  Local:   http://localhost:5173/", false);
        assert_eq!(ev.len(), 1);
        assert!(matches!(ev[0].kind, ViteEventKind::Connected { port: 5173 }));
    }

    #[test]
    fn empty_line_emits_nothing() {
        assert!(parse_line("", false).is_empty());
        assert!(parse_line("  ", false).is_empty());
    }

    #[test]
    fn stderr_unrecognized_becomes_error() {
        let ev = parse_line("Failed to compile ChartView.tsx", true);
        assert_eq!(ev.len(), 1);
        assert!(matches!(ev[0].kind, ViteEventKind::Error { .. }));
    }

    #[test]
    fn config_builder_chains() {
        let cfg = ViteConfig::for_project("/tmp/test").with_port(5180).with_bin("custom-vite");
        assert_eq!(cfg.port, Some(5180));
        assert_eq!(cfg.vite_bin.as_deref(), Some("custom-vite"));
    }
}
