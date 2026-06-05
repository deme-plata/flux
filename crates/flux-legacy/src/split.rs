//! flux-legacy PROTOTYPE 2 — the ACTUATOR. P1 (analyze/plan/render) tells you *what* to refactor;
//! P2 *does* the #1 move: it splits a god-file into cohesive modules and emits a concrete, applyable
//! **dry-run patch** (new module files + parent `mod` wiring) — the original is never touched until a
//! human applies the staged patch.
//!
//! Grouping is by **item-name prefix** (the first snake_case / type token), a general heuristic that
//! works on any brownfield file. We deliberately do NOT use `flux_refactor::handler_extract::group_tools`:
//! that helper is hardcoded to flux's own `flux_*` tool names (v0.1 stub) and returns empty on a
//! q-api-server handler — leaning on it would be a silent no-op. This is real, if simple, structure.
//!
//! HONEST SCOPE (what's still pretend): this partitions top-level items and wires modules; it prepends
//! the file's `use` header + `use super::*;` to each module so most paths resolve, but it does NOT
//! rewrite visibility or fix every import — the staged split is a compiling-shaped *starting point* a
//! human reviews, not a guaranteed green build. That's why it stages to a side dir and defaults to dry-run.

use serde::{Deserialize, Serialize};

/// Top-level item kinds we recognize when partitioning a god-file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ItemKind { Fn, Struct, Enum, Trait, Impl, Mod, TypeAlias, Const, Other }

/// One top-level item with its full source span (attributes/doc-comments attached).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    pub kind: ItemKind,
    pub name: String,
    pub src: String,
    pub loc: usize,
}

/// One proposed module file in the split.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleSplit {
    pub module: String,
    pub item_names: Vec<String>,
    /// full generated module-file contents (header + items)
    pub src: String,
    pub loc: usize,
}

/// The complete dry-run patch for splitting one god-file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SplitPatch {
    pub original_file: String,
    pub original_loc: usize,
    pub items_total: usize,
    pub modules: Vec<ModuleSplit>,
    /// the `mod a; mod b;` lines to add where the god-file's contents used to live
    pub mod_wiring: String,
    pub strategy: String,
    pub caveats: Vec<String>,
}

// ───────────────────────── parsing ─────────────────────────

fn strip_vis(t: &str) -> &str {
    let t = t.trim_start();
    for p in ["pub(crate) ", "pub(super) ", "pub(self) ", "pub "] {
        if let Some(r) = t.strip_prefix(p) {
            return r.trim_start();
        }
    }
    t
}

fn item_kind(line: &str) -> Option<ItemKind> {
    let t = strip_vis(line);
    let t = t.strip_prefix("async ").unwrap_or(t);
    let t = t.strip_prefix("unsafe ").unwrap_or(t);
    use ItemKind::*;
    for (kw, k) in [
        ("fn ", Fn), ("struct ", Struct), ("enum ", Enum), ("trait ", Trait),
        ("impl ", Impl), ("impl<", Impl), ("mod ", Mod), ("type ", TypeAlias),
        ("const ", Const), ("static ", Const),
    ] {
        if t.starts_with(kw) {
            return Some(k);
        }
    }
    None
}

fn ident(s: &str) -> String {
    s.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect()
}

fn item_name(line: &str, kind: ItemKind) -> String {
    let t = strip_vis(line);
    let t = t.strip_prefix("async ").unwrap_or(t);
    let t = t.strip_prefix("unsafe ").unwrap_or(t);
    if kind == ItemKind::Impl {
        // "impl Foo {" or "impl<T> Trait for Foo {" → the implemented TYPE (after `for`, else after impl)
        let after = t.trim_start_matches("impl").trim_start();
        let after = after.split_once(" for ").map(|(_, r)| r).unwrap_or(after);
        let after = after.trim_start_matches('<'); // skip generics start if any
        return ident(after.trim_start_matches(|c: char| !c.is_alphanumeric() && c != '_'));
    }
    let kw_end = t.find(char::is_whitespace).map(|i| i + 1).unwrap_or(0);
    ident(t[kw_end..].trim_start())
}

