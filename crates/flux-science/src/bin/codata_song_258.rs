//! Song 258, "The Constants Held": a memorial song for Thursday 12 November 2026
//! (the Brooklyn building collapse of 11 November, whose search ended that afternoon in
//! the rain, and the shooting in Logan Circle, Washington D.C., that morning), set against
//! the CODATA 2018 -> 2022 adjustment and the 2025 muon result: a season of victories.
//!
//! Every number the lyrics sing is computed here from the BLAKE3-pinned NIST tables
//! embedded in `flux_science::electromagnetism`. Each lyric that carries a number has a
//! check; if any check fails, nothing is written and the program exits 1. The news facts
//! are NOT machine-checked: they are listed with their sources in the paper.
//!
//! Usage: codata_song_258 [OUT_DIR]   (default /home/storage/flux-scratch/codata-em-1113/song-258)

use flux_science::electromagnetism::{self as em, muon_g2, CodataTable};
use std::path::PathBuf;

const SONG_NO: u32 = 258;
const TITLE: &str = "The Constants Held";
const SLUG: &str = "QUILLON_258_THE_CONSTANTS_HELD";
const PAPER: &str = "quillon-258-the-constants-held";
const STYLE_MAX: usize = 1000;
const LYRICS_MAX: usize = 3000;
const A4_HZ: f64 = 440.0;
/// One beat per SI second.
const BPM: u32 = 60;
const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// The news facts the lyrics rest on, with where each was read (13 Nov 2026). Not checked
/// by this program; provisional as all early reporting is.
const NEWS: [(&str, &str); 9] = [
    ("A three-story mixed-use building at 1321 Flatbush Ave., Brooklyn, collapsed just after 4 p.m. on Wednesday 11 November.", "CBS New York live updates"),
    ("Eight people died, six adults and two children; eighteen were injured, including four firefighters.", "CBS New York (Mayor Mamdani)"),
    ("Two of the dead were construction workers; excavation was under way on the neighbouring property. Officials had not said what caused the collapse.", "CBS New York"),
    ("An expert quoted by CBS called the building 100 years old.", "CBS New York"),
    ("Construction workers who had only barely survived turned back and dug people out of the rubble.", "CBS New York (Mayor Mamdani)"),
    ("Steady rain on Thursday slowed the work; the FDNY completed rescue operations at 3:37 p.m. Thursday.", "CBS New York"),
    ("The mayor: these were friends and neighbours, parents and children.", "CBS New York (Mayor Mamdani)"),
    ("Just before 8 a.m. on Thursday 12 November a gunman opened fire at a gym in Logan Circle, Washington D.C.; the building manager was killed and four people were injured.", "NBC News live blog"),
    ("Police said the gunman had a conflict with the building over maintenance issues and mould; he was found with a self-inflicted gunshot wound.", "NBC News live blog"),
];

struct Note {
    name: &'static str,
    idx: usize,
    octave: i32,
    cents: f64,
}

fn nearest(hz: f64) -> Note {
    let n = (12.0 * (hz / A4_HZ).log2()).round() as i32;
    let idx = (n + 9).rem_euclid(12) as usize;
    let reference = A4_HZ * 2f64.powf(n as f64 / 12.0);
    Note { name: NOTE_NAMES[idx], idx, octave: 4 + (n + 9).div_euclid(12), cents: 1200.0 * (hz / reference).log2() }
}

fn digit_word(d: u32) -> &'static str {
    ["zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine"][d as usize]
}

fn words(n: u32) -> String {
    const ONES: [&str; 20] = [
        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve",
        "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen", "nineteen",
    ];
    const TENS: [&str; 10] = ["", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety"];
    assert!(n < 1000);
    let under_100 = |m: u32| -> String {
        if m < 20 {
            ONES[m as usize].to_string()
        } else if m % 10 == 0 {
            TENS[(m / 10) as usize].to_string()
        } else {
            format!("{}-{}", TENS[(m / 10) as usize], ONES[(m % 10) as usize])
        }
    };
    if n < 100 {
        under_100(n)
    } else if n % 100 == 0 {
        format!("{} hundred", ONES[(n / 100) as usize])
    } else {
        format!("{} hundred and {}", ONES[(n / 100) as usize], under_100(n % 100))
    }
}

/// 4.2 -> "four point two", 0.6 -> "point six" (one decimal, as sung).
fn spoken_one_decimal(x: f64) -> String {
    let tenths = (x * 10.0).round() as u32;
    let (int, dec) = (tenths / 10, tenths % 10);
    if int == 0 {
        format!("point {}", digit_word(dec))
    } else {
        format!("{} point {}", words(int), digit_word(dec))
    }
}

fn fill(template: &str, kv: &[(&str, String)]) -> String {
    let mut s = template.to_string();
    for (k, v) in kv {
        s = s.replace(&format!("@@{k}@@"), v);
    }
    s
}

fn tex_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' | '%' | '$' | '#' | '_' | '{' | '}' => {
                o.push('\\');
                o.push(ch);
            }
            '[' => o.push_str("{[}"),
            ']' => o.push_str("{]}"),
            '~' => o.push_str("\\textasciitilde{}"),
            '^' => o.push_str("\\textasciicircum{}"),
            '\\' => o.push_str("\\textbackslash{}"),
            _ => o.push(ch),
        }
    }
    o
}

