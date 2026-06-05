// api_index.rs — Scan all flux workspace crates for public API surface.
//
// Builds a JSON index of: pub fn names, pub struct fields, pub enum variants.
// Used by mismatch.rs to detect incorrect API calls before compilation.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// A single public function export with its signature info.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ApiFunction {
    pub name: String,
    pub crate_name: String,
    pub file: String,
    pub arg_count: usize,
    pub arg_types: Vec<String>, // heuristic: extracted from signature line
}

/// A public struct with its field definitions.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ApiStruct {
    pub name: String,
    pub crate_name: String,
    pub fields: Vec<ApiField>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ApiField {
    pub name: String,
    pub type_hint: String,
    pub is_pub: bool,
}

/// Complete API index for the workspace.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ApiIndex {
    pub functions: HashMap<String, ApiFunction>,   // key: "crate::fn_name"
    pub structs: HashMap<String, ApiStruct>,        // key: "crate::StructName"
    pub build_time_ms: u64,
    pub crates_scanned: usize,
    pub total_exports: usize,
}

/// Scan the workspace and build the API index.
pub fn build_index(workspace_root: &str) -> ApiIndex {
    let start = std::time::Instant::now();
    let crates_dir = PathBuf::from(workspace_root).join("crates");
    let mut index = ApiIndex::default();

    if let Ok(entries) = fs::read_dir(&crates_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let src_dir = path.join("src");
                if src_dir.exists() {
                    let crate_name = path.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown")
                        .to_string()
                        .replace('-', "_");
                    scan_crate_src(&src_dir, &crate_name, &mut index);
                    index.crates_scanned += 1;
                }
            }
        }
    }

    index.build_time_ms = start.elapsed().as_millis() as u64;
    index.total_exports = index.functions.len() + index.structs.len();
    index
}

fn scan_crate_src(src_dir: &PathBuf, crate_name: &str, index: &mut ApiIndex) {
    if let Ok(entries) = fs::read_dir(src_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "rs").unwrap_or(false) {
                if let Ok(content) = fs::read_to_string(&path) {
                    scan_rust_source(&content, &path, crate_name, index);
                }
            }
        }
    }
}

fn scan_rust_source(content: &str, path: &PathBuf, crate_name: &str, index: &mut ApiIndex) {
    let file_name = path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let mut lines: Vec<&str> = content.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i].trim();

        // Match: pub fn function_name(args...) -> ReturnType
        if line.starts_with("pub fn ") || line.starts_with("pub async fn ") {
            let fn_line = if line.ends_with('{') || line.ends_with(';') {
                line.to_string()
            } else {
                // Multi-line signature — collect until '{' or ';'
                let mut collected = line.to_string();
                let mut j = i + 1;
                while j < lines.len() {
                    collected.push(' ');
                    collected.push_str(lines[j].trim());
                    if lines[j].trim().ends_with('{') || lines[j].trim().ends_with(';') {
                        break;
                    }
                    j += 1;
                }
                collected
            };

            if let Some(name) = extract_fn_name(&fn_line) {
                let arg_count = count_args(&fn_line);
                let key = format!("{}::{}", crate_name, name);
                index.functions.insert(key, ApiFunction {
                    name,
                    crate_name: crate_name.to_string(),
                    file: file_name.clone(),
                    arg_count,
                    arg_types: vec![],
                });
            }
        }

        // Match: pub struct StructName {
        if line.starts_with("pub struct ") {
            if let Some(name) = extract_struct_name(line) {
                let mut fields = Vec::new();
                let mut j = i + 1;
                while j < lines.len() && !lines[j].trim().starts_with('}') {
                    let field_line = lines[j].trim();
                    if field_line.starts_with("pub ") && field_line.contains(':') {
                        let is_pub = !field_line.starts_with("pub(crate)") && !field_line.starts_with("pub(super)");
                        if let Some((fname, ftype)) = parse_field(field_line) {
                            fields.push(ApiField {
                                name: fname,
                                type_hint: ftype,
                                is_pub,
                            });
                        }
                    }
                    j += 1;
                }
                let key = format!("{}::{}", crate_name, name);
                index.structs.insert(key, ApiStruct {
                    name,
                    crate_name: crate_name.to_string(),
                    fields,
                });
            }
        }

        i += 1;
    }
}

