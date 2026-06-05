//! wallet_xray — three new MCP combo buttons for AI agents working on the
//! SIGIL wallet (or any Vite/React project).
//!
//! Designed to solve the "CSS sledgehammer doesn't work" problem: most
//! visible colours in a React app are inline `style={{...}}` literals that
//! no override CSS can reach. These tools let an agent:
//!
//!   1. `flux_wallet_xray`    — scan the project, return a JSON map of
//!                              hardcoded colours + inline styles + tailwind
//!                              class frequency, so the agent KNOWS what
//!                              needs editing before touching anything.
//!   2. `flux_wallet_recolor` — apply a hex→hex palette map across `src/`
//!                              in one shot. Quillon → SIGIL is the default.
//!   3. `flux_wallet_components` — list every component with LOC + inline-
//!                                 style count + import count. Surfaces the
//!                                 "fat components" first.

use crate::handlers::{ToolDef, ToolRegistry};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_wallet_xray",
            description: "X-ray a Vite/React project for an AI editor. Walks the src tree and returns hardcoded hex colours, inline style counts, top Tailwind utility classes, and component file inventory — so you know WHAT to edit before reaching for the sledgehammer. Default root: sigil/gui/sigil-wallet.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "root":  { "type": "string", "description": "Project src dir (default /home/storage/deepseek-codewhale/sigil/gui/sigil-wallet/src)" },
                    "top_n": { "type": "integer", "description": "How many top items per category (default 40)" }
                }
            }),
        },
        flux_wallet_xray,
    );
    registry.register(
        ToolDef {
            name: "flux_wallet_recolor",
            description: "Surgical hex→hex palette swap across a Vite/React project. Default map is Quillon emerald/teal/cyan/amber → SIGIL violet/gold. Skips reds, greys, whites. Returns count of substitutions.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "root":   { "type": "string", "description": "Project src dir" },
                    "extra":  { "type": "object", "description": "Extra hex pairs to add to the default map (lowercase keys, no #)" },
                    "dry_run":{ "type": "boolean", "description": "If true, report what would change without writing (default false)" }
                }
            }),
        },
        flux_wallet_recolor,
    );
    registry.register(
        ToolDef {
            name: "flux_wallet_components",
            description: "List every component in a Vite/React project with LOC, import count, inline-style count, and tailwind utility count. Newest-first by edit time. Use to find fat components before refactoring.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "root":   { "type": "string", "description": "Project src dir" },
                    "min_loc":{ "type": "integer", "description": "Only return components with >= this many LOC (default 0)" },
                    "limit":  { "type": "integer", "description": "Max entries (default 40)" }
                }
            }),
        },
        flux_wallet_components,
    );
}

fn default_root() -> PathBuf {
    PathBuf::from("/home/storage/deepseek-codewhale/sigil/gui/sigil-wallet/src")
}

fn walk_tsx(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    fn rec(p: &Path, out: &mut Vec<PathBuf>) {
        if let Ok(rd) = fs::read_dir(p) {
            for ent in rd.flatten() {
                let path = ent.path();
                let ft = ent.file_type().ok();
                if ft.map(|t| t.is_dir()).unwrap_or(false) {
                    rec(&path, out);
                } else if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                    if ext == "tsx" || ext == "ts" {
                        out.push(path);
                    }
                }
            }
        }
    }
    rec(root, &mut out);
    out
}

// ── flux_wallet_xray ──────────────────────────────────────────────────

