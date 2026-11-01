//! quillon_referee_songs — Quillon soundtrack 245, 246 and 247, computed from the supply referee, plus a
//! short arXiv-style paper that shows every derivation.
//!
//! The referee is the newest instrument on Quillon (October 2026): for a span of blocks it compares how much a
//! node's wallet total grew with how much the block headers say was minted. Its answer is a RATIO. A musical
//! interval is also a ratio of two frequencies, so the referee can be heard: 1 is a unison, 2 is an octave, and
//! two tones a few cents apart beat against each other at a rate you can count. The three songs are tuned to one
//! reference, A4 = 440 Hz (the referee's "tuning fork"), and each one sings one measurement:
//!
//!   245 The Second Harmonic — the Stockholm follower paid almost every block twice: ratio ≈ 2, an octave.
//!   246 Same Height         — Stockholm and Epsilon hold the same blocks, but not yet the same purse.
//!   247 Meri                — Epsilon's own journal ran a few tens of cents flat over eight days; cause unknown.
//!
//! House rules (same as le_grand_k_song.rs / quillonium_137.rs): every number is computed at generation time from
//! the input files; every claim carries an epistemic label (exact / measured / dated / convention / choice /
//! historical); the "what is still pretend" section says what the songs do NOT carry. The lyrics are written only
//! if the measurements still support every line they sing — otherwise the program refuses and writes nothing.
//!
//! Usage: quillon_referee_songs <inputs.json> <ping.txt> <arxiv-selected.json> [out_dir]
//! inputs.json comes from the read-only collector (collect.py beside the data); ping.txt is `ping -q` output.
use flux_arxiv_latex::doc::{Block, Document};
use flux_arxiv_latex::{latex_escape, parse_arxiv_json};
use flux_science::constants::SPEED_OF_LIGHT;
use serde_json::Value;

/// Concert pitch A4 = 440 Hz (ISO 16) — a CONVENTION. It is the reference every interval is measured against.
const FORK_HZ: f64 = 440.0;
/// QUG has 24 decimals: 1 QUG = 10^24 base units (design constant of the chain).
const QUG: f64 = 1e24;
/// Suno's limits for the style box and the lyrics box (characters).
const STYLE_MAX: usize = 1000;
const LYRICS_MAX: usize = 3000;
/// A journal step "agrees" when its ratio is within this relative band (the 10-31 referee's own band).
const AGREE_BAND: f64 = 0.005;
/// Typical group index of silica fibre — a TYPICAL MATERIAL VALUE, not measured on this path.
const FIBRE_INDEX: f64 = 1.468;

fn cents(r: f64) -> f64 {
    1200.0 * r.log2()
}

fn big(v: &Value) -> u128 {
    match v {
        Value::String(s) => s.parse().unwrap_or_else(|_| panic!("not an integer: {s}")),
        Value::Number(n) => n.as_u64().expect("integer") as u128,
        _ => panic!("expected an integer, got {v}"),
    }
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or_else(|| panic!("expected a number, got {v}"))
}

