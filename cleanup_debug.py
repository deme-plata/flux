"""Remove debug traces from auto-update."""
path = "/home/storage/deepseek-codewhale/flux/crates/fluxmux/src/main.rs"
with open(path) as f:
    c = f.read()

# Remove debug traces
c = c.replace(
    '''        // Guard: skip if already downloaded this version
        if std::fs::metadata("/tmp/fluxmux.new").is_ok() {
            eprintln!("[auto-update] already downloaded, skipping");
            return;
        }''',
    '''        // Guard: skip if already downloaded this version
        if std::fs::metadata("/tmp/fluxmux.new").is_ok() { return; }'''
)

c = c.replace(
    '''        eprintln!("[auto-update] remote={} local={}", remote, env!("CARGO_PKG_VERSION"));
        if remote == env!("CARGO_PKG_VERSION") { eprintln!("[auto-update] same version, skipping"); return; }''',
    '''        if remote == env!("CARGO_PKG_VERSION") { return; }'''
)

c = c.replace(
    '''            Ok(b) => { eprintln!("[auto-update] downloaded {} bytes", b.len()); b },
            Err(e) => { eprintln!("[auto-update] download error: {}", e); return; },''',
    '''            Ok(b) => b, Err(_) => return;,'''
)

with open(path, 'w') as f:
    f.write(c)
print("OK: Removed debug traces")
