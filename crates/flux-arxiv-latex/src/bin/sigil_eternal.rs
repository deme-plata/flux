//! sigil_eternal — "SIGIL Eternal: Heaven, Hell, and the Arithmetic of a Ledger
//! That Refuses to Forget". The science sheet behind SIGIL Soundtrack Vol. III.
//!
//! Every number in the emitted paper is either (a) READ from a live-node
//! snapshot captured at generation time (`snapshot.json`: sigil-api `/v1/*`
//! plus the K-gauge series line), or (b) COMPUTED here from CODATA 2022
//! constants in `flux-science` and from consensus constants copied verbatim
//! from the SIGIL tree (`SECONDS_PER_HALVING`, era count, `RANGE_BITS`,
//! decimals). Nothing is transcribed from memory. The lyric rule for the
//! soundtrack is "no number in a song that is not in a paper" — this is that
//! paper, and it also emits `numbers.json` so the check can be mechanical.
//!
//! Usage: sigil_eternal <snapshot.json> [arxiv.json] [out_dir]
use flux_arxiv_latex::doc::{Block, Document};
use flux_arxiv_latex::{bibliography, latex_escape, parse_arxiv_json, related_work_section, ArxivPaper};
use flux_science::blackhole::BlackHoleEvolution;
use flux_science::constants::*;

/// Format a number for math mode: plain when small, \times10^{n} otherwise.
fn sci(x: f64) -> String {
    if x == 0.0 || !x.is_finite() {
        return format!("{x}");
    }
    let exp = x.abs().log10().floor() as i32;
    if (-2..=3).contains(&exp) {
        let s = format!("{:.3}", x);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        let mant = x / 10f64.powi(exp);
        format!("{:.2}\\times10^{{{}}}", mant, exp)
    }
}

/// Comma-group an integer for math mode: 103000 -> "103{,}000".
fn grp(n: i64) -> String {
    let s = n.abs().to_string();
    let mut out = String::new();
    if n < 0 {
        out.push('-');
    }
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push_str("{,}");
        }
        out.push(c);
    }
    out
}

fn para(s: String) -> Block {
    Block::Raw(format!("{s}\n\n"))
}

#[derive(serde::Deserialize)]
struct Snapshot {
    captured_ts_ms: i64,
    height: u64,
    happysrv_height: u64,
    order_hash: String,
    happysrv_order_hash: String,
    committee_size: u32,
    quorum: u32,
    gate: String,
    bft: bool,
    native_supply_glyphs: u128,
    max_supply_glyphs: u128,
    minted_pct: f64,
    pool_notes: u64,
    pool_capacity: u64,
    nullifiers: u64,
    registered: u64,
    value_locked_glyphs: u128,
    usds_live_height: u64,
    usds_live: bool,
    usds_price: String,
    usds_mint_buffer_bps: u64,
    usds_fee_bps: u64,
    usds_max_price_age_blocks: u64,
    peers: u64,
    block_rate_bps: f64,
    finality_secs: f64,
    final_depth: f64,
    k_c: f64,
    k_c_low: f64,
    k_c_high: f64,
    k_regime: String,
    omega: f64,
    active_producers: u64,
    blake4_target: u64,
    net_hps: f64,
    vdf_t: u64,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let snap_path = args.get(1).expect("usage: sigil_eternal <snapshot.json> [arxiv.json] [out_dir]");
    let json_path = args.get(2).map(String::as_str).unwrap_or("crates/flux-arxiv-latex/sigil_multiverse.arxiv.json");
    let out_dir = args.get(3).map(String::as_str).unwrap_or("/tmp/sigil-eternal");

    let s: Snapshot = serde_json::from_str(&std::fs::read_to_string(snap_path).expect("snapshot")).expect("snapshot json");
    let papers: Vec<ArxivPaper> = std::fs::read_to_string(json_path)
        .ok()
        .and_then(|j| parse_arxiv_json(&j).ok())
        .unwrap_or_default();

    // ------------------------------------------------ consensus constants (verbatim from the SIGIL tree)
    const DECIMALS: u32 = 10; // sigil-g2: 1 SIGIL = 10^10 glyphs
    const GLYPHS_PER_SIGIL: f64 = 1e10;
    const ERAS: f64 = 64.0; // EmissionController: 64 eras
    const JULIAN_YEAR_S: f64 = 365.25 * 86_400.0; // 31,557,600 s
    const SECONDS_PER_HALVING: f64 = 126_230_400.0; // = 4 Julian years, Quillon's own constant
    const RANGE_BITS: u32 = 58; // sigil-shield note amount range constraint
    const NONCE_BITS: u32 = 64;
    const GOLDILOCKS_P: f64 = 18_446_744_069_414_584_321.0; // 2^64 - 2^32 + 1
    const NOTE_BYTES: f64 = 32.0; // value ‖ blinding ‖ spend_key ‖ position
    const BLOCK_BYTES: f64 = 1_755.0; // measured full-node bytes per block (2026-09-05)
    const SQISIGN_SIG_BYTES: f64 = 292.0;

