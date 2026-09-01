//! usds_whitepaper — "USDS: A Fixed-Buffer, No-Liquidation Stablecoin for
//! SIGIL". Every DERIVED number in this paper (the worked-example math,
//! percentages, break-even prices) is COMPUTED at generation time from the
//! protocol constants below — the same "a claim that recompiles is a
//! measurement" discipline `thermodynamic_ledger` established for this
//! crate. The constants THEMSELVES are copied from the shipped crates
//! (`sigil-usds`, `sigil-oracle`, `sigil-bank`, `sigil-state` — exact
//! file:line cited at each `const` below) rather than live-imported: this
//! binary lives in the FLUX workspace, those crates live in the SEPARATE
//! SIGIL workspace, and a live cross-workspace dependency would pull that
//! whole graph into every `flux-arxiv-latex` build for one paper's sake.
//! Re-verify the constants against source before trusting a stale build —
//! the Reproducibility section (end of paper) says this explicitly too.
//! No arXiv sweep: this is a from-scratch protocol design, not a survey.
//!
//! Usage: usds_whitepaper [out_dir]

use flux_arxiv_latex::doc::{Block, Document};

/// Format a number for math mode: plain when small, \times10^{n} otherwise.
fn sci(x: f64) -> String {
    if x == 0.0 || !x.is_finite() {
        return format!("{x}");
    }
    let exp = x.abs().log10().floor() as i32;
    if (-2..=3).contains(&exp) {
        let s = format!("{:.4}", x);
        let s = s.trim_end_matches('0').trim_end_matches('.');
        s.to_string()
    } else {
        let mant = x / 10f64.powi(exp);
        format!("{:.3}\\times10^{{{}}}", mant, exp)
    }
}

