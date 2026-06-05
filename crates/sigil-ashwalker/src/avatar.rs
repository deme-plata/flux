//! avatar.rs — rich-text terminal portrait for a [`CharSheet`](crate::traits::CharSheet).
//!
//! Renders a procedural hooded-mage avatar in **24-bit ANSI colour** using unicode upper-half-blocks
//! (`▀`): each character cell shows TWO vertical pixels (fg = top, bg = bottom), so a 16×16 pixel
//! portrait becomes 8 text rows at full colour. The palette is derived from the character's seed-hue
//! and Origin, so every hero has a unique, reproducible face. No deps — just escape codes.

use crate::traits::CharSheet;

/// HSV→RGB (h in 0..360, s/v in 0..1).
fn hsv(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let h = h.rem_euclid(360.0);
    let c = v * s;
    let x = c * (1.0 - (((h / 60.0) % 2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h as i32 / 60) % 6 {
        0 => (c, x, 0.0), 1 => (x, c, 0.0), 2 => (0.0, c, x),
        3 => (0.0, x, c), 4 => (x, 0.0, c), _ => (c, 0.0, x),
    };
    (((r + m) * 255.0) as u8, ((g + m) * 255.0) as u8, ((b + m) * 255.0) as u8)
}

/// 16×16 region map of a hooded Sigil-wielder (symmetric).
/// `.`=void  H=hood  R=robe  T=collar-trim  F=face  E=eyes(glow)  S=sigil(glow)
const TEMPLATE: [&str; 16] = [
    "......HHHH......",
    ".....HHHHHH.....",
    "....HHHHHHHH....",
    "...HHHHHHHHHH...",
    "..HHHHHHHHHHHH..",
    "..HHHFFFFFFHHH..",
    "..HHFFFFFFFFHH..",
    "..HHFEEFFEEFHH..",
    "..HHFEEFFEEFHH..",
    "..HHFFFFFFFFHH..",
    "..HHHFFSSFFHHH..",
    "...HHHFFFFHHH...",
    "....TTTTTTTT....",
    "...RRRRRRRRRR...",
    "..RRRRSSSSRRRR..",
    "..RRRRRRRRRRRR..",
];

/// The seed-derived palette for this hero.
fn palette(sheet: &CharSheet) -> [(u8,u8,u8); 7] {
    let h = sheet.hue as f32;
    let hood   = hsv(h, 0.45, 0.55);
    let robe   = hsv(h, 0.58, 0.32);
    let trim   = hsv((h + 35.0) % 360.0, 0.70, 0.85);
    let face   = hsv(26.0, 0.42, 0.84);                 // warm skin
    let accent = hsv((h + 165.0) % 360.0, 0.95, 1.0);   // complementary glow (eyes + sigil)
    let void   = (20u8, 18, 28);                         // dark panel
    // index by the region order used in `region_color`
    [void, hood, robe, trim, face, accent, void]
}

fn region_color(ch: char, p: &[(u8,u8,u8); 7]) -> (u8,u8,u8) {
    match ch {
        'H' => p[1], 'R' => p[2], 'T' => p[3], 'F' => p[4],
        'E' | 'S' => p[5], _ => p[0],
    }
}

/// Render JUST the colour portrait (8 text rows, left-padded).
pub fn render_avatar(sheet: &CharSheet) -> String {
    let p = palette(sheet);
    let grid: Vec<Vec<char>> = TEMPLATE.iter().map(|r| r.chars().collect()).collect();
    let mut out = String::new();
    let mut row = 0;
    while row < 16 {
        out.push_str("    ");
        for col in 0..16 {
            let (tr, tg, tb) = region_color(grid[row][col], &p);
            let (br, bg, bb) = region_color(grid[row + 1][col], &p);
            out.push_str(&format!("\x1b[38;2;{tr};{tg};{tb}m\x1b[48;2;{br};{bg};{bb}m▀"));
        }
        out.push_str("\x1b[0m\n");
        row += 2;
    }
    out
}

/// The full creation screen: portrait + the character card beside/under it.
pub fn portrait_card(sheet: &CharSheet) -> String {
    let mut s = String::new();
    s.push_str(&format!("  ╔═══ ASHWALKER · CHARACTER CREATED ═══  seed {:#018x}\n", sheet.seed));
    s.push('\n');
    s.push_str(&render_avatar(sheet));
    s.push('\n');
    s.push_str(&sheet.card());
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::CharSheet;

    #[test]
    fn hsv_primaries() {
        assert_eq!(hsv(0.0, 1.0, 1.0), (255, 0, 0));
        assert_eq!(hsv(120.0, 1.0, 1.0), (0, 255, 0));
        assert_eq!(hsv(240.0, 1.0, 1.0), (0, 0, 255));
    }

    #[test]
    fn avatar_is_colored_block_art() {
        let a = render_avatar(&CharSheet::create("Rocky"));
        assert!(a.contains('▀'), "uses half-blocks");
        assert!(a.contains("\x1b[38;2;"), "uses 24-bit fg colour");
        assert!(a.contains("\x1b[48;2;"), "uses 24-bit bg colour");
        assert!(a.contains("\x1b[0m"), "resets colour per row");
        assert_eq!(a.matches('\n').count(), 8, "16 pixel rows → 8 half-block text rows");
    }

    #[test]
    fn deterministic_per_name_unique_across_names() {
        let r1 = render_avatar(&CharSheet::create("Rocky"));
        let r2 = render_avatar(&CharSheet::create("Rocky"));
        assert_eq!(r1, r2, "same name → same face");
        let v = render_avatar(&CharSheet::create("Viktor"));
        assert_ne!(r1, v, "different name → different palette/face");
    }

    #[test]
    fn portrait_card_includes_name_and_art() {
        let card = portrait_card(&CharSheet::create("Ashwalker"));
        assert!(card.contains("CHARACTER CREATED"));
        assert!(card.contains("Ashwalker"));
        assert!(card.contains('▀'));
    }
}
