//! verify.rs — headless render check. Loads a built page in headless Chromium,
//! captures the rendered DOM, any console errors/warnings, and a screenshot,
//! and returns a pass/fail. This is the signal an AGENT actually needs after a
//! build/deploy — "did it render without runtime errors?" — which the live HMR
//! engine could never give (no browser = no signal).
//!
//! No npm/playwright dependency: it shells out to whatever Chromium is on the
//! box (env `CHROME_BIN`, the playwright cache, or a system chrome).

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyReport {
    /// rendered cleanly AND no console errors.
    pub ok: bool,
    /// The page produced a non-trivial DOM (React actually mounted).
    pub rendered: bool,
    pub dom_chars: usize,
    pub console_errors: Vec<String>,
    pub console_warnings: Vec<String>,
    pub screenshot: Option<PathBuf>,
    pub chrome: PathBuf,
    pub note: String,
}

/// Locate a usable Chromium: `CHROME_BIN`, then the playwright cache, then a
/// system binary. Returns None if nothing renderable is installed.
pub fn find_chrome() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CHROME_BIN") {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }
    // playwright cache: /root/.cache/ms-playwright/chromium-*/chrome-linux*/chrome
    if let Ok(rd) = std::fs::read_dir("/root/.cache/ms-playwright") {
        let mut cands: Vec<PathBuf> = rd
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("chromium"))
            .flat_map(|e| {
                ["chrome-linux64/chrome", "chrome-linux/chrome"]
                    .iter()
                    .map(|s| e.path().join(s))
                    .collect::<Vec<_>>()
            })
            .filter(|p| p.is_file())
            .collect();
        cands.sort();
        if let Some(p) = cands.pop() {
            return Some(p);
        }
    }
    for name in ["chromium", "chromium-browser", "google-chrome", "google-chrome-stable", "chrome"] {
        if let Ok(out) = std::process::Command::new("which").arg(name).output() {
            if out.status.success() {
                let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// Convenience: verify a project's built `dist/`. Serves the directory over an
/// ephemeral localhost HTTP server (NOT file:// — Chrome blocks ES-module
/// scripts from the null file:// origin, so a SPA never mounts there) and
/// renders `index.html`.
pub async fn verify_dist(project: &Path, screenshot_out: &Path) -> Result<VerifyReport> {
    let dist = project.join("dist");
    if !dist.join("index.html").is_file() {
        return Err(anyhow!("no built page at {}/index.html — run build_project first", dist.display()));
    }
    let (port, server) = serve_dir(dist).await?;
    let url = format!("http://127.0.0.1:{port}/index.html");
    let r = verify(&url, screenshot_out).await;
    server.abort();
    r
}

/// Minimal static file server on an ephemeral localhost port — just enough to
/// render a built SPA in headless Chrome. Returns the port + a handle to abort.
async fn serve_dir(dir: PathBuf) -> Result<(u16, tokio::task::JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.context("bind localhost")?;
    let port = listener.local_addr()?.port();
    let handle = tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let dir = dir.clone();
            tokio::spawn(async move {
                let _ = serve_conn(&mut sock, &dir).await;
            });
        }
    });
    Ok((port, handle))
}

async fn serve_conn(sock: &mut tokio::net::TcpStream, dir: &Path) -> Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buf = [0u8; 2048];
    let n = sock.read(&mut buf).await?;
    let req = String::from_utf8_lossy(&buf[..n]);
    let path = req
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("/")
        .split('?')
        .next()
        .unwrap_or("/");
    // sanitize: no traversal, default to index.html
    let rel = path.trim_start_matches('/').replace("..", "");
    let rel = if rel.is_empty() { "index.html".to_string() } else { rel };
    let file = dir.join(&rel);
    match tokio::fs::read(&file).await {
        Ok(body) => {
            let ct = content_type(&rel);
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            sock.write_all(head.as_bytes()).await?;
            sock.write_all(&body).await?;
        }
        Err(_) => {
            let msg = b"not found";
            let head = format!(
                "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                msg.len()
            );
            sock.write_all(head.as_bytes()).await?;
            sock.write_all(msg).await?;
        }
    }
    let _ = sock.flush().await;
    Ok(())
}

fn content_type(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

/// Render `target` (a file:// or http(s) URL) headless, capturing DOM, console
/// logs, and a screenshot.
pub async fn verify(target: &str, screenshot_out: &Path) -> Result<VerifyReport> {
    let chrome = find_chrome().ok_or_else(|| {
        anyhow!("no headless Chromium found (set CHROME_BIN, install a system chrome, or `npx playwright install chromium`)")
    })?;

    let out = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        Command::new(&chrome)
            .args([
                "--headless=new",
                "--disable-gpu",
                "--no-sandbox",
                "--hide-scrollbars",
                "--disable-dev-shm-usage",
                "--enable-logging=stderr",
                "--v=1",
                "--virtual-time-budget=6000",
                "--window-size=1280,2400",
                &format!("--screenshot={}", screenshot_out.display()),
                "--dump-dom",
                target,
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    .context("headless chrome timed out after 60s")?
    .context("failed to spawn headless chrome")?;

    let dom = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    let mut console_errors = Vec::new();
    let mut console_warnings = Vec::new();
    for line in stderr.lines() {
        if !line.contains(":CONSOLE(") && !line.contains("Uncaught") {
            continue;
        }
        let msg = line.splitn(2, ']').nth(1).unwrap_or(line).trim().to_string();
        if line.contains(":ERROR:") || line.contains("Uncaught") {
            console_errors.push(msg);
        } else if line.contains(":WARNING:") {
            console_warnings.push(msg);
        }
    }

    let dom_chars = dom.trim().len();
    // React mounted if the #root div has real children (not the empty shell).
    let root_filled = dom
        .split("id=\"root\"")
        .nth(1)
        .map(|after| after.trim_start_matches('>').trim().len() > 40)
        .unwrap_or(false);
    let rendered = dom_chars > 1000 && root_filled;
    let screenshot = screenshot_out.is_file().then(|| screenshot_out.to_path_buf());

    let ok = rendered && console_errors.is_empty();
    let note = if !rendered {
        "page DOM looks empty — React may not have mounted".into()
    } else if !console_errors.is_empty() {
        format!("{} console error(s) at runtime", console_errors.len())
    } else {
        "rendered clean".into()
    };

    Ok(VerifyReport { ok, rendered, dom_chars, console_errors, console_warnings, screenshot, chrome, note })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_chrome_or_says_so() {
        // Non-fatal: just exercise the detector. On CI without chrome it's None.
        let _ = find_chrome();
    }
}