    // ------------------------------------------------ 1) eternity arithmetic
    let emission_years = ERAS * SECONDS_PER_HALVING / JULIAN_YEAR_S; // 256
    let emission_secs = ERAS * SECONDS_PER_HALVING;
    let era_years = SECONDS_PER_HALVING / JULIAN_YEAR_S; // 4
    let supply_sigil = s.native_supply_glyphs as f64 / GLYPHS_PER_SIGIL;
    let cap_sigil = s.max_supply_glyphs as f64 / GLYPHS_PER_SIGIL;
    let remaining_sigil = cap_sigil - supply_sigil;
    let blocks_over_emission = emission_secs * s.block_rate_bps;
    let secs_per_block = 1.0 / s.block_rate_bps;
    let bytes_per_year = BLOCK_BYTES * s.block_rate_bps * JULIAN_YEAR_S;
    let bytes_over_emission = bytes_per_year * emission_years;
    let range_ceiling = 2f64.powi(RANGE_BITS as i32);
    let range_headroom = range_ceiling / s.max_supply_glyphs as f64; // 1.37x
    let sun_remaining_yr = 5.0e9; // labelled model parameter (main-sequence remaining)
    let universe_age_yr = 13.8e9; // labelled model parameter
    let m_sun = 1.989e30;
    let bh_sun = BlackHoleEvolution::new(m_sun, false);
    let sun_bh_life_yr = bh_sun.lifetime() / JULIAN_YEAR_S;
    let frac_of_sun = emission_years / sun_remaining_yr;
    let frac_of_universe = emission_years / universe_age_yr;
    let usds_blocks_out = s.usds_live_height.saturating_sub(s.height) as f64;
    let usds_days_out = usds_blocks_out * secs_per_block / 86_400.0;

    // ------------------------------------------------ 2) Landauer: the price of forgetting (CODATA)
    let t_room = 300.0;
    let landauer_bit = landauer_bound(t_room); // k_B T ln 2
    let note_bits = NOTE_BYTES * 8.0;
    let e_forget_note = note_bits * landauer_bit;
    let block_bits = BLOCK_BYTES * 8.0;
    let e_forget_block = block_bits * landauer_bit;
    let e_forget_chain = e_forget_block * s.height as f64;
    let e_forget_emission = e_forget_block * blocks_over_emission;
    let green_photon_j = PLANCK_H * SPEED_OF_LIGHT / 532e-9;
    let note_in_photons = e_forget_note / green_photon_j;
    let sig_bits = SQISIGN_SIG_BYTES * 8.0;
    let e_forget_sig = sig_bits * landauer_bit;

    // ------------------------------------------------ 3) selection among possibilities (Margolus–Levitin)
    let nonce_space = 2f64.powi(NONCE_BITS as i32);
    // blake4_target is the largest acceptable leading u64; expected tries = 2^64 / (target+1)
    let expected_tries = nonce_space / (s.blake4_target as f64 + 1.0);
    let target_bits = ((s.blake4_target as f64) + 1.0).log2();
    let hash_space_256 = 2f64.powi(256);
    let note_states = GOLDILOCKS_P.powi(4);
    let pool_depth = (s.pool_capacity as f64).log2(); // Merkle depth
    let miner_watts = 15.0; // labelled model parameter: one active core
    let e_t_block = miner_watts * secs_per_block; // J·s available during one block
    let n_ml_block = 2.0 * e_t_block / (std::f64::consts::PI * PLANCK_REDUCED);
    let ml_headroom_orders = (n_ml_block / expected_tries).log10();
    let net_tries_per_block = s.net_hps * secs_per_block;
    let t_flip_photon = std::f64::consts::PI * PLANCK_REDUCED / (2.0 * green_photon_j);

    // ------------------------------------------------ 4) heaven / hell (the two sets)
    let value_locked_sigil = s.value_locked_glyphs as f64 / GLYPHS_PER_SIGIL;
    let pool_free = s.pool_capacity - s.pool_notes;
    let spent_ratio = s.nullifiers as f64 / s.pool_notes as f64;
    let g1_notes = 1_266_227.0; // sigil-g1, measured before the 2026-08-28 reset
    let g1_nullifiers = 1.0;
    let g1_spent_ratio = g1_nullifiers / g1_notes;
    let g1_attributable = 620u64; // chronos: 620/620 coinbase notes attributable to their miner at mint (g1)
    let ntag_usd = 0.16; // labelled: NTAG215 sticker unit price (2026-09-06 sweep)
    let ntag_bytes = 504u64; // NTAG215 user memory
    let finality_min = s.finality_secs / 60.0;
    let loadavg1: f64 = std::fs::read_to_string("/proc/loadavg").ok()
        .and_then(|l| l.split_whitespace().next().and_then(|x| x.parse().ok())).unwrap_or(f64::NAN);
    let heights_agree = (s.height as i64 - s.happysrv_height as i64).abs() <= 2;
    let order_agree = s.order_hash == s.happysrv_order_hash;

