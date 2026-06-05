//! ASHWALKER · LIVE — real-time, Witcher-3-feel terminal combat.
//!
//!   flux-cargo-wrapper run -p sigil-ashwalker --bin ashwalker-live
//!
//! Controls (raw terminal, 20 fps):
//!   W A S D / arrows  move + face        SPACE  dodge-roll (i-frames)
//!   J light           K heavy            L parry → riposte
//!   1 Igni  2 Quen  3 Aard  4 Yrden  5 Axii    Q quit
//!
//! Combat is *sincere*: foes flash UPPERCASE + "⚠ INCOMING" while they wind up. Read the tell —
//! dodge through it, parry it, or burn them down first.
//!
//! Headless/CI: when stdin isn't a TTY (or `ASHWALKER_DEMO=1`), a deterministic scripted bout plays
//! itself so the loop is verifiable without a terminal.

use sigil_ashwalker::rt::*;
use sigil_ashwalker::spectator;
use sigil_ashwalker::{Foe, Terrain, V3};
use std::io::{self, Read, Write};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

fn is_tty() -> bool {
    // std-only: ask `tty` whether fd 0 is a terminal.
    std::process::Command::new("tty")
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn stty(args: &[&str]) {
    let _ = std::process::Command::new("stty")
        .args(args)
        .stdin(std::process::Stdio::inherit())
        .status();
}

fn clear() {
    if sigil_ashwalker::spectator::use_scroll_log() {
        println!("\n\x1b[1m── frame ──\x1b[0m");
        return;
    }
    print!("\x1b[2J\x1b[H");
}

fn hero_name() -> String {
    std::env::var("ASHWALKER_NAME").unwrap_or_else(|_| "Ashwalker".into())
}

/// Opening cinematic — visible in the terminal before raw mode / the bout.
fn show_intro(wait_for_enter: bool) {
    if std::env::var("ASHWALKER_SKIP_INTRO").is_ok() {
        return;
    }
    clear();
    print!("{}", rt_intro_screen(&hero_name()));
    println!("  {CRIMSON}Press ENTER to enter the Ashlands…{RESET}\n", CRIMSON = sigil_ashwalker::gore::CRIMSON, RESET = sigil_ashwalker::gore::RESET);
    let _ = io::stdout().flush();
    if wait_for_enter {
        let mut line = String::new();
        let _ = io::stdin().read_line(&mut line);
    } else {
        let ms = if std::env::var("ASHWALKER_FAST").is_ok() { 400 } else { 2200 };
        thread::sleep(Duration::from_millis(ms));
    }
}

fn draw(g: &RtGame) {
    clear();
    if std::env::var("ASHWALKER_PLAIN").is_err() {
        print!("{}", sigil_ashwalker::spectator::render(g, "you", &hero_name()));
    } else {
        print!("{}", rt_render(g));
    }
    // last few feed lines
    for l in g.feed.iter().rev().take(5).rev() {
        println!("   · {l}");
    }
    println!("  [{}]", rt_controls_line());
    let _ = std::io::stdout().flush();
}

/// Translate a key byte → an action, using the current facing for the dodge direction.
fn key_to_action(b: u8, face: Dir) -> Option<Action> {
    match b {
        b'w' | b'W' => Some(Action::Move(Dir::new(0, -1))),
        b's' | b'S' => Some(Action::Move(Dir::new(0, 1))),
        b'a' | b'A' => Some(Action::Move(Dir::new(-1, 0))),
        b'd' | b'D' => Some(Action::Move(Dir::new(1, 0))),
        b' ' => Some(Action::Dodge(face)),
        b'j' | b'J' => Some(Action::Light),
        b'k' | b'K' => Some(Action::Heavy),
        b'l' | b'L' => Some(Action::Parry),
        b'1' => Some(Action::Cast(Sign::Igni)),
        b'2' => Some(Action::Cast(Sign::Quen)),
        b'3' => Some(Action::Cast(Sign::Aard)),
        b'4' => Some(Action::Cast(Sign::Yrden)),
        b'5' => Some(Action::Cast(Sign::Axii)),
        _ => None,
    }
}

fn write_transcript(g: &RtGame, hero: &str) {
    let path = std::env::var("ASHWALKER_TRANSCRIPT").unwrap_or_else(|_| "ashwalker-transcript.md".into());
    let _ = std::fs::write(&path, g.transcript_markdown(hero));
    println!("  transcript → {path}");
}

fn step_pause(g: &mut RtGame) {
    if std::env::var("ASHWALKER_STEP").is_err() {
        return;
    }
    let combo_beat = g
        .feed
        .last()
        .map(|l| l.contains("COMBO") || l.contains("PARRY") || l.contains("FUSE"))
        .unwrap_or(false);
    let telegraph = g
        .enemies
        .iter()
        .any(|e| e.alive() && !e.ally && e.telegraphing());
    let pause = g.pulse_event.is_some()
        || g.death_cam > 0
        || g.won()
        || !g.player.alive()
        || telegraph
        || combo_beat;
    if !pause {
        return;
    }
    let hint = g
        .pulse_event
        .as_deref()
        .or_else(|| g.feed.last().map(|s| s.as_str()))
        .unwrap_or("beat");
    let short: String = hint.chars().take(72).collect();
    print!("  [STEP] {short} — ENTER…");
    let _ = io::stdout().flush();
    let mut line = String::new();
    let _ = io::stdin().read_line(&mut line);
    g.pulse_event = None;
}

fn finale(g: &RtGame) {
    println!("\n══════ {} ══════", if g.player.alive() && g.won() {
        "VICTORY — the gate to Crown & Ash opens"
    } else if !g.player.alive() {
        "FALLEN in the Ashlands"
    } else {
        "the bout ends"
    });
    if g.player.alive() && g.won() {
        let a = g.ascend();
        println!("\n{}\n", a.blurb);
        println!("  ┌─ CROWN & ASH ASCENSION ─────────────────────────────");
        println!("  │ House : {}", a.house);
        println!("  │ Title : {}", a.title);
        println!("  │ Crown : {}   Ash : {}   Levies : {}", a.crown, a.ash, a.troops);
        println!("  └─ seed → {}", a.seed_line());
    } else {
        println!(
            "  slain {} · dodges {} · parries {} · signs {} — the Ashlands keep their gate.",
            g.player.slain, g.player.perfect_dodges, g.player.parries, g.player.signs_cast
        );
    }
}

/// Viktor's AI driver — returns a one-line intent for the spectator HUD.
fn autoplay_action(g: &mut RtGame) -> String {
    let mut intent = String::from("scanning…");
    let ppos = g.player.pos;
    let face = g.player.face;

    if let Some(prev) = g.pending_sign() {
        if prev == Sign::Quen && g.player.sigil >= Sign::Yrden.cost() && g.player.busy == 0 {
            let intent = "▸ FUSE Yrden → COMBO Warded-Rally (flux_zk_combo + council_consensus)".into();
            g.input(Action::Cast(Sign::Yrden));
            return intent;
        }
    }

    let mut incoming = false;
    let mut incoming_kind = "";
    for e in &g.enemies {
        if !e.alive() || e.ally {
            continue;
        }
        if let EnemyPhase::Windup { left, strike } = e.phase {
            let strike_reach = match strike {
                Strike::Bite | Strike::Lunge => 1,
                Strike::Smash | Strike::Cleave => 2,
            };
            if left <= 2 && e.pos.reach(ppos) <= strike_reach {
                incoming = true;
                incoming_kind = e.kind.name();
                let vx = e.pos.x - ppos.x;
                let vy = e.pos.y - ppos.y;
                if vx * face.dx + vy * face.dy >= 0 && g.player.guard == 0 && g.player.busy == 0 {
                    intent = format!("▸ PARRY vs {incoming_kind}");
                    g.input(Action::Parry);
                    return intent;
                }
            }
        }
    }
    if incoming && g.player.stamina >= 25 && g.player.busy == 0 {
        intent = format!("▸ DODGE {incoming_kind}'s tell");
        g.input(Action::Dodge(face));
        return intent;
    }

    if g.player.riposte && g.player.busy == 0 {
        intent = "▸ RIPOSTE light".into();
        g.input(Action::Light);
        return intent;
    }

    if g.player.hp < g.player.max_hp / 3
        && g.player.sigil >= Sign::Quen.cost()
        && g.player.shield < 12
        && g.player.busy == 0
    {
        intent = "▸ CAST Quen → flux_zk_combo".into();
        g.input(Action::Cast(Sign::Quen));
        return intent;
    }

    let cone_foes = g
        .enemies
        .iter()
        .filter(|e| e.alive() && !e.ally && rt_in_arc(ppos, face, e.pos, 3))
        .count();
    if cone_foes >= 2
        && g.player.sigil >= Sign::Quen.cost() + Sign::Yrden.cost()
        && g.player.busy == 0
        && g.combo_window_open()
    {
        intent = "▸ SETUP COMBO Quen→Yrden (Warded-Rally)".into();
        g.input(Action::Cast(Sign::Quen));
        return intent;
    }
    if cone_foes >= 2 && g.player.sigil >= Sign::Igni.cost() && g.player.busy == 0 {
        intent = format!("▸ CAST Igni → mining_status ({cone_foes} in cone)");
        g.input(Action::Cast(Sign::Igni));
        return intent;
    }

    if g.living_foes() == 1
        && g.player.sigil >= Sign::Igni.cost()
        && g.player.busy == 0
        && g
            .enemies
            .iter()
            .any(|e| e.alive() && !e.ally && rt_in_arc(ppos, face, e.pos, 3))
    {
        intent = "▸ FINISHER Igni".into();
        g.input(Action::Cast(Sign::Igni));
        return intent;
    }

    if let Some(e) = g
        .enemies
        .iter()
        .filter(|e| e.alive() && !e.ally)
        .min_by_key(|e| e.pos.reach(ppos))
    {
        if ppos.z < e.pos.z && g.player.busy == 0 && g.player.move_cd == 0 {
            intent = format!("▸ CLIMB ramp → {}", e.kind.name());
            let on_ramp = g.world.at(ppos) == Terrain::Ramp || ppos == V3::new(5, 5, 0);
            if on_ramp {
                let dx = (e.pos.x - ppos.x).signum();
                let dy = (e.pos.y - ppos.y).signum();
                g.input(Action::Move(Dir::new(dx.max(-1).min(1), dy.max(-1).min(1))));
            } else {
                let ramp = V3::new(5, 5, 0);
                let dx = (ramp.x - ppos.x).signum();
                let dy = (ramp.y - ppos.y).signum();
                g.input(Action::Move(Dir::new(dx, dy)));
            }
            return intent;
        }
        if rt_in_arc(ppos, face, e.pos, 2) && g.player.busy == 0 && g.player.stamina >= 30 {
            intent = format!("▸ HEAVY vs {}", e.kind.name());
            g.input(Action::Heavy);
            return intent;
        }
        if rt_in_arc(ppos, face, e.pos, 1) && g.player.busy == 0 && g.player.light_cd == 0 {
            intent = format!("▸ light chain x{} vs {}", g.player.combo + 1, e.kind.name());
            g.input(Action::Light);
            return intent;
        }
        if g.player.busy == 0 && g.player.move_cd == 0 {
            let dx = (e.pos.x - ppos.x).signum();
            let dy = (e.pos.y - ppos.y).signum();
            if dx != 0 || dy != 0 {
                intent = format!("▸ SEEK {}", e.kind.name());
                g.input(Action::Move(Dir::new(dx, dy)));
                return intent;
            }
        }
    }

    if g.player.busy == 0 && g.player.sigil >= Sign::Yrden.cost() && g.tick % 40 == 0 {
        intent = "▸ CAST Yrden → council_consensus".into();
        g.input(Action::Cast(Sign::Yrden));
        return intent;
    }
    intent
}

/// Auto-play a full run (intro + fight + finale) — for CI and "play it for me".
fn autoplay() {
    let fast = std::env::var("ASHWALKER_FAST").is_ok();
    let pace = if fast { 35 } else { 110 };
    let mut g = RtGame::new_from_env();
    for e in &mut g.enemies {
        if e.kind == Foe::EmberLord {
            e.pos = V3::new(6, 4, 0);
        }
    }
    show_intro(false);
    println!(
        "  {CRIMSON}🤖 grok-viktor SPECTATOR MODE — MCP combos, boss radar, colored arena{RESET}
",
        CRIMSON = sigil_ashwalker::gore::CRIMSON,
        RESET = sigil_ashwalker::gore::RESET
    );
    let _ = io::stdout().flush();
    let hero = hero_name();
    let ai = "grok-viktor";
    let mut last_feed = 0usize;
    let mut ai_intent = String::from("boot…");

    for t in 0..9000u64 {
        if g.player.busy == 0 && g.player.light_cd == 0 && g.player.move_cd == 0 {
            ai_intent = autoplay_action(&mut g);
        }
        g.tick();
        let feed_changed = g.feed.len() != last_feed;
        let pulse = feed_changed
            || t % 5 == 0
            || g.enemies.iter().any(|e| e.alive() && !e.ally && e.telegraphing())
            || g.won()
            || !g.player.alive();
        if pulse {
            clear();
            print!("{}", spectator::render(&g, &ai_intent, &hero));
            let _ = io::stdout().flush();
            step_pause(&mut g);
            last_feed = g.feed.len();
            if !fast {
                thread::sleep(Duration::from_millis(pace));
            }
        }
        if g.won() || !g.player.alive() {
            break;
        }
    }
    clear();
    print!("{}", spectator::render(&g, &ai_intent, &hero));
    write_transcript(&g, &hero_name());
    finale(&g);
}

/// Short scripted bout — regression / mechanic smoke test.
fn demo() {
    let mut g = RtGame::new_from_env();
    let script: &[(u64, Action)] = &[
        (1, Action::Move(Dir::new(1, 0))),
        (6, Action::Cast(Sign::Quen)),
        (9, Action::Light),
        (12, Action::Light),
        (20, Action::Cast(Sign::Igni)),
        (28, Action::Parry),
        (30, Action::Light),
    ];
    let fast = std::env::var("ASHWALKER_FAST").is_ok();
    let mut si = 0;
    show_intro(false);
    for t in 0..120u64 {
        while si < script.len() && script[si].0 == t {
            g.input(script[si].1);
            si += 1;
        }
        g.tick();
        if t % 2 == 0 || g.won() || !g.player.alive() {
            draw_demo(&g);
            if !fast {
                thread::sleep(Duration::from_millis(90));
            }
        }
        if g.won() || !g.player.alive() {
            break;
        }
    }
    finale(&g);
}

fn draw_demo(g: &RtGame) {
    clear();
    print!("{}", rt_render(g));
    for l in g.feed.iter().rev().take(4).rev() {
        println!("   · {l}");
    }
    let _ = std::io::stdout().flush();
}

fn main() {
    if std::env::var("ASHWALKER_AUTOPLAY").is_ok() {
        if sigil_ashwalker::spectator::use_scroll_log() {
            eprintln!(
                "ASHWALKER: scroll/compact HUD (terminal {} rows). Use ASHWALKER_FULL=1 for big arena.",
                sigil_ashwalker::spectator::terminal_rows()
            );
        }
        autoplay();
        return;
    }
    if std::env::var("ASHWALKER_DEMO").is_ok() || !is_tty() {
        demo();
        return;
    }

    // ── interactive raw-terminal mode ──
    show_intro(true);
    stty(&["raw", "-echo"]);
    let (tx, rx) = mpsc::channel::<u8>();
    thread::spawn(move || {
        let mut byte = [0u8; 1];
        let mut stdin = std::io::stdin();
        while stdin.read(&mut byte).map(|n| n == 1).unwrap_or(false) {
            if tx.send(byte[0]).is_err() {
                break;
            }
        }
    });

    let mut g = RtGame::new_from_env();
    let tickdur = Duration::from_millis(TICK_MS);
    let mut quit = false;
    while g.player.alive() && !g.won() && !quit {
        let frame_start = Instant::now();
        // drain all input that arrived since last frame
        while let Ok(b) = rx.try_recv() {
            if b == b'q' || b == b'Q' || b == 3 {
                quit = true;
                break;
            }
            if let Some(act) = key_to_action(b, g.player.face) {
                g.input(act);
            }
        }
        g.tick();
        draw(&g);
        if let Some(rem) = tickdur.checked_sub(frame_start.elapsed()) {
            thread::sleep(rem);
        }
    }

    stty(&["sane"]);
    print!("\x1b[2J\x1b[H");
    finale(&g);
}
