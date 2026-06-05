//! PTY-based terminal recording + replay.
//!
//! `pty-rec` wraps the user's shell with `script -t` (util-linux) which is
//! always present on Linux and produces two files:
//!   * `typescript` — raw bytes of the session (stdout, stdin echo, ANSI codes)
//!   * `timing` — one `<delta_t> <bytes_written>` line per write event
//!
//! `pty-render` reads those two files, runs a tiny VT100-ish state machine to
//! materialize a per-line view of the terminal at each event boundary, and
//! emits a video by passing the resulting per-line drawtext chain to ffmpeg.
//!
//! Why not asciinema/agg? Because util-linux's `script` is on every Linux box
//! (Epsilon included), and we'd rather not require an apt-get install before
//! `flux-record pty-rec` works.

use anyhow::{Context, Result};
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct PtyOpts {
    pub shell: String,
    pub typescript: PathBuf,
    pub timing: PathBuf,
}

impl Default for PtyOpts {
    fn default() -> Self {
        Self {
            shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into()),
            typescript: PathBuf::from("typescript"),
            timing: PathBuf::from("timing"),
        }
    }
}

/// Spawn `script -t` wrapping the user's shell. Blocks until the shell exits.
/// On exit, `typescript` and `timing` are written to the paths in `opts`.
pub fn record(opts: &PtyOpts) -> Result<()> {
    // util-linux 2.34+ syntax. The legacy `script -t timing typescript` form
    // also works on older releases, so prefer it for portability.
    //
    //   script --quiet --timing=<file> <typescript> --command "<shell>"
    //
    // We use the BSD-compatible form: `script -t 2>timing -q typescript -c <shell>`.
    let status = Command::new("script")
        .args([
            "-q",
            "-t",
            "-c", &opts.shell,
            &opts.typescript.display().to_string(),
        ])
        .env("SCRIPT_TIMING", opts.timing.display().to_string())
        .status()
        .context("failed to spawn `script` — install with `apt-get install bsdutils` (usually preinstalled)")?;

    // Modern `script` writes the timing to stderr when `-t` is given without a
    // file arg. We instead use the newer --log-timing/--log-out form which is
    // explicit about both paths. Fall back to the older form if that fails.
    if !status.success() {
        // Retry with newer util-linux long-form.
        let s2 = Command::new("script")
            .args([
                "--quiet",
                "--log-timing", &opts.timing.display().to_string(),
                "--log-out", &opts.typescript.display().to_string(),
                "--command", &opts.shell,
            ])
            .status()
            .context("retry with `script --log-timing` also failed")?;
        if !s2.success() {
            anyhow::bail!("`script` exited with {s2}");
        }
    }
    Ok(())
}

/// Frame the recorded session into discrete (t_s, screen_text) snapshots.
/// Returns a vector of (timestamp_seconds, accumulated_terminal_text).
pub struct Frame {
    pub t_s: f64,
    pub text: String,
}

/// Build a list of frames from a `script -t` typescript + timing pair.
///
/// Each timing line is `<delta_seconds> <bytes>`. We read `bytes` from the
/// typescript and apply them via a minimal VT100 state machine, then emit a
/// frame at the cumulative timestamp. Frames at very close timestamps are
/// merged so the output isn't gigantic.
pub fn frame(typescript: &Path, timing: &Path, fps: u32) -> Result<Vec<Frame>> {
    let timing_file = File::open(timing).with_context(|| format!("opening {}", timing.display()))?;
    let mut ts_file = File::open(typescript).with_context(|| format!("opening {}", typescript.display()))?;

    let mut term = TerminalState::new(80, 40);
    let mut t = 0.0_f64;
    let mut frames: Vec<Frame> = Vec::new();
    let min_dt = 1.0 / fps as f64;
    let mut last_emit_t = -1.0_f64;

    for line in BufReader::new(timing_file).lines() {
        let line = line?;
        let mut parts = line.split_whitespace();
        let dt: f64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        let nbytes: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);

        t += dt;

        if nbytes > 0 {
            let mut buf = vec![0u8; nbytes];
            ts_file.read_exact(&mut buf).context("reading typescript bytes")?;
            term.feed(&buf);
        }

        if t - last_emit_t >= min_dt {
            frames.push(Frame { t_s: t, text: term.render() });
            last_emit_t = t;
        }
    }

    if frames.is_empty() {
        frames.push(Frame { t_s: 0.0, text: term.render() });
    }
    Ok(frames)
}

/// Render frames into an mkv suitable as a `flux-record render --raw` input.
/// Each frame becomes a static drawtext that's enabled across [frame.t,
/// next_frame.t) so the result plays back like a real terminal recording.
pub fn render(frames: &[Frame], out: &Path, opts: &RenderOpts) -> Result<()> {
    let filter = build_filtergraph(frames, opts);
    let script = write_filter_script(&filter)?;

    let last_t = frames.last().map(|f| f.t_s).unwrap_or(60.0);
    let dur = (last_t + 2.0).ceil() as u32;

    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-hide_banner",
            "-loglevel", "warning",
            "-f", "lavfi", "-i",
            &format!("color=c=0x0c0e14:size={w}x{h}:rate={fps}:duration={dur}",
                w = opts.width, h = opts.height, fps = opts.fps),
            "-f", "lavfi", "-i",
            &format!("anullsrc=channel_layout=stereo:sample_rate=44100:duration={dur}"),
            "-filter_complex_script", &script.display().to_string(),
            "-map", "[vout]",
            "-map", "1:a",
            "-c:v", "libx264",
            "-preset", "veryfast",
            "-crf", "22",
            "-pix_fmt", "yuv420p",
            "-c:a", "aac",
            "-b:a", "128k",
            &out.display().to_string(),
        ])
        .status()
        .context("failed to spawn ffmpeg for pty render")?;

    if status.success() {
        let _ = std::fs::remove_file(&script);
    } else {
        eprintln!("flux-record: pty filter script preserved at {}", script.display());
        anyhow::bail!("ffmpeg pty render exited with {status}");
    }
    Ok(())
}

