// fluxc-core/p2p_worker.rs — fluxc auto-update over HTTPS, with optional
// libp2p gossip notification layered on top.
//
// Architecture (the simple half — what runs today):
//
//   Epsilon (publisher)
//     `fluxc release [VERSION]`
//       ├─ figures out the current binary path
//       ├─ copies it to /home/orobit/q-narwhalknight/dist-final/downloads/fluxc-vX.Y.Z-musl
//       ├─ computes sha256 + blake3 + size
//       └─ writes /downloads/fluxc-latest.json manifest
//
//   Delta (consumer)
//     `fluxc auto-update [--interval N] [--apply]`
//       loop:
//         ├─ GET https://quillon.xyz/downloads/fluxc-latest.json
//         ├─ if manifest.version > self.version:
//         │     ├─ GET manifest.url → /tmp/fluxc.new.<v>
//         │     ├─ verify sha256 matches manifest.sha256_hex
//         │     ├─ chmod +x
//         │     └─ if --apply: atomic rename → current_exe() path
//         └─ sleep(interval)
//
// HTTP uses curl subprocess (matches `webhook.rs::send_http_post` pattern).
// No new deps. The libp2p gossip "release available" notification stays as
// a TODO at the end of `publish_release`; the HTTPS poll covers the actual
// distribution.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseManifest {
    /// Product name. Default `fluxc` for backwards compat. Lets one manifest
    /// directory hold multiple products (`fluxc-latest.json`,
    /// `flux-arena-latest.json`, `flux-arena-server-latest.json`).
    #[serde(default = "default_product")]
    pub product: String,
    pub version: String,
    pub url: String,
    /// Absent on sigil-* manifests, which carry only `blake3_hex` — `flux_release_check`
    /// failed on them with `missing field sha256_hex` until 2026-09-13. Optional fields
    /// here mean "this manifest did not say", never a fabricated value.
    #[serde(default)]
    pub sha256_hex: String,
    pub blake3_hex: String,
    pub size_bytes: u64,
    #[serde(default)]
    pub released_at_us: u64,
    #[serde(default)]
    pub publisher: String,
    #[serde(default)]
    pub publisher_wallet_hex: String,
    #[serde(default)]
    pub notes: String,
    /// sigil-top style extras, kept so a checker can show them: release channel,
    /// the flux-rev stamp of the source, the git tag, and per-target artifacts.
    #[serde(default)]
    pub channel: String,
    #[serde(default)]
    pub flux_rev: String,
    #[serde(default)]
    pub source_tag: String,
    #[serde(default)]
    pub targets: std::collections::BTreeMap<String, serde_json::Value>,
}

/// The download root a product publishes to. SIGIL products live on sigilgraph.org
/// and Quillon/Flux products on quillon.xyz — two projects, two homes; a SIGIL
/// manifest must never be looked up (or handed out) on the Quillon domain.
pub fn url_base_for(product: &str) -> &'static str {
    if product.starts_with("sigil-") { SIGIL_URL_BASE } else { DEFAULT_URL_BASE }
}

fn default_product() -> String { "fluxc".into() }

const DEFAULT_FLUXC_MANIFEST_URL: &str =
    "https://quillon.xyz/downloads/fluxc-latest.json";
const DEFAULT_DOWNLOADS_DIR: &str =
    "/home/orobit/q-narwhalknight/dist-final/downloads";
/// fluxapp.xyz's download root; `fluxc release` mirrors the signed release there too.
const DEFAULT_MIRROR_DIR: &str =
    "/home/orobit/q-narwhalknight/dist-fluxapp/downloads";
const DEFAULT_URL_BASE: &str =
    "https://quillon.xyz/downloads";
const SIGIL_URL_BASE: &str =
    "https://sigilgraph.org/downloads";

/// Manifest URL for an arbitrary product. Mirrors the convention used by the
/// default `fluxc-latest.json` — every product publishes to
/// `${url_base}/${product}-latest.json`.
pub fn manifest_url_for(product: &str) -> String {
    format!("{}/{}-latest.json", url_base_for(product), product)
}

/// Suffix that distinguishes the on-disk binary name. Defaults to `musl` for
/// fluxc (historical) and "linux-x86_64" for everything else, matching the
/// project's existing distribution naming.
fn binary_suffix(product: &str) -> &'static str {
    match product {
        "fluxc" => "musl",
        _ => "linux-x86_64",
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Release signing (2026-10-23)
// ─────────────────────────────────────────────────────────────────────────────
//
// Until 2026-10-23 a Flux release was trusted on TLS alone. The manifest named a
// sha256, but it came unsigned from the same server as the binary, so whoever could
// change one could change both — and `fluxc self-update` did not even read the
// manifest. That server (Epsilon) had third parties with root from July to
// September 2026. Now every Flux manifest carries a detached Ed25519 signature at
// `<manifest>.sig` (128 hex, over the EXACT bytes), made by the release key below,
// and every Flux updater refuses a manifest that does not verify against the key
// compiled into it. SIGIL products (`sigil-*`) have their own pinned key and their
// own release ceremony; this code refuses to publish or install them.

/// The Flux release public key (Ed25519, 32 bytes hex). The secret half lives at
/// `/root/.config/flux/release-sign.seed` on the release host (root-only). Changing
/// this constant orphans every installed fluxc that pinned the old one.
pub const FLUX_RELEASE_PUBKEY_HEX: &str =
    "e9b6aecec3531889a49d39e148663187b3bf728ec178429aa75c50580d20d78a";
const FLUX_RELEASE_SEED_DEFAULT: &str = "/root/.config/flux/release-sign.seed";

/// Check a detached 128-hex Ed25519 signature over `body` against the pinned key.
/// Pure — the caller fetches. Fails closed on anything that is not a valid signature
/// by that key (including an HTML page served where the `.sig` should be).
pub fn verify_release_sig(body: &[u8], sig_hex: &str) -> Result<(), String> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let pk: [u8; 32] = hex::decode(FLUX_RELEASE_PUBKEY_HEX).ok()
        .and_then(|v| v.try_into().ok())
        .ok_or_else(|| "pinned release key malformed".to_string())?;
    let sig: [u8; 64] = hex::decode(sig_hex.trim()).ok()
        .and_then(|v| v.try_into().ok())
        .ok_or_else(|| format!(
            "release signature malformed (want 128-hex ed25519, got {} chars — an HTML page means the .sig is absent)",
            sig_hex.trim().len()))?;
    let vk = VerifyingKey::from_bytes(&pk).map_err(|e| format!("pinned release key invalid: {e}"))?;
    vk.verify(body, &Signature::from_bytes(&sig))
        .map_err(|_| "RELEASE SIGNATURE INVALID — the manifest is not signed by the pinned Flux release key; refusing".to_string())
}

