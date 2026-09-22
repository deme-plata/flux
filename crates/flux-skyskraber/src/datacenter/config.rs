use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, path::PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub name: String,
    pub data_dir: PathBuf,
    pub http_listen: SocketAddr,
    pub p2p_port: u16,
    pub bootstrap_peers: Vec<String>,
    pub workers: usize,
    pub max_job_bytes: usize,
    pub storage_quota_bytes: u64,
    pub sigil_topology_url: String,
    pub sigil_topic_prefix: String,
    pub workspace: PathBuf,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            name: "quillon-datacenter".into(), data_dir: "./datacenter-data".into(),
            http_listen: "127.0.0.1:19470".parse().unwrap(), p2p_port: 19471,
            bootstrap_peers: vec![], workers: 2, max_job_bytes: 262144,
            storage_quota_bytes: 67108864,
            sigil_topology_url: "http://127.0.0.1:18181/v1/network/topology".into(),
            sigil_topic_prefix: "/sigil/g2/".into(), workspace: ".".into(),
        }
    }
}
impl Config {
    pub fn validate(&self) -> super::Result<()> {
        if !self.http_listen.ip().is_loopback() || self.http_listen.port() == 0 {
            return Err("HTTP must use a nonzero loopback address; use an authenticated gateway for remote access".into());
        }
        if self.p2p_port == 0 || self.p2p_port == self.http_listen.port() {
            return Err("P2P and HTTP need different nonzero ports".into());
        }
        if !(1..=32).contains(&self.workers) || !(1..=1_048_576).contains(&self.max_job_bytes) {
            return Err("workers must be 1..32; max_job_bytes must be 1..1048576".into());
        }
        if self.storage_quota_bytes < 65536 || self.storage_quota_bytes > 1_073_741_824 {
            return Err("storage quota must be 64 KiB..1 GiB in this pilot".into());
        }
        if self.name.is_empty() || self.name.len() > 128 || self.data_dir.as_os_str().is_empty() {
            return Err("name and data_dir must be valid".into());
        }
        let url = reqwest::Url::parse(&self.sigil_topology_url)?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none()
            || !url.username().is_empty() || url.password().is_some() {
            return Err("SIGIL URL must be HTTP(S) without embedded credentials".into());
        }
        if !self.sigil_topic_prefix.starts_with("/sigil/") || !self.sigil_topic_prefix.ends_with('/') {
            return Err("SIGIL topic prefix must look like /sigil/g2/".into());
        }
        Ok(())
    }
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn configuration_rejects_exposed_api_and_unbounded_workers() {
        let mut c=Config::default(); c.validate().unwrap();
        c.http_listen="0.0.0.0:19470".parse().unwrap(); assert!(c.validate().is_err());
        c=Config::default(); c.workers=0; assert!(c.validate().is_err());
        c=Config::default(); c.p2p_port=c.http_listen.port(); assert!(c.validate().is_err());
        assert!(serde_json::from_str::<Config>(r#"{"workerz":4}"#).is_err());
    }
}
