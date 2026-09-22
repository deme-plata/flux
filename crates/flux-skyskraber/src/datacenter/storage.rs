use std::{fs, io::Write, path::{Path, PathBuf}};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use flux_aether::{FileBlock, Shard};

#[derive(Serialize, Deserialize)]
struct Object { manifest: FileBlock, shards: Vec<Shard> }
pub struct Store { root: PathBuf, key: [u8;32], quota: u64 }
pub fn private_file(path:&Path, bytes:&[u8])->super::Result<()> {
    let mut options=fs::OpenOptions::new(); options.write(true).create_new(true);
    #[cfg(unix)] {use std::os::unix::fs::OpenOptionsExt;options.mode(0o600);}
    let mut f=options.open(path)?; f.write_all(bytes)?; f.sync_all()?; Ok(())
}
impl Store {
    pub fn open(root:&Path,quota:u64)->super::Result<Self> {
        fs::create_dir_all(root)?;
        #[cfg(unix)] {use std::os::unix::fs::PermissionsExt;fs::set_permissions(root,fs::Permissions::from_mode(0o700))?;}
        let key_path=root.join("aether.key");
        if !key_path.exists() {let mut bytes=[0u8;32];rand::rngs::OsRng.fill_bytes(&mut bytes);private_file(&key_path,&bytes)?;}
        let key: [u8;32]=fs::read(key_path)?.try_into().map_err(|_|"Aether key must have 32 bytes")?;
        Ok(Self {root:root.to_owned(),key,quota})
    }
    pub fn put(&self,data:&[u8])->super::Result<String> {
        let (manifest,shards)=flux_aether::shard_file(data,4096,&self.key,[0;32]);
        let id=hex::encode(manifest.content_root); let path=self.root.join(format!("{id}.json"));
        if path.exists() {if self.get(&id)? != data {return Err("existing object mismatch".into());} return Ok(id);}
        let recovered=flux_aether::reassemble(&manifest,&shards,&self.key).map_err(|e|format!("Aether verification: {e:?}"))?;
        if recovered!=data {return Err("Aether roundtrip mismatch".into());}
        let bytes=serde_json::to_vec(&Object{manifest,shards})?;
        let used=fs::read_dir(&self.root)?.try_fold(0u64,|n,e|->std::io::Result<u64>{Ok(n+e?.metadata()?.len())})?;
        if used.saturating_add(bytes.len() as u64)>self.quota {return Err("Aether storage quota reached".into());}
        private_file(&path,&bytes)?; Ok(id)
    }
    pub fn get(&self,id:&str)->super::Result<Vec<u8>> {
        if id.len()!=64 || !id.bytes().all(|b|b.is_ascii_hexdigit()) {return Err("object id must be 64 hex characters".into());}
        let o:Object=serde_json::from_slice(&fs::read(self.root.join(format!("{}.json",id.to_lowercase())))?)?;
        let data=flux_aether::reassemble(&o.manifest,&o.shards,&self.key).map_err(|e|format!("Aether verification: {e:?}"))?;
        if blake3::hash(&data).to_hex().as_str()!=id.to_lowercase() {return Err("object identity mismatch".into());}
        Ok(data)
    }
}
#[cfg(test)] mod tests {use super::*;
    #[test] fn persist_reopen_and_detect_corruption() {
        let p=std::env::temp_dir().join(format!("quillon-store-{}",rand::random::<u64>()));
        let s=Store::open(&p,65536).unwrap();let id=s.put(b"verified receipt").unwrap();drop(s);
        let s=Store::open(&p,65536).unwrap();assert_eq!(s.get(&id).unwrap(),b"verified receipt");
        assert!(s.get("../key").is_err());fs::write(p.join(format!("{id}.json")),b"bad").unwrap();assert!(s.get(&id).is_err());
        fs::remove_dir_all(p).unwrap();
    }
}