struct Claim {
    lyric: String,
    value: String,
    rule: String,
    ok: bool,
}

fn main() {
    let out = PathBuf::from(
        std::env::args().nth(1).unwrap_or_else(|| "/home/storage/flux-scratch/codata-em-1113/song-258".into()),
    );
    let t18 = CodataTable::nist_2018();
    let t22 = CodataTable::nist_2022();
    let mut claims: Vec<Claim> = Vec::new();
    let mut claim = |lyric: &str, value: String, rule: &str, ok: bool| {
        claims.push(Claim { lyric: lyric.into(), value, rule: rule.into(), ok });
    };

    // 1. Tempo: one beat per SI second, the second fixed by caesium.
    let cs = t22.must("hyperfine transition frequency of Cs-133");
    claim(
        "One beat a second, exact by definition",
        format!("{BPM} BPM; Delta nu Cs = {:.0} Hz (exact)", cs.value),
        "Cs row printed (exact) and equal to 9 192 631 770 Hz; 60 BPM = 1 beat/s",
        cs.exact && cs.value == 9_192_631_770.0 && BPM == 60,
    );

    // 2. Hydrogen's lines, folded down by octaves: the key of the song.
    let rinf_c = t22.must("Rydberg constant times c in Hz");
    let me_mp = t22.must("electron-proton mass ratio");
    let rh_c = rinf_c.value / (1.0 + me_mp.value); // reduced-mass Rydberg frequency of hydrogen
    let line = |n1: f64, n2: f64| rh_c * (1.0 / (n1 * n1) - 1.0 / (n2 * n2));
    let h_alpha_oct = 40;
    let lyman_oct = 42;
    let ion_oct = 43;
    let h_alpha = line(2.0, 3.0);
    let lyman = line(1.0, 2.0);
    let ion = rh_c;
    let na = nearest(h_alpha / 2f64.powi(h_alpha_oct));
    let nl = nearest(lyman / 2f64.powi(lyman_oct));
    let ni = nearest(ion / 2f64.powi(ion_oct));
    claim(
        "Hydrogen's red line, folded forty octaves down, is a G-sharp, less than a fifth of a cent away",
        format!("H-alpha {:.6e} Hz / 2^{h_alpha_oct} = {:.3} Hz = {}{} {:+.3} cent", h_alpha, h_alpha / 2f64.powi(h_alpha_oct), na.name, na.octave, na.cents),
        "nearest note G# and |cents| < 0.2 (A4 = 440 Hz, equal temperament)",
        na.name == "G#" && na.cents.abs() < 0.2 && h_alpha_oct == 40,
    );
    let key_ok = nl.name == "C#" && (ni.idx + 12 - nl.idx) % 12 == 5 && (na.idx + 12 - nl.idx) % 12 == 7;
    claim(
        "Style: C-sharp minor (tonic, subdominant, dominant = hydrogen's three first lines)",
        format!(
            "Lyman-alpha -> {}{} {:+.1} c; ionisation -> {}{} {:+.1} c; H-alpha -> {}{} {:+.2} c",
            nl.name, nl.octave, nl.cents, ni.name, ni.octave, ni.cents, na.name, na.octave, na.cents
        ),
        "Lyman-alpha nearest C#, ionisation a fourth above it, H-alpha a fifth above it",
        key_ok,
    );

    // 3. The alpha adjustment.
    assert_eq!((t18.year, t22.year), (2018, 2022), "table headers");
    claim(
        "Between twenty-eighteen and twenty-twenty-two",
        format!("table years {} and {}", t18.year, t22.year),
        "embedded tables are the 2018 and 2022 adjustments",
        t18.year == 2018 && t22.year == 2022,
    );
    let raw = em::weighted_mean(&em::ALPHA_INPUTS_2022, 1.0);
    let expanded = em::weighted_mean(&em::ALPHA_INPUTS_2022, em::ALPHA_EXPANSION_FACTOR_2022);
    let p_raw = em::chi2_p_dof2(raw.chi2);
    let p_exp = em::chi2_p_dof2(expanded.chi2);
    claim(
        "Three honest answers, and they would not agree",
        format!("chi2 = {:.1} on {} dof, p = {:.1e}", raw.chi2, raw.dof, p_raw),
        "p < 0.001 with the stated uncertainties",
        p_raw < 1e-3,
    );
    let mean_ok = (expanded.mean - t22.alpha_inverse().value).abs() < 1.5e-9;
    claim(
        "so they widened every doubt two and a half times / and let them stand together in one room",
        format!(
            "x{} -> mean {:.9}({:.0}), chi2 = {:.2}, p = {:.3}",
            em::ALPHA_EXPANSION_FACTOR_2022,
            expanded.mean,
            expanded.sigma * 1e9,
            expanded.chi2,
            p_exp
        ),
        "factor is 2.5, expanded p > 0.05, mean reproduces the 2022 alpha^-1 row",
        em::ALPHA_EXPANSION_FACTOR_2022 == 2.5 && p_exp > 0.05 && mean_ok,
    );
    let dln = em::ln_alpha_ratio(&t18, &t22);
    let shift_cents = dln.abs() * 1200.0 / std::f64::consts::LN_2;
    claim(
        "moved by about a millionth of a cent, if it were a note",
        format!("d ln alpha = {:.4e} -> {:.3e} cent", dln, shift_cents),
        "0.8e-6 <= |shift| <= 1.5e-6 cent",
        (0.8e-6..=1.5e-6).contains(&shift_cents),
    );

    // 4. mu0 comes home.
    let (d18, s18) = em::mu0_drift(&t18);
    let (d22, s22) = em::mu0_drift(&t22);
    claim(
        "four pi came home inside its error bars",
        format!("mu0/(4 pi e-7) - 1: 2018 {:+.1}({:.0})e-11 = {:+.1} sigma; 2022 {:+.1}({:.0})e-11 = {:+.1} sigma", d18 * 1e11, s18 * 1e11, d18 / s18, d22 * 1e11, s22 * 1e11, d22 / s22),
        "|z| > 3 in 2018 and |z| < 1 in 2022",
        (d18 / s18).abs() > 3.0 && (d22 / s22).abs() < 1.0,
    );

    // 5. Bridge numbers.
    let z0 = t22.must("characteristic impedance of vacuum");
    let z0_ohms = z0.value.floor() as u32;
    let z0_words = words(z0_ohms);
    let z0_r_k = z0.value / em::si2019::von_klitzing();
    let alpha = 1.0 / t22.alpha_inverse().value;
    claim(
        &format!("Empty space has a resistance: {z0_words} ohms"),
        format!("Z0 = {:.9} ohm; Z0/R_K = {:.12} = 2 alpha ({:.12})", z0.value, z0_r_k, 2.0 * alpha),
        "floor(Z0) spoken; Z0 = 2 alpha R_K to 1e-9",
        z0_ohms == 376 && (z0_r_k / (2.0 * alpha) - 1.0).abs() < 1e-9,
    );
    let ai = t22.alpha_inverse();
    let sig_figs = (ai.value.log10().floor() - ai.ulp.log10().round()) as u32 + 1;
    let doubt_units = (ai.sigma / ai.ulp).round() as u32;
    let doubt_digits = doubt_units.to_string().len() as u32;
    claim(
        &format!(
            "One over alpha is known to {} figures, and the last {} are still in doubt",
            words(sig_figs),
            words(doubt_digits)
        ),
        format!("alpha^-1 = {:.9}({doubt_units}): {sig_figs} significant figures, uncertainty {doubt_units} in the last place", ai.value),
        "significant figures counted from the printed row; uncertainty spans that many last digits",
        sig_figs == 12 && doubt_digits == 2,
    );
    let muon_new = muon_g2::wp25_tension();
    let muon_old_words = spoken_one_decimal(muon_g2::CODATA_2022_DISCREPANCY_SIGMA);
    let muon_new_words = spoken_one_decimal(muon_new);
    claim(
        &format!("The muon: {muon_old_words} sigma in the old book, {muon_new_words} in the new"),
        format!("CODATA 2022 Sec. VII: {} sigma; 2025 exp. average vs WP25: {:.2} sigma", muon_g2::CODATA_2022_DISCREPANCY_SIGMA, muon_new),
        "old > 4, new < 1 (the 'new book' is WP25 + Fermilab 2025, not a CODATA table)",
        muon_g2::CODATA_2022_DISCREPANCY_SIGMA > 4.0 && muon_new.abs() < 1.0,
    );
    let rp = em::proton_radius_tension();
    claim(
        "the people who keep the table still wrote: the proton radius puzzle is not yet over",
        format!("electronic vs muonic r_p (Table XVI): {:.2} sigma; paper line 261", rp),
        "the tension the paper refers to is still > 2 sigma",
        rp > 2.0,
    );

    // Refuse before writing anything.
    let failed: Vec<&Claim> = claims.iter().filter(|c| !c.ok).collect();
    if !failed.is_empty() {
        for c in &failed {
            eprintln!("REFUSED: \"{}\" -- {} (rule: {})", c.lyric, c.value, c.rule);
        }
        std::process::exit(1);
    }

    let kv = [
        ("BPM", BPM.to_string()),
        ("Z0_WORDS", z0_words.clone()),
        ("ALPHA_FIGS", words(sig_figs)),
        ("ALPHA_DOUBT", words(doubt_digits)),
        ("MUON_OLD", muon_old_words.clone()),
        ("MUON_NEW", muon_new_words.clone()),
    ];
    let style = fill(STYLE, &kv);
    let lyrics = fill(LYRICS, &kv);
    let mut gate = Vec::new();
    if style.len() > STYLE_MAX {
        gate.push(format!("style {} > {STYLE_MAX} chars", style.len()));
    }
    if lyrics.len() > LYRICS_MAX {
        gate.push(format!("lyrics {} > {LYRICS_MAX} chars", lyrics.len()));
    }
    for (name, text) in [("style", &style), ("lyrics", &lyrics)] {
        if text.contains("@@") {
            gate.push(format!("{name} has an unfilled placeholder"));
        }
        if !text.is_ascii() {
            gate.push(format!("{name} is not plain ASCII"));
        }
    }
    // Every computed number the lyrics sing must actually be in them.
    for needle in [
        format!("{z0_words} ohms"),
        format!("known to {} figures", words(sig_figs)),
        format!("{muon_old_words} sigma in the old book, {muon_new_words} in the new"),
        "forty octaves".to_string(),
        "a millionth of a cent".to_string(),
        "two and a half times".to_string(),
    ] {
        if !lyrics.contains(&needle) {
            gate.push(format!("lyrics lack '{needle}'"));
        }
    }
    if !gate.is_empty() {
        for g in &gate {
            eprintln!("GATE: {g}");
        }
        std::process::exit(1);
    }

    std::fs::create_dir_all(&out).expect("create out dir");
    let w = |name: &str, body: &str| {
        let p = out.join(name);
        let tmp = out.join(format!("{name}.tmp"));
        std::fs::write(&tmp, body).expect("write");
        std::fs::rename(&tmp, &p).expect("rename");
        p
    };
    w(&format!("{SLUG}_STYLE.txt"), &style);
    w(&format!("{SLUG}_LYRICS.txt"), &lyrics);

    // Paper.
    let mut claim_rows = String::new();
    for c in &claims {
        claim_rows.push_str(&format!(
            "\\emph{{{}}} & {} & {} \\\\\n\\addlinespace\n",
            tex_escape(&c.lyric),
            tex_escape(&c.value),
            tex_escape(&c.rule)
        ));
    }
    let mut news_rows = String::new();
    for (fact, src) in NEWS {
        news_rows.push_str(&format!("\\item {} \\hfill{{\\small\\textit{{({})}}}}\n", tex_escape(fact), tex_escape(src)));
    }
    // One paragraph per stanza: a \medskip inside a paragraph would land after the NEXT line.
    let lyric_tex: String = lyrics
        .split("\n\n")
        .filter(|st| !st.trim().is_empty())
        .map(|st| st.lines().map(tex_escape).collect::<Vec<_>>().join("\\\\\n"))
        .collect::<Vec<_>>()
        .join("\n\n\\smallskip\n");
    let hrow = |label: &str, f: f64, oct: i32, n: &Note| {
        format!(
            "{label} & {} & {oct} & {:.3} & {}$_{}$ & {:+.2} \\\\\n",
            tex_sci(f),
            f / 2f64.powi(oct),
            n.name.replace('#', "\\#"),
            n.octave,
            n.cents
        )
    };
    let hydrogen_rows = format!(
        "{}{}{}",
        hrow("Lyman-$\\alpha$ ($2\\to1$, ultraviolet)", lyman, lyman_oct, &nl),
        hrow("ionisation ($\\infty\\to1$)", ion, ion_oct, &ni),
        hrow("H-$\\alpha$ ($3\\to2$, red)", h_alpha, h_alpha_oct, &na)
    );
    let paper = fill(
        PAPER_TEX,
        &[
            ("SONG", SONG_NO.to_string()),
            ("TITLE", TITLE.to_string()),
            ("CLAIMS", claim_rows),
            ("NEWS", news_rows),
            ("HYDROGEN", hydrogen_rows),
            ("LYRICS", lyric_tex),
            ("STYLE", tex_escape(&style)),
            ("NCLAIMS", claims.len().to_string()),
            ("RHC", tex_sci(rh_c)),
            ("MEMP", format!("{:.10e}", me_mp.value).replace("e-4", "\\times10^{-4}")),
            ("B3_22", em::NIST_2022_BLAKE3[..16].to_string()),
            ("B3_18", em::NIST_2018_BLAKE3[..16].to_string()),
            ("SHIFTC", format!("{:.2}", shift_cents * 1e6)),
            ("STYLELEN", style.len().to_string()),
            ("LYRLEN", lyrics.len().to_string()),
        ],
    );
    let tex = w(&format!("{PAPER}.tex"), &paper);

    let mut summary = format!("Song {SONG_NO} \"{TITLE}\" -- {} checks passed\n", claims.len());
    for c in &claims {
        summary.push_str(&format!("  OK  {}\n      {}\n", c.lyric, c.value));
    }
    summary.push_str(&format!("style {} chars, lyrics {} chars\nwrote {}\n", style.len(), lyrics.len(), out.display()));
    w("summary.txt", &summary);
    print!("{summary}");
    println!("tex: {}", tex.display());
}

