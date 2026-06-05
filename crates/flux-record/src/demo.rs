//! Synthesize a realistic-looking "raw" video from a Claude Code transcript
//! alone — no actual screen capture required.
//!
//! v2 design (changes vs v1):
//!   1. **Scrolling terminal** — at any time `t`, the visible terminal text
//!      is the LAST `N` formatted lines that have arrived by `t`, not the
//!      first `N`. Implemented as a series of "snapshot" drawtext calls
//!      (one every SNAPSHOT_INTERVAL seconds) that each render a multi-line
//!      string covering its window.
//!   2. **Generic dev preview** — right panel shows a recently-touched file
//!      tree, a running event counter, a test-result counter mined from
//!      tool_result bodies, and a list of git commits found in the session.
//!      No more dashboard-mockup widgets.
//!   3. **Timeline compression** — caller passes a `duration_s`; we rescale
//!      all event timestamps so the last event lands at `duration_s`,
//!      letting an N-hour transcript fit a M-minute video.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

use crate::transcript::{self, Event, EventKind};

pub struct DemoOpts {
    pub duration_s: u32,
    pub fps: u32,
    pub cwd_label: String,
}

impl Default for DemoOpts {
    fn default() -> Self {
        Self { duration_s: 480, fps: 30, cwd_label: "~/work/project".into() }
    }
}

/// Width of one snapshot window in seconds. The terminal panel re-renders at
/// this cadence — every SNAPSHOT_INTERVAL seconds we drop a new multi-line
/// drawtext showing the most recent terminal state.
const SNAPSHOT_INTERVAL: f64 = 4.0;
/// How many terminal lines fit in the panel (1080 - 60 top / 26 per line).
const TERMINAL_VISIBLE_LINES: usize = 38;
/// How many recent files to list in the preview panel.
const PREVIEW_FILES: usize = 10;
/// Number of floating particles in the preview-panel particle field.
const PARTICLE_COUNT: usize = 32;
/// Color palette for the particle field (hex literals, no leading "#").
const PARTICLE_PALETTE: &[&str] = &[
    "0x4f46e5", // indigo
    "0x06b6d4", // cyan
    "0xa855f7", // purple
    "0xf472b6", // pink
    "0x22d3ee", // sky
    "0xfbbf24", // amber
    "0x10b981", // emerald
];

pub fn synthesize(events: &[Event], out: &Path, opts: &DemoOpts) -> Result<()> {
    // Rescale to fit the requested duration.
    let scaled: Vec<Event> = transcript::compress_timeline(
        events.to_vec(),
        opts.duration_s as f64 * 0.92, // leave a small tail so the final state lingers
    );

    let snapshots = build_snapshots(&scaled, opts);
    let filter = build_filtergraph(&snapshots, opts);
    let script = write_filter_script(&filter)?;

    let dur = opts.duration_s.to_string();
    let fps = opts.fps.to_string();

    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-hide_banner",
            "-loglevel", "warning",
            "-f", "lavfi", "-i",
            &format!("color=c=0x0c0e14:size=1280x1080:rate={fps}:duration={dur}"),
            "-f", "lavfi", "-i",
            &format!("color=c=0x111827:size=640x1080:rate={fps}:duration={dur}"),
            "-f", "lavfi", "-i",
            &format!("anullsrc=channel_layout=stereo:sample_rate=44100:duration={dur}"),
            "-filter_complex_script", &script.display().to_string(),
            "-map", "[vout]",
            "-map", "2:a",
            "-c:v", "libx264",
            "-preset", "ultrafast",
            "-crf", "22",
            "-pix_fmt", "yuv420p",
            "-c:a", "aac",
            "-b:a", "128k",
            &out.display().to_string(),
        ])
        .status()
        .context("failed to spawn ffmpeg for demo synthesis")?;

    if !status.success() {
        eprintln!("flux-record: demo filter script preserved at {}", script.display());
        anyhow::bail!("ffmpeg demo synthesis exited with {status}");
    }
    // Keep the script around for debugging — small price and lets us inspect.
    eprintln!("flux-record: demo filter script at {} ({} bytes)",
        script.display(),
        script.metadata().map(|m| m.len()).unwrap_or(0)
    );
    Ok(())
}

