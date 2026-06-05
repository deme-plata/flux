//! Spectator overlay for `ashwalker-live` autoplay — MCP combo callouts, boss radar, colored arena.
//!
//! Wired from [`crate::rt`] + the live bin when `ASHWALKER_AUTOPLAY` (or `ASHWALKER_SPECTATE`) is set.

use crate::bosses::Archetype;
use crate::gore;
use crate::rt::{foe_archetype, EnemyPhase, RtGame, Sign, Strike};
use crate::{Foe, Terrain, V3};

fn strike_reach(s: Strike) -> i32 {
    match s {
        Strike::Bite | Strike::Lunge => 1,
        Strike::Smash | Strike::Cleave => 2,
    }
}

fn strike_label(s: Strike) -> &'static str {
    match s {
        Strike::Bite => "BITE",
        Strike::Lunge => "LUNGE",
        Strike::Smash => "SMASH",
        Strike::Cleave => "CLEAVE",
    }
}

/// Boss-trap readout for elite foes (minions share the Crown boss voice).
pub fn elite_boss_line(kind: Foe, hp: i32, max_hp: i32, phase: EnemyPhase) -> Option<String> {
    let a = foe_archetype(kind)?;
    let frac = (hp as f64 / max_hp.max(1) as f64).clamp(0.0, 1.0);
    let pnum = ((1.0 - frac) * 4.0) as usize + 1;
    let wind = match phase {
        EnemyPhase::Windup { left, strike } => {
            format!(
                "⚠ WINDUP {} · reach {} · {left} ticks",
                strike_label(strike),
                strike_reach(strike)
            )
        }
        EnemyPhase::Stagger { left } => format!("★ STAGGERED — {left} ticks open"),
        EnemyPhase::Recover { left } => format!("recovering — punish in {left}"),
        EnemyPhase::Stalk => "stalking".into(),
    };
    let trap = match a {
        Archetype::PrismCzar => "trap: attuned Sign HEALS it — use steel / off-sign",
        Archetype::Sediment => "trap: ARMORED until staggered — heavy/Aard/parry opens core",
        Archetype::Nullglyph => "trap: Signs feed drain — bank Sigil, save Aard",
        _ => "",
    };
    Some(format!(
        "  {CRIMSON}{boss}{RESET} ({minion}) · phase {pnum} · {wind}\n      {trap}",
        CRIMSON = gore::CRIMSON,
        RESET = gore::RESET,
        boss = a.name(),
        minion = kind.name(),
        trap = trap,
    ))
}

fn bar(cur: i32, max: i32, width: usize, full: char) -> String {
    let max = max.max(1);
    let n = (((cur.max(0) as f64 / max as f64) * width as f64).round() as usize).min(width);
    (0..width).map(|i| if i < n { full } else { '·' }).collect()
}

