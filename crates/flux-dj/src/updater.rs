//! Auto-updater: version check + confirmed, atomic self-replace.
//!
//! Modeled directly on the proven precedent already in this workspace's
//! backup mirror at `gui/slint-wallet/src/updater.rs` (Q-NarwhalKnight's
//! Slint wallet auto-updater) — same shape, reused where it fits:
//!   - `semver::Version` comparison against `env!("CARGO_PKG_VERSION")`
//!   - a version-manifest endpoint (`/api/v1/version` there, `/api/v1/dj-version`
//!     here) returning the latest published version + a download target
//!   - a MANDATORY SHA-256 checksum on the downloaded bytes — the Slint
//!     updater explicitly REFUSES to apply an update with no checksum
//!     ("Server did not provide SHA-256 checksum — refusing update for
//!     safety"); this module keeps that rule
//!   - write the download to a `.update-tmp` file next to the binary, then
//!     atomically swap it into place — the Slint updater uses the
//!     `self-replace` crate for this because a plain rename can't overwrite
//!     a *running* executable on Windows; we use the same crate for the
//!     same reason (see `download_and_apply_to_running_binary`)
//!
//! What's different from the Slint precedent, deliberately: the Slint
//! updater is a GUI where the "confirm" step is a user clicking a button in
//! `Updater::state()`. flux-dj has two different "confirmers" — a human's
//! AI relaying a yes/no through MCP tool calls, and (for the standalone
//! server with no AI in the loop) an explicit CLI flag or HTTP endpoint —
//! so this module deliberately separates "check" (pure network read, zero
//! filesystem/binary writes) from "apply" (the only function that touches
//! the binary), so the confirmation gate is enforced by which function got
//! called, not by a flag inside one function. See `mcp.rs`'s
//! `dj_check_for_update` / `dj_apply_update` and `flux-dj-server.rs`'s
//! `GET /api/v1/dj-version` / `POST /update/apply`.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// This binary's own version, embedded at compile time — same
/// `version.workspace = true` → `CARGO_PKG_VERSION` pattern every other
/// crate in this workspace uses (see `flux_version_status`/`flux_version_sync`).
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// What a version-check endpoint (`GET /api/v1/dj-version`) returns.
/// `download_url` may be absolute (`https://...`) or relative
/// (`/downloads/...`, resolved against the server that served the
/// manifest — see [`resolve_url`]). `sha256` is optional on the wire but
/// mandatory in practice: [`download_verified_blocking`] refuses to apply
/// an update that doesn't carry one.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VersionManifest {
    pub version: String,
    pub download_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// Result of a (side-effect-free) update check.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UpdateCheckResult {
    pub update_available: bool,
    pub current_version: String,
    pub latest_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_url: Option<String>,
}

/// Result of a confirmed apply.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApplyResult {
    pub applied: bool,
    pub previous_version: String,
    pub new_version: String,
    /// Always true when `applied`. Neither `dj_apply_update` (MCP) nor
    /// `download_and_apply_to_running_binary` restarts the process itself —
    /// see the module doc for why the MCP path deliberately never
    /// self-exits. `flux-dj-server`'s `POST /update/apply?restart=true` is
    /// the one call site that actually restarts, on its own schedule.
    pub restart_required: bool,
}

const ENV_UPDATE_SERVER_URL: &str = "FLUX_DJ_UPDATE_SERVER_URL";
const ENV_DOWNLOADS_DIR: &str = "FLUX_DJ_DOWNLOADS_DIR";
const ENV_DOWNLOAD_BASE_URL: &str = "FLUX_DJ_DOWNLOAD_BASE_URL";
const DEFAULT_PORT: u16 = 4210;

/// Where a remote/co-located instance should ask "what's the latest
/// version". Defaults to the local server's own default port — the common
/// deployment is `flux-dj-server` + `flux-dj-mcp` on the same host; a
/// remote user's copy overrides this to point at the operator's canonical
/// instance (e.g. Viktor's), same idea as the Slint wallet pointing at
/// `q-api-server`'s `/api/v1/version`.
pub fn default_update_server_url() -> String {
    std::env::var(ENV_UPDATE_SERVER_URL).unwrap_or_else(|_| format!("http://127.0.0.1:{DEFAULT_PORT}"))
}

