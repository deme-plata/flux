//! Content-addressed snapshot / restore / verify.
use std::fs;
use std::io;
use std::path::Path;
use serde::{Deserialize, Serialize};

/// One backed-up file: its path (relative), content id, and size.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry { /// relative path
    pub path: String, /// BLAKE3 content id (hex)
    pub cid: String, /// size in bytes
    pub size: u64 }

/// The snapshot manifest.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Manifest { /// files
    pub entries: Vec<Entry>, /// total logical bytes
    pub total_bytes: u64, /// unique (deduped) bytes actually stored
    pub unique_bytes: u64 }

fn cid_of(bytes: &[u8]) -> String { blake3::hash(bytes).to_hex().to_string() }

/// Snapshot every file directly under `src` into the CID `store` (dedup).
pub fn snapshot(src: &Path, store: &Path) -> io::Result<Manifest> {
    fs::create_dir_all(store)?;
    let mut m = Manifest::default();
    for e in fs::read_dir(src)? {
        let p = e?.path();
        if !p.is_file() { continue; }
        let bytes = fs::read(&p)?;
        let cid = cid_of(&bytes);
        let dst = store.join(&cid);
        let size = bytes.len() as u64;
        if !dst.exists() { fs::write(&dst, &bytes)?; m.unique_bytes += size; }
        m.total_bytes += size;
        m.entries.push(Entry { path: p.file_name().unwrap().to_string_lossy().into(), cid, size });
    }
    Ok(m)
}

/// Restore the manifest into `dst`, VERIFYING each file hashes to its CID.
/// Returns the number of files restored; errors on any integrity mismatch.
pub fn restore(m: &Manifest, store: &Path, dst: &Path) -> io::Result<usize> {
    fs::create_dir_all(dst)?;
    let mut n = 0;
    for e in &m.entries {
        let bytes = fs::read(store.join(&e.cid))?;
        if cid_of(&bytes) != e.cid {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("integrity fail: {}", e.path)));
        }
        fs::write(dst.join(&e.path), &bytes)?;
        n += 1;
    }
    Ok(n)
}

/// Verify the store has every CID and each hashes correctly (no restore).
pub fn verify(m: &Manifest, store: &Path) -> Result<(), String> {
    for e in &m.entries {
        let bytes = fs::read(store.join(&e.cid)).map_err(|_| format!("missing {}", e.cid))?;
        if cid_of(&bytes) != e.cid { return Err(format!("corrupt {}", e.cid)); }
    }
    Ok(())
}

/// Path of the persisted manifest inside a CID store.
pub fn manifest_path(store: &Path) -> std::path::PathBuf { store.join("manifest.json") }

/// Snapshot `src` AND persist the manifest into the store, so a later verify or
/// restore needs only the store path (no manifest passed around). This is the
/// entry point the MCP `flux_archive` tool drives.
pub fn snapshot_and_save(src: &Path, store: &Path) -> io::Result<Manifest> {
    let m = snapshot(src, store)?;
    let json = serde_json::to_vec_pretty(&m)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(manifest_path(store), json)?;
    Ok(m)
}

/// Load the persisted manifest from a store.
pub fn load_manifest(store: &Path) -> io::Result<Manifest> {
    let bytes = fs::read(manifest_path(store))?;
    serde_json::from_slice(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_save_load_roundtrip() {
        let base = std::env::temp_dir().join(format!("flux-archive-mcp-{}", std::process::id()));
        let (src, store, dst) = (base.join("src"), base.join("store"), base.join("dst"));
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("x.bin"), b"persisted manifest").unwrap();
        let m = snapshot_and_save(&src, &store).unwrap();
        assert!(manifest_path(&store).exists(), "manifest persisted");
        // load it back stateless and use it for verify + restore
        let loaded = load_manifest(&store).unwrap();
        assert_eq!(loaded.entries.len(), m.entries.len());
        assert!(verify(&loaded, &store).is_ok());
        assert_eq!(restore(&loaded, &store, &dst).unwrap(), 1);
        let _ = fs::remove_dir_all(&base);
    }
    #[test]
    fn snapshot_restore_verify_roundtrip() {
        let base = std::env::temp_dir().join(format!("flux-archive-test-{}", std::process::id()));
        let (src, store, dst) = (base.join("src"), base.join("store"), base.join("dst"));
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.bin"), b"hello aether").unwrap();
        fs::write(src.join("b.bin"), b"hello aether").unwrap(); // dup → dedup
        fs::write(src.join("c.bin"), vec![7u8; 4096]).unwrap();
        let m = snapshot(&src, &store).unwrap();
        assert_eq!(m.entries.len(), 3);
        assert!(m.unique_bytes < m.total_bytes, "dedup saved bytes");
        assert!(verify(&m, &store).is_ok());
        assert_eq!(restore(&m, &store, &dst).unwrap(), 3);
        assert_eq!(fs::read(dst.join("c.bin")).unwrap(), vec![7u8; 4096]);
        // tamper a store object → verify + restore must fail.
        let cid = &m.entries[2].cid;
        fs::write(store.join(cid), b"corrupted").unwrap();
        assert!(verify(&m, &store).is_err());
        let _ = fs::remove_dir_all(&base);
    }
}
