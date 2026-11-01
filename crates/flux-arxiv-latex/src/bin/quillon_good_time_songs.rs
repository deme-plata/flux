//! quillon_good_time_songs — Quillon soundtrack 248, 249 and 250, "A Good Time": a party night in three songs whose
//! keys and shared tempo are COMPUTED from CODATA 2022 (flux-science) and the 2019 SI, plus a short arXiv-style paper.
//!
//!   248 Red Is A-Flat      (dusk)     hydrogen's Balmer lines, R∞ computed from α, mₑ, c, h; Hα folded = A♭
//!   249 We Glow in B-Flat  (midnight) Wien's law at body temperature, b solved from h, c, k; λ_max folded = B♭
//!   250 Thirty-Two Bits    (sunrise)  the caesium second: one beat = 2^32 caesium periods = 128.4 BPM; Cs folded = C♯
//!                                     + Quillon's clock-based emission, constants READ from the Quillon source file
//!
//! All three share one tempo (a DJ set mixes at one tempo), set by the caesium clock. House rules as in
//! le_grand_k_song.rs: every number computed at generation time, every claim labelled (exact / measured / design /
//! convention / historical / choice), and the program REFUSES to write lyrics the numbers no longer support.
//!
//! Usage: quillon_good_time_songs <emission_controller.rs> [out_dir]
use flux_arxiv_latex::doc::{Block, Document};
use flux_arxiv_latex::latex_escape;
use flux_science::constants::{BOLTZMANN, ELECTRON_MASS, FINE_STRUCTURE_INV, PLANCK_H, PROTON_MASS, SPEED_OF_LIGHT};

/// SI defining constant, exact since 2019-05-20.
const CAESIUM_HFS_HZ: f64 = 9_192_631_770.0;
/// Concert pitch A4 = 440 Hz (ISO 16) — a CONVENTION; note names depend on it.
const CONCERT_A4_HZ: f64 = 440.0;
/// CODATA 2022 recommended R∞ (m⁻¹). Used ONLY to cross-check the value computed from α, mₑ, c, h.
const RYDBERG_CODATA_2022: f64 = 10_973_731.568_157;
/// CODATA 2022 Wien wavelength displacement constant b (m K). Used ONLY to cross-check the value solved here.
const WIEN_B_CODATA_2022: f64 = 2.897_771_955e-3;
/// 37.0 °C — a physiological CONVENTION for body temperature (checked for robustness over 30–40 °C).
const BODY_K: f64 = 310.15;
/// One beat = 2^COUNTER_BITS periods of the caesium clock (choice of mapping: a 32-bit counter overflowing).
const COUNTER_BITS: u32 = 32;
const STYLE_MAX: usize = 1000;
const LYRICS_MAX: usize = 3000;
const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

struct Pitch {
    octaves_down: i32,
    hz: f64,
    name: &'static str,
    octave: i32,
    cents: f64,
    midi: i32,
}

/// Lower `nu` by whole octaves into [C4, C5) and name the nearest equal-tempered note at A4 = 440 Hz.
fn pitch(nu: f64) -> Pitch {
    let c4 = CONCERT_A4_HZ * 2f64.powf(-9.0 / 12.0);
    let k = (nu / c4).log2().floor() as i32;
    let hz = nu / 2f64.powi(k);
    let semis = 12.0 * (hz / c4).log2();
    let n = semis.round();
    let midi = 60 + n as i32;
    Pitch { octaves_down: k, hz, name: NOTE_NAMES[midi.rem_euclid(12) as usize], octave: midi.div_euclid(12) - 1, cents: 100.0 * (semis - n), midi }
}

/// How a singer says a black key in a flat key: "G#" → "A-flat".
fn flat_word(name: &str) -> String {
    match name {
        "C#" => "D-flat".into(),
        "D#" => "E-flat".into(),
        "F#" => "G-flat".into(),
        "G#" => "A-flat".into(),
        "A#" => "B-flat".into(),
        n => n.to_string(),
    }
}

fn tex_flat(name: &str) -> String {
    let w = flat_word(name);
    match w.strip_suffix("-flat") {
        Some(l) => format!("{l}$\\flat$"),
        None => w,
    }
}

/// x as m\times10^{e} for LaTeX math, mantissa with `d` decimals.
fn tex_sci(x: f64, d: usize) -> String {
    let e = x.abs().log10().floor() as i32;
    format!("{:.d$}\\times10^{{{e}}}", x / 10f64.powi(e))
}