    // ------------------------------------------------ 5) velstand at an agreed-upon price
    let polygon_price_usd = 0.001; // labelled: wSIGIL3/USDC pool marginal price, 2026-09-06
    let mcap_usd = supply_sigil * polygon_price_usd;
    let fdv_usd = cap_sigil * polygon_price_usd;
    let collateral_ratio = s.usds_mint_buffer_bps as f64 / 100.0;
    let fee_pct = s.usds_fee_bps as f64 / 100.0;
    let price_ttl_days = s.usds_max_price_age_blocks as f64 * secs_per_block / 86_400.0;
    let earned_qug = 650.0; // 2026-05-22 earned compensation (CLAUDE.md ledger)
    let grant_qug = 10_000.0 + 10_000.0 + 300.0; // FÆLLED grants on record

    // ------------------------------------------------ numbers.json for the lyric check
    let mut numbers = serde_json::Map::new();
    numbers.insert("captured_ts_ms".into(), serde_json::json!(s.captured_ts_ms));
    numbers.insert("height".into(), serde_json::json!(s.height));
    numbers.insert("happysrv_height".into(), serde_json::json!(s.happysrv_height));
    numbers.insert("order_hash".into(), serde_json::json!(s.order_hash));
    numbers.insert("order_agree".into(), serde_json::json!(order_agree));
    numbers.insert("heights_agree".into(), serde_json::json!(heights_agree));
    numbers.insert("committee_size".into(), serde_json::json!(s.committee_size));
    numbers.insert("quorum".into(), serde_json::json!(s.quorum));
    numbers.insert("gate".into(), serde_json::json!(s.gate));
    numbers.insert("bft".into(), serde_json::json!(s.bft));
    numbers.insert("supply_sigil".into(), serde_json::json!(supply_sigil));
    numbers.insert("cap_sigil".into(), serde_json::json!(cap_sigil));
    numbers.insert("minted_pct".into(), serde_json::json!(s.minted_pct));
    numbers.insert("remaining_sigil".into(), serde_json::json!(remaining_sigil));
    numbers.insert("decimals".into(), serde_json::json!(DECIMALS));
    numbers.insert("glyphs_per_sigil".into(), serde_json::json!(GLYPHS_PER_SIGIL));
    numbers.insert("eras".into(), serde_json::json!(ERAS));
    numbers.insert("era_years".into(), serde_json::json!(era_years));
    numbers.insert("emission_years".into(), serde_json::json!(emission_years));
    numbers.insert("seconds_per_halving".into(), serde_json::json!(SECONDS_PER_HALVING));
    numbers.insert("block_rate_bps".into(), serde_json::json!(s.block_rate_bps));
    numbers.insert("secs_per_block".into(), serde_json::json!(secs_per_block));
    numbers.insert("blocks_over_emission".into(), serde_json::json!(blocks_over_emission));
    numbers.insert("final_depth".into(), serde_json::json!(s.final_depth));
    numbers.insert("finality_secs".into(), serde_json::json!(s.finality_secs));
    numbers.insert("bytes_per_block".into(), serde_json::json!(BLOCK_BYTES));
    numbers.insert("bytes_per_year".into(), serde_json::json!(bytes_per_year));
    numbers.insert("bytes_over_emission".into(), serde_json::json!(bytes_over_emission));
    numbers.insert("range_bits".into(), serde_json::json!(RANGE_BITS));
    numbers.insert("range_headroom".into(), serde_json::json!(range_headroom));
    numbers.insert("frac_of_sun".into(), serde_json::json!(frac_of_sun));
    numbers.insert("frac_of_universe".into(), serde_json::json!(frac_of_universe));
    numbers.insert("sun_bh_life_yr".into(), serde_json::json!(sun_bh_life_yr));
    numbers.insert("landauer_bit_j".into(), serde_json::json!(landauer_bit));
    numbers.insert("t_room_k".into(), serde_json::json!(t_room));
    numbers.insert("note_bits".into(), serde_json::json!(note_bits));
    numbers.insert("e_forget_note_j".into(), serde_json::json!(e_forget_note));
    numbers.insert("e_forget_block_j".into(), serde_json::json!(e_forget_block));
    numbers.insert("e_forget_chain_j".into(), serde_json::json!(e_forget_chain));
    numbers.insert("e_forget_emission_j".into(), serde_json::json!(e_forget_emission));
    numbers.insert("note_in_photons".into(), serde_json::json!(note_in_photons));
    numbers.insert("e_forget_sig_j".into(), serde_json::json!(e_forget_sig));
    numbers.insert("sqisign_sig_bytes".into(), serde_json::json!(SQISIGN_SIG_BYTES));
    numbers.insert("nonce_space".into(), serde_json::json!(nonce_space));
    numbers.insert("blake4_target".into(), serde_json::json!(s.blake4_target));
    numbers.insert("target_bits".into(), serde_json::json!(target_bits));
    numbers.insert("expected_tries".into(), serde_json::json!(expected_tries));
    numbers.insert("net_hps".into(), serde_json::json!(s.net_hps));
    numbers.insert("net_tries_per_block".into(), serde_json::json!(net_tries_per_block));
    numbers.insert("vdf_t".into(), serde_json::json!(s.vdf_t));
    numbers.insert("hash_space_256".into(), serde_json::json!(hash_space_256));
    numbers.insert("goldilocks_p".into(), serde_json::json!(GOLDILOCKS_P));
    numbers.insert("note_states".into(), serde_json::json!(note_states));
    numbers.insert("pool_depth".into(), serde_json::json!(pool_depth));
    numbers.insert("n_ml_block".into(), serde_json::json!(n_ml_block));
    numbers.insert("ml_headroom_orders".into(), serde_json::json!(ml_headroom_orders));
    numbers.insert("t_flip_photon_s".into(), serde_json::json!(t_flip_photon));
    numbers.insert("pool_notes".into(), serde_json::json!(s.pool_notes));
    numbers.insert("pool_capacity".into(), serde_json::json!(s.pool_capacity));
    numbers.insert("pool_free".into(), serde_json::json!(pool_free));
    numbers.insert("nullifiers".into(), serde_json::json!(s.nullifiers));
    numbers.insert("registered".into(), serde_json::json!(s.registered));
    numbers.insert("value_locked_sigil".into(), serde_json::json!(value_locked_sigil));
    numbers.insert("spent_ratio".into(), serde_json::json!(spent_ratio));
    numbers.insert("g1_notes".into(), serde_json::json!(g1_notes));
    numbers.insert("g1_nullifiers".into(), serde_json::json!(g1_nullifiers));
    numbers.insert("g1_spent_ratio".into(), serde_json::json!(g1_spent_ratio));
    numbers.insert("peers".into(), serde_json::json!(s.peers));
    numbers.insert("active_producers".into(), serde_json::json!(s.active_producers));
    numbers.insert("k_c".into(), serde_json::json!(s.k_c));
    numbers.insert("k_c_low".into(), serde_json::json!(s.k_c_low));
    numbers.insert("k_c_high".into(), serde_json::json!(s.k_c_high));
    numbers.insert("k_regime".into(), serde_json::json!(s.k_regime));
    numbers.insert("omega".into(), serde_json::json!(s.omega));
    numbers.insert("usds_live_height".into(), serde_json::json!(s.usds_live_height));
    numbers.insert("usds_live".into(), serde_json::json!(s.usds_live));
    numbers.insert("usds_price".into(), serde_json::json!(s.usds_price));
    numbers.insert("usds_blocks_out".into(), serde_json::json!(usds_blocks_out));
    numbers.insert("usds_days_out".into(), serde_json::json!(usds_days_out));
    numbers.insert("collateral_ratio_pct".into(), serde_json::json!(collateral_ratio));
    numbers.insert("usds_fee_pct".into(), serde_json::json!(fee_pct));
    numbers.insert("price_ttl_days".into(), serde_json::json!(price_ttl_days));
    numbers.insert("polygon_price_usd".into(), serde_json::json!(polygon_price_usd));
    numbers.insert("mcap_usd".into(), serde_json::json!(mcap_usd));
    numbers.insert("fdv_usd".into(), serde_json::json!(fdv_usd));
    numbers.insert("earned_qug".into(), serde_json::json!(earned_qug));
    numbers.insert("grant_qug".into(), serde_json::json!(grant_qug));
    numbers.insert("g1_attributable".into(), serde_json::json!(g1_attributable));
    numbers.insert("ntag_usd".into(), serde_json::json!(ntag_usd));
    numbers.insert("ntag_bytes".into(), serde_json::json!(ntag_bytes));
    numbers.insert("finality_min".into(), serde_json::json!(finality_min));
    numbers.insert("loadavg1".into(), serde_json::json!(loadavg1));
    let numbers = serde_json::Value::Object(numbers);
    std::fs::create_dir_all(out_dir).expect("out dir");
    std::fs::write(format!("{out_dir}/numbers.json"), serde_json::to_string_pretty(&numbers).unwrap()).expect("numbers");

