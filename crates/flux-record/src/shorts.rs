//! Highlight-reel auto-clipper. Detects "interesting moments" in the event
//! stream and renders them as 9:16 vertical clips for YouTube Shorts / TikTok.
//!
//! Heuristics for what counts as interesting:
//!   - any error in a tool result (with a pre-roll + post-roll window),
//!   - a dense burst of tool calls (≥ 3 within 8s),
//!   - the moment the FIRST Edit/Write happens after an error (the fix),
//!   - explicit user prompts longer than 80 chars (questions = drama).
//!
//! Clips are scored, deduped (no overlapping clips), trimmed to max_seconds,
//! and capped at max_clips.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

use crate::transcript::{Event, EventKind};

pub struct Clip {
    pub start_s: f64,
    pub end_s: f64,
    pub caption: String,
    pub score: f64,
}

pub fn pick(events: &[Event], max_seconds: u32, max_clips: u32) -> Vec<Clip> {
    let mut candidates: Vec<Clip> = Vec::new();

    let pre = 3.0_f64;
    let post = 4.0_f64;

    for (i, e) in events.iter().enumerate() {
        match &e.kind {
            EventKind::ToolResult { is_error: true } => {
                candidates.push(Clip {
                    start_s: (e.t_s - pre).max(0.0),
                    end_s: e.t_s + post,
                    caption: format!("ERROR: {}", e.label),
                    score: 10.0,
                });
            }
            EventKind::User if e.label.chars().count() > 80 => {
                candidates.push(Clip {
                    start_s: (e.t_s - 0.5).max(0.0),
                    end_s: e.t_s + post,
                    caption: format!("Prompt: {}", e.label),
                    score: 4.0,
                });
            }
            EventKind::ToolUse(_) => {
                // Look-ahead: dense burst of 3+ tool_use within 8s.
                let mut burst_end = e.t_s;
                let mut count = 1;
                for next in events.iter().skip(i + 1) {
                    if next.t_s - e.t_s > 8.0 { break; }
                    if matches!(next.kind, EventKind::ToolUse(_)) {
                        count += 1;
                        burst_end = next.t_s;
                    }
                }
                if count >= 3 {
                    candidates.push(Clip {
                        start_s: (e.t_s - pre).max(0.0),
                        end_s: burst_end + post,
                        caption: format!("{}× tool burst", count),
                        score: 6.0 + count as f64,
                    });
                }
            }
            _ => {}
        }
    }

    // Score-sort descending.
    candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());

    // Greedy non-overlap selection, cap total length per clip.
    let mut picked: Vec<Clip> = Vec::new();
    for mut c in candidates {
        if c.end_s - c.start_s > max_seconds as f64 {
            c.end_s = c.start_s + max_seconds as f64;
        }
        let overlap = picked.iter().any(|p| !(c.end_s <= p.start_s || c.start_s >= p.end_s));
        if overlap {
            continue;
        }
        picked.push(c);
        if picked.len() >= max_clips as usize {
            break;
        }
    }

    // Output in chronological order for nicer "shorts/01..05" filenames.
    picked.sort_by(|a, b| a.start_s.partial_cmp(&b.start_s).unwrap());
    picked
}

pub fn render_vertical(raw: &Path, clip: &Clip, out: &Path) -> Result<()> {
    let duration = (clip.end_s - clip.start_s).max(0.5);

    // 9:16 vertical: crop a centered 608×1080 strip from 1920×1080, then scale up.
    // Burned-in caption at top with bold box. Caption is truncated to fit the
    // 1080px-wide frame at 52pt — empirically ~38 chars before overflow.
    let truncated = truncate_caption(&clip.caption, 38);
    let caption = esc(&truncated);
    let filter = format!(
        "scale=1920:1080:force_original_aspect_ratio=decrease,\
         pad=1920:1080:(ow-iw)/2:(oh-ih)/2:color=0x0c0e14,\
         crop=608:1080:(in_w-608)/2:0,\
         scale=1080:1920,\
         drawtext=text='{caption}':fontsize=52:fontcolor=white:\
             x=(w-tw)/2:y=120:\
             box=1:boxcolor=0x0b1020@0.75:boxborderw=24,\
         drawtext=text='flux-record':fontsize=28:fontcolor=0x9aa9ff:\
             x=(w-tw)/2:y=h-90:alpha=0.8"
    );

    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-hide_banner",
            "-loglevel", "warning",
            "-ss", &format!("{:.3}", clip.start_s),
            "-i", &raw.display().to_string(),
            "-t", &format!("{:.3}", duration),
            "-vf", &filter,
            "-c:v", "libx264",
            "-preset", "veryfast",
            "-crf", "20",
            "-pix_fmt", "yuv420p",
            "-c:a", "aac",
            "-b:a", "160k",
            "-movflags", "+faststart",
            &out.display().to_string(),
        ])
        .status()
        .context("failed to spawn ffmpeg for short")?;

    if !status.success() {
        anyhow::bail!("ffmpeg short render exited with {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::{Event, EventKind};

    fn ev(t: f64, kind: EventKind, label: &str) -> Event {
        Event { t_s: t, kind, label: label.to_string(), body: label.to_string() }
    }

    #[test]
    fn error_becomes_high_score_clip() {
        let events = vec![
            ev(30.0, EventKind::ToolResult { is_error: true }, "compile failed"),
        ];
        let clips = pick(&events, 60, 5);
        assert_eq!(clips.len(), 1);
        assert!(clips[0].caption.starts_with("ERROR"));
        assert!(clips[0].score >= 10.0);
    }

    #[test]
    fn dense_tool_burst_picked_up() {
        let events: Vec<Event> = (0..4)
            .map(|i| ev(10.0 + i as f64 * 0.5, EventKind::ToolUse("Bash".into()), "x"))
            .collect();
        let clips = pick(&events, 60, 5);
        assert!(!clips.is_empty());
        assert!(clips.iter().any(|c| c.caption.contains("tool burst")));
    }

    #[test]
    fn overlapping_clips_deduped() {
        let events = vec![
            ev(30.0, EventKind::ToolResult { is_error: true }, "err A"),
            ev(31.0, EventKind::ToolResult { is_error: true }, "err B"),
        ];
        let clips = pick(&events, 60, 5);
        assert_eq!(clips.len(), 1, "overlapping windows collapsed");
    }
}

fn truncate_caption(s: &str, max_chars: usize) -> String {
    let cleaned = s.replace('\n', " ");
    let chars: Vec<char> = cleaned.chars().collect();
    if chars.len() <= max_chars {
        return cleaned;
    }
    // Cut at the last whitespace within max_chars-1 so we don't break a word.
    let head: String = chars.iter().take(max_chars - 1).collect();
    let break_idx = head.rfind(char::is_whitespace).unwrap_or(head.len());
    let mut clipped = head[..break_idx].trim_end().to_string();
    if clipped.is_empty() {
        clipped = head;
    }
    format!("{clipped}…")
}

fn esc(s: &str) -> String {
    // ASCII apostrophe must become U+2019 — see crate::ffmpeg::esc for why.
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