/// Directory `flux-dj-server` scans for published releases
/// (`flux-dj-server-vX.Y.Z`), analogous to q-api-server's
/// `dist-final/downloads/` scan for `slint-wallet-vX.Y.Z`.
pub fn downloads_dir() -> PathBuf {
    let dir = std::env::var(ENV_DOWNLOADS_DIR).unwrap_or_else(|_| "/home/storage/deepseek-codewhale/flux/data/flux-dj/downloads".to_string());
    PathBuf::from(dir)
}

/// Base URL to prefix onto a relative `download_url` the server reports.
/// Empty when unset — an empty base plus [`resolve_url`] yields a
/// same-origin relative path, which is correct as long as the caller
/// resolves it against the host it queried (which every call site here does).
pub fn download_base_url() -> String {
    std::env::var(ENV_DOWNLOAD_BASE_URL).unwrap_or_default()
}

/// If `maybe_relative` is already absolute (`http://`/`https://`), return it
/// unchanged; otherwise join it onto `base`. Pure string logic, no I/O —
/// used both server-side (turning a relative `/downloads/...` path into
/// what the manifest reports) and client-side (turning a possibly-relative
/// `download_url` from a fetched manifest into something `reqwest` can GET).
pub fn resolve_url(base: &str, maybe_relative: &str) -> String {
    if maybe_relative.starts_with("http://") || maybe_relative.starts_with("https://") {
        return maybe_relative.to_string();
    }
    format!("{}/{}", base.trim_end_matches('/'), maybe_relative.trim_start_matches('/'))
}

/// Pure comparison: is `remote` newer than `current`? No I/O.
pub fn compare_versions(current: &str, remote: &str) -> Result<bool> {
    let c = Version::parse(current).map_err(|e| anyhow!("bad current version '{current}': {e}"))?;
    let r = Version::parse(remote).map_err(|e| anyhow!("bad remote version '{remote}': {e}"))?;
    Ok(r > c)
}

/// Pure orchestration: given an already-fetched manifest, decide whether an
/// update is available. No I/O — the network fetch is a separate step
/// ([`fetch_manifest_blocking`]) so this logic is directly unit-testable.
pub fn evaluate_update(current_version: &str, manifest: &VersionManifest) -> Result<UpdateCheckResult> {
    let available = compare_versions(current_version, &manifest.version)?;
    Ok(UpdateCheckResult {
        update_available: available,
        current_version: current_version.to_string(),
        latest_version: manifest.version.clone(),
        download_url: if available { Some(manifest.download_url.clone()) } else { None },
    })
}

/// GET `{update_server_url}/api/v1/dj-version` and parse the manifest.
/// Blocking (works from the synchronous MCP stdio loop with no ambient
/// tokio runtime); the HTTP server wraps this in `spawn_blocking` for its
/// periodic background check.
pub fn fetch_manifest_blocking(update_server_url: &str) -> Result<VersionManifest> {
    let url = format!("{}/api/v1/dj-version", update_server_url.trim_end_matches('/'));
    let client = reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(10)).build()?;
    let resp = client.get(&url).send().map_err(|e| anyhow!("version check request to {url} failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(anyhow!("version check failed: HTTP {} from {url}", resp.status()));
    }
    let manifest: VersionManifest = resp.json().map_err(|e| anyhow!("could not parse version manifest from {url}: {e}"))?;
    Ok(manifest)
}

/// The CHECK step: fetch the manifest + evaluate it. This function performs
/// exactly one network GET and does not write anything to disk — it is
/// safe to call as often as wanted (`dj_check_for_update`, the periodic
/// background check) and is what `tests::check_never_touches_filesystem`
/// pins.
pub fn check_for_update(update_server_url: &str) -> Result<UpdateCheckResult> {
    let manifest = fetch_manifest_blocking(update_server_url)?;
    evaluate_update(CURRENT_VERSION, &manifest)
}

/// Download `download_url`, verify it against `expected_sha256` (MANDATORY —
/// `None` is refused immediately, before any network call, mirroring the
/// Slint updater's "refuse blind updates" rule), and write the verified
/// bytes to `dest`. Takes an explicit destination path (rather than always
/// deriving one from `current_exe()`) specifically so this — the part that
/// actually moves bytes onto disk — is unit-testable against a harmless
/// scratch file instead of the running binary.
pub fn download_verified_blocking(download_url: &str, expected_sha256: Option<&str>, dest: &Path) -> Result<()> {
    let expected = expected_sha256.ok_or_else(|| anyhow!("server did not provide a SHA-256 checksum — refusing update for safety"))?;

    let client = reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(600)).build()?;
    let resp = client.get(download_url).send().map_err(|e| anyhow!("download from {download_url} failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(anyhow!("download failed: HTTP {} from {download_url}", resp.status()));
    }
    let bytes = resp.bytes().map_err(|e| anyhow!("reading download body failed: {e}"))?;

    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected {
        return Err(anyhow!("SHA-256 mismatch: expected {expected}, got {actual} — possible corruption or tampering, refusing to write"));
    }

    if dest.exists() {
        let _ = std::fs::remove_file(dest);
    }
    std::fs::write(dest, &bytes).map_err(|e| anyhow!("writing verified download to {}: {e}", dest.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o755));
    }
    Ok(())
}