/// Sign `body` with the release seed (`FLUX_RELEASE_SEED`, default the root-only file).
/// Refuses a seed whose public key is not the pinned one: a wrong seed must fail here,
/// on the release host, not at every client afterwards.
fn sign_release(body: &[u8]) -> Result<String, String> {
    let path = std::env::var("FLUX_RELEASE_SEED").unwrap_or_else(|_| FLUX_RELEASE_SEED_DEFAULT.into());
    sign_release_with(&path, body)
}

fn sign_release_with(path: &str, body: &[u8]) -> Result<String, String> {
    use ed25519_dalek::{Signer, SigningKey};
    let seed: [u8; 32] = std::fs::read(path)
        .map_err(|e| format!("release seed {path}: {e}"))?
        .try_into()
        .map_err(|_| format!("release seed {path} is not 32 bytes"))?;
    let sk = SigningKey::from_bytes(&seed);
    let pk_hex = hex_encode(sk.verifying_key().as_bytes());
    if pk_hex != FLUX_RELEASE_PUBKEY_HEX {
        return Err(format!("release seed {path} has pubkey {}…, not the pinned {}… — refusing to sign", &pk_hex[..8], &FLUX_RELEASE_PUBKEY_HEX[..8]));
    }
    let sig = hex_encode(&sk.sign(body).to_bytes());
    verify_release_sig(body, &sig)?;
    Ok(sig)
}

/// Fetch `<manifest_url>` and `<manifest_url>.sig`, verify, parse. The ONE way a Flux
/// updater learns what the newest release is.
pub fn fetch_verified_manifest(manifest_url: &str) -> Result<ReleaseManifest, String> {
    let (m, body) = fetch_manifest_raw(manifest_url)?;
    if m.product.starts_with("sigil-") {
        return Err(format!("{} is a SIGIL product — it updates itself with SIGIL's own pinned key, not through fluxc", m.product));
    }
    let sig = http_get_string(&format!("{manifest_url}.sig"), 10)
        .map_err(|e| format!("no release signature at {manifest_url}.sig ({e}) — an unsigned manifest is not a release"))?;
    verify_release_sig(body.as_bytes(), &sig)?;
    Ok(m)
}

/// Download `manifest.url` to a temp file and check it against the manifest's hash
/// (and size, when stated). The manifest must already be verified.
pub fn download_verified(manifest: &ReleaseManifest) -> Result<PathBuf, String> {
    let dl_tmp = std::env::temp_dir().join(format!("{}.new.{}", manifest.product, manifest.version));
    http_get_to_file(&manifest.url, &dl_tmp, 120)?;
    let bytes = std::fs::read(&dl_tmp).map_err(|e| format!("read tmp: {}", e))?;
    // A manifest names the artifact by sha256 (fluxc) or by blake3 only (sigil-*);
    // check whichever it carries, and refuse one that carries neither — an unhashed
    // download is not a release. Never slice an empty digest (that was a panic).
    let (algo, want, got) = if !manifest.sha256_hex.is_empty() {
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        ("sha256", manifest.sha256_hex.clone(), hex_encode(&hasher.finalize()))
    } else if !manifest.blake3_hex.is_empty() {
        ("blake3", manifest.blake3_hex.clone(), blake3::hash(&bytes).to_hex().to_string())
    } else {
        let _ = std::fs::remove_file(&dl_tmp);
        return Err("manifest carries neither sha256_hex nor blake3_hex — refusing an unhashed release".into());
    };
    if got != want || (manifest.size_bytes != 0 && manifest.size_bytes != bytes.len() as u64) {
        let _ = std::fs::remove_file(&dl_tmp);
        return Err(format!(
            "{} / size mismatch — expected {}… ({} B), got {}… ({} B)",
            algo, &want[..16.min(want.len())], manifest.size_bytes, &got[..16.min(got.len())], bytes.len()
        ));
    }
    println!("✓ {} matches ({} bytes)", algo, bytes.len());
    let _ = Command::new("chmod").arg("+x").arg(&dl_tmp).status();
    Ok(dl_tmp)
}

/// A path that is a cargo build output (`…/target/<profile>/…`). Installing a
/// download over one would overwrite a developer build — on Epsilon every session's
/// `fluxc` is `target/debug/fluxc` (via ~/.flux/bin/fluxc → symlink).
fn is_build_output(p: &Path) -> bool {
    let s = p.to_string_lossy();
    if s.contains("/target/debug/") || s.contains("/target/release/") || s.contains("/target/release-fast/") {
        return true;
    }
    // Any profile, any target triple (`target/x86_64-unknown-linux-musl/release/fluxc` slipped through the
    // path test above): cargo marks every target dir with a CACHEDIR.TAG carrying this signature.
    p.ancestors().skip(1).any(|a| {
        std::fs::read_to_string(a.join("CACHEDIR.TAG"))
            .map(|t| t.starts_with("Signature: 8a477f597d28d172789f06886806bc55"))
            .unwrap_or(false)
    })
}

