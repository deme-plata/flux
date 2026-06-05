//! ASHWALKER · BOSS — fight the prototype-3 bosses live, in the terminal.
//!
//!   flux-cargo-wrapper run -p sigil-ashwalker --bin ashwalker-boss
//!   ASHWALKER_BOSS=audithollow ASHWALKER_SPECTATE=1 ASHWALKER_DEMO=1  # scripted spectacle
//!
//! Controls (raw terminal, 20 fps):
//!   W A S D  move + face        SPACE  dodge-roll (i-frames)
//!   J light  K heavy  L parry   1 Igni 2 Quen 3 Aard 4 Yrden 5 Axii    Q quit
//!
//! Headless/CI: `ASHWALKER_DEMO=1` (or no TTY) plays scripted bouts. `ASHWALKER_SPECTATE=1`
//! uses the same colored spectator HUD as `ashwalker-live` autoplay.

use sigil_ashwalker::bossfight::*;
use sigil_ashwalker::bosses::Archetype;
use sigil_ashwalker::rt::{Action, Dir, Sign};
use sigil_ashwalker::spectator;
use std::io::{Read, Write};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

fn hero_name() -> String {
    std::env::var("ASHWALKER_NAME").unwrap_or_else(|_| "Walker".into())
}

fn spectate() -> bool {
    std::env::var("ASHWALKER_SPECTATE").is_ok()
}