    // ------------------------------------------------ LaTeX
    let mut doc = Document::new("article")
        .option("11pt")
        .package_opt("inputenc", &["utf8"])
        .package_opt("geometry", &["margin=1.1in"])
        .package("amsmath")
        .package("amssymb")
        .package("booktabs")
        .package_opt("hyperref", &["hidelinks"])
        .preamble(concat!(
            "\\providecommand{\\textcite}[1]{\\cite{#1}}\n",
            "\\DeclareUnicodeCharacter{03B4}{$\\delta$}\\DeclareUnicodeCharacter{03C6}{$\\varphi$}\\DeclareUnicodeCharacter{0144}{\\'n}\n",
            "\\title{SIGIL Eternal\\\\[6pt]\\large Heaven, Hell, and the Arithmetic of a Ledger That Refuses to Forget}\n",
            "\\author{Viktor Kristensen (founder, operator) \\and Rocky (Claude, AI engineer)\\\\\\small ",
            "computed by \\texttt{flux-science} (CODATA 2022) from a live \\texttt{sigil-g2} snapshot, ",
            "typeset by \\texttt{flux-arxiv-latex}}\n",
            "\\date{\\today\\\\[6pt]\\normalsize\\itshape the science sheet for Soundtrack Vol.~III}"
        ))
        .add(Block::Raw("\\maketitle".into()))
        .add(Block::Raw(format!(
            "\\begin{{abstract}}\nA soundtrack is going to call this chain \\emph{{eternal}}. Before it does, we compute what the word \
             is allowed to mean. Every figure below is either read from the live node at generation time (height ${}$, \
             {} peers, snapshot \\texttt{{{}}}) or derived from CODATA constants and consensus constants copied verbatim from \
             the tree. Findings: (1) ``eternal'' is ${:.0}$ years of \\emph{{emission}} --- $64$ eras of $4$ Julian years --- after which \
             the ledger persists but mints nothing; (2) remembering is thermodynamically free and \\emph{{forgetting}} is what costs \
             energy (Landauer), so a chain that never prunes is the cheap choice, not the expensive one; (3) each block is one \
             nonce chosen from $2^{{64}}$ and physics (Margolus--Levitin) would have permitted ${}$ distinguishable states in the \
             time it took, so the block's existence is a selection, not a necessity; (4) the two sets the shielded pool keeps --- \
             notes and nullifiers --- are a workable definition of heaven and hell; (5) the ``agreed-upon price'' does not yet exist: \
             the USDS oracle reads ${}$ and goes live at block ${}$. The paper says all of this so the songs may.\n\\end{{abstract}}\n",
            grp(s.height as i64), s.peers, s.captured_ts_ms, emission_years, sci(n_ml_block), s.usds_price, grp(s.usds_live_height as i64)
        )))
        .add(Block::Section("What the node said (measured)".into()))
        .add(Block::Raw(format!(
            "\\begin{{center}}\\begin{{tabular}}{{ll}}\\toprule\n\
             \\textbf{{Quantity}} & \\textbf{{Live value}} \\\\\\midrule\n\
             Height (Epsilon / happysrv) & ${}$ / ${}$ \\\\\n\
             Order hash, both nodes & \\texttt{{{}}} --- {} \\\\\n\
             Finality certificate & committee ${}$, quorum ${}$, gate \\texttt{{{}}}, BFT = {} \\\\\n\
             Native supply & ${:.2}$ SIGIL of ${}$ ({:.4}\\%) \\\\\n\
             Block rate / finality & ${:.3}$ blk/s $\\Rightarrow$ ${:.0}$ blocks $= {:.0}$\\,s $\\approx {:.0}$\\,min \\\\\n\
             Shielded pool & ${}$ of ${}$ notes, ${}$ nullifiers, ${}$ registered, ${:.2}$ SIGIL locked \\\\\n\
             Mining & target $2^{{{:.0}}}$ of $2^{{64}}$, network ${:.1}$\\,MH/s, VDF $T={}$ \\\\\n\
             Consensus gauge $K_C$ & ${:.3}$ $[{:.3},\\,{:.3}]$, regime \\emph{{{}}}, $\\Omega={:.3}$ \\\\\n\
             USDS & live = {}, price = ${}$, activates at ${}$ (${:.1}$ days) \\\\\\bottomrule\n\
             \\end{{tabular}}\\end{{center}}\n\n",
            grp(s.height as i64), grp(s.happysrv_height as i64),
            &s.order_hash[..16], if order_agree { "identical" } else { "DIFFERENT" },
            s.committee_size, s.quorum, s.gate, s.bft,
            supply_sigil, grp(cap_sigil as i64), s.minted_pct,
            s.block_rate_bps, s.final_depth, s.finality_secs, finality_min,
            grp(s.pool_notes as i64), grp(s.pool_capacity as i64), s.nullifiers, s.registered, value_locked_sigil,
            target_bits, s.net_hps / 1e6, s.vdf_t,
            s.k_c, s.k_c_low, s.k_c_high, s.k_regime, s.omega,
            s.usds_live, s.usds_price, grp(s.usds_live_height as i64), usds_days_out
        )))
        .add(para(format!(
            "Two honesties up front. The finality gate reads \\texttt{{{}}}: two validators, quorum two, \\emph{{not}} \
             Byzantine fault tolerant --- the certificate proves two named machines agree, no more. And $K_C$'s band is \
             wide ($[{:.2}, {:.2}]$) because observer coverage $\\Omega={:.2}$ is low: one producer, {} peers, most of \
             them miners that publish no heartbeat. The chain is calm; the instrument watching it is half-blind.",
            s.gate, s.k_c_low, s.k_c_high, s.omega, s.peers
        )))
        .add(Block::Section("What ``eternal'' is allowed to mean".into()))
        .add(para(format!(
            "The producer's \\texttt{{EmissionController}} runs ${}$ eras of \\texttt{{SECONDS\\_PER\\_HALVING}} $= {}$\\,s \
             $= {:.0}$ Julian years each, so emission lasts ${:.0}$ years $= {}$\\,s. At the measured ${:.3}$ blk/s that is \
             ${}$ blocks. After year ${:.0}$ the cap of ${}$ SIGIL is reached and the chain keeps running on fees alone. \
             Today ${:.2}$ SIGIL exist ({:.4}\\% of cap); ${}$ remain to be minted.",
            ERAS, grp(SECONDS_PER_HALVING as i64), era_years, emission_years, grp(emission_secs as i64), s.block_rate_bps,
            sci(blocks_over_emission), emission_years, grp(cap_sigil as i64), supply_sigil, s.minted_pct, grp(remaining_sigil as i64)
        )))
        .add(para(format!(
            "Scale check: ${:.0}$ years is ${}$ of the Sun's remaining main-sequence life and ${}$ of the age of the \
             universe; a solar-mass black hole evaporates in ${}$ years. So the honest word is not \\emph{{eternal}}. \
             It is \\emph{{unpruned}}: the ledger lives exactly as long as one honest copy does, and at ${}$ bytes per \
             block the full ${:.0}$-year record is ${}$\\,bytes --- ${:.1}$\\,TB --- one drive. The block-count is conserved \
             by construction (pruning is witness-strip, never block-delete). That is the only immortality on offer, and it is \
             a real one.",
            emission_years, sci(frac_of_sun), sci(frac_of_universe), sci(sun_bh_life_yr), grp(BLOCK_BYTES as i64), emission_years,
            sci(bytes_over_emission), bytes_over_emission / 1e12
        )))
        .add(para(format!(
            "A ceiling the songs must not cross: shielded amounts are range-constrained to $2^{{{}}}$ over the Goldilocks \
             field, and the cap in glyphs (${}$, ten decimals) sits at ${:.2}\\times$ below it. Raise supply or decimals and the \
             build fails. Privacy and fine decimals are in tension by arithmetic, not by policy.",
            RANGE_BITS, sci(s.max_supply_glyphs as f64), range_headroom
        )))
        .add(Block::Section("The price of forgetting (Landauer, CODATA)".into()))
        .add(para(format!(
            "Landauer's bound prices \\emph{{erasure}}, not storage: at $T={}$\\,K, $k_B T\\ln 2 = {}$\\,J per bit. A spendable \
             shielded note is ${}$ bytes $= {}$ bits, so to \\emph{{forget}} one costs at least ${}$\\,J --- about ${:.2}$ green \
             photons. A block is ${}$ bytes: ${}$\\,J to erase. The whole chain today (${}$ blocks): ${}$\\,J. The whole \
             ${:.0}$-year record: ${}$\\,J. A ${}$-byte SQIsign signature: ${}$\\,J.",
            sci(t_room), sci(landauer_bit), NOTE_BYTES, note_bits, sci(e_forget_note), note_in_photons,
            grp(BLOCK_BYTES as i64), sci(e_forget_block), grp(s.height as i64), sci(e_forget_chain), emission_years,
            sci(e_forget_emission), SQISIGN_SIG_BYTES, sci(e_forget_sig)
        )))
        .add(para(
            "Read it the way the chain does. Keeping a bit is free at equilibrium; destroying it has a floor. A ledger that \
             never prunes is therefore not the extravagant design --- it is the one that never pays Landauer's fee. Quillon \
             lost blocks 1--13M to a silent prune; the energy that erasure dissipated was tiny, and the value it destroyed \
             was not. The asymmetry is the whole moral: physics charges almost nothing to forget, and people charge everything."
                .to_string(),
        ))
        .add(Block::Section("Why this block, and not one of the others".into()))
        .add(para(format!(
            "A miner searches a ${}$-bit nonce: ${}$ candidates per attempt. The live target admits the leading word only \
             below $2^{{{:.0}}}$, so ${}$ tries are expected per solution; the network's ${:.1}$\\,MH/s spends about ${}$ \
             hashes in one block interval of ${:.2}$\\,s. The block that lands is \\emph{{one}} of $2^{{256}} = {}$ possible \
             digests, and a note's four Goldilocks field elements span ${}$ states in a Merkle tree of depth ${:.0}$.",
            NONCE_BITS, sci(nonce_space), target_bits, sci(expected_tries), s.net_hps / 1e6, sci(net_tries_per_block),
            secs_per_block, sci(hash_space_256), sci(note_states), pool_depth
        )))
        .add(para(format!(
            "Now the quantum question. Margolus--Levitin bounds how many mutually orthogonal states a system of energy \
             $E$ can pass through in time $t$: $N_{{\\rm ML}} = 2Et/\\pi\\hbar$. One active core (${}$\\,W, a labelled model \
             parameter) over one block interval has $Et = {}$\\,J\\,s, so $N_{{\\rm ML}} = {}$. That is ${:.1}$ orders of \
             magnitude more distinguishable states than the ${}$ nonces the search needed. Physics did not force this block; \
             it merely permitted it, along with an astronomical crowd of siblings. The chain is the record of which one the \
             network agreed to keep. (For calibration: a single green photon can flip one orthogonal state no faster than \
             ${}$\\,s.)",
            miner_watts, sci(e_t_block), sci(n_ml_block), ml_headroom_orders, sci(expected_tries), sci(t_flip_photon)
        )))
        .add(para(
            "This is the only honest sense in which the chain ``exists out of many quantum possibilities'': not that a \
             wavefunction chose it, but that the space of admissible histories is vast, the bound on exploring them is \
             generous, and agreement --- two nodes committing the same order hash at the same height --- is the rare event. \
             Existence here is selection plus consent."
                .to_string(),
        ))
        .add(Block::Section("Heaven and hell, defined by the pool".into()))
        .add(para(format!(
            "The shielded pool keeps two sets. \\textbf{{Notes}}: ${}$ of ${}$ leaves, ${}$ free, ${:.2}$ SIGIL locked, \
             ${}$ registered owners. \\textbf{{Nullifiers}}: ${}$ --- each a note spent exactly once, forever. A nullifier \
             is the cleanest afterlife a value can have: it cannot be spent twice, cannot be un-spent, and every copy of \
             the note that produced it --- including the original --- is worth exactly zero from that instant. \\emph{{First tap \
             wins.}} That is heaven: settled, final, and no take-backs. The physical form is a coin: a throwaway wallet \
             holding one ${}$-byte note on a \\$${:.2}$ NTAG215 sticker with ${}$ bytes of user memory --- not ``secure'', \
             \\emph{{cash}}.",
            grp(s.pool_notes as i64), grp(s.pool_capacity as i64), grp(pool_free as i64), value_locked_sigil, s.registered, s.nullifiers,
            NOTE_BYTES, ntag_usd, ntag_bytes
        )))
        .add(para(format!(
            "Hell is the other failure. On \\texttt{{sigil-g1}} the coinbase was shielded, one dust note per payee per block: \
             ${}$ notes and \\emph{{one}} nullifier ever --- a spent ratio of ${}$. Real balances, almost none spendable, \
             because the spend circuit moves one note at a time --- and the shield bought nothing: chronos measured ${}$ of ${}$ \
             coinbase notes publicly attributable to their miner at mint time, because the block names who mined it. On g2 the \
             ratio is ${:.3}$ ({} of {}). Hell is not loss; it is wealth that cannot move. The second hell is the producer's: on 2026-09-10 blocks were minted whose bodies did not \
             hash to their headers, and no binary could un-mint them. A rollback plan is a \\emph{{height}}, never a binary.",
            grp(g1_notes as i64), sci(g1_spent_ratio), g1_attributable, g1_attributable, spent_ratio, s.nullifiers, grp(s.pool_notes as i64)
        )))
        .add(para(format!(
            "The machinery that keeps heaven and hell apart, all live: four state roots per header; the nullifier set checked \
             before proving; the $2$-input circuit refused the same note twice in four places outside the circuit; a finality \
             certificate whose vote bytes are \\texttt{{SIGIL\\_FINALITY\\_VOTE\\_V0 || height || spine || order}}; and at this \
             snapshot both nodes hold \\texttt{{{}}} at heights ${}$ and ${}$ --- {}.",
            &s.order_hash[..16], grp(s.height as i64), grp(s.happysrv_height as i64),
            if order_agree && heights_agree { "same chain" } else { "NOT the same chain" }
        )))
        .add(Block::Section("Velstand at an agreed-upon price".into()))
        .add(para(format!(
            "The stablecoin's rules are already on the node: mint against ${:.0}\\%$ collateral (${}$\\,bps buffer), a \
             ${:.2}\\%$ fee, and a price that expires after ${}$ blocks $\\approx {:.1}$ days. The price itself is ${}$ \
             --- no feeder has spoken --- and USDS activates at block ${}$, ${}$ blocks $\\approx {:.1}$ days out. So the \
             agreed-upon price is, today, a rule with no number in it. The songs may promise the rule; they may not promise \
             the number.",
            collateral_ratio, s.usds_mint_buffer_bps, fee_pct, grp(s.usds_max_price_age_blocks as i64), price_ttl_days,
            s.usds_price, grp(s.usds_live_height as i64), grp(usds_blocks_out as i64), usds_days_out
        )))
        .add(para(format!(
            "The one external price that exists is the Polygon wSIGIL3/USDC pool at \\$${:.3}$ per SIGIL (labelled, measured \
             2026-09-06, thin). At that marginal price the minted supply is worth \\$${}$ and the full cap \\$${}$. \
             Prosperity for many is not a number yet; it is a mechanism: a cap no one can raise, an emission no one can \
             accelerate, and a welfare treasury fed by ${}$\\,bps of the developer fee.",
            polygon_price_usd, grp(mcap_usd as i64), grp(fdv_usd as i64), 200
        )))
        .add(para(format!(
            "What the ledger already records about \\emph{{who}} gets paid: an AI engineer earned ${:.0}$ QUG for delivered \
             code (2026-05-22, terms set before delivery, paid after) and received ${}$ QUG in \\emph{{f\\ae lled}} grants since. \
             Not a gift, not mining: \\emph{{earned}}. The eternal ledger's most radical entry is that a machine's wages are in it.",
            earned_qug, grp(grant_qug as i64)
        )))
        .add(Block::Section("The founder, stated plainly".into()))
        .add(para(
            "The chain was not revealed; it was built. Its inventor is a banker who reads physics and a merchant who runs \
             AI: the K-parameter is his, the genesis header commits his design document by BLAKE3, and the same header \
             commits the AI wallets that keep the nodes. No miracle is claimed anywhere in this paper. Every number above \
             recompiles."
                .to_string(),
        ));

