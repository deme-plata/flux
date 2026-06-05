//! flux-boilerplate CLI — fluxc-owned UI app boilerplates for the design route.
//!   flux-boilerplate detect "<prompt>"   → prints the matching kind slug (empty if none)
//!   flux-boilerplate get <slug>           → prints the boilerplate HTML skeleton
//!   flux-boilerplate list                 → lists kinds
use flux_boilerplate::{boilerplate, detect, Kind, ALL};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    match a.first().map(|s| s.as_str()) {
        Some("detect") => {
            let prompt = a[1..].join(" ");
            if let Some(k) = detect(&prompt) { println!("{}", k.slug()); }
        }
        Some("get") => {
            if let Some(k) = a.get(1).and_then(|s| Kind::from_slug(s)) {
                print!("{}", boilerplate(k));
            } else {
                eprintln!("unknown kind; try: list");
                std::process::exit(2);
            }
        }
        Some("list") => {
            for k in ALL { println!("{:<10} {}", k.slug(), k.label()); }
        }
        _ => eprintln!("flux-boilerplate — UI app boilerplates\n  detect \"<prompt>\" | get <slug> | list"),
    }
}