/// State of the synthesized "session" at one snapshot point. Every field
/// except the renderer chrome is derived from real transcript data.
struct Snapshot {
    /// Video time (s) at which this snapshot starts being visible.
    t_s: f64,
    /// Window length (s).
    window_s: f64,
    /// Multi-line terminal text — the last TERMINAL_VISIBLE_LINES formatted
    /// event lines as of this snapshot.
    terminal: String,
    /// Top-N most-touched files in this session so far.
    files: String,
    /// MCP-tool / Bash-tool / Edit-tool / etc. usage histogram.
    tools: String,
    /// Recent compile_ms values mined from flux_iterate output ("Compile: N ms").
    compile: String,
    /// Aggregate counters: events, tool calls, tests, errors.
    stats: String,
    /// Detected work phase — most-frequent crate path in recent file events.
    phase: String,
    /// Conventional-commits hashes + subjects pulled from the transcript.
    commits: String,
    /// Most-recent assistant explanation snippet — what I was just saying.
    narration: String,
    /// Currently in_progress tasks (subjects) from TaskCreate/TaskUpdate trail.
    tasks: String,
}

fn build_snapshots(events: &[Event], opts: &DemoOpts) -> Vec<Snapshot> {
    let mut snapshots: Vec<Snapshot> = Vec::new();

    let mut all_lines: Vec<String> = Vec::new();
    all_lines.push(format!("{} $ ", opts.cwd_label));

    let mut recent_files: Vec<String> = Vec::new();
    let mut file_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut tool_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut total_tool_calls: usize = 0;
    let mut tests_passed: usize = 0;
    let mut tests_failed: usize = 0;
    let mut errors: usize = 0;
    let mut commits: Vec<String> = Vec::new();
    let mut compile_ms: Vec<u64> = Vec::new();
    let mut webhooks_fired: u32 = 0;
    let mut last_webhook_op: String = String::new();
    let mut mcp_calls: u32 = 0;
    // Tasks: (status, subject) keyed by task numeric id. Lets us project the
    // current set of in_progress tasks at any snapshot.
    let mut tasks: std::collections::BTreeMap<usize, (String, String)> = std::collections::BTreeMap::new();
    let mut next_task_id: usize = 1;
    let mut last_narration: String = String::new();
    let mut next_t = 0.0;

    let total_s = opts.duration_s as f64;
    let mut event_idx = 0;

    while next_t <= total_s {
        while event_idx < events.len() && events[event_idx].t_s <= next_t {
            let e = &events[event_idx];
            for l in format_event(e) {
                all_lines.push(l);
            }
            if let Some(f) = extract_file(e) {
                *file_counts.entry(f.clone()).or_insert(0) += 1;
                recent_files.retain(|x| x != &f);
                recent_files.insert(0, f);
                if recent_files.len() > PREVIEW_FILES { recent_files.truncate(PREVIEW_FILES); }
            }
            if let EventKind::ToolUse(name) = &e.kind {
                *tool_counts.entry(name.clone()).or_insert(0) += 1;
                total_tool_calls += 1;

                // MCP-side activity: any tool starting with `mcp__` is a call
                // to an external MCP server. Webhook subset is anything
                // matching `*webhook*`.
                if name.starts_with("mcp__") {
                    mcp_calls += 1;
                }
                if name.contains("webhook") || name.contains("Webhook") {
                    webhooks_fired += 1;
                    // Extract a short op tag: register / test / list / trigger.
                    let lower = name.to_lowercase();
                    last_webhook_op = if lower.contains("register") { "register".into() }
                        else if lower.contains("test") { "test".into() }
                        else if lower.contains("list") { "list".into() }
                        else if lower.contains("trigger") { "trigger".into() }
                        else { "fired".into() };
                }

                // Mine TaskCreate / TaskUpdate from the tool input JSON. This
                // is free — the structured input is already in the transcript.
                if name == "TaskCreate" || name == "TaskUpdate" {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&e.body) {
                        if name == "TaskCreate" {
                            let subject = v.get("subject").and_then(|x| x.as_str()).unwrap_or("").to_string();
                            if !subject.is_empty() {
                                tasks.insert(next_task_id, ("pending".to_string(), subject));
                                next_task_id += 1;
                            }
                        } else {
                            let id = v.get("taskId").and_then(|x| x.as_str()).and_then(|s| s.parse::<usize>().ok());
                            let status = v.get("status").and_then(|x| x.as_str()).unwrap_or("").to_string();
                            if let (Some(id), false) = (id, status.is_empty()) {
                                if let Some(t) = tasks.get_mut(&id) {
                                    t.0 = status;
                                }
                            }
                        }
                    }
                }
            }
            if matches!(e.kind, EventKind::AssistantText) {
                let snippet = clip(&e.label, 90);
                if !snippet.trim().is_empty() {
                    last_narration = snippet;
                }
            }
            if e.is_error() {
                errors += 1;
            }
            let (p, f) = scrape_test_counts(&e.body);
            tests_passed += p;
            tests_failed += f;
            for ms in scrape_compile_ms(&e.body) {
                compile_ms.push(ms);
                if compile_ms.len() > 8 { compile_ms.remove(0); }
            }
            for c in scrape_commits(&e.body) {
                if !commits.iter().any(|x| x == &c) {
                    commits.push(c);
                    if commits.len() > 4 { commits.remove(0); }
                }
            }
            event_idx += 1;
        }

        let head = all_lines.len().saturating_sub(TERMINAL_VISIBLE_LINES);
        let visible_terminal = all_lines[head..].join("\n");

        // Top-touched files (rather than just most-recent) — more informative.
        let mut top_files: Vec<(&String, &usize)> = file_counts.iter().collect();
        top_files.sort_by(|a, b| b.1.cmp(a.1));
        let files_text = if top_files.is_empty() {
            "  (no files touched yet)".to_string()
        } else {
            top_files.iter().take(8).map(|(f, n)| {
                let short = shorten_path(f, 44);
                format!("  {:>3}x  {short}", n)
            }).collect::<Vec<_>>().join("\n")
        };

        // MCP tool histogram, top-6 by count.
        let mut tool_rows: Vec<(&String, &usize)> = tool_counts.iter().collect();
        tool_rows.sort_by(|a, b| b.1.cmp(a.1));
        let tools_text = if tool_rows.is_empty() {
            "  (no tools yet)".to_string()
        } else {
            tool_rows.iter().take(6).map(|(name, n)| {
                let bar = histogram_bar(**n, *tool_rows[0].1, 12);
                format!("  {:<10} {:>3}  {bar}", clip(name, 10), n)
            }).collect::<Vec<_>>().join("\n")
        };

        // Last compile times as a compact sparkline.
        let compile_text = if compile_ms.is_empty() {
            "  (no builds yet)".to_string()
        } else {
            let spark = sparkline(&compile_ms);
            let last = compile_ms.last().copied().unwrap_or(0);
            format!("  {spark}\n  last: {}ms   builds: {}", last, compile_ms.len())
        };

        // Phase = most-touched crate name in the file_counts table.
        let phase_text = detect_phase(&file_counts);

        // Include MCP + webhook lines so viewers see external-service activity.
        let webhook_tail = if last_webhook_op.is_empty() {
            String::new()
        } else {
            format!(" (last: {})", last_webhook_op)
        };
        let stats_text = format!(
            "  events: {}   errors: {}\n  tool calls: {}   mcp: {}\n  webhooks: {}{}\n  tests: {} ✓  {} ✗",
            event_idx, errors, total_tool_calls, mcp_calls, webhooks_fired, webhook_tail, tests_passed, tests_failed
        );

        let commits_text = if commits.is_empty() {
            "  (no commits yet)".to_string()
        } else {
            commits.iter().rev().map(|c| format!("  {c}")).collect::<Vec<_>>().join("\n")
        };

        // Active tasks: in_progress first, then most recent few pending.
        let mut in_progress: Vec<&(String, String)> = tasks.values()
            .filter(|(s, _)| s == "in_progress")
            .collect();
        let pending_count = tasks.values().filter(|(s, _)| s == "pending").count();
        let completed_count = tasks.values().filter(|(s, _)| s == "completed").count();
        let tasks_text = if in_progress.is_empty() && tasks.is_empty() {
            "  (no tasks yet)".to_string()
        } else {
            let mut lines: Vec<String> = Vec::new();
            in_progress.truncate(3);
            for (_, subj) in in_progress {
                lines.push(format!("  ▶ {}", clip(subj, 46)));
            }
            lines.push(format!("  · {pending_count} pending  {completed_count} done"));
            lines.join("\n")
        };

        let narration_text = if last_narration.is_empty() {
            "  (warming up…)".to_string()
        } else {
            // Wrap to ~52 chars per line, max 3 lines for vertical space.
            wrap_lines(&last_narration, 52, 3, "  ")
        };

        snapshots.push(Snapshot {
            t_s: next_t,
            window_s: SNAPSHOT_INTERVAL,
            terminal: visible_terminal,
            files: files_text,
            tools: tools_text,
            compile: compile_text,
            stats: stats_text,
            phase: phase_text,
            commits: commits_text,
            narration: narration_text,
            tasks: tasks_text,
        });
        next_t += SNAPSHOT_INTERVAL;
    }
    snapshots
}