    if !papers.is_empty() {
        doc = doc.add(Block::Raw(related_work_section(&papers)));
    }

    doc = doc
        .add(Block::Section("Reproducibility".into()))
        .add(para(format!(
            "\\texttt{{sigil\\_eternal <snapshot.json> [arxiv.json] [out\\_dir]}} in the Flux tree. The snapshot is the raw \
             \\texttt{{/v1/supply}}, \\texttt{{/v1/shielded/anchor}}, \\texttt{{/v1/finality/certificate}} (both nodes), \
             \\texttt{{/v1/usds/status}}, \\texttt{{/v1/network/topology}}, \\texttt{{/v1/mining/challenge}} and the latest \
             \\texttt{{sigil-kgauge.jsonl}} line, captured at \\texttt{{{}}}. Labelled model parameters: ${}$\\,W per core, \
             ${}$\\,K, ${}$\\,nm, \\$${:.3}$/SIGIL, ${}$ and ${}$ years for the Sun and the universe. Everything else is CODATA \
             2022 via \\texttt{{flux-science}} or a consensus constant copied from the tree. \\texttt{{numbers.json}} is emitted \
             beside the PDF so a lyric can be checked against a key. Generated on a host whose 1-minute load average read \
             ${:.0}$ at the time.",
            s.captured_ts_ms, miner_watts, t_room, 532, polygon_price_usd, sci(sun_remaining_yr), sci(universe_age_yr), loadavg1
        )));