/// Terminal height from `LINES` (default 24). Short windows need the compact HUD.
pub fn terminal_rows() -> usize {
    std::env::var("LINES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(24)
}

/// Big isometric arena only when the terminal is tall enough (or `ASHWALKER_FULL=1`).
pub fn use_full_arena() -> bool {
    std::env::var("ASHWALKER_FULL").is_ok() || terminal_rows() >= 32
}

/// Scroll/log mode: do not erase the screen (readable in short IDE terminals).
pub fn use_scroll_log() -> bool {
    std::env::var("ASHWALKER_SCROLL").is_ok()
        || std::env::var("ASHWALKER_NO_CLEAR").is_ok()
        || !use_full_arena()
}

fn enemy_phase_tag(e: &crate::rt::RtEnemy) -> String {
    use crate::rt::EnemyPhase;
    match e.phase {
        EnemyPhase::Windup { left, strike } => {
            format!(
                "WINDUP {:?} r{} {}t",
                strike,
                strike_reach(strike),
                left
            )
        }
        EnemyPhase::Stagger { left } => format!("STAGGER {left}t"),
        EnemyPhase::Recover { left } => format!("recover {left}t"),
        EnemyPhase::Stalk => "stalking".into(),
    }
}

/// Text-only HUD for narrow/short terminals — no isometric canvas.
pub fn render_compact(g: &RtGame, ai: &str, hero: &str) -> String {
    use crate::gore::{self, CRIMSON, RESET};
    let mut out = String::new();
    out.push_str(&format!(
        "\n{CRIMSON}==== ASHWALKER | {hero} | tick {tick} | {foes} foes ===={RESET}\n",
        tick = g.tick,
        foes = g.living_foes(),
        CRIMSON = CRIMSON,
        RESET = RESET,
    ));
    out.push_str(&format!("  {CRIMSON}AI{RESET} {ai}\n", CRIMSON = CRIMSON, RESET = RESET));
    out.push_str(&format!(
        "  {CRIMSON}HERO{RESET} @({x},{y},z{z}) HP [{hpb}] {hp}/{maxhp} STA [{stab}] SIG [{sigb}] ward {ward} combo x{combo}\n",
        x = g.player.pos.x,
        y = g.player.pos.y,
        z = g.player.pos.z,
        hpb = bar(g.player.hp, g.player.max_hp, 16, '#'),
        hp = g.player.hp.max(0),
        maxhp = g.player.max_hp,
        stab = bar(g.player.stamina, g.player.max_stamina, 12, '='),
        sigb = bar(g.player.sigil, g.player.max_sigil, 10, '*'),
        ward = g.player.shield,
        combo = g.player.combo,
        CRIMSON = CRIMSON,
        RESET = RESET,
    ));
    out.push_str(&format!(
        "  slain {} dodges {} parries {} signs {} fusions {}\n",
        g.player.slain,
        g.player.perfect_dodges,
        g.player.parries,
        g.player.signs_cast,
        g.player.mcp_fusions,
    ));
    out.push_str(&format!(
        "  {CRIMSON}TACTICAL{RESET} {}\n",
        g.rt_tactical_line(),
        CRIMSON = CRIMSON,
        RESET = RESET,
    ));
    if let Some(ev) = &g.pulse_event {
        out.push_str(&format!(
            "  {CRIMSON}!! EVENT{RESET} {ev}\n",
            CRIMSON = CRIMSON,
            RESET = RESET,
        ));
    }
    if let Some(m) = &g.last_mcp {
        out.push_str(&format!(
            "  {CRIMSON}MCP{RESET} {m}\n",
            CRIMSON = CRIMSON,
            RESET = RESET,
        ));
    }
    out.push_str(&format!(
        "\n  {CRIMSON}-- FOES --{RESET}\n",
        CRIMSON = CRIMSON,
        RESET = RESET,
    ));
    let mut any = false;
    for e in g.enemies.iter().filter(|e| e.alive()) {
        any = true;
        let tag = if e.ally { '+' } else { '!' };
        let boss = foe_archetype(e.kind)
            .map(|a| format!(" [{}]", a.name()))
            .unwrap_or_default();
        out.push_str(&format!(
            "  {tag} {name}{boss} [{bar}] {hp}/{max}  {phase}\n",
            name = e.kind.name(),
            bar = bar(e.hp, e.kind.max_hp(), 10, '#'),
            hp = e.hp.max(0),
            max = e.kind.max_hp(),
            phase = enemy_phase_tag(e),
        ));
    }
    if !any {
        out.push_str("  (arena clear)\n");
    }
    out.push_str(&format!(
        "\n  {CRIMSON}-- COMBAT LOG --{RESET}\n",
        CRIMSON = CRIMSON,
        RESET = RESET,
    ));
    for l in g.feed.iter().rev().take(16).rev() {
        out.push_str("   > ");
        out.push_str(l);
        out.push('\n');
    }
    out
}
/// Full-screen spectator frame: colored arena + boss radar + MCP strip + combat log.
pub fn render(g: &RtGame, ai: &str, hero: &str) -> String {
    if !use_full_arena() {
        return render_compact(g, ai, hero);
    }
    let (w, h, d) = (g.world.w, g.world.h, g.world.d);
    let sw = ((w + h) * 2 + 4) as usize;
    let sh = ((w + h) + d * 2 + 4) as usize;
    let mut buf = vec![vec![' '; sw]; sh];
    let mut col = vec![vec![0u8; sw]; sh];
    let proj = |v: V3| -> (usize, usize) {
        let sx = ((v.x - v.y) * 2 + (h * 2)) as usize;
        let sy = ((v.x + v.y) - v.z * 2 + d * 2) as usize;
        (sx.min(sw - 1), sy.min(sh - 1))
    };
    let mut cells: Vec<V3> = Vec::new();
    for z in 0..d {
        for y in 0..h {
            for x in 0..w {
                cells.push(V3::new(x, y, z));
            }
        }
    }
    cells.sort_by_key(|v| (v.z, v.x + v.y));
    for v in &cells {
        let ch = match g.world.at(*v) {
            Terrain::Floor => '.',
            Terrain::Wall => '#',
            Terrain::Ramp => '/',
            Terrain::Hazard => '~',
            Terrain::Gate => '0',
        };
        let (sx, sy) = proj(*v);
        buf[sy][sx] = if g.yrden == Some(*v) { 'Y' } else { ch };
    }
    for s in g.blood.cells() {
        let (sx, sy) = proj(s.pos);
        buf[sy][sx] = s.ch;
        col[sy][sx] = if s.heavy { 4 } else { 1 };
    }
    for e in g.enemies.iter().filter(|e| e.alive()) {
        let base = if e.ally { '+' } else { e.kind.glyph() };
        let ch = if e.telegraphing() {
            base.to_ascii_uppercase()
        } else {
            base
        };
        let (sx, sy) = proj(e.pos);
        buf[sy][sx] = ch;
        col[sy][sx] = if e.telegraphing() {
            5
        } else if foe_archetype(e.kind).is_some() {
            3
        } else {
            6
        };
    }
    let pg = if g.player.invulnerable() { '*' } else { '@' };
    let (psx, psy) = proj(g.player.pos);
    buf[psy][psx] = pg;
    col[psy][psx] = 2;

    let telegraphs = g
        .enemies
        .iter()
        .filter(|e| e.alive() && !e.ally && e.telegraphing())
        .count();
    let mut out = String::new();
    let incoming = if telegraphs > 0 {
        format!(
            "  {CRIMSON}⚠×{telegraphs} INCOMING{RESET}",
            CRIMSON = gore::CRIMSON,
            RESET = gore::RESET
        )
    } else {
        String::new()
    };
    out.push_str(&format!(
        "\n{CRIMSON}╔══════════════════════════════════════════════════════════════╗{RESET}\n\
         {CRIMSON}║{RESET}  SPECTATOR · {hero} ← 🤖 {ai:<20} tick {:>5}  foes {}{incoming}  {CRIMSON}║{RESET}\n\
         {CRIMSON}╚══════════════════════════════════════════════════════════════╝{RESET}\n",
        g.tick,
        g.living_foes(),
        CRIMSON = gore::CRIMSON,
        RESET = gore::RESET,
    ));
    let ansi = |c: u8| -> &'static str {
        match c {
            1 => gore::RED,
            4 => gore::DARK_RED,
            2 => "\x1b[38;5;51m",
            3 => "\x1b[38;5;201m",
            5 => "\x1b[1;38;5;196m",
            6 => "\x1b[38;5;208m",
            _ => "",
        }
    };
    for (ri, row) in buf.iter().enumerate() {
        let plain: String = row.iter().collect();
        let trimmed = plain.trim_end();
        if trimmed.is_empty() {
            continue;
        }
        let keep = trimmed.chars().count();
        let mut line = String::new();
        let mut cur = 0u8;
        for (i, ch) in row.iter().enumerate().take(keep) {
            let c = col[ri][i];
            if c != cur {
                line.push_str(gore::RESET);
                if c != 0 {
                    line.push_str(ansi(c));
                }
                cur = c;
            }
            line.push(*ch);
        }
        if cur != 0 {
            line.push_str(gore::RESET);
        }
        out.push_str("  ");
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str(&format!(
        "  {CRIMSON}HERO{RESET} HP [{}] {}/{}  STA [{}]  SIG [{}]  ward {}  · combo x{}  · MCP fusions {}\n",
        bar(g.player.hp, g.player.max_hp, 18, '#'),
        g.player.hp.max(0),
        g.player.max_hp,
        bar(g.player.stamina, g.player.max_stamina, 14, '='),
        bar(g.player.sigil, g.player.max_sigil, 10, '*'),
        g.player.shield,
        g.player.combo,
        g.player.mcp_fusions,
        CRIMSON = gore::CRIMSON,
        RESET = gore::RESET,
    ));
    let state = if g.player.riposte {
        "RIPOSTE-READY"
    } else if g.player.guard > 0 {
        "GUARD"
    } else if g.player.busy > 0 {
        "committed"
    } else if g.player.invulnerable() {
        "i-frames"
    } else {
        "ready"
    };
    out.push_str(&format!(
        "  state: {state} · slain {} · dodges {} · parries {} · signs {}\n",
        g.player.slain,
        g.player.perfect_dodges,
        g.player.parries,
        g.player.signs_cast
    ));
    if let Some(line) = &g.last_mcp {
        out.push_str(&format!(
            "  {CRIMSON}⚡ MCP{RESET} {line}\n",
            CRIMSON = gore::CRIMSON,
            RESET = gore::RESET
        ));
    }
    if let Some(ev) = &g.pulse_event {
        out.push_str(&format!(
            "\n  {CRIMSON}══ EVENT ══{RESET} {ev}\n",
            CRIMSON = gore::CRIMSON,
            RESET = gore::RESET,
        ));
    }
    if g.death_cam > 0 {
        out.push_str(&format!(
            "  {CRIMSON}◆◆◆ DEATH-CAM ×{dcam} ◆◆◆{RESET}\n",
            CRIMSON = gore::CRIMSON,
            RESET = gore::RESET,
            dcam = g.death_cam,
        ));
    }
    out.push_str(&format!(
        "  {CRIMSON}TACTICAL{RESET} {}\n",
        g.rt_tactical_line(),
        CRIMSON = gore::CRIMSON,
        RESET = gore::RESET,
    ));
    out.push_str(&format!(
        "  {CRIMSON}SNAPSHOT{RESET} hash={:#018x}\n  {}",
        g.snapshot_hash(),
        g.fight_snapshot(),
        CRIMSON = gore::CRIMSON,
        RESET = gore::RESET,
    ));
    if let Some(trap) = g.enemies.iter().filter(|e| e.alive() && !e.ally).find_map(|e| foe_archetype(e.kind).map(|a| a.trap())) {
        out.push_str(&format!("  {CRIMSON}TRAP{RESET} {trap}\n", CRIMSON = gore::CRIMSON, RESET = gore::RESET));
    }
    out.push_str(&format!(
        "\n  {CRIMSON}── BOSS RADAR ──{RESET}\n",
        CRIMSON = gore::CRIMSON,
        RESET = gore::RESET
    ));
    let mut any_boss = false;
    for e in g.enemies.iter().filter(|e| e.alive() && !e.ally) {
        if let Some(line) = elite_boss_line(e.kind, e.hp, e.kind.max_hp(), e.phase) {
            any_boss = true;
            out.push_str(&line);
            out.push('\n');
        } else if e.telegraphing() {
            if let EnemyPhase::Windup { left, strike } = e.phase {
                out.push_str(&format!(
                    "  {} · {CRIMSON}⚠ {} · reach {} · {left} ticks{RESET}\n",
                    e.kind.name(),
                    strike_label(strike),
                    strike_reach(strike),
                    CRIMSON = gore::CRIMSON,
                    RESET = gore::RESET,
                ));
            }
        }
    }
    if !any_boss && telegraphs == 0 {
        out.push_str("  (quiet — no boss windups)\n");
    }
    out.push_str(&format!(
        "\n  {CRIMSON}── SIGIL → MCP MAP ──{RESET}\n",
        CRIMSON = gore::CRIMSON,
        RESET = gore::RESET
    ));
    for s in Sign::all() {
        let m = s.mcp();
        out.push_str(&format!(
            "  {sign} → {CRIMSON}{mcp}{RESET} ({cost} sigil) — {blurb}\n",
            sign = s.name(),
            mcp = m.name(),
            cost = s.cost(),
            blurb = m.blurb(),
            CRIMSON = gore::CRIMSON,
            RESET = gore::RESET,
        ));
    }
    out.push_str(&format!(
        "\n  {CRIMSON}── COMBAT LOG ──{RESET}\n",
        CRIMSON = gore::CRIMSON,
        RESET = gore::RESET
    ));
    for l in g.feed.iter().rev().take(10).rev() {
        out.push_str("   · ");
        out.push_str(l);
        out.push('\n');
    }
    out
}

pub fn render_boss(g: &crate::bossfight::BossFight, hero: &str) -> String {
    use crate::gore::{self, CRIMSON, RESET};
    if !use_full_arena() {
        let mut out = String::new();
        out.push_str(&format!(
            "\n{CRIMSON}==== BOSS | {hero} | {} | tick {}{RESET}\n",
            g.brain.archetype().name(),
            g.tick,
            CRIMSON = CRIMSON,
            RESET = RESET,
        ));
        out.push_str(&format!("  TRAP: {}\n", g.status_line()));
        out.push_str(&format!("  STATE: {}\n", g.boss_snapshot()));
        out.push_str(&format!(
            "  HERO HP {}/{} @({},{})\n",
            g.player.hp, g.player.max_hp, g.player.pos.x, g.player.pos.y
        ));
        out.push_str("\n  -- COMBAT LOG --\n");
        for l in g.feed.iter().rev().take(16).rev() {
            out.push_str("   > ");
            out.push_str(l);
            out.push_str("\n");
        }
        return out;
    }
    let mut out = String::new();
    let flash = g.feed.iter().rev().take(3).any(|l| {
        l.contains("COMBO")
            || l.contains("REPLAY")
            || l.contains("REWIND")
            || l.contains("REZZES")
            || l.contains("INCOMING")
    }) || g.status_line().contains("REPLAY ARMED");
    if flash {
        out.push_str(&format!(
            "  {CRIMSON}▓▓ BOSS EVENT FLASH ▓▓{RESET}\n",
            CRIMSON = gore::CRIMSON,
            RESET = gore::RESET,
        ));
    }
    let (top, bot) = if flash {
        (
            "╔▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓╗",
            "╚▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓╝",
        )
    } else {
        (
            "╔══════════════════════════════════════════════════════════════╗",
            "╚══════════════════════════════════════════════════════════════╝",
        )
    };
    out.push_str(&format!(
        "\n{CRIMSON}{top}{RESET}\n         {CRIMSON}║{RESET}  BOSS SPECTATOR · {hero} · {}  tick {:>5}  {CRIMSON}║{RESET}\n         {CRIMSON}{bot}{RESET}\n",
        g.brain.archetype().name(),
        g.tick,
        CRIMSON = gore::CRIMSON,
        RESET = gore::RESET,
    ));
    out.push_str(&crate::bossfight::boss_render(g));
    out.push_str(&format!("  {CRIMSON}TRAP{RESET} {}\n", g.status_line(), CRIMSON = CRIMSON, RESET = RESET));
    out.push_str(&format!("  {CRIMSON}STATE{RESET} {}\n", g.boss_snapshot(), CRIMSON = CRIMSON, RESET = RESET));
    out.push_str(&format!("\n  {CRIMSON}── COMBAT LOG ──{RESET}\n", CRIMSON = gore::CRIMSON, RESET = gore::RESET));
    for l in g.feed.iter().rev().take(12).rev() {
        out.push_str("   · ");
        out.push_str(l);
        out.push('\n');
    }
    out
}