/// Put a verified download in place at `target`: refuses a symlink (it would write
/// THROUGH it) and a cargo build output; renames atomically when it can.
pub fn install_verified(downloaded: &Path, target: &Path) -> Result<(), String> {
    if std::fs::symlink_metadata(target).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
        let to = std::fs::read_link(target).map(|p| p.display().to_string()).unwrap_or_default();
        return Err(format!("{} is a symlink to {} — refusing to write through it (replace the link yourself if you mean it)", target.display(), to));
    }
    if is_build_output(target) {
        return Err(format!("{} is a cargo build output — refusing to overwrite a developer build", target.display()));
    }
    let staged = target.with_extension("new");
    std::fs::copy(downloaded, &staged).map_err(|e| format!("copy to {}: {}", staged.display(), e))?;
    let _ = Command::new("chmod").arg("+x").arg(&staged).status();
    std::fs::rename(&staged, target).map_err(|e| format!("rename into {}: {}", target.display(), e))?;
    let _ = std::fs::remove_file(downloaded);
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Publisher (release): runs on Epsilon
// ─────────────────────────────────────────────────────────────────────────────

pub fn publish_release(version: &str, _legacy_hash: &str, _legacy_sig: &str) {
    publish_release_product("fluxc", version, None).unwrap_or_else(|e| {
        eprintln!("flux release: {}", e);
        std::process::exit(1);
    });
}

/// Publish a release manifest for an arbitrary product. The artifact is
/// `explicit_binary` if provided, otherwise the current executable. The
/// manifest is written to `${FLUX_DOWNLOADS_DIR}/${product}-latest.json` and
/// the binary to `${FLUX_DOWNLOADS_DIR}/${product}-v${version}-${suffix}`.
pub fn publish_release_product(
    product: &str,
    version: &str,
    explicit_binary: Option<PathBuf>,
) -> Result<(), String> {
    if product.is_empty() {
        return Err("product name required".into());
    }
    if product.starts_with("sigil-") {
        return Err(format!(
            "{product} is a SIGIL product with its own signed ceremony (scripts/release-sigil-top.sh) — \
             fluxc release would write an UNSIGNED {product}-latest.json over the signed one"
        ));
    }
    println!("📡 fluxc release — product={} version={}", product, version);

    let src = match explicit_binary {
        Some(p) => p,
        None => std::env::current_exe()
            .map_err(|e| format!("current_exe: {}", e))?,
    };
    let bytes = std::fs::read(&src)
        .map_err(|e| format!("read {}: {}", src.display(), e))?;
    println!(
        "   source: {} ({:.2} MB)",
        src.display(),
        bytes.len() as f64 / (1024.0 * 1024.0)
    );

    let sha256_hex = {
        let mut h = Sha256::new();
        h.update(&bytes);
        hex_encode(&h.finalize())
    };
    let blake3_hex = {
        let mut h = blake3::Hasher::new();
        h.update(&bytes);
        hex_encode(h.finalize().as_bytes())
    };
    println!("   sha256:  {}", &sha256_hex[..40]);
    println!("   blake3:  {}", &blake3_hex[..40]);

    let downloads_dir = std::env::var("FLUX_DOWNLOADS_DIR")
        .unwrap_or_else(|_| DEFAULT_DOWNLOADS_DIR.into());
    std::fs::create_dir_all(&downloads_dir)
        .map_err(|e| format!("mkdir {}: {}", downloads_dir, e))?;

    let bin_name = format!("{}-v{}-{}", product, version, binary_suffix(product));
    let manifest_name = format!("{}-latest.json", product);
    let dst_bin = Path::new(&downloads_dir).join(&bin_name);
    let dst_manifest = Path::new(&downloads_dir).join(&manifest_name);

    let tmp_bin = dst_bin.with_extension("tmp");
    std::fs::write(&tmp_bin, &bytes)
        .map_err(|e| format!("write {}: {}", tmp_bin.display(), e))?;
    let _ = Command::new("chmod").arg("+x").arg(&tmp_bin).status();
    std::fs::rename(&tmp_bin, &dst_bin)
        .map_err(|e| format!("rename: {}", e))?;
    println!("   wrote:   {}", dst_bin.display());

    let now_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0);
    let publisher = std::env::var("FLUX_RELEASE_PUBLISHER").unwrap_or_else(|_| {
        format!(
            "{}@{}",
            std::env::var("USER").unwrap_or_else(|_| "anon".into()),
            hostname_short(),
        )
    });
    let wallet = std::env::var("FLUX_AGENT_WALLET").unwrap_or_default();
    let url_base = std::env::var("FLUX_RELEASE_URL_BASE")
        .unwrap_or_else(|_| "https://quillon.xyz/downloads".into());
    let url = format!("{}/{}", url_base.trim_end_matches('/'), bin_name);

    let manifest = ReleaseManifest {
        product: product.to_string(),
        version: version.to_string(),
        url,
        sha256_hex,
        blake3_hex,
        size_bytes: bytes.len() as u64,
        released_at_us: now_us,
        publisher,
        publisher_wallet_hex: wallet,
        notes: std::env::var("FLUX_RELEASE_NOTES").unwrap_or_default(),
        channel: String::new(),
        flux_rev: String::new(),
        source_tag: String::new(),
        targets: Default::default(),
    };
    let manifest_json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| format!("serialize manifest: {}", e))?;
    // Sign the EXACT bytes that get written; nothing may re-serialise them afterwards.
    let sig_hex = sign_release(manifest_json.as_bytes())?;
    println!("   signed:  {} ({}…)", manifest_name, &FLUX_RELEASE_PUBKEY_HEX[..8]);

    // ONE channel: identical signed bytes in every download root. On 2026-10-23 the two
    // roots disagreed three ways (quillon.xyz said 0.28.0, fluxapp.xyz 0.36.0, and the
    // self-update file was 0.40.0). FLUX_DOWNLOADS_MIRRORS (':'-separated) adds roots;
    // the default root also feeds fluxapp.xyz's.
    let mirrors: Vec<PathBuf> = match std::env::var("FLUX_DOWNLOADS_MIRRORS") {
        Ok(v) => v.split(':').filter(|s| !s.is_empty()).map(PathBuf::from).collect(),
        Err(_) if downloads_dir == DEFAULT_DOWNLOADS_DIR => vec![PathBuf::from(DEFAULT_MIRROR_DIR)],
        Err(_) => Vec::new(),
    };
    let mut roots = vec![PathBuf::from(&downloads_dir)];
    roots.extend(mirrors.into_iter().filter(|m| m.is_dir()));
    let put = |dst: &Path, data: &[u8]| -> Result<(), String> {
        let tmp = dst.with_extension("tmp");
        std::fs::write(&tmp, data).map_err(|e| format!("write {}: {}", tmp.display(), e))?;
        std::fs::rename(&tmp, dst).map_err(|e| format!("rename {}: {}", dst.display(), e))
    };
    for root in &roots {
        if root != Path::new(&downloads_dir) {
            put(&root.join(&bin_name), &bytes)?;
            let _ = Command::new("chmod").arg("+x").arg(root.join(&bin_name)).status();
        }
        // The stable name humans and setup-flux.sh fetch. Overwritten on purpose, atomically;
        // every versioned file stays.
        if product == "fluxc" {
            put(&root.join("fluxc-musl-x64"), &bytes)?;
            let _ = Command::new("chmod").arg("+x").arg(root.join("fluxc-musl-x64")).status();
        }
        // signature first, then the manifest it signs
        put(&root.join(format!("{manifest_name}.sig")), sig_hex.as_bytes())?;
        put(&root.join(&manifest_name), manifest_json.as_bytes())?;
        // read back what landed and check it, not what we meant to write
        let got_m = std::fs::read(root.join(&manifest_name)).map_err(|e| format!("re-read manifest: {e}"))?;
        let got_s = std::fs::read_to_string(root.join(format!("{manifest_name}.sig"))).map_err(|e| format!("re-read sig: {e}"))?;
        verify_release_sig(&got_m, &got_s)?;
        println!("   ✓ {}: manifest + .sig verify against the pinned key", root.display());
    }
    println!("   manifest:{}", dst_manifest.display());
    println!(
        "✓ Release v{} published — Delta will pull on its next auto-update tick",
        version
    );
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Auto-update (consumer): runs on Delta (or any node)
// ─────────────────────────────────────────────────────────────────────────────