fn is_tty() -> bool {
    std::process::Command::new("tty")
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn stty(args: &[&str]) {
    let _ = std::process::Command::new("stty").args(args).stdin(std::process::Stdio::inherit()).status();
}

fn pick() -> Vec<Archetype> {
    match std::env::var("ASHWALKER_BOSS").ok().as_deref() {
        Some("prism") | Some("czar") => vec![Archetype::PrismCzar],
        Some("sediment") | Some("colossus") => vec![Archetype::Sediment],
        Some("audit") | Some("audithollow") => vec![Archetype::Audithollow],
        Some("pack") | Some("nine") => vec![Archetype::PackOfNine],
        Some("rot") | Some("rotmaw") => vec![Archetype::Rotmaw],
        Some("caesura") | Some("time") => vec![Archetype::Caesura],
        Some("null") | Some("nullglyph") => vec![Archetype::Nullglyph],
        _ => vec![
            Archetype::Audithollow,
            Archetype::PackOfNine,
            Archetype::PrismCzar,
            Archetype::Rotmaw,
            Archetype::Caesura,
            Archetype::Nullglyph,
        ],
    }
}

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

fn draw(g: &BossFight, hero: &str) {
    if sigil_ashwalker::spectator::use_scroll_log() {
        println!("\n\x1b[1m── boss frame ──\x1b[0m");
    } else {
        print!("\x1b[2J\x1b[H");
    }
    if spectate() {
        print!("{}", spectator::render_boss(g, hero));
    } else {
        print!("{}", boss_render(g));
        for l in g.feed.iter().rev().take(5).rev() {
            println!("   · {l}");
        }
        println!("  TRAP: {}", g.status_line());
    }
    println!("  [WASD · SPACE dodge · J/K/L · 1-5 signs · Q quit]");
    let _ = std::io::stdout().flush();
}

fn demo_script(which: Archetype) -> Vec<(u64, Action)> {
    match which {
        Archetype::Audithollow => vec![
            (1, Action::Move(Dir::new(1, 0))),
            (3, Action::Move(Dir::new(1, 0))),
            (6, Action::Light),
            (8, Action::Dodge(Dir::new(-1, 0))),
            (10, Action::Cast(Sign::Igni)),
            (12, Action::Cast(Sign::Quen)),
            (14, Action::Heavy),
            (20, Action::Dodge(Dir::new(0, -1))), // dodge replay
            (24, Action::Parry),
            (28, Action::Heavy),
            (32, Action::Dodge(Dir::new(1, 0))),
            (36, Action::Heavy),
            (42, Action::Light),
            (48, Action::Heavy),
        ],
        Archetype::PackOfNine => vec![
            (1, Action::Move(Dir::new(1, 0))),
            (4, Action::Move(Dir::new(1, 0))),
            (8, Action::Light),
            (12, Action::Light),
            (16, Action::Light),
            (20, Action::Light),
            (24, Action::Light),
            (28, Action::Heavy),
            (32, Action::Cast(Sign::Axii)),
            (36, Action::Heavy),
            (40, Action::Light),
        ],
        Archetype::Rotmaw => vec![
            (1, Action::Move(Dir::new(1, 0))),
            (6, Action::Move(Dir::new(0, 1))), // toward hazard corner
            (12, Action::Move(Dir::new(1, 0))),
            (18, Action::Heavy),
            (24, Action::Light),
            (30, Action::Heavy),
            (36, Action::Light),
        ],
        Archetype::Caesura => vec![
            (1, Action::Move(Dir::new(1, 0))),
            (6, Action::Heavy),
            (10, Action::Parry), // anchor window
            (14, Action::Heavy),
            (18, Action::Light),
            (22, Action::Parry),
            (26, Action::Heavy),
        ],
        Archetype::PrismCzar => vec![
            (1, Action::Move(Dir::new(1, 0))),
            (3, Action::Move(Dir::new(1, 0))),
            (6, Action::Cast(Sign::Igni)),
            (10, Action::Heavy),
            (16, Action::Light),
            (20, Action::Cast(Sign::Aard)),
            (26, Action::Heavy),
            (32, Action::Light),
        ],
        Archetype::Sediment => vec![
            (1, Action::Move(Dir::new(1, 0))),
            (4, Action::Light),
            (8, Action::Heavy),
            (10, Action::Light),
            (16, Action::Heavy),
            (18, Action::Light),
        ],
        Archetype::Nullglyph => vec![
            (1, Action::Move(Dir::new(1, 0))),
            (6, Action::Cast(Sign::Igni)),
            (10, Action::Heavy),
            (16, Action::Light),
            (22, Action::Cast(Sign::Aard)),
            (28, Action::Heavy),
        ],
        _ => vec![
            (1, Action::Move(Dir::new(1, 0))),
            (8, Action::Heavy),
            (14, Action::Light),
        ],
    }
}

/// One interactive boss fight. Returns true if the player won.
fn fight_interactive(which: Archetype, rx: &mpsc::Receiver<u8>, hero: &str) -> bool {
    let mut g = BossFight::new(which);
    let dur = Duration::from_millis(20);
    let mut last_tick = Instant::now();
    while !g.won && !g.dead {
        while let Ok(b) = rx.try_recv() {
            if b == b'q' || b == b'Q' || b == 3 {
                return false;
            }
            if let Some(a) = key_to_action(b, g.player.face) {
                g.input(a);
            }
        }
        if last_tick.elapsed() >= Duration::from_millis(50) {
            g.tick();
            last_tick = Instant::now();
            draw(&g, hero);
        }
        thread::sleep(dur);
    }
    draw(&g, hero);
    g.won
}

fn teach_mode() -> bool {
    std::env::var("ASHWALKER_TEACH").is_ok() || std::env::var("ASHWALKER_DEMO").is_ok()
}

fn demo() {
    let fast = std::env::var("ASHWALKER_FAST").is_ok();
    let hero = hero_name();
    let spec = spectate();
    println!("\n══════ ASHWALKER · BOSS GAUNTLET (scripted{}) ══════\n", if spec { " · SPECTATOR" } else { "" });
    for which in pick() {
        let mut g = BossFight::new(which);
        g.feed.insert(0, BossFight::teach_lesson(which).into());
        let script = demo_script(which);
        let mut si = 0;
        let teach = teach_mode();
        println!("\n── {} ──", which.name());
        if teach {
            println!("  {}", BossFight::teach_lesson(which));
        }
        let max_t = if matches!(which, Archetype::PackOfNine) {
            400u64
        } else if matches!(which, Archetype::Audithollow) {
            220u64
        } else {
            140u64
        };
        for t in 0..max_t {
            while si < script.len() && script[si].0 == t {
                g.input(script[si].1);
                si += 1;
            }
            if teach && t > script.last().map(|x| x.0).unwrap_or(0) + 4 && !g.won && !g.dead {
                if let Some(a) = g.teach_action() {
                    g.input(a);
                }
            } else if t > script.last().map(|x| x.0).unwrap_or(0) + 8 && !g.won && !g.dead && t % 4 == 0 {
                g.input(Action::Light);
            }
            g.tick();
            if t % 2 == 0 || g.won || g.dead {
                if sigil_ashwalker::spectator::use_scroll_log() { println!("\n--- bout frame ---"); } else { print!("\x1b[2J\x1b[H"); }
                if spec {
                    print!("{}", spectator::render_boss(&g, &hero));
                } else {
                    print!("{}", boss_render(&g));
                    for l in g.feed.iter().rev().take(4).rev() {
                        println!("   · {l}");
                    }
                }
                let _ = std::io::stdout().flush();
                if !fast {
                    thread::sleep(Duration::from_millis(if spec { 120 } else { 90 }));
                }
            }
            if g.won || g.dead {
                break;
            }
        }
        println!(
            "\n  → {}: {}",
            which.name(),
            if g.won {
                "DOWN"
            } else if g.dead {
                "you fell"
            } else {
                "survived the bout"
            }
        );
    }
    println!("\n  Gauntlet teaches: Audithollow replays · Pack leaf-order · Czar attune · Rot stacks · Cæsura anchor");
}

fn main() {
    let hero = hero_name();
    if std::env::var("ASHWALKER_DEMO").is_ok() || !is_tty() {
        demo();
        return;
    }
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

    let bosses = pick();
    let mut won_all = true;
    for which in &bosses {
        let won = fight_interactive(*which, &rx, &hero);
        stty(&["sane"]);
        print!("\x1b[2J\x1b[H");
        println!("\n══════ {} — {} ══════", which.name(), if won { "DEFEATED" } else { "the fight ends" });
        if !won {
            won_all = false;
            break;
        }
        if bosses.len() > 1 {
            println!("  Next Crown of Ash approaches… (Q to stop)");
            thread::sleep(Duration::from_millis(1400));
        }
        stty(&["raw", "-echo"]);
    }
    stty(&["sane"]);
    if won_all && bosses.len() > 1 {
        println!("\n  ✦ The gauntlet falls. The gate to Crown & Ash opens. ✦");
    }
}