//! money.rs — flux-legacy WARD: **who writes the money?**
//!
//! Born 2026-10-30 on the Quillon follower: one block was paid by TWO writers of the same balance
//! store — the gossip `BalanceConsensusEngine` and the turbo-sync `StateApplicator` legacy bridge —
//! and only one of them obeyed the "already paid" mark, so the follower's supply grew 1.99× what
//! the headers mint. Size metrics (god-files, fan-in) cannot see that; it is a property of *where
//! money is written*. SIGIL's answer is ONE writer (`ChainTip::apply`, once per block). This ward
//! measures how far a legacy tree is from that, statically:
//!
//!   1. KEY FNS — functions whose body names the balance key family (e.g. `wallet_balance_`).
//!   2. SINKS   — functions that WRITE it: name the key (or call a key fn) AND call a put/merge.
//!   3. WRITERS — every function that reaches a sink through the call graph (by name).
//!   4. ROOTS   — writers nothing else in the tree calls: the entry points where money moves
//!                (route handlers passed by name, `main`, spawned tasks inside them, tools).
//!   5. GUARD   — a path root→sink is guarded when some function on it names a guard token (the
//!                per-block "already paid" mark). Every root is reported guarded or UNGUARDED, with
//!                one example chain.
//!
//! Unguarded does not by itself mean "bug": it means money moves without the per-block mark —
//! either outside block application (an API debit, a migration tool: something SIGIL forbids by
//! construction) or on a block path that can pay twice. The ward lists them; a human (or the next
//! ward) decides which is which.
//!
//! Static and NAME-based (no type resolution): same-named functions merge; calls through trait
//! objects, fn pointers or closures stored in fields are invisible; `#[cfg(test)]` modules and
//! `tests/`, `benches/`, `examples/` are skipped. The report repeats these caveats.

use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// What to look for. Defaults fit Quillon (`q-narwhalknight`).
#[derive(Debug, Clone)]
pub struct MoneyConfig {
    /// Substring that names the balance key family inside a string literal or identifier.
    pub key: String,
    /// Tokens whose presence in a function marks the path as carrying the per-block mark.
    pub guard_tokens: Vec<String>,
    /// Call names that count as a storage write.
    pub write_calls: Vec<String>,
    /// A name defined by more than this many functions (`new`, `get`, `process`…) is ambiguous: calls
    /// through it are NOT followed (they would wire half the tree to any one writer that shares it).
    /// Skipped writer names are listed in the report.
    pub max_defs_per_name: usize,
}

