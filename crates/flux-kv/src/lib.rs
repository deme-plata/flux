//! flux-kv — a thin namespaced key/value store over flux-db.
//!
//! The durable server-side persistence layer for browser apps that can't link
//! Rust flux-db directly (no WASM/node binding). flux-vision's `/api/save`
//! bridge SSHes to epsilon and shells `flux-kv`, so project state survives
//! beyond browser localStorage, in a real LSM-tree store.

use flux_db::Database;

pub struct Kv {
    db: Database,
}

impl Kv {
    /// Open (or create) the store at `path`.
    pub fn open(path: impl Into<std::path::PathBuf>) -> Result<Self, String> {
        Ok(Self { db: Database::open(path)? })
    }

    pub fn put(&self, key: &str, value: &[u8]) -> Result<(), String> {
        self.db.put(key.as_bytes(), value)
    }

    pub fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        self.db.get(key.as_bytes())
    }

    pub fn delete(&self, key: &str) -> Result<(), String> {
        self.db.delete(key.as_bytes())
    }

    /// Keys (as UTF-8) sharing `prefix`, in sorted order.
    pub fn list(&self, prefix: &str) -> Vec<String> {
        self.db
            .scan_prefix(prefix.as_bytes())
            .into_iter()
            .filter_map(|(k, _)| String::from_utf8(k).ok())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("flux-kv-{tag}-{}-{n}", std::process::id()))
    }

    #[test]
    fn put_get_roundtrip() {
        let kv = Kv::open(tmp("rt")).unwrap();
        kv.put("flux-vision:projects", br#"[{"name":"a"}]"#).unwrap();
        let got = kv.get("flux-vision:projects").unwrap().unwrap();
        assert_eq!(got, br#"[{"name":"a"}]"#);
        assert!(kv.get("missing").unwrap().is_none());
    }

    #[test]
    fn delete_removes() {
        let kv = Kv::open(tmp("del")).unwrap();
        kv.put("k", b"v").unwrap();
        kv.delete("k").unwrap();
        assert!(kv.get("k").unwrap().is_none());
    }

    #[test]
    fn list_by_prefix() {
        let kv = Kv::open(tmp("list")).unwrap();
        kv.put("fv:a", b"1").unwrap();
        kv.put("fv:b", b"2").unwrap();
        kv.put("other:c", b"3").unwrap();
        let mut ks = kv.list("fv:");
        ks.sort();
        assert_eq!(ks, vec!["fv:a".to_string(), "fv:b".to_string()]);
    }

    #[test]
    fn overwrite_keeps_latest() {
        let kv = Kv::open(tmp("ow")).unwrap();
        kv.put("k", b"old").unwrap();
        kv.put("k", b"new").unwrap();
        assert_eq!(kv.get("k").unwrap().unwrap(), b"new");
    }
}
