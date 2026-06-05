//! Karaoke-style ASS subtitle generator from assistant text events.
//!
//! Produces a single .ass file: each assistant text turn becomes a series of
//! word-grouped events with a soft pop-in animation. Drop it into ffmpeg as
//! `-vf "ass=captions.ass"` (or burn-in via the filtergraph) to get
//! YouTube-style captions burned into the cinematic render.

use crate::transcript::{Event, EventKind};

pub fn render_ass(events: &[Event]) -> String {
    let mut out = String::new();
    out.push_str(HEADER);

    // We display ~4 words at a time, ~1.6s each. Time-warp so all captions for
    // a turn fit between this turn's t_s and the next event's t_s (or +12s).
    let mut next_starts: Vec<f64> = events.iter().skip(1).map(|e| e.t_s).collect();
    next_starts.push(events.last().map(|e| e.t_s + 12.0).unwrap_or(12.0));

    for (e, next_t) in events.iter().zip(next_starts.iter()) {
        if !matches!(e.kind, EventKind::AssistantText) {
            continue;
        }
        let body = e.body.trim();
        if body.is_empty() {
            continue;
        }

        let groups = group_words(body, 4);
        if groups.is_empty() {
            continue;
        }

        let window = (next_t - e.t_s).clamp(2.0, 16.0);
        let per = (window / groups.len() as f64).clamp(0.8, 2.4);

        for (i, g) in groups.iter().enumerate() {
            let t0 = e.t_s + per * i as f64;
            let t1 = (t0 + per).min(e.t_s + window);
            out.push_str(&dialogue_line(t0, t1, g));
        }
    }
    out
}

fn group_words(text: &str, n: usize) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    words
        .chunks(n.max(1))
        .map(|chunk| chunk.join(" "))
        .collect()
}

fn dialogue_line(start_s: f64, end_s: f64, text: &str) -> String {
    let start = fmt_ass_time(start_s);
    let end = fmt_ass_time(end_s);
    let safe = text
        .replace('{', "(")
        .replace('}', ")")
        .replace('\\', "/")
        .replace('\n', " ");
    // {\fad(120,160)} = fade in 120ms, fade out 160ms. \pos centers near bottom.
    format!(
        "Dialogue: 0,{start},{end},Karaoke,,0,0,0,,{{\\fad(120,160)\\pos(960,920)}}{safe}\n"
    )
}

fn fmt_ass_time(t: f64) -> String {
    let t = t.max(0.0);
    let h = (t / 3600.0).floor() as u32;
    let m = ((t % 3600.0) / 60.0).floor() as u32;
    let s = t % 60.0;
    let cs = ((s - s.floor()) * 100.0).round() as u32;
    let s_int = s.floor() as u32;
    format!("{}:{:02}:{:02}.{:02}", h, m, s_int, cs)
}

const HEADER: &str = "\
[Script Info]
ScriptType: v4.00+
PlayResX: 1920
PlayResY: 1080
WrapStyle: 2
ScaledBorderAndShadow: yes

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Karaoke,DejaVu Sans,56,&H00E6ECFF,&H000000FF,&H80000914,&H80000914,1,0,0,0,100,100,0,0,1,4,2,2,80,80,80,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
";
