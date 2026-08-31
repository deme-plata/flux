//! flux-cm CLI — operate the community desk from the shell.
//!
//! The library is the ledger; this binary is the desk clerk. Every command
//! loads the desk file, applies one action, and saves it back atomically.
//!
//!     flux-cm --desk /path/desk.json onboard --handle pedre --name "Pedre" \
//!         --chains quillon,sigil --skills shilling,twitter --langs english --hours 10
//!     flux-cm --desk /path/desk.json post --title "..." --chain sigil \
//!         --bounty 25 --currency sigil --hours 4 --deadline-days 7
//!     flux-cm --desk /path/desk.json assign wk-xxxx wisdom
//!     flux-cm --desk /path/desk.json report

use flux_cm::{Chain, CmStatus, CommunityDesk, Currency, Timeline, WorkSpec};
use std::path::PathBuf;
use std::process::exit;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let desk_path = take_flag(&mut args, "--desk")
        .or_else(|| std::env::var("FLUX_CM_DESK").ok())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("flux-cm-desk.json"));

    if args.is_empty() {
        usage();
        exit(2);
    }
    let cmd = args.remove(0);
    let mut desk = if desk_path.exists() {
        match CommunityDesk::load(&desk_path) {
            Ok(d) => d,
            Err(e) => die(&format!("cannot load desk {}: {e}", desk_path.display())),
        }
    } else {
        CommunityDesk::default()
    };

    let mutated = match run(&cmd, &mut args, &mut desk) {
        Ok(m) => m,
        Err(e) => die(&e),
    };
    if mutated {
        if let Err(e) = desk.save(&desk_path) {
            die(&format!("cannot save desk {}: {e}", desk_path.display()));
        }
    }
}