fn cents(r: f64) -> f64 {
    1200.0 * r.log2()
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

/// Any number below a trillion, read like a numbers station: "nine billion, one hundred and ninety-two million, ...".
fn words_big(n: u64) -> String {
    assert!(n < 1_000_000_000_000);
    if n < 1000 {
        return words(n);
    }
    let groups = [(n / 1_000_000_000, " billion"), ((n / 1_000_000) % 1000, " million"), ((n / 1000) % 1000, " thousand"), (n % 1000, "")];
    let mut parts: Vec<String> = Vec::new();
    for (v, unit) in groups {
        if v == 0 {
            continue;
        }
        let w = words(v).replacen("a hundred", "one hundred", 1);
        if unit.is_empty() && v < 100 && !parts.is_empty() {
            parts.push(format!("and {w}"));
        } else {
            parts.push(format!("{w}{unit}"));
        }
    }
    parts.join(", ").replace(", and", " and")
}

/// 9.343 → "nine point three four" (digits after the point read one by one).
fn spoken_decimal(x: f64, places: usize) -> String {
    let s = format!("{x:.places$}");
    let (i, f) = s.split_once('.').unwrap();
    const W: [&str; 10] = ["zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine"];
    format!("{} point {}", words(i.parse().unwrap()), f.chars().map(|c| W[c.to_digit(10).unwrap() as usize]).collect::<Vec<_>>().join(" "))
}

/// A rounded count, honest about the rounding: 3.89 → "nearly four".
fn about(x: f64) -> String {
    let r = x.round();
    let w = words(r as u64);
    if (x - r).abs() < 0.05 { w } else if x < r { format!("nearly {w}") } else { format!("just over {w}") }
}

fn ordinal(d: u32) -> String {
    const O: [&str; 32] = [
        "", "first", "second", "third", "fourth", "fifth", "sixth", "seventh", "eighth", "ninth", "tenth", "eleventh",
        "twelfth", "thirteenth", "fourteenth", "fifteenth", "sixteenth", "seventeenth", "eighteenth", "nineteenth",
        "twentieth", "twenty-first", "twenty-second", "twenty-third", "twenty-fourth", "twenty-fifth", "twenty-sixth",
        "twenty-seventh", "twenty-eighth", "twenty-ninth", "thirtieth", "thirty-first",
    ];
    O[d as usize].to_string()
}

/// Proleptic Gregorian date from days since 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The value of `pub const NAME: T = <value>;` in a Rust source file, underscores removed.
fn const_value(src: &str, name: &str) -> String {
    let line = src
        .lines()
        .find(|l| l.trim_start().starts_with(&format!("pub const {name}:")))
        .unwrap_or_else(|| panic!("{name} not found in the emission source"));
    line.split('=').nth(1).unwrap().trim().trim_end_matches(';').replace('_', "")
}

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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let src_path = args.get(1).expect("usage: quillon_good_time_songs <emission_controller.rs> [out_dir]");
    let out_dir = args.get(2).map(String::as_str).unwrap_or("/home/orobit/tmp/quillon-248-250-good-time/out");

    // ── helper self-checks ──
    let a4 = pitch(CONCERT_A4_HZ);
    assert!(a4.name == "A" && a4.octave == 4 && a4.cents.abs() < 1e-9, "note namer broken at A4");
    assert_eq!(words_big(9_192_631_770), "nine billion, one hundred and ninety-two million, six hundred and thirty-one thousand, seven hundred and seventy");
    assert_eq!(words_big(31_557_600), "thirty-one million, five hundred and fifty-seven thousand, six hundred");
    assert_eq!(spoken_decimal(9.343, 2), "nine point three four");
    assert_eq!(civil_from_days(0), (1970, 1, 1));

    let (c, h, k) = (SPEED_OF_LIGHT, PLANCK_H, BOLTZMANN);

    // ═══ 248: hydrogen ═══ R∞ = mₑ c α² / (2h), computed; reduced mass for hydrogen
    let alpha = 1.0 / FINE_STRUCTURE_INV;
    let r_inf = ELECTRON_MASS * c * alpha * alpha / (2.0 * h);
    let r_inf_rel = (r_inf - RYDBERG_CODATA_2022) / RYDBERG_CODATA_2022;
    let mu = 1.0 / (1.0 + ELECTRON_MASS / PROTON_MASS);
    let r_h = r_inf * mu;
    let balmer: Vec<(u32, f64, f64)> = (3..=6u32)
        .map(|n| {
            let term = 0.25 - 1.0 / (n * n) as f64;
            (n, c * r_h * term, 1.0 / (r_h * term)) // (upper level, frequency Hz, vacuum wavelength m)
        })
        .collect();
    let h_alpha = pitch(balmer[0].1);
    let ivals: Vec<f64> = balmer.iter().map(|b| cents(b.1 / balmer[0].1)).collect();
    let steps: Vec<i32> = ivals.iter().map(|x| (x / 100.0).round() as i32).collect();
    let chord_names: Vec<String> = steps.iter().map(|s| flat_word(NOTE_NAMES[(h_alpha.midi + s).rem_euclid(12) as usize])).collect();
    // the minor sixth is spelled F-flat in A-flat minor (the equal-tempered key is E)
    let chord_sung: Vec<String> = chord_names.iter().map(|n| if n == "E" { "F-flat".to_string() } else { n.clone() }).collect();

    // ═══ 249: a warm body ═══ Wien: x = 5(1 − e^−x), b = hc/(kx), solved here
    let mut x = 5.0f64;
    for _ in 0..60 {
        let f = x - 5.0 * (1.0 - (-x).exp());
        let df = 1.0 - 5.0 * (-x).exp();
        x -= f / df;
    }
    let wien_b = h * c / (k * x);
    let wien_rel = (wien_b - WIEN_B_CODATA_2022) / WIEN_B_CODATA_2022;
    let lam_body = wien_b / BODY_K;
    let nu_body = c / lam_body;
    let body = pitch(nu_body);
    let (mut t_lo_note, mut t_ok) = (String::new(), true);
    let mut range_cents = (f64::MAX, f64::MIN);
    for i in 0..=100 {
        let t = 303.15 + i as f64 * 0.1;
        let p = pitch(c * t / wien_b);
        if p.name != body.name {
            t_ok = false;
            t_lo_note = format!("{} at {t:.2} K", p.name);
        }
        range_cents = (range_cents.0.min(p.cents), range_cents.1.max(p.cents));
    }

    // ═══ 250: the second ═══ one beat = 2^32 caesium periods
    let beat_s = 2f64.powi(COUNTER_BITS as i32) / CAESIUM_HFS_HZ;
    let bpm_exact = 60.0 / beat_s;
    let bpm = bpm_exact.round() as u64;
    let cs = pitch(CAESIUM_HFS_HZ);

    // Quillon's emission constants, READ from the source file (design constants)
    let src = std::fs::read_to_string(src_path).expect("emission source");
    let genesis: i64 = const_value(&src, "GENESIS_TIMESTAMP").parse().unwrap();
    let halving_s: u64 = const_value(&src, "SECONDS_PER_HALVING").parse().unwrap();
    let year_s: f64 = const_value(&src, "SECONDS_PER_YEAR").parse().unwrap();
    let max_supply: u128 = const_value(&src, "QUG_MAX_SUPPLY").parse().unwrap();
    let annual_base: u128 = const_value(&src, "BASE_ANNUAL_EMISSION").parse().unwrap();
    let eras_line = src.lines().find(|l| l.contains("halving eras of")).expect("era doc line");
    let eras: u64 = eras_line.split_whitespace().find_map(|w| w.parse().ok()).expect("era count");
    let unit = max_supply / 21_000_000;
    let decimals = (unit as f64).log10().round() as u32;
    let annual_qug = (annual_base / unit) as f64;
    let years_per_era = halving_s as f64 / year_s;
    let total_years = eras as f64 * years_per_era;
    let per_second = annual_qug / year_s;
    let per_beat = per_second * beat_s;
    let ticks_per_era = halving_s as u128 * CAESIUM_HFS_HZ as u128;
    let (gy, gm, gd) = civil_from_days(genesis.div_euclid(86_400));
    let g_sec = genesis.rem_euclid(86_400);
    let month_names = ["", "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

    // ═══ refuse to sing what the numbers do not say ═══
    assert!(r_inf_rel.abs() < 2e-9, "computed R∞ disagrees with CODATA 2022 by {r_inf_rel:e}");
    assert!(wien_rel.abs() < 1e-9, "solved Wien b disagrees with CODATA 2022 by {wien_rel:e}");
    assert_eq!(h_alpha.name, "G#", "248 sings 'red is A-flat'");
    assert!(h_alpha.cents.abs() < 25.0, "H-alpha is {:.1} cents from A-flat", h_alpha.cents);
    assert_eq!(steps, vec![0, 5, 7, 8], "248 sings 'a fourth, a fifth, a minor sixth above the red'");
    assert!(ivals.iter().zip(&steps).all(|(v, s)| (v - 100.0 * *s as f64).abs() < 25.0), "a Balmer line is more than 25 cents from its note");
    assert!((balmer[3].1 / balmer[0].1 - 1.6).abs() < 1e-12, "H-delta/H-alpha must be exactly 8:5");
    assert_eq!((balmer[0].2 * 1e9).round() as u64, 656, "248 sings six hundred and fifty-six nanometres");
    assert_eq!(body.name, "A#", "249 sings 'we glow in B-flat'");
    assert!(t_ok, "249 sings 'thirty degrees or forty, still B-flat' — but {t_lo_note}");
    assert_eq!(bpm, 128, "250 and the whole set run at a hundred and twenty-eight");
    assert_eq!(cs.name, "C#", "250 sings 'C sharp, almost' (the sunrise in D-flat)");
    assert!(cs.cents < 0.0, "'almost' — caesium is flat of C#");
    assert_eq!(decimals, 24, "QUG has twenty-four decimals");
    assert_eq!(annual_qug, 2_625_000.0, "250 sings two million, six hundred and twenty-five thousand a year");
    assert_eq!(year_s, 31_557_600.0, "250 sings a Julian year of seconds");
    assert!((years_per_era - 4.0).abs() < 1e-12 && eras == 64 && (total_years - 256.0).abs() < 1e-9, "250 sings sixty-four eras, two hundred and fifty-six years");
    assert!((gy, gm, gd, g_sec) == (2026, 2, 22, 43_200), "250 sings noon on the twenty-second of February, twenty twenty-six");

    let kv = [
        ("BPM", bpm.to_string()),
        ("HA_NM", words((balmer[0].2 * 1e9).round() as u64)),
        ("HA_THZ", words((balmer[0].1 / 1e12).round() as u64)),
        ("HA_OCT", words(h_alpha.octaves_down as u64)),
        ("KEY248", flat_word(h_alpha.name)),
        ("CHORD", chord_sung.join(", ")),
        ("BODY_C", words((BODY_K - 273.15).round() as u64)),
        ("BODY_UM", spoken_decimal(lam_body * 1e6, 2)),
        ("BODY_OCT", words(body.octaves_down as u64)),
        ("KEY249", flat_word(body.name)),
        ("CS_SPOKEN", words_big(CAESIUM_HFS_HZ as u64)),
        ("CS_OCT", words(cs.octaves_down as u64)),
        ("BITS", words(COUNTER_BITS as u64)),
        ("BPM_WORDS", words(bpm)),
        ("ANNUAL", words_big(annual_qug as u64)),
        ("YEAR_S", words_big(year_s as u64)),
        ("PER_BEAT", format!("{} hundredths", about(per_beat * 100.0))),
        ("GDAY", ordinal(gd)),
        ("GMONTH", month_names[gm as usize].to_string()),
        ("GYEAR", format!("{} {}", words((gy / 100) as u64), words((gy % 100) as u64))),
        ("ERAS", words(eras)),
        ("YEARS", words(total_years as u64)),
    ];
    let songs: [(u32, &str, &str, &str, &str); 3] = [
        (248, "Red Is A-Flat", "RED_IS_A_FLAT", STYLE_248, LYRICS_248),
        (249, "We Glow in B-Flat", "WE_GLOW_IN_B_FLAT", STYLE_249, LYRICS_249),
        (250, "Thirty-Two Bits of a Second", "THIRTY_TWO_BITS_OF_A_SECOND", STYLE_250, LYRICS_250),
    ];
    let mut rendered = Vec::new();
    for (n, title, slug, st, ly) in songs {
        let (style, lyrics) = (fill(st, &kv), capitalise_lines(&fill(ly, &kv)));
        let (sn, ln) = (style.chars().count(), lyrics.chars().count());
        assert!(sn <= STYLE_MAX, "{n} style is {sn} chars, Suno takes {STYLE_MAX}");
        assert!(ln <= LYRICS_MAX, "{n} lyrics are {ln} chars, Suno takes {LYRICS_MAX}");
        rendered.push((n, title, slug, style, lyrics, sn, ln));
    }

    // ═══ the paper ═══
    let balmer_rows: String = balmer
        .iter()
        .zip(&ivals)
        .zip(&chord_names)
        .map(|((b, iv), nm)| {
            let p = pitch(b.1);
            format!(
                "H{} ($n={}\\to2$) & {:.3} & {:.4} & {:.2} & {}{} & {:+.1} & {:.1} & {} \\\\\n",
                ["$\\alpha$", "$\\beta$", "$\\gamma$", "$\\delta$"][(b.0 - 3) as usize],
                b.0,
                b.2 * 1e9,
                b.1 / 1e12,
                p.hz,
                tex_flat(p.name),
                p.octave,
                p.cents,
                iv,
                tex_flat(nm.as_str())
            )
        })
        .collect();
    let lyric_tex = |l: &str| -> String {
        l.lines()
            .map(|x| if x.trim().is_empty() { "\\par\\medskip\\noindent\n".to_string() } else { format!("{}\\\\{{}}\n", latex_escape(x)) })
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
    let src_hash = hex::encode(blake3::hash(src.as_bytes()).as_bytes());
    let pv = [
        ("AINV", format!("{FINE_STRUCTURE_INV}")),
        ("ME", tex_sci(ELECTRON_MASS, 10)),
        ("MP", tex_sci(PROTON_MASS, 11)),
        ("RINF", format!("{r_inf:.6}")),
        ("RINFC", format!("{RYDBERG_CODATA_2022:.6}")),
        ("RINFREL", format!("{r_inf_rel:.1e}")),
        ("MU", format!("{mu:.9}")),
        ("RH", format!("{r_h:.4}")),
        ("BALMER", balmer_rows),
        ("HAOCT", h_alpha.octaves_down.to_string()),
        ("HAHZ", format!("{:.3}", h_alpha.hz)),
        ("HAC", format!("{:+.2}", h_alpha.cents)),
        ("IVB", format!("{:.1}", ivals[1])),
        ("IVG", format!("{:.1}", ivals[2])),
        ("IVD", format!("{:.1}", ivals[3])),
        ("X", format!("{x:.12}")),
        ("WIENB", tex_sci(wien_b, 9)),
        ("WIENREL", format!("{wien_rel:.1e}")),
        ("TBODY", format!("{BODY_K:.2}")),
        ("LBODY", format!("{:.3}", lam_body * 1e6)),
        ("NUBODY", format!("{:.4}", nu_body / 1e12)),
        ("BODYOCT", body.octaves_down.to_string()),
        ("BODYHZ", format!("{:.2}", body.hz)),
        ("BODYC", format!("{:+.1}", body.cents)),
        ("RCLO", format!("{:+.1}", range_cents.0)),
        ("RCHI", format!("{:+.1}", range_cents.1)),
        ("BEATS", format!("{beat_s:.9}")),
        ("BPMX", format!("{bpm_exact:.4}")),
        ("BPM", bpm.to_string()),
        ("CSOCT", cs.octaves_down.to_string()),
        ("CSHZ", format!("{:.3}", cs.hz)),
        ("CSC", format!("{:+.1}", cs.cents)),
        ("GENESIS", genesis.to_string()),
        ("GDATE", format!("{gd} {} {gy}, 12:00 UTC", month_names[gm as usize])),
        ("HALV", halving_s.to_string()),
        ("YEARS", format!("{year_s:.0}")),
        ("ERAS", eras.to_string()),
        ("TOTY", format!("{total_years:.0}")),
        ("ANNUAL", format!("{annual_qug:.0}")),
        ("PERSEC", format!("{per_second:.6}")),
        ("PERBEAT", format!("{per_beat:.5}")),
        ("TICKS", format!("${}$", tex_sci(ticks_per_era as f64, 4))),
        ("SRC", latex_escape(src_path)),
        ("SRCHASH", src_hash.clone()),
        ("S248", rendered[0].5.to_string()),
        ("L248", rendered[0].6.to_string()),
    ];
    let body_tex = fill(PAPER_BODY, &pv);
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
            "\\title{A good time: a party night in three songs, its keys and its tempo computed from CODATA 2022}\n\
\\author{Viktor Kristensen and Rocky\\\\\\small Quillon Graph soundtrack, songs 248--250 \\quad generated by \\texttt{flux-arxiv-latex} (\\texttt{quillon\\_good\\_time\\_songs})}\n\
\\date{1 November 2026}\n",
        )
        .add(Block::Raw("\\maketitle\n".into()))
        .add(Block::Raw(body_tex))
        .add(Block::Raw(REFERENCES.into()))
        .add(Block::Raw(format!("\\appendix\n\\section{{The three songs, as generated}}\n{appendix}")));
    std::fs::create_dir_all(out_dir).expect("out dir");
    let res = doc.compile_pdf(out_dir, "quillon-good-time-songs-248-250");
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
        "bpm_exact": bpm_exact, "bpm": bpm, "beat_s": beat_s,
        "s248": {"r_inf": r_inf, "r_inf_rel_codata": r_inf_rel, "h_alpha_nm": balmer[0].2 * 1e9, "h_alpha_hz_folded": h_alpha.hz,
                 "h_alpha_note": h_alpha.name, "h_alpha_cents": h_alpha.cents, "intervals_cents": ivals, "chord": chord_sung},
        "s249": {"wien_x": x, "wien_b": wien_b, "wien_rel_codata": wien_rel, "lambda_um": lam_body * 1e6, "note": body.name,
                 "cents": body.cents, "range_30_40C_cents": [range_cents.0, range_cents.1]},
        "s250": {"cs_note": cs.name, "cs_cents": cs.cents, "annual_qug": annual_qug, "per_second": per_second,
                 "per_beat": per_beat, "eras": eras, "years": total_years, "genesis": genesis, "source_blake3": src_hash},
        "chars": rendered.iter().map(|r| serde_json::json!({"song": r.0, "style": r.5, "lyrics": r.6})).collect::<Vec<_>>(),
    });
    std::fs::write(format!("{out_dir}/quillon-good-time-songs-params.json"), serde_json::to_string_pretty(&params).unwrap()).expect("params");
    println!("OK {}", res.pdf_path.unwrap());
    println!("{}", serde_json::to_string(&params).unwrap());
}