/// Atomically move `temp_path` into `target_path` (same-filesystem rename —
/// the core primitive `self-replace` also relies on). Exposed as its own
/// function so it's unit-testable against harmless scratch files without
/// invoking `self_replace::self_replace` against the live test binary.
pub fn atomic_swap(temp_path: &Path, target_path: &Path) -> Result<()> {
    std::fs::rename(temp_path, target_path).map_err(|e| anyhow!("atomic rename {} -> {} failed: {e}", temp_path.display(), target_path.display()))
}

/// Compute SHA-256 of a file on disk.
pub fn sha256_of_file(path: &Path) -> Result<String> {
    let data = std::fs::read(path).map_err(|e| anyhow!("reading {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    Ok(format!("{:x}", hasher.finalize()))
}

/// Scan `dir` for files named `{prefix}X.Y.Z` and return the
/// highest-semver match + its path. Numeric (major, minor, patch) tuple
/// comparison, not string comparison, so `1.10.0` correctly beats `1.9.5` —
/// same logic q-api-server's `detect_latest_wallet_version` /
/// `detect_latest_node_version` use for the equivalent scan. Pure
/// filesystem read, no network.
pub fn detect_latest_release(dir: &Path, prefix: &str) -> Option<(String, PathBuf)> {
    let mut best: Option<((u64, u64, u64), String, PathBuf)> = None;
    let read_dir = std::fs::read_dir(dir).ok()?;
    for entry in read_dir.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let Some(version_part) = name_str.strip_prefix(prefix) else { continue };
        let parts: Vec<&str> = version_part.split('.').collect();
        if parts.len() != 3 {
            continue;
        }
        let (Ok(major), Ok(minor), Ok(patch)) = (parts[0].parse::<u64>(), parts[1].parse::<u64>(), parts[2].parse::<u64>()) else { continue };
        let key = (major, minor, patch);
        if best.as_ref().map(|(bk, _, _)| key > *bk).unwrap_or(true) {
            best = Some((key, version_part.to_string(), entry.path()));
        }
    }
    best.map(|(_, version, path)| (version, path))
}

/// Production APPLY entry point: download + verify + atomically replace the
/// CURRENTLY RUNNING executable via the `self-replace` crate — the same
/// crate and the same call shape the Slint wallet updater uses
/// (`self_replace::self_replace(&temp_path)`), specifically because a plain
/// rename cannot overwrite a running executable on Windows; `self-replace`
/// handles that platform difference (rename-in-place on Linux, rename-aside
/// + rename-into-place on Windows) so this module doesn't have to.
///
/// NOT exercised by an automated test in this session — see the crate
/// report / `tests` module doc for why (it would replace the live test
/// binary mid-run). [`download_verified_blocking`] and [`atomic_swap`],
/// which are the two operations this function composes, ARE covered by
/// tests against scratch files.
pub fn download_and_apply_to_running_binary(manifest: &VersionManifest, update_server_url: &str) -> Result<()> {
    let current_exe = std::env::current_exe()?;
    let temp_path = current_exe.with_extension("update-tmp");
    if temp_path.exists() {
        let _ = std::fs::remove_file(&temp_path);
    }
    let download_url = resolve_url(update_server_url, &manifest.download_url);
    download_verified_blocking(&download_url, manifest.sha256.as_deref(), &temp_path)?;
    self_replace::self_replace(&temp_path)?;
    let _ = std::fs::remove_file(&temp_path);
    Ok(())
}

/// The APPLY step: re-fetch the manifest fresh (never trust a stale check
/// from earlier — the operator may have published again, or the version
/// available may have changed), confirm it's still newer, then download +
/// verify + replace. This is the ONLY function in this module that writes
/// to the binary/filesystem — the confirmation gate lives in "did the
/// caller invoke `apply_update`/`download_and_apply_to_running_binary` at
/// all", not in a flag.
pub fn apply_update(update_server_url: &str) -> Result<ApplyResult> {
    let manifest = fetch_manifest_blocking(update_server_url)?;
    let check = evaluate_update(CURRENT_VERSION, &manifest)?;
    if !check.update_available {
        return Ok(ApplyResult {
            applied: false,
            previous_version: CURRENT_VERSION.to_string(),
            new_version: CURRENT_VERSION.to_string(),
            restart_required: false,
        });
    }
    download_and_apply_to_running_binary(&manifest, update_server_url)?;
    Ok(ApplyResult {
        applied: true,
        previous_version: CURRENT_VERSION.to_string(),
        new_version: manifest.version.clone(),
        restart_required: true,
    })
}

/// Spawn a fresh copy of the (now-replaced) executable with the same args
/// and exit this process. Same pattern as the Slint updater's
/// `Updater::restart()`. Never called from the MCP path (see module doc);
/// only `flux-dj-server`'s `POST /update/apply?restart=true` calls this,
/// and only after giving the HTTP response time to flush.
pub fn restart_process() -> ! {
    let exe = std::env::current_exe().expect("current_exe for restart");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let _ = std::process::Command::new(&exe).args(&args).spawn();
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Spin up a tiny, real (not mocked-in-process) HTTP/1.1 responder on a
    /// loopback ephemeral port serving one fixed JSON body for the first
    /// request it receives, then stop. Deliberately hand-rolled with
    /// `std::net` (no axum/tokio) so tests calling the *blocking* reqwest
    /// path never nest inside an existing tokio runtime.
    fn spawn_one_shot_json_server(body: String) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local_addr");
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        format!("http://{addr}")
    }

    #[test]
    fn compare_versions_newer_equal_older() {
        assert!(compare_versions("1.0.0", "1.0.1").unwrap());
        assert!(compare_versions("1.0.0", "2.0.0").unwrap());
        assert!(!compare_versions("1.0.0", "1.0.0").unwrap());
        assert!(!compare_versions("2.0.0", "1.9.9").unwrap());
    }

    #[test]
    fn compare_versions_rejects_malformed_semver() {
        assert!(compare_versions("not-a-version", "1.0.0").is_err());
        assert!(compare_versions("1.0.0", "not-a-version").is_err());
    }

    #[test]
    fn evaluate_update_reports_download_url_only_when_available() {
        let manifest = VersionManifest { version: "0.0.1".into(), download_url: "http://example/old".into(), sha256: None };
        let result = evaluate_update("999.0.0", &manifest).unwrap();
        assert!(!result.update_available);
        assert!(result.download_url.is_none());

        let manifest = VersionManifest { version: "999.0.0".into(), download_url: "http://example/new".into(), sha256: None };
        let result = evaluate_update("0.0.1", &manifest).unwrap();
        assert!(result.update_available);
        assert_eq!(result.download_url.as_deref(), Some("http://example/new"));
    }

    #[test]
    fn resolve_url_leaves_absolute_untouched_and_joins_relative() {
        assert_eq!(resolve_url("http://host:1234", "https://elsewhere/x"), "https://elsewhere/x");
        assert_eq!(resolve_url("http://host:1234", "/downloads/foo"), "http://host:1234/downloads/foo");
        assert_eq!(resolve_url("http://host:1234/", "downloads/foo"), "http://host:1234/downloads/foo");
        assert_eq!(resolve_url("", "/downloads/foo"), "/downloads/foo");
    }

    /// The hard requirement from the spec: calling the CHECK path must
    /// never write anything to disk / touch the running binary — only
    /// `apply_update` (never exercised here, see its own doc) does that.
    #[test]
    fn check_for_update_never_touches_filesystem_or_binary() {
        let manifest = VersionManifest { version: "999.0.0".into(), download_url: "/downloads/flux-dj-server-v999.0.0".into(), sha256: Some("deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef".into()) };
        let body = serde_json::to_string(&manifest).unwrap();
        let server_url = spawn_one_shot_json_server(body);

        let current_exe = std::env::current_exe().expect("current_exe");
        let before = std::fs::metadata(&current_exe).expect("metadata before");
        let temp_marker = current_exe.with_extension("update-tmp");
        assert!(!temp_marker.exists(), "precondition: no stray update-tmp file");

        let result = check_for_update(&server_url).expect("check_for_update should succeed against the mock server");
        assert!(result.update_available);
        assert_eq!(result.latest_version, "999.0.0");
        assert_eq!(result.current_version, CURRENT_VERSION);

        // The actual assertion: the check must be side-effect free.
        assert!(!temp_marker.exists(), "check_for_update must never create an update-tmp file");
        let after = std::fs::metadata(&current_exe).expect("metadata after");
        assert_eq!(before.len(), after.len(), "check_for_update must never modify the running binary's size");
        assert_eq!(before.modified().unwrap(), after.modified().unwrap(), "check_for_update must never modify the running binary's mtime");
    }

    #[test]
    fn check_for_update_reports_no_update_when_remote_is_older() {
        let manifest = VersionManifest { version: "0.0.1".into(), download_url: "/downloads/old".into(), sha256: None };
        let body = serde_json::to_string(&manifest).unwrap();
        let server_url = spawn_one_shot_json_server(body);

        let result = check_for_update(&server_url).unwrap();
        assert!(!result.update_available);
    }

    #[test]
    fn download_verified_blocking_refuses_without_checksum() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");
        // No network call should even happen — verified by using a
        // non-existent host that would error if dialed, and asserting the
        // specific "no checksum" message rather than a network error.
        let err = download_verified_blocking("http://127.0.0.1:1/unreachable", None, &dest).unwrap_err();
        assert!(err.to_string().contains("SHA-256"), "expected a checksum-refusal error, got: {err}");
        assert!(!dest.exists());
    }

    #[test]
    fn download_verified_blocking_accepts_matching_checksum_and_rejects_mismatch() {
        let payload = b"pretend-flux-dj-binary-bytes";
        let mut hasher = Sha256::new();
        hasher.update(payload);
        let correct_hash = format!("{:x}", hasher.finalize());

        // one-shot raw-bytes HTTP server
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for _ in 0..2 {
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut buf = [0u8; 2048];
                    let _ = stream.read(&mut buf);
                    let mut resp = format!("HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", payload.len()).into_bytes();
                    resp.extend_from_slice(payload);
                    let _ = stream.write_all(&resp);
                    let _ = stream.flush();
                } else {
                    break;
                }
            }
        });
        let url = format!("http://{addr}");

        let dir = tempfile::tempdir().unwrap();

        // correct checksum -> file written with exact bytes
        let dest_ok = dir.path().join("ok.bin");
        download_verified_blocking(&url, Some(&correct_hash), &dest_ok).expect("matching checksum should succeed");
        assert_eq!(std::fs::read(&dest_ok).unwrap(), payload);

        // wrong checksum -> rejected, nothing left on disk
        let dest_bad = dir.path().join("bad.bin");
        let err = download_verified_blocking(&url, Some("0000000000000000000000000000000000000000000000000000000000000000"), &dest_bad).unwrap_err();
        assert!(err.to_string().contains("mismatch"), "expected a mismatch error, got: {err}");
        assert!(!dest_bad.exists(), "a failed-checksum download must not leave a file behind");
    }

    #[test]
    fn atomic_swap_replaces_target_with_temp_contents_and_consumes_temp() {
        let dir = tempfile::tempdir().unwrap();
        let temp = dir.path().join("new.bin");
        let target = dir.path().join("current.bin");
        std::fs::write(&temp, b"NEW-CONTENT").unwrap();
        std::fs::write(&target, b"OLD-CONTENT").unwrap();

        atomic_swap(&temp, &target).unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"NEW-CONTENT");
        assert!(!temp.exists(), "rename must consume the source file");
    }

    #[test]
    fn detect_latest_release_uses_numeric_semver_not_string_order() {
        let dir = tempfile::tempdir().unwrap();
        for f in ["flux-dj-server-v1.2.0", "flux-dj-server-v1.10.0", "flux-dj-server-v1.9.5", "flux-dj-server-v0.9.0", "not-a-release-file"] {
            std::fs::write(dir.path().join(f), b"x").unwrap();
        }
        let (version, path) = detect_latest_release(dir.path(), "flux-dj-server-v").expect("should find a release");
        assert_eq!(version, "1.10.0", "1.10.0 must beat 1.9.5 numerically, not lexicographically");
        assert!(path.ends_with("flux-dj-server-v1.10.0"));
    }

    #[test]
    fn detect_latest_release_empty_dir_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(detect_latest_release(dir.path(), "flux-dj-server-v").is_none());
    }

    #[test]
    fn sha256_of_file_matches_known_hash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.bin");
        std::fs::write(&path, b"hello").unwrap();
        // sha256("hello")
        assert_eq!(sha256_of_file(&path).unwrap(), "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    }
}
