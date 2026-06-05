// flux-macros — Declarative macros for fast Flux iteration

/// version_bump! — Write current Flux workspace version to a version file.
/// Reads workspace Cargo.toml dynamically instead of hardcoding semver.
/// Also writes to dist-final/downloads/ for the live CDN.
#[macro_export]
macro_rules! version_bump {
    () => {{
        // Try to read the workspace version from Cargo.toml at runtime
        let v = std::env::current_dir()
            .ok()
            .and_then(|d| {
                std::fs::read_to_string(d.join("Cargo.toml")).ok()
            })
            .and_then(|content| {
                content.lines()
                    .skip_while(|l| !l.trim().starts_with("[workspace.package]"))
                    .skip(1)
                    .take_while(|l| !l.trim().starts_with('['))
                    .find_map(|line| {
                        let t = line.trim();
                        if t.starts_with("version") {
                            t.find('"').and_then(|s| {
                                t[s+1..].find('"').map(|e| t[s+1..s+1+e].to_string())
                            })
                        } else { None }
                    })
            })
            .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
        let _ = std::fs::write(
            "/home/orobit/q-narwhalknight/dist-final/downloads/fluxmux.version",
            &v,
        );
        v
    }};
    ($major:expr, $minor:expr, $patch:expr) => {{
        // Legacy form with hardcoded values — still works but prefers dynamic
        let v = format!("{}.{}.{}", $major, $minor, $patch);
        let _ = std::fs::write("/home/orobit/q-narwhalknight/dist-final/downloads/fluxmux.version", &v);
        v
    }};
}

#[macro_export]
macro_rules! benchmark {
    ($label:expr, $expr:expr) => {{
        let start = std::time::Instant::now();
        let result = { $expr };
        eprintln!("⚡ {}: {:?}", $label, start.elapsed());
        result
    }};
}

#[macro_export]
macro_rules! heal_on_fail {
    ($expr:expr, $fallback:expr) => {{
        match $expr { Ok(val) => val, Err(e) => { eprintln!("🏥 Heal: {}", e); $fallback } }
    }};
}

#[macro_export]
macro_rules! animate_update {
    ($total:expr, $body:block) => {{
        for step in 0..$total {
            let filled = (step+1)*20/$total;
            let bar: String = (0..20).map(|i| if i<filled {'█'} else {'░'}).collect();
            eprintln!("\r⬇️ [{}{}] {}/{}", bar, " ".repeat(20-filled), step+1, $total);
            $body
        }
        eprintln!("\r✅ Update complete!                    ");
    }};
}

/// fluxc_build! — Compile a package via fluxc and fire webhook to fluxmux
#[macro_export]
macro_rules! fluxc_build {
    ($pkg:expr) => {{
        use std::process::Command;
        let start = std::time::Instant::now();
        let output = Command::new("./target/debug/fluxc")
            .args(["build", "--rust-only", "-p", $pkg]).output();
        let elapsed = start.elapsed().as_millis() as u64;
        let success = output.as_ref().map(|o| o.status.success()).unwrap_or(false);
        // Fire webhook to fluxmux :9099
        if let Ok(client) = reqwest::blocking::Client::new()
            .post("http://127.0.0.1:9099/build_complete")
            .json(&serde_json::json!({"pkg":$pkg,"success":success,"elapsed_ms":elapsed}))
            .send() { let _ = client; }
        output
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn test_benchmark() { assert_eq!(benchmark!("t",{2+2}), 4); }
    #[test] fn test_heal_ok() { assert_eq!(heal_on_fail!(Ok::<i32,&str>(42),0), 42); }
    #[test] fn test_heal_err() { assert_eq!(heal_on_fail!(Err::<i32,&str>("x"),99), 99); }
}