// ───────────────────────────── the songs ─────────────────────────────

const STYLE_248: &str = "Epic festival progressive trance anthem, A-flat minor, @@BPM@@ BPM, four-on-the-floor kick, a huge supersaw lead whose hook climbs four notes: A-flat, D-flat, E-flat, F-flat. An open-air party at an old hilltop observatory at dusk: the dome motor groans open, the crowd cheers. Powerful, clear female lead vocal; a man's warm spoken voice like a planetarium narrator. Fretless bass sings the melody in the breakdown over warm pads; a big choir in the final drop; side-chained sub-bass, rolling arpeggios, cinematic strings, risers and a long snare roll before each drop. The light show is four colours, red, cyan, blue and violet. Euphoric, cosmic and communal, made for ten thousand hands in the air.";

const LYRICS_248: &str = "[Intro: crickets on a hilltop, a crowd murmuring, an observatory dome groaning open, a telescope whirring into place]

[Spoken, a man, planetarium voice]
Ladies and gentlemen. Hydrogen.

[Verse 1]
Up on the hill the dome rolls open
The sun goes down behind the town
We point the old glass at a cloud of fire
And the whole cloud glows red
The simplest atom, one proton, one electron
Falls from the third step to the second
And lets go of one red light

[Pre-Chorus]
@@HA_NM@@ nanometres
@@HA_THZ@@ trillion times a second
Take it down @@HA_OCT@@ octaves
And you can sing it

