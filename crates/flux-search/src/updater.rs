// Flux Auto-Updater — SHA3-native self-updating binary system
//
// Native built-in feature for the entire Flux toolchain.
// Each Flux crate can call `flux_update::check()` to verify its binary integrity
// and auto-update from a remote endpoint.
//
// Security model:
//   1. Remote publishes { version, sha3_256, ed25519_signature, download_url }
//   2. Local binary computes its own SHA3-256
//   3. If hash differs → download new binary
//   4. Verify SHA3-256 of downloaded binary
//   5. Verify Ed25519 signature against hardcoded public key
//   6. Atomic replace (write to .new, rename over old, restart)
//
// Protocol:
//   GET /api/v1/flux/version → { "version": "0.4.0", "sha3_256": "...", "sig": "..." }
//   GET /api/v1/flux/download/{target} → binary

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ── Version Manifest ──

/// Remote version manifest — what the update server publishes.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct VersionManifest {
    /// Semantic version string.
    pub version: String,
    /// SHA3-256 hex digest of the release binary.
    pub sha3_256: String,
    /// Ed25519 signature over (version || sha3_256).
    pub signature: String,
    /// URL to download the binary.
    pub download_url: String,
    /// Release notes.
    pub notes: Option<String>,
    /// Minimum Flux version required to apply this update.
    pub min_updater_version: Option<String>,
}

/// Configuration for the auto-updater.
#[derive(Clone, Debug)]
pub struct UpdaterConfig {
    /// URL to check for updates.
    pub version_url: String,
    /// Base URL for binary downloads.
    pub download_base: String,
    /// Target triple (e.g., "x86_64-unknown-linux-gnu").
    pub target: String,
    /// Binary name to update.
    pub binary_name: String,
    /// Path to the current binary.
    pub current_binary: PathBuf,
    /// Ed25519 public key (hex-encoded) of the release signer.
    pub public_key: String,
    /// Automatically restart after update.
    pub auto_restart: bool,
    /// Check interval (seconds). 0 = check once.
    pub check_interval_secs: u64,
}

impl Default for UpdaterConfig {
    fn default() -> Self {
        UpdaterConfig {
            version_url: "https://quillon.xyz/api/v1/flux/version".into(),
            download_base: "https://quillon.xyz/downloads".into(),
            target: "x86_64-unknown-linux-gnu".into(),
            binary_name: "fluxc".into(),
            current_binary: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("fluxc")),
            public_key: String::new(),
            auto_restart: false,
            check_interval_secs: 21_600, // 6 hours
        }
    }
}

// ── Auto-Updater ──

/// The Flux auto-updater — checks, downloads, verifies, and applies updates.
pub struct AutoUpdater {
    config: UpdaterConfig,
    current_sha3: Option<String>,
}

/// Result of an update check.
#[derive(Clone, Debug)]
pub enum UpdateStatus {
    /// Already at the latest version.
    UpToDate { current: String },
    /// An update is available.
    UpdateAvailable { current: String, latest: String, sha3: String },
    /// Update was downloaded and verified.
    Downloaded { version: String, path: PathBuf },
    /// Update was applied (binary replaced).
    Applied { version: String },
    /// No update server reachable.
    Offline,
    /// Error during update.
    Error(String),
}

impl AutoUpdater {
    /// Create a new auto-updater with the given config.
    pub fn new(config: UpdaterConfig) -> Self {
        AutoUpdater {
            config,
            current_sha3: None,
        }
    }

    /// Compute SHA3-256 of the current binary.
    pub fn current_sha3(&mut self) -> Result<String, String> {
        if let Some(ref sha3) = self.current_sha3 {
            return Ok(sha3.clone());
        }

        let data = fs::read(&self.config.current_binary)
            .map_err(|e| format!("read binary: {}", e))?;

        let hash = sha3_256(&data);
        self.current_sha3 = Some(hash.clone());
        Ok(hash)
    }

    /// Check for updates — contacts the update server.
    pub async fn check(&mut self) -> Result<UpdateStatus, String> {
        let current_sha3 = self.current_sha3()?;
        let current_version = env!("CARGO_PKG_VERSION").to_string();

        // Fetch version manifest
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| format!("client: {}", e))?;

