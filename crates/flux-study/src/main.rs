//! `flux-study` — the command line.
//!
//!   flux-study init                       seed Viktor's goals (no-op if goals exist)
//!   flux-study goals                      every goal, its deadline, days left, why
//!   flux-study today [--minutes N]        the plan for today (default budget 180 min)
//!   flux-study loop [--every M] [--minutes N]   print the plan every M minutes (default 60)
//!   flux-study done <goal> <task>         mark done today
//!   flux-study add-goal "<title>" <YYYY-MM-DD> "<why>"
//!   flux-study add-task <goal> "<title>" <minutes>
//!   flux-study progress                   done/total per goal + days left
//!   flux-study credit "<module>" <points> "<evidence>"   SIGIL University ledger entry
//!   flux-study university                 the ledger and the total
//!
//! Store: $FLUX_STUDY_FILE, else /home/storage/claude-code/study/study.json, else ~/.flux/study.json.
use flux_study::{default_path, today, Study};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = run(&args) {
        eprintln!("flux-study: {e}");
        std::process::exit(1);
    }
}

fn flag(args: &[String], name: &str, default: u64) -> u64 {
    args.windows(2).find(|w| w[0] == name).and_then(|w| w[1].parse().ok()).unwrap_or(default)
}

fn run(args: &[String]) -> Result<(), String> {
    let path = default_path();
    let mut st = Study::load(&path)?;
    let day = today();
    let cmd = args.first().map(String::as_str).unwrap_or("today");
    match cmd {
        "init" => {
            let before = st.goals.len();
            st.seed_viktor();
            st.save(&path)?;
            println!("{} goal(s) in {}{}", st.goals.len(), path.display(), if before == 0 { " (seeded)" } else { " (already there — untouched)" });
        }
        "goals" => {
            for (g, done, total, left) in st.progress(&day) {
                println!("[{}] {}  ·  {}  ·  {} day(s) left  ·  {done}/{total} done", g.id, g.title, g.deadline, left);
                println!("      why: {}", g.why);
                for t in &g.tasks {
                    println!("      {} {}.{}  {}  ({} min){}", if t.done { "✓" } else { "·" }, g.id, t.id, t.title, t.minutes,
                        if t.done { format!("  done {}", t.done_on) } else { String::new() });
                }
            }
        }
        "today" => print_plan(&st, &day, flag(args, "--minutes", 180) as u32),
        "loop" => {
            let every = flag(args, "--every", 60);
            let budget = flag(args, "--minutes", 180) as u32;
            loop {
                let st = Study::load(&path)?;
                print_plan(&st, &today(), budget);
                println!("── next plan in {every} min (Ctrl-C to stop; mark work with `flux-study done <goal> <task>`) ──\n");
                std::thread::sleep(std::time::Duration::from_secs(every * 60));
            }
        }
        "done" => {
            let g: u32 = args.get(1).and_then(|s| s.parse().ok()).ok_or("usage: done <goal> <task>")?;
            let t: u32 = args.get(2).and_then(|s| s.parse().ok()).ok_or("usage: done <goal> <task>")?;
            st.done(g, t, &day)?;
            st.save(&path)?;
            println!("✓ {g}.{t} done on {day}");
        }
        "add-goal" => {
            let (t, d, w) = (args.get(1).ok_or("title")?, args.get(2).ok_or("deadline")?, args.get(3).map(String::as_str).unwrap_or(""));
            let id = st.add_goal(t, d, w)?;
            st.save(&path)?;
            println!("goal {id} added");
        }
        "add-task" => {
            let g: u32 = args.get(1).and_then(|s| s.parse().ok()).ok_or("usage: add-task <goal> \"<title>\" <minutes>")?;
            let t = args.get(2).ok_or("title")?;
            let m: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(60);
            let id = st.add_task(g, t, m)?;
            st.save(&path)?;
            println!("task {g}.{id} added");
        }
        "progress" => {
            for (g, done, total, left) in st.progress(&day) {
                let pct = if total == 0 { 0 } else { done * 100 / total };
                println!("{:>3}%  {done:>2}/{total:<2}  {:>4} d  [{}] {}", pct, left, g.id, g.title);
            }
        }
        "credit" => {
            let m = args.get(1).ok_or("module")?;
            let p: u32 = args.get(2).and_then(|s| s.parse().ok()).ok_or("points")?;
            let e = args.get(3).map(String::as_str).unwrap_or("");
            st.credit(&day, m, p, e);
            st.save(&path)?;
            println!("+{p} points · {m} · total {}", st.university_points());
        }
        "university" => {
            for c in &st.university { println!("{}  {:>3} pt  {}  — {}", c.day, c.points, c.module, c.evidence); }
            println!("total: {} points over {} entries (local ledger; the on-chain registry is sigil-university)", st.university_points(), st.university.len());
        }
        other => return Err(format!("unknown command {other:?}; see the top of main.rs")),
    }
    Ok(())
}

fn print_plan(st: &Study, day: &str, budget: u32) {
    let plan = st.plan(day, budget);
    println!("📚 {day} · budget {budget} min");
    if plan.is_empty() { println!("   nothing open — add a goal or a task."); return; }
    let mut used = 0;
    for (left, g, t) in plan {
        used += t.minutes;
        println!("   {}.{}  {:<70}  {:>4} min  · {} ({} d)", g.id, t.id, t.title, t.minutes, short(&g.title), left);
    }
    println!("   = {used} min planned. Nearest deadline first; the why is one `goals` away.");
}

fn short(s: &str) -> String { s.chars().take(38).collect::<String>() + if s.chars().count() > 38 { "…" } else { "" } }