pub fn run_auto_updater() {
    let interval = std::env::var("FLUX_AUTOUPDATE_INTERVAL_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(60);
    let apply = std::env::var("FLUX_AUTOUPDATE_APPLY")
        .map(|v| v == "1" || v.to_lowercase() == "true")
        .unwrap_or(false);
    run_auto_updater_with(interval, apply);
}

pub fn run_auto_updater_with(interval_secs: u64, apply: bool) {
    let manifest_url = std::env::var("FLUX_MANIFEST_URL")
        .unwrap_or_else(|_| DEFAULT_FLUXC_MANIFEST_URL.into());
    let self_version = env!("CARGO_PKG_VERSION").to_string();
    let target_path = std::env::current_exe().ok();

    println!("⚡ fluxc auto-update");
    println!("   self:     v{}", self_version);
    println!("   manifest: {}", manifest_url);
    println!(
        "   target:   {}",
        target_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<unknown>".into())
    );
    println!("   interval: {}s   apply: {}", interval_secs, apply);

    loop {
        match check_once(&manifest_url, &self_version, target_path.as_deref(), apply) {
            Ok(CheckOutcome::UpToDate { version }) => {
                println!("✓ {} — up to date (v{})", now_short(), version);
            }
            Ok(CheckOutcome::Downloaded { version, applied, path }) => {
                if applied {
                    println!(
                        "🚀 {} — applied v{} → {}",
                        now_short(),
                        version,
                        path.display()
                    );
                    println!(
                        "   Restart the process / systemd unit to start using the new binary."
                    );
                } else {
                    println!(
                        "📥 {} — downloaded v{} to {} (pass --apply or set FLUX_AUTOUPDATE_APPLY=1 to install)",
                        now_short(),
                        version,
                        path.display()
                    );
                }
            }
            Err(e) => {
                eprintln!("⚠ {} — auto-update tick failed: {}", now_short(), e);
            }
        }
        std::thread::sleep(Duration::from_secs(interval_secs.max(5)));
    }
}

#[derive(Debug)]
enum CheckOutcome {
    UpToDate { version: String },
    Downloaded {
        version: String,
        applied: bool,
        path: PathBuf,
    },
}

fn check_once(
    manifest_url: &str,
    self_version: &str,
    target_path: Option<&Path>,
    apply: bool,
) -> Result<CheckOutcome, String> {
    // Signature first: nothing in an unverified manifest is trusted, not even its version.
    let manifest = fetch_verified_manifest(manifest_url)?;

    if !semver_gt(&manifest.version, self_version) {
        return Ok(CheckOutcome::UpToDate { version: manifest.version });
    }

    println!("🔔 new release available: v{} → v{}", self_version, manifest.version);

    let dl_tmp = download_verified(&manifest)?;

    if !apply || target_path.is_none() {
        return Ok(CheckOutcome::Downloaded {
            version: manifest.version,
            applied: false,
            path: dl_tmp,
        });
    }
    let target = target_path.unwrap().to_path_buf();
    install_verified(&dl_tmp, &target)?;
    Ok(CheckOutcome::Downloaded {
        version: manifest.version,
        applied: true,
        path: target,
    })
}

/// Read-only manifest fetch + parse. Used by `flux_release_check` MCP tool —
/// returns the current manifest without downloading the binary or applying it.
pub fn fetch_manifest(manifest_url: &str) -> Result<ReleaseManifest, String> {
    fetch_manifest_raw(manifest_url).map(|(m, _)| m)
}

/// Fetch and parse, but also hand back the EXACT bytes — a detached signature
/// is over those bytes, so a verifier must not re-serialize. An SPA fallback
/// answering a missing manifest with its landing page (HTTP 200, text/html —
/// measured on sigilgraph.org 2026-09-02) fails here as a parse error that
/// names the first bytes, not as a mysterious "bad signature" later.
pub fn fetch_manifest_raw(manifest_url: &str) -> Result<(ReleaseManifest, String), String> {
    let body = http_get_string(manifest_url, 10)?;
    let m: ReleaseManifest = serde_json::from_str(&body).map_err(|e| {
        let head: String = body.chars().take(48).collect();
        format!("parse manifest: {} — body starts {:?} (an HTML page here means the file is ABSENT and an SPA fallback answered)", e, head)
    })?;
    Ok((m, body))
}

// ─────────────────────────────────────────────────────────────────────────────
// QuillonOS module staging (v0.17.x — `fluxc os-stage`)
// ─────────────────────────────────────────────────────────────────────────────
//
// Generalizes the wasm-shipping pipeline I hand-rolled today for `init` and
// `sh`. One invocation cargo-builds N packages to wasm32-wasip1, hashes them,
// writes stub SQIsign proofs, and merges the result into
// `<output_dir>/manifest.json` while preserving entries for modules this run
// didn't touch.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StagedModule {
    pub package: String,
    pub bin_name: String,
    pub wasm_path: PathBuf,
    pub proof_path: PathBuf,
    pub size_bytes: u64,
    pub blake3_hex: String,
    pub compiled_at_us: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageReport {
    pub output_dir: PathBuf,
    pub manifest_path: PathBuf,
    pub fluxc_version: String,
    pub agent_wallet: Option<String>,
    pub modules: Vec<StagedModule>,
    pub preserved_entries: usize,
}

const WASI_TARGET: &str = "wasm32-wasip1";

/// Build the listed packages to `wasm32-wasip1`, stage artifacts + proofs,
/// merge into the QuillonOS manifest at `<output_dir>/manifest.json`.
pub fn os_stage_modules(packages: &[&str], output_dir: &Path) -> Result<StageReport, String> {
    if packages.is_empty() {
        return Err("at least one --package required".into());
    }
    ensure_wasi_target()?;
    let workspace_root = find_workspace_root()?;
    let target_dir = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| workspace_root.join("target"));
    let wasm_release = target_dir.join(WASI_TARGET).join("release");

    let wasm_out = output_dir.join("wasm");
    let proof_out = output_dir.join("proofs");
    std::fs::create_dir_all(&wasm_out).map_err(|e| format!("mkdir wasm: {}", e))?;
    std::fs::create_dir_all(&proof_out).map_err(|e| format!("mkdir proofs: {}", e))?;

    println!("📦 fluxc os-stage — {} package(s) → {}", packages.len(), output_dir.display());

    // Build all packages in one cargo invocation (shares dep graph + faster).
    let mut cmd = Command::new("cargo");
    cmd.arg("build")
        .arg("--target").arg(WASI_TARGET)
        .arg("--release")
        .current_dir(&workspace_root);
    for p in packages {
        cmd.arg("--package").arg(p);
    }
    let status = cmd.status()
        .map_err(|e| format!("cargo build: {}", e))?;
    if !status.success() {
        return Err(format!("cargo build exited {}", status));
    }

    let agent_wallet = std::env::var("FLUX_AGENT_WALLET").ok();
    let fluxc_version = env!("CARGO_PKG_VERSION").to_string();
    let now_us = SystemTime::now().duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64).unwrap_or(0);

    let mut staged: Vec<StagedModule> = Vec::new();
    for pkg in packages {
        // Default binary name: package with `-` → `_` (cargo's convention),
        // but for `quillonos-init` the bin is named `init` (declared in
        // Cargo.toml [[bin]] name). Try both — prefer matching bin name.
        let candidates = [
            pkg.trim_start_matches("quillonos-").to_string(),
            pkg.to_string(),
            pkg.replace('-', "_"),
        ];
        let mut found: Option<(String, PathBuf)> = None;
        for c in &candidates {
            let p = wasm_release.join(format!("{}.wasm", c));
            if p.exists() { found = Some((c.clone(), p)); break; }
        }
        let (bin_name, src) = found.ok_or_else(|| format!(
            "no wasm artifact for package '{}' in {}", pkg, wasm_release.display()
        ))?;

        let bytes = std::fs::read(&src).map_err(|e| format!("read {}: {}", src.display(), e))?;
        let size_bytes = bytes.len() as u64;
        let blake3_hex = {
            let mut h = blake3::Hasher::new();
            h.update(&bytes);
            hex_encode(h.finalize().as_bytes())
        };

        // Atomic copy.
        let dst_wasm = wasm_out.join(format!("{}.wasm", bin_name));
        let tmp = dst_wasm.with_extension("tmp");
        std::fs::write(&tmp, &bytes).map_err(|e| format!("write {}: {}", tmp.display(), e))?;
        std::fs::rename(&tmp, &dst_wasm).map_err(|e| format!("rename: {}", e))?;

        // Write stub proof JSON.
        let proof_path = proof_out.join(format!("{}.wasm.proof", bin_name));
        let proof = serde_json::json!({
            "version": 1,
            "module": bin_name,
            "artifact_blake3_hex": blake3_hex,
            "size_bytes": size_bytes,
            "agent_wallet": agent_wallet.clone().unwrap_or_default(),
            "fluxc_version": fluxc_version,
            "compiled_at_us": now_us,
            "sqisign_pubkey_hex": "(slice-β-pending)",
            "sqisign_sig_hex": "(slice-β-pending)",
            "synthetic": true,
            "note": "BLAKE3 is real; SQIsign signing pends flux-sqisign wasm32-wasi cross-compile."
        });
        std::fs::write(&proof_path, serde_json::to_vec_pretty(&proof).unwrap_or_default())
            .map_err(|e| format!("write proof: {}", e))?;

        println!("  ✓ {} → {} ({} B, blake3 {}…)",
            pkg, dst_wasm.display(), size_bytes, &blake3_hex[..16]);
        staged.push(StagedModule {
            package: pkg.to_string(),
            bin_name,
            wasm_path: dst_wasm,
            proof_path,
            size_bytes,
            blake3_hex,
            compiled_at_us: now_us,
        });
    }

    let manifest_path = output_dir.join("manifest.json");
    let preserved = merge_manifest(&manifest_path, &staged, &agent_wallet, &fluxc_version)?;

    println!("📜 manifest: {} ({} staged, {} preserved)",
        manifest_path.display(), staged.len(), preserved);

    Ok(StageReport {
        output_dir: output_dir.to_path_buf(),
        manifest_path,
        fluxc_version,
        agent_wallet,
        modules: staged,
        preserved_entries: preserved,
    })
}

