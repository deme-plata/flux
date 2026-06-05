"""Debug: add eprintln tracing to auto-update."""
path = "/home/storage/deepseek-codewhale/flux/crates/fluxmux/src/main.rs"
with open(path) as f:
    c = f.read()

old = '''        // Guard: skip if already downloaded this version
        if std::fs::metadata("/tmp/fluxmux.new").is_ok() { return; }'''

new = '''        // Guard: skip if already downloaded this version
        if std::fs::metadata("/tmp/fluxmux.new").is_ok() {
            eprintln!("[auto-update] already downloaded, skipping");
            return;
        }'''

c = c.replace(old, new)

old2 = '''        if remote == env!("CARGO_PKG_VERSION") { return; }'''
new2 = '''        eprintln!("[auto-update] remote={} local={}", remote, env!("CARGO_PKG_VERSION"));
        if remote == env!("CARGO_PKG_VERSION") { eprintln!("[auto-update] same version, skipping"); return; }'''

c = c.replace(old2, new2)

old3 = '''            Ok(b) => b, Err(_) => return,'''
new3 = '''            Ok(b) => { eprintln!("[auto-update] downloaded {} bytes", b.len()); b },
            Err(e) => { eprintln!("[auto-update] download error: {}", e); return; },'''

c = c.replace(old3, new3)

with open(path, 'w') as f:
    f.write(c)
print("OK: Added debug traces")