/// Returns whether the desk was mutated (and must be saved).
fn run(cmd: &str, args: &mut Vec<String>, desk: &mut CommunityDesk) -> Result<bool, String> {
    match cmd {
        "onboard" => {
            let handle = req_flag(args, "--handle")?;
            let name = take_flag(args, "--name").unwrap_or_else(|| handle.clone());
            let chains = parse_chains(&req_flag(args, "--chains")?)?;
            let skills = csv(take_flag(args, "--skills"));
            let langs = csv(take_flag(args, "--langs"));
            let qnk = take_flag(args, "--qnk");
            let sigil = take_flag(args, "--sigil");
            let hours: u32 = req_flag(args, "--hours")?
                .parse()
                .map_err(|_| "--hours must be a number".to_string())?;
            let id = desk
                .onboard(&handle, &name, chains, skills, langs, qnk, sigil, hours)
                .map_err(|e| e.to_string())?;
            println!("onboarded @{handle} as {id} (Candidate)");
            Ok(true)
        }
        "activate" => {
            let id = resolve_cm(desk, &pop(args, "handle")?)?;
            desk.activate(&id).map_err(|e| e.to_string())?;
            println!("activated {id}");
            Ok(true)
        }
        "status" => {
            let id = resolve_cm(desk, &pop(args, "handle")?)?;
            let status = parse_status(&pop(args, "status")?)?;
            desk.set_status(&id, status).map_err(|e| e.to_string())?;
            println!("{id} → {status:?}");
            Ok(true)
        }
        "post" => {
            let currency = parse_currency(&req_flag(args, "--currency")?)?;
            let bounty_base = parse_amount(&req_flag(args, "--bounty")?, currency)?;
            let deadline_ts_ms = take_flag(args, "--deadline-days")
                .map(|d| {
                    d.parse::<u64>()
                        .map(|days| now_ms() + days * 86_400_000)
                        .map_err(|_| "--deadline-days must be a number".to_string())
                })
                .transpose()?;
            let spec = WorkSpec {
                title: req_flag(args, "--title")?,
                brief: take_flag(args, "--brief").unwrap_or_default(),
                chain: parse_chain(&req_flag(args, "--chain")?)?,
                skills_required: csv(take_flag(args, "--skills")),
                bounty_base,
                currency,
                hours_estimate: req_flag(args, "--hours")?
                    .parse()
                    .map_err(|_| "--hours must be a number".to_string())?,
                deadline_ts_ms,
            };
            let id = desk.post_work(spec).map_err(|e| e.to_string())?;
            println!("posted {id}");
            Ok(true)
        }
        "suggest" => {
            let wk = pop(args, "work-id")?;
            for (cm_id, score) in desk.suggest(&wk).map_err(|e| e.to_string())? {
                let handle = desk.cm(&cm_id).map(|c| c.handle.clone()).unwrap_or_default();
                println!("{score:.3}  @{handle}  ({cm_id})");
            }
            Ok(false)
        }
        "assign" => {
            let wk = pop(args, "work-id")?;
            let id = resolve_cm(desk, &pop(args, "handle")?)?;
            desk.assign(&wk, &id).map_err(|e| e.to_string())?;
            println!("{wk} assigned to {id}");
            Ok(true)
        }
        "submit" => {
            let wk = pop(args, "work-id")?;
            desk.submit(&wk).map_err(|e| e.to_string())?;
            println!("{wk} submitted");
            Ok(true)
        }
        "approve" => {
            let wk = pop(args, "work-id")?;
            desk.approve(&wk).map_err(|e| e.to_string())?;
            println!("{wk} approved");
            Ok(true)
        }
        "pay" => {
            let wk = pop(args, "work-id")?;
            let tx = pop(args, "tx-ref")?;
            desk.record_payment(&wk, &tx).map_err(|e| e.to_string())?;
            println!("{wk} paid (tx {tx})");
            Ok(true)
        }
        "cancel" => {
            let wk = pop(args, "work-id")?;
            let reason = rest(args, "reason")?;
            desk.cancel(&wk, &reason).map_err(|e| e.to_string())?;
            println!("{wk} cancelled");
            Ok(true)
        }
        "note" => {
            let id = resolve_cm(desk, &pop(args, "handle")?)?;
            let text = rest(args, "text")?;
            desk.note(&id, &text).map_err(|e| e.to_string())?;
            Ok(true)
        }
        "kudos" => {
            let id = resolve_cm(desk, &pop(args, "handle")?)?;
            let text = rest(args, "text")?;
            desk.kudos(&id, &text).map_err(|e| e.to_string())?;
            Ok(true)
        }
        "board" => {
            for w in desk.board() {
                let assignee = w
                    .assignee
                    .as_deref()
                    .and_then(|id| desk.cm(id))
                    .map(|c| format!("@{}", c.handle))
                    .unwrap_or_else(|| "—".to_string());
                println!(
                    "{}  {:?}  \"{}\"  {}  {}  est {}h  {}",
                    w.id,
                    w.status,
                    w.title,
                    w.chain.name(),
                    w.currency.display(w.bounty_base),
                    w.hours_estimate,
                    assignee
                );
            }
            Ok(false)
        }
        "timeline" => {
            let md = match args.first() {
                Some(handle) => {
                    let id = resolve_cm(desk, handle)?;
                    desk.cm_timeline_markdown(&id)
                }
                None => Timeline::render_markdown(&desk.timeline().all().iter().collect::<Vec<_>>()),
            };
            print!("{md}");
            Ok(false)
        }
        "report" => {
            print!("{}", desk.capital_report().render_markdown());
            Ok(false)
        }
        "help" | "--help" | "-h" => {
            usage();
            Ok(false)
        }
        other => Err(format!("unknown command: {other} (try `flux-cm help`)")),
    }
}

// ── Arg helpers ──────────────────────────────────────────────────────────────

fn take_flag(args: &mut Vec<String>, name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    if i + 1 >= args.len() {
        return None;
    }
    args.remove(i);
    Some(args.remove(i))
}

fn req_flag(args: &mut Vec<String>, name: &str) -> Result<String, String> {
    take_flag(args, name).ok_or_else(|| format!("missing {name} <value>"))
}

fn pop(args: &mut Vec<String>, what: &str) -> Result<String, String> {
    if args.is_empty() {
        return Err(format!("missing <{what}>"));
    }
    Ok(args.remove(0))
}

fn rest(args: &mut Vec<String>, what: &str) -> Result<String, String> {
    if args.is_empty() {
        return Err(format!("missing <{what}>"));
    }
    Ok(std::mem::take(args).join(" "))
}

fn csv(v: Option<String>) -> Vec<String> {
    v.map(|s| {
        s.split(',')
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect()
    })
    .unwrap_or_default()
}