[Chorus]
Red is @@KEY248@@, red is @@KEY248@@
Every atom in the sky shines the same red
Nobody owns it, anybody can check it
Turn it up, red is @@KEY248@@

[Drop: the four-note hook, @@CHORD@@, the lights flash red, cyan, blue, violet]

[Verse 2]
From the fourth step, cyan; from the fifth, blue
From the sixth, violet, and every one's a note
A fourth, a fifth, a minor sixth above the red
Hydrogen was always singing in @@KEY248@@ minor

[Bridge: drums drop out, fretless bass and choir]
On the chain, every node can check the same block
On the hill, every telescope sees the same red
Nobody needs a king to tell you what a colour is
Just a number nobody can print again

[Build: snare roll, the crowd counts down]
Four lights, three steps, two hands up, one atom

[Final Chorus, choir]
Red is @@KEY248@@, red is @@KEY248@@
Every atom in the sky shines the same red
Nobody owns it, anybody can check it
Turn it up, red is @@KEY248@@

[Outro: the hook slows down, the first stars, the crowd sings the four notes alone]

[Spoken, softly]
Hydrogen. Good evening.";

const STYLE_249: &str = "Peak-time nu-disco and funk house, B-flat minor, @@BPM@@ BPM, four-on-the-floor kick with live drums, slap bass and a singing fretless bass trading licks, a tight horn section of trumpets, tenor sax and trombone, clavinet, wah guitar, handclaps, cowbell and string stabs. A soulful female belter and a male falsetto trade lines and the crowd shouts back. Midnight at an open-air party at a hilltop observatory, the dancers shown by a thermal camera on a giant screen, everyone glowing. Joyful, sweaty, warm, an irresistible groove; a breakdown with only bass and handclaps, then the horns come back huge.";