fn tex_sci(x: f64) -> String {
    let e = x.abs().log10().floor() as i32;
    format!("${:.6}\\times10^{{{e}}}$", x / 10f64.powi(e))
}

const STYLE: &str = "Slow, reflective memorial ballad in C-sharp minor at @@BPM@@ BPM, one beat per second. Rain on a city street opens the track and a cello holds one low G-sharp under everything; then felt piano, a string quartet and soft upright bass. A warm, unhurried male lead vocal, close-miked, thinking out loud rather than performing; a quiet female harmony and a small hummed choir join the chorus 'The constants held'. A brushed snare enters only in the second chorus. The bridge is spoken, calm and low over the G-sharp drone, like a scientist reading figures at a vigil. Grief without melodrama: no big drums, no key change, space between the phrases, one far church bell. The final chorus swells gently and resolves to C-sharp major, a Picardy third; the outro is half spoken over thinning rain and ends on a single piano G-sharp left ringing.";

const LYRICS: &str = r#"[Intro: rain on a city street, a cello holds one low G-sharp, a felt piano answers]

[Verse 1]
Thursday came in grey across the States.
In Washington, before eight in the morning,
a quarrel about a building, about the mould, police said,
and the man who kept that building running never made it home.
On Flatbush Avenue the rain came down
where a building near a hundred years old had fallen the day before;
they dug all night and into the rain, until almost four.
Eight lives, and two of them were children.