fn resolve_cm(desk: &CommunityDesk, handle_or_id: &str) -> Result<String, String> {
    if desk.cm(handle_or_id).is_some() {
        return Ok(handle_or_id.to_string());
    }
    desk.cm_by_handle(handle_or_id)
        .map(|c| c.id.clone())
        .ok_or_else(|| format!("no CM with handle or id: {handle_or_id}"))
}

fn parse_chain(s: &str) -> Result<Chain, String> {
    match s.to_lowercase().as_str() {
        "quillon" | "qnk" | "quillon-graph" => Ok(Chain::Quillon),
        "sigil" => Ok(Chain::Sigil),
        "polygon" | "matic" => Ok(Chain::Polygon),
        other => Err(format!("unknown chain: {other} (quillon|sigil|polygon)")),
    }
}

fn parse_chains(s: &str) -> Result<Vec<Chain>, String> {
    s.split(',').map(|p| parse_chain(p.trim())).collect()
}

fn parse_currency(s: &str) -> Result<Currency, String> {
    match s.to_lowercase().as_str() {
        "qug" => Ok(Currency::Qug),
        "sigil" => Ok(Currency::Sigil),
        other => Err(format!("unknown currency: {other} (qug|sigil)")),
    }
}

fn parse_status(s: &str) -> Result<CmStatus, String> {
    match s.to_lowercase().as_str() {
        "candidate" => Ok(CmStatus::Candidate),
        "active" => Ok(CmStatus::Active),
        "onleave" | "on-leave" => Ok(CmStatus::OnLeave),
        "alumni" => Ok(CmStatus::Alumni),
        other => Err(format!("unknown status: {other} (candidate|active|onleave|alumni)")),
    }
}

/// "12.5" + SIGIL(10 dp) → 125_000_000_000 base units. Whole or decimal.
fn parse_amount(s: &str, currency: Currency) -> Result<u128, String> {
    let bad = || format!("bad amount: {s}");
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i, f),
        None => (s, ""),
    };
    let decimals = currency.decimals() as usize;
    if frac.len() > decimals {
        return Err(format!("{s} has more than {decimals} decimals for {}", currency.symbol()));
    }
    let int: u128 = int.parse().map_err(|_| bad())?;
    let frac_scaled: u128 = if frac.is_empty() {
        0
    } else {
        let padded = format!("{frac:0<decimals$}");
        padded.parse().map_err(|_| bad())?
    };
    Ok(int * 10u128.pow(currency.decimals()) + frac_scaled)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn die(msg: &str) -> ! {
    eprintln!("flux-cm: {msg}");
    exit(1)
}

fn usage() {
    println!(
        "flux-cm — community desk CLI (desk file: --desk PATH or $FLUX_CM_DESK)

  onboard  --handle H [--name N] --chains quillon,sigil,polygon [--skills a,b]
           [--langs a,b] [--qnk ADDR] [--sigil ADDR] --hours N
  activate <handle>            status <handle> <candidate|active|onleave|alumni>
  post     --title T [--brief B] --chain C [--skills a,b] --bounty AMT
           --currency qug|sigil --hours N [--deadline-days D]
  suggest  <work-id>           assign <work-id> <handle>
  submit   <work-id>           approve <work-id>
  pay      <work-id> <tx-ref>  cancel <work-id> <reason...>
  note     <handle> <text...>  kudos <handle> <text...>
  board  ·  timeline [handle]  ·  report"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amount_parsing_scales_per_currency() {
        assert_eq!(parse_amount("1", Currency::Sigil).unwrap(), 10_000_000_000);
        assert_eq!(parse_amount("12.5", Currency::Sigil).unwrap(), 125_000_000_000);
        assert_eq!(
            parse_amount("300", Currency::Qug).unwrap(),
            300 * 10u128.pow(24)
        );
        assert!(parse_amount("1.12345678901", Currency::Sigil).is_err()); // 11 dp > 10
        assert!(parse_amount("abc", Currency::Sigil).is_err());
    }

    #[test]
    fn flag_taking_removes_pairs() {
        let mut args: Vec<String> = ["--desk", "d.json", "onboard", "--hours", "5"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(take_flag(&mut args, "--desk").as_deref(), Some("d.json"));
        assert_eq!(args, ["onboard", "--hours", "5"]);
        assert_eq!(take_flag(&mut args, "--missing"), None);
    }
}