fn ensure_wasi_target() -> Result<(), String> {
    let out = Command::new("rustup").args(["target", "list", "--installed"]).output()
        .map_err(|e| format!("rustup: {}", e))?;
    let installed = String::from_utf8_lossy(&out.stdout);
    if installed.lines().any(|l| l.trim() == WASI_TARGET) {
        return Ok(());
    }
    Err(format!(
        "{} target not installed — run: rustup target add {}",
        WASI_TARGET, WASI_TARGET
    ))
}

fn find_workspace_root() -> Result<PathBuf, String> {
    if let Ok(p) = std::env::var("FLUX_WORKSPACE_ROOT") {
        let p = PathBuf::from(p);
        if p.join("Cargo.toml").exists() { return Ok(p); }
    }
    let cwd = std::env::current_dir().map_err(|e| format!("cwd: {}", e))?;
    let mut cur = Some(cwd.as_path());
    while let Some(d) = cur {
        let cargo = d.join("Cargo.toml");
        if cargo.exists() {
            if let Ok(txt) = std::fs::read_to_string(&cargo) {
                if txt.contains("[workspace]") {
                    return Ok(d.to_path_buf());
                }
            }
        }
        cur = d.parent();
    }
    Err("workspace root not found — set FLUX_WORKSPACE_ROOT".into())
}