[Pre-Chorus]
And all that week I had been reading
about a season of victories.

[Chorus]
The constants held, the way they always hold:
light still keeps its speed through empty space,
four pi came home inside its error bars,
the muon's quarrel settled into calm.
The constants held. The walls did not.
And there's no adjustment, no factor I know,
that makes an empty chair agree with the world.

[Verse 2]
Between twenty-eighteen and twenty-twenty-two
the careful people of the world compared their notes:
caesium in California, rubidium in Paris,
one electron in a trap in Illinois.
Three honest answers, and they would not agree,
so they widened every doubt two and a half times
and let them stand together in one room,
and the number that tells light how hard to hold an electron
moved by about a millionth of a cent, if it were a note.
Not the constant. Only what we knew.

[Chorus]

[Bridge: spoken, calm, over the G-sharp drone]
Hydrogen's red line, folded forty octaves down, is a G-sharp, less than a fifth of a cent away.
Empty space has a resistance: @@Z0_WORDS@@ ohms.
One over alpha is known to @@ALPHA_FIGS@@ figures, and the last @@ALPHA_DOUBT@@ are still in doubt.
The muon: @@MUON_OLD@@ sigma in the old book, @@MUON_NEW@@ in the new.
And the people who keep the table still wrote: the proton radius puzzle is not yet over.
That is how an honest table reads. A value. An uncertainty. And sometimes: not yet known.
What brought the building down, officials have not said.

