//! Column families for flux-db (v0.14).
//!
//! A column family is an isolated key-space stored in its own sub-directory
//! `<db_path>/cf_<name>/`. Same key in two CFs holds two values; dropping a
//! CF is a `rm -rf` of its subdirectory.
//!
//! Each CF is itself a `Database` — same memtable + WAL + SST machinery,
//! same transaction semantics, same block cache (CFs share one cache so
//! hot data from one doesn't push hot data from another out wholesale).
//!
//! API (RocksDB-shaped):
//!
//! ```text
//! let db = Database::open(path)?;
//! let users  = db.create_cf("users")?;
//! let orders = db.create_cf("orders")?;
//! users.put(b"u1", b"alice")?;
//! orders.put(b"u1", b"order123")?;     // independent — no clash
//! assert_eq!(users.get(b"u1")?,  Some(b"alice".to_vec()));
//! assert_eq!(orders.get(b"u1")?, Some(b"order123".to_vec()));
//! db.drop_cf("orders")?;               // removes the sub-directory
//! ```

use std::collections::HashMap;
use std::path::PathBuf;

use parking_lot::Mutex;

use crate::Database;

/// Stable name for the implicit "default" column family that every database
/// has. `db.put()` / `db.get()` always go through this one.
pub const DEFAULT_CF: &str = "default";

/// Sub-directory prefix for non-default column families on disk.
pub const CF_DIR_PREFIX: &str = "cf_";

/// Per-Database registry of open CF handles. Held by `Database` so repeated
/// `cf("users")` calls return the same handle (and therefore the same
/// memtable, not two copies of one).
pub(crate) struct CfRegistry {
    inner: Mutex<HashMap<String, Database>>,
    base_path: PathBuf,
}

impl CfRegistry {
    pub fn new(base_path: PathBuf) -> Self {
        Self { inner: Mutex::new(HashMap::new()), base_path }
    }

    /// Create or open a column family with the given name. Returns a
    /// `Database` handle scoped to the CF's sub-directory.
    pub fn create_cf(&self, name: &str) -> Result<Database, String> {
        if name == DEFAULT_CF {
            return Err("cannot create reserved CF \"default\"".into());
        }
        if name.is_empty() || name.contains('/') || name.contains('\0') {
            return Err(format!("invalid CF name {name:?}"));
        }
        let mut guard = self.inner.lock();
        if let Some(existing) = guard.get(name) {
            return Ok(existing.clone());
        }
        let cf_path = self.base_path.join(format!("{CF_DIR_PREFIX}{name}"));
        let db = Database::open(&cf_path)?;
        guard.insert(name.to_string(), db.clone());
        Ok(db)
    }

    /// Look up an already-opened CF by name. None if it hasn't been opened
    /// or doesn't exist.
    pub fn cf(&self, name: &str) -> Option<Database> {
        self.inner.lock().get(name).cloned()
    }

    /// List the names of currently-open CFs.
    pub fn list(&self) -> Vec<String> {
        self.inner.lock().keys().cloned().collect()
    }

    /// Drop a CF: removes the handle and recursively deletes its
    /// on-disk directory. Idempotent.
    pub fn drop_cf(&self, name: &str) -> Result<(), String> {
        if name == DEFAULT_CF {
            return Err("cannot drop the default CF".into());
        }
        self.inner.lock().remove(name);
        let cf_path = self.base_path.join(format!("{CF_DIR_PREFIX}{name}"));
        if cf_path.exists() {
            std::fs::remove_dir_all(&cf_path)
                .map_err(|e| format!("remove_dir_all {}: {}", cf_path.display(), e))?;
        }
        Ok(())
    }
}
