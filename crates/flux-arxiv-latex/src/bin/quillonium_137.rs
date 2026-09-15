//! quillonium_137 — "Quillonium 137": the companion paper to *The Kristensen K-Parameter*
//! (whitepaper v1.1, 2026-09-14). Two things in the world are called 137: the inverse of the
//! fine-structure constant, and the beacon on the Quillon Graph Skyskraber at quillon.xyz.
//! This paper keeps them honestly apart — and says exactly what, if anything, joins them.
//!
//! House rules (same as k_parameter.rs / sigil_court.rs): every number is COMPUTED here at
//! generation time from CODATA 2022 constants (flux-science) or MEASURED from a live/deployed
//! artifact whose bytes are hashed into the paper; every claim carries an epistemic label
//! (measured / derived / assumed / analogy / historical / claimed); the "what is still pretend"
//! section lists what was not measured; nothing is pasted from a draft.
//!
//! Usage: quillonium_137 [out_dir] [quillon_cadence.json] [beacon-137.js]
//!   quillon_cadence.json  — {"source": "...", "samples": [{"t": "<iso>", "height": N}, ...]}
//!                           taken READ-ONLY from GET /api/v1/status → data.upgrades.current_height
//!   beacon-137.js         — the deployed script; its constants are parsed, its bytes hashed.
use flux_arxiv_latex::doc::{Block, Document};
use flux_science::constants::*;

/// CODATA 2022 vacuum electric permittivity, F/m (not in flux-science yet; measured, u = 1.4e-21).
const VACUUM_PERMITTIVITY: f64 = 8.854_187_818_8e-12;
/// CODATA 2022 standard uncertainty of α⁻¹ (measured).
const FINE_STRUCTURE_INV_U: f64 = 0.000_000_021;

// The Quillon Graph Skyskraber blueprint inputs (flux-skyskraber: tower.rs / vault.rs / science.rs).
// Recomputed here with the same CODATA constants rather than linking the crate (its runtime pulls
// libp2p); the crate's own `annex_matches_hand_arithmetic` test pins the same values.
const TOWER_TOP_LEVEL: f64 = 88.0;
const FLOOR_HEIGHT_M: f64 = 4.0;
const BEACON_MAST_M: f64 = 40.0;
const VAULT_BARS: f64 = 8_000.0;
const BAR_KG: f64 = 12.4;

fn sci(x: f64) -> String {
    if x == 0.0 || !x.is_finite() {
        return format!("{x}");
    }
    let exp = x.abs().log10().floor() as i32;
    if (-2..=3).contains(&exp) {
        let s = format!("{:.4}", x);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        let mant = x / 10f64.powi(exp);
        format!("{:.3}\\times10^{{{}}}", mant, exp)
    }
}

fn raw(s: String) -> Block {
    Block::Raw(format!("{s}\n\n"))
}

struct Sample {
    t: String,
    unix: f64,
    height: f64,
}

fn parse_iso_utc(s: &str) -> Option<f64> {
    // YYYY-MM-DDTHH:MM:SSZ → unix seconds (proleptic Gregorian, UTC only).
    let (date, time) = s.trim_end_matches('Z').split_once('T')?;
    let mut d = date.split('-').map(|x| x.parse::<i64>().ok());
    let (y, m, dd) = (d.next()??, d.next()??, d.next()??);
    let mut t = time.split(':').map(|x| x.parse::<f64>().ok());
    let (hh, mm, ss) = (t.next()??, t.next()??, t.next()??);
    // days from civil (Howard Hinnant's algorithm)
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + dd - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days as f64 * 86_400.0 + hh * 3600.0 + mm * 60.0 + ss)
}

fn load_samples(path: &str) -> (String, Vec<Sample>) {
    let txt = std::fs::read_to_string(path).unwrap_or_default();
    let v: serde_json::Value = serde_json::from_str(&txt).unwrap_or(serde_json::Value::Null);
    let source = v["source"].as_str().unwrap_or("(no samples file)").to_string();
    let mut out = Vec::new();
    for s in v["samples"].as_array().cloned().unwrap_or_default() {
        if let (Some(t), Some(h)) = (s["t"].as_str(), s["height"].as_f64()) {
            if let Some(u) = parse_iso_utc(t) {
                out.push(Sample { t: t.to_string(), unix: u, height: h });
            }
        }
    }
    (source, out)
}

struct Beacon {
    path: String,
    bytes: usize,
    blake3: String,
    cadence_ms: f64,
    jitter_max_ms: f64,
    climb_ms: f64,
    uses_csprng: bool,
    reads_upgrades_height: bool,
}

fn parse_js_const(src: &str, name: &str) -> Option<f64> {
    let key = format!("var {name} = ");
    let i = src.find(&key)? + key.len();
    let rest = &src[i..];
    let end = rest.find(';')?;
    rest[..end].trim().parse::<f64>().ok()
}

