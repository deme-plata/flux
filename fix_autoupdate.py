"""Fix auto-update infinite loop: skip if already updated, don't exec."""
path = "/home/storage/deepseek-codewhale/flux/crates/fluxmux/src/main.rs"
with open(path) as f:
    c = f.read()

# Fix: add guard against re-download + remove auto-exec (let user restart)
old = '''/// Background update check: fetch version from server, download if newer.
fn check_for_update_bg() {
    std::thread::spawn(|| {
        let client = match reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(5)).build() {
            Ok(c) => c, Err(_) => return,
        };
        let remote = match client.get("https://quillon.xyz/downloads/fluxmux.version")
            .send().and_then(|r| r.text()) {
            Ok(t) => t, Err(_) => return,
        };
        let remote = remote.trim();
        if remote == env!("CARGO_PKG_VERSION") { return; }
        let bytes = match client.get("https://quillon.xyz/downloads/fluxmux")
            .send().and_then(|r| r.bytes()) {
            Ok(b) => b, Err(_) => return,
        };
        let path = "/tmp/fluxmux.new";
        if std::fs::write(path, &bytes).is_err() { return; }
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
        // Signal main loop + auto-exec
        let _ = std::fs::write("/tmp/flux-update.ready", &remote);
        use std::os::unix::process::CommandExt;
        let _ = std::process::Command::new(path).exec();
    });
}'''

new = '''/// Background update check: fetch version from server, download if newer.
/// Writes /tmp/flux-update.ready flag for UI indicator. User restarts manually.
fn check_for_update_bg() {
    std::thread::spawn(|| {
        // Guard: skip if already downloaded this version
        if std::fs::metadata("/tmp/fluxmux.new").is_ok() { return; }
        let client = match reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(10)).build() {
            Ok(c) => c, Err(_) => return,
        };
        let remote = match client.get("https://quillon.xyz/downloads/fluxmux.version")
            .send().and_then(|r| r.text()) {
            Ok(t) => t.trim().to_string(), Err(_) => return,
        };
        if remote == env!("CARGO_PKG_VERSION") { return; }
        let bytes = match client.get("https://quillon.xyz/downloads/fluxmux")
            .send().and_then(|r| r.bytes()) {
            Ok(b) => b, Err(_) => return,
        };
        let path = "/tmp/fluxmux.new";
        if std::fs::write(path, &bytes).is_err() { return; }
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
        // Signal main loop for UI indicator (no auto-exec — user restarts manually)
        let _ = std::fs::write("/tmp/flux-update.ready", &remote);
    });
}'''

c = c.replace(old, new)
with open(path, 'w') as f:
    f.write(c)
print("OK: Fixed auto-update infinite loop + removed auto-exec")
