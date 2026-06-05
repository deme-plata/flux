//! flux-aether-magnet <file> [key] — shard a file through flux-aether and print
//! its magnet link. The file becomes torrent-distributable: mixed, encrypted,
//! content-addressed, here/there/nowhere.
use std::fs;
use flux_aether::shard_file;
use flux_torrent::{magnet, TorrentInfo};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let file = a.get(1).cloned().unwrap_or_default();
    let key = a.get(2).map(|s| s.as_bytes().to_vec()).unwrap_or_else(|| b"flux-aether-public".to_vec());
    let data = fs::read(&file).expect("read file");
    let (fb, shards) = shard_file(&data, 16384, &key, [0u8; 32]);
    let t = TorrentInfo::from_file_block(&fb);
    eprintln!("file: {file} ({} bytes) -> {} shards ({} data + parity), content-addressed", data.len(), fb.n, fb.k);
    let _ = shards;
    println!("{}", magnet(&t));
}