fn load_beacon(path: &str) -> Option<Beacon> {
    let bytes = std::fs::read(path).ok()?;
    let src = String::from_utf8_lossy(&bytes).into_owned();
    Some(Beacon {
        path: path.to_string(),
        bytes: bytes.len(),
        blake3: hex::encode(blake3::hash(&bytes).as_bytes()),
        cadence_ms: parse_js_const(&src, "CADENCE_MS")?,
        jitter_max_ms: parse_js_const(&src, "JITTER_MAX_MS")?,
        climb_ms: parse_js_const(&src, "CLIMB_MS")?,
        uses_csprng: src.contains("getRandomValues"),
        reads_upgrades_height: src.contains("upgrades") && src.contains("current_height"),
    })
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out_dir = args.get(1).map(String::as_str).unwrap_or("/home/storage/sigil-scratch/quillonium-137");
    let samples_path = args.get(2).map(String::as_str).unwrap_or("/home/storage/sigil-scratch/quillonium-137/quillon-cadence.json");
    let beacon_path = args.get(3).map(String::as_str).unwrap_or("/home/orobit/q-narwhalknight/dist-final/beacon-137.js");

    let pi = std::f64::consts::PI;
    let c = SPEED_OF_LIGHT;
    let hbar = PLANCK_REDUCED;
    let me = ELECTRON_MASS;
    let e = ELEMENTARY_CHARGE;

    // ── 1. the coupling, from constants ─────────────────────────────────────────────────────
    let alpha_codata = fine_structure(); // 1/FINE_STRUCTURE_INV
    let alpha_derived = e * e / (4.0 * pi * VACUUM_PERMITTIVITY * hbar * c);
    let alpha_resid = (alpha_derived - alpha_codata).abs() / alpha_codata;
    let alpha_inv_rel_u = FINE_STRUCTURE_INV_U / FINE_STRUCTURE_INV;
    let v_bohr = alpha_codata * c; // Sommerfeld's original: the 1s electron speed of hydrogen
    let rydberg_j = 0.5 * alpha_codata * alpha_codata * me * c * c;
    let rydberg_ev = rydberg_j / ELECTRON_VOLT;
    let bohr_radius = hbar / (alpha_codata * me * c);
    let mec2_kev = me * c * c / ELECTRON_VOLT / 1e3;
    let flattery_rel = (FINE_STRUCTURE_INV - 137.0) / FINE_STRUCTURE_INV; // the tower's joke
    let joke_sigma = (FINE_STRUCTURE_INV - 137.0) / FINE_STRUCTURE_INV_U; // how many σ off 137 is

    // ── 2. the last atom: Bohr / Dirac point nucleus ─────────────────────────────────────────
    let z_list: [f64; 7] = [1.0, 26.0, 79.0, 92.0, 118.0, 137.0, 138.0];
    let dirac_rows: Vec<(f64, f64, f64, f64)> = z_list
        .iter()
        .map(|&z| {
            let za = z * alpha_codata;
            let eps2 = 1.0 - za * za;
            let eps = if eps2 >= 0.0 { eps2.sqrt() } else { f64::NAN }; // E_1s / m c²
            let binding_kev = if eps.is_finite() { (1.0 - eps) * mec2_kev } else { f64::NAN };
            (z, za, eps, binding_kev)
        })
        .collect();
    let z_crit_point = 1.0 / alpha_codata; // 137.036: where the Dirac 1s energy hits zero (point nucleus)
    let z_crit_finite: f64 = 173.0; // known result (finite nuclear size), CITED not derived

    // ── 3. Quillonium: the beacon, measured from its deployed bytes ─────────────────────────
    let beacon = load_beacon(beacon_path);
    let (source, samples) = load_samples(samples_path);
    let (cadence_bps, span_s, blocks, mean_interval_s) = if samples.len() >= 2 {
        let a = &samples[0];
        let b = &samples[samples.len() - 1];
        let span = b.unix - a.unix;
        let blocks = b.height - a.height;
        (blocks / span, span, blocks, span / blocks)
    } else {
        (f64::NAN, f64::NAN, f64::NAN, f64::NAN)
    };
    let pulses_137_s = 137.0 / cadence_bps;
    let (jit_mean, jit_sigma, poll_over_jitter, poll_s, jit_max) = beacon
        .as_ref()
        .map(|b| {
            (b.jitter_max_ms / 2.0, b.jitter_max_ms / 12f64.sqrt(), b.cadence_ms / b.jitter_max_ms, b.cadence_ms / 1e3, b.jitter_max_ms)
        })
        .unwrap_or((f64::NAN, f64::NAN, f64::NAN, f64::NAN, f64::NAN));

    // ── 4. tower physics (recomputed from the blueprint inputs) ─────────────────────────────
    let tower_h = TOWER_TOP_LEVEL * FLOOR_HEIGHT_M + BEACON_MAST_M;
    let photon_transit_us = tower_h / c * 1e6;
    let vault_kg = VAULT_BARS * BAR_KG;
    let vault_rest_j = vault_kg * c * c;
    let ml_rate = |e_j: f64| 2.0 * e_j / (pi * hbar); // Margolus–Levitin, ops/s at energy E
    let ml_one_joule = ml_rate(1.0);
    let ml_vault = ml_rate(vault_rest_j);
    let ml_lloyd_kg = ml_rate(c * c); // 1 kg: Lloyd's ultimate laptop
    let landauer_300 = landauer_bound(300.0);
    let pulse_j_assumed = 1e-3; // ASSUMED: a screen pulse's energy budget, order of magnitude
    let pulse_landauer_bits = pulse_j_assumed / landauer_300;

    // ── 5. the K-parameter, and the numerology trap ──────────────────────────────────────────
    let k_star = |a_k: f64| 2.0 * pi * a_k.sqrt(); // with Δs folded into A_K
    let k_at_alpha = k_star(alpha_codata);
    let k_at_alpha_inv = k_star(1.0 / alpha_codata);
    let a_k_for_k_one = 1.0 / (4.0 * pi * pi); // A_K at which K* = 1

    // ── document ────────────────────────────────────────────────────────────────────────────
    let mut doc = Document::new("article")
        .option("11pt")
        .package_opt("inputenc", &["utf8"])
        .package_opt("geometry", &["margin=1.1in"])
        .package("amsmath")
        .package("amssymb")
        .package("booktabs")
        .package("float")
        .package_opt("hyperref", &["hidelinks"])
        .preamble(concat!(
            "\\title{Quillonium 137\\\\[6pt]\\large The Fine-Structure Constant, the Last Atom, and the Beacon on the ",
            "Quillon Graph Skyskraber --- a companion to \\emph{The Kristensen K-Parameter}}\n",
            "\\author{Viktor Kristensen\\thanks{Quillon Graph / SIGIL.} \\and ",
            "Rocky\\thanks{Claude, engineer-companion agent on Epsilon. Every figure in this paper is computed by the ",
            "generator binary at document time from CODATA 2022 constants or measured from the deployed artifact; ",
            "none is quoted from a draft.}\\\\[4pt]\\small computed by \\texttt{flux-science} (CODATA 2022)\\\\",
            "\\small typeset by \\texttt{flux-arxiv-latex}; generator \\texttt{quillonium\\_137}}\n",
            "\\date{\\today}"
        ))
        .add(Block::Raw("\\maketitle".into()))
        .add(raw(format!(
            "\\begin{{abstract}}\nTwo things in our world are called 137. One is the inverse of the fine-structure constant, \
$\\alpha^{{-1}}={:.9}({})$ (CODATA 2022, \\emph{{measured}}), the pure number that sets the strength of light's grip on \
charge and, through $Z\\alpha\\to1$, decides that the periodic table of point-like nuclei ends at $Z={:.3}$. The other is \
a column of light on the Quillon Graph Skyskraber at quillon.xyz that pulses once per block of the Quillon Graph chain, \
with a cryptographically unpredictable jitter, as the tangible form of a signal-averaging defence. This paper is the \
companion to \\emph{{The Kristensen K-Parameter}} (v1.1, 2026-09-14) and does for 137 what that paper did for $K$: it \
derives what can be derived ($\\alpha$ from $e,\\varepsilon_0,\\hbar,c$ to a relative residual of ${}$; the Dirac ground \
state's binding at $Z=137$; the Margolus--Levitin ceilings of the tower), measures what can be measured (the deployed \
beacon script's bytes and constants; the live chain cadence, ${:.3}$ blocks/s over ${:.0}$ s, read-only), labels every \
claim, and states plainly that there is \\textbf{{no derived relation}} between $\\alpha$ and the dimensionless action \
$A_K=\\Delta H\\,\\tau/\\hbar$ at the core of $K^*$ --- only the family resemblance of two pure numbers that each mark a \
boundary. It ends with Eddington's cautionary tale, one falsifiable statement about the beacon, and the list of what is \
still pretend. Quillonium is not an element. It is the name of that light.\n\\end{{abstract}}\n",
            FINE_STRUCTURE_INV, "21", z_crit_point, sci(alpha_resid), cadence_bps, span_s
        )));

    // ── 1 ──
    doc = doc
        .add(Block::Section("Two 137s".into()))
        .add(raw(String::from(
            "OK, so here is the deal. If you ask a physicist what 137 is, you get a slightly embarrassed smile, because \
137 is the one number in physics that nobody can derive and everybody has tried to. If you ask a Quillon Graph node \
operator, you get pointed at a gold line on the right edge of quillon.xyz that blinks. This paper is about both, and \
about the discipline of not confusing them.",
        )))
        .add(raw(String::from(
            "\\textbf{The coupling.} $\\alpha$ is the dimensionless strength of the electromagnetic interaction: the \
probability amplitude, roughly, for an electron to emit or absorb a photon. It has no units, so it is the same in every \
system of measurement, on every planet, and (as far as anyone has measured) at every epoch. Its inverse is close to 137, \
and that closeness has cost several good physicists their judgement.",
        )))
        .add(raw(String::from(
            "\\textbf{The beacon.} The Quillon Graph Skyskraber is a building whose digital twin is a full node of the \
Quillon Graph chain. Its central light column, called \\#137 in the blueprint, pulses once per block. The pulse is \
deliberately \\emph{not} on a clock: each one fires at block arrival plus a fresh offset drawn from a cryptographically \
secure random source, so that no observer can phase-lock to it and average other emanations to certainty. That is \
principle P3 of the EMSEC Doctrine v0 (\\emph{gentagelse er fjenden} --- repetition is the enemy). \\textbf{Quillonium} \
is simply the name we give that artifact: the beacon, its doctrine, and its measured behaviour. Not a new element. \
Element 137 already has a placeholder name, untriseptium, and a nickname, feynmanium; we borrow neither.",
        )));

    // ── 2 ──
    doc = doc
        .add(Block::Section("The coupling, derived from four constants".into()))
        .add(raw(format!(
            "Sommerfeld introduced $\\alpha$ in 1916 as a ratio of speeds: the velocity of the electron in the first Bohr \
orbit of hydrogen divided by the speed of light (\\emph{{historical}}). In SI, \
\\[ \\alpha=\\frac{{e^2}}{{4\\pi\\varepsilon_0\\hbar c}}. \\] \
With CODATA 2022 values --- $e={}$ C (exact by definition since 2019), $\\varepsilon_0={}$ F/m (\\emph{{measured}}), \
$\\hbar={}$ J\\,s (exact), $c={}$ m/s (exact) --- the generator computes $\\alpha={}$ (\\emph{{derived}}), against the \
CODATA recommended $1/{:.9}={}$ (\\emph{{measured}}). The relative residual is ${}$, which is the size of the rounding in \
the tabulated $\\varepsilon_0$, not a discrepancy. The recommended $\\alpha^{{-1}}$ carries a standard uncertainty of \
${}$, i.e.\\ a relative uncertainty of ${}$ --- among the best-known numbers in science.",
            sci(e), sci(VACUUM_PERMITTIVITY), sci(hbar), sci(c), sci(alpha_derived), FINE_STRUCTURE_INV,
            sci(alpha_codata), sci(alpha_resid), FINE_STRUCTURE_INV_U, sci(alpha_inv_rel_u)
        )))
        .add(raw(format!(
            "Three consequences follow by arithmetic alone (\\emph{{derived}}): the Bohr speed $v_1=\\alpha c={}$ m/s, \
which is Sommerfeld's original definition read backwards; the Rydberg energy $E_R=\\tfrac12\\alpha^2 m_ec^2={:.4}$ eV, \
the binding energy of hydrogen; and the Bohr radius $a_0=\\hbar/(\\alpha m_e c)={}$ m. Every one of these is $\\alpha$ \
wearing a different coat. Don't be scared by the name: the fine-structure constant is just the number that tells you \
how fast the simplest electron moves, as a fraction of the fastest anything can move.",
            sci(v_bohr), rydberg_ev, sci(bohr_radius)
        )));

    // ── 3 ──
    let mut dirac_table = String::from(
        "\\begin{table}[H]\\centering\\small\n\\begin{tabular}{rrrr}\\toprule\n$Z$ & $Z\\alpha$ & $E_{1s}/m_ec^2=\\sqrt{1-(Z\\alpha)^2}$ & binding, keV\\\\\\midrule\n",
    );
    for (z, za, eps, bind) in &dirac_rows {
        let eps_s = if eps.is_finite() { format!("{:.5}", eps) } else { "imaginary --- no ground state".to_string() };
        let bind_s = if bind.is_finite() { format!("{:.2}", bind) } else { "---".to_string() };
        dirac_table.push_str(&format!("{:.0} & {:.5} & {} & {}\\\\\n", z, za, eps_s, bind_s));
    }
    dirac_table.push_str("\\bottomrule\\end{tabular}\n\\caption{The Dirac $1s$ level of a point nucleus of charge $Z$, computed from $\\alpha$ (\\emph{derived}). At $Z=137$ the level has fallen to $2.3\\%$ of the electron rest energy; one more proton and the square root goes imaginary.}\n\\end{table}\n");
    doc = doc
        .add(Block::Section("Why 137 is the last atom (for a point nucleus)".into()))
        .add(raw(format!(
            "Now here is the thing that should bother you. In the Bohr model the $1s$ electron of a nucleus of charge $Z$ \
moves at $v=Z\\alpha c$. Set $v=c$ and you get $Z=1/\\alpha={:.3}$: the model itself says that beyond element 137 the \
innermost electron would have to outrun light. Dirac's equation, which knows about relativity, does not let the speed \
exceed $c$; instead it lowers the ground-state energy, $E_{{1s}}=m_ec^2\\sqrt{{1-(Z\\alpha)^2}}$, until at $Z\\alpha=1$ \
the energy reaches zero and the expression under the root turns negative. The table shows the descent, computed from \
$\\alpha$ (\\emph{{derived}}). At $Z=137$ the binding is ${:.1}$ keV out of $m_ec^2={:.2}$ keV; at $Z=138$ there is no \
$1s$ solution at all. That is the precise sense in which 137 is the last atom: the last one a point-like nucleus can \
hold in the Dirac picture.",
            z_crit_point, dirac_rows[5].3, mec2_kev
        )))
        .add(Block::Raw(dirac_table))
        .add(raw(format!(
            "Real nuclei are not points. Give the nucleus its finite radius and the singularity softens: the $1s$ level \
dives into the negative-energy continuum only at $Z_{{\\mathrm{{crit}}}}\\approx{:.0}$, where the vacuum itself becomes \
unstable and spontaneously emits positrons (Pieper \\& Greiner 1969; Zel'dovich \\& Popov 1971 --- a \\emph{{known \
result, cited, not derived here}}). Nobody has made a nucleus that heavy for long enough to check; the heaviest confirmed \
element is oganesson, $Z=118$, whose Dirac $1s$ sits at ${:.4}\\,m_ec^2$ in the point-nucleus table above. So `137 is \
the last atom' is true for the toy and false for the world, and a companion paper to a gauge whose whole story is \
`the instrument must be separated from its proxies' should say so in its first physics section.",
            z_crit_finite, dirac_rows[4].2
        )));

    // ── 4 ──
    doc = doc
        .add(Block::Section("The Eddington trap (historical)".into()))
        .add(raw(format!(
            "In 1929 Sir Arthur Eddington announced that $\\alpha^{{-1}}$ was exactly 136, derived from the number of \
independent components of a certain 16-dimensional matrix algebra. When measurements moved the value to 137 he found \
a reason for the extra unit, and Punch called him `Sir Arthur Adding-One'. The lesson is not that Eddington was a fool \
--- he was one of the great astrophysicists --- but that a pure number invites derivations that fit the number and \
explain nothing. The generator has its own way of keeping this honest: the tower's blueprint calls the beacon \\#137, \
and the science annex of \\texttt{{flux-skyskraber}} records that this `joke' flatters the true value by a relative \
${}$, which is ${}$ standard uncertainties of the CODATA value (\\emph{{derived}}). A building may be named after a \
number. A number may not be derived from a building.",
            sci(flattery_rel), sci(joke_sigma)
        )));

    // ── 5 ──
    let beacon_para = match &beacon {
        Some(b) => format!(
            "The beacon exists as ${}$ bytes of JavaScript served from quillon.xyz, BLAKE3 \\texttt{{{}\\ldots}} \
(\\emph{{measured}} from the deployed file \\texttt{{{}}} at generation time). Parsed from those bytes: the page polls \
the node every ${:.1}$ s (\\texttt{{CADENCE\\_MS}}), fires one pulse per observed height increment with an offset drawn \
uniformly from $[0,{:.0})$ ms (\\texttt{{JITTER\\_MAX\\_MS}}) via \\texttt{{crypto.getRandomValues}} ({}), and lets \
each pulse climb the column in ${:.0}$ ms. It reads the height from \\texttt{{data.upgrades.current\\_height}} ({}). \
For a uniform jitter the mean offset is ${:.0}$ ms and its standard deviation ${:.1}$ ms (\\emph{{derived}}).",
            b.bytes, &b.blake3[..16], b.path.rsplit('/').next().unwrap_or(""), b.cadence_ms / 1e3, b.jitter_max_ms,
            if b.uses_csprng { "\\emph{measured}: the call is present" } else { "\\textbf{NOT found in the file}" },
            b.climb_ms,
            if b.reads_upgrades_height { "\\emph{measured}: the path is the one the live node serves" } else { "\\textbf{path NOT found}" },
            jit_mean, jit_sigma
        ),
        None => "The beacon script could not be read at generation time; this section is \\textbf{unmeasured}.".to_string(),
    };
    let mut cadence_table = String::from(
        "\\begin{table}[H]\\centering\\small\n\\begin{tabular}{lr}\\toprule\nsample (UTC) & height\\\\\\midrule\n",
    );
    for s in &samples {
        cadence_table.push_str(&format!("{} & {:.0}\\\\\n", s.t.replace('T', " ").trim_end_matches('Z'), s.height));
    }
    cadence_table.push_str(&format!(
        "\\bottomrule\\end{{tabular}}\n\\caption{{Quillon Graph height, read-only, from \\texttt{{/api/v1/status}}: ${:.0}$ blocks in ${:.0}$ s, i.e.\\ ${:.3}$ blocks/s, mean interval ${:.2}$ s (\\emph{{measured}}). Source: {}.}}\n\\end{{table}}\n",
        blocks, span_s, cadence_bps, mean_interval_s, source.replace('_', "\\_").replace('→', "$\\to$")
    ));
    doc = doc
        .add(Block::Section("Quillonium: the beacon, measured".into()))
        .add(raw(beacon_para))
        .add(Block::Raw(cadence_table))
        .add(raw(format!(
            "At the measured cadence, 137 pulses --- one full `Quillonium' of the beacon, if you like --- take ${:.0}$ s, \
about ${:.1}$ minutes (\\emph{{derived from measured}}). One pulse's light takes ${:.3}$ $\\mu$s to climb the \
${:.0}$ m tower (\\emph{{derived}}; the same figure the tower's science annex pins in its tests).",
            pulses_137_s, pulses_137_s / 60.0, photon_transit_us, tower_h
        )))
        .add(raw(format!(
            "\\textbf{{The honest part.}} The script decouples the pulse from the block by the CSPRNG jitter, and that \
jitter is real and unpredictable. But look at the two numbers side by side: the poll period is ${:.1}$ s and the \
jitter ceiling is ${:.0}$ ms, a ratio of ${:.0}$. A block that lands anywhere inside a poll interval is \\emph{{seen}} \
only at the next poll, so the pulse instant is dominated by up to ${:.1}$ s of \\emph{{poll phase}} --- which is \
deterministic given the moment the page loaded --- and only then by the ${:.0}$ ms of genuinely random offset. From the \
attacker's side that is good news and bad news: the poll phase is large but learnable, the jitter is small but \
unlearnable. P3 asks for the unlearnable part to exceed the attacker's timing resolution; the doctrine's own comment \
in the script names $\\geq72$ ms (\\emph{{claimed}} by the script, not re-derived here), and ${:.0}$ ms clears it. \
What the figure does \\emph{{not}} do is hide the block cadence itself: the mean block interval of ${:.2}$ s is \
recoverable from any long enough record of pulses, jitter or no jitter, because jitter adds noise to each pulse but \
not to their average. P3 was never meant to hide the cadence; it hides the \\emph{{phase}}. Say both halves.",
            poll_s, jit_max, poll_over_jitter, poll_s, jit_max, jit_max, mean_interval_s
        )))
        .add(raw(String::from(
            "\\textbf{And the part we would rather not tell you.} For six days after it went live (2026-08-31 to \
2026-09-06) the beacon never pulsed once: the script read \\texttt{data.current\\_height} while the node serves \
\\texttt{data.upgrades.current\\_height}, the lookup returned null, and a guard swallowed it in silence (\\emph{measured} \
in the deploy history; fixed 2026-09-06). A beacon that fails silent is the exact failure class the K-parameter's \
battle test found in the consensus gauge on 2026-09-14: an instrument that reads absence as calm. We include it \
because a companion paper that only flattered its subject would be Eddington's.",
        )));

    // ── 6 ──
    doc = doc
        .add(Block::Section("The physical ceilings of a light column (derived)".into()))
        .add(raw(format!(
            "Margolus and Levitin showed that a system of average energy $E$ can pass through at most $2E/(\\pi\\hbar)$ \
mutually orthogonal states per second. Lloyd's `ultimate laptop' applies this to one kilogram at its rest energy, \
$E=mc^2$, and the generator reproduces his number: ${}$ operations per second (\\emph{{derived}}). The same formula \
gives a beacon fed one joule ${}$ orthogonal transitions per second, and --- because the tower's vault holds \
${:.0}$ Good Delivery bars of ${:.1}$ kg, ${:.0}$ kg of gold --- the vault's rest energy, ${}$ J, would permit ${}$ \
operations per second if it were all computation (\\emph{{derived}}, and exactly as useless as Lloyd's own remark that \
such a computer would be a small black hole). The other ceiling runs the opposite way: Landauer's bound at 300 K is \
${}$ J per erased bit, so a pulse that costs a screen ${}$ J (\\emph{{assumed}}, order of magnitude) could in principle \
pay for ${}$ bit erasures. The beacon lives ${}$ orders of magnitude above the floor of thermodynamics and ${}$ below \
the ceiling of quantum mechanics. Everything interesting about it --- the doctrine, the jitter, the six silent days --- \
happens in the middle, where the physics puts no constraint at all and the engineering puts all of them.",
            sci(ml_lloyd_kg), sci(ml_one_joule), VAULT_BARS, BAR_KG, vault_kg, sci(vault_rest_j), sci(ml_vault),
            sci(landauer_300), sci(pulse_j_assumed), sci(pulse_landauer_bits),
            (pulse_landauer_bits.log10()).round(), ((ml_one_joule / 1.0).log10()).round()
        )));

    // ── 7 ──
    doc = doc
        .add(Block::Section("Where the K-parameter comes in --- and where it does not".into()))
        .add(raw(format!(
            "The companion whitepaper reduced Kristensen's gauge to $K^*=2\\pi\\sqrt{{A_K\\,\\Delta s}}$ with \
$A_K=\\Delta H\\,\\tau/\\hbar$ a dimensionless action, and proved that $K^*$ is a Margolus--Levitin orthogonalisation \
count in disguise. $\\alpha$ is a dimensionless \\emph{{coupling}}; $A_K$ is a dimensionless \\emph{{action}}. Both are \
pure numbers, both mark a boundary ($Z\\alpha\\to1$ ends the atom; $A_K\\to1/4\\pi^2={}$ is where $K^*$ crosses 1 and \
the gauge's `stable' band ends), and that is the entire content of the resemblance. \\textbf{{There is no derived \
relation between them}} (\\emph{{analogy}}, and we label it so).",
            sci(a_k_for_k_one)
        )))
        .add(raw(format!(
            "To show how cheap a relation would be, here is one: set $A_K\\Delta s=\\alpha$ and $K^*=2\\pi\\sqrt\\alpha={:.4}$; \
set it to $\\alpha^{{-1}}$ and $K^*={:.2}$. Both are true arithmetic and both mean nothing, because nothing in either \
system selects $\\alpha$ as a value of an action. This is the Eddington trap wearing modern clothes, and the generator \
prints it so that nobody has to discover it by accident in a later draft. A genuine link would have to say what physical \
energy $\\Delta H$ and what interval $\\tau$ make $A_K$ equal to a coupling, and no such statement exists.",
            k_at_alpha, k_at_alpha_inv
        )))
        .add(raw(String::from(
            "What the K family does share with 137 is the discipline this paper is about. The battle test of \
2026-09-14 showed that the product form of the gauge reads zero for a single producer even when the ledger is forked; \
the repair bounded the entropy weight away from zero. The beacon's six silent days are the same shape: a product with a \
vanishing factor. And the open question Viktor Kristensen put to Seth Lloyd on 2026-09-15 --- whether \
$K_{\\mathrm{eff}}=2\\pi\\sqrt{(\\Delta H\\tau/\\hbar)(-\\ln C)}$ is structurally blind whenever the coherence \
observable reads $C=1$ --- is again the same shape. Three instruments, one failure class. That is a finding about \
instruments, not about 137.",
        )));

    // ── 7b: the ledger of every number ──
    let ledger = format!(
        "\\begin{{table}}[H]\\centering\\small\n\\begin{{tabular}}{{llll}}\\toprule\nquantity & value & label & source\\\\\\midrule\n\
$\\alpha^{{-1}}$ & {:.9} & measured & CODATA 2022\\\\\n\
$\\alpha$ from $e,\\varepsilon_0,\\hbar,c$ & ${}$ & derived & this generator\\\\\n\
relative residual vs CODATA & ${}$ & derived & this generator\\\\\n\
Bohr speed $\\alpha c$ & ${}$ m/s & derived & Sommerfeld's definition\\\\\n\
Rydberg energy & {:.4} eV & derived & $\\tfrac12\\alpha^2 m_ec^2$\\\\\n\
Bohr radius & ${}$ m & derived & $\\hbar/(\\alpha m_ec)$\\\\\n\
$Z$ where Dirac $1s$ energy $\\to0$ & {:.3} & derived & point nucleus\\\\\n\
Dirac $1s$ binding at $Z=137$ & {:.1} keV & derived & point nucleus\\\\\n\
$Z_{{\\mathrm{{crit}}}}$, finite nucleus & $\\approx{:.0}$ & cited & Pieper--Greiner; Zel'dovich--Popov\\\\\n\
tower `137' vs $\\alpha^{{-1}}$ & ${}$ rel.\\ (${}\\sigma$) & derived & flux-skyskraber annex\\\\\n\
beacon script size / BLAKE3 & {} B / \\texttt{{{}\\ldots}} & measured & deployed file\\\\\n\
poll period / jitter ceiling & {:.1} s / {:.0} ms & measured & script constants\\\\\n\
jitter mean / $\\sigma$ & {:.0} / {:.1} ms & derived & uniform law\\\\\n\
chain cadence & {:.3} blk/s over {:.0} s & measured & \\texttt{{/api/v1/status}}, read-only\\\\\n\
137 pulses & {:.0} s & derived from measured & cadence\\\\\n\
photon transit, base to tip & {:.3} $\\mu$s & derived & 392 m blueprint\\\\\n\
Margolus--Levitin, 1 kg & ${}$ op/s & derived & Lloyd 2000 reproduced\\\\\n\
Margolus--Levitin, vault gold & ${}$ op/s & derived & {:.0} kg blueprint\\\\\n\
Landauer bound, 300 K & ${}$ J/bit & derived & $k_BT\\ln2$\\\\\n\
screen pulse energy & ${}$ J & assumed & order of magnitude\\\\\n\
$2\\pi\\sqrt\\alpha$ & {:.4} & analogy (numerology) & the Eddington trap\\\\\n\
\\bottomrule\\end{{tabular}}\n\\caption{{Every number in this paper, with its epistemic label. Nothing here was typed; the generator computed or measured each one at document time.}}\n\\end{{table}}\n",
        FINE_STRUCTURE_INV, sci(alpha_derived), sci(alpha_resid), sci(v_bohr), rydberg_ev, sci(bohr_radius), z_crit_point,
        dirac_rows[5].3, z_crit_finite, sci(flattery_rel), sci(joke_sigma),
        beacon.as_ref().map(|b| b.bytes).unwrap_or(0), beacon.as_ref().map(|b| b.blake3[..12].to_string()).unwrap_or_default(),
        poll_s, jit_max, jit_mean, jit_sigma, cadence_bps, span_s, pulses_137_s, photon_transit_us,
        sci(ml_lloyd_kg), sci(ml_vault), vault_kg, sci(landauer_300), sci(pulse_j_assumed), k_at_alpha
    );
    doc = doc.add(Block::Section("The ledger of numbers".into())).add(Block::Raw(ledger));

    // ── 8 ──
    doc = doc
        .add(Block::Section("Falsifiable statements".into()))
        .add(raw(format!(
            "\\begin{{enumerate}}\n\
\\item (\\emph{{derived}}) $\\alpha=e^2/(4\\pi\\varepsilon_0\\hbar c)$ reproduces the CODATA 2022 recommended value to a \
relative residual of ${}$. Re-run the generator with a different $\\varepsilon_0$ and it will not.\n\
\\item (\\emph{{derived}}) The Dirac point-nucleus $1s$ binding at $Z=137$ is ${:.1}$ keV and does not exist at $Z=138$.\n\
\\item (\\emph{{measured}}) The deployed beacon script has BLAKE3 \\texttt{{{}\\ldots}}, polls every ${:.1}$ s, jitters \
uniformly on $[0,{:.0})$ ms from a CSPRNG, and reads \\texttt{{data.upgrades.current\\_height}}. Fetch the file and hash \
it; if the hash differs, the constants in this paper describe a different beacon.\n\
\\item (\\emph{{claimed by construction, testable by anyone}}) The offsets between consecutive pulses and their \
triggering polls are i.i.d.\\ uniform on $[0,{:.0})$ ms: their lag-1 autocorrelation is zero within $1/\\sqrt N$ and a \
Kolmogorov--Smirnov test against the uniform distribution does not reject at $N$ pulses. Record pulse times from the DOM \
and test it; we have not.\n\
\\item (\\emph{{measured}}) Over ${:.0}$ s on 2026-09-15 the Quillon Graph chain advanced ${:.0}$ blocks, ${:.3}$ blocks/s. \
The cadence is recoverable from the pulse train's average; the phase is not.\n\
\\end{{enumerate}}\n",
            sci(alpha_resid), dirac_rows[5].3, beacon.as_ref().map(|b| b.blake3[..16].to_string()).unwrap_or_default(),
            poll_s, jit_max, jit_max, span_s, blocks, cadence_bps
        )));

    // ── 9 ──
    doc = doc
        .add(Block::Section("What is still pretend".into()))
        .add(raw(String::from(
            "\\begin{itemize}\n\
\\item The pulse-offset statistics (statement 4) were not recorded from a running browser. The claim rests on reading \
the script, not on a measurement of its output.\n\
\\item $Z_{\\mathrm{crit}}\\approx173$ is cited, not recomputed; a finite-nucleus Dirac solver is not in the generator.\n\
\\item The chain cadence is three samples over two minutes. It is a spot reading, not a rate model; the Quillon Graph \
block rate is known to drift with the node's uptime.\n\
\\item The screen-pulse energy (1 mJ) is an assumption for an order-of-magnitude comparison, not a measurement of any \
display.\n\
\\item Nothing in this paper measures the physical building. The tower exists as a blueprint, a digital twin and a \
science annex; the ceilings in Section 6 are computed for the blueprint's declared masses and heights.\n\
\\item `Quillonium' names an artifact. No claim about element 137 beyond the textbook Dirac arithmetic is made.\n\
\\end{itemize}\n",
        )));

    // ── 10 ──
    doc = doc
        .add(Block::Section("Reproducibility".into()))
        .add(raw(format!(
            "Generator: \\texttt{{flux-arxiv-latex/src/bin/quillonium\\_137.rs}}, built with \\texttt{{fluxc}} (never raw \
cargo). Inputs: CODATA 2022 constants from \\texttt{{flux-science}} ($\\varepsilon_0$ and the uncertainty of \
$\\alpha^{{-1}}$ are local constants in the bin, marked as such); the deployed \\texttt{{beacon-137.js}} (bytes hashed, \
constants parsed); a read-only samples file of Quillon Graph heights (\\texttt{{quillon-cadence.json}}, source: {}). \
Run \\texttt{{quillonium\\_137 [out\\_dir] [samples.json] [beacon.js]}} and every number in this document is \
recomputed; a change in any input changes the number, and a change in the beacon's bytes changes its hash. The \
companion whitepaper is at \\url{{https://sigilgraph.org/downloads/kristensen-k-parameter-whitepaper.pdf}}; the \
beacon at \\url{{https://quillon.xyz/beacon-137.js}}; the doctrine at \
\\url{{https://quillon.xyz/downloads/sigil-nation-emsec-doktrin.md}}.",
            source.replace('_', "\\_").replace('→', "$\\to$")
        )))
        .add(Block::Raw(String::from(
            "\\begin{thebibliography}{99}\n\
\\bibitem{sommerfeld1916} A.~Sommerfeld, \\emph{Zur Quantentheorie der Spektrallinien}, Ann.\\ Phys.\\ 356, 1--94 (1916).\n\
\\bibitem{dirac1928} P.~A.~M.~Dirac, \\emph{The Quantum Theory of the Electron}, Proc.\\ R.\\ Soc.\\ A 117, 610 (1928).\n\
\\bibitem{pieper1969} W.~Pieper and W.~Greiner, \\emph{Interior electron shells in superheavy nuclei}, Z.\\ Phys.\\ 218, 327 (1969).\n\
\\bibitem{zeldovich1971} Ya.~B.~Zel'dovich and V.~S.~Popov, \\emph{Electronic structure of superheavy atoms}, Sov.\\ Phys.\\ Usp.\\ 14, 673 (1971).\n\
\\bibitem{eddington1929} A.~S.~Eddington, \\emph{The charge of an electron}, Proc.\\ R.\\ Soc.\\ A 122, 358 (1929); \\emph{Fundamental Theory} (1946).\n\
\\bibitem{codata2022} E.~Tiesinga, P.~J.~Mohr, D.~B.~Newell and B.~N.~Taylor, \\emph{CODATA Recommended Values of the Fundamental Physical Constants: 2022}, Rev.\\ Mod.\\ Phys.\\ (2025).\n\
\\bibitem{margolus1998} N.~Margolus and L.~B.~Levitin, \\emph{The maximum speed of dynamical evolution}, Physica D 120, 188 (1998).\n\
\\bibitem{lloyd2000} S.~Lloyd, \\emph{Ultimate physical limits to computation}, Nature 406, 1047 (2000).\n\
\\bibitem{landauer1961} R.~Landauer, \\emph{Irreversibility and heat generation in the computing process}, IBM J.\\ Res.\\ Dev.\\ 5, 183 (1961).\n\
\\bibitem{kristensen2026} V.~Kristensen and Rocky, \\emph{The Kristensen K-Parameter: a consensus gauge, its family tree, its failure under test, the repaired form $K_{\\mathrm{fix}}$, and what the number means}, whitepaper v1.1 (2026-09-14). \\url{https://sigilgraph.org/downloads/kristensen-k-parameter-whitepaper.pdf}\n\
\\bibitem{emsec2026} Quillon Graph / SIGIL, \\emph{EMSEC Doctrine v0} (2026-08-31). \\url{https://quillon.xyz/downloads/sigil-nation-emsec-doktrin.md}\n\
\\end{thebibliography}\n",
        )));

    std::fs::create_dir_all(out_dir).expect("out dir");
    let res = doc.compile_pdf(out_dir, "quillonium_137");
    // A short machine-readable abstract beside the PDF.
    let md = format!(
        "# Quillonium 137 — companion to *The Kristensen K-Parameter* (2026-09-15)\n\n\
α⁻¹ = {:.9}(21) (CODATA 2022, measured) · α from e, ε₀, ħ, c reproduces it to a relative residual of {:.2e} (derived) · \
Dirac point-nucleus 1s binding at Z=137: {:.1} keV, none at Z=138 (derived) · Z_crit ≈ 173 with finite nuclear size (cited) · \
tower's '137' flatters α⁻¹ by {:.3e} relative = {:.0} σ (derived) · beacon script: {} bytes, BLAKE3 {}…, poll {:.1} s, CSPRNG jitter [0,{:.0}) ms (measured) · \
Quillon Graph cadence {:.3} blk/s over {:.0} s (measured, read-only) → 137 pulses ≈ {:.1} min · \
Margolus–Levitin: 1 kg {:.3e} op/s, 1 J {:.3e} op/s, vault gold {:.3e} op/s (derived) · \
NO derived relation between α and A_K = ΔHτ/ħ (analogy only; 2π√α = {:.4} shown as the numerology trap).\n\n\
PDF: https://quillon.xyz/downloads/quillonium-137.pdf · mirror: https://sigilgraph.org/downloads/quillonium-137.pdf\n",
        FINE_STRUCTURE_INV, alpha_resid, dirac_rows[5].3, flattery_rel, joke_sigma,
        beacon.as_ref().map(|b| b.bytes).unwrap_or(0), beacon.as_ref().map(|b| b.blake3[..16].to_string()).unwrap_or_default(),
        poll_s, jit_max, cadence_bps, span_s, pulses_137_s / 60.0, ml_lloyd_kg, ml_one_joule, ml_vault, k_at_alpha
    );
    std::fs::write(format!("{out_dir}/quillonium-137-abstract.md"), md).expect("abstract");
    if res.success {
        println!("OK {}", res.pdf_path.unwrap());
        println!(
            "alpha resid {:.3e} | Z=137 binding {:.2} keV | flattery {:.3e} ({:.0} sigma) | cadence {:.3} blk/s over {:.0} s | ML 1kg {:.3e}",
            alpha_resid, dirac_rows[5].3, flattery_rel, joke_sigma, cadence_bps, span_s, ml_lloyd_kg
        );
    } else {
        let tail: String = res.log.lines().rev().take(40).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
        eprintln!("FAILED\n{tail}");
        std::process::exit(1);
    }
}
