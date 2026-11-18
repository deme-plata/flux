//! flux-roadmap — the PRINCE2 board for a roadmap, kept in one JSON file (Viktor 2026-11-15: "go for prince2").
//! Library: `flux_legacy::control::prince2`. Board file: `--board P`, else $FLUX_ROADMAP_BOARD, else ./ROADMAP-PRINCE2.json.

use flux_legacy::control::prince2::{self, Board, Decision, Forecast, Item, ItemStatus, StageReport};
use std::process::exit;

const HELP: &str = "flux-roadmap — PRINCE2 Agile board (stages · tolerances · exceptions · justification · done-checks)

  init <project> <executive>                 new board (default tolerances: 8 h, 12 GB free, 20 files, money and production ask the board)
  show                                       the highlight report
  stage <name> <goal…>                       open a stage
  add <id> <stage> --benefit B [--title T] [--check CMD] [--on DATE]
                                             add an item (refused without a benefit)
  check <id> --hours H [--mem-gb G] [--files N] [--money Q] [--production]
                                             forecast vs tolerances; a breach raises an exception (exit 3)
  decide <seq> continue|replan|drop <note…>  RECORD THE BOARD'S DECISION — the operator's word; an agent never runs this on its own
  done <id>                                  mark an item done
  rejustify <id> [benefit…]                  the benefit was checked again today
  end-stage <stage>                          close a stage at its gate (refused while an exception is undecided)
  lesson <text…>                             add to the lessons log
  import-overview <overview.md> <stage> <as-of-date>
                                             put an old roadmap table (| **N** | Step | What | Why | Size |) in front of the board

  --board P    board file   --today DATE  override today's date (YYYY-MM-DD)";

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    // Howard Hinnant's civil_from_days, the inverse of prince2::days_from_ymd
    let z = z + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn system_today() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// Pull `--name value` (or a bare `--name` switch) out of args.
fn take(args: &mut Vec<String>, name: &str, switch: bool) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.remove(i);
    if switch {
        return Some(String::new());
    }
    if i < args.len() {
        Some(args.remove(i))
    } else {
        die(2, &format!("{name} needs a value"))
    }
}

fn die(code: i32, msg: &str) -> ! {
    eprintln!("{msg}");
    exit(code)
}

fn num<T: std::str::FromStr>(v: Option<String>, name: &str, default: T) -> T {
    match v {
        None => default,
        Some(s) => s.parse().unwrap_or_else(|_| die(2, &format!("{name}: not a number: {s}"))),
    }
}