const LYRICS_249: &str = "[Intro: crowd noise, a camera beeping, the DJ on the microphone]

[Spoken, the DJ]
Look up at the screen. That's you. You're glowing.

[Verse 1]
Midnight on the hill, the telescope's asleep
But the camera that sees heat is wide awake
Every body on the floor is a little star
@@BODY_C@@ degrees and dancing

[Pre-Chorus]
A warm body shines brightest
At @@BODY_UM@@ micrometres
A light no eye can see
Bring it down @@BODY_OCT@@ octaves
And it's a note

[Chorus]
We glow in @@KEY249@@, we glow in @@KEY249@@
A colour no eye can see
@@BODY_C@@ degrees, the whole hill glowing
We glow in @@KEY249@@ (@@KEY249@@!)

[Verse 2]
Warm skin or a fever, it doesn't matter
Thirty degrees or forty, check it if you like
From cool skin to a burning head
Every one of us still lands on @@KEY249@@

[Breakdown: only slap bass, fretless bass and handclaps]
Work makes heat, that's the deal
The miners' machines are glowing too
Nothing in this world works without getting warm
So dance, it's honest work

[Final Chorus, horns huge]
We glow in @@KEY249@@, we glow in @@KEY249@@
A colour no eye can see
@@BODY_C@@ degrees, the whole hill glowing
We glow in @@KEY249@@ (@@KEY249@@!)