/// Read existing manifest.json (if any), drop entries this run rewrote, append
/// the new ones, write atomically. Returns the count of preserved entries.
fn merge_manifest(
    manifest_path: &Path,
    staged: &[StagedModule],
    agent_wallet: &Option<String>,
    fluxc_version: &str,
) -> Result<usize, String> {
    let mut current: serde_json::Value = if manifest_path.exists() {
        let s = std::fs::read_to_string(manifest_path).map_err(|e| format!("read manifest: {}", e))?;
        serde_json::from_str(&s).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };
    let obj = current.as_object_mut().ok_or("manifest root is not an object")?;
    obj.insert("name".into(), serde_json::json!("QuillonOS"));
    obj.insert("version".into(), serde_json::json!("0.1.1-alpha"));
    obj.insert("kernel".into(), serde_json::json!("wasi-preview1"));
    obj.insert("compiler".into(), serde_json::json!(format!("fluxc {}", fluxc_version)));
    obj.insert("signing".into(), serde_json::json!("SQIsign Level 5 (slice-β: stub sigs, real BLAKE3)"));
    obj.insert("hash_algo".into(), serde_json::json!("blake3"));
    obj.insert("release_t_unix".into(), serde_json::json!(
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
    ));

    let mut existing: Vec<serde_json::Value> = obj.get("modules").and_then(|v| v.as_array())
        .cloned().unwrap_or_default();
    let restaged_names: std::collections::HashSet<String> = staged.iter()
        .map(|m| m.bin_name.clone()).collect();
    existing.retain(|e| !restaged_names.contains(
        e.get("name").and_then(|v| v.as_str()).unwrap_or("")
    ));
    let preserved = existing.len();
    for m in staged {
        existing.push(serde_json::json!({
            "name": m.bin_name,
            "wasm": format!("wasm/{}.wasm", m.bin_name),
            "size_bytes": m.size_bytes,
            "blake3": m.blake3_hex,
            "sigil_proof": format!("proofs/{}.wasm.proof", m.bin_name),
            "agent_wallet": agent_wallet.clone().unwrap_or_default(),
        }));
    }
    obj.insert("modules".into(), serde_json::json!(existing));

    let body = serde_json::to_string_pretty(&current)
        .map_err(|e| format!("serialize manifest: {}", e))?;
    let tmp = manifest_path.with_extension("json.tmp");
    std::fs::write(&tmp, &body).map_err(|e| format!("write manifest tmp: {}", e))?;
    std::fs::rename(&tmp, manifest_path).map_err(|e| format!("rename manifest: {}", e))?;
    Ok(preserved)
}

// ─────────────────────────────────────────────────────────────────────────────
// P2P worker — v6.0: real libp2p runtime (drives flux-p2p NetworkManager).
// ─────────────────────────────────────────────────────────────────────────────

pub fn run_p2p_worker() {
    // v6.0: Activate runtime — drive the real libp2p NetworkManager from flux-p2p.
    // run_p2p_worker is sync (called straight from main.rs), so we own a tokio
    // runtime here; the swarm event loop is spawned inside NetworkManager::start.
    let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("❌ fluxc p2p-worker: failed to build tokio runtime: {}", e);
            return;
        }
    };
    rt.block_on(run_p2p_worker_async());
}