/// Lightweight top-level item scan: brace-depth tracking, attributes/doc-comments attach to the next
/// item, and bare `use`/`extern crate` lines are collected as the shared file header.
/// Returns (header, items).
///
/// Brace tracking is **lexer-aware**: braces inside `//` and `/* */` comments, `"…"`/`r#"…"#`
/// strings, and `'…'` char literals are NOT counted — that's what previously mis-bounded items on
/// real code (the precheck lane caught it on handlers.rs). Lifetimes (`'a`) are distinguished from
/// char literals so a brace right after one still counts.
/// Carries comment/raw-string state across lines via [`LexState`].
#[derive(Default)]
struct LexState {
    in_block_comment: bool,
    /// `Some(n)` while inside a raw string opened with `n` hashes (`r#"…"#`); ends at `"` + n `#`.
    raw_hashes: Option<usize>,
}

/// Is `b` (starting at a `'`) a char literal (`'x'`, `'\n'`, `'{'`) rather than a lifetime (`'a`)?
fn is_char_literal(b: &[u8]) -> bool {
    if b.len() < 2 {
        return false;
    }
    if b[1] == b'\\' {
        return true; // escape → always a char literal
    }
    b.len() >= 3 && b[2] == b'\'' // 'x' shape; otherwise it's a lifetime
}

/// Net code-context brace delta for one line (opens − closes), plus whether a `{` and a top-level `;`
/// were seen in code. Updates `st` for multi-line comments / raw strings.
fn scan_line(line: &str, st: &mut LexState) -> (i32, bool, bool) {
    let b = line.as_bytes();
    let mut i = 0;
    let (mut depth, mut saw_open, mut saw_semi) = (0i32, false, false);
    let (mut in_str, mut in_char) = (false, false);
    while i < b.len() {
        let c = b[i];
        if st.in_block_comment {
            if c == b'*' && i + 1 < b.len() && b[i + 1] == b'/' {
                st.in_block_comment = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if let Some(h) = st.raw_hashes {
            if c == b'"' && (0..h).all(|k| i + 1 + k < b.len() && b[i + 1 + k] == b'#') {
                st.raw_hashes = None;
                i += 1 + h;
                continue;
            }
            i += 1;
            continue;
        }
        if in_str {
            if c == b'\\' { i += 2; continue; }
            if c == b'"' { in_str = false; }
            i += 1;
            continue;
        }
        if in_char {
            if c == b'\\' { i += 2; continue; }
            if c == b'\'' { in_char = false; }
            i += 1;
            continue;
        }
        // code context
        if c == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            break; // line comment → ignore the rest
        }
        if c == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            st.in_block_comment = true;
            i += 2;
            continue;
        }
        if c == b'r' && i + 1 < b.len() && (b[i + 1] == b'"' || b[i + 1] == b'#') {
            let mut k = i + 1;
            let mut h = 0;
            while k < b.len() && b[k] == b'#' { h += 1; k += 1; }
            if k < b.len() && b[k] == b'"' {
                st.raw_hashes = Some(h);
                i = k + 1;
                continue;
            }
        }
        if c == b'"' { in_str = true; i += 1; continue; }
        if c == b'\'' {
            if is_char_literal(&b[i..]) { in_char = true; }
            i += 1;
            continue; // lifetime or char-open: skip the quote either way
        }
        match c {
            b'{' => { depth += 1; saw_open = true; }
            b'}' => depth -= 1,
            b';' => saw_semi = true,
            _ => {}
        }
        i += 1;
    }
    (depth, saw_open, saw_semi)
}

/// Net brace balance of `src` counting only CODE context — braces inside strings/chars/comments are
/// ignored. `0` = balanced. This is the lexer-aware count the `precheck` lane must use instead of a
/// raw `matches('{')` (a `format!("}}")` or `"{"` in a string is not a real unbalanced brace).
pub fn code_brace_balance(src: &str) -> i32 {
    let mut st = LexState::default();
    src.lines().map(|l| scan_line(l, &mut st).0).sum()
}