[Outro: horns, the crowd chanting the note, the camera beeps]

[Spoken, the DJ]
Stay warm. Sunrise is coming.";

const STYLE_250: &str = "Euphoric sunrise progressive house, D-flat major, @@BPM@@ BPM, a ticking clock as the hi-hat, warm analog pads, a melodic fretless bass as the lead voice, a nylon-string guitar arpeggio, glittering plucks, a slow build into a big final drop with a gospel-tinged choir. A man and a woman sing together, tired and happy. A man's dry spoken voice reads numbers. Field recordings: birds at dawn, wind on a hilltop, an old observatory clock ticking, the crowd's last cheer. Golden and emotional, a whole night resolving into morning.";

const LYRICS_250: &str = "[Intro: birds at dawn, an old clock ticking, one long warm pad, the crowd quiet]

[Spoken, a man, dry]
@@CS_SPOKEN@@.
That's one second.

[Verse 1]
The caesium atom keeps the time
It ticks that many times a second, never late
Bring it down @@CS_OCT@@ octaves and it hums
C sharp, almost, so the sun comes up in D-flat
Count two to the @@BITS@@ of its ticks
And the counter rolls over and starts again
That's one beat

[Pre-Chorus]
@@BPM_WORDS@@ beats a minute
Every beat a counter overflowing
All night long we danced on caesium
And never missed a tick

[Chorus]
It's a good time, it's a good time
Measured to the tick, and the sun is coming up
Nobody owns the second, anybody can count it
It's a good time

[Verse 2]
And the chain keeps time the same way
It doesn't pay by the block, it pays by the clock
@@ANNUAL@@ a year
However fast the blocks come in
A year of @@YEAR_S@@ seconds
@@PER_BEAT@@ of a coin for every beat we danced

[Bridge: drums drop out, fretless bass alone, the clock]
It started at noon on the @@GDAY@@ of @@GMONTH@@, @@GYEAR@@
And it runs for @@YEARS@@ years
@@ERAS@@ eras, every one half the last
Long after this sunrise, a second is still a second