fn print_report(r: &StageReport) {
    println!("stage {} closed", r.stage);
    for (label, v) in [("done", &r.done), ("carried to the next stage", &r.carried), ("dropped", &r.dropped), ("benefit stale", &r.stale_justification), ("no done-check", &r.without_done_check)] {
        println!("  {label}: {}", if v.is_empty() { "—".to_string() } else { v.join(", ") });
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{HELP}");
        return;
    }
    let path = take(&mut args, "--board", false)
        .or_else(|| std::env::var("FLUX_ROADMAP_BOARD").ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "ROADMAP-PRINCE2.json".into());
    let today = take(&mut args, "--today", false).unwrap_or_else(system_today);
    if prince2::days_from_ymd(&today).is_none() {
        die(2, &format!("--today {today:?} is not YYYY-MM-DD"));
    }
    let cmd = args.remove(0);
    let load = || Board::load(&path).unwrap_or_else(|e| die(1, &format!("cannot read board {path}: {e} (run `flux-roadmap init` first)")));
    let save = |b: &Board| b.save(&path).unwrap_or_else(|e| die(1, &format!("cannot write board {path}: {e}")));
    let ok = |r: Result<(), String>| r.unwrap_or_else(|e| die(1, &e));

    match cmd.as_str() {
        "init" => {
            if std::path::Path::new(&path).exists() {
                die(1, &format!("{path} already exists; refusing to overwrite a board"));
            }
            let (project, exec) = match args.as_slice() {
                [p, e] => (p.clone(), e.clone()),
                _ => die(2, "usage: init <project> <executive>"),
            };
            let b = Board::new(project, exec);
            save(&b);
            println!("board {path} created");
        }
        "show" => print!("{}", load().highlight(&today)),
        "stage" => {
            if args.len() < 2 {
                die(2, "usage: stage <name> <goal…>");
            }
            let mut b = load();
            ok(b.add_stage(&args[0], &args[1..].join(" ")));
            save(&b);
            println!("stage {} opened", args[0]);
        }
        "add" => {
            let benefit = take(&mut args, "--benefit", false).unwrap_or_default();
            let title = take(&mut args, "--title", false);
            let check = take(&mut args, "--check", false).unwrap_or_default();
            let on = take(&mut args, "--on", false).unwrap_or_else(|| today.clone());
            let (id, stage) = match args.as_slice() {
                [i, s] => (i.clone(), s.clone()),
                _ => die(2, "usage: add <id> <stage> --benefit B [--title T] [--check CMD] [--on DATE]"),
            };
            let mut b = load();
            ok(b.add_item(Item {
                title: title.unwrap_or_else(|| id.clone()),
                id: id.clone(),
                stage,
                benefit,
                justified_on: on,
                done_check: check,
                status: ItemStatus::Planned,
                tolerances: None,
                notes: vec![],
            }));
            save(&b);
            println!("item {id} added");
        }
        "check" => {
            let hours = num(take(&mut args, "--hours", false), "--hours", 0.0f64);
            let mem = take(&mut args, "--mem-gb", false).map(|s| num(Some(s), "--mem-gb", 0.0f64));
            let files = num(take(&mut args, "--files", false), "--files", 0u32);
            let money = num(take(&mut args, "--money", false), "--money", 0.0f64);
            let production = take(&mut args, "--production", true).is_some();
            let id = args.first().cloned().unwrap_or_else(|| die(2, "usage: check <id> --hours H …"));
            let mut b = load();
            let f = Forecast { hours, min_free_mem_gb: mem, files, money_qug: money, touches_production: production };
            let found = b.check(&id, f, &today).unwrap_or_else(|e| die(1, &e));
            save(&b);
            if found.is_empty() {
                println!("{id}: inside tolerance — carry on");
            } else {
                let seq = b.exceptions.last().map(|e| e.seq).unwrap_or(0);
                println!("{id}: EXCEPTION #{seq} — waits for the board ({})", b.executive);
                for x in &found {
                    println!("  {}: forecast {} vs tolerance {}", x.tolerance, x.forecast, x.limit);
                }
                exit(3);
            }
        }
        "decide" => {
            if args.len() < 2 {
                die(2, "usage: decide <seq> continue|replan|drop <note…>");
            }
            let seq: u64 = args[0].parse().unwrap_or_else(|_| die(2, "seq must be a number"));
            let d = match args[1].as_str() {
                "continue" => Decision::Continue,
                "replan" => Decision::Replan,
                "drop" => Decision::Drop,
                other => die(2, &format!("decision must be continue|replan|drop, not {other}")),
            };
            let mut b = load();
            ok(b.decide(seq, d, &args[2..].join(" "), &today));
            save(&b);
            println!("exception #{seq}: {d:?} recorded");
        }
        "done" => {
            let id = args.first().cloned().unwrap_or_else(|| die(2, "usage: done <id>"));
            let mut b = load();
            ok(b.mark_done(&id, &today));
            save(&b);
            println!("{id} done");
        }
        "rejustify" => {
            let id = args.first().cloned().unwrap_or_else(|| die(2, "usage: rejustify <id> [benefit…]"));
            let benefit = if args.len() > 1 { Some(args[1..].join(" ")) } else { None };
            let mut b = load();
            ok(b.rejustify(&id, &today, benefit.as_deref()));
            save(&b);
            println!("{id} re-justified {today}");
        }
        "end-stage" => {
            let st = args.first().cloned().unwrap_or_else(|| die(2, "usage: end-stage <stage>"));
            let mut b = load();
            let r = b.end_stage(&st, &today).unwrap_or_else(|e| die(1, &e));
            save(&b);
            print_report(&r);
        }
        "lesson" => {
            if args.is_empty() {
                die(2, "usage: lesson <text…>");
            }
            let mut b = load();
            b.add_lesson(&today, &args.join(" "));
            save(&b);
            println!("lesson recorded");
        }
        "import-overview" => {
            let (md, stage, asof) = match args.as_slice() {
                [m, s, a] => (m.clone(), s.clone(), a.clone()),
                _ => die(2, "usage: import-overview <overview.md> <stage> <as-of-date>"),
            };
            let text = std::fs::read_to_string(&md).unwrap_or_else(|e| die(1, &format!("{md}: {e}")));
            let rows = prince2::parse_overview_rows(&text);
            if rows.is_empty() {
                die(1, &format!("{md}: no `| **N** | Step | What | Why | Size |` rows found"));
            }
            let mut b = load();
            let mut added = 0;
            for r in rows {
                let id = format!("{stage}-{}", r.number);
                if b.items.iter().any(|i| i.id == id) {
                    continue;
                }
                ok(b.add_item(Item {
                    id,
                    title: r.step,
                    stage: stage.clone(),
                    benefit: r.why,
                    justified_on: asof.clone(),
                    done_check: String::new(),
                    status: if r.done { ItemStatus::Done } else { ItemStatus::Planned },
                    tolerances: None,
                    notes: vec![format!("imported from {md}")],
                }));
                added += 1;
            }
            save(&b);
            println!("{added} item(s) imported into stage {stage} (benefit dated {asof}; re-check each with `rejustify`)");
        }
        other => die(2, &format!("unknown command {other}\n\n{HELP}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn civil_round_trips_with_days_from_ymd() {
        for s in ["1970-01-01", "2000-02-29", "2026-11-15", "2026-12-31", "2100-03-01"] {
            let z = prince2::days_from_ymd(s).unwrap();
            let (y, m, d) = civil_from_days(z);
            assert_eq!(format!("{y:04}-{m:02}-{d:02}"), s);
        }
    }
}
