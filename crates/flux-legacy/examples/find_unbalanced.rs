//! Diagnostic: which items does the splitter mis-bound (non-zero code-brace balance)?
//!   flux-cargo-wrapper run -p flux-legacy --example find_unbalanced -- <file.rs>
use flux_legacy::split::{code_brace_balance, parse_items};

fn main() {
    let path = std::env::args().nth(1).expect("usage: find_unbalanced <file.rs>");
    let src = std::fs::read_to_string(&path).unwrap();
    let (_h, items) = parse_items(&src);
    let mut bad = 0;
    for it in &items {
        let bal = code_brace_balance(&it.src);
        if bal != 0 {
            bad += 1;
            let lines: Vec<&str> = it.src.lines().collect();
            let first = lines.first().copied().unwrap_or("");
            println!("── {} (bal {bal:+}, {} loc, kind {:?}) ──", it.name, it.loc, it.kind);
            println!("   first: {}", first.trim());
            for t in lines.iter().rev().take(3).rev() {
                println!("   tail : {}", t.trim());
            }
        }
    }
    println!("\n{bad} mis-bounded item(s) of {}", items.len());
}