/// Greedy word-wrap with a max number of lines, each indented.
fn wrap_lines(text: &str, width: usize, max_lines: usize, indent: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for w in words {
        if cur.is_empty() {
            cur.push_str(w);
        } else if cur.chars().count() + 1 + w.chars().count() <= width {
            cur.push(' ');
            cur.push_str(w);
        } else {
            lines.push(format!("{indent}{cur}"));
            if lines.len() >= max_lines {
                cur.clear();
                break;
            }
            cur = w.to_string();
        }
    }
    if !cur.is_empty() && lines.len() < max_lines {
        lines.push(format!("{indent}{cur}"));
    }
    if lines.len() == max_lines {
        // Add ellipsis if there's overflow content
        if let Some(last) = lines.last_mut() {
            if last.chars().count() >= width - 1 {
                last.push('…');
            }
        }
    }
    lines.join("\n")
}

/// Compress a long file path to a tail of `max` chars (with "…/" prefix when truncated).
fn shorten_path(path: &str, max: usize) -> String {
    if path.chars().count() <= max {
        return path.to_string();
    }
    let chars: Vec<char> = path.chars().collect();
    let start = chars.len() - max + 2;
    let tail: String = chars[start..].iter().collect();
    format!("…/{tail}")
}

/// Render an integer value as a Unicode block bar of `width` characters.
fn histogram_bar(value: usize, max: usize, width: usize) -> String {
    if max == 0 || width == 0 {
        return String::new();
    }
    let filled = ((value as f64 / max as f64) * width as f64).round() as usize;
    let mut s = String::with_capacity(width);
    for _ in 0..filled { s.push('█'); }
    for _ in filled..width { s.push('░'); }
    s
}