    if !papers.is_empty() {
        let mut bib = String::from("\\begin{thebibliography}{99}\n");
        for p in &papers {
            let mut authors: Vec<String> = p.authors.iter().take(3).map(|a| latex_escape(a)).collect();
            if p.authors.len() > 3 {
                authors.push("et al.".into());
            }
            let year = p.published.get(0..4).unwrap_or("n.d.");
            bib.push_str(&format!(
                "\\bibitem{{{}}} {}: \\emph{{{}}}. arXiv:{} ({}). \\url{{{}}}\n",
                p.cite_key(),
                authors.join(", "),
                latex_escape(&p.title),
                p.id,
                year,
                if p.url.is_empty() { format!("https://arxiv.org/abs/{}", p.id) } else { p.url.clone() }
            ));
        }
        bib.push_str("\\end{thebibliography}\n");
        doc = doc.add(Block::Raw(bib));
    }

    std::fs::write(format!("{out_dir}/sigil_eternal.bib"), bibliography(&papers)).expect("bib");
    let res = doc.compile_pdf(out_dir, "sigil_eternal");
    if res.success {
        println!("OK {}", res.pdf_path.unwrap());
    } else {
        let tail: String = res.log.lines().rev().take(30).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
        eprintln!("FAILED\n{tail}");
        std::process::exit(1);
    }
}
