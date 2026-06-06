//! Flux fleet search — cross-node flux_search via SSH (read-only default).

use crate::handlers::platform_security::{reject_unknown_fields, sanitize_search_query, validate_node_name};
use crate::handlers::{ToolDef, ToolRegistry};
use flux_search::{SearchEngine, SearchQuery};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::PathBuf;

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_fleet_search",
            description: "Federated search: local flux_search + optional SSH fan-out to fleet nodes (epsilon, delta, beta). Deduplicates by URL. Read-only — no reindex unless reindex=true.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "nodes": { "type": "array", "items": { "type": "string" } },
                    "per_page": { "type": "integer" },
                    "reindex": { "type": "boolean" }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        },
        flux_fleet_search,
    );
}

fn index_path() -> PathBuf {
    std::env::var("FLUX_SEARCH_INDEX")
        .map(PathBuf::from)
        .unwrap_or_else(|_| crate::handlers::ws().join("target/flux-search/index.json"))
}

fn node_host(node: &str) -> Option<&'static str> {
    match node {
        "local" | "epsilon" => None,
        "delta" => Some("root@5.79.79.158"),
        "beta" => Some("root@185.182.185.227"),
        _ => None,
    }
}

fn local_search(query: &str, per_page: usize, reindex: bool) -> Vec<(String, String, f64)> {
    let mut engine = if reindex {
        SearchEngine::new()
    } else {
        SearchEngine::load_or_new(&index_path())
    };
    if reindex {
        let ws = crate::handlers::ws().to_string_lossy().to_string();
        let _ = engine.index_path(&ws, true);
        let _ = engine.save_to_path(&index_path());
    }
    let resp = engine.search(SearchQuery {
        q: query.into(),
        page: 1,
        per_page,
        category: None,
        language: None,
    });
    resp.results
        .iter()
        .map(|r| (r.url.clone(), "local".into(), r.score))
        .collect()
}

fn remote_search_via_ssh(host: &str, node: &str, query: &str) -> Vec<(String, String, f64)> {
    // Query passed via base64 env — no shell interpolation of user string.
    use base64::Engine;
    let q_b64 = base64::engine::general_purpose::STANDARD.encode(query.as_bytes());
    let remote_root = std::env::var("FLUX_REMOTE_ROOT")
        .unwrap_or_else(|_| "/home/storage/deepseek-codewhale/flux".into());
    let script = format!(
        "cd {remote_root} && FLUX_SEARCH_QUERY_B64={q_b64} python3 -c \"\
import os,json,base64,subprocess\n\
q=base64.b64decode(os.environ['FLUX_SEARCH_QUERY_B64']).decode()\n\
req=json.dumps({{'jsonrpc':'2.0','id':1,'method':'tools/call',\
'params':{{'name':'flux_search','arguments':{{'query':q,'per_page':8}}}}}})\n\
p=subprocess.run(['./target/debug/fluxc','mcp'],input=req.encode(),capture_output=True,timeout=120)\n\
print(p.stdout.decode()[-4000:])\n\""
    );
    let out = std::process::Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=12", host, &script])
        .output();
    let Ok(output) = out else {
        return vec![];
    };
    if !output.status.success() {
        return vec![];
    }
    let text = String::from_utf8_lossy(&output.stdout);
    // Best-effort parse: extract lines with file paths from search output
    let mut hits = Vec::new();
    for line in text.lines() {
        if line.contains('/') && (line.contains(".rs") || line.contains(".md") || line.contains(".html")) {
            let url = line.trim().to_string();
            if !url.is_empty() {
                hits.push((url, node.to_string(), 0.5));
            }
        }
    }
    hits.truncate(8);
    hits
}

fn flux_fleet_search(args: &Value) -> String {
    if let Err(e) = reject_unknown_fields(args, &["query", "nodes", "per_page", "reindex"]) {
        return format!("✗ flux_fleet_search: {e}");
    }
    let query = match args.get("query").and_then(|v| v.as_str()) {
        Some(q) => match sanitize_search_query(q) {
            Ok(s) => s,
            Err(e) => return format!("✗ flux_fleet_search: {e}"),
        },
        None => return "✗ flux_fleet_search: query required".into(),
    };
    let per_page = args.get("per_page").and_then(|v| v.as_u64()).unwrap_or(8) as usize;
    let reindex = args.get("reindex").and_then(|v| v.as_bool()).unwrap_or(false);
    let nodes: Vec<String> = args
        .get("nodes")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_else(|| vec!["local".into(), "delta".into()]);

    let mut merged: Vec<(String, String, f64)> = Vec::new();
    let mut seen = BTreeSet::new();
    let mut node_count = 0usize;

    for node in &nodes {
        if validate_node_name(node).is_err() {
            continue;
        }
        let hits = if node == "local" || node == "epsilon" {
            node_count += 1;
            local_search(&query, per_page, reindex && node_count == 1)
        } else if let Some(host) = node_host(node) {
            node_count += 1;
            remote_search_via_ssh(host, node, &query)
        } else {
            continue;
        };
        for (url, origin, score) in hits {
            if seen.insert(url.clone()) {
                merged.push((url, origin, score));
            }
        }
    }

    let payload = json!({
            "query": query,
            "node_count": node_count,
            "unique_hits": merged.len(),
        });


    fluxc_core::webhook::auto_dispatch("fleet_search", payload.clone());


    crate::handlers::platform_webhook::dispatch("flux_fleet_search", "fleet_search", payload);

    if merged.is_empty() {
        return format!(
            "📭 flux_fleet_search: no hits for '{query}' across {node_count} node(s)\n  hint: set reindex=true or run flux_search_index first"
        );
    }

    let mut out = format!(
        "✓ flux_fleet_search — {} unique hit(s) from {} node(s), query='{}':\n",
        merged.len(),
        node_count,
        query
    );
    for (i, (url, origin, score)) in merged.iter().take(per_page).enumerate() {
        out.push_str(&format!("  {}. [{}] score={:.2} {}\n", i + 1, origin, score, url));
    }
    out
}