/// Render a u64 sequence as an 8-step block sparkline.
fn sparkline(values: &[u64]) -> String {
    if values.is_empty() { return String::new(); }
    let max = *values.iter().max().unwrap_or(&1).max(&1);
    let blocks = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    values.iter().map(|v| {
        let idx = ((*v as f64 / max as f64) * (blocks.len() as f64 - 1.0)).round() as usize;
        blocks[idx.min(blocks.len() - 1)]
    }).collect()
}

/// Try to identify what crate the agent is currently working on by counting
/// the crate name component of each file path in the touched-files table.
fn detect_phase(file_counts: &std::collections::HashMap<String, usize>) -> String {
    let mut crate_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (path, count) in file_counts {
        if let Some(c) = path.split('/').skip_while(|s| *s != "crates").nth(1) {
            *crate_counts.entry(c.to_string()).or_insert(0) += count;
        }
    }
    if let Some((name, _)) = crate_counts.iter().max_by_key(|(_, n)| **n) {
        format!("  → {name}")
    } else {
        "  → (warming up…)".to_string()
    }
}

/// Scrape compile_ms values from flux_iterate / cargo output. Matches strings
/// like "Compile: 1234ms" or "Finished `dev` profile [...] target(s) in 1.23s"
/// or "Build time: 4908ms".
fn scrape_compile_ms(body: &str) -> Vec<u64> {
    let mut out = Vec::new();
    for line in body.lines() {
        // Pattern: "Compile: NNNms" or "Build time: NNNms" or "complete in NNNms"
        for marker in &["Compile:", "Build time:", "complete in", "in"] {
            if let Some(idx) = line.find(marker) {
                let after = &line[idx + marker.len()..];
                if let Some(num_end) = after.find("ms") {
                    let digits: String = after[..num_end].chars()
                        .filter(|c| c.is_ascii_digit())
                        .collect();
                    if let Ok(ms) = digits.parse::<u64>() {
                        if ms > 0 && ms < 10_000_000 {
                            out.push(ms);
                            break; // one ms per line is enough
                        }
                    }
                }
            }
        }
    }
    out
}

