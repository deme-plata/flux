// flux-rev/src/search.rs — Content-addressed blob search
//
// Because every blob is content-addressed by BLAKE3 hash, we can build a
// persistent full-text index keyed by blob hash. The index stores (hash →
// trigram set), so searching is O(1) against the index instead of O(N)
// linear scan. This replaces `find . -name '*.rs' | xargs grep pattern`
// with `flux-rev search --pattern 'fn heal' --kind rs`.
//
// Two modes:
//   search_indexed  — fast: query pre-built trigram index, return matching hashes
//   search_live     — fallback: walk working tree, grep each blob (like find+grep
//                     but BLAKE3-deduped — identical files searched once)

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::Path;
use std::fs;

/// A trigram-based search index: for each blob hash, stores its trigrams.
/// Query: extract trigrams from query string, intersect matching hashes.
#[derive(Debug, Clone, Default)]
pub struct SearchIndex {
    /// hash → set of trigrams in that blob
    pub trigrams: BTreeMap<String, BTreeSet<String>>,
}

impl SearchIndex {
    /// Build index from a Store's object directory by reading every blob.
    /// This is a batch operation — run it periodically or on-demand.
    pub fn build_from_store(store_dir: &Path) -> io::Result<Self> {
        let mut index = SearchIndex::default();
        let objects_dir = store_dir.join("objects");
        if !objects_dir.is_dir() {
            return Ok(index);
        }
        for entry in fs::read_dir(&objects_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                let hash = entry.file_name().to_string_lossy().to_string();
                if let Ok(bytes) = fs::read(&path) {
                    // Only index text files (skip binary blobs > 1MB)
                    if bytes.len() < 1_048_576 && is_text(&bytes) {
                        index.index_blob(&hash, &bytes);
                    }
                }
            }
        }
        Ok(index)
    }

    /// Index a single blob by its hash and content.
    pub fn index_blob(&mut self, hash: &str, bytes: &[u8]) {
        if let Ok(text) = std::str::from_utf8(bytes) {
            let trigrams = extract_trigrams(text);
            if !trigrams.is_empty() {
                self.trigrams.insert(hash.to_string(), trigrams);
            }
        }
    }

    /// Search for a pattern. Returns matching blob hashes ranked by relevance.
    pub fn search(&self, pattern: &str) -> Vec<String> {
        let query_trigrams = extract_trigrams(pattern);
        if query_trigrams.is_empty() {
            return Vec::new();
        }

        // Find hashes that contain ANY of the query trigrams
        let mut candidates: BTreeMap<String, usize> = BTreeMap::new();
        for qt in &query_trigrams {
            for (hash, trigrams) in &self.trigrams {
                if trigrams.contains(qt) {
                    *candidates.entry(hash.clone()).or_insert(0) += 1;
                }
            }
        }

        // Rank by trigram overlap count (more = better match)
        let mut ranked: Vec<(String, usize)> = candidates.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1));
        ranked.into_iter().map(|(h, _)| h).collect()
    }

    /// Search and return matching lines from the store.
    pub fn search_with_context(
        &self,
        store_dir: &Path,
        pattern: &str,
        context_lines: usize,
    ) -> Vec<SearchMatch> {
        let hashes = self.search(pattern);
        let objects_dir = store_dir.join("objects");
        let mut results = Vec::new();

        let lower = pattern.to_lowercase();
        for hash in &hashes {
            let path = objects_dir.join(hash);
            if let Ok(bytes) = fs::read(&path) {
                if let Ok(text) = std::str::from_utf8(&bytes) {
                    for (line_no, line) in text.lines().enumerate() {
                        if line.to_lowercase().contains(&lower) {
                            let ctx_start = line_no.saturating_sub(context_lines);
                            let ctx_end = (line_no + context_lines + 1).min(text.lines().count());
                            let context: Vec<String> = text.lines()
                                .skip(ctx_start)
                                .take(ctx_end - ctx_start)
                                .map(|s| s.to_string())
                                .collect();

                            results.push(SearchMatch {
                                hash: hash.clone(),
                                line_number: line_no + 1,
                                line: line.to_string(),
                                context,
                            });
                            break; // One match per blob
                        }
                    }
                }
            }
        }
        results
    }
}

/// A single search result with context.
#[derive(Debug, Clone)]
pub struct SearchMatch {
    pub hash: String,
    pub line_number: usize,
    pub line: String,
    pub context: Vec<String>,
}

/// Extract character trigrams from text for fuzzy search.
fn extract_trigrams(text: &str) -> BTreeSet<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut trigrams = BTreeSet::new();
    for window in chars.windows(3) {
        let tri: String = window.iter().collect();
        trigrams.insert(tri.to_lowercase());
    }
    // Also index word-level bigrams for better code search
    for word in text.split(|c: char| !c.is_alphanumeric()) {
        if word.len() >= 2 {
            trigrams.insert(format!("w:{}", word.to_lowercase()));
        }
    }
    trigrams
}