/// The actual worker: build a NetworkManager (env-configurable), start the real
/// libp2p swarm, then loop draining app-events + reporting peer count until Ctrl-C.
async fn run_p2p_worker_async() {
    use flux_p2p::{NetworkConfig, NetworkManager, SwarmAppEvent};

    // Base config + env overrides:
    //   FLUX_NODE_ID, FLUX_LISTEN_ADDR, FLUX_BOOTSTRAP_PEERS (comma-separated multiaddrs)
    let mut config = NetworkConfig::default();
    if let Ok(id) = std::env::var("FLUX_NODE_ID") {
        if !id.trim().is_empty() {
            config.node_id = id;
        }
    }
    if let Ok(addr) = std::env::var("FLUX_LISTEN_ADDR") {
        if !addr.trim().is_empty() {
            config.listen_addr = addr;
        }
    }
    if let Ok(peers) = std::env::var("FLUX_BOOTSTRAP_PEERS") {
        let list: Vec<String> = peers
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if !list.is_empty() {
            config.bootstrap_peers = list;
        }
    }

    println!("⚡ fluxc p2p-worker — activating real libp2p runtime");
    println!("   node_id   = {}", config.node_id);
    println!("   listen    = {}", config.listen_addr);
    println!("   bootstrap = {} peer(s)", config.bootstrap_peers.len());
    println!("   topics    = {}", config.gossipsub_topics.len());

    let mut net = NetworkManager::new(config);
    if let Err(e) = net.start().await {
        eprintln!("❌ fluxc p2p-worker: NetworkManager::start failed: {}", e);
        return;
    }
    println!("✅ P2P swarm started — entering worker loop (Ctrl-C to stop)");

    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(5));
    let mut last_peer_count = u32::MAX;
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                println!("\n⏹  Ctrl-C received — shutting down P2P worker");
                let _ = net.stop().await;
                break;
            }
            _ = ticker.tick() => {
                for ev in net.drain_events() {
                    match ev {
                        SwarmAppEvent::GossipsubMessage { topic, from, data, .. } =>
                            println!("📨 gossip [{}] {} bytes from {}", topic, data.len(), from),
                        SwarmAppEvent::PeerConnected { peer_id, addr } =>
                            println!("🔗 peer connected: {} ({})", peer_id, addr),
                        SwarmAppEvent::PeerDisconnected { peer_id } =>
                            println!("✂️  peer disconnected: {}", peer_id),
                        SwarmAppEvent::NewListenAddr(addr) =>
                            println!("📍 listening on {}", addr),
                        SwarmAppEvent::PeerIdentified { peer_id, agent_version, .. } =>
                            println!("🪪 peer identified: {} ({})", peer_id, agent_version),
                        _ => {}
                    }
                }
                let pc = net.peer_count();
                if pc != last_peer_count {
                    println!("👥 peers: {}", pc);
                    last_peer_count = pc;
                }
            }
        }
    }
    println!("👋 fluxc p2p-worker stopped");
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn http_get_string(url: &str, timeout_secs: u64) -> Result<String, String> {
    let out = Command::new("curl")
        .args(["-fsSL", "--max-time"])
        .arg(timeout_secs.to_string())
        .arg("--connect-timeout")
        .arg("5")
        .arg(url)
        .output()
        .map_err(|e| format!("curl: {}", e))?;
    if !out.status.success() {
        return Err(format!(
            "curl exit {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    String::from_utf8(out.stdout).map_err(|e| format!("utf8: {}", e))
}

fn http_get_to_file(url: &str, dst: &Path, timeout_secs: u64) -> Result<(), String> {
    let status = Command::new("curl")
        .args(["-fsSL", "--max-time"])
        .arg(timeout_secs.to_string())
        .arg("--connect-timeout")
        .arg("5")
        .arg("-o")
        .arg(dst)
        .arg(url)
        .status()
        .map_err(|e| format!("curl: {}", e))?;
    if !status.success() {
        return Err(format!("curl exit {} for {}", status, url));
    }
    Ok(())
}

fn semver_gt(a: &str, b: &str) -> bool {
    parse_semver(a) > parse_semver(b)
}

fn parse_semver(s: &str) -> (u32, u32, u32) {
    let s = s.trim_start_matches('v');
    let mut it = s.splitn(3, '.').map(|p| {
        p.chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse::<u32>()
            .unwrap_or(0)
    });
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

fn hostname_short() -> String {
    Command::new("hostname")
        .arg("-s")
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok().map(|s| s.trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "host".into())
}

fn now_short() -> String {
    let s = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mins = (s % 86400) / 60;
    let hr = mins / 60;
    let min = mins % 60;
    let sec = s % 60;
    format!("{:02}:{:02}:{:02}", hr, min, sec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_semver_basics() {
        assert_eq!(parse_semver("0.13.0"), (0, 13, 0));
        assert_eq!(parse_semver("v1.2.3"), (1, 2, 3));
        assert_eq!(parse_semver("0.13.0-beta1"), (0, 13, 0));
    }

    #[test]
    fn semver_gt_works() {
        assert!(semver_gt("0.13.1", "0.13.0"));
        assert!(semver_gt("0.14.0", "0.13.99"));
        assert!(!semver_gt("0.13.0", "0.13.0"));
        assert!(!semver_gt("0.12.99", "0.13.0"));
    }

    #[test]
    fn hex_roundtrip() {
        assert_eq!(hex_encode(&[0xab, 0xcd, 0x01]), "abcd01");
    }

    #[test]
    fn sigil_products_resolve_to_sigilgraph_org() {
        assert_eq!(manifest_url_for("sigil-top"), "https://sigilgraph.org/downloads/sigil-top-latest.json");
        assert_eq!(manifest_url_for("sigil-ai"), "https://sigilgraph.org/downloads/sigil-ai-latest.json");
        assert_eq!(manifest_url_for("fluxc"), "https://quillon.xyz/downloads/fluxc-latest.json");
        assert_eq!(manifest_url_for("flux-arena"), "https://quillon.xyz/downloads/flux-arena-latest.json");
    }

    #[test]
    fn sigil_top_manifest_parses_without_sha256() {
        // The live sigil-top-latest.json shape (2026-09-13): blake3 only, extra fields,
        // no sha256_hex / released_at_us / publisher. Used to fail: `missing field sha256_hex`.
        let body = r#"{
          "product": "sigil-top", "version": "10.0.2", "channel": "stable",
          "url": "https://sigilgraph.org/downloads/sigil-top-v10.0.2-linux-x64",
          "blake3_hex": "5451748e695a599df405a1b6c9871d0aa2109f0a599a0b8647e1db1a1c23b1c6", "size_bytes": 34348896,
          "flux_rev": "27c5949b", "source_tag": "v10.0.2",
          "verify": "fluxc verify-proof <artifact> <artifact>.proof",
          "notes": "100 blk/s network",
          "targets": { "linux-x64": { "url": "x" }, "windows-x64": { "url": "y" }, "linux-arm64": { "url": "z" } }
        }"#;
        let m: ReleaseManifest = serde_json::from_str(body).expect("sigil manifest parses");
        assert_eq!(m.version, "10.0.2");
        assert!(m.sha256_hex.is_empty(), "absent means absent, never fabricated");
        assert_eq!(m.blake3_hex.len(), 64);
        assert_eq!(m.channel, "stable");
        assert_eq!(m.flux_rev, "27c5949b");
        assert_eq!(m.targets.len(), 3);
        assert_eq!(m.released_at_us, 0);
    }

    #[test]
    fn fluxc_manifest_still_parses() {
        let body = r#"{"version":"0.41.0","url":"u","sha256_hex":"aa","blake3_hex":"bb","size_bytes":1,"released_at_us":2,"publisher":"epsilon"}"#;
        let m: ReleaseManifest = serde_json::from_str(body).unwrap();
        assert_eq!(m.product, "fluxc");
        assert_eq!(m.sha256_hex, "aa");
        assert!(m.targets.is_empty());
    }
}

#[cfg(test)]
mod release_sig_tests {
    use super::*;

    /// Signed on Epsilon 2026-10-23 with the real Flux release seed (Ed25519 is
    /// deterministic). Plain text, not a manifest: it can never pass as a release.
    const BODY: &[u8] = b"flux release key test vector 2026-10-23 (not a manifest)";
    const SIG: &str = "3f6df48d57f9c6e62b8d244e0989d3b91615e420e7c3c997f0d9315c9f8083bf0ff3d0c23118b600cfbf96d6d5881173f5c3911f626f980c6558b419776bf90d";

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("flux-relsig-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_pinned_key_accepts_its_own_signature() {
        assert!(verify_release_sig(BODY, SIG).is_ok());
        assert!(verify_release_sig(BODY, &format!("{SIG}\n")).is_ok(), "a trailing newline is not another signature");
    }

    #[test]
    fn other_bytes_another_key_or_junk_are_refused() {
        use ed25519_dalek::{Signer, SigningKey};
        assert!(verify_release_sig(b"flux release key test vector 2026-10-24 (not a manifest)", SIG).is_err());
        let thief = SigningKey::from_bytes(&[7u8; 32]);
        let forged = hex_encode(&thief.sign(BODY).to_bytes());
        assert!(verify_release_sig(BODY, &forged).unwrap_err().contains("RELEASE SIGNATURE INVALID"));
        // what a missing .sig looks like on our sites: the HTML fallback page, HTTP 200
        let e = verify_release_sig(BODY, "<!doctype html><html>…</html>").unwrap_err();
        assert!(e.contains("HTML page"), "{e}");
        assert!(verify_release_sig(BODY, "").is_err());
    }

    #[test]
    fn a_seed_that_is_not_the_pinned_key_never_signs() {
        let d = scratch("seed");
        let wrong = d.join("wrong.seed");
        std::fs::write(&wrong, [9u8; 32]).unwrap();
        let e = sign_release_with(wrong.to_str().unwrap(), BODY).unwrap_err();
        assert!(e.contains("not the pinned"), "{e}");
        std::fs::write(&wrong, [9u8; 31]).unwrap();
        assert!(sign_release_with(wrong.to_str().unwrap(), BODY).unwrap_err().contains("not 32 bytes"));
    }

    #[test]
    fn sigil_products_are_never_published_by_fluxc() {
        let e = publish_release_product("sigil-top", "99.0.0", Some(PathBuf::from("/nonexistent"))).unwrap_err();
        assert!(e.contains("SIGIL product"), "{e}");
    }

    #[test]
    fn install_refuses_symlinks_and_build_outputs_and_installs_a_plain_file() {
        let d = scratch("install");
        let real = d.join("real-fluxc");
        std::fs::write(&real, b"dev build").unwrap();
        let link = d.join("fluxc-link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let dl = d.join("downloaded");
        std::fs::write(&dl, b"release").unwrap();
        assert!(install_verified(&dl, &link).unwrap_err().contains("symlink"));
        assert_eq!(std::fs::read(&real).unwrap(), b"dev build", "the link target must be untouched");
        let build = d.join("target/debug/fluxc");
        std::fs::create_dir_all(build.parent().unwrap()).unwrap();
        std::fs::write(&build, b"dev build").unwrap();
        assert!(install_verified(&dl, &build).unwrap_err().contains("build output"));
        // a cross build: no /target/<profile>/ in the path, only cargo's CACHEDIR.TAG says what it is
        let tgt = d.join("tgt");
        let cross = tgt.join("x86_64-unknown-linux-musl/release/fluxc");
        std::fs::create_dir_all(cross.parent().unwrap()).unwrap();
        std::fs::write(tgt.join("CACHEDIR.TAG"), "Signature: 8a477f597d28d172789f06886806bc55\n# cargo\n").unwrap();
        std::fs::write(&cross, b"musl build").unwrap();
        assert!(install_verified(&dl, &cross).unwrap_err().contains("build output"));
        assert_eq!(std::fs::read(&cross).unwrap(), b"musl build");
        let plain = d.join("bin-fluxc");
        std::fs::write(&plain, b"old").unwrap();
        install_verified(&dl, &plain).unwrap();
        assert_eq!(std::fs::read(&plain).unwrap(), b"release");
        assert!(!dl.exists(), "the staged download is consumed");
    }
}