fn flux_wallet_xray(args: &Value) -> String {
    let root = args
        .get("root")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .unwrap_or_else(default_root);
    let top_n = args
        .get("top_n")
        .and_then(|v| v.as_u64())
        .unwrap_or(40) as usize;

    let files = walk_tsx(&root);
    let mut hex_count: BTreeMap<String, u32> = BTreeMap::new();
    let mut inline_style_files = 0u32;
    let mut inline_style_total = 0u32;
    let mut tw_count: BTreeMap<String, u32> = BTreeMap::new();
    let mut total_loc: u64 = 0;

    for p in &files {
        let Ok(src) = fs::read_to_string(p) else { continue };
        total_loc += src.lines().count() as u64;

        // hex colours (#abcdef)
        for cap in HEX_RE.captures_iter(&src) {
            let h = cap[1].to_lowercase();
            *hex_count.entry(h).or_default() += 1;
        }
        // inline style={{...}}
        let inl = INLINE_STYLE_RE.captures_iter(&src).count() as u32;
        if inl > 0 {
            inline_style_files += 1;
            inline_style_total += inl;
        }
        // Tailwind utility classes (bg-X-N / text-X-N / border-X-N)
        for cap in TW_RE.captures_iter(&src) {
            let c = format!("{}-{}-{}", &cap[1], &cap[2], &cap[3]);
            *tw_count.entry(c).or_default() += 1;
        }
    }

    let mut hex_sorted: Vec<(String, u32)> = hex_count.into_iter().collect();
    hex_sorted.sort_by(|a, b| b.1.cmp(&a.1));
    hex_sorted.truncate(top_n);

    let mut tw_sorted: Vec<(String, u32)> = tw_count.into_iter().collect();
    tw_sorted.sort_by(|a, b| b.1.cmp(&a.1));
    tw_sorted.truncate(top_n);

    let report = json!({
        "root": root.display().to_string(),
        "files_scanned": files.len(),
        "total_loc": total_loc,
        "inline_style_files": inline_style_files,
        "inline_style_total": inline_style_total,
        "top_hex_colors": hex_sorted.iter().map(|(h,n)| json!({ "hex": format!("#{}", h), "count": n })).collect::<Vec<_>>(),
        "top_tailwind_classes": tw_sorted.iter().map(|(c,n)| json!({ "class": c, "count": n })).collect::<Vec<_>>(),
    });
    let header = format!(
        "🔍 flux_wallet_xray — {} files, {} LOC scanned\n  inline-style sites: {} files / {} total\n  → top hex/tailwind below; use flux_wallet_recolor for batch substitution.\n\n",
        files.len(), total_loc, inline_style_files, inline_style_total
    );
    format!("{}{}", header, serde_json::to_string_pretty(&report).unwrap_or_default())
}

// ── flux_wallet_recolor ───────────────────────────────────────────────

fn default_palette_map() -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    // emerald/green → violet
    for (k, v) in [
        ("10b981","8b5cf6"),("34d399","c084fc"),("6ee7b7","c4b5fd"),
        ("059669","7c3aed"),("047857","6d28d9"),("065f46","4c1d95"),
        ("22c55e","8b5cf6"),("16a34a","7c3aed"),("4ade80","c084fc"),
        ("00e676","c084fc"),("00ff88","c084fc"),
        // teal → violet
        ("14b8a6","7c3aed"),("5eead4","a78bfa"),
        // cyan → bright violet
        ("06b6d4","8b5cf6"),("22d3ee","c084fc"),("67e8f9","d8b4fe"),
        ("00e5ff","c084fc"),("00d4ff","c084fc"),("00d9ff","c084fc"),("00ffff","c084fc"),
        // blue → deep violet
        ("3b82f6","7c3aed"),("2563eb","6d28d9"),("60a5fa","a78bfa"),("0080ff","7c3aed"),
        // amber/yellow → sigil gold
        ("ffd700","fbbf24"),("fcd34d","fbbf24"),("d4af37","fbbf24"),
        ("d4a017","fbbf24"),("b45309","d97706"),
        ("fb923c","f59e0b"),("ff6b35","f59e0b"),
        // other purples → harmonise
        ("7c4dff","8b5cf6"),("a855f7","8b5cf6"),("6b46c1","8b5cf6"),
    ] {
        m.insert(k.to_string(), v.to_string());
    }
    m
}