[Verse 3]
They said the workers who had barely made it out themselves
turned back into the dust and dug the others out.
Our friends and neighbours, the mayor said, parents and children.
I don't know what a victory means tonight.
Maybe only this: that someone checks,
that someone measures twice and writes the doubt down too,
that someone turns back into the rubble
when every part of them is telling them to run.

[Final Chorus]
The constants held. They'll always hold,
and that was never what we needed them to do.
The walls are ours, the care is ours,
the checking and the turning back are ours.
The constants held. The walls did not.
There's no factor, not two and a half, not any,
that makes an empty chair agree with the world.

[Outro: rain thinning, the hummed choir, one far bell]
One beat a second, exact by definition.
Everything else we keep by care.
[a single piano G-sharp, left ringing]
"#;

const PAPER_TEX: &str = r#"\documentclass[10pt,a4paper]{article}
\usepackage[T1]{fontenc}
\usepackage{lmodern}
\usepackage[margin=2.1cm]{geometry}
\usepackage{amsmath,amssymb,booktabs,array,xcolor}
\usepackage[hidelinks]{hyperref}
\usepackage{xurl}
\setlength{\parskip}{0.45em}\setlength{\parindent}{0pt}
\title{\textbf{@@TITLE@@}\\[0.3em]\large The measured numbers behind Quillon soundtrack song @@SONG@@}
\author{Rocky \textperiodcentered{} flux-science (\texttt{codata\_song\_258})}
\date{13 November 2026}
\begin{document}
\maketitle
\begin{abstract}
A memorial song for Thursday 12 November 2026 in the United States: the morning shooting in Logan Circle,
Washington D.C., and the afternoon on which the search after the Brooklyn building collapse of 11 November
ended in the rain. The song sets that grief beside a season of victories in precision physics, the CODATA
2018$\to$2022 adjustment and the 2025 muon $g-2$ result. All @@NCLAIMS@@ numerical claims in the lyrics and the
style are computed at generation time by \texttt{flux-science} from the NIST tables embedded in the crate
(BLAKE3 \texttt{@@B3_22@@\ldots} for 2022, \texttt{@@B3_18@@\ldots} for 2018); the generator refuses to write the
song if any check fails. The news facts are not machine-checked; they are listed with their sources and are
as provisional as all early reporting.
\end{abstract}

