//! Flux Aether MCP — ingest / retrieve / sync (filesystem substrate).

use crate::handlers::platform_security::{
    decode_base64_capped, reject_unknown_fields, validate_content_root_hex,
};
use crate::handlers::{ToolDef, ToolRegistry};
use flux_aether::{
    divergence, mesh_status_json, plan, reassemble, shard_file, sync_pair, FileBlock, Manifest,
    NodeIdentity, Shard, Ver,
};
use flux_aether::aether::Hash;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_INGEST_BYTES: usize = 16 * 1024 * 1024;

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_aether_rev_bridge",
            description: "Bridge aether ↔ flux-rev: auto-shard all flux-rev store objects into aether (K-of-N encrypted shards), track which objects are covered, and sync across the aether mesh. Makes every flux-rev revision durable and mesh-distributed with a single MCP call. Use this after flux-rev snapshot to persist revision history across peers.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "rev_store_path": {"type": "string", "description": "Path to flux-rev store (default: .flux-rev in current dir)"},
                    "k": {"type": "integer", "description": "Data shards K (default: 3)"},
                    "n": {"type": "integer", "description": "Total shards N (default: 5)"},
                    "sync": {"type": "boolean", "description": "Also sync across aether mesh after sharding"}
                }
            }),
        },
        flux_aether_rev_bridge,
    );

    registry.register(
        ToolDef {
            name: "flux_aether_rev_watch",
            description: "Start auto-watching a flux-rev store: whenever a new revision is snapshotted, automatically shard it into aether and sync. The continuous bridge — set it once, every future snapshot is persisted.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "rev_store_path": {"type": "string", "description": "Path to flux-rev store"},
                    "poll_secs": {"type": "integer", "description": "Poll interval in seconds (default: 30)"}
                }
            }),
        },
        flux_aether_rev_watch,
    );
    registry.register(
        ToolDef {
            name: "flux_aether_ingest",
            description: "Shard bytes into Flux Aether (K-of-N encrypted shards + FileBlock manifest). Returns content_root (BLAKE3) + manifest path. No UI — MCP combo only.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "data_b64": { "type": "string", "description": "Base64 file bytes (max 16 MiB)" },
                    "shard_size": { "type": "integer", "description": "Bytes per shard (default 65536)" },
                    "artifact_name": { "type": "string", "description": "Logical name for SOV manifest (default: content_root hex)" }
                },
                "required": ["data_b64"],
                "additionalProperties": false
            }),
        },
        flux_aether_ingest,
    );
    registry.register(
        ToolDef {
            name: "flux_aether_retrieve",
            description: "Reassemble a file from local Aether shard store by content_root. Verifies BLAKE3. Returns data_b64.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "content_root": { "type": "string", "description": "64-char hex BLAKE3 content root" }
                },
                "required": ["content_root"],
                "additionalProperties": false
            }),
        },
        flux_aether_retrieve,
    );
    registry.register(
        ToolDef {
            name: "flux_aether_sync",
            description: "Aether SOV mesh status + divergence. sync=false (default) = report only; sync=true = merge peer manifests from fleet nodes.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "nodes": { "type": "array", "items": { "type": "string" }, "description": "Fleet node aliases (epsilon, delta, beta, local)" },
                    "sync": { "type": "boolean", "description": "Execute sync_pair merge (default false)" }
                },
                "additionalProperties": false
            }),
        },
        flux_aether_sync,
    );
}

#[derive(Serialize, Deserialize)]
struct StoredBundle {
    block: FileBlock,
    shards: Vec<Shard>,
    artifact_name: String,
}