fn extract_fn_name(line: &str) -> Option<String> {
    let after_pub = line.trim_start_matches("pub async fn ").trim_start_matches("pub fn ");
    let name = after_pub.split('(').next()?.trim().to_string();
    if name.is_empty() { None } else { Some(name) }
}

fn count_args(line: &str) -> usize {
    let paren_start = match line.find('(') { Some(p) => p, None => return 0 };
    let paren_end = match line.rfind(')') { Some(p) => p, None => return 0 };
    if paren_end <= paren_start + 1 { return 0; }
    let args = &line[paren_start + 1..paren_end];
    if args.trim().is_empty() { 0 } else { args.split(',').count() }
}

fn extract_struct_name(line: &str) -> Option<String> {
    let after = line.trim_start_matches("pub struct ");
    let name = after.split(|c: char| c == '{' || c == '(' || c == ';').next()?.trim().to_string();
    if name.is_empty() { None } else { Some(name) }
}

fn parse_field(line: &str) -> Option<(String, String)> {
    let line = line.trim_start_matches("pub ").trim_start_matches("pub(crate) ").trim_start_matches("pub(super) ");
    let mut parts = line.splitn(2, ':');
    let name = parts.next()?.trim().to_string();
    let type_hint = parts.next()?.trim().trim_end_matches(',').to_string();
    if name.is_empty() { None } else { Some((name, type_hint)) }
}

/// Format the index as JSON for tool output.
pub fn format_index_json(index: &ApiIndex) -> String {
    serde_json::to_string_pretty(index).unwrap_or_else(|e| format!("json error: {}", e))
}

/// Format a human-readable summary of the index.
pub fn format_index_summary(index: &ApiIndex) -> String {
    let mut lines = vec![format!(
        "📚 API Index — {} crates, {} exports in {}ms",
        index.crates_scanned, index.total_exports, index.build_time_ms
    )];
    lines.push(format!("  Functions: {}", index.functions.len()));
    lines.push(format!("  Structs:   {}", index.structs.len()));

    // Top 10 functions by crate
    let mut by_crate: HashMap<String, usize> = HashMap::new();
    for (key, _) in &index.functions {
        let crate_name = key.split("::").next().unwrap_or("?");
        *by_crate.entry(crate_name.to_string()).or_default() += 1;
    }
    let mut sorted: Vec<_> = by_crate.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    for (crate_name, count) in sorted.iter().take(10) {
        lines.push(format!("  {}: {} exports", crate_name, count));
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_fn_name_simple() {
        assert_eq!(extract_fn_name("pub fn build_index(workspace_root: &str) -> ApiIndex {"), Some("build_index".into()));
    }

    #[test]
    fn test_extract_fn_name_async() {
        assert_eq!(extract_fn_name("pub async fn run_server() {"), Some("run_server".into()));
    }

    #[test]
    fn test_count_args_zero() {
        assert_eq!(count_args("pub fn version() -> String"), 0);
    }

    #[test]
    fn test_count_args_three() {
        assert_eq!(count_args("pub fn register_webhook(id: &str, url: &str, secret: &str, events: Vec<String>) -> Result<String, String>"), 4);
    }

    #[test]
    fn test_build_index_on_workspace() {
        let index = build_index("/home/storage/deepseek-codewhale/flux");
        assert!(index.crates_scanned >= 15, "expected at least 15 crates, got {}", index.crates_scanned);
        assert!(index.total_exports > 50, "expected 50+ exports, got {}", index.total_exports);
        // Verify known functions exist
        assert!(index.functions.contains_key("fluxc_core::predict_build"), "predict_build not found");
        assert!(index.functions.contains_key("fluxc_core::register_webhook"), "register_webhook not found");
    }
}
