//! ASHWALKER demo — a scripted auto-playthrough. Renders the isometric 3D Ashlands each turn,
//! casts MCP combos to clear the foes, ramps up to the high platform, and ascends into Crown & Ash.
//!   flux-cargo-wrapper run -p sigil-ashwalker --bin ashwalker
//!   (ASHWALKER_FAST=1 skips the cinematic frame delay)
use sigil_ashwalker::*;
use std::{thread, time::Duration};

fn frame_delay() {
    if std::env::var("ASHWALKER_FAST").is_err() { thread::sleep(Duration::from_millis(280)); }
}

/// Greedy one-step move that reduces 3D reach to `goal` (routes via the ramp when going up).
fn approach(g: &mut Game, goal: V3) {
    // if we need to climb and aren't on the platform yet, head for the ramp foot first
    let aim = if g.player.pos.z < goal.z && g.player.pos.z == 0 { V3::new(5,5,0) } else { goal };
    let best = STEPS8.iter().copied()
        .min_by_key(|&(dx,dy)| g.player.pos.add(V3::new(dx,dy,0)).reach(aim));
    if let Some((dx,dy)) = best { g.step(dx, dy); }
}

fn nearest_foe_pos(g: &Game) -> Option<V3> {
    g.enemies.iter().filter(|e| e.alive() && !e.ally)
        .min_by_key(|e| g.player.pos.reach(e.pos)).map(|e| e.pos)
}

fn show(g: &Game) { print!("\x1b[2J\x1b[H"); println!("{}", render(g)); for l in g.log.iter().rev().take(4).rev() { println!("   · {l}"); } frame_delay(); }

fn main() {
    // ── character creation: a deterministic, unique hero from a name + a rich-text avatar ──
    let name = std::env::var("ASHWALKER_NAME").unwrap_or_else(|_| "Ashwalker".into());
    let sheet = traits::CharSheet::create(&name);
    println!("\n══════ ASHWALKER — a SIGIL Adventure ══════");
    print!("{}", avatar::portrait_card(&sheet));
    println!("  (set ASHWALKER_NAME=<name> to roll a different hero — every name is a unique Sigil-wielder)\n");

    let mut g = Game::new();
    println!("Move in 3D through the Ashlands · cast MCP combos · reach the gate · ascend into Crown & Ash\n");
    show(&g);

    let mut turn = 0;
    while turn < 40 && g.player.alive() {
        turn += 1;
        if !g.won() {
            // close on the nearest foe, then unleash an MCP combo
            if let Some(fp) = nearest_foe_pos(&g) {
                if g.player.pos.reach(fp) > 1 { approach(&mut g, fp); }
                // pick a combo by situation: heal-combo when hurt, else damage combos cycling
                if g.player.hp < g.player.max_hp / 3 && g.player.sigil >= Mcp::ZkVeil.cost()+Mcp::CouncilQuorum.cost() {
                    g.combo(Mcp::ZkVeil, Mcp::CouncilQuorum, None);
                } else if g.player.sigil >= Mcp::DexSwap.cost()+Mcp::FluxCombo.cost() && turn % 3 == 1 {
                    g.combo(Mcp::DexSwap, Mcp::FluxCombo, None);      // Blink-Nova
                } else if g.player.sigil >= Mcp::Hashstorm.cost()+Mcp::FluxCombo.cost() && turn % 3 == 2 {
                    g.combo(Mcp::Hashstorm, Mcp::FluxCombo, None);    // Hashfire
                } else if g.player.sigil >= Mcp::Hashstorm.cost() {
                    g.cast(Mcp::Hashstorm, None);                     // single burn to regen sigil
                } else {
                    g.cast(Mcp::ZkVeil, None);                        // ward up + bank sigil
                }
            }
        } else {
            // foes cleared — walk to the gate (climbs the ramp to z=1)
            let gate = g.world.gate;
            if g.player.pos.reach(gate) == 0 { break; }
            approach(&mut g, gate);
            if g.player.pos == gate { break; }
        }
        g.end_turn();
        show(&g);
    }

    println!("\n══════ {} ══════", if g.player.alive() && g.won() { "VICTORY — the gate opens" } else if !g.player.alive() { "FALLEN in the Ashlands" } else { "the run ends" });
    if g.player.alive() && g.won() {
        let a = g.ascend();
        println!("\n{}\n", a.blurb);
        println!("  ┌─ CROWN & ASH ASCENSION ─────────────────────────────");
        println!("  │ House : {}", a.house);
        println!("  │ Title : {}", a.title);
        println!("  │ Crown : {}   Ash : {}   Levies : {}", a.crown, a.ash, a.troops);
        println!("  └─ seed → {}", a.seed_line());
        println!("\n  (prototype 2: this seed boots a House in the Crown & Ash realm sim.)");
    } else {
        println!("  slain {} · combos {} · the Ashlands keep their gate.", g.player.slain, g.player.combos_cast);
    }
}
