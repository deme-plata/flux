//! flux-concert — conduct a full "number" and show the shape of the music:
//! the harmony curve bar-by-bar, the climaxes (reached many times), the aha
//! moments, and the overall progress. Writes the performance JSON to
//! FLUX_ORCHESTRA_STATUS_PATH for a "Conduct" button on the desktop / qwen app.

use flux_orchestrator::{
    performance_status_json, Bar, Conductor, Direction, Orchestra, PlayResult, Tempo, Track,
};

fn unison_bar(players: &[&str]) -> Bar {
    players.iter().map(|n| PlayResult::of(n, true, 12_000, "unison")).collect()
}
fn build_up(players: &[&str]) -> Bar {
    // a few in unison, one still finding the note → not yet climax
    players
        .iter()
        .enumerate()
        .map(|(i, n)| PlayResult::of(n, true, 12_000 + i as u64 * 1500, if i == 0 { "search" } else { "unison" }))
        .collect()
}
fn muddy(players: &[&str]) -> Bar {
    players
        .iter()
        .enumerate()
        .map(|(i, n)| PlayResult::of(n, i % 2 == 0, 12_000 + (i as u64 * 9000), &format!("note{i}")))
        .collect()
}

fn bar_glyph(h: f64, coherent: bool, climax: bool, aha: bool) -> String {
    let blocks = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    let idx = ((h * (blocks.len() as f64 - 1.0)).round() as usize).min(blocks.len() - 1);
    let mark = if climax { "✺" } else if aha { "✦" } else if coherent { "·" } else { " " };
    format!("{}{}", blocks[idx], mark)
}

fn main() {
    let o = Orchestra::flux_default();
    let players = ["Claude Code", "Codex", "Qwen", "Gemini CLI", "Grok", "Cursor"];

    // A "number" that builds, peaks, dips, and peaks again — many climaxes.
    let track = Track::new("Flux Symphony No.1 — 'Self-Hosting'")
        .bar(muddy(&players))
        .bar(build_up(&players))
        .bar(unison_bar(&players)) // climax
        .bar(muddy(&players))
        .bar(build_up(&players))
        .bar(unison_bar(&players)) // climax
        .bar(unison_bar(&players)) // sustained climax
        .bar(build_up(&players))
        .bar(unison_bar(&players)); // climax

    let perf = o.perform(&track, Tempo::default(), Conductor::Ai);

    println!("🎼  {}\n", perf.track);
    println!("    conductor: AI · {} instruments · {} bars\n", o.instruments.len(), perf.bars.len());
    print!("    harmony  ");
    for (i, c) in perf.bars.iter().enumerate() {
        let climax = perf.climaxes.contains(&i);
        let aha = perf.aha_moments.contains(&i);
        print!("{}", bar_glyph(c.harmony, c.coherent, climax, aha));
    }
    println!("\n             ✺=climax  ✦=aha  ·=coherent\n");

    println!("    peak harmony   : {:.3}", perf.peak_harmony);
    println!("    climaxes       : {} at bars {:?}", perf.climaxes.len(), perf.climaxes);
    println!("    many climaxes  : {}", perf.many_climaxes);
    println!("    aha moments    : {} at bars {:?}", perf.aha_moments.len(), perf.aha_moments);
    println!("    progress       : {:.0}%", perf.progress * 100.0);
    println!("    coherent ratio : {:.0}%", perf.coherent_ratio * 100.0);

    // last bar's conductor directions
    if let Some(last) = perf.bars.last() {
        let dirs: Vec<String> = last
            .directions
            .iter()
            .map(|d| match d {
                Direction::Tutti => "tutti".into(),
                Direction::Solo(n) => format!("solo:{n}"),
                Direction::Rest(n) => format!("rest:{n}"),
                Direction::Retune(n) => format!("retune:{n}"),
            })
            .collect();
        println!("    final baton    : {}", if dirs.is_empty() { "—".into() } else { dirs.join(", ") });
    }

    let verdict = perf.many_climaxes && perf.progress > 0.3;
    println!("\n    VERDICT: {} — climax reached many times, with aha moments and progress",
        if verdict { "✓ COHERENT PERFORMANCE" } else { "✗ flat / incoherent" });

    let path = std::env::var("FLUX_ORCHESTRA_STATUS_PATH")
        .unwrap_or_else(|_| "/tmp/flux-orchestra-status.json".to_string());
    let _ = std::fs::write(&path, performance_status_json(&perf));
    println!("    status written → {}", path);
}