fn format_event(e: &Event) -> Vec<String> {
    match &e.kind {
        EventKind::User => vec![format!("> {}", clip(&e.label, 78))],
        EventKind::AssistantText => vec![format!("* {}", clip(&e.label, 78))],
        EventKind::ToolUse(name) => {
            let glyph = match name.as_str() {
                "Bash" => "$",
                "Read" => "R",
                "Edit" | "NotebookEdit" => "E",
                "Write" => "W",
                "Grep" => "?",
                "Glob" => "*",
                "WebFetch" | "WebSearch" => "@",
                "Task" | "Agent" => "A",
                "HMR" => "~",
                "DOM" => "D",
                "ROUTE" => ">",
                _ => "-",
            };
            vec![format!("{glyph} {}: {}", name, clip(&e.label, 65))]
        }
        EventKind::ToolResult { is_error: false } => Vec::new(),
        EventKind::ToolResult { is_error: true } => {
            vec![format!("X ERROR: {}", clip(&e.label, 65))]
        }
        EventKind::System => Vec::new(),
    }
}

fn extract_file(e: &Event) -> Option<String> {
    let name = e.tool_name()?;
    if !matches!(name, "Read" | "Edit" | "Write" | "NotebookEdit") {
        return None;
    }
    // The label for Read/Edit/Write is the basename. To make the tree useful
    // (and not show many "lib.rs" entries), prefer the full input.file_path
    // from the body if we can find it.
    if let Some(line) = e.body.lines().find(|l| l.contains("file_path")) {
        if let Some(start) = line.find(':') {
            let after = &line[start+1..];
            let path = after.trim().trim_matches(|c: char| c == '"' || c == ',' || c.is_whitespace());
            if !path.is_empty() && path.len() < 120 {
                return Some(path.to_string());
            }
        }
    }
    Some(e.label.clone())
}

fn scrape_test_counts(body: &str) -> (usize, usize) {
    // Match patterns like "13 passed; 0 failed" or "test result: ok. 25 passed; 1 failed".
    let mut p = 0; let mut f = 0;
    for line in body.lines() {
        for cap in line.split_whitespace().collect::<Vec<_>>().windows(2) {
            if cap[1].starts_with("passed") {
                if let Ok(n) = cap[0].trim_matches(|c: char| !c.is_ascii_digit()).parse::<usize>() {
                    p = p.max(n);
                }
            }
            if cap[1].starts_with("failed") {
                if let Ok(n) = cap[0].trim_matches(|c: char| !c.is_ascii_digit()).parse::<usize>() {
                    f = f.max(n);
                }
            }
        }
    }
    (p, f)
}

fn scrape_commits(body: &str) -> Vec<String> {
    // Match a 7-char hex commit hash followed by a space and a message that
    // looks like a git subject line. Require at least one a–f character so
    // we don't match pure-decimal PIDs and the message must contain a
    // conventional-commits-style prefix (feat/fix/chore/refactor/test/docs).
    let mut out = Vec::new();
    for line in body.lines() {
        let line = line.trim_start_matches(|c: char| c == '[' || c == ']' || c.is_whitespace());
        let first: Vec<&str> = line.splitn(2, ' ').collect();
        if first.len() != 2 || first[0].len() != 7 {
            continue;
        }
        let hash = first[0];
        if !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        if !hash.chars().any(|c| matches!(c, 'a'..='f')) {
            continue; // pure decimal — almost certainly not a commit
        }
        let msg = first[1].trim_start();
        let conventional = ["feat", "fix", "chore", "refactor", "test", "docs", "build", "perf", "ci", "style"];
        if !conventional.iter().any(|p| msg.starts_with(p)) {
            continue;
        }
        out.push(format!("{hash} {}", clip(msg, 48)));
    }
    out
}

