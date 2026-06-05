//! flux-bbq-book — dogfood flux-bbq on the job that proved it: analyze
//! "Shadows in the Chain" ch 1-24 on one GPU, SERIAL (heat=1), so the box never
//! chokes. Each chapter is a Skewer; the Pit cooks them in order; results come
//! back as Vec<Cooked> (no shared file to clobber).
//!
//! Env: FLUX_BBQ_BOX (host:port), FLUX_BBQ_MODEL, FLUX_BBQ_HEAT (default 1).

use std::fs;

use flux_bbq::{ollama_grill, tally, Pit, Skewer};

fn score_of(s: &str) -> Option<u32> {
    let u = s.to_uppercase();
    let i = u.find("SCORE")?;
    u[i..]
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()
}

fn main() {
    let box_hp = std::env::var("FLUX_BBQ_BOX").unwrap_or_else(|_| "108.143.3.52:16083".into());
    let model = std::env::var("FLUX_BBQ_MODEL").unwrap_or_else(|_| "deepseek-coder-v2:16b".into());
    let heat: usize = std::env::var("FLUX_BBQ_HEAT").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    let ep = format!("http://{box_hp}");
    let asset = "/home/storage/deepseek-codewhale/sigil/crates/sigil-book/assets";

    let mut skewers = Vec::new();
    for n in 1..=24 {
        let path = format!("{asset}/chapter{n}_content_improved.tex");
        let Ok(tex) = fs::read_to_string(&path) else { continue };
        let body: String = tex.chars().take(8000).collect();
        let prompt = format!(
            "Analyze chapter {n} of 'Shadows in the Chain' (cyberpunk thriller, post-quantum \
             blockchain conspiracy). In <130 words: 1-line plot, the central THEME, one prose \
             STRENGTH, one WEAKNESS, and whether the crypto/quantum TECH rings true. Your FINAL \
             line MUST be exactly: SCORE: <0-100>\n\nCHAPTER:\n{body}"
        );
        skewers.push(Skewer::new(format!("ch{n}"), &model, prompt));
    }

    eprintln!("🔥 flux-bbq: {} skewers · heat={heat} · grill={ep} ({model})", skewers.len());
    let pit = Pit::new(&ep).with_heat(heat);
    let cooked = pit.cook(&skewers, ollama_grill(&ep));

    let mut report = format!("# Shadows in the Chain — flux-bbq · {model} · heat={heat}\n\n");
    let mut scores = Vec::new();
    for c in &cooked {
        let sc = if c.ok { score_of(&c.output) } else { None };
        if let Some(s) = sc {
            scores.push(s);
        }
        let tag = sc.map(|s| s.to_string()).unwrap_or_else(|| if c.ok { "?".into() } else { "ERR".into() });
        report.push_str(&format!(
            "## {} — score {tag}/100 · {}ms\n{}\n\n---\n\n",
            c.id, c.ms, if c.ok { c.output.trim() } else { &c.error }
        ));
        eprintln!("  {} -> {tag}/100 ({}ms){}", c.id, c.ms, if c.ok { "" } else { " FAIL" });
    }
    let (ok, fail) = tally(&cooked);
    report.push_str("## Scoreboard\n| ch | score |\n|----|-------|\n");
    for c in &cooked {
        let sc = if c.ok {
            score_of(&c.output).map(|s| s.to_string()).unwrap_or("?".into())
        } else {
            "ERR".into()
        };
        report.push_str(&format!("| {} | {} |\n", c.id, sc));
    }
    let avg = if scores.is_empty() { 0.0 } else { scores.iter().sum::<u32>() as f64 / scores.len() as f64 };
    report.push_str(&format!(
        "\n**cooked ok: {ok} · failed: {fail} · numeric scores: {} · avg {:.1}/100**\n",
        scores.len(),
        avg
    ));
    fs::write("/tmp/shadows-bbq.md", &report).expect("write report");
    eprintln!("✅ done: {ok} ok, {fail} failed, avg {:.1}/100 → /tmp/shadows-bbq.md", avg);
}
