//! flux-rev CLI — content-addressed, genesis-stamped version control over a working dir.
//!   flux-rev genesis <dir> [--from <src>] [--version <v>] [--note <s>]   import canonical source
//!   flux-rev snapshot <dir> [-m <msg>]                                   new revision (parent=HEAD)
//!   flux-rev checkout <dir> <revid> [--into <dest>]                      materialize a revision
//!   flux-rev log <dir>                                                   lineage from HEAD
//!   flux-rev diff <dir> <a> <b>                                          exact path-level diff
//!   flux-rev head <dir>                                                  print HEAD revision id
//!   flux-rev roadmap [--out <f>] [--flux-root <d>] [--sigil-root <d>]    signed roadmap attestation
//!   flux-rev roadmap --verify <f> [--recheck] [--allow-unsigned]         re-check hash + signature
use flux_rev::*;
use std::path::{Path, PathBuf};

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let cmd = a.first().map(|s| s.as_str()).unwrap_or("");
    let author = std::env::var("FLUX_REV_AUTHOR").unwrap_or_else(|_| "claude-desktop-viktor".into());
    match cmd {
        "genesis" => {
            let dir = a.get(1).cloned().unwrap_or_default();
            let work = Path::new(&dir);
            let from = arg(&a, "--from").unwrap_or_else(|| "git-daemon".into());
            let version = arg(&a, "--version").unwrap_or_else(|| "0.0.0".into());
            let note = arg(&a, "--note").unwrap_or_else(|| "genesis import of canonical source".into());
            let store = Store::open(work).expect("open store");
            let g = stamp_genesis(&store, &from, &version, &author, &note).expect("stamp");
            let rev = snapshot(work, &store, None, &g.id(), &version, &author, "genesis import").expect("snapshot");
            let files = store.get_manifest(&rev.manifest).map(|m| m.entries.len()).unwrap_or(0);
            println!("🌱 genesis stamped — {} (from {}, v{}, by {})", &g.id()[..16], from, version, author);
            println!("📦 revision {}  ·  {} files  ·  HEAD set", &rev.id[..16], files);
            println!("   full: {}", rev.id);
        }
        "snapshot" => {
            let dir = a.get(1).cloned().unwrap_or_default();
            let work = Path::new(&dir);
            let msg = arg(&a, "-m").or_else(|| arg(&a, "--message")).unwrap_or_else(|| "snapshot".into());
            let store = Store::open(work).expect("open store");
            let head = store.read_head();
            let (gid, version) = match &head {
                Some(h) => { let r = store.get_revision(h).expect("head rev"); (r.genesis, r.workspace_version) }
                None => { eprintln!("✗ no HEAD — run `flux-rev genesis {}` first", dir); std::process::exit(2); }
            };
            let rev = snapshot(work, &store, head.clone(), &gid, &version, &author, &msg).expect("snapshot");
            if Some(&rev.id) == head.as_ref() {
                println!("· no changes — HEAD unchanged at {}", &rev.id[..16]);
            } else {
                println!("📦 revision {}  ·  parent {}", &rev.id[..16], head.as_deref().map(|h| &h[..16]).unwrap_or("∅"));
                if let Some(p) = &rev.parent {
                    if let Ok(d) = diff(&store, p, &rev.id) {
                        println!("   +{} ~{} -{}", d.added.len(), d.changed.len(), d.removed.len());
                    }
                }
                println!("   full: {}", rev.id);
            }
        }
        "checkout" => {
            let dir = a.get(1).cloned().unwrap_or_default();
            let revid = a.get(2).cloned().unwrap_or_default();
            let into = arg(&a, "--into").unwrap_or_else(|| dir.clone());
            let store = Store::open(Path::new(&dir)).expect("open store");
            let n = checkout(&store, &revid, Path::new(&into)).expect("checkout");
            println!("✅ checked out {} → {} ({} files)", &revid[..16.min(revid.len())], into, n);
        }
        "log" => {
            let dir = a.get(1).cloned().unwrap_or_default();
            let store = Store::open(Path::new(&dir)).expect("open store");
            let mut cur = store.read_head();
            let mut n = 0;
            while let Some(id) = cur {
                let r = match store.get_revision(&id) { Ok(r) => r, Err(_) => break };
                println!("● {}  {}  «{}»  ({})", &r.id[..16], r.author, r.message, r.workspace_version);
                cur = r.parent; n += 1;
                if n > 5000 { break; }
            }
            if n == 0 { println!("(no revisions — run genesis)"); }
        }
        "diff" => {
            let dir = a.get(1).cloned().unwrap_or_default();
            let (x, y) = (a.get(2).cloned().unwrap_or_default(), a.get(3).cloned().unwrap_or_default());
            let store = Store::open(Path::new(&dir)).expect("open store");
            let d = diff(&store, &x, &y).expect("diff");
            for p in &d.added { println!("+ {}", p); }
            for p in &d.changed { println!("~ {}", p); }
            for p in &d.removed { println!("- {}", p); }
            println!("  +{} ~{} -{}", d.added.len(), d.changed.len(), d.removed.len());
        }
        "head" => {
            let dir = a.get(1).cloned().unwrap_or_default();
            let store = Store::open(Path::new(&dir)).expect("open store");
            println!("{}", store.read_head().unwrap_or_else(|| "(none)".into()));
        }
        "roadmap" => cmd_roadmap(&a, &author),
        _ => {
            eprintln!("flux-rev — content-addressed version control (git replacement)\n  genesis|snapshot|checkout|log|diff|head <dir> …\n  roadmap [--out <f>] | roadmap --verify <f> [--recheck] [--allow-unsigned]");
        }
    }
}