fn clip(s: &str, max: usize) -> String {
    let s = s.replace('\n', " ").replace('\r', " ");
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() <= max {
        s
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

fn build_filtergraph(snapshots: &[Snapshot], opts: &DemoOpts) -> String {
    let mut g = String::new();

    // ── LEFT PANEL: terminal ──
    g.push_str("[0:v]");
    // Terminal panel header. The "● ● ●" dots are decorative window-chrome
    // but the cwd label is the real workspace path the user passed in.
    g.push_str(&format!(
        "drawbox=x=0:y=0:w=iw:h=44:color=0x1e293b@0.95:t=fill\
         ,drawtext=text='● ● ●':fontsize=22:fontcolor=0xfbbf24:x=18:y=12\
         ,drawtext=text='{} — claude code session (synthesized from transcript)':fontsize=18:fontcolor=0x94a3b8:x=80:y=14",
        esc(&opts.cwd_label)
    ));

    for s in snapshots {
        let enable = esce(&format!("between(t,{},{})", s.t_s, s.t_s + s.window_s));
        let term_text = esc(&s.terminal);
        g.push_str(&format!(
            ",drawtext=text='{term_text}':fontsize=18:\
             fontfile=/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf:\
             fontcolor=0xe2e8f0:x=24:y=60:line_spacing=8:\
             enable='{enable}'"
        ));
    }
    // Blinking caret at fixed prompt position.
    let blink = esce("lt(mod(t,1),0.5)");
    g.push_str(&format!(
        ",drawtext=text='_':fontsize=18:fontcolor=0xe2e8f0:\
         fontfile=/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf:\
         x=24:y=1040:enable='{blink}'"
    ));
    g.push_str("[term]");

    // ── RIGHT PANEL: telemetry mined from the transcript ──
    // The data here is all real (extracted from the session JSONL); the
    // panel chrome itself is decorative. Header reflects that honestly —
    // we are NOT showing a live browser or any GUI.
    g.push_str(";[1:v]");
    g.push_str(
        "drawbox=x=0:y=0:w=iw:h=44:color=0x1e293b@0.95:t=fill\
         ,drawtext=text='session telemetry':fontsize=20:fontcolor=0x94a3b8:x=18:y=12\
         ,drawtext=text='derived from ~/.claude/projects/.../*.jsonl':fontsize=11:fontcolor=0x64748b:x=18:y=32",
    );
    // Particle field — drawbox at time-varying positions. Each particle has a
    // different phase + amplitude so the field looks organic, not synchronised.
    // Particles use the ffmpeg expression evaluator (commas are escaped, single
    // quotes wrap the expressions).
    // Small decorative element — honest about what it is. A tiny Keplerian
    // orbit (8 particles) at the top, labeled so viewers know it represents
    // nothing in the data. Tightly bounded so it doesn't dominate the panel.
    let bold = "fontfile=/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf";
    let cx_c = 320.0_f64;
    let cy_c = 130.0_f64;
    let g_units = 1.0_f64;
    let m_center = 80000.0_f64;
    for p in 0..8 {
        let r = 32.0 + (p % 3) as f64 * 8.0;
        let omega = (g_units * m_center / r.powi(3)).sqrt();
        let phase = (p as f64) * 0.78;
        let color = PARTICLE_PALETTE[p % PARTICLE_PALETTE.len()];
        let x_expr = esce(&format!("{cx_c}+{r}*cos({omega}*t+{phase})"));
        let y_expr = esce(&format!("{cy_c}+{r}*sin({omega}*t+{phase})"));
        g.push_str(&format!(
            ",drawbox=x='{x_expr}':y='{y_expr}':w=5:h=5:color={color}@0.75:t=fill"
        ));
    }
    let pulse_alpha = esce("0.55+0.35*sin(2*t)");
    g.push_str(&format!(
        ",drawbox=x=315:y=125:w=10:h=10:color=0xfbbf24@1:t=fill\
         ,drawtext=text='● decorative — kepler orbit':fontsize=11:fontcolor=0x475569:x=24:y=190:alpha='{pulse_alpha}'"
    ));
    // Section headers (static). Right panel is 640x1080; everything below
    // the particle field (y > 220) is data panels.
    let bold = "fontfile=/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf";
    let mono = "fontfile=/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf";

    // Layout: particles 44..270, then 8 data panels tightly stacked through
    // 1060. Headers are 14pt amber-bold; body fonts vary per panel.
    g.push_str(&format!(",drawtext=text='NOW':fontsize=14:fontcolor=0xfbbf24:{bold}:x=24:y=280"));
    g.push_str(&format!(",drawtext=text='RUNNING TASKS':fontsize=14:fontcolor=0xfbbf24:{bold}:x=24:y=375"));
    g.push_str(&format!(",drawtext=text='PHASE':fontsize=14:fontcolor=0xfbbf24:{bold}:x=24:y=475"));
    g.push_str(&format!(",drawtext=text='TOP FILES':fontsize=14:fontcolor=0xfbbf24:{bold}:x=24:y=525"));
    g.push_str(&format!(",drawtext=text='MCP TOOLS':fontsize=14:fontcolor=0xfbbf24:{bold}:x=24:y=655"));
    g.push_str(&format!(",drawtext=text='COMPILE TIMES':fontsize=14:fontcolor=0xfbbf24:{bold}:x=24:y=780"));
    g.push_str(&format!(",drawtext=text='STATS':fontsize=14:fontcolor=0xfbbf24:{bold}:x=24:y=860"));
    g.push_str(&format!(",drawtext=text='COMMITS':fontsize=14:fontcolor=0xfbbf24:{bold}:x=24:y=955"));

    // Dividers between sections.
    for y in [270, 365, 465, 515, 645, 770, 850, 945] {
        g.push_str(&format!(",drawbox=x=24:y={y}:w=iw-48:h=1:color=0x334155:t=fill"));
    }

    for s in snapshots {
        let enable = esce(&format!("between(t,{},{})", s.t_s, s.t_s + s.window_s));
        let narration = esc(&s.narration);
        let tasks = esc(&s.tasks);
        let phase = esc(&s.phase);
        let files = esc(&s.files);
        let tools = esc(&s.tools);
        let compile = esc(&s.compile);
        let stats = esc(&s.stats);
        let commits = esc(&s.commits);

        // NOW — recent assistant narration (free explainer text mined from
        // what Claude actually said in the session — no LLM call needed).
        g.push_str(&format!(
            ",drawtext=text='{narration}':fontsize=15:{mono}:\
             fontcolor=0xfde68a:x=24:y=300:line_spacing=4:enable='{enable}'"
        ));
        // RUNNING TASKS — derived from TaskCreate/TaskUpdate trail.
        g.push_str(&format!(
            ",drawtext=text='{tasks}':fontsize=13:{mono}:\
             fontcolor=0xfca5a5:x=24:y=395:line_spacing=4:enable='{enable}'"
        ));
        g.push_str(&format!(
            ",drawtext=text='{phase}':fontsize=17:{bold}:\
             fontcolor=0xfacc15:x=24:y=495:enable='{enable}'"
        ));
        g.push_str(&format!(
            ",drawtext=text='{files}':fontsize=12:{mono}:\
             fontcolor=0xc7d2fe:x=24:y=545:line_spacing=3:enable='{enable}'"
        ));
        g.push_str(&format!(
            ",drawtext=text='{tools}':fontsize=12:{mono}:\
             fontcolor=0x93c5fd:x=24:y=675:line_spacing=3:enable='{enable}'"
        ));
        g.push_str(&format!(
            ",drawtext=text='{compile}':fontsize=16:{mono}:\
             fontcolor=0xa7f3d0:x=24:y=800:line_spacing=3:enable='{enable}'"
        ));
        g.push_str(&format!(
            ",drawtext=text='{stats}':fontsize=13:{mono}:\
             fontcolor=0x86efac:x=24:y=880:line_spacing=3:enable='{enable}'"
        ));
        g.push_str(&format!(
            ",drawtext=text='{commits}':fontsize=11:{mono}:\
             fontcolor=0xfb7185:x=24:y=975:line_spacing=3:enable='{enable}'"
        ));
    }
    g.push_str("[prev]");

    // Composite + accent divider.
    g.push_str(";[term][prev]hstack=inputs=2[stack]");
    g.push_str(";[stack]drawbox=x=1279:y=0:w=2:h=ih:color=0x4f46e5@0.6:t=fill[vout]");
    g
}

fn esc(s: &str) -> String {
    // `%` triggers drawtext's `%{name}` expansion and our `\%` escape ends up
    // unescaped back into `%`, which still trips the parser ("Stray %").
    // Substitute the fullwidth percent (U+FF05) — visually identical, no
    // parser confusion. Same trick we use for apostrophes.
    s.replace('\\', "\\\\")
        .replace('\'', "\u{2019}")
        .replace('%', "\u{FF05}")
        .replace(':', "\\:")
        .replace(',', "\\,")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('=', "\\=")
        .replace(';', "\\;")
        // Curly braces also delimit expansion; rare in source text but safe to map.
        .replace('{', "\u{FF5B}")
        .replace('}', "\u{FF5D}")
}

fn esce(s: &str) -> String {
    s.replace(',', "\\,")
}

fn write_filter_script(filter: &str) -> Result<std::path::PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "flux-record-demo-{}.txt",
        std::process::id()
    ));
    std::fs::write(&path, filter).context("writing demo filtergraph script")?;
    Ok(path)
}