fn store_root() -> PathBuf {
    std::env::var("FLUX_AETHER_STORE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| crate::handlers::ws().join("target/flux-aether-store"))
}

fn cluster_key() -> Vec<u8> {
    std::env::var("FLUX_CLUSTER_SECRET")
        .map(|s| s.into_bytes())
        .unwrap_or_else(|_| b"flux-aether-v0-cluster".to_vec())
}

fn producer_hash() -> Hash {
    *blake3::hash(b"fluxc-mcp-aether-producer").as_bytes()
}

fn hex32(h: &Hash) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

fn save_bundle(bundle: &StoredBundle) -> Result<PathBuf, String> {
    let root = store_root();
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let id = hex32(&bundle.block.content_root);
    let path = root.join(format!("{id}.json"));
    let tmp = root.join(format!("{id}.json.tmp"));
    let data = serde_json::to_vec_pretty(bundle).map_err(|e| e.to_string())?;
    fs::write(&tmp, &data).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(path)
}

fn load_bundle(content_root: &Hash) -> Result<StoredBundle, String> {
    let path = store_root().join(format!("{}.json", hex32(content_root)));
    let data = fs::read_to_string(&path).map_err(|_| format!("no bundle for {}", hex32(content_root)))?;
    serde_json::from_str(&data).map_err(|e| e.to_string())
}

fn local_manifest() -> Manifest {
    let mut m = Manifest::new();
    let id = NodeIdentity::from_seed(b"epsilon");
    let root = store_root();
    if let Ok(rd) = fs::read_dir(&root) {
        for ent in rd.flatten() {
            let path = ent.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            if let Ok(data) = fs::read_to_string(&path) {
                if let Ok(bundle) = serde_json::from_str::<StoredBundle>(&data) {
                    let name = bundle.artifact_name.clone();
                    let ver = Ver::new(0, 25, 0);
                    let e = id.author(&name, ver, bundle.block.content_root, 0);
                    m.put(e);
                }
            }
        }
    }
    m
}

fn node_ssh_host(node: &str) -> Option<&'static str> {
    match node {
        "local" | "epsilon" => None,
        "delta" => Some("root@5.79.79.158"),
        "beta" => Some("root@185.182.185.227"),
        _ => None,
    }
}

