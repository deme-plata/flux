//! scaffold.rs — `flux_game_scaffold`: the ASHWALKER **mold stamp** (prototype-6 capstone).
//!
//! One call mints a fresh SIGIL terminal-ARPG crate from the mold — like `flux_chain_template` stamps
//! a chain. `scaffold_game(name)` returns the full file set (Cargo.toml + lib.rs + world/hero/combat/
//! boss skeletons + a bin + TEMPLATE.md), with **name-derived identity** (crate name, hue, intro) and
//! every generated file a compiling stub with a passing test — so the new crate is green on `cargo
//! check` from minute one. `write_scaffold` lays it on disk. Pure generation; std-only.

/// One file to write for the new crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScaffoldFile { pub path: String, pub content: String }

fn seed(name: &str) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in name.trim().to_lowercase().bytes() { h ^= b as u64; h = h.wrapping_mul(0x100000001b3); }
    h
}
fn title(name: &str) -> String {
    let n = name.trim();
    let mut c = n.chars();
    match c.next() { Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(), None => "Game".into() }
}
fn slug(name: &str) -> String {
    let s: String = name.trim().to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "game".into() } else { s }
}

/// Stamp a new ARPG crate from the mold. `name` drives the identity.
pub fn scaffold_game(name: &str) -> Vec<ScaffoldFile> {
    let t = title(name);
    let sl = slug(name);
    let krate = format!("sigil-{sl}");
    let hue = (seed(name) % 360) as u16;
    let f = |path: &str, content: String| ScaffoldFile { path: path.into(), content };

    vec![
        f("Cargo.toml", format!(
"[package]\nname = \"{krate}\"\nversion.workspace = true\nedition.workspace = true\nlicense.workspace = true\n\n\
# {t} — a SIGIL terminal ARPG, stamped from the ASHWALKER mold (flux_game_scaffold).\n\
# FLUXFOOD: std-only core. Build/test via flux_combo, never raw cargo.\n\n\
[[bin]]\nname = \"{sl}\"\npath = \"src/bin/{sl}.rs\"\n")),

        f("src/lib.rs", format!(
"//! {t} — a SIGIL Adventure (scaffolded prototype 1).\n//!\n\
//! Stamped from the ASHWALKER mold: 3D iso world, MCP-combo casting, a unique hero, a boss, and a\n\
//! gate that ascends into Crown & Ash. Fill the stubs cluster by cluster (see TEMPLATE.md).\n\n\
/// Seed-derived avatar hue for {t}.\npub const HUE: u16 = {hue};\npub const GAME: &str = \"{t}\";\n\n\
pub mod world;\npub mod hero;\npub mod combat;\npub mod boss;\n")),

        f("src/world.rs", format!(
"//! world — the {t} arena (3D grid; z is elevation). Grow into ramps/hazards/gate per the mold.\n\n\
#[derive(Debug, Clone, Copy, PartialEq, Eq)]\npub struct P {{ pub x: i32, pub y: i32, pub z: i32 }}\n\
#[derive(Debug, Clone)]\npub struct World {{ pub w: i32, pub h: i32 }}\n\
impl World {{ pub fn arena() -> World {{ World {{ w: 9, h: 9 }} }} pub fn in_bounds(&self, p: P) -> bool {{ p.x >= 0 && p.y >= 0 && p.x < self.w && p.y < self.h }} }}\n\n\
#[cfg(test)]\nmod tests {{ use super::*; #[test] fn arena_bounds() {{ let w = World::arena(); assert!(w.in_bounds(P{{x:1,y:1,z:0}})); assert!(!w.in_bounds(P{{x:-1,y:0,z:0}})); }} }}\n")),

        f("src/hero.rs", format!(
"//! hero — the {t} Sigil-wielder (deterministic from a name-seed; grow Origins/Traits per the mold).\n\n\
fn fnv(s: &str) -> u64 {{ let mut h=0xcbf29ce484222325u64; for b in s.bytes() {{ h ^= b as u64; h = h.wrapping_mul(0x100000001b3); }} h }}\n\
#[derive(Debug, Clone)]\npub struct Hero {{ pub name: String, pub seed: u64, pub hp: i32, pub sigil: i32 }}\n\
impl Hero {{ pub fn create(name: &str) -> Hero {{ let seed = fnv(name); Hero {{ name: name.into(), seed, hp: 60, sigil: 30 }} }} }}\n\n\
#[cfg(test)]\nmod tests {{ use super::*; #[test] fn deterministic() {{ assert_eq!(Hero::create(\"x\").seed, Hero::create(\"x\").seed); assert!(Hero::create(\"x\").hp > 0); }} }}\n")),

        f("src/combat.rs", format!(
"//! combat — {t} MCP-combo casting. Each spell is an MCP tool name; chaining two FUSES them.\n\n\
#[derive(Debug, Clone, Copy, PartialEq, Eq)]\npub enum Mcp {{ FluxCombo, DexSwap, ZkVeil }}\n\
impl Mcp {{ pub fn name(self) -> &'static str {{ match self {{ Mcp::FluxCombo=>\"flux_combo\", Mcp::DexSwap=>\"dex_swap\", Mcp::ZkVeil=>\"flux_zk_combo\" }} }} }}\n\
/// fuse two MCP tools into a combo damage value (stronger than either alone).\n\
pub fn combo(a: Mcp, b: Mcp) -> i32 {{ let base = |m: Mcp| match m {{ Mcp::FluxCombo=>12, Mcp::DexSwap=>6, Mcp::ZkVeil=>0 }}; base(a) + base(b) + 6 }}\n\n\
#[cfg(test)]\nmod tests {{ use super::*; #[test] fn combo_beats_singles() {{ assert!(combo(Mcp::FluxCombo, Mcp::DexSwap) > 12); }} }}\n")),

        f("src/boss.rs", format!(
"//! boss — the {t} gatekeeper. Grow into the Merkle boss-gen + adaptive AI per the mold.\n\n\
#[derive(Debug, Clone)]\npub struct Boss {{ pub name: String, pub hp: i32 }}\n\
impl Boss {{ pub fn gatekeeper(root: u64) -> Boss {{ Boss {{ name: format!(\"Warden-{{:04x}}\", root & 0xffff), hp: 80 + (root % 80) as i32 }} }} pub fn alive(&self) -> bool {{ self.hp > 0 }} }}\n\n\
#[cfg(test)]\nmod tests {{ use super::*; #[test] fn boss_from_root() {{ let b = Boss::gatekeeper(0x1234); assert!(b.hp > 0 && b.alive()); }} }}\n")),

        f(&format!("src/bin/{sl}.rs"), format!(
"//! {t} — scripted demo bout. Grow into the live loop per the mold.\nuse {ck}::*;\n\n\
fn main() {{\n    println!(\"\\n══════ {{}} — a SIGIL Adventure ══════\", GAME);\n    let h = hero::Hero::create(\"Ashwalker\");\n    let mut b = boss::Boss::gatekeeper(h.seed);\n    println!(\"{{}} (hp {{}}, sigil {{}}) faces {{}} (hp {{}})\", h.name, h.hp, h.sigil, b.name, b.hp);\n    let dmg = combat::combo(combat::Mcp::FluxCombo, combat::Mcp::DexSwap);\n    while b.alive() {{ b.hp -= dmg; println!(\"  combo for {{}} → {{}} hp\", dmg, b.hp.max(0)); }}\n    println!(\"VICTORY — the gate opens. (hue {{}})\", HUE);\n}}\n",
            ck = krate.replace('-', "_"))),

        f("TEMPLATE.md", format!(
"# {t} — SIGIL ARPG (scaffolded from the ASHWALKER mold)\n\n\
Crate `{krate}`, hue {hue}. Stamped by `flux_game_scaffold`. Grow the 10 clusters (Character/Body ·\n\
MCP-Combat · World · Enemies/Bosses · Items · Companions · Progression · Multiplayer · Skill/AI ·\n\
Agent/Render) — see the ASHWALKER mold. FLUXFOOD std-only; verify with flux_combo.\n")),
    ]
}