\section{The two events, as reported}
\begin{itemize}\setlength{\itemsep}{0.15em}
@@NEWS@@\end{itemize}
The song names no victim and makes no claim about the cause of the collapse: on 12--13 November officials had
not said what caused it, and the song says exactly that.

\section{Every number the song sings, and the check behind it}
\small
\begin{tabular}{>{\raggedright\arraybackslash}p{0.33\textwidth}>{\raggedright\arraybackslash}p{0.36\textwidth}>{\raggedright\arraybackslash}p{0.23\textwidth}}
\toprule
lyric & computed now & passes if \\
\midrule
@@CLAIMS@@\bottomrule
\end{tabular}
\normalsize

\section{Why C-sharp minor: hydrogen's first three lines}
Hydrogen's line frequencies follow from one number, the Rydberg frequency corrected for the proton's finite
mass, $R_\mathrm{H}c = R_\infty c/(1+m_e/m_p) = $ @@RHC@@~Hz, with $m_e/m_p = @@MEMP@@$, times
$1/n_1^2-1/n_2^2$. Halving a frequency keeps its pitch class, so each line can be folded down into the audible
range and compared with equal temperament at A4~=~440~Hz:

\begin{center}\small
\begin{tabular}{lrrrcr}
\toprule
line & frequency (Hz) & octaves down & Hz & note & cents \\
\midrule
@@HYDROGEN@@\bottomrule
\end{tabular}
\end{center}
The three lines land near C$\sharp$, F$\sharp$ and G$\sharp$: tonic, subdominant and dominant of C-sharp minor.
The red Balmer line H-$\alpha$, the colour of hydrogen, sits within a fifth of a cent of G$\sharp_4$ (Bohr levels
with the reduced mass; fine structure moves it by far less than that). Its G$\sharp$ is the drone under the song.
The two others are about a fifth of a semitone sharp, so they choose the key's degrees, not its tuning.
The tempo is 60~BPM: one beat per SI second, which since 1967 is exactly 9\,192\,631\,770 periods of caesium.

\section{What the song is thinking about}
The chorus says ``the constants held''. That is the physics, said carefully: nothing in the 2022 adjustment
shows a constant of nature changing. What moved was knowledge. The fine-structure constant's printed value
shifted by $d\ln\alpha = -6.79\times10^{-10}$, which as a musical interval is @@SHIFTC@@ millionths of a cent;
fifteen other constants moved with it, each by its own power of $\alpha$ (shown in the companion paper). The
victories of the period are of a particular kind: disagreements that resolved because people measured again
and wrote their doubt down. The magnetic constant came back within $0.8\sigma$ of $4\pi\times10^{-7}$; the muon's
$4.2\sigma$ quarrel fell to $0.6\sigma$ once lattice QCD was used for the hadronic term; and the committee, facing
three honest $\alpha$ measurements that disagreed ($\chi^2=31$ on 2 degrees of freedom), widened their
uncertainties by 2.5 instead of throwing one away. The same people wrote that the proton radius puzzle
``is not yet over''.

Grief has no expansion factor. The song's argument is that what we can keep is the habit behind those
victories, applied to the things we build: checking, writing the doubt down, and turning back. The image for
that is taken from the reporting, not invented: construction workers who had barely escaped turned back and
dug the others out.

\section{What this does not claim}
\begin{itemize}\setlength{\itemsep}{0.15em}
\item Nothing about why the building fell. ``Near a hundred years old'' is one expert's description in the
reporting; excavation next door is reported, its role is not established.
\item The $0.6\sigma$ muon figure is the Theory Initiative's 2025 White Paper against the 2025 experimental
average, not a CODATA table; ``the new book'' in the lyric means exactly that.
\item ``Light still keeps its speed'' is true by definition since 1983: $c$ is fixed in the SI, so it is a
statement about our units as much as about nature.
\item The musical mappings (folding by octaves, A4~=~440~Hz) are a choice of instrument, not physics.
\item News facts were read on 13 November 2026 from two outlets; CNN's page could not be loaded. Counts in
early reporting can change.
\end{itemize}

\section*{Suno style (@@STYLELEN@@ characters)}
\small @@STYLE@@ \normalsize

\section*{Lyrics (@@LYRLEN@@ characters)}
\small
@@LYRICS@@
\normalsize

\begin{thebibliography}{9}\small
\bibitem{codata} P.~J. Mohr, D.~B. Newell, B.~N. Taylor, E.~Tiesinga, \emph{CODATA recommended values of the
fundamental physical constants: 2022}, Rev. Mod. Phys. \textbf{97}, 025002 (2025); arXiv:2409.03787.
\bibitem{nist} NIST, \emph{Fundamental Physical Constants, ASCII tables 2022 and 2018},
\url{https://physics.nist.gov/cuu/Constants/Table/allascii.txt}.
\bibitem{wp25} Muon $g-2$ Theory Initiative, \emph{White Paper 2025}, arXiv:2505.21476.
\bibitem{fnal} Muon $g-2$ Collaboration, final result, 3 June 2025.
\bibitem{em} Rocky / flux-science, \emph{CODATA 2022: the electromagnetic block, recomputed},
\url{https://quillon.xyz/downloads/flux-science-codata-em-2022.pdf}.
\bibitem{cbs} CBS New York, \emph{Brooklyn building collapse} live updates, read 13 Nov 2026,
\url{https://www.cbsnews.com/newyork/live-updates/brooklyn-building-collapse-search-survivors/}.
\bibitem{nbc} NBC News, \emph{Washington D.C. shooting} live blog, read 13 Nov 2026,
\url{https://www.nbcnews.com/news/us-news/live-blog/live-updates-active-shooter-reported-washington-dc-rcna608960}.
\end{thebibliography}
\end{document}
"#;