/// 0..=999 in words ("a hundred and four").
fn words(n: u64) -> String {
    const ONES: [&str; 20] = [
        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve",
        "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen", "nineteen",
    ];
    const TENS: [&str; 10] = ["", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety"];
    assert!(n < 1000);
    let below_100 = |m: u64| -> String {
        if m < 20 {
            ONES[m as usize].to_string()
        } else if m % 10 == 0 {
            TENS[(m / 10) as usize].to_string()
        } else {
            format!("{}-{}", TENS[(m / 10) as usize], ONES[(m % 10) as usize])
        }
    };
    match (n / 100, n % 100) {
        (0, r) => below_100(r),
        (1, 0) => "a hundred".into(),
        (1, r) => format!("a hundred and {}", below_100(r)),
        (h, 0) => format!("{} hundred", ONES[h as usize]),
        (h, r) => format!("{} hundred and {}", ONES[h as usize], below_100(r)),
    }
}

/// 0..1e9 in words, sung style: "three hundred and seven thousand, seven hundred and forty-one".
fn words_big(n: u64) -> String {
    assert!(n < 1_000_000_000);
    let (m, t, u) = (n / 1_000_000, (n / 1000) % 1000, n % 1000);
    let mut parts = Vec::new();
    if m > 0 {
        parts.push(format!("{} million", words(m)));
    }
    if t > 0 {
        parts.push(format!("{} thousand", words(t)));
    }
    if u > 0 {
        let w = words(u);
        parts.push(if (m > 0 || t > 0) && u < 100 { format!("and {w}") } else { w });
    }
    if parts.is_empty() {
        return "zero".into();
    }
    parts.join(", ").replace(", and", " and")
}

/// A ratio read out digit by digit to four decimals: 1.9870 → "one point nine eight seven zero".
fn spoken_ratio(r: f64) -> String {
    const W: [&str; 10] = ["zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine"];
    let s = format!("{r:.4}");
    s.chars()
        .map(|c| if c == '.' { "point".to_string() } else { W[c.to_digit(10).unwrap() as usize].to_string() })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A rounded count for a lyric, honest about the rounding: 7.67 → "nearly eight", 8.3 → "just over eight".
fn about(x: f64) -> String {
    let r = x.round();
    let w = words(r as u64);
    if (x - r).abs() < 0.15 { w } else if x < r { format!("nearly {w}") } else { format!("just over {w}") }
}

/// Capitalise the first letter of every sung line (a placeholder may start a line in lowercase).
fn capitalise_lines(t: &str) -> String {
    t.lines()
        .map(|l| {
            let mut c = l.chars();
            match c.next() {
                Some(f) if f.is_ascii_lowercase() => f.to_ascii_uppercase().to_string() + c.as_str(),
                _ => l.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Thousands separators for prose/LaTeX: 307741 → 307,741 (LaTeX gets {,}).
fn grouped(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push_str("{,}");
        }
        out.push(ch);
    }
    out
}

/// Replace @@KEY@@ placeholders; refuse if any is left unfilled.
fn fill(template: &str, kv: &[(&str, String)]) -> String {
    let mut s = template.to_string();
    for (k, v) in kv {
        s = s.replace(&format!("@@{k}@@"), v);
    }
    if let Some(i) = s.find("@@") {
        panic!("unfilled placeholder: {}", s[i..].chars().take(40).collect::<String>());
    }
    s
}

/// One 10,000-block journal step from the 10-31 referee file, with its time span and longest pause.
struct Step {
    h0: u64,
    grew: f64,
    minted: f64,
    span: String,
    pause_s: u64,
    pause_at: String,
}

fn parse_steps(path: &str) -> Vec<Step> {
    let text = std::fs::read_to_string(path).expect("journal steps file");
    let mut out = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 20 || !f[0].contains("->") {
            continue;
        }
        let h0: u64 = f[0].split("->").next().unwrap().parse().unwrap();
        let get = |key: &str, off: usize| -> &str { f[f.iter().position(|w| *w == key).unwrap() + off] };
        let span_i = f.iter().position(|w| *w == "span").unwrap();
        let span = f[span_i + 1..span_i + 8].join(" ");
        let (pause_s, pause_at) = match f.iter().position(|w| *w == "pause") {
            Some(i) => (f[i + 1].parse().unwrap(), f[i + 4..i + 7].join(" ")),
            None => (0, String::new()),
        };
        out.push(Step {
            h0,
            grew: get("grew", 1).parse().unwrap(),
            minted: get("minted", 1).parse().unwrap(),
            span,
            pause_s,
            pause_at,
        });
    }
    out
}

/// A census file says "missing 0" and lists NO non-coinbase transaction lines.
fn census_is_empty(path: &str) -> bool {
    let t = std::fs::read_to_string(path).expect("census file");
    let lines: Vec<&str> = t.lines().filter(|l| !l.trim().is_empty()).collect();
    lines.len() == 2 && lines[0].ends_with("missing 0") && lines[1].starts_with("non-coinbase txs FROM the zero address")
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// "Sat 2026-10-31 02:58:56 CET" → unix seconds (CET = UTC+1; refuses any other zone).
fn systemd_cet_to_unix(t: &str) -> i64 {
    let f: Vec<&str> = t.split_whitespace().collect();
    assert_eq!(f[3], "CET", "only CET timestamps are handled");
    let d: Vec<i64> = f[1].split('-').map(|x| x.parse().unwrap()).collect();
    let c: Vec<i64> = f[2].split(':').map(|x| x.parse().unwrap()).collect();
    days_from_civil(d[0], d[1], d[2]) * 86_400 + c[0] * 3600 + c[1] * 60 + c[2] - 3600
}

fn blake3_file(path: &str) -> String {
    hex::encode(blake3::hash(&std::fs::read(path).expect("input file")).as_bytes())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let usage = "usage: quillon_referee_songs <inputs.json> <ping.txt> <arxiv-selected.json> [out_dir]";
    let inputs_path = args.get(1).expect(usage);
    let ping_path = args.get(2).expect(usage);
    let arxiv_path = args.get(3).expect(usage);
    let out_dir = args.get(4).map(String::as_str).unwrap_or("/home/orobit/tmp/quillon-245-247-referee/out");

    // ── self-checks of the helpers before anything is trusted ──
    assert_eq!(words(104), "a hundred and four");
    assert_eq!(words_big(307_741), "three hundred and seven thousand, seven hundred and forty-one");
    assert_eq!(words_big(4_012), "four thousand and twelve");
    assert_eq!(about(7.67), "nearly eight");
    assert_eq!(capitalise_lines("a b\n[X]\nok"), "A b\n[X]\nOk");
    assert_eq!(spoken_ratio(1.987), "one point nine eight seven zero");
    assert!((cents(2.0) - 1200.0).abs() < 1e-9 && cents(1.0) == 0.0);

    let inp: Value = serde_json::from_str(&std::fs::read_to_string(inputs_path).expect("inputs.json")).expect("json");
    let arch = &inp["archives"];
    let mut read_files: Vec<(String, String)> = vec![
        (inputs_path.clone(), blake3_file(inputs_path)),
        (ping_path.clone(), blake3_file(ping_path)),
        (arxiv_path.clone(), blake3_file(arxiv_path)),
    ];

    // ═══ 1. the double payment (archived, 10-30 evening, Stockholm) ═══
    let sto_pre = &arch["ab_before_restart_1030"]["rows"]["stockholm"];
    let sto_post = &arch["ab_after_restart_1030"]["rows"]["stockholm"];
    let r_pre = num(&sto_pre["grew_qug"]) / num(&sto_pre["minted_qug"]);
    let r_dbl = num(&sto_post["grew_qug"]) / num(&sto_post["minted_qug"]);
    let dbl_blocks = sto_post["blocks"].as_u64().unwrap();
    let saves = &arch["saves_per_height_1030"]["raw"];
    let (saved_twice, heights) = (saves["saved_twice"].as_u64().unwrap(), saves["heights_in_interval"].as_u64().unwrap());
    let ctl_mult: Vec<String> = arch["test_control_1031"]["multiples"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
    let ctl_res = &arch["test_control_1031"]["result"];
    let fx_res = &arch["test_fixed_1031"]["result"];
    let c_dbl = cents(r_dbl);
    let oct_dev = c_dbl - 1200.0;
    // the follower's voice at FORK·r against the fork's own octave 2·FORK
    let beat_245 = (2.0 * FORK_HZ - FORK_HZ * r_dbl).abs();
    let bpm_245 = (beat_245 * 60.0 / 4.0).round() as u64; // sixteenth notes tick at the beat rate

    // ═══ 2. the fresh window (today), both nodes, exact base units ═══
    let ab = &inp["ab_window"];
    let wait_s = ab["wait_s"].as_u64().unwrap();
    let side = |name: &str| -> (u64, u64, u128, u128, usize) {
        let s = &ab[name];
        let (a, b) = (s["from"].as_u64().unwrap(), s["to"].as_u64().unwrap());
        (a, b, big(&s["grew_base"]), big(&s["headers_minted_base"]), s["missing"].as_array().unwrap().len())
    };
    let (sa, sb, s_grew, s_mint, s_full) = side("stockholm");
    let (ea, eb, e_grew, e_mint, e_full) = side("epsilon");
    let s_exact = s_full == 0 && s_grew == s_mint;
    let e_exact = e_full == 0 && e_grew == e_mint;
    let mint_per_block = s_mint as f64 / QUG / (sb - sa) as f64;
    let fresh_blocks = sb - sa;

    // ═══ 3. two books, one road: identity + history + distance ═══
    let ident = inp["identity_sample"].as_array().unwrap();
    let ident_same = ident
        .iter()
        .filter(|x| x["epsilon_digest"] == x["stockholm_digest"] && x["epsilon_next_prev"] == x["stockholm_next_prev"] && !x["stockholm_digest"].is_null())
        .count();
    let (id_lo, id_hi) = (ident.first().unwrap()["height"].as_u64().unwrap(), ident.last().unwrap()["height"].as_u64().unwrap());
    let hs = &arch["holescan_1030"];
    let (scanned, scan_s, scan_missing) = (hs["scanned"].as_u64().unwrap(), hs["seconds"].as_u64().unwrap(), hs["missing"].as_u64().unwrap());
    let (scan_from, scan_to) = (hs["from"].as_u64().unwrap(), hs["to"].as_u64().unwrap());
    let scan_rate = scanned as f64 / scan_s as f64;
    let bpm_246 = scan_rate.round() as u64;
    let ping = std::fs::read_to_string(ping_path).expect("ping");
    let rtt_line = ping.lines().find(|l| l.contains("min/avg/max")).expect("ping summary");
    let rtt: Vec<f64> = rtt_line.split('=').nth(1).unwrap().trim().trim_end_matches(" ms").split('/').map(|x| x.parse().unwrap()).collect();
    let loss_ok = ping.contains(" 0% packet loss");
    let rtt_min_s = rtt[0] / 1000.0;
    let light_km = SPEED_OF_LIGHT * rtt_min_s / 2.0 / 1000.0;
    let fibre_km = light_km / FIBRE_INDEX;
    let ec = &inp["epsilon_compare"];
    let sc = &inp["stockholm_compare"];
    let (e_sup, s_sup) = (big(&ec["total_supply_base_units"]), big(&sc["total_supply_base_units"]));
    let (e_h, s_h) = (ec["height"].as_u64().unwrap(), sc["height"].as_u64().unwrap());
    let r_purse = s_sup as f64 / e_sup as f64;
    let c_purse = cents(r_purse);
    let gap_qug = (e_sup as f64 - s_sup as f64) / QUG;
    let gap_k = (gap_qug / 1000.0).round() as u64;
    let roots_differ = ec["balance_root_hex"] != sc["balance_root_hex"];

    // ═══ 4. the first book breathes: Epsilon's journal, 22–30 Oct (archived) + the two new steps ═══
    // The 10-31 referee wrote its 19 steps into THREE files (all-steps, run1, last-steps); read all, keep one copy of
    // each step (a repeated step must agree exactly), and refuse unless they form one unbroken chain.
    let steps_dir = std::path::Path::new(arch["journal_steps_1031"]["path"].as_str().unwrap()).parent().unwrap().to_path_buf();
    let mut steps: Vec<Step> = Vec::new();
    for name in ["all-steps.txt", "run1.txt", "last-steps.txt"] {
        let path = steps_dir.join(name).to_string_lossy().to_string();
        for st in parse_steps(&path) {
            match steps.iter().find(|x| x.h0 == st.h0) {
                Some(x) => assert!(x.grew == st.grew && x.minted == st.minted, "step {} appears twice with different numbers", st.h0),
                None => steps.push(st),
            }
        }
        read_files.push((path.clone(), blake3_file(&path)));
    }
    steps.sort_by_key(|x| x.h0);
    assert!(steps.windows(2).all(|w| w[1].h0 == w[0].h0 + 10_000), "the journal steps have a gap — a step file is missing");
    let n_steps = steps.len();
    let (sum_g, sum_m): (f64, f64) = steps.iter().fold((0.0, 0.0), |(g, m), s| (g + s.grew, m + s.minted));
    let r_period = sum_g / sum_m;
    let c_period = cents(r_period);
    let agree = steps.iter().filter(|s| (s.grew / s.minted - 1.0).abs() <= AGREE_BAND).count();
    let inhale = steps.iter().max_by(|a, b| (a.grew - a.minted).total_cmp(&(b.grew - b.minted))).unwrap();
    let exhale = steps.iter().min_by(|a, b| (a.grew - a.minted).total_cmp(&(b.grew - b.minted))).unwrap();
    let (in_q, out_q) = (inhale.grew - inhale.minted, exhale.grew - exhale.minted);
    let c_in = cents(inhale.grew / inhale.minted);
    let census_in = format!("/home/storage/qnk-114h-logs/referee/census-{}.txt", inhale.h0);
    let census_out = format!("/home/storage/qnk-114h-logs/referee/census-{}.txt", exhale.h0);
    let (cin_ok, cout_ok) = (census_is_empty(&census_in), census_is_empty(&census_out));
    read_files.push((census_in.clone(), blake3_file(&census_in)));
    read_files.push((census_out.clone(), blake3_file(&census_out)));
    // the production process start time (systemd, CET) must fall inside the exhale step's longest pause (UTC)
    let enter = inp["receipt_before"]["q_api_server"]["active_enter"].as_str().unwrap().to_string();
    let enter_hm_cet = enter.split_whitespace().nth(2).unwrap()[..5].to_string(); // "05:44"
    let cet_h: u32 = enter_hm_cet[..2].parse().unwrap();
    let enter_hm_utc = format!("{:02}:{}", (cet_h + 23) % 24, &enter_hm_cet[3..]); // CET = UTC+1 in late October
    let day = |mmdd: &str| -> i64 { mmdd[3..5].parse().unwrap() };
    let days = day(&steps.last().unwrap().span[19..24]) - day(&steps[0].span[..5]);
    let restart_in_exhale = exhale.pause_s >= 60 && exhale.pause_at.ends_with(&format!("{enter_hm_utc} UTC")) && enter.contains("2026-10-29");
    let f_ledger = FORK_HZ * r_period;
    let beat_247 = FORK_HZ - f_ledger;
    let bpm_247 = (beat_247 * 60.0 / 4.0).round() as u64;
    let fresh = inp["fresh_steps"].as_array().unwrap();
    let fr = |i: usize, who: &str| -> (u64, u64, f64, bool) {
        let s = &fresh[i];
        let (g, m) = (big(&s[format!("{who}_grew_base")]), big(&s["headers_minted_base"]));
        (s["h0"].as_u64().unwrap(), s["h1"].as_u64().unwrap(), g as f64 / m as f64, g == m && s["missing_on_stockholm"].as_array().unwrap().is_empty())
    };
    let (f1a, f1b, f1_e, f1_e_exact) = fr(0, "epsilon");
    let (_, _, f1_s, f1_s_exact) = fr(0, "stockholm");
    let (f2a, f2b, f2_e, f2_e_exact) = fr(1, "epsilon");
    let (_, _, f2_s, f2_s_exact) = fr(1, "stockholm");
    let fresh_e_agree = (f1_e - 1.0).abs() <= AGREE_BAND && (f2_e - 1.0).abs() <= AGREE_BAND;
    let fresh_e_exact = f1_e_exact && f2_e_exact;
    let short_of = |i: usize, who: &str| -> f64 {
        let s = &fresh[i];
        (big(&s["headers_minted_base"]) as f64 - big(&s[format!("{who}_grew_base")]) as f64) / QUG
    };
    let (sto_short1, sto_short2) = (short_of(0, "stockholm"), short_of(1, "stockholm"));
    let (eps_extra1, eps_extra2) = (-short_of(0, "epsilon"), -short_of(1, "epsilon"));
    let sto_short = sto_short1 + sto_short2;
    let sto_restart = inp["stockholm_unit"][1].as_str().unwrap().to_string();
    let sto_restart_unix = systemd_cet_to_unix(&sto_restart);
    let step1_span = (fresh[0]["ts_min"].as_i64().unwrap(), fresh[0]["ts_max"].as_i64().unwrap());
    let sto_restart_in_step1 = step1_span.0 <= sto_restart_unix && sto_restart_unix <= step1_span.1;
    let receipt_same = inp["receipt_before"] == inp["receipt_after"];

    // ═══ refuse to sing what the measurements do not say ═══
    assert!(receipt_same, "q-api-server moved during the collection — report that first, write no songs");
    assert!((oct_dev).abs() < 25.0, "the double payment is no longer within 25 cents of an octave — rewrite 245");
    assert!(oct_dev < 0.0, "245 sings 'a little flat' of the octave");
    assert!(saved_twice + 1 == heights, "245 sings '{saved_twice} of {heights}' as 'all but one'");
    assert!(ctl_mult.iter().any(|m| m == "2.00"), "245 sings that the old code paid two point zero zero");
    assert_eq!(ctl_res[0], "FAILED", "the unfixed control must fail");
    assert_eq!(fx_res[0], "ok", "the fixed tests must pass");
    assert_eq!(fx_res[2], "0", "the fixed tests must have 0 failures");
    assert!(s_exact, "245 sings 'to the last of twenty-four decimals' about TODAY's Stockholm window — it was not exact");
    assert!(fresh_blocks >= 50, "today's window is too short to sing ({fresh_blocks} blocks)");
    assert!((60..=140).contains(&bpm_245), "245's tempo {bpm_245} out of range");
    assert_eq!(ident_same, ident.len(), "246 sings 'every answer came back the same' — {ident_same} of {}", ident.len());
    assert_eq!(scan_missing, 0, "246 sings 'not one was missing'");
    assert!(loss_ok, "246 sings the round trip; the ping lost packets");
    assert!(roots_differ && r_purse < 1.0, "246 sings that Stockholm's purse is lower");
    assert!((100..=160).contains(&bpm_246), "246's tempo {bpm_246} out of range");
    assert!(c_period < -5.0 && c_period > -99.0, "247 sings the ledger flat by {c_period:.1} cents");
    assert!(in_q > 1000.0 && out_q < -1000.0, "247 sings a breath in and a breath out");
    assert!(cin_ok && cout_ok, "247 sings 'no payment in the blocks' — a census file disagrees");
    assert!(restart_in_exhale, "247 sings that the breath went out across the restart");
    assert!((90..=140).contains(&bpm_247), "247's tempo {bpm_247} out of range");
    assert!(inhale.span.starts_with("10-27") && inhale.span.contains("10-28"), "247 sings the inhale between the 27th and the 28th");
    assert!(steps.iter().all(|s| s.span.starts_with("10-")) && (5..=12).contains(&days), "247 sings {days} days");
    assert_eq!(scan_from, 24_100_000, "246 sings 'from twenty-four point one million'");
    assert!(mint_per_block < 1.0, "the paper says a block mints well under one QUG");
    assert!(e_exact, "the paper says Epsilon's window today is exact once every block is read");
    assert!(sto_short.round() < 1.0 || sto_restart_in_step1, "the paper ties Stockholm's fresh shortfall to its restart; the restart is not inside step 1");

    // ═══ the Suno texts ═══
    let date_words = "the first of November";
    let fresh_e_line = format!(
        "Two more steps of ten thousand: {} and {}\n{}",
        spoken_ratio(f1_e),
        spoken_ratio(f2_e),
        if fresh_e_exact { "To the last coin, this time" } else if fresh_e_agree { "Close, but not to the last coin" } else { "Still not the note it should be" }
    );
    let sto_bridge = if sto_short.round() >= 1.0 {
        format!("At the tip it pays right now, to the last coin\nBut the last two steps still came up short\n{} coins that never arrived", words_big(sto_short.round() as u64))
    } else {
        "Now it pays right, the old mistake stays in the purse".to_string()
    };
    let kv = [
        ("BPM245", bpm_245.to_string()),
        ("BPM246", bpm_246.to_string()),
        ("BPM247", bpm_247.to_string()),
        ("TWICE", words(saved_twice)),
        ("HEIGHTS", words(heights)),
        ("RDBL", spoken_ratio(r_dbl)),
        ("OCTDEV", words(oct_dev.abs().round() as u64)),
        ("CTL", "two point zero zero".into()),
        ("DATE", date_words.into()),
        ("FRESHN", words_big(fresh_blocks)),
        ("FRESHMIN", words(wait_s / 60)),
        ("SCANNED", words_big(scanned)),
        ("NIDENT", words(ident.len() as u64)),
        ("RTTMS", words(rtt[0].round() as u64)),
        ("LIGHTKM", words_big((light_km / 100.0).round() as u64 * 100)),
        ("GAPK", words(gap_k)),
        ("CPERIOD", words(c_period.abs().round() as u64)),
        ("NSTEPS", words(n_steps as u64)),
        ("NAGREE", words(agree as u64)),
        ("BEAT", about(beat_247)),
        ("INQ", words_big(in_q.round() as u64)),
        ("OUTQ", words_big((-out_q).round() as u64)),
        ("DAYS", words(days as u64)),
        ("FRESHE", fresh_e_line),
        ("STOBRIDGE", sto_bridge),
    ];
    let songs: [(u32, &str, &str, &str, &str); 3] = [
        (245, "The Second Harmonic", "THE_SECOND_HARMONIC", STYLE_245, LYRICS_245),
        (246, "Same Height", "SAME_HEIGHT", STYLE_246, LYRICS_246),
        (247, "Meri", "MERI", STYLE_247, LYRICS_247),
    ];
    let mut rendered = Vec::new();
    for (n, title, slug, st, ly) in songs {
        let (style, lyrics) = (fill(st, &kv), capitalise_lines(&fill(ly, &kv)));
        let (sn, ln) = (style.chars().count(), lyrics.chars().count());
        // Every check before the first write: a refused run leaves nothing behind.
        assert!(sn <= STYLE_MAX, "{n} style is {sn} chars, Suno takes {STYLE_MAX}");
        assert!(ln <= LYRICS_MAX, "{n} lyrics are {ln} chars, Suno takes {LYRICS_MAX}");
        rendered.push((n, title, slug, style, lyrics, sn, ln));
    }

    // ═══ related work, from real arXiv records (each gloss written from that paper's abstract) ═══
    let arxiv_raw = std::fs::read_to_string(arxiv_path).expect("arxiv json");
    let papers = parse_arxiv_json(&arxiv_raw).expect("arxiv parse");
    let glosses: Vec<Value> = serde_json::from_str::<Vec<Value>>(&arxiv_raw).unwrap();
    let mut related = String::from("\\section{Related work}\n");
    let mut bib = String::new();
    for (p, g) in papers.iter().zip(glosses.iter()) {
        let key = p.cite_key();
        related.push_str(&format!("{} \\cite{{{key}}}\n\n", latex_escape(g["gloss"].as_str().expect("gloss"))));
        let authors = if p.authors.len() > 3 { format!("{} et al.", p.authors[..3].join(", ")) } else { p.authors.join(", ") };
        bib.push_str(&format!(
            "\\bibitem{{{key}}} {}, \\emph{{{}}}, arXiv:{} ({}).\n",
            latex_escape(&authors),
            latex_escape(&p.title),
            latex_escape(p.id.split('v').next().unwrap()),
            &p.published[..4.min(p.published.len())]
        ));
    }

    // ═══ the paper ═══
    let step_rows: String = steps
        .iter()
        .map(|s| {
            let r = s.grew / s.minted;
            let mark = if (r - 1.0).abs() <= AGREE_BAND { "" } else { "$\\leftarrow$" };
            format!(
                "{} & {:.2} & {:.2} & {:+.2} & {:.4} & {:+.0} & {} {mark}\\\\\n",
                grouped(s.h0 / 1000) + "k",
                s.grew,
                s.minted,
                s.grew - s.minted,
                r,
                cents(r).max(-9999.0),
                latex_escape(&s.span.replace(" .. ", "--"))
            )
        })
        .collect();
    let fresh_rows = format!(
        "{}k--{}k & Epsilon & {:.6} & {} \\\\\n{}k--{}k & Stockholm & {:.6} & {} \\\\\n{}k--{}k & Epsilon & {:.6} & {} \\\\\n{}k--{}k & Stockholm & {:.6} & {} \\\\\n",
        grouped(f1a / 1000), grouped(f1b / 1000), f1_e, if f1_e_exact { "exact" } else { "no" },
        grouped(f1a / 1000), grouped(f1b / 1000), f1_s, if f1_s_exact { "exact" } else { "no" },
        grouped(f2a / 1000), grouped(f2b / 1000), f2_e, if f2_e_exact { "exact" } else { "no" },
        grouped(f2a / 1000), grouped(f2b / 1000), f2_s, if f2_s_exact { "exact" } else { "no" },
    );
    let files_tex: String = read_files
        .iter()
        .map(|(p, h)| format!("\\texttt{{{}}}\\\\\\texttt{{{}}}\\\\[2pt]\n", latex_escape(p), h))
        .collect();
    let lyric_tex = |l: &str| -> String {
        l.lines()
            .map(|x| if x.trim().is_empty() { "\\par\\medskip\\noindent\n".to_string() } else { format!("{}\\\\{{}}\n", latex_escape(x).replace('ō', "\\={o}")) })
            .collect()
    };
    let appendix: String = rendered
        .iter()
        .map(|(n, title, _, style, lyrics, sn, ln)| {
            format!(
                "\\subsection*{{{n} {title} (style {sn} / lyrics {ln} characters)}}\n{{\\small\\noindent\\textbf{{Style.}} {}\\par\\medskip\\noindent\n{}}}\n",
                latex_escape(style),
                lyric_tex(lyrics)
            )
        })
        .collect();

    let pv = [
        ("FORK", format!("{FORK_HZ:.0}")),
        ("RPRE", format!("{r_pre:.4}")),
        ("CPRE", format!("{:.1}", cents(r_pre))),
        ("RDBL", format!("{r_dbl:.4}")),
        ("DBLB", dbl_blocks.to_string()),
        ("CDBL", format!("{c_dbl:.1}")),
        ("OCTDEV", format!("{oct_dev:+.1}")),
        ("TWICE", saved_twice.to_string()),
        ("HEIGHTS", heights.to_string()),
        ("CTLM", ctl_mult.join(", ")),
        ("CTLR", format!("{} passed, {} failed", ctl_res[1].as_str().unwrap(), ctl_res[2].as_str().unwrap())),
        ("FXR", format!("{} passed, {} failed", fx_res[1].as_str().unwrap(), fx_res[2].as_str().unwrap())),
        ("BEAT245", format!("{beat_245:.2}")),
        ("BPM245", bpm_245.to_string()),
        ("WAIT", wait_s.to_string()),
        ("SA", grouped(sa)),
        ("SB", grouped(sb)),
        ("SBLK", fresh_blocks.to_string()),
        ("SGREW", grouped(s_grew as u64 / 1_000_000) + "\\times10^{6}"),
        ("SGREWQ", format!("{:.6}", s_grew as f64 / QUG)),
        ("SEXACT", if s_exact { "equal to the base unit".into() } else { "NOT equal".to_string() }),
        ("EA", grouped(ea)),
        ("EB", grouped(eb)),
        ("EBLK", (eb - ea).to_string()),
        ("EGREWQ", format!("{:.6}", e_grew as f64 / QUG)),
        ("EMINTQ", format!("{:.6}", e_mint as f64 / QUG)),
        ("EEXACT", if e_exact { "equal to the base unit".into() } else if e_full > 0 { format!("not checked (Stockholm lacked {e_full} of these blocks when they were summed)") } else { format!("not equal (ratio {:.6})", e_grew as f64 / e_mint as f64) }),
        ("RPREB", sto_pre["blocks"].as_u64().unwrap().to_string()),
        ("MPB", format!("{mint_per_block:.3}")),
        ("NIDENT", ident.len().to_string()),
        ("IDSAME", ident_same.to_string()),
        ("IDLO", grouped(id_lo)),
        ("IDHI", grouped(id_hi)),
        ("SCANNED", grouped(scanned)),
        ("SCANFROM", grouped(scan_from)),
        ("SCANTO", grouped(scan_to)),
        ("SCANS", grouped(scan_s)),
        ("SCANRATE", format!("{scan_rate:.1}")),
        ("BPM246", bpm_246.to_string()),
        ("RTTMIN", format!("{:.3}", rtt[0])),
        ("RTTAVG", format!("{:.3}", rtt[1])),
        ("LIGHTKM", format!("{:.0}", light_km)),
        ("FIBREKM", format!("{:.0}", fibre_km)),
        ("FIBREN", format!("{FIBRE_INDEX}")),
        ("EH", grouped(e_h)),
        ("SH", grouped(s_h)),
        ("ESUP", format!("{:.3}", e_sup as f64 / QUG)),
        ("SSUP", format!("{:.3}", s_sup as f64 / QUG)),
        ("RPURSE", format!("{r_purse:.5}")),
        ("CPURSE", format!("{c_purse:.1}")),
        ("CPURSEABS", format!("{:.1}", c_purse.abs())),
        ("GAP", format!("{gap_qug:.0}")),
        ("NSTEPS", n_steps.to_string()),
        ("STEPROWS", step_rows),
        ("SUMG", format!("{sum_g:.2}")),
        ("SUMM", format!("{sum_m:.2}")),
        ("RPERIOD", format!("{r_period:.4}")),
        ("CPERIOD", format!("{c_period:.1}")),
        ("NAGREE", agree.to_string()),
        ("BAND", format!("{:.1}", AGREE_BAND * 100.0)),
        ("INQ", format!("{in_q:+.2}")),
        ("INH", grouped(inhale.h0)),
        ("CIN", format!("{c_in:+.0}")),
        ("OUTQ", format!("{out_q:+.2}")),
        ("OUTH", grouped(exhale.h0)),
        ("PAUSE", exhale.pause_s.to_string()),
        ("PAUSEAT", latex_escape(&exhale.pause_at)),
        ("ENTER", latex_escape(&enter)),
        ("FLEDGER", format!("{f_ledger:.2}")),
        ("BEAT247", format!("{beat_247:.2}")),
        ("BPM247", bpm_247.to_string()),
        ("FRESHROWS", fresh_rows),
        ("FRESHVERDICT", if fresh_e_exact { "both exact".into() } else if fresh_e_agree { format!("both inside the $\\pm${:.1}\\,\\% band but neither is exact: they grew {eps_extra1:+.2} and {eps_extra2:+.2}\\,QUG more than they minted", AGREE_BAND * 100.0) } else { "not both inside the band".to_string() }),
        ("STOS1", format!("{sto_short1:.2}")),
        ("STOS2", format!("{sto_short2:.2}")),
        ("STOS", format!("{sto_short:.0}")),
        ("STORESTART", latex_escape(&sto_restart)),
        ("STORIN", if sto_restart_in_step1 { "lies inside the first of these steps".into() } else { "does not lie inside them".to_string() }),
        ("PID", latex_escape(inp["receipt_before"]["q_api_server"]["main_pid"].as_str().unwrap())),
        ("T0", latex_escape(inp["collected_utc_start"].as_str().unwrap())),
        ("T1", latex_escape(inp["collected_utc_end"].as_str().unwrap())),
        ("FILES", files_tex),
        ("RELATED", related),
    ];
    let body = fill(PAPER_BODY, &pv);
    let doc = Document::new("article")
        .option("11pt")
        .option("a4paper")
        .package_opt("inputenc", &["utf8"])
        .package_opt("fontenc", &["T1"])
        .package("lmodern")
        .package("amsmath")
        .package("amssymb")
        .package("booktabs")
        .package_opt("geometry", &["margin=1.05in"])
        .package_opt("hyperref", &["hidelinks"])
        .preamble(
            "\\title{Hearing the referee: two Quillon ledgers as musical intervals, and three songs computed from them}\n\
\\author{Viktor Kristensen and Rocky\\\\\\small Quillon Graph soundtrack, songs 245--247 \\quad generated by \\texttt{flux-arxiv-latex} (\\texttt{quillon\\_referee\\_songs})}\n\
\\date{1 November 2026}\n",
        )
        .add(Block::Raw("\\maketitle\n".into()))
        .add(Block::Raw(body))
        .add(Block::Raw(format!("\\begin{{thebibliography}}{{99}}\n{bib}\\bibitem{{iso16}} ISO 16:1975, \\emph{{Acoustics --- Standard tuning frequency (Standard musical pitch)}}.\n\\end{{thebibliography}}\n")))
        .add(Block::Raw(format!("\\appendix\n\\section{{The three songs, as generated}}\n{appendix}")));

    std::fs::create_dir_all(out_dir).expect("out dir");
    let res = doc.compile_pdf(out_dir, "quillon-referee-songs-245-247");
    if !res.success {
        let tail: String = res.log.lines().rev().take(40).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
        eprintln!("PDF FAILED\n{tail}");
        std::process::exit(1);
    }
    for (n, _, slug, style, lyrics, _, _) in &rendered {
        std::fs::write(format!("{out_dir}/QUILLON_{n}_{slug}_STYLE.txt"), format!("{style}\n")).expect("style");
        std::fs::write(format!("{out_dir}/QUILLON_{n}_{slug}_LYRICS.txt"), format!("{lyrics}\n")).expect("lyrics");
    }
    let params = serde_json::json!({
        "fork_hz": FORK_HZ,
        "s245": {"ratio": r_dbl, "cents": c_dbl, "octave_dev_cents": oct_dev, "beat_hz": beat_245, "bpm": bpm_245,
                 "saved_twice": saved_twice, "heights": heights, "fresh_window_blocks": fresh_blocks, "fresh_exact": s_exact},
        "s246": {"identity_same": ident_same, "identity_n": ident.len(), "scanned": scanned, "scan_missing": scan_missing,
                 "scan_rate": scan_rate, "bpm": bpm_246, "rtt_min_ms": rtt[0], "light_km": light_km,
                 "purse_ratio": r_purse, "purse_cents": c_purse, "gap_qug": gap_qug},
        "s247": {"steps": n_steps, "agree": agree, "ratio": r_period, "cents": c_period, "ledger_hz": f_ledger,
                 "beat_hz": beat_247, "bpm": bpm_247, "inhale_qug": in_q, "exhale_qug": out_q,
                 "fresh": [[f1a, f1b, f1_e, f1_s], [f2a, f2b, f2_e, f2_s]], "fresh_epsilon_agree": fresh_e_agree,
                 "epsilon_window_exact": e_exact},
        "chars": rendered.iter().map(|r| serde_json::json!({"song": r.0, "style": r.5, "lyrics": r.6})).collect::<Vec<_>>(),
    });
    std::fs::write(format!("{out_dir}/quillon-referee-songs-params.json"), serde_json::to_string_pretty(&params).unwrap()).expect("params");
    println!("OK {}", res.pdf_path.unwrap());
    println!("{}", serde_json::to_string(&params).unwrap());
}

// ───────────────────────────── the songs ─────────────────────────────

const STYLE_245: &str = "Tuvan throat singing meets dark hypnotic steppe techno, A minor pentatonic over a low drone on A, @@BPM245@@ BPM, a galloping horse rhythm on frame drum and a soft kick. One male singer holds a deep drone and whistles a high overtone melody above it from the same breath, then growls a rumbling undertone. A horsehead fiddle and a plucked two-string lute carry the riff; a fretless bass slides between drone and overtone. A pure sine tuning fork on A rings under everything. In the verses a second voice doubles the singer one octave higher, a little flat, so the two voices beat slowly against each other; the sixteenth-note hi-hats tick at the speed of that wobble. Field recordings: steppe wind, a horse breathing, a server fan. The final chorus collapses to one voice in unison, the doubling gone, the beating stops. Ancient, cold, hypnotic, then one clean note.";

const LYRICS_245: &str = "[Intro: steppe wind, a horse breathing, a tuning fork struck once on A, a low drone begins]

[Spoken, an old man, low]
One breath. Listen. Two notes.

[Verse 1]
In Kyzyl my grandfather sang to the horses
One breath, one throat, and two notes came out
The low one was the earth, the high one was the wind
He said a good singer makes one thing sound twice
In a song that is a gift
In a song that is a gift

[Verse 2]
Far in the north a second node woke up in Stockholm
To keep a copy of the chain and check the first
It took every block the network gave it
And paid the miners of nearly every block twice
@@TWICE@@ of @@HEIGHTS@@
Written down twice in its own book

[Chorus]
One breath, two notes, that's a song
One block, two coins, that's wrong
An octave sounds right
That's why nobody heard it
The referee doesn't listen
The referee counts

[Instrumental: fiddle and fretless bass trade the riff, the octave voice beats against the drone]

[Verse 3]
It added what the headers minted
And what the wallets grew, side by side
@@RDBL@@, it read that night
An octave, @@OCTDEV@@ cents shy of two
Two doors into the same house
Two hands on the same payment
Only one of them asked if the block was paid

[Bridge: drums drop out, only the drone, the fork and the overtone]
Paid once, whoever gets there first
The mark and the coin in the same stroke
The old code paid @@CTL@@
The new code pays one

[Build: the gallop returns, the octave voice fades one note at a time]

[Verse 4]
On @@DATE@@ we counted again
@@FRESHN@@ blocks in @@FRESHMIN@@ minutes
What the headers minted, the wallets grew
To the last of twenty-four decimals

[Final Chorus, one voice, unison]
One breath, one note, that's the chain
One block, one coin, the same
Sing two notes if you can
But pay once
The referee doesn't listen
The referee counts

[Outro: the fork rings alone on A, the horse breathes, the wind stops]

[Spoken, the old man, softly]
One breath. One note. Good.";

const STYLE_246: &str = "Swedish folk polska in 3/4 meets Nordic deep house, A minor, @@BPM246@@ BPM. A woman's kulning herding call, high and piercing, thrown across a lake; a second distant voice answers with the same call from the far shore. Keyed fiddle with sympathetic resonance strings is the lead melody, its drone strings ringing in sympathy; a fretless bass sings below it. Hand-clapped polska pulse, a soft four-on-the-floor kick only in the drops, warm Rhodes, a deep rolling sub-bass. A pure sine tuning fork on A at the start. In the bridge a low drone sits slightly under the key, a little out of tune, as if it came from another village. Field recordings: cowbells in a summer pasture, lake water, wind in birch trees, a ferry horn. Bright, spacious, longing, then warm.";

const LYRICS_246: &str = "[Intro: cowbells, lake water, a tuning fork on A, then a woman's high herding call across the water]

[Spoken, a woman, Swedish-accented English]
Kom, kom, kom.

[Verse 1]
My grandmother called the cows home in Dalarna
A voice so high it crossed the lake
She never saw them in the forest
But when the bells came back, she knew
Call it out, and listen
What comes back is what is there

[Verse 2]
Now there's a second book in Stockholm
A copy of the chain, far from the first
It read every height from twenty-four point one million
@@SCANNED@@
And not one of them was missing

[Chorus]
Call the height across the water
Hear the same height coming back
Same block, same link, same height
Two books, one road
Kom, kom, kom

[Instrumental: the fiddle's sympathetic strings ring, the far voice answers the call]

[Verse 3]
On @@DATE@@ I asked them both
@@NIDENT@@ times, at heights from end to end
Every answer came back the same
Header for header, link for link
The answer takes @@RTTMS@@ thousandths of a second
Even light could not go there and back
From further than @@LIGHTKM@@ kilometres

[Bridge: the drums drop out, a low drone a little under the key]
But the purse sounds lower in Stockholm
@@GAPK@@ thousand coins short
The strings that ring in sympathy
Ring for every block
But not for that
Something went wrong while it was catching up
@@STOBRIDGE@@
We'll build the purse again from the blocks

[Final Chorus]
Call the height across the water
Hear the same height coming back
Same block, same link, same height
Two books, one road
One purse, not yet

[Outro: two herding calls on the same note, one from each shore, then cowbells and the lake]

[Spoken, softly]
Kom, kom. Hem.";

const STYLE_247: &str = "Japanese bamboo flute meets slow dub techno, A minor in an old Japanese pentatonic, @@BPM247@@ BPM. A solo shakuhachi is the lead voice: long breathy tones, deep meri bends that pull notes below pitch, air noise as part of the sound. A pure sine tuning fork on A rings throughout; against it an old piano is tuned slightly flat, so the two beat in a slow wobble, and the sixteenth-note hi-hats tick at exactly the speed of that wobble. Deep dub chords with long tape echo, a warm sub-bass, a soft rimshot, a fretless bass in the middle register answering the flute. Field recordings: temple bell, rain on a wooden roof, a hard drive spinning down and up again. A man's quiet spoken voice reads numbers. Meditative, uneasy, patient, with no resolution.";

const LYRICS_247: &str = "[Intro: rain on a wooden roof, a temple bell, a tuning fork on A, then the flute enters a little flat]

[Spoken, a man, quiet]
Ichion jōbutsu. One note, and you are awake.

[Verse 1]
The monks in the basket hats played for breath, not for songs
The note bends down when you lower your chin
They call it meri, the shadow of a note
A little under where it should be
And the ear knows, before the mind does

[Verse 2]
For @@DAYS@@ days I listened to the first book breathe
Every ten thousand blocks I counted
What the headers minted, and what the wallets grew
@@NAGREE@@ times of @@NSTEPS@@ they agreed
And over all of it, the book ran low

[Chorus]
@@CPERIOD@@ cents flat
The ledger plays meri
Against the fork it wobbles @@BEAT@@ times a second
The ear can hear it
The ear can't name it
(Breathe in, breathe out)

[Instrumental: the flute bends down, the flat piano beats against the fork]

[Verse 3]
Between the twenty-seventh and the twenty-eighth
It breathed in @@INQ@@ coins
No payment in the blocks, no hand on the chain
Then the machine went to sleep and woke again
And @@OUTQ@@ breathed out
Nobody wrote it in, nobody wrote it out

[Bridge: drums drop out, only the flute, the fork and the rain]
Maybe a pool breathed it in
Maybe a writer with no name
The referee can hear it
The referee can't say who
So write the root in every header
So a node that disagrees
Stops instead of drifting

[Verse 4]
On @@DATE@@ I counted again
@@FRESHE@@

[Final Chorus]
@@CPERIOD@@ cents flat
The ledger plays meri
Against the fork it wobbles @@BEAT@@ times a second
The ear can hear it
Until it's written in the header
I listen

[Outro: the flute holds one long bent note, the fork rings, the piano stops, a drive spins down]

[Spoken, quiet]
One note. Still listening.";

// ───────────────────────────── the paper ─────────────────────────────

const PAPER_BODY: &str = r#"\begin{abstract}
In October 2026 Quillon Graph got its first independent second node (Stockholm) and a new check, the \emph{supply referee}: over a span of blocks it divides how much a node's wallet total grew by how much the block headers say was minted. An honest node reads exactly 1. A ratio is also what a musical interval is, so the referee can be listened to: 1 is a unison, 2 is an octave, and two tones a few cents apart beat at a countable rate. We collect the referee's measurements from both nodes --- archived ones from 30--31 October and fresh ones taken today --- and compute from them three songs for the Quillon soundtrack, all tuned to one reference, A4 = @@FORK@@\,Hz. Song 245 sings the follower that paid almost every block twice (ratio @@RDBL@@, an octave @@OCTDEV@@ cents off). Song 246 sings two nodes that hold the same blocks but not yet the same purse. Song 247 sings Epsilon's own journal, @@CPERIOD@@ cents flat over eight days, cause still unknown. Every number is computed by the generator from the input files listed in Section~\ref{sec:repro}; every claim carries one of six labels: \emph{exact}, \emph{measured}, \emph{dated} (a measurement that belongs to a moment, not to the design), \emph{convention}, \emph{historical}, \emph{choice}. The generator refuses to write the lyrics if any measurement stops supporting a line.
\end{abstract}

\section{What the referee measures}
A Quillon block header carries \texttt{total\_coinbase\_reward}: the number of base units the block mints (1 QUG = $10^{24}$ base units, \emph{exact}, a design constant). Each node also keeps its own wallet balances. The referee reads the total of all wallet balances at two heights $H_0 < H_1$, and sums the headers' minted amounts over the blocks in between:
\[ r \;=\; \frac{S(H_1) - S(H_0)}{\sum_{h=H_0+1}^{H_1} C(h)} . \]
If the only thing that changes balances in those blocks is minting, an honest node reads $r = 1$, and in base units the two integers are equal, not merely close. Transfers move coins between wallets and do not change $S$. A node that pays a block twice reads $r \approx 2$; a node whose balances move outside the blocks reads something else, and the referee cannot say what moved them --- it only says that the blocks do not explain it. That limit is the subject of song 247.

The headers are read from \emph{Stockholm's} API, so production answers only a handful of calls. Using one set of headers for both nodes is valid only if both hold the same blocks; Section~\ref{sec:same} checks that.

\section{Why a ratio is an interval}
Two tones whose frequencies stand in the ratio $r$ are $1200\log_2 r$ cents apart (a cent is one hundredth of an equal-tempered semitone, \emph{convention}). The octave is $r = 2$ exactly, 1200 cents; the unison is $r = 1$, 0 cents. Two tones at $f_1$ and $f_2$ close together are heard as one tone whose loudness rises and falls $|f_1 - f_2|$ times a second (\emph{beats}, elementary acoustics). This gives the three songs one shared method: a reference tone at A4 = @@FORK@@\,Hz (\emph{convention}, ISO~16 \cite{iso16}) stands for ``what the headers minted'', and a second voice at $@@FORK@@\cdot r$ stands for ``what the wallets grew''.

The surprising consequence, and the reason a referee is needed at all: \textbf{a payment made exactly twice is an octave, and an octave sounds right.} Doubling is the most consonant interval there is. A listener checking for harmony would hear nothing wrong. Only counting finds it.

\section{Two books, one road}\label{sec:same}
\paragraph{Same blocks (measured, today).} At @@NIDENT@@ heights spread evenly from @@IDLO@@ to @@IDHI@@ we read the block header from both nodes and compared a SHA-256 digest of the whole header, and the next block's \texttt{prev\_block\_hash} (the chain's own link). @@IDSAME@@ of @@NIDENT@@ were identical on both counts.
\paragraph{No holes (measured, 30 October, dated).} A scan of Stockholm's database found @@SCANNED@@ heights, @@SCANFROM@@--@@SCANTO@@, with 0 missing, in @@SCANS@@\,s (@@SCANRATE@@ heights per second).
\paragraph{Distance (measured, today; bound exact).} The fastest of 20 pings from Epsilon to Stockholm took @@RTTMIN@@\,ms (average @@RTTAVG@@\,ms). Light in vacuum ($c$ exact, CODATA) covers at most $c \cdot t/2 = @@LIGHTKM@@$\,km in half that time --- a hard upper bound on the one-way path. In silica fibre (group index $\approx @@FIBREN@@$, a \emph{typical} material value, not measured here) the bound is about @@FIBREKM@@\,km.
\paragraph{Not the same purse (measured, today).} Epsilon reports @@ESUP@@\,QUG at height @@EH@@; Stockholm @@SSUP@@\,QUG at height @@SH@@ (a few blocks apart; a block mints about @@MPB@@\,QUG, so the difference in height changes nothing below). Ratio @@RPURSE@@, that is @@CPURSE@@ cents: Stockholm's purse is about @@GAP@@\,QUG short, and its balance root differs from Epsilon's. The 31 October snapshot comparison (archived, not re-run) located the difference in a few wallets, frozen since Stockholm's catch-up; the rebuild of Stockholm's balances from a snapshot is planned and not yet done. The section on song 247 shows the purse still falling behind in the steps since.

\section{Song 245: the second harmonic}
\paragraph{The double payment (measured, 30 October, dated).} Before a restart, over @@RPREB@@ blocks, Stockholm's wallets grew $r = @@RPRE@@$ times what the headers minted (@@CPRE@@ cents). After the restart, over @@DBLB@@ blocks, $r = @@RDBL@@$: @@CDBL@@ cents, an octave @@OCTDEV@@ cents flat. In that interval @@TWICE@@ of @@HEIGHTS@@ heights were written to its database twice. Cause (measured 31 October, published): two code paths could each pay a block, and only one of them checked the ``already paid'' mark. This is the failure mode the replication literature warns about: state-machine replication needs every replica to turn the same input into the same state \cite{arxiv1603_07351v2}, and here the same block gave different balances depending on which path delivered it.
\paragraph{The fix (measured).} One rule for both paths: a block is paid once, whoever gets there first, with the mark and the payment written together. The unfixed control test paid @@CTLM@@ times the header (@@CTLR@@); the fixed tests: @@FXR@@.
\paragraph{Today (measured).} (The 10-30 script prints its own ratios too; its Epsilon figure, 1.0141 today and 1.0760 on 10-30, sums before Stockholm has stored the newest block. The sums here were taken once every block was present.) Over @@WAIT@@\,s, heights @@SA@@--@@SB@@ (@@SBLK@@ blocks), Stockholm's supply grew @@SGREWQ@@\,QUG; compared with what the headers minted, it was @@SEXACT@@. Epsilon over the same wait, heights @@EA@@--@@EB@@ (@@EBLK@@ blocks): grew @@EGREWQ@@ vs minted @@EMINTQ@@, @@EEXACT@@.
\paragraph{The music (computed; mapping is choice).} Reference A4 = @@FORK@@\,Hz; the follower's voice at $@@FORK@@ \times @@RDBL@@$ beats against the reference's octave at $|880 - 440r| = @@BEAT245@@$\,Hz. The hi-hats play sixteenth notes at that rate: $@@BEAT245@@ \times 60/4 \approx @@BPM245@@$\,BPM. The genre is Tuvan \emph{sygyt}, in which one singer holds a low drone and amplifies a single high harmonic from the same breath \cite{arxiv2606_04943v1}: one breath, two notes, the musical picture of one block, two payments. The last chorus is sung in unison: the measured fix.

\section{Song 246: same height}
The music (choice): a Swedish \emph{kulning} herding call, thrown across water and answered from the far shore --- a request whose answer tells you what is there. Tempo: the hole scan's @@SCANRATE@@ heights per second, rounded, @@BPM246@@\,BPM (choice of mapping; the rate is dated). The lead is a keyed fiddle whose sympathetic strings ring only when a played note matches their tuning; the lyric uses that physics for the two nodes: they ring together for every block, not for the purse. The bridge drone sits @@CPURSEABS@@ cents under the key (the paper's number; a music model will round it).

\section{Song 247: meri}
\begin{table}[h]\centering\scriptsize
\begin{tabular}{lrrrrrl}\toprule
from height & grew (QUG) & minted (QUG) & difference & ratio & cents & span (UTC)\\\midrule
@@STEPROWS@@\bottomrule\end{tabular}
\caption{Epsilon's balance journal against its headers, @@NSTEPS@@ steps of 10,000 blocks, 22--30 October (measured 31 October, dated). Arrows mark steps outside $\pm$@@BAND@@\,\%.}
\end{table}
\paragraph{The first book breathes (measured, dated).} Over the @@NSTEPS@@ steps Epsilon's wallets grew @@SUMG@@\,QUG while its headers minted @@SUMM@@: $r = @@RPERIOD@@$, @@CPERIOD@@ cents. @@NAGREE@@ of @@NSTEPS@@ steps agree within $\pm$@@BAND@@\,\%. The step from height @@INH@@ grew @@INQ@@\,QUG more than it minted ($@@CIN@@$ cents); the step from @@OUTH@@ grew @@OUTQ@@\,QUG \emph{less}. A census of both steps found no non-coinbase transactions at all. The second step's longest gap between block timestamps is @@PAUSE@@\,s at @@PAUSEAT@@, and the production process now running started at @@ENTER@@ --- the same minute: the coins went out across a restart. What wrote them in and what took them out is \textbf{not known}. Candidates include balance changes made outside blocks (for example trading-pool deposits) and batch writers; none is proven.
\paragraph{Since then (measured, today).} The two journal steps closed since, read against one set of headers:
\begin{center}\small\begin{tabular}{llrl}\toprule steps & node & ratio & equal in base units\\\midrule
@@FRESHROWS@@\bottomrule\end{tabular}\end{center}
Epsilon's two fresh steps are @@FRESHVERDICT@@ --- a small, positive change that today's single 10-minute window did not happen to catch.

\textbf{New finding (measured today, not root-caused).} Stockholm's two fresh steps read below 1: its wallets grew @@STOS1@@ and @@STOS2@@\,QUG \emph{less} than its own headers minted (@@STOS@@\,QUG in all), although its 10-minute window at the tip is exact. Its process restarted at @@STORESTART@@, which @@STORIN@@. After a restart a follower catches up on the blocks it missed, and catch-up is where Stockholm's October shortfall appeared. So the double-payment fix holds at the tip, and a separate fault that under-credits during catch-up is the leading suspect; it is not proven.
\paragraph{The music (computed; mapping is choice).} The ledger's voice at $@@FORK@@ \times @@RPERIOD@@ = @@FLEDGER@@$\,Hz against the fork beats $@@BEAT247@@$\,Hz; sixteenth notes at that rate give @@BPM247@@\,BPM. The genre is the shakuhachi, whose players bend a note below pitch by lowering the chin (\emph{meri}): a note a little under where it should be, which the ear catches before it can say why. The song ends unresolved, because the measurement is. Its bridge sings the planned remedy, published by the project on 31 October: a state root in every header, so that a node that disagrees stops instead of drifting.

@@RELATED@@

\section{What is still pretend}
\begin{itemize}
\item (\emph{choice}) Every mapping from measurement to music: the A4 reference, ratio $\mapsto$ interval, beat rate $\mapsto$ hi-hat rate, scan rate $\mapsto$ tempo, the genres and the stories. The chain fixes the numbers; it does not ask for songs.
\item (\emph{not measured}) Whether a music model keeps the tempo, the octave doubling, the flat piano or the unison ending has not been checked; nobody has heard these songs yet. It plays in equal temperament: the cents are rounded away.
\item (\emph{dated}) Ratios, block counts, the round trip and the scan rate belong to the moments they were measured. The design facts the songs sing as timeless are only these: a header says what its block minted; a block is paid once; anyone can add the two up.
\item (\emph{unknown}) The cause of Epsilon's breath in and out (song 247). The cause of Stockholm's catch-up shortfall beyond what is stated (song 246).
\item (\emph{historical}) The grandfather in Kyzyl, the grandmother in Dalarna and the monks are characters, not people we met.
\end{itemize}

\section{Reproduce}\label{sec:repro}
Data: \texttt{collect.py} (read-only; collection @@T0@@ to @@T1@@; q-api-server MainPID @@PID@@ unchanged before and after). Generator, built and run with:
\begin{flushleft}\footnotesize\texttt{fluxc build -p flux-arxiv-latex --bin quillon\_referee\_songs}\\
\texttt{quillon\_referee\_songs inputs.json ping.txt arxiv-selected.json [out\_dir]}\end{flushleft}
It refuses to write anything if the octave drifts more than 25 cents, if today's Stockholm window is not equal to the base unit, if any identity sample differs, if the scan found a hole, if the purse is not lower, if the period is not flat, if a census lists a transaction, if the restart is not inside the exhale step, or if a text exceeds Suno's limits. Input files (BLAKE3):\\[4pt]
{\footnotesize
@@FILES@@}
"#;