pub fn parse_items(src: &str) -> (String, Vec<Item>) {
    let lines: Vec<&str> = src.lines().collect();
    let mut items = Vec::new();
    let mut header = String::new();
    let mut preamble: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let tl = line.trim_start();
        // collect shared imports — consume the WHOLE (possibly multi-line) `use …;` block so its
        // continuation lines (`extract::{Path, State},` / closing `};`) don't leak into the next
        // item's preamble and unbalance its braces.
        if tl.starts_with("use ") || tl.starts_with("extern crate ") {
            let mut lex = LexState::default();
            loop {
                header.push_str(lines[i]);
                header.push('\n');
                let (_d, _o, had_semi) = scan_line(lines[i], &mut lex);
                if had_semi || i + 1 >= lines.len() {
                    break;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        // buffer attributes / doc comments / blank lines for the next item
        if tl.starts_with("#[") || tl.starts_with("#!") || tl.starts_with("///") || tl.starts_with("//!") || tl.starts_with("//") || tl.is_empty() {
            preamble.push(line.to_string());
            i += 1;
            continue;
        }
        if let Some(kind) = item_kind(tl) {
            let name = item_name(tl, kind);
            let start = i;
            // Walk to the item's end with the lexer-aware tracker: a brace-delimited item ends when
            // depth returns to 0 after opening; a semicolon item (struct;/type=/const=) ends at the
            // first top-level `;` seen before any brace opens. Braces in strings/chars/comments don't count.
            let mut lex = LexState::default();
            let mut depth = 0i32;
            let mut seen_open = false;
            loop {
                let (delta, had_open, had_semi) = scan_line(lines[i], &mut lex);
                depth += delta;
                if had_open {
                    seen_open = true;
                }
                if seen_open && depth <= 0 {
                    break;
                }
                if !seen_open && had_semi {
                    break;
                }
                if i + 1 >= lines.len() {
                    break;
                }
                i += 1;
            }
            let mut body = String::new();
            if !preamble.is_empty() {
                // attach buffered attrs/docs (trim leading blanks)
                while preamble.first().map(|l| l.trim().is_empty()).unwrap_or(false) {
                    preamble.remove(0);
                }
                for p in &preamble {
                    body.push_str(p);
                    body.push('\n');
                }
            }
            for l in &lines[start..=i] {
                body.push_str(l);
                body.push('\n');
            }
            let loc = body.lines().count();
            items.push(Item { kind, name: if name.is_empty() { "anon".into() } else { name }, src: body, loc });
            preamble.clear();
            i += 1;
            continue;
        }
        // A stray top-level line that OPENS a brace block (a macro invocation like
        // `thread_local! { … }`, `lazy_static! { … }`) must be consumed as a whole block — otherwise
        // its inner `static`/`fn`/`const` lines get mis-read as top-level items and the macro's closing
        // brace is orphaned (the systematic off-by-one the precheck caught on handlers.rs).
        let mut lex = LexState::default();
        let (delta, had_open, _) = scan_line(lines[i], &mut lex);
        if had_open && delta > 0 {
            let start = i;
            let mut depth = delta;
            while depth > 0 && i + 1 < lines.len() {
                i += 1;
                depth += scan_line(lines[i], &mut lex).0;
            }
            let mut body = String::new();
            for p in preamble.drain(..) {
                body.push_str(&p);
                body.push('\n');
            }
            for l in &lines[start..=i] {
                body.push_str(l);
                body.push('\n');
            }
            let loc = body.lines().count();
            items.push(Item { kind: ItemKind::Other, name: "macro_block".into(), src: body, loc });
            i += 1;
            continue;
        }
        // a stray non-brace line — keep it buffered so nothing is lost
        preamble.push(line.to_string());
        i += 1;
    }
    (header, items)
}

// ───────────────────────── grouping + patch ─────────────────────────

/// First cohesion token of an item name: `handle_send_qug` → `handle`, `BlockHeader` → `block`.
fn prefix(name: &str) -> String {
    if name.contains('_') {
        name.split('_').next().unwrap_or(name).to_lowercase()
    } else {
        // CamelCase → first lowercased word (up to the 2nd uppercase)
        let mut out = String::new();
        for (i, c) in name.chars().enumerate() {
            if i > 0 && c.is_uppercase() {
                break;
            }
            out.push(c.to_ascii_lowercase());
        }
        if out.is_empty() { name.to_lowercase() } else { out }
    }
}

/// Plan a god-file split: parse items, group by name prefix, merge smallest groups until `max_modules`,
/// and emit each module's full source (shared header + `use super::*;` + items). Pure — no I/O.
pub fn plan_split(file_path: &str, src: &str, max_modules: usize) -> SplitPatch {
    let (header, items) = parse_items(src);
    let original_loc = src.lines().count();
    let max_modules = max_modules.max(1);

    // bucket item indices by prefix, preserving first-seen order
    let mut order: Vec<String> = Vec::new();
    let mut buckets: std::collections::HashMap<String, Vec<usize>> = std::collections::HashMap::new();
    for (idx, it) in items.iter().enumerate() {
        let p = prefix(&it.name);
        buckets.entry(p.clone()).or_default().push(idx);
        if !order.contains(&p) {
            order.push(p);
        }
    }
    // merge the smallest buckets into the largest until we're within max_modules
    while order.len() > max_modules {
        order.sort_by_key(|p| buckets[p].len());
        let smallest = order.remove(0);
        // fold into the now-smallest remaining bucket (keeps sizes balanced)
        let moved = buckets.remove(&smallest).unwrap_or_default();
        let target = order.iter().min_by_key(|p| buckets[*p].len()).cloned().unwrap_or_else(|| {
            order.push("core".to_string());
            "core".to_string()
        });
        buckets.entry(target).or_default().extend(moved);
    }

    let mod_base = std::path::Path::new(file_path)
        .file_stem().and_then(|s| s.to_str()).unwrap_or("mod").to_string();

    let mut modules = Vec::new();
    let mut wiring = String::new();
    for p in &order {
        let idxs = match buckets.get(p) { Some(v) if !v.is_empty() => v, _ => continue };
        let module = format!("{mod_base}_{p}");
        let mut s = String::new();
        s.push_str(&format!("//! {module} — split from {file_path} by flux-legacy (prefix `{p}`).\n"));
        if !header.trim().is_empty() {
            s.push_str(&header);
        }
        s.push_str("use super::*;\n\n");
        let mut names = Vec::new();
        for &i in idxs {
            s.push_str(items[i].src.trim_end());
            s.push_str("\n\n");
            names.push(items[i].name.clone());
        }
        let loc = s.lines().count();
        wiring.push_str(&format!("mod {module};\n"));
        modules.push(ModuleSplit { module, item_names: names, src: s, loc });
    }

    let caveats = vec![
        "DRY-RUN: originals untouched; review before applying.".to_string(),
        "Imports/visibility not rewritten — each module gets the file `use` header + `use super::*;`; some items may need `pub` bumps.".to_string(),
        "impl blocks are grouped by their type's prefix, which may differ from where the type is defined.".to_string(),
    ];
    SplitPatch {
        original_file: file_path.to_string(),
        original_loc,
        items_total: items.len(),
        modules,
        mod_wiring: wiring,
        strategy: format!("group-by-name-prefix → ≤{max_modules} modules"),
        caveats,
    }
}

/// Human dry-run view of a [`SplitPatch`].
pub fn render_patch(p: &SplitPatch) -> String {
    let mut o = String::new();
    o.push_str(&format!(
        "⟳ SPLIT PLAN · {} ({} LOC, {} items) · {}\n",
        p.original_file, p.original_loc, p.items_total, p.strategy
    ));
    for m in &p.modules {
        o.push_str(&format!(
            "  → {:<28} {:>4} LOC · {} items: {}\n",
            format!("{}.rs", m.module),
            m.loc,
            m.item_names.len(),
            {
                let shown: Vec<String> = m.item_names.iter().take(6).cloned().collect();
                let mut s = shown.join(", ");
                if m.item_names.len() > 6 { s.push_str(&format!(", +{}", m.item_names.len() - 6)); }
                s
            }
        ));
    }
    o.push_str("  wiring:\n");
    for l in p.mod_wiring.lines() {
        o.push_str(&format!("    {l}\n"));
    }
    o.push_str("  caveats:\n");
    for c in &p.caveats {
        o.push_str(&format!("    • {c}\n"));
    }
    o
}

/// Stage the patch to disk (the APPLY step): writes each module file under
/// `<staging_root>/<module>.rs`, plus a `MOD_WIRING.txt`. Originals are NOT modified. Returns the
/// paths written. Call only after a human has read [`render_patch`].
pub fn stage_patch(staging_root: &str, p: &SplitPatch) -> std::io::Result<Vec<String>> {
    std::fs::create_dir_all(staging_root)?;
    let mut written = Vec::new();
    for m in &p.modules {
        let path = format!("{staging_root}/{}.rs", m.module);
        std::fs::write(&path, &m.src)?;
        written.push(path);
    }
    let wiring_path = format!("{staging_root}/MOD_WIRING.txt");
    std::fs::write(&wiring_path, &p.mod_wiring)?;
    written.push(wiring_path);
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOD: &str = r#"
use std::collections::HashMap;
use serde::Serialize;

/// send money
pub fn handle_send_qug(to: &str) -> bool { to.starts_with("qnk") }

pub fn handle_send_token(to: &str, t: &str) -> bool { !to.is_empty() && !t.is_empty() }

#[derive(Serialize)]
pub struct BlockHeader { pub height: u64 }

pub struct BlockBody { pub txs: Vec<String> }

pub fn verify_block(h: u64) -> bool { h > 0 }

impl BlockHeader {
    pub fn new(height: u64) -> Self { Self { height } }
}

pub enum BlockError { Bad, Worse }

pub fn handle_balance(addr: &str) -> u64 { addr.len() as u64 }
"#;

    #[test]
    fn parses_all_top_level_items() {
        let (header, items) = parse_items(GOD);
        assert!(header.contains("use std::collections::HashMap"));
        assert!(header.contains("use serde::Serialize"));
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        for expect in ["handle_send_qug", "handle_send_token", "BlockHeader", "BlockBody", "verify_block", "BlockError", "handle_balance"] {
            assert!(names.contains(&expect), "missing item {expect}; got {names:?}");
        }
        // the impl block is captured too (named by its type)
        assert!(items.iter().any(|i| i.kind == ItemKind::Impl && i.name == "BlockHeader"));
    }

    #[test]
    fn brace_aware_parsing_ignores_braces_in_strings_chars_comments() {
        let src = "pub fn tricky() {\n\
                   \x20   let s = \"a } brace in a string {\";\n\
                   \x20   let c = '}';\n\
                   \x20   // a } in a comment {\n\
                   \x20   /* block } comment { */\n\
                   \x20   let r = r#\"raw } string {\"#;\n\
                   }\n\
                   pub fn after() -> u32 { 42 }\n";
        let (_h, items) = parse_items(src);
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert!(names.contains(&"tricky"), "got {names:?}");
        // the item AFTER the brace-trap fn must be found — proof the first item was bounded correctly
        assert!(names.contains(&"after"), "item after the brace-trap fn not found; got {names:?}");
        // every extracted item has balanced CODE braces (what the precheck lane checks)
        for it in &items {
            let mut st = LexState::default();
            let depth: i32 = it.src.lines().map(|l| scan_line(l, &mut st).0).sum();
            assert_eq!(depth, 0, "item `{}` has unbalanced code braces", it.name);
        }
    }

    #[test]
    fn split_groups_by_prefix_and_loses_no_item() {
        let p = plan_split("crates/q-api-server/src/handlers.rs", GOD, 4);
        // every parsed item appears in exactly one module
        let mut placed: Vec<String> = p.modules.iter().flat_map(|m| m.item_names.clone()).collect();
        placed.sort();
        assert_eq!(placed.len(), p.items_total, "no item dropped or duplicated");
        // the two handle_* fns should land together (prefix `handle`)
        let handle_mod = p.modules.iter().find(|m| m.module.ends_with("_handle")).expect("a handle module");
        assert!(handle_mod.item_names.contains(&"handle_send_qug".to_string()));
        assert!(handle_mod.item_names.contains(&"handle_send_token".to_string()));
        // module files carry the shared header + super import
        assert!(handle_mod.src.contains("use super::*;"));
        assert!(handle_mod.src.contains("use std::collections::HashMap"));
    }

    #[test]
    fn respects_max_modules_cap() {
        let p = plan_split("handlers.rs", GOD, 2);
        assert!(p.modules.len() <= 2, "merged to ≤2 modules, got {}", p.modules.len());
        // still no item lost after merging
        let placed: usize = p.modules.iter().map(|m| m.item_names.len()).sum();
        assert_eq!(placed, p.items_total);
    }

    #[test]
    fn render_and_wiring_present() {
        let p = plan_split("handlers.rs", GOD, 3);
        let txt = render_patch(&p);
        assert!(txt.contains("SPLIT PLAN"));
        assert!(txt.contains("caveats"));
        assert!(p.mod_wiring.contains("mod handlers_"));
    }

    #[test]
    fn stage_writes_files_without_touching_original() {
        let p = plan_split("handlers.rs", GOD, 3);
        let dir = std::env::temp_dir().join(format!("flux-legacy-exec-test-{}", std::process::id()));
        let dirs = dir.to_string_lossy().to_string();
        let written = stage_patch(&dirs, &p).expect("stage ok");
        assert_eq!(written.len(), p.modules.len() + 1, "one file per module + wiring");
        assert!(written.iter().any(|w| w.ends_with("MOD_WIRING.txt")));
        // first module file actually exists + has content
        assert!(std::fs::read_to_string(&written[0]).unwrap().contains("use super::*;"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