impl Default for MoneyConfig {
    fn default() -> Self {
        Self {
            key: "wallet_balance_".into(),
            guard_tokens: vec![
                "processed_balance_block".into(),
                "processed_balance_key".into(),
                "claim_block_credit".into(),
                "get_processed_block_flag".into(),
            ],
            // write_batch*: Quillon's KV trait (kv.rs) writes a Vec of (cf, key, value) in one call —
            // the DEX reconciler, swap debit/credit and LP paths write balances ONLY that way, and
            // were invisible to this ward until 2026-11-02.
            write_calls: ["put", "put_cf", "put_sync", "put_cf_opt", "merge", "merge_cf",
                          "write_batch", "write_batch_cf", "write_batch_bulk", "write_batch_turbo"]
                .iter().map(|s| s.to_string()).collect(),
            max_defs_per_name: 6,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MoneyFn {
    pub file: String,
    pub name: String,
    pub line: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct MoneyRoot {
    pub file: String,
    pub name: String,
    pub line: usize,
    pub guarded: bool,
    /// root → … → sink, each hop as `name (file:line)`. For an unguarded root this chain avoids
    /// every guard; for a guarded root it is one shortest chain.
    pub chain: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MoneyReport {
    pub root: String,
    pub key: String,
    pub guard_tokens: Vec<String>,
    pub files_scanned: usize,
    pub fns_scanned: usize,
    pub key_fns: usize,
    /// the functions that name the key family (where a sink's key comes from)
    pub key_fn_list: Vec<MoneyFn>,
    pub sinks: Vec<MoneyFn>,
    pub writers: usize,
    pub roots: Vec<MoneyRoot>,
    pub unguarded_roots: usize,
    /// writers per crate (`crates/<name>/…`), most first.
    pub writers_by_crate: Vec<(String, usize)>,
    /// writer names defined too often to follow by name (`max_defs_per_name`) — their callers are
    /// NOT counted as writers; review these by hand.
    pub ambiguous_writer_names: Vec<(String, usize)>,
    pub caveats: Vec<String>,
}

// ───────────────────────────── lexing ─────────────────────────────

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Two masks of `src`, byte-for-byte the same length (newlines kept so line numbers survive):
/// `code` blanks comments AND string/char contents; `strs` blanks comments only.
pub fn masks(src: &str) -> (Vec<u8>, Vec<u8>) {
    let s = src.as_bytes();
    let n = s.len();
    let mut code = s.to_vec();
    let mut strs = s.to_vec();
    let blank = |v: &mut Vec<u8>, a: usize, b: usize| {
        for x in &mut v[a..b.min(n)] {
            if *x != b'\n' {
                *x = b' ';
            }
        }
    };
    let mut i = 0;
    while i < n {
        let c = s[i];
        // line comment
        if c == b'/' && i + 1 < n && s[i + 1] == b'/' {
            let e = s[i..].iter().position(|&x| x == b'\n').map(|p| i + p).unwrap_or(n);
            blank(&mut code, i, e);
            blank(&mut strs, i, e);
            i = e;
            continue;
        }
        // block comment (nests in Rust)
        if c == b'/' && i + 1 < n && s[i + 1] == b'*' {
            let (mut j, mut depth) = (i + 2, 1);
            while j < n && depth > 0 {
                if s[j] == b'/' && j + 1 < n && s[j + 1] == b'*' {
                    depth += 1;
                    j += 2;
                } else if s[j] == b'*' && j + 1 < n && s[j + 1] == b'/' {
                    depth -= 1;
                    j += 2;
                } else {
                    j += 1;
                }
            }
            blank(&mut code, i, j);
            blank(&mut strs, i, j);
            i = j;
            continue;
        }
        // raw string r"…", r#"…"#, br#"…"#
        let raw_start = c == b'r'
            && (i == 0 || !is_ident(s[i - 1]) || (s[i - 1] == b'b' && (i < 2 || !is_ident(s[i - 2]))));
        if raw_start {
            let mut j = i + 1;
            let mut hashes = 0;
            while j < n && s[j] == b'#' {
                hashes += 1;
                j += 1;
            }
            if j < n && s[j] == b'"' {
                let body = j + 1;
                let mut k = body;
                let end = loop {
                    if k >= n {
                        break n;
                    }
                    if s[k] == b'"' && s[k + 1..].iter().take(hashes).filter(|&&h| h == b'#').count() == hashes
                        && k + hashes < n
                    {
                        break k;
                    }
                    k += 1;
                };
                blank(&mut code, body, end);
                i = (end + 1 + hashes).min(n);
                continue;
            }
        }
        // ordinary (or byte) string
        if c == b'"' {
            let mut j = i + 1;
            while j < n && s[j] != b'"' {
                if s[j] == b'\\' {
                    j += 1;
                }
                j += 1;
            }
            blank(&mut code, i + 1, j);
            i = (j + 1).min(n);
            continue;
        }
        // char literal vs lifetime
        if c == b'\'' && i + 1 < n {
            if s[i + 1] == b'\\' {
                let mut j = i + 3; // past `'\x`: an escaped quote must not close it
                while j < n && s[j] != b'\'' {
                    j += 1;
                }
                blank(&mut code, i + 1, j);
                i = (j + 1).min(n);
                continue;
            }
            let clen = src[i + 1..].chars().next().map(|ch| ch.len_utf8()).unwrap_or(1);
            if i + 1 + clen < n && s[i + 1 + clen] == b'\'' {
                blank(&mut code, i + 1, i + 1 + clen);
                i += 2 + clen;
                continue;
            }
        }
        i += 1;
    }
    (code, strs)
}

/// Blank `#[cfg(test)] mod x { … }` regions in both masks (test code is not a money path).
fn blank_test_mods(code: &mut [u8], strs: &mut [u8]) {
    let pat = b"#[cfg(test)]";
    let mut i = 0;
    while let Some(p) = find(code, pat, i) {
        let after = p + pat.len();
        // the next item must be `mod name {`
        let mut j = after;
        while j < code.len() && (code[j].is_ascii_whitespace()) {
            j += 1;
        }
        if code[j..].starts_with(b"mod ") || code[j..].starts_with(b"pub mod ") {
            if let Some(open) = code[j..].iter().position(|&b| b == b'{' || b == b';').map(|q| j + q) {
                if code[open] == b'{' {
                    if let Some(close) = match_brace(code, open) {
                        for k in p..=close {
                            if code[k] != b'\n' {
                                code[k] = b' ';
                                strs[k] = b' ';
                            }
                        }
                        i = close + 1;
                        continue;
                    }
                }
            }
        }
        i = after;
    }
}

fn find(h: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= h.len() {
        return None;
    }
    h[from..].windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}

fn match_brace(code: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0i64;
    for (k, &b) in code.iter().enumerate().skip(open) {
        if b == b'{' {
            depth += 1;
        } else if b == b'}' {
            depth -= 1;
            if depth == 0 {
                return Some(k);
            }
        }
    }
    None
}

/// One function with a body: name, 1-based line of `fn`, body byte span `{ … }` (inclusive).
#[derive(Debug, Clone)]
pub struct FnSpan {
    pub name: String,
    pub line: usize,
    pub open: usize,
    pub close: usize,
    pub is_test: bool,
}

/// Every `fn name … { … }` in a masked source — free fns, methods in `impl`/`trait` blocks, nested
/// fns. Declarations without a body (`fn f();`) and fn-pointer types (`fn(`) are skipped.
pub fn find_fns(code: &[u8]) -> Vec<FnSpan> {
    let mut out = Vec::new();
    let n = code.len();
    // newline offsets once per file; a line number is a binary search, not a recount from the top
    // (recounting made a 28k-line main.rs cost billions of steps)
    let newlines: Vec<usize> = code.iter().enumerate().filter(|(_, &b)| b == b'\n').map(|(k, _)| k).collect();
    let mut i = 0;
    while let Some(p) = find(code, b"fn", i) {
        i = p + 2;
        if p > 0 && is_ident(code[p - 1]) {
            continue;
        }
        if p + 2 >= n || !code[p + 2].is_ascii_whitespace() {
            continue;
        }
        let mut j = p + 2;
        while j < n && code[j].is_ascii_whitespace() {
            j += 1;
        }
        let ns = j;
        while j < n && is_ident(code[j]) {
            j += 1;
        }
        if j == ns {
            continue;
        }
        let name = String::from_utf8_lossy(&code[ns..j]).to_string();
        // body: the first `{` before any `;` (a `;` first = declaration only) — counted only OUTSIDE
        // `( … )` and `[ … ]`, because `fn f(k: &[u8; 32])` carries a `;` in its parameter list (the
        // 2026-10-30 bridge key fn `legacy_wallet_balance_key` was invisible until this)
        let mut depth = 0i32;
        let mut found = None;
        for (k, &b) in code.iter().enumerate().skip(j) {
            match b {
                b'(' | b'[' => depth += 1,
                b')' | b']' => depth -= 1,
                b'{' | b';' if depth <= 0 => { found = Some(k); break; }
                _ => {}
            }
        }
        let Some(q) = found else { break };
        if code[q] == b';' {
            continue;
        }
        let Some(close) = match_brace(code, q) else { continue };
        let line = 1 + newlines.partition_point(|&k| k < p);
        // attributes directly above this fn: back to the previous `}` `;` or `{` (item boundary)
        let lo = p.saturating_sub(4096);
        let bound = code[lo..p].iter().rposition(|&b| b == b'}' || b == b';' || b == b'{').map(|b| lo + b + 1).unwrap_or(lo);
        let head = String::from_utf8_lossy(&code[bound..p]);
        let is_test = head.contains("test]") || head.contains("cfg(test)");
        out.push(FnSpan { name, line, open: q, close, is_test });
    }
    out
}

/// Text of `span`'s body with every nested fn body blanked (so calls are attributed to the
/// innermost function only).
fn own_text(mask: &[u8], span: &FnSpan, all: &[FnSpan]) -> Vec<u8> {
    let mut v = mask[span.open..=span.close].to_vec();
    for c in all {
        if c.open > span.open && c.close < span.close {
            for k in c.open..=c.close {
                v[k - span.open] = b' ';
            }
        }
    }
    v
}

/// Names called in `text`: `name(`, `.name(`, `Path::name(` — macros (`name!(`) excluded.
fn calls_in(text: &[u8]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let n = text.len();
    let mut i = 0;
    while i < n {
        if is_ident(text[i]) && (i == 0 || !is_ident(text[i - 1])) {
            let s = i;
            while i < n && is_ident(text[i]) {
                i += 1;
            }
            let mut j = i;
            while j < n && text[j] == b' ' {
                j += 1;
            }
            // turbofish name::<T>(
            if j + 2 < n && &text[j..j + 3] == b"::<" {
                let mut depth = 0;
                let mut k = j + 2;
                while k < n {
                    if text[k] == b'<' {
                        depth += 1;
                    } else if text[k] == b'>' {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    k += 1;
                }
                j = k + 1;
            }
            // `fn name(` is a declaration (a nested fn's signature left in the outer body), not a call
            let mut b = s;
            while b > 0 && text[b - 1].is_ascii_whitespace() {
                b -= 1;
            }
            let declared = b >= 2 && &text[b - 2..b] == b"fn" && (b == 2 || !is_ident(text[b - 3]));
            if j < n && text[j] == b'(' && !text[s].is_ascii_digit() && !declared {
                out.insert(String::from_utf8_lossy(&text[s..i]).to_string());
            }
        } else {
            i += 1;
        }
    }
    out
}

fn contains(h: &[u8], needle: &str) -> bool {
    find(h, needle.as_bytes(), 0).is_some()
}

// ───────────────────────────── scanning ─────────────────────────────

struct Node {
    file: String,
    name: String,
    line: usize,
    calls: BTreeSet<String>,
    key_fn: bool,
    writes: bool,
    guard: bool,
}

fn rs_files(root: &Path) -> Vec<PathBuf> {
    let base = if root.join("crates").is_dir() { root.join("crates") } else { root.to_path_buf() };
    let mut out = Vec::new();
    let mut stack = vec![base];
    const SKIP: &[&str] = &["target", ".git", "node_modules", "tests", "benches", "examples", ".flux-legacy-preview"];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if p.is_dir() {
                if !SKIP.contains(&name.as_str()) && !name.starts_with('.') {
                    stack.push(p);
                }
            } else if name.ends_with(".rs") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Scan a source string (one file) into nodes. Public for tests and for single-file use.
fn scan_source(rel: &str, src: &str, cfg: &MoneyConfig) -> Vec<Node> {
    let (mut code, mut strs) = masks(src);
    blank_test_mods(&mut code, &mut strs);
    let spans = find_fns(&code);
    let mut out = Vec::new();
    for sp in spans.iter().filter(|s| !s.is_test) {
        let own_code = own_text(&code, sp, &spans);
        let own_strs = own_text(&strs, sp, &spans);
        let calls = calls_in(&own_code);
        let key_fn = contains(&own_strs, &cfg.key);
        let writes = cfg.write_calls.iter().any(|w| calls.contains(w));
        let guard = cfg.guard_tokens.iter().any(|g| contains(&own_strs, g));
        out.push(Node { file: rel.to_string(), name: sp.name.clone(), line: sp.line, calls, key_fn, writes, guard });
    }
    out
}

/// The ward. `root` is a repo (scans `crates/**`) or a single crate dir.
pub fn scan(root: &str, cfg: &MoneyConfig) -> MoneyReport {
    let rootp = Path::new(root);
    let t0 = std::time::Instant::now();
    let files = rs_files(rootp);
    let mut nodes: Vec<Node> = Vec::new();
    for f in &files {
        let Ok(src) = std::fs::read_to_string(f) else { continue };
        let rel = f.strip_prefix(rootp).unwrap_or(f).to_string_lossy().to_string();
        let t = std::time::Instant::now();
        nodes.extend(scan_source(&rel, &src, cfg));
        if t.elapsed().as_secs_f64() > 2.0 {
            eprintln!("money: slow file {rel} {:.1}s", t.elapsed().as_secs_f64());
        }
    }
    eprintln!("money: scanned {} files / {} fns in {:.1}s", files.len(), nodes.len(), t0.elapsed().as_secs_f64());
    let t1 = std::time::Instant::now();
    let r = analyze_nodes(root, files.len(), nodes, cfg);
    eprintln!("money: analysis {:.1}s", t1.elapsed().as_secs_f64());
    r
}

/// Same analysis over in-memory sources `(rel_path, src)` — used by tests.
pub fn scan_sources(sources: &[(&str, &str)], cfg: &MoneyConfig) -> MoneyReport {
    let mut nodes = Vec::new();
    for (rel, src) in sources {
        nodes.extend(scan_source(rel, src, cfg));
    }
    analyze_nodes("<memory>", sources.len(), nodes, cfg)
}

fn label(n: &Node) -> String {
    format!("{} ({}:{})", n.name, n.file, n.line)
}

fn analyze_nodes(root: &str, files_scanned: usize, nodes: Vec<Node>, cfg: &MoneyConfig) -> MoneyReport {
    let by_name: HashMap<&str, Vec<usize>> = nodes.iter().enumerate().fold(HashMap::new(), |mut m, (i, n)| {
        m.entry(n.name.as_str()).or_default().push(i);
        m
    });
    let ambiguous = |name: &str| by_name.get(name).map_or(false, |v| v.len() > cfg.max_defs_per_name);
    // a key fn is followed by name only when that name is specific (a `get` that builds the key must
    // not turn every `get`+`put` caller in the tree into a sink)
    let key_names: HashSet<&str> = nodes.iter().filter(|n| n.key_fn).map(|n| n.name.as_str())
        .filter(|nm| !ambiguous(nm)).collect();
    // SINK: writes, and names the key itself or calls a key fn
    let is_sink: Vec<bool> = nodes
        .iter()
        .map(|n| n.writes && (n.key_fn || n.calls.iter().any(|c| key_names.contains(c.as_str()))))
        .collect();
    // WRITERS: fixpoint over the by-name call graph, never through an ambiguous name
    let mut writer = is_sink.clone();
    loop {
        let names: HashSet<&str> = nodes.iter().zip(&writer).filter(|(_, &w)| w).map(|(n, _)| n.name.as_str())
            .filter(|nm| !ambiguous(nm)).collect();
        let mut changed = false;
        for (i, n) in nodes.iter().enumerate() {
            if !writer[i] && n.calls.iter().any(|c| names.contains(c.as_str())) {
                writer[i] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    // callees that are writers, per node
    let wcallees: Vec<Vec<usize>> = nodes
        .iter()
        .map(|n| {
            let mut v: Vec<usize> = n
                .calls
                .iter()
                .filter(|c| !ambiguous(c.as_str()))
                .filter_map(|c| by_name.get(c.as_str()))
                .flatten()
                .copied()
                .filter(|&j| writer[j])
                .collect();
            v.sort();
            v.dedup();
            v
        })
        .collect();
    // ROOTS: writers whose name no OTHER scanned function calls
    let called: HashSet<&str> = nodes
        .iter()
        .flat_map(|n| n.calls.iter().filter(move |c| c.as_str() != n.name).map(|c| c.as_str()))
        .collect();
    // unguarded chain search (memoised DFS; cycles cut by an on-stack set)
    // returns (chain, cut): `cut` = some branch was skipped because it looped back onto the stack,
    // so a "none found" answer is provisional and must not be cached (a false "guarded" would hide
    // exactly what this ward exists to show).
    fn unguarded_chain(
        i: usize, nodes: &[Node], sink: &[bool], wc: &[Vec<usize>],
        memo: &mut HashMap<usize, Option<Vec<usize>>>, on: &mut HashSet<usize>,
    ) -> (Option<Vec<usize>>, bool) {
        if let Some(m) = memo.get(&i) {
            return (m.clone(), false);
        }
        if nodes[i].guard {
            memo.insert(i, None);
            return (None, false);
        }
        if sink[i] {
            let r = Some(vec![i]);
            memo.insert(i, r.clone());
            return (r, false);
        }
        if !on.insert(i) {
            return (None, true);
        }
        let mut best: Option<Vec<usize>> = None;
        let mut cut = false;
        for &j in &wc[i] {
            let (c, jc) = unguarded_chain(j, nodes, sink, wc, memo, on);
            cut |= jc;
            if let Some(mut c) = c {
                if best.as_ref().map_or(true, |b| c.len() + 1 < b.len()) {
                    c.insert(0, i);
                    best = Some(c);
                }
            }
        }
        on.remove(&i);
        if best.is_some() || !cut {
            memo.insert(i, best.clone());
        }
        (best, cut)
    }
    fn any_chain(i: usize, sink: &[bool], wc: &[Vec<usize>]) -> Vec<usize> {
        // BFS to the nearest sink
        let mut prev: HashMap<usize, usize> = HashMap::new();
        let mut q = std::collections::VecDeque::from([i]);
        let mut seen = HashSet::from([i]);
        while let Some(x) = q.pop_front() {
            if sink[x] {
                let mut path = vec![x];
                let mut cur = x;
                while let Some(&p) = prev.get(&cur) {
                    path.push(p);
                    cur = p;
                }
                path.reverse();
                return path;
            }
            for &y in &wc[x] {
                if seen.insert(y) {
                    prev.insert(y, x);
                    q.push_back(y);
                }
            }
        }
        vec![i]
    }
    let mut memo = HashMap::new();
    let mut roots = Vec::new();
    for (i, n) in nodes.iter().enumerate() {
        if !writer[i] || called.contains(n.name.as_str()) {
            continue;
        }
        let mut on = HashSet::new();
        let (ug, _) = unguarded_chain(i, &nodes, &is_sink, &wcallees, &mut memo, &mut on);
        let (guarded, chain) = match ug {
            Some(c) => (false, c),
            None => (true, any_chain(i, &is_sink, &wcallees)),
        };
        roots.push(MoneyRoot {
            file: n.file.clone(),
            name: n.name.clone(),
            line: n.line,
            guarded,
            chain: chain.iter().map(|&k| label(&nodes[k])).collect(),
        });
    }
    roots.sort_by(|a, b| a.guarded.cmp(&b.guarded).then(a.file.cmp(&b.file)).then(a.line.cmp(&b.line)));
    let mut by_crate: BTreeMap<String, usize> = BTreeMap::new();
    for (n, _) in nodes.iter().zip(&writer).filter(|(_, &w)| w) {
        let c = n.file.split('/').next().unwrap_or("").to_string();
        *by_crate.entry(c).or_default() += 1;
    }
    let mut writers_by_crate: Vec<(String, usize)> = by_crate.into_iter().collect();
    writers_by_crate.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut amb: BTreeMap<String, usize> = BTreeMap::new();
    for (n, _) in nodes.iter().zip(&writer).filter(|(_, &w)| w) {
        if ambiguous(&n.name) {
            amb.insert(n.name.clone(), by_name.get(n.name.as_str()).map_or(0, |v| v.len()));
        }
    }
    let ambiguous_writer_names: Vec<(String, usize)> = amb.into_iter().collect();
    let sinks = nodes
        .iter()
        .zip(&is_sink)
        .filter(|(_, &s)| s)
        .map(|(n, _)| MoneyFn { file: n.file.clone(), name: n.name.clone(), line: n.line })
        .collect();
    MoneyReport {
        root: root.to_string(),
        key: cfg.key.clone(),
        guard_tokens: cfg.guard_tokens.clone(),
        files_scanned,
        fns_scanned: nodes.len(),
        key_fns: nodes.iter().filter(|n| n.key_fn).count(),
        key_fn_list: nodes.iter().filter(|n| n.key_fn)
            .map(|n| MoneyFn { file: n.file.clone(), name: n.name.clone(), line: n.line }).collect(),
        sinks,
        writers: writer.iter().filter(|&&w| w).count(),
        unguarded_roots: roots.iter().filter(|r| !r.guarded).count(),
        roots,
        writers_by_crate,
        ambiguous_writer_names,
        caveats: vec![
            "name-based call graph: same-named functions merge; trait-object / fn-pointer / stored-closure calls are invisible".into(),
            "a route handler passed by name (post(handler)) counts as a root — correct, it is an entry point".into(),
            "#[cfg(test)] modules, #[test] fns and tests/ benches/ examples/ are skipped".into(),
            "UNGUARDED = money moves without the per-block mark: outside block application, or a block path that can pay twice — decide per root".into(),
        ],
    }
}

// ───────────────────────────── rendering ─────────────────────────────

pub fn render_text(r: &MoneyReport, all: bool) -> String {
    let mut s = String::new();
    s.push_str(&format!("💰 MONEY WARD — who writes `{}*`?   ({})\n", r.key, r.root));
    s.push_str(&format!(
        "   {} files · {} functions · {} name the key · {} SINKS write it · {} WRITERS reach a sink · {} ENTRY POINTS ({} UNGUARDED)\n",
        r.files_scanned, r.fns_scanned, r.key_fns, r.sinks.len(), r.writers, r.roots.len(), r.unguarded_roots
    ));
    s.push_str(&format!("   guard = any of: {}\n\n", r.guard_tokens.join(", ")));
    s.push_str("KEY FNS — functions that name the key family\n");
    for k in &r.key_fn_list {
        s.push_str(&format!("   {}:{}  {}\n", k.file, k.line, k.name));
    }
    s.push_str("\nSINKS — functions that write the balance store directly\n");
    for k in &r.sinks {
        s.push_str(&format!("   {}:{}  {}\n", k.file, k.line, k.name));
    }
    let lim = if all { usize::MAX } else { 40 };
    for (title, g) in [("ENTRY POINTS — UNGUARDED (money moves without the per-block mark)", false), ("ENTRY POINTS — guarded", true)] {
        let list: Vec<&MoneyRoot> = r.roots.iter().filter(|x| x.guarded == g).collect();
        s.push_str(&format!("\n{} — {}\n", title, list.len()));
        for x in list.iter().take(lim) {
            s.push_str(&format!("   {}:{}  {}\n      {}\n", x.file, x.line, x.name, x.chain.join(" → ")));
        }
        if list.len() > lim {
            s.push_str(&format!("   … {} more (--all)\n", list.len() - lim));
        }
    }
    if !r.ambiguous_writer_names.is_empty() {
        s.push_str("\nAMBIGUOUS WRITER NAMES — not followed by name (defined in many places); review their callers by hand\n");
        for (nm, k) in &r.ambiguous_writer_names {
            s.push_str(&format!("   {nm} ({k} definitions)\n"));
        }
    }
    s.push_str("\nWRITERS BY CRATE\n");
    for (c, k) in &r.writers_by_crate {
        s.push_str(&format!("   {:>5}  {}\n", k, c));
    }
    s.push_str("\nCAVEATS\n");
    for c in &r.caveats {
        s.push_str(&format!("   · {}\n", c));
    }
    s
}

// ───────────────────────────── tests ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const STORE: &str = r#"
        pub struct Store;
        impl Store {
            /// writes wallet_balance_ (doc comment must NOT count as naming the key)
            pub async fn add_balance(&self, a: &str, v: u128) {
                let k = format!("wallet_balance_{}", a);
                self.db.put("manifest", k.as_bytes(), &v.to_le_bytes());
            }
            fn key(a: &[u8]) -> String { format!("wallet_balance_{}", hex(a)) }
            fn credit_batch(&self, b: &mut Batch, a: &[u8]) { b.put_cf(&cf, &key(a), "}{"); }
            pub fn load(&self) { let p = "wallet_balance_"; self.db.scan(p); }
        }
    "#;

    const PATHS: &str = r#"
        fn engine_pay(s: &Store, b: &Block) {
            if s.get("processed_balance_block:x") { return; }
            s.add_balance("a", 1);
        }
        fn bridge_pay(s: &Store, b: &Block) { helper(s); }
        fn helper(s: &Store) { s.credit_batch(&mut batch, &[1]); }
        fn api_send(s: &Store) { let c = '{'; s.add_balance("b", 2); }
        fn reader(s: &Store) { s.load(); }
        #[cfg(test)]
        mod tests { fn test_writer(s: &Store) { s.add_balance("t", 9); } }
    "#;

    fn report() -> MoneyReport {
        scan_sources(&[("q-storage/src/store.rs", STORE), ("q-api-server/src/main.rs", PATHS)], &MoneyConfig::default())
    }

    #[test]
    fn finds_sinks_direct_and_via_key_fn_but_not_readers() {
        let r = report();
        let names: Vec<&str> = r.sinks.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"add_balance"), "{names:?}");
        assert!(names.contains(&"credit_batch"), "a put of a key built by a key fn is a sink: {names:?}");
        assert!(!names.contains(&"load"), "naming the key without writing is not a sink");
        assert!(!names.contains(&"key"), "the key fn itself does not write");
    }

    #[test]
    fn classifies_entry_points_guarded_and_unguarded() {
        let r = report();
        let get = |n: &str| r.roots.iter().find(|x| x.name == n).unwrap_or_else(|| panic!("no root {n}: {:?}", r.roots));
        assert!(get("engine_pay").guarded);
        assert!(!get("bridge_pay").guarded, "bridge → helper → credit_batch carries no mark");
        assert_eq!(get("bridge_pay").chain.len(), 3);
        assert!(!get("api_send").guarded);
        assert!(r.roots.iter().all(|x| x.name != "reader"), "a reader is not a writer");
        assert!(r.roots.iter().all(|x| x.name != "test_writer"), "cfg(test) code is skipped");
        assert_eq!(r.unguarded_roots, 2);
    }

    #[test]
    fn braces_inside_strings_and_chars_do_not_break_spans() {
        let (code, _) = masks(STORE);
        let fns = find_fns(&code);
        let names: Vec<&str> = fns.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["add_balance", "key", "credit_batch", "load"]);
    }

    #[test]
    fn nested_fn_calls_belong_to_the_inner_fn() {
        let src = "fn outer() { fn inner(s: &S) { s.add_balance(); } let _ = 1; }\n\
                   fn add_balance(&self) { self.db.put(\"wallet_balance_x\", 1); }";
        let r = scan_sources(&[("c/src/a.rs", src)], &MoneyConfig::default());
        let roots: Vec<&str> = r.roots.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(roots, vec!["inner"], "outer does not call add_balance itself");
    }

    #[test]
    fn a_test_attribute_on_the_previous_fn_does_not_mark_this_one() {
        let src = "#[test]\nfn t() { }\nfn api(s: &S) { s.add_balance(); }\n\
                   fn add_balance(&self) { self.db.put(\"wallet_balance_x\", 1); }\n\
                   #[cfg(test)]\nfn helper(s: &S) { s.add_balance(); }";
        let r = scan_sources(&[("c/src/a.rs", src)], &MoneyConfig::default());
        let roots: Vec<&str> = r.roots.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(roots, vec!["api"]);
    }

    #[test]
    fn escaped_quote_char_does_not_open_a_literal() {
        let src = "fn a() { let q = '\\''; let b = '{'; }\nfn b() { }";
        let (code, _) = masks(src);
        let names: Vec<String> = find_fns(&code).into_iter().map(|f| f.name).collect();
        assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn array_types_in_the_signature_do_not_hide_the_body() {
        // the exact shape of Quillon's legacy_wallet_balance_key + apply_balance_credit
        let src = "fn legacy_key(account: &[u8; 32]) -> Vec<u8> { format!(\"wallet_balance_{}\", 1).into_bytes() }\n\
                   fn credit(b: &mut B, a: &[u8; 32]) { b.put_cf(&cf, &legacy_key(a), 1); }\n\
                   fn turbo(b: &mut B) { credit(b, &[0u8; 32]); }";
        let r = scan_sources(&[("c/src/a.rs", src)], &MoneyConfig::default());
        let sinks: Vec<&str> = r.sinks.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(sinks, vec!["credit"]);
        let roots: Vec<(&str, bool)> = r.roots.iter().map(|x| (x.name.as_str(), x.guarded)).collect();
        assert_eq!(roots, vec![("turbo", false)]);
    }

    #[test]
    fn a_batch_write_of_the_key_is_a_sink() {
        // the exact shape of Quillon's apply_dex_qug_adjustments (q-storage lib.rs, v10.11.114f):
        // the key is formatted inline and written with the KV trait's write_batch, from a 5-s loop
        let src = "fn apply_dex(&self) { let k = format!(\"wallet_balance_{}\", w);\n\
                   let batch = vec![(CF, k.as_bytes().to_vec(), v)]; self.hot_db.write_batch(batch).await; }\n\
                   fn reconciler_loop(s: S) { loop { s.apply_dex(); } }\n\
                   fn read_only(&self) { let k = format!(\"wallet_balance_{}\", w); self.db.get(k); }";
        let r = scan_sources(&[("q/src/lib.rs", src)], &MoneyConfig::default());
        let sinks: Vec<&str> = r.sinks.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(sinks, vec!["apply_dex"], "write_batch of the key must count as a write");
        let roots: Vec<(&str, bool)> = r.roots.iter().map(|x| (x.name.as_str(), x.guarded)).collect();
        assert_eq!(roots, vec![("reconciler_loop", false)]);
    }

    #[test]
    fn raw_strings_and_lifetimes_lex_cleanly() {
        let src = "fn f<'a>(x: &'a str) -> &'a str { let r = r#\"{ \"q\" }\"#; x }\nfn g() {}";
        let (code, _) = masks(src);
        let names: Vec<String> = find_fns(&code).into_iter().map(|f| f.name).collect();
        assert_eq!(names, vec!["f".to_string(), "g".to_string()]);
    }
}