fn flux_wallet_recolor(args: &Value) -> String {
    let root = args
        .get("root")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .unwrap_or_else(default_root);
    let dry_run = args
        .get("dry_run")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let mut map = default_palette_map();
    if let Some(extra) = args.get("extra").and_then(|v| v.as_object()) {
        for (k, v) in extra {
            if let Some(s) = v.as_str() {
                map.insert(k.to_lowercase(), s.to_lowercase());
            }
        }
    }

    let files = walk_tsx(&root);
    let mut total_subs = 0u32;
    let mut files_touched = 0u32;
    let mut per_file: Vec<(String, u32)> = Vec::new();

    for p in &files {
        let Ok(src) = fs::read_to_string(p) else { continue };
        let mut count = 0u32;
        let new = HEX_RE.replace_all(&src, |caps: &regex::Captures| {
            let h = caps[1].to_lowercase();
            if let Some(target) = map.get(&h) {
                count += 1;
                format!("#{}", target)
            } else {
                caps[0].to_string()
            }
        });
        if count > 0 {
            files_touched += 1;
            total_subs += count;
            per_file.push((p.display().to_string(), count));
            if !dry_run {
                let _ = fs::write(p, new.as_bytes());
            }
        }
    }
    per_file.sort_by(|a, b| b.1.cmp(&a.1));
    per_file.truncate(20);

    let verdict = if dry_run { "🔬 dry-run" } else { "🎨 applied" };
    let mut s = format!(
        "{verdict} flux_wallet_recolor — {} substitutions across {} files (of {} scanned)\n  default map: Quillon emerald/teal/cyan/amber → SIGIL violet/gold\n  top 20 files by changes:\n",
        total_subs, files_touched, files.len()
    );
    for (f, n) in per_file {
        s.push_str(&format!("    {:>4}  {}\n", n, f));
    }
    s
}

// ── flux_wallet_components ────────────────────────────────────────────

fn flux_wallet_components(args: &Value) -> String {
    let root = args
        .get("root")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .unwrap_or_else(default_root);
    let min_loc = args
        .get("min_loc")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(40) as usize;

    let files = walk_tsx(&root);
    let mut rows: Vec<(String, usize, u32, u32, u32)> = Vec::new();
    for p in &files {
        let Ok(src) = fs::read_to_string(p) else { continue };
        let loc = src.lines().count();
        if loc < min_loc { continue }
        let imports = src.lines().filter(|l| l.trim_start().starts_with("import ")).count() as u32;
        let inline_styles = INLINE_STYLE_RE.captures_iter(&src).count() as u32;
        let tw_classes = TW_RE.captures_iter(&src).count() as u32;
        rows.push((
            p.strip_prefix(&root).unwrap_or(p).display().to_string(),
            loc, imports, inline_styles, tw_classes,
        ));
    }
    rows.sort_by(|a, b| b.1.cmp(&a.1));
    rows.truncate(limit);

    let mut s = format!(
        "🧩 flux_wallet_components — top {} (of {} scanned) by LOC\n  cols: LOC | imports | inline-styles | tailwind-classes\n\n",
        rows.len(), files.len()
    );
    for (path, loc, imp, inl, tw) in rows {
        s.push_str(&format!("  {:>5}  {:>3}  {:>4}  {:>5}   {}\n", loc, imp, inl, tw, path));
    }
    s
}

// ── shared regex (lazy_static-free; rebuild per call is cheap) ────────

lazy_static::lazy_static! {
    static ref HEX_RE: regex::Regex = regex::Regex::new(r"#([0-9a-fA-F]{6})\b").unwrap();
    static ref INLINE_STYLE_RE: regex::Regex = regex::Regex::new(r"style=\{\{[^}]+\}\}").unwrap();
    static ref TW_RE: regex::Regex = regex::Regex::new(
        r"\b(bg|text|border|ring|shadow|from|to|via)-(slate|gray|zinc|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose)-(50|100|200|300|400|500|600|700|800|900|950)"
    ).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_map_has_emerald_and_amber_targets() {
        let m = default_palette_map();
        assert_eq!(m.get("10b981").map(|s| s.as_str()), Some("8b5cf6"));
        assert_eq!(m.get("ffd700").map(|s| s.as_str()), Some("fbbf24"));
        assert_eq!(m.get("b45309").map(|s| s.as_str()), Some("d97706"));
        assert!(m.get("ef4444").is_none(), "red should not be remapped");
    }

    #[test]
    fn xray_handles_missing_root() {
        let r = flux_wallet_xray(&json!({"root": "/nonexistent-flux-xray"}));
        assert!(r.contains("0 files"), "got: {r}");
    }

    #[test]
    fn recolor_dry_run_reports_without_writing() {
        let dir = std::env::temp_dir().join(format!(
            "flux-wallet-xray-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_micros()).unwrap_or(0),
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("foo.tsx");
        std::fs::write(&p, "const c = '#10b981';").unwrap();
        let r = flux_wallet_recolor(&json!({"root": dir.display().to_string(), "dry_run": true}));
        assert!(r.contains("dry-run"));
        let after = std::fs::read_to_string(&p).unwrap();
        assert!(after.contains("10b981"), "dry-run wrote: {after}");
    }
}