// ── roadmap attestation ──

/// Resolve `--flux-root` / `--sigil-root`. Defaults are the two checkouts this tool is normally
/// run from; both are overridable because the attestation must be reproducible on another machine.
fn roots(a: &[String]) -> roadmap::Roots {
    let mut r = roadmap::Roots::new();
    let flux = arg(a, "--flux-root")
        .or_else(|| std::env::var("FLUX_ROOT").ok())
        .unwrap_or_else(|| "/home/storage/deepseek-codewhale/flux".into());
    let sigil = arg(a, "--sigil-root")
        .or_else(|| std::env::var("SIGIL_ROOT").ok())
        .unwrap_or_else(|| "/home/storage/deepseek-codewhale/sigil".into());
    r.insert("flux".into(), PathBuf::from(flux));
    r.insert("sigil".into(), PathBuf::from(sigil));
    r
}

/// The fluxc version the attestation is stamped with. Read from the workspace manifest of the
/// flux root — anchored to line start and first match only, because the `version =` line in that
/// file carries a long trailing comment that a loose grep happily matches inside.
fn fluxc_version(flux_root: &Path) -> String {
    std::fs::read_to_string(flux_root.join("Cargo.toml"))
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("version = \""))
                .and_then(|l| l.split('"').nth(1).map(str::to_string))
        })
        .unwrap_or_else(|| "unknown".into())
}

fn fluxc_git(flux_root: &Path) -> String {
    std::process::Command::new("git")
        .args(["-C", &flux_root.to_string_lossy(), "rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

fn cmd_roadmap(a: &[String], author: &str) {
    let rs = roots(a);
    let has = |f: &str| a.iter().any(|x| x == f);

    if let Some(file) = arg(a, "--verify") {
        let bytes = match std::fs::read(&file) {
            Ok(b) => b,
            Err(e) => { eprintln!("✗ read {file}: {e}"); std::process::exit(2); }
        };
        let att = match roadmap::from_json(&bytes) {
            Ok(x) => x,
            Err(e) => { eprintln!("✗ VERIFY FAILED — {e}"); std::process::exit(3); }
        };
        match roadmap::verify(&att, has("--allow-unsigned")) {
            Ok(v) => {
                println!("✅ ROADMAP ATTESTATION VERIFIED");
                println!("   bundle   {}", v.bundle_id);
                println!("   layers   entries→tree_id ✓   body→bundle_id ✓   signature {}",
                    if v.hybrid { "✓ require-both SQIsign-L5 + Ed25519" }
                    else if v.signed { "✓ (single leg)" } else { "· UNSIGNED (accepted by --allow-unsigned)" });
                if v.signed { println!("   signer   sqisign pk {}…", &v.signer_sqisign_pk_hex[..32.min(v.signer_sqisign_pk_hex.len())]); }
                println!("   project  {}  ·  fluxc {} ({})", att.body.project, att.body.fluxc_version, att.body.fluxc_git);
                for (id, st, tid) in &v.items {
                    println!("   · {:<13} {:<11} {}", id, st.as_str(), tid);
                }
                if has("--recheck") {
                    match roadmap::recheck(&att, &rs) {
                        Ok(d) if d.is_empty() => println!("   recheck  ✓ working tree still matches every attested byte"),
                        Ok(d) => {
                            println!("   recheck  ⚠ {} attested path(s) have DRIFTED since this attestation", d.len());
                            for line in &d { println!("            - {line}"); }
                            println!("            (drift is not tamper — the attestation is still authentic for the tree it named)");
                        }
                        Err(e) => println!("   recheck  ✗ {e}"),
                    }
                }
            }
            Err(e) => { eprintln!("✗ VERIFY FAILED — {e}"); std::process::exit(3); }
        }
        return;
    }

    let flux_root = rs.get("flux").cloned().unwrap_or_default();
    let body = match roadmap::build_body(
        &rs,
        &fluxc_version(&flux_root),
        &fluxc_git(&flux_root),
        author,
        roadmap::now(),
    ) {
        Ok(b) => b,
        Err(e) => { eprintln!("✗ {e}"); std::process::exit(2); }
    };

    let att = if has("--unsigned") {
        roadmap::unsigned(body)
    } else {
        match roadmap::load_agent_keys_hybrid(arg(a, "--key").as_deref()) {
            Ok(k) => roadmap::sign_body(body, &k),
            Err(e) => { eprintln!("✗ {e}\n   (use --key <file>, or --unsigned to emit hash-only)"); std::process::exit(2); }
        }
    };
    let att = match att { Ok(x) => x, Err(e) => { eprintln!("✗ {e}"); std::process::exit(2); } };

    let out = arg(a, "--out").unwrap_or_else(|| "roadmap-attestation.json".into());
    let json = roadmap::to_json(&att).expect("serialize");
    if let Err(e) = std::fs::write(&out, &json) { eprintln!("✗ write {out}: {e}"); std::process::exit(2); }

    println!("🗺️  roadmap attestation — {} item(s), {} bytes", att.body.items.len(), json.len());
    for it in &att.body.items {
        println!("   · {:<13} {:<11} {}  ({} files, {} B)", it.id, it.status.as_str(), it.tree_id, it.files, it.bytes);
    }
    println!("   bundle   {}", att.bundle_id);
    println!("   signed   {}", if att.is_hybrid() { "require-both SQIsign-L5 + Ed25519" } else if att.is_signed() { "SQIsign only" } else { "NO (--unsigned)" });
    println!("   → {out}");
    println!("   verify:  flux-rev roadmap --verify {out} --recheck");
}