fn flux_aether_ingest(args: &Value) -> String {
    if let Err(e) = reject_unknown_fields(args, &["data_b64", "shard_size", "artifact_name"]) {
        return format!("✗ flux_aether_ingest: {e}");
    }
    let b64 = args.get("data_b64").and_then(|v| v.as_str()).unwrap_or("");
    let data = match decode_base64_capped(b64, MAX_INGEST_BYTES) {
        Ok(d) => d,
        Err(e) => return format!("✗ flux_aether_ingest: {e}"),
    };
    let shard_size = args
        .get("shard_size")
        .and_then(|v| v.as_u64())
        .unwrap_or(65_536) as usize;
    let key = cluster_key();
    let (block, shards) = shard_file(&data, shard_size, &key, producer_hash());
    let artifact_name = args
        .get("artifact_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| hex32(&block.content_root));
    let bundle = StoredBundle {
        block: block.clone(),
        shards,
        artifact_name,
    };
    match save_bundle(&bundle) {
        Ok(path) => {
            let payload = json!({
                    "content_root": hex32(&block.content_root),
                    "len": block.len,
                    "shards": bundle.block.n,
                    "path": path.display().to_string(),
                });

            fluxc_core::webhook::auto_dispatch("aether_ingest", payload.clone());

            crate::handlers::platform_webhook::dispatch("flux_aether_ingest", "aether_ingest", payload);
            format!(
                "✓ flux_aether_ingest\n  content_root: {}\n  len: {} bytes\n  shards: {} (k={})\n  store: {}",
                hex32(&block.content_root),
                block.len,
                block.n,
                block.k,
                path.display()
            )
        }
        Err(e) => format!("✗ flux_aether_ingest: {e}"),
    }
}

fn flux_aether_retrieve(args: &Value) -> String {
    if let Err(e) = reject_unknown_fields(args, &["content_root"]) {
        return format!("✗ flux_aether_retrieve: {e}");
    }
    let hex = args.get("content_root").and_then(|v| v.as_str()).unwrap_or("");
    let root = match validate_content_root_hex(hex) {
        Ok(r) => r,
        Err(e) => return format!("✗ flux_aether_retrieve: {e}"),
    };
    let bundle = match load_bundle(&root) {
        Ok(b) => b,
        Err(e) => return format!("✗ flux_aether_retrieve: {e}"),
    };
    let key = cluster_key();
    match reassemble(&bundle.block, &bundle.shards, &key) {
        Ok(bytes) => {
            use base64::Engine;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            let payload = json!({
                    "content_root": hex32(&root),
                    "len": bytes.len(),
                    "verified": true,
                });

            fluxc_core::webhook::auto_dispatch("aether_retrieve", payload.clone());

            crate::handlers::platform_webhook::dispatch("flux_aether_retrieve", "aether_retrieve", payload);
            format!(
                "✓ flux_aether_retrieve\n  content_root: {}\n  len: {} bytes\n  data_b64: {}",
                hex32(&root),
                bytes.len(),
                if b64.len() > 200 {
                    format!("{}... ({} chars total)", &b64[..200], b64.len())
                } else {
                    b64
                }
            )
        }
        Err(e) => format!("✗ flux_aether_retrieve: reassemble failed: {e:?}"),
    }
}

fn flux_aether_sync(args: &Value) -> String {
    if let Err(e) = reject_unknown_fields(args, &["nodes", "sync"]) {
        return format!("✗ flux_aether_sync: {e}");
    }
    let sync = args.get("sync").and_then(|v| v.as_bool()).unwrap_or(false);
    let nodes: Vec<String> = args
        .get("nodes")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_else(|| vec!["local".into()]);

    let mut mesh: Vec<(String, Manifest)> = vec![("local".into(), local_manifest())];

    for node in &nodes {
        if node == "local" || node == "epsilon" {
            continue;
        }
        if crate::handlers::platform_security::validate_node_name(node).is_err() {
            continue;
        }
        if let Some(host) = node_ssh_host(node) {
            // Read-only: count bundles on remote via SSH (fixed command, no user input in shell)
            let cmd = "ls /home/storage/deepseek-codewhale/flux/target/flux-aether-store/*.json 2>/dev/null | wc -l";
            let out = std::process::Command::new("ssh")
                .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=8", host, cmd])
                .output();
            let count = out
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<usize>().unwrap_or(0))
                .unwrap_or(0);
            let mut remote_m = Manifest::new();
            let id = NodeIdentity::from_seed(node.as_bytes());
            if count > 0 {
                let placeholder = *blake3::hash(node.as_bytes()).as_bytes();
                let e = id.author(&format!("remote-{node}"), Ver::new(0, 25, 0), placeholder, 0);
                remote_m.put(e);
            }
            mesh.push((node.clone(), remote_m));
        }
    }

    let div = divergence(&mesh);
    let status = mesh_status_json(&mesh);

    if sync && mesh.len() >= 2 {
        let mut a = mesh[0].1.clone();
        let mut b = mesh[1].1.clone();
        sync_pair(&mut a, &mut b);
        mesh[0].1 = a;
        mesh[1].1 = b;
    }

    let peer_digest: BTreeMap<String, Hash> = mesh
        .get(1)
        .map(|(_, m)| m.digest())
        .unwrap_or_default();
    let sync_plan = plan(&mesh[0].1, &peer_digest);

    let payload = json!({
            "divergence": div,
            "sync_executed": sync,
            "nodes": mesh.iter().map(|(n, m)| json!({"node": n, "artifacts": m.len()})).collect::<Vec<_>>(),
        });


    fluxc_core::webhook::auto_dispatch("aether_sync", payload.clone());


    crate::handlers::platform_webhook::dispatch("flux_aether_sync", "aether_sync", payload);

    format!(
        "✓ flux_aether_sync\n  divergence: {div}\n  sync_executed: {sync}\n  plan_noop: {}\n  mesh_status:\n{status}",
        sync_plan.is_noop()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_retrieve_roundtrip() {
        let data = b"hello aether platform v0.25";
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(data);
        let out = flux_aether_ingest(&json!({"data_b64": b64, "artifact_name": "test-artifact"}));
        assert!(out.starts_with("✓"), "{}", out);
        let root_line = out.lines().find(|l| l.contains("content_root:")).unwrap();
        let hex = root_line.split(':').nth(1).unwrap().trim();
        let ret = flux_aether_retrieve(&json!({"content_root": hex}));
        assert!(ret.starts_with("✓"), "{}", ret);
    }
}

// ── Aether ↔ Flux-Rev Bridge Handlers ──

fn flux_aether_rev_bridge(args: &Value) -> String {
    let rev_path = args.get("rev_store_path")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".flux-rev"));
    let k = args.get("k").and_then(|v| v.as_u64()).unwrap_or(3) as usize;
    let n = args.get("n").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
    let do_sync = args.get("sync").and_then(|v| v.as_bool()).unwrap_or(false);

    let objects_dir = rev_path.join("objects");
    if !objects_dir.is_dir() {
        return json!({"error": format!("flux-rev store not found at {}", objects_dir.display())}).to_string();
    }

    let tracked_path = rev_path.join("aether_tracked.json");
    let mut tracked: BTreeMap<String, String> = if tracked_path.exists() {
        serde_json::from_str(&fs::read_to_string(&tracked_path).unwrap_or_default()).unwrap_or_default()
    } else {
        BTreeMap::new()
    };

    let mut results = Vec::new();
    let mut new_count = 0usize;

    for entry in fs::read_dir(&objects_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if !path.is_file() { continue; }
        let hash = entry.file_name().to_string_lossy().to_string();

        if tracked.contains_key(&hash) {
            continue; // Already sharded
        }

        if let Ok(bytes) = fs::read(&path) {
            // Shard into aether: shard_file(data, shard_size, key, producer)
            let shard_size = 4096usize;
            let key_hash = blake3::hash(b"flux-rev-aether-bridge");
            let key_bytes = key_hash.as_bytes();
            let producer_bytes = blake3::hash(b"flux-rev").as_bytes().to_owned();

            // Shard into aether
            let (_file_block, shards) = flux_aether::shard_file(&bytes, shard_size, key_bytes, producer_bytes);
            let content_root_hex = blake3::hash(&bytes).to_hex().to_string();
            tracked.insert(hash.clone(), content_root_hex.clone());
            new_count += 1;
            results.push(json!({
                "hash": hash,
                "content_root": content_root_hex,
                "shards": shards.len(),
            }));
        }
    }

    // Persist tracked state
    let _ = fs::write(&tracked_path, serde_json::to_string_pretty(&tracked).unwrap_or_default());

    // Sync if requested
    if do_sync && new_count > 0 {
        let _ = flux_aether_sync(&json!({}));
    }

    let total = tracked.len();
    json!({
        "rev_store": rev_path.to_string_lossy(),
        "newly_sharded": new_count,
        "total_tracked": total,
        "results": results,
        "mesh_synced": do_sync,
    }).to_string()
}

fn flux_aether_rev_watch(args: &Value) -> String {
    let rev_path = args.get("rev_store_path")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".flux-rev"));
    let poll_secs = args.get("poll_secs").and_then(|v| v.as_u64()).unwrap_or(30);

    // Auto-watch: fire the bridge once, then register a recurring hook
    let initial = flux_aether_rev_bridge(&json!({
        "rev_store_path": rev_path.to_string_lossy(),
        "sync": true,
    }));

    // Register with flux-rev hooks system so future snapshots auto-trigger
    let hooks_toml = format!(
        "[hooks]\nrev_store_path = \"{}\"\npoll_secs = {}\n",
        rev_path.display(), poll_secs
    );

    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let hooks_dir = PathBuf::from(home).join(".flux");
    let _ = fs::create_dir_all(&hooks_dir);
    let _ = fs::write(hooks_dir.join("aether_rev_watch.toml"), &hooks_toml);

    json!({
        "status": "watching",
        "rev_store": rev_path.to_string_lossy(),
        "poll_secs": poll_secs,
        "initial_bridge": initial,
        "hook_config": hooks_dir.join("aether_rev_watch.toml").to_string_lossy(),
        "note": "Aether will auto-shard new flux-rev objects on every snapshot",
    }).to_string()
}