[Build: the guitar arpeggio climbs, the choir enters]

[Final Chorus, choir]
It's a good time, it's a good time
Measured to the tick, and the sun is coming up
Nobody owns the second, anybody can count it
It's a good time

[Outro: the clock ticks on, the guitar plays the last arpeggio in D-flat, birds, the crowd's last cheer]

[Spoken, a woman, softly]
Same time next year?";

// ───────────────────────────── the paper ─────────────────────────────

const PAPER_BODY: &str = r#"\begin{abstract}
Three party songs for the Quillon Graph soundtrack, one night from dusk to sunrise, whose keys and shared tempo are computed from fundamental constants. At dusk, hydrogen: its first visible spectral line, folded down by whole octaves, lands on A$\flat$ (@@HAC@@ cents), and its next three lines are a fourth, a fifth and a minor sixth above it, notes of A$\flat$ minor. At midnight, a crowd: by Wien's law a body at 37\,$^\circ$C shines brightest at @@LBODY@@\,$\mu$m, which folds to B$\flat$, and every temperature from 30 to 40\,$^\circ$C lands on the same note. At sunrise, the second: one beat is $2^{32}$ periods of the caesium clock that defines it, @@BPMX@@ beats per minute, the tempo of the whole set; caesium itself folds to C$\sharp$, and Quillon's emission schedule, read from its source code, pays by the clock. Every number is computed by the generator; every claim carries one of six labels: \emph{exact}, \emph{measured} (CODATA 2022), \emph{design} (a constant in Quillon's source), \emph{convention}, \emph{historical}, \emph{choice}. The program refuses to write the lyrics if any number stops supporting a line.
\end{abstract}

\section{The method}
A frequency lowered by a whole octave is halved and keeps its note name. Any frequency, however high, can therefore be lowered by $k$ octaves into the range of a voice and named against concert pitch A4 = 440\,Hz (\emph{convention}, ISO~16). This is the method of the earlier song 244, \emph{Le Grand K}. The note names depend on the 440\,Hz convention; the octave counts and the intervals between lines do not.

\section{Song 248: hydrogen}
The Rydberg constant is computed, not typed: $R_\infty = m_e c\,\alpha^2/(2h)$ with $\alpha^{-1} = @@AINV@@$ and $m_e = @@ME@@$\,kg (\emph{measured}, CODATA 2022) and $c$, $h$ (\emph{exact}) gives $R_\infty = @@RINF@@$\,m$^{-1}$, against the CODATA 2022 recommended @@RINFC@@\,m$^{-1}$ (relative difference @@RINFREL@@). For hydrogen the electron orbits the proton's centre of mass: $R_H = R_\infty/(1 + m_e/m_p) = @@RH@@$\,m$^{-1}$ (reduced-mass factor @@MU@@, $m_p = @@MP@@$\,kg). The Balmer lines are $\nu = c R_H\,(1/4 - 1/n^2)$ (Bohr level; fine structure shifts them by under a cent and is ignored):
\begin{center}\small\begin{tabular}{lrrrlrrl}\toprule
line & $\lambda$ (nm) & $\nu$ (THz) & folded (Hz) & note & cents & above H$\alpha$ & sung\\\midrule
@@BALMER@@\bottomrule\end{tabular}\end{center}
H$\alpha$, the red light of glowing hydrogen clouds, lowered @@HAOCT@@ octaves, is @@HAHZ@@\,Hz: A$\flat$4, @@HAC@@ cents (\emph{derived}). The ratios of the lines do not depend on any constant: H$\beta$/H$\alpha$ = 27/20 (@@IVB@@ cents, a fourth sharp by about twenty cents), H$\gamma$/H$\alpha$ = 189/125 (@@IVG@@ cents, a fifth), H$\delta$/H$\alpha$ = 8/5 exactly (@@IVD@@ cents, a pure minor sixth). A$\flat$, D$\flat$, E$\flat$ and F$\flat$ are all notes of A$\flat$ minor; the song's hook climbs them in the order hydrogen emits its colours (\emph{choice}).

\section{Song 249: a warm body}
Wien's displacement constant is solved, not typed: the brightest wavelength of a black body at temperature $T$ is $\lambda_{\max} = b/T$ with $b = hc/(k x)$, where $x$ solves $x = 5(1 - e^{-x})$. Newton's method gives $x = @@X@@$ and $b = @@WIENB@@$\,m\,K, against the CODATA 2022 value (relative difference @@WIENREL@@); $h$, $c$ and $k$ are all exact, so $b$ is exact too. For $T = @@TBODY@@$\,K (37\,$^\circ$C, a physiological \emph{convention}) $\lambda_{\max} = @@LBODY@@\,\mu$m, far infrared, invisible. The frequency of that wavelength, $c/\lambda_{\max} = @@NUBODY@@$\,THz, lowered @@BODYOCT@@ octaves is @@BODYHZ@@\,Hz: B$\flat$4, @@BODYC@@ cents (\emph{derived}). Scanning $T$ from 303.15 to 313.15\,K in steps of 0.1\,K, every value lands on B$\flat$, between @@RCLO@@ and @@RCHI@@ cents: the note does not depend on whose body or how warm. (Note: $\lambda_{\max}$ is the peak per unit \emph{wavelength}; the peak per unit frequency lies elsewhere. We follow the wavelength form, the one Wien's law is usually stated in.) A human body is not a perfect black body; skin is close to one in the far infrared, which is why thermal cameras work.

\section{Song 250: the second}
The second is defined by fixing the caesium-133 hyperfine frequency at 9\,192\,631\,770\,Hz (\emph{exact}). A beat of $2^{32}$ caesium periods --- the moment a 32-bit counter clocked by caesium overflows --- lasts @@BEATS@@\,s, which is @@BPMX@@ beats per minute; rounded, @@BPM@@, the classic tempo of house music (mapping is \emph{choice}; the arithmetic is exact). All three songs run at this tempo so that they mix into one another like a DJ set. Caesium itself, lowered @@CSOCT@@ octaves, is @@CSHZ@@\,Hz, C$\sharp$4 at @@CSC@@ cents, as in song 244; the sunrise is in D$\flat$ major (\emph{choice}).

\paragraph{Quillon pays by the clock (design).} The constants below are read by the generator from Quillon's emission source (file and BLAKE3 hash under Reproduce): genesis at Unix time @@GENESIS@@ (@@GDATE@@), a Julian year of @@YEARS@@\,s, eras of @@HALV@@\,s (four Julian years), @@ERAS@@ eras (@@TOTY@@ years), and an era-0 emission of @@ANNUAL@@\,QUG per year. The reward per block is set from the measured block rate so that the annual emission stays on schedule however fast blocks arrive: the chain pays by the clock, not by the block. That is @@PERSEC@@\,QUG per second, or @@PERBEAT@@\,QUG per beat of this set. One era is @@TICKS@@ caesium periods. These are design numbers; what a node has actually paid is a measurement, and the referee songs (245--247) are about that.

\section{What is still pretend}
\begin{itemize}
\item (\emph{choice}) Every mapping from physics to music: octave folding, the A4 reference, the $2^{32}$ beat, the modes, the genres and the party. Physics fixes the numbers; it does not ask for a party.
\item (\emph{convention}) Note names rest on A4 = 440\,Hz; the body temperature is a convention (but the note is the same from 30 to 40\,$^\circ$C).
\item (\emph{not measured}) Whether a music model keeps the key, the tempo or the four-note hook has not been checked; nobody has heard these songs yet. It plays in equal temperament: the cents are rounded away.
\item (\emph{design}) Song 250 sings Quillon's schedule, not a measurement of what was paid.
\end{itemize}

\section{Reproduce}
\begin{flushleft}\footnotesize\texttt{fluxc build -p flux-arxiv-latex --bin quillon\_good\_time\_songs}\\
\texttt{quillon\_good\_time\_songs <emission\_controller.rs> [out\_dir]}\\[4pt]
Quillon source read: \texttt{@@SRC@@}\\
BLAKE3: \texttt{@@SRCHASH@@}\end{flushleft}
The program refuses to write anything if the computed $R_\infty$ or $b$ disagree with CODATA 2022, if H$\alpha$ stops landing on A$\flat$, if the lines stop being a fourth, a fifth and a minor sixth, if any body temperature from 30 to 40\,$^\circ$C leaves B$\flat$, if the beat stops rounding to 128, if caesium stops being a flat C$\sharp$, if Quillon's source no longer says 24 decimals, 2,625,000 a year, a Julian year, 64 eras of four years and noon on 22 February 2026, or if a text exceeds Suno's limits (this set: song 248 style @@S248@@, lyrics @@L248@@ characters).
"#;

const REFERENCES: &str = r#"\begin{thebibliography}{9}
\bibitem{codata2022} E.~Tiesinga, P.~J.~Mohr, D.~B.~Newell and B.~N.~Taylor, \emph{CODATA Recommended Values of the Fundamental Physical Constants: 2022}, Rev.\ Mod.\ Phys.\ (2025); arXiv:2409.03787.
\bibitem{si2019} Bureau International des Poids et Mesures, \emph{The International System of Units (SI)}, 9th edition (2019).
\bibitem{iso16} ISO 16:1975, \emph{Acoustics --- Standard tuning frequency (Standard musical pitch)}.
\bibitem{balmer} J.~J.~Balmer, \emph{Notiz \"uber die Spectrallinien des Wasserstoffs}, Annalen der Physik 261, 80 (1885) (\emph{historical}).
\bibitem{wien} W.~Wien, \emph{Eine neue Beziehung der Strahlung schwarzer K\"orper zum zweiten Hauptsatz der W\"armetheorie}, Sitzungsberichte der Preussischen Akademie der Wissenschaften (1893) (\emph{historical}).
\bibitem{legrandk} Quillon Graph soundtrack, song 244, \emph{Le Grand K} (2026): the octave-folding method.
\end{thebibliography}
"#;