pub struct RenderOpts {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

impl Default for RenderOpts {
    fn default() -> Self {
        Self { width: 1920, height: 1080, fps: 30 }
    }
}

fn build_filtergraph(frames: &[Frame], opts: &RenderOpts) -> String {
    let mut g = String::from("[0:v]");
    // Window chrome — first filter after [0:v], NO leading comma.
    g.push_str(
        "drawbox=x=0:y=0:w=iw:h=44:color=0x1e293b@0.95:t=fill\
         ,drawtext=text='● ● ●':fontsize=22:fontcolor=0xfbbf24:x=18:y=12\
         ,drawtext=text='claude-code session':fontsize=22:fontcolor=0x94a3b8:x=80:y=12",
    );

    // Each frame becomes one big drawtext block enabled over its window.
    let next_starts: Vec<f64> = frames.iter().skip(1).map(|f| f.t_s).collect();
    let last_end = frames.last().map(|f| f.t_s + 2.0).unwrap_or(0.0);

    for (i, f) in frames.iter().enumerate() {
        let start = f.t_s;
        let end = next_starts.get(i).copied().unwrap_or(last_end);
        let enable = esce(&format!("between(t,{start},{end})"));
        let text = esc(&f.text);
        g.push_str(&format!(
            ",drawtext=text='{text}':fontsize=18:\
             fontfile=/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf:\
             fontcolor=0xe2e8f0:x=24:y=60:line_spacing=4:\
             enable='{enable}'"
        ));
    }
    let _ = opts;
    g.push_str("[vout]");
    g
}

// ── tiny VT100 state machine ──

struct TerminalState {
    cols: usize,
    rows: usize,
    cur_row: usize,
    cur_col: usize,
    buf: Vec<Vec<char>>,
}

impl TerminalState {
    fn new(cols: usize, rows: usize) -> Self {
        Self {
            cols,
            rows,
            cur_row: 0,
            cur_col: 0,
            buf: vec![vec![' '; cols]; rows],
        }
    }

    /// Apply a chunk of bytes. Handles \n, \r, \b, and skips CSI escape
    /// sequences (`ESC [ ... letter`) so colors don't leak into the rendered
    /// text. Everything else is laid down at the cursor.
    fn feed(&mut self, bytes: &[u8]) {
        let mut i = 0;
        let s = String::from_utf8_lossy(bytes);
        let chars: Vec<char> = s.chars().collect();
        while i < chars.len() {
            let c = chars[i];
            match c {
                '\n' => {
                    self.cur_col = 0;
                    if self.cur_row + 1 >= self.rows {
                        self.scroll_up();
                    } else {
                        self.cur_row += 1;
                    }
                }
                '\r' => self.cur_col = 0,
                '\x08' => self.cur_col = self.cur_col.saturating_sub(1),
                '\x1b' => {
                    // Skip CSI sequences: ESC [ ... <letter>
                    if i + 1 < chars.len() && chars[i + 1] == '[' {
                        i += 2;
                        while i < chars.len() && !matches!(chars[i], 'a'..='z' | 'A'..='Z' | '~') {
                            i += 1;
                        }
                    } else {
                        // Other ESC sequences: skip one more char.
                        i += 1;
                    }
                }
                c if c.is_control() => {}
                c => {
                    if self.cur_col >= self.cols {
                        self.cur_col = 0;
                        if self.cur_row + 1 >= self.rows {
                            self.scroll_up();
                        } else {
                            self.cur_row += 1;
                        }
                    }
                    self.buf[self.cur_row][self.cur_col] = c;
                    self.cur_col += 1;
                }
            }
            i += 1;
        }
    }

    fn scroll_up(&mut self) {
        for r in 0..self.rows - 1 {
            self.buf[r] = self.buf[r + 1].clone();
        }
        self.buf[self.rows - 1] = vec![' '; self.cols];
    }

    fn render(&self) -> String {
        let mut out = String::new();
        for row in &self.buf {
            let s: String = row.iter().collect();
            out.push_str(s.trim_end());
            out.push('\n');
        }
        out.trim_end().to_string()
    }
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\'', "\u{2019}")
        .replace(':', "\\:")
        .replace('%', "\\%")
        .replace(',', "\\,")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('=', "\\=")
        .replace(';', "\\;")
}

fn esce(s: &str) -> String {
    s.replace(',', "\\,")
}

fn write_filter_script(filter: &str) -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "flux-record-pty-{}.txt",
        std::process::id()
    ));
    std::fs::write(&path, filter).context("writing pty filtergraph script")?;
    Ok(path)
}
