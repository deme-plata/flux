//! storage_pool.rs — one aether-shared disk pool on /home/storage.
//!
//! Shares the SAME backing storage between (a) the SIGIL-sync node DB and
//! (b) browser SIGIL-OS user quotas — served over flux-aether. Free space on the
//! 68 TB /home/storage NVMe is allocated fairly: reserve the sync DB first, then
//! hand each browser-OS user a quota dir. No double-counting; allocate only if
//! the pool has room. (Allocation = a tracked reservation + a per-user dir under
//! the aether root; the agent never deletes a user's data without consent.)

#[derive(Debug, Clone)]
pub struct StoragePool {
    pub root: String,         // e.g. /home/storage/aether
    pub total_gb: f64,        // pool size (from df on /home/storage)
    pub sigil_db_gb: f64,     // reserved for the SIGIL-sync node DB (shared pool)
    pub users: Vec<(String, f64)>, // (user_id, quota_gb)
}

impl StoragePool {
    pub fn new(root: &str, total_gb: f64, sigil_db_gb: f64) -> Self {
        Self { root: root.into(), total_gb, sigil_db_gb, users: vec![] }
    }
    pub fn allocated_gb(&self) -> f64 { self.sigil_db_gb + self.users.iter().map(|(_, g)| g).sum::<f64>() }
    pub fn free_gb(&self) -> f64 { (self.total_gb - self.allocated_gb()).max(0.0) }

    /// Allocate `gb` to a browser SIGIL-OS user from the shared pool. Fails if the
    /// pool (after the sync-DB reservation) can't fit it.
    pub fn allocate(&mut self, user: &str, gb: f64) -> Result<String, String> {
        if gb <= 0.0 { return Err("quota must be > 0".into()); }
        if gb > self.free_gb() {
            return Err(format!("pool full: {:.0}GB free < {:.0}GB requested (sigil-sync DB reserves {:.0}GB)", self.free_gb(), gb, self.sigil_db_gb));
        }
        if let Some(u) = self.users.iter_mut().find(|(id, _)| id == user) { u.1 += gb; }
        else { self.users.push((user.into(), gb)); }
        Ok(self.user_dir(user))
    }
    /// The user's aether dir — shares the pool with the sync DB under one root.
    pub fn user_dir(&self, user: &str) -> String { format!("{}/users/{}", self.root.trim_end_matches('/'), user) }
    /// The SIGIL-sync DB dir in the same pool.
    pub fn sigil_sync_dir(&self) -> String { format!("{}/sigil-sync-db", self.root.trim_end_matches('/')) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_one_pool_between_sync_db_and_users() {
        // 68000GB pool, reserve 2500GB for the sigil-sync DB
        let mut p = StoragePool::new("/home/storage/aether", 68000.0, 2500.0);
        assert!((p.free_gb() - 65500.0).abs() < 1e-6);
        let dir = p.allocate("browser-user-1", 50.0).unwrap();
        assert_eq!(dir, "/home/storage/aether/users/browser-user-1");
        assert!((p.free_gb() - 65450.0).abs() < 1e-6);
        assert_eq!(p.sigil_sync_dir(), "/home/storage/aether/sigil-sync-db");
    }

    #[test]
    fn refuses_over_pool_capacity() {
        let mut p = StoragePool::new("/home/storage/aether", 100.0, 90.0); // only 10GB free
        assert!(p.allocate("u1", 5.0).is_ok());
        assert!(p.allocate("u1", 10.0).is_err()); // 5 free < 10 → refused
    }

    #[test]
    fn same_user_quota_accumulates() {
        let mut p = StoragePool::new("/home/storage/aether", 1000.0, 0.0);
        p.allocate("u", 10.0).unwrap();
        p.allocate("u", 5.0).unwrap();
        assert_eq!(p.users.len(), 1);
        assert!((p.allocated_gb() - 15.0).abs() < 1e-6);
    }
}