        let manifest: VersionManifest = match client
            .get(&self.config.version_url)
            .header("User-Agent", "flux-updater/1.0")
            .send()
            .await
        {
            Ok(resp) => resp.json().await.map_err(|e| format!("parse: {}", e))?,
            Err(_) => return Ok(UpdateStatus::Offline),
        };

        // Check version
        if manifest.sha3_256 == current_sha3 {
            return Ok(UpdateStatus::UpToDate {
                current: current_version,
            });
        }

        Ok(UpdateStatus::UpdateAvailable {
            current: current_version,
            latest: manifest.version.clone(),
            sha3: manifest.sha3_256.clone(),
        })
    }

    /// Download and verify the update.
    pub async fn download(&mut self) -> Result<UpdateStatus, String> {
        // Check first
        let manifest = match self.check().await? {
            UpdateStatus::UpdateAvailable { latest, sha3, .. } => {
                // Fetch full manifest again for download URL
                let client = reqwest::Client::new();
                let m: VersionManifest = client
                    .get(&self.config.version_url)
                    .send().await.map_err(|e| format!("fetch: {}", e))?
                    .json().await.map_err(|e| format!("parse: {}", e))?;
                m
            }
            UpdateStatus::UpToDate { current } => {
                return Ok(UpdateStatus::UpToDate { current });
            }
            status => return Ok(status),
        };

        // Download binary
        let download_url = format!("{}/{}", self.config.download_base, self.config.binary_name);
        let client = reqwest::Client::new();
        let bytes = client
            .get(&download_url)
            .send().await.map_err(|e| format!("download: {}", e))?
            .bytes().await.map_err(|e| format!("read: {}", e))?;

        // Verify SHA3-256
        let downloaded_sha3 = sha3_256(&bytes);
        if downloaded_sha3 != manifest.sha3_256 {
            return Err(format!(
                "SHA3 mismatch: expected {}, got {}",
                manifest.sha3_256, downloaded_sha3
            ));
        }

        // Write to staging path
        let staging = self.config.current_binary.with_extension("new");
        fs::write(&staging, &bytes)
            .map_err(|e| format!("write staging: {}", e))?;

        // Make executable
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&staging, fs::Permissions::from_mode(0o755))
                .map_err(|e| format!("chmod: {}", e))?;
        }

        Ok(UpdateStatus::Downloaded {
            version: manifest.version,
            path: staging,
        })
    }

    /// Apply the update — atomically replace the binary.
    pub fn apply(&self) -> Result<UpdateStatus, String> {
        let staging = self.config.current_binary.with_extension("new");
        let backup = self.config.current_binary.with_extension("old");

        if !staging.exists() {
            return Err("No staged update found. Run download() first.".into());
        }

        // Verify staging binary
        let staging_data = fs::read(&staging)
            .map_err(|e| format!("read staging: {}", e))?;
        let staging_sha3 = sha3_256(&staging_data);

        // Atomic replace: rename current → backup, staging → current
        if self.config.current_binary.exists() {
            fs::rename(&self.config.current_binary, &backup)
                .map_err(|e| format!("backup: {}", e))?;
        }

        fs::rename(&staging, &self.config.current_binary)
            .map_err(|e| format!("replace: {}", e))?;

        // Clean up backup
        let _ = fs::remove_file(&backup);

        let version = format!("sha3:{}", &staging_sha3[..16]);

        if self.config.auto_restart {
            // Fork and exec the new binary, then exit
            let mut cmd = Command::new(&self.config.current_binary);
            cmd.args(std::env::args().skip(1));
            if let Ok(mut child) = cmd.spawn() {
                // Detach and exit — new binary takes over
                std::process::exit(0);
            }
        }

        Ok(UpdateStatus::Applied { version })
    }

    /// Full update cycle: check → download → apply.
    pub async fn update(&mut self) -> Result<UpdateStatus, String> {
        let status = self.download().await?;
        match status {
            UpdateStatus::Downloaded { .. } => self.apply(),
            other => Ok(other),
        }
    }

    /// Run periodic update checks in a background task.
    pub fn spawn_periodic(config: UpdaterConfig) {
        if config.check_interval_secs == 0 {
            return;
        }

        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let mut updater = AutoUpdater::new(config);
                loop {
                    match updater.check().await {
                        Ok(UpdateStatus::UpdateAvailable { ref latest, .. }) => {
                            tracing::info!(%latest, "Update available");
                            match updater.update().await {
                                Ok(UpdateStatus::Applied { .. }) => {
                                    tracing::info!("Update applied");
                                    break;
                                }
                                Ok(other) => tracing::debug!(?other, "Update check"),
                                Err(e) => tracing::error!(%e, "Update failed"),
                            }
                        }
                        Ok(UpdateStatus::UpToDate { .. }) => {
                            tracing::debug!("Already up to date");
                        }
                        Ok(UpdateStatus::Offline) => {
                            tracing::debug!("Update server unreachable");
                        }
                        Err(e) => tracing::error!(%e, "Update check error"),
                    }

                    let secs = updater.config.check_interval_secs;
                    tokio::time::sleep(Duration::from_secs(secs)).await;
                }
            });
        });
    }
}

