//! flux-rev CLI — content-addressed, genesis-stamped version control over a working dir.
//!   flux-rev genesis <dir> [--from <src>] [--version <v>] [--note <s>]   import canonical source
//!   flux-rev snapshot <dir> [-m <msg>]                                   new revision (parent=HEAD)
//!   flux-rev checkout <dir> <revid> [--into <dest>]                      materialize a revision
//!   flux-rev log <dir>                                                   lineage from HEAD
//!   flux-rev diff <dir> <a> <b>                                          exact path-level diff
//!   flux-rev head <dir>                                                  print HEAD revision id
use flux_rev::*;
use std::path::Path;

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
        _ => {
            eprintln!("flux-rev — content-addressed version control (git replacement)\n  genesis|snapshot|checkout|log|diff|head <dir> …");
        }
    }
}