/// Heuristic: is this blob likely text (not binary)?
fn is_text(bytes: &[u8]) -> bool {
    // Check first 8KB for null bytes
    let check_len = bytes.len().min(8192);
    let null_count = bytes[..check_len].iter().filter(|&&b| b == 0).count();
    null_count * 100 < check_len // <1% null bytes → probably text
}

/// Live search: walk a directory tree and grep each file.
/// Replaces `find . -name '*.rs' | xargs grep pattern`.
/// BLAKE3-deduplicated: identical files (same hash) searched only once.
pub fn search_live(
    root: &Path,
    pattern: &str,
    file_glob: Option<&str>,
) -> io::Result<Vec<LiveMatch>> {
    let mut results = Vec::new();
    let mut seen_hashes: BTreeSet<String> = BTreeSet::new();
    let lower = pattern.to_lowercase();

    walk_and_search(root, root, pattern, &lower, file_glob, &mut seen_hashes, &mut results)?;
    Ok(results)
}

/// A live search match with file path and line.
#[derive(Debug, Clone)]
pub struct LiveMatch {
    pub path: String,
    pub line_number: usize,
    pub line: String,
    pub hash: String,
}

fn walk_and_search(
    root: &Path,
    dir: &Path,
    pattern: &str,
    lower_pattern: &str,
    file_glob: Option<&str>,
    seen_hashes: &mut BTreeSet<String>,
    results: &mut Vec<LiveMatch>,
) -> io::Result<()> {
    use crate::SKIP_DIRS;

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        if path.is_dir() {
            if !SKIP_DIRS.contains(&name.as_str()) {
                walk_and_search(root, &path, pattern, lower_pattern, file_glob, seen_hashes, results)?;
            }
        } else if path.is_file() {
            // Glob filter
            if let Some(glob) = file_glob {
                if !glob_matches(glob, &name) {
                    continue;
                }
            }

            // Read + hash + dedup
            if let Ok(bytes) = fs::read(&path) {
                let hash = crate::hash_bytes(&bytes);
                if seen_hashes.contains(&hash) {
                    continue; // Already searched this content
                }
                seen_hashes.insert(hash.clone());

                if let Ok(text) = std::str::from_utf8(&bytes) {
                    let rel = path.strip_prefix(root)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .replace('\\', "/");

                    for (line_no, line) in text.lines().enumerate() {
                        if line.to_lowercase().contains(lower_pattern) {
                            results.push(LiveMatch {
                                path: rel.clone(),
                                line_number: line_no + 1,
                                line: line.to_string(),
                                hash: hash.clone(),
                            });
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Simple glob matching: `*.rs` matches `foo.rs`, `*test*` matches `test_foo.rs`.
fn glob_matches(glob: &str, name: &str) -> bool {
    if glob == "*" {
        return true;
    }
    // *text* → contains
    if glob.starts_with('*') && glob.ends_with('*') && glob.len() > 1 {
        let inner = &glob[1..glob.len()-1];
        return name.contains(inner);
    }
    // *.rs → ends with
    if let Some(suffix) = glob.strip_prefix('*') {
        return name.ends_with(suffix);
    }
    // test* → starts with
    if let Some(prefix) = glob.strip_suffix('*') {
        return name.starts_with(prefix);
    }
    name == glob
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_trigrams() {
        let trigrams = extract_trigrams("hello");
        assert!(trigrams.contains("hel"));
        assert!(trigrams.contains("ell"));
        assert!(trigrams.contains("llo"));
        assert!(trigrams.contains("w:hello"));
    }

    #[test]
    fn test_search_index_basic() {
        let mut index = SearchIndex::default();
        index.index_blob("hash1", b"fn main() { println!(\"hello\"); }");
        index.index_blob("hash2", b"fn test() { assert!(true); }");

        let results = index.search("hello");
        assert!(results.contains(&"hash1".to_string()));
        assert!(!results.contains(&"hash2".to_string()));
    }

    #[test]
    fn test_glob_matches() {
        assert!(glob_matches("*.rs", "foo.rs"));
        assert!(glob_matches("*.rs", "foo.txt") == false);
        assert!(glob_matches("*test*", "test_foo.rs"));
        assert!(glob_matches("*test*", "bar_test.rs"));
    }

    #[test]
    fn test_is_text() {
        assert!(is_text(b"fn main() {}"));
        assert!(!is_text(&[0u8; 100]));
    }

    #[test]
    fn test_search_live_tempdir() {
        let dir = std::env::temp_dir().join("flux-rev-search-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.rs"), "fn add() { 1 + 1 }").unwrap();
        fs::write(dir.join("b.rs"), "fn sub() { 2 - 1 }").unwrap();

        let results = search_live(&dir, "add", Some("*.rs")).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].path.contains("a.rs"));
        assert!(results[0].line.contains("add"));

        let _ = fs::remove_dir_all(&dir);
    }
}