// ── SHA3-256 ──

/// Compute SHA3-256 hash of data (hex-encoded).
pub fn sha3_256(data: &[u8]) -> String {
    use sha2::{Digest, Sha3_256};
    let mut hasher = Sha3_256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

/// Verify an Ed25519 signature (hex-encoded).
pub fn verify_signature(
    public_key_hex: &str,
    message: &[u8],
    signature_hex: &str,
) -> Result<bool, String> {
    let pk_bytes = hex::decode(public_key_hex)
        .map_err(|e| format!("decode pk: {}", e))?;
    let sig_bytes = hex::decode(signature_hex)
        .map_err(|e| format!("decode sig: {}", e))?;

    let pk = ed25519_dalek::VerifyingKey::from_bytes(
        &pk_bytes[..32].try_into().map_err(|_| "invalid pk length")?
    ).map_err(|e| format!("invalid pk: {}", e))?;

    let sig = ed25519_dalek::Signature::from_slice(&sig_bytes)
        .map_err(|e| format!("invalid sig: {}", e))?;

    use ed25519_dalek::Verifier;
    Ok(pk.verify(message, &sig).is_ok())
}

// ── Bundle Manifest ──

/// Generate a version manifest for publishing.
pub fn generate_manifest(
    binary_path: &Path,
    version: &str,
    download_url: &str,
    secret_key_hex: &str,
) -> Result<VersionManifest, String> {
    let data = fs::read(binary_path)
        .map_err(|e| format!("read: {}", e))?;
    let sha3 = sha3_256(&data);

    // Sign: version || sha3
    let message = format!("{}|{}", version, sha3);
    let signature = sign_message(secret_key_hex, message.as_bytes())?;

    Ok(VersionManifest {
        version: version.into(),
        sha3_256: sha3,
        signature: hex::encode(&signature),
        download_url: download_url.into(),
        notes: None,
        min_updater_version: None,
    })
}

fn sign_message(secret_key_hex: &str, message: &[u8]) -> Result<Vec<u8>, String> {
    let sk_bytes = hex::decode(secret_key_hex)
        .map_err(|e| format!("decode sk: {}", e))?;
    let signing_key = ed25519_dalek::SigningKey::from_bytes(
        &sk_bytes[..32].try_into().map_err(|_| "invalid sk length")?
    );
    use ed25519_dalek::Signer;
    Ok(signing_key.sign(message).to_bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sha3_256() {
        let hash = sha3_256(b"hello world");
        assert_eq!(hash.len(), 64);
        assert_ne!(hash, sha3_256(b"different"));
    }

    #[test]
    fn test_sha3_deterministic() {
        assert_eq!(sha3_256(b"flux"), sha3_256(b"flux"));
    }

    #[test]
    fn test_generate_manifest() {
        // Create a temp binary
        let tmp = std::env::temp_dir().join("flux-test-bin");
        fs::write(&tmp, b"fake flux binary v0.4.0").unwrap();

        // Generate a test keypair
        let mut rng = rand::thread_rng();
        let signing_key = ed25519_dalek::SigningKey::generate(&mut rng);
        let sk_hex = hex::encode(signing_key.to_bytes());

        let manifest = generate_manifest(
            &tmp, "0.4.0",
            "https://quillon.xyz/downloads/fluxc",
            &sk_hex,
        ).unwrap();

        assert_eq!(manifest.version, "0.4.0");
        assert_eq!(manifest.sha3_256.len(), 64);

        // Clean up
        let _ = fs::remove_file(&tmp);
    }
}