/// Write a scaffold to `root` (creates dirs). Returns the paths written.
pub fn write_scaffold(root: &str, files: &[ScaffoldFile]) -> std::io::Result<Vec<String>> {
    let mut out = Vec::new();
    for f in files {
        let path = format!("{}/{}", root.trim_end_matches('/'), f.path);
        if let Some(parent) = std::path::Path::new(&path).parent() { std::fs::create_dir_all(parent)?; }
        std::fs::write(&path, &f.content)?;
        out.push(path);
    }
    Ok(out)
}

/// One-glance summary of what a stamp would produce (for an MCP/CLI dry-run).
pub fn render_plan(name: &str, files: &[ScaffoldFile]) -> String {
    let mut s = format!("🎮 flux_game_scaffold → sigil-{} ({} files)\n", slug(name), files.len());
    for f in files { s.push_str(&format!("  + {} ({} bytes)\n", f.path, f.content.len())); }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_a_full_crate() {
        let files = scaffold_game("Emberfall");
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"Cargo.toml"));
        assert!(paths.contains(&"src/lib.rs"));
        assert!(paths.iter().any(|p| p.starts_with("src/bin/")));
        assert!(paths.contains(&"TEMPLATE.md"));
        assert!(files.len() >= 7, "Cargo + lib + 4 modules + bin + template");
    }

    #[test]
    fn name_derives_identity() {
        let f = scaffold_game("Emberfall");
        let cargo = &f.iter().find(|x| x.path == "Cargo.toml").unwrap().content;
        assert!(cargo.contains("name = \"sigil-emberfall\""), "crate name from the game name");
        let lib = &f.iter().find(|x| x.path == "src/lib.rs").unwrap().content;
        assert!(lib.contains("GAME: &str = \"Emberfall\""));
        // bin path matches the slug
        assert!(f.iter().any(|x| x.path == "src/bin/emberfall.rs"));
    }

    #[test]
    fn deterministic_same_name_same_stamp() {
        assert_eq!(scaffold_game("Nyxgate"), scaffold_game("Nyxgate"));
        // different names → different crate + hue
        let a = scaffold_game("Nyxgate"); let b = scaffold_game("Ashreach");
        assert_ne!(a, b);
    }

    #[test]
    fn generated_modules_look_compilable() {
        let f = scaffold_game("Test");
        for m in ["src/world.rs", "src/hero.rs", "src/combat.rs", "src/boss.rs"] {
            let c = &f.iter().find(|x| x.path == m).unwrap().content;
            // each stub carries a pub item + a #[test] so the fresh crate is green on check
            assert!(c.contains("pub "), "{m} has a public API");
            assert!(c.contains("#[test]"), "{m} ships a test");
        }
    }

    #[test]
    fn write_then_read_back() {
        let files = scaffold_game("Tmpgame");
        let dir = std::env::temp_dir().join(format!("scaffold-test-{}", std::process::id()));
        let dirs = dir.to_string_lossy().to_string();
        let written = write_scaffold(&dirs, &files).expect("write");
        assert_eq!(written.len(), files.len());
        let cargo = std::fs::read_to_string(format!("{dirs}/Cargo.toml")).unwrap();
        assert!(cargo.contains("sigil-tmpgame"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plan_lists_files() {
        let p = render_plan("Emberfall", &scaffold_game("Emberfall"));
        assert!(p.contains("sigil-emberfall") && p.contains("Cargo.toml"));
    }
}