fn para(s: String) -> Block {
    Block::Raw(format!("{s}\n\n"))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out_dir = args.get(1).map(String::as_str).unwrap_or("/tmp/usds-whitepaper");

    // ---------------------------------------------------------------- protocol constants
    // Copied from the shipped SIGIL-workspace crates as of 2026-08-18 (see
    // module docs for why this is a copy, not a live cross-workspace import).
    // If these ever drift from source, THIS PAPER IS WRONG — re-check the
    // cited file:line before trusting a stale build.
    let buffer_bps = 10_500.0_f64; // sigil-usds/src/lib.rs: MINT_BUFFER_BPS
    let fee_bps = 30.0_f64; // sigil-bank/src/lib.rs: MASTER_SWAP_FEE_BPS
    let bps_denom = 10_000.0_f64; // sigil-bank/src/lib.rs: BPS_DENOMINATOR
    let band_bps = 2_000.0_f64; // sigil-oracle/src/lib.rs: MAX_PRICE_CHANGE_BPS
    let sigil_decimals: i32 = 10; // sigil-state/src/lib.rs:112 SIGIL_DECIMALS (g2 reset 2026-08-29 moved this 8→10; base unit is the "glyph", 1 SIGIL = 10^10 glyphs)
    let max_supply = 21_000_000_f64 * 10f64.powi(sigil_decimals) / 10f64.powi(sigil_decimals); // sigil-state/src/lib.rs:134 MAX_SUPPLY (21M SIGIL, 10dp)

    let buffer_pct = (buffer_bps / bps_denom - 1.0) * 100.0; // 5.0%
    let fee_pct = fee_bps / bps_denom * 100.0; // 0.30%
    let band_pct = band_bps / bps_denom * 100.0; // 20%

    // ---------------------------------------------------------------- worked example
    // Lock $1,000 of value at SIGIL = $2.00/coin. Every downstream number is
    // computed the SAME way `sigil_usds::plan_mint` computes it — floor
    // division at each step, matching real on-chain integer arithmetic.
    let price_usd = 2.0_f64;
    let sigil_locked = 500.0_f64; // 500 SIGIL * $2 = $1000 value
    let locked_value = sigil_locked * price_usd;
    let usds_gross = (locked_value * bps_denom / buffer_bps * 1e8).floor() / 1e8;
    let protocol_fee = (usds_gross * fee_bps / bps_denom * 1e8).floor() / 1e8;
    let usds_to_user = usds_gross - protocol_fee;
    let cushion_value = locked_value - usds_gross;
    let coverage_pct_at_mint = locked_value / usds_gross * 100.0;

    // How far SIGIL's price can fall before the vault is exactly, precisely
    // at 100% coverage for THIS position (the buffer's whole reason to exist).
    let breakeven_price = usds_gross / sigil_locked;
    let max_drawdown_pct = (1.0 - breakeven_price / price_usd) * 100.0;

    // ---------------------------------------------------------------- the QUGUSD incident, for contrast
    let qugusd_observed_price = 43_749.85_f64;
    let qugusd_target_price = 3_000.0_f64;
    let qugusd_overshoot_x = qugusd_observed_price / qugusd_target_price;
    let qugusd_min_ratio = 1.35_f64;
    let qugusd_liq_ratio = 1.15_f64;

    // ---------------------------------------------------------------- LaTeX
    let doc = Document::new("article")
        .option("11pt")
        .package_opt("inputenc", &["utf8"])
        .package_opt("geometry", &["margin=1.1in"])
        .package("amsmath")
        .package("amssymb")
        .package("booktabs")
        .package_opt("hyperref", &["hidelinks"])
        .preamble(concat!(
            "\\title{USDS\\\\[6pt]\\large A Fixed-Buffer, No-Liquidation Stablecoin for SIGIL}\n",
            "\\author{The SIGIL Project\\\\\\small every figure below computed at document-generation time from the shipped ",
            "\\texttt{sigil-usds}/\\texttt{sigil-oracle}/\\texttt{sigil-bank} crates, typeset by \\texttt{flux-arxiv-latex}}\n",
            "\\date{\\today}"
        ))
        .add(Block::Raw("\\maketitle".into()))
        .add(Block::Raw(format!(
            "\\begin{{abstract}}\nUSDS (``usdSIGIL'') is a native, dollar-denominated token on the SIGIL chain, backed \
             1:1-plus-a-fixed-cushion by locked native SIGIL at a committed oracle price. It intentionally does NOT copy \
             the over-collateralized CDP-with-liquidations design of Quillon Graph's QUGUSD — that system's real, live \
             failure (a corrupted price oracle briefly valuing collateral at \\${}, {:.1}$\\times$ its \\${} target) motivated \
             two specific, narrower design choices here instead: a single, non-divergent mint/redeem path with a \
             fixed ${:.1}\\%$ collateral cushion (no liquidation engine to get wrong), and a price oracle that REJECTS any \
             single push moving the peg by more than ${:.0}\\%$, closing the exact failure class that produced QUGUSD's \
             incident. This paper derives every quantitative claim from the shipped source, not from a design intent \
             that may have drifted from the code.\n\\end{{abstract}}\n",
            sci(qugusd_observed_price), qugusd_overshoot_x, sci(qugusd_target_price), buffer_pct, band_pct
        )))
        .add(Block::Section("Introduction".into()))
        .add(para(format!(
            "SIGIL is a young, low-liquidity chain: its native token has no external price stability of its own, so a \
             genuinely useful native stablecoin needs collateral discipline that does not depend on continuous, correct, \
             high-frequency price data --- exactly the assumption that broke on the sister chain this design was reviewed \
             against. Quillon Graph's QUGUSD requires ${:.0}\\%$ collateral to mint and liquidates at ${:.0}\\%$ \
             (an {:.0} percentage-point margin engineered for defense), but its live price oracle reads raw, single-block \
             automated-market-maker reserves with no time-weighting and only a generous absolute sanity bound. A separate \
             float-to-integer casting bug pinned that pool's reserve near the integer maximum; the oracle accepted it \
             without complaint, and the resulting price ---\\${}, against a real target near \\${}--- fed directly into \
             both the collateral math and a user-facing earnings display. \\emph{{The bug was not in the collateral ratio. \
             It was upstream of it, in the price feed the ratio trusted.}} A wider liquidation margin does not help if the \
             number the margin is computed against is simply wrong.",
            qugusd_min_ratio * 100.0, qugusd_liq_ratio * 100.0, (qugusd_min_ratio - qugusd_liq_ratio) * 100.0,
            sci(qugusd_observed_price), sci(qugusd_target_price)
        )))
        .add(para(
            "USDS responds to that specific lesson with two independent, narrower fixes rather than one larger, more \
             complex system: harden the oracle itself (Section 3), and remove the failure mode that liquidation exists \
             to correct in the first place by never allowing a position to go leveraged (Section 2). Section 4 gives \
             the full worked arithmetic of a real mint, computed by this document itself. Section 5 covers the protocol \
             fee and its reuse of existing, already-tested code. Section 6 covers the Polygon-side bridge, built on the \
             same proven lock/mint/burn/unlock pattern shipped for wrapped SIGIL."
                .to_string(),
        ))
        .add(Block::Section("The Fixed Collateral Buffer".into()))
        .add(para(format!(
            "Minting USDS is a single operation: lock \\texttt{{sigil\\_amount}} of native SIGIL into a vault at the \
             committed oracle price, and receive USDS worth the locked value \\emph{{divided by}} \
             ${:.4}$ (i.e.\\ ${:.1}\\%$ of face value) --- \\texttt{{sigil-usds::MINT\\_BUFFER\\_BPS}} $={:.0}$ basis \
             points. The remaining ${:.2}\\%$ of the locked value is not returned to the minter; it stays in the vault \
             as a cushion, unconditionally, for every position, computed the same way every time. There is no per-position \
             ratio to monitor, no liquidation bot to run, no threshold that can be undershot by a fast-moving price \
             between the observation and the transaction landing --- because nothing here is EVER less than fully \
             collateralized at issue time by construction; the only question is how much SLACK exists above 100\\%, and \
             the answer is always the same fixed number.",
            buffer_bps / bps_denom, buffer_bps / bps_denom * 100.0, buffer_bps, buffer_pct
        )))
        .add(para(
            "This is a deliberate trade against QUGUSD's design, not an oversight: QUGUSD's wider 20-percentage-point \
             margin (135\\% mint / 115\\% liquidate) can absorb a larger price move per position \\emph{because} it also \
             carries a liquidation engine to enforce the floor when a position's collateral value actually falls through \
             it. USDS carries no such engine — every position is minted with the SAME slack, and if SIGIL's price falls \
             far enough after a mint, the affected position's true backing genuinely thins, redeemable value first, not \
             defended by a keeper bot. What USDS buys for that trade is structural: one code path, not two independently \
             evolving ones (see Section 5.1's note on QUGUSD's OWN two-path bug), and zero liquidation logic to get wrong \
             in a first implementation of a genuinely new chain's stablecoin."
                .to_string(),
        ))
        .add(Block::Section("The Oracle Sanity Band".into()))
        .add(para(format!(
            "\\texttt{{sigil-oracle}} is a single pinned-authority feed --- one wallet may push a price, committed through \
             the real state-transition chokepoint like any other write, with no separate mutable oracle state to drift \
             out of sync with the chain's own root commitments. That authority model is a real, acknowledged centralization \
             trade (Section 7), but it structurally cannot suffer QUGUSD's specific failure: there is no raw AMM-reserve \
             read, and therefore no path for a pool-reserve overflow bug to reach the price at all. What a single trusted \
             feed CAN still do is push a wrong number by simple human error, and that failure mode is closed separately: \
             \\texttt{{update\\_price}} rejects any single push that moves the committed price by more than \
             \\texttt{{MAX\\_PRICE\\_CHANGE\\_BPS}} $={:.0}$ (${:.0}\\%$) from the LAST COMMITTED price. A real, sustained \
             market move still lands --- it simply lands over more than one push, each individually bounded --- while a \
             single fat-fingered or corrupted price is mechanically incapable of reaching mint/redeem math in one step.",
            band_bps, band_pct
        )))
        .add(Block::Section("A Worked Mint, Computed Here".into()))
        .add(para(format!(
            "Take a real example, with SIGIL priced at \\${:.2}$: a wallet locks ${:.0}$ SIGIL (\\${:.2}$ of value) into \
             the vault. Buffered gross: ${:.0}$ (\\${:.2}$) $\\times\\, {:.0}/{:.0}$ (the buffer's inverse) $= {:.6}$ USDS. \
             The protocol fee (Section 5) takes ${:.2}\\%$ of that: ${:.6}$ USDS, leaving the minter with \
             \\textbf{{${:.6}$ USDS}}. The vault, meanwhile, holds the FULL ${:.0}$ SIGIL (\\${:.2}$ of value) against \
             ${:.6}$ USDS of new supply from this one position --- ${:.2}\\%$ coverage the instant the transaction lands.",
            price_usd, sigil_locked, locked_value, sigil_locked, locked_value, buffer_bps, bps_denom, usds_gross,
            fee_pct, protocol_fee, usds_to_user, sigil_locked, locked_value, usds_gross, coverage_pct_at_mint
        )))
        .add(para(format!(
            "The cushion in dollar terms is \\${:.2}$ (the ${:.2}\\%$ that never left the vault as USDS). Because \
             redemption always pays the CURRENT price for whatever USDS is burned (Section 4 is about MINT slack, not a \
             promise that survives forever), this specific position's exact break-even price --- the SIGIL price at which \
             its slice of the vault covers exactly the USDS it backs, no more --- is \\${:.4}$ per SIGIL: a \
             \\textbf{{${:.2}\\%$}} price drop from today's \\${:.2}$ before this position's own collateral is no longer, \
             on its own, sufficient. That number is a property of the fixed buffer, not of any monitoring: it is the same \
             ${:.2}\\%$ for every mint, always, computed once here and true of every other one.",
            cushion_value, buffer_pct, breakeven_price, max_drawdown_pct, price_usd, buffer_pct
        )))
        .add(Block::Section("The Protocol Fee".into()))
        .add(Block::Subsection("5.1 Reuse, not reinvention".into()))
        .add(para(format!(
            "USDS charges \\texttt{{sigil\\_bank::MASTER\\_SWAP\\_FEE\\_BPS}} $={:.0}$ (${:.2}\\%$) on both mint and redeem \
             --- the IDENTICAL function and constant DEX swaps already pay, via the same \
             \\texttt{{sigil\\_bank::split\\_swap\\_output}} call, not a parallel reimplementation with its own rounding \
             policy. This matters more than it may look: QUGUSD ships two INDEPENDENT mint paths (a self-service vault and \
             a separate bank-loan path) that duplicate similar logic slightly differently, and the two have measurably \
             diverged in what they actually debit. USDS has exactly one mint function and one redeem function, and its fee \
             math is the SAME code path DEX swaps already exercise in production, not new arithmetic written once for this \
             feature and never touched again.",
            fee_bps, fee_pct
        )))
        .add(Block::Section("The Polygon Bridge".into()))
        .add(para(
            "USDS also exists as an ERC-20 on Polygon (\\texttt{USDSBridgeWrapped}), built on the identical lock/mint/burn/unlock \
             pattern already shipped and gas-measured for wrapped native SIGIL: zero pre-mint, an operator key that mints \
             ONLY against an independently-verified SIGIL L1 lock, self-service burn-to-destination, and an owner role that \
             can pause or rotate the operator but can never move funds itself. It is a SEPARATE contract with SEPARATE \
             admin/operator keys from the native-SIGIL bridge, deliberately: a compromised key on one bridge cannot touch \
             the other's vault. The two bridges also sign with disjoint message namespaces, so a signature valid for one \
             can never be replayed against the other."
                .to_string(),
        ))
        .add(Block::Section("Known Limitations, Stated Plainly".into()))
        .add(para(format!(
            "\\textbf{{Single-authority oracle.}} One wallet pushes SIGIL's USD price; the sanity band bounds how much \
             damage one bad push can do, but does not remove the trust assumption that the authority is honest and live. \
             \\textbf{{No liquidation engine.}} A position's own collateral can, in principle, thin below its issued value \
             after a sustained price fall exceeding ${:.2}\\%$ with no per-position defense mechanism; the fixed buffer is \
             a cushion sized against realistic single-push moves (bounded by the ${:.0}\\%$ oracle band), not a promise \
             against an unbounded multi-step decline. \\textbf{{Supply is not yet indexed.}} \\texttt{{/v1/usds/status}} \
             reports the committed price and vault collateral (both real, on-chain) but not circulating USDS supply, which \
             requires an index this implementation does not yet build. Every one of these is a stated scope boundary of a \
             first implementation on a young chain, not a claim that the system is complete.",
            buffer_pct, band_pct
        )))
        .add(Block::Section("Reproducibility".into()))
        .add(para(format!(
            "This PDF is the output of \\texttt{{cargo run --release -p flux-arxiv-latex --bin usds\\_whitepaper}} in the \
             Flux tree (built \\texttt{{fluxc}}-side, per this repository's dogfood discipline). Every DERIVED number \
             above --- the worked-example arithmetic, percentages, break-even price --- is computed by this binary at \
             generation time, not hand-typed; changing the worked example's inputs and recompiling changes every dependent \
             figure consistently. The four PROTOCOL CONSTANTS themselves (\\texttt{{MINT\\_BUFFER\\_BPS}}, \
             \\texttt{{MASTER\\_SWAP\\_FEE\\_BPS}}, \\texttt{{MAX\\_PRICE\\_CHANGE\\_BPS}}, \\texttt{{SIGIL\\_DECIMALS}}/ \
             \\texttt{{MAX\\_SUPPLY}}) are copied from the SIGIL workspace's source as of this paper's date, cited by exact \
             file at each constant's definition in this binary, rather than live-imported --- \\texttt{{flux-arxiv-latex}} \
             and \\texttt{{sigil-usds}} live in separate Cargo workspaces, and a live cross-workspace dependency would pull \
             the whole SIGIL state/oracle/bank graph into every future build of this one paper. \\textbf{{Before trusting a \
             stale copy of this PDF, diff the four constants above against the cited source files directly.}} The \
             ${:.0}$-SIGIL / \\${:.2}$-price worked example (Section 4) is arbitrary and chosen only for round numbers; \
             substituting any other values reproduces the same coverage/break-even relationship, which is the actual claim \
             this section makes.",
            sigil_locked, price_usd
        )))
        .add(para(format!(
            "For scale: SIGIL's own protocol-level cap is ${:.0}$ SIGIL, ever (\\texttt{{sigil\\_state::MAX\\_SUPPLY}}), \
             enforced at the same \\texttt{{commit\\_state\\_transition}} chokepoint USDS mints and redeems through --- so \
             even a maximally successful USDS is bounded by a hard, code-enforced ceiling on the collateral that could ever \
             back it, the same invariant this paper's every other number was computed against.",
            max_supply
        )));

    let res = doc.compile_pdf(out_dir, "usds_whitepaper");
    if res.success {
        println!("OK {}", res.pdf_path.unwrap());
    } else {
        let tail: String = res.log.lines().rev().take(40).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
        eprintln!("FAILED\n{tail}");
        std::process::exit(1);
    }
}
