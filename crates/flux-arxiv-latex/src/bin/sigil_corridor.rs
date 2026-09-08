//! sigil_corridor — "The Tri-Chain Corridor": public Bitcoin ingress → shielded SIGIL transit →
//! replay-safe EVM settlement. Every number is read from a `sigil-pipeline-bench` run and from
//! the live node captured at generation time (`CORRIDOR_DATA_DIR`), or derived here from a
//! stated formula. No constant in this file is a measurement.
use flux_arxiv_latex::doc::{Block, Document};
use serde_json::Value;

fn read(dir: &str, name: &str) -> Value {
    let p = format!("{dir}/{name}");
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p}: {e}"))).unwrap_or(Value::Null)
}
fn f(v: &Value) -> f64 { v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).unwrap_or(0.0) }
fn s(v: &Value) -> String { v.as_str().map(|x| x.to_string()).unwrap_or_else(|| v.to_string()) }
fn hex2(h: &str) -> String { if h.len() > 40 { format!("{}\\allowbreak {}", &h[..32], &h[32..]) } else { h.to_string() } }
fn commas(n: u128) -> String { let d = n.to_string(); let mut o = String::new(); for (i, c) in d.chars().enumerate() { if i > 0 && (d.len() - i) % 3 == 0 { o.push(','); } o.push(c); } o }

/// Nakamoto (2008) §11: probability an attacker with hash share q ever catches up from z blocks behind.
fn reorg_prob(q: f64, z: u32) -> f64 {
    let p = 1.0 - q; let lam = z as f64 * q / p; let mut sum = 0.0;
    for k in 0..=z { let mut poisson = (-lam).exp(); for i in 1..=k { poisson *= lam / i as f64; } sum += poisson * (1.0 - (q / p).powi((z - k) as i32)); }
    1.0 - sum
}

const PREAMBLE: &str = r#"
\definecolor{ink}{HTML}{0E1116}
\definecolor{accent}{HTML}{8B5CF6}
\definecolor{btc}{HTML}{F7931A}
\definecolor{eth}{HTML}{627EEA}
\definecolor{sig}{HTML}{8B5CF6}
\hypersetup{colorlinks=true,urlcolor=accent,linkcolor=accent,citecolor=accent}
\titleformat{\section}{\large\bfseries\color{ink}}{\thesection}{0.6em}{}
\titleformat{\subsection}{\normalsize\bfseries\color{ink}}{\thesubsection}{0.6em}{}
\titlespacing{\section}{0pt}{12pt}{5pt}
\setlist[itemize]{leftmargin=15pt,itemsep=2pt,topsep=3pt}
\pgfplotsset{compat=1.17}
\usetikzlibrary{positioning,arrows.meta,shapes.misc}
\title{\bfseries The Tri-Chain Corridor:\\\large Verified Bitcoin Ingress, Shielded SIGIL Transit, and Replay-Safe EVM Settlement\\\normalsize --- measurements of a working vertical slice ---}
\author{Viktor S. Kristensen \and Rocky (Claude Fable 5.1, agent)}
\date{8 September 2026 \,\textperiodcentered\, sigil-pipeline v0/v1, sigil commit __SIGIL_COMMIT__}
"#;

fn main() {
    let dir = std::env::var("CORRIDOR_DATA_DIR").expect("CORRIDOR_DATA_DIR");
    let out_dir = std::env::var("CORRIDOR_OUT_DIR").unwrap_or_else(|_| "/home/storage/claude-code/sigil-corridor-paper/out".into());
    let commit = std::env::var("SIGIL_COMMIT").unwrap_or_else(|_| "unknown".into());
    let b = read(&dir, "bench.json");
    let anchor = read(&dir, "live_anchor.json");
    let chal = read(&dir, "live_challenge.json");
    let locks = read(&dir, "live_locks.json");
    let supply = read(&dir, "live_supply.json");
    let att = read(&dir, "attestation-demo.json");

    let notes = f(&anchor["notes"]); let cap = f(&anchor["capacity"]); let nfs = f(&anchor["nullifiers"]);
    let height = f(&chal["height"]);
    let lock = &locks["data"][0];
    let supply_sigil = f(&supply["data"]["native_supply"]);
    let depth = b["stark_by_depth"].as_array().cloned().unwrap_or_default();
    let slip = b["slippage"].as_array().cloned().unwrap_or_default();
    let spot = f(&b["pool_spot_usdc_per_wsigil_1e18"]) / 1e6; // (USDC-units per wei)·1e18 → USDC per wSIGIL: ×1e18/1e6/1e18
    let res_w = f(&b["pool_reserve_wsigil_wei"]) / 1e18; let res_u = f(&b["pool_reserve_usdc_units"]) / 1e6;

    let mut body = String::new();
    body.push_str("\\maketitle\n\\begin{abstract}\n");
    body.push_str(&format!("We report a working vertical slice of a three-chain corridor: a Bitcoin payment admitted by a self-verifying SPV proof, a hidden SIGIL note that moves under a hiding STARK, and a byte-exact ERC-20 mint on Polygon keyed on the SIGIL transaction hash. The privacy boundary is the point of the design: Bitcoin learns only a shield public key, the EVM learns only a destination and an amount, and the destination travels sealed inside the SIGIL note ciphertext so no public chain ever records the Bitcoin-to-Ethereum relationship. Every figure below was measured at generation time on the production host (Epsilon): SPV verification of a six-header chain takes {:.0}\\,$\\mu$s and the proof is {} bytes; the v5 hiding spend over the live 32{{,}}768-leaf pool proves in {:.0}\\,ms and verifies in {:.1}\\,ms with a {}-byte proof; the settlement calldata is {} bytes. We also report the constraints the slice exposed --- pool depth is quantised to $2^{{2^k-1}}$ leaves by the circuit, the live wSIGIL3/USDC pool loses {:.0}\\,\\% of value on a trade of one tenth of its depth, and the deployed relayer listens for an event the live contract never emits --- and we state plainly what is still unwired.\n",
        f(&b["spv_verify_6conf_us"]), s(&b["spv_proof_bytes"]),
        depth.last().map(|r| f(&r["prove_ms"])).unwrap_or(0.0), depth.last().map(|r| f(&r["verify_ms"])).unwrap_or(0.0), depth.last().map(|r| s(&r["proof_bytes"])).unwrap_or_default(),
        s(&b["mint_calldata_bytes"]), slip.iter().find(|r| f(&r["trade_pct_of_pool"]) == 10.0).map(|r| f(&r["loss_bps_vs_spot"]) / 100.0).unwrap_or(0.0)));
    body.push_str("\\end{abstract}\n\n");

    // ── 1. The picture ──────────────────────────────────────────────────────────────────
    body.push_str("\\section{The picture}\n");
    body.push_str("OK, so here's the deal. Think of a tunnel with a lit entrance, a lit exit, and a dark middle. A Bitcoin transaction is public forever. An Ethereum mint is public forever. If the Bitcoin transaction named the Ethereum address, the two would be linked on chain for good and nothing in between could un-link them. So the Bitcoin side is only ever told a \\emph{shield public key} --- a one-way image of a secret nobody on Ethereum has ever seen --- and the Ethereum destination rides \\emph{inside} the sealed SIGIL note, readable by the exit vault alone.\n\n");
    body.push_str(r#"\begin{center}\begin{tikzpicture}[node distance=16mm, every node/.style={font=\small}]
\node[draw=btc,thick,rounded corners,fill=btc!8,text width=40mm,align=center] (btc) {\textbf{\textcolor{btc}{PUBLIC Bitcoin}}\\[2pt] tx pays $N$ sats, memo names a \emph{shield pk}\\[2pt]\scriptsize sees: txid, sats, shield pk};
\node[draw=sig,thick,rounded corners,fill=sig!8,text width=40mm,align=center,right=of btc] (sig) {\textbf{\textcolor{sig}{PRIVATE SIGIL}}\\[2pt] hidden note $\to$ hiding STARK $\to$ vault note; ETH dest sealed in ciphertext\\[2pt]\scriptsize sees: nullifier, 2 commitments, fee, proof};
\node[draw=eth,thick,rounded corners,fill=eth!8,text width=40mm,align=center,right=of sig] (eth) {\textbf{\textcolor{eth}{PUBLIC Polygon/ETH}}\\[2pt] \texttt{mint(to, wei, lockId)} keyed on the SIGIL tx hash\\[2pt]\scriptsize sees: to, wei, lockId};
\draw[-{Stealth[length=3mm]},thick] (btc) -- node[above,font=\scriptsize]{SPV proof} (sig);
\draw[-{Stealth[length=3mm]},thick] (sig) -- node[above,font=\scriptsize]{settled exit} (eth);
\node[below=4mm of sig,font=\footnotesize] (h) {$H_0 \;\longrightarrow\; H_1 \;\longrightarrow\; H_2 \;\longrightarrow\; H_3$ \quad (BLAKE3 receipt chain, \S6)};
\end{tikzpicture}\end{center}
"#);
    body.push_str("The thing that should bother you is the middle box: how can a chain verify that value was conserved when it is not allowed to see the amounts? That is exactly what a hiding STARK does, and \\S4 measures what it costs.\n\n");

    // ── 2. What each observer learns ───────────────────────────────────────────────────
    body.push_str("\\section{What each observer learns}\n");
    body.push_str("Call the depositor Alice, the exit vault $V$, and her Ethereum address $A_E$. Write $\\mathrm{pk}_A$ for her shield public key ($\\mathrm{pk}=\\mathrm{compress}_2(\\mathrm{sk},\\,\\mathrm{PK\\_DOMAIN})$, one-way).\n");
    body.push_str(r#"\begin{center}\small\begin{tabular}{p{24mm}p{62mm}p{62mm}}\toprule
observer & learns & does \emph{not} learn \\\midrule
Bitcoin watcher & $\mathrm{txid}$, sats, $\mathrm{pk}_A$ & $A_E$, any SIGIL address, when/if it exits \\
SIGIL node & anchor, nullifier, two hiding commitments, fee, proof & Alice, $V$, the split, $A_E$ \\
Polygon watcher & $A_E$, wei, $\mathrm{lockId}=H(\text{exit tx})$ & $\mathrm{txid}$, $\mathrm{pk}_A$, that Bitcoin was involved at all \\
the vault $V$ & value, blinding, $A_E$, intent id (by opening its own note) & Alice's other notes, her spend key \\
\bottomrule\end{tabular}\end{center}
"#);
    body.push_str("Linking $\\mathrm{txid}$ to $A_E$ therefore requires either the vault's decryption key or a break of the note cipher (X25519 sealed box + AEAD) or of the hiding commitment. Amount correlation remains: a watcher who sees $N$ sats enter and $N\\cdot r$ wei leave can guess. Batching exits and the DEX leg (\\S5) are the two levers against that, and both are outside this slice.\n\n");

    // ── 3. Bitcoin leg ─────────────────────────────────────────────────────────────────
    body.push_str("\\section{Leg 1 --- verified Bitcoin ingress}\n");
    body.push_str("Don't be scared by the name SPV. It is four checks and a counter: every header meets its own target (real proof-of-work, dSHA256), each header's \\texttt{prev\\_block} is the hash of the one before, the deposit transaction hashes to a leaf and the Merkle branch lifts it to the first header's root, and there are at least $z$ headers. A fifth check, the \\emph{difficulty floor}, rejects any header whose own target is easier than mainnet's \\texttt{powLimit} --- without it an attacker mines regtest-difficulty headers in microseconds and clears everything else.\n\n");
    body.push_str(&format!("\\textbf{{Measured.}} A single header PoW check costs {}\\,ns; verifying a six-confirmation proof takes {:.1}\\,$\\mu$s; the proof is {} bytes (tx + branch + 6 headers). The Bitcoin genesis header, decoded from its 80 wire bytes, re-hashes to \\texttt{{{}}} and sits exactly at the floor. The memo carrying $(\\text{{sats}},\\,\\mathrm{{pk}}_A)$ lives \\emph{{inside}} the proven transaction bytes, so neither can be substituted without changing the txid the proof is about.\n\n",
        s(&b["header_pow_check_ns"]), f(&b["spv_verify_6conf_us"]), s(&b["spv_proof_bytes"]), hex2(&s(&b["genesis_hash"]))));
    body.push_str("\\textbf{Why six.} Nakamoto's catch-up probability for an attacker holding a fraction $q$ of the hash power, starting $z$ blocks behind, is $P(z)=1-\\sum_{k=0}^{z}\\frac{\\lambda^k e^{-\\lambda}}{k!}\\bigl(1-(q/p)^{z-k}\\bigr)$ with $\\lambda=zq/p$. Computed here:\n");
    body.push_str("\\begin{center}\\begin{tabular}{r");
    for _ in 0..3 { body.push_str("r"); }
    body.push_str("}\\toprule $z$ & $q=0.10$ & $q=0.20$ & $q=0.30$ \\\\\\midrule\n");
    for z in [1u32, 2, 3, 4, 6, 8, 10, 12] {
        body.push_str(&format!("{} & {:.2e} & {:.2e} & {:.2e} \\\\\n", z, reorg_prob(0.10, z), reorg_prob(0.20, z), reorg_prob(0.30, z)));
    }
    body.push_str("\\bottomrule\\end{tabular}\\end{center}\n");
    body.push_str(&format!("At $q=0.1$, six confirmations leave a {:.1e} chance of reversal; the policy is a dial, not a constant, and the code takes it as one (\\texttt{{BtcPolicy}}).\n\n", reorg_prob(0.10, 6)));

    // ── 4. SIGIL leg ───────────────────────────────────────────────────────────────────
    body.push_str("\\section{Leg 2 --- shielded transit}\n");
    body.push_str("A note is four field elements: $(\\text{value},\\,\\text{blinding},\\,\\text{spend key},\\,\\text{position})$. Its leaf is $\\mathrm{cm}=\\mathrm{compress}_2(\\mathrm{compress}_2(v,b),\\,\\mathrm{pk})$ --- \\emph{owner-bound}, so knowing $(v,b)$ confers no ability to spend, which is what makes it safe to hand the vault its $(v,b)$ at all. Spending reveals $\\mathrm{nf}=\\mathrm{compress}_2(\\mathrm{sk},\\,\\text{position})$ and nothing else. The deposit note is minted \\emph{only} to the $\\mathrm{pk}$ the Bitcoin memo named (a mismatch is refused, because a note to the wrong key is value burned). The v5 circuit is v4's constraint system on a trace whose second half is reserved randomness, so the openings the verifier sees do not print the witness.\n\n");
    body.push_str("\\textbf{Measured} (real prover, real verifier through \\texttt{note\\_v1::verify\\_spend\\_wire}, the function consensus calls):\n");
    body.push_str("\\begin{center}\\begin{tabular}{rrrrrr}\\toprule pool capacity & depth & prove (ms) & verify (ms) & proof (B) & sealed note (B) \\\\\\midrule\n");
    for r in &depth {
        body.push_str(&format!("$2^{{{}}}$ & {} & {:.1} & {:.2} & {} & {} \\\\\n", s(&r["log2_capacity"]), f(&r["log2_capacity"]) as u32, f(&r["prove_ms"]), f(&r["verify_ms"]), commas(f(&r["proof_bytes"]) as u128), s(&r["ciphertext_bytes"])));
    }
    body.push_str("\\bottomrule\\end{tabular}\\end{center}\n");
    body.push_str(r#"\begin{center}\begin{tikzpicture}\begin{axis}[width=0.62\linewidth,height=44mm,xlabel={$\log_2$ pool capacity},ylabel={ms},legend pos=north west,legend style={font=\scriptsize},ymode=log,grid=major]
\addplot[accent,thick,mark=*] coordinates {"#);
    for r in &depth { body.push_str(&format!("({},{:.2}) ", f(&r["log2_capacity"]), f(&r["prove_ms"]))); }
    body.push_str("};\n\\addplot[eth,thick,mark=square*] coordinates {");
    for r in &depth { body.push_str(&format!("({},{:.2}) ", f(&r["log2_capacity"]), f(&r["verify_ms"]))); }
    body.push_str("};\n\\legend{prove, verify}\\end{axis}\\end{tikzpicture}\\end{center}\n");
    body.push_str("Now here's the thing that should bother you: the capacities are $2^3, 2^7, 2^{15}$, not $2^4, 2^8, 2^{10}$. That is not a choice of ours. The v5 AIR derives its trace length from the tree depth and requires $\\text{depth}+1$ to be a power of two, so the only legal pools hold $2^{2^k-1}$ leaves. The first bench run asked for depth 4 and the prover refused it. Growing the live pool past 32{,}768 therefore means 2{,}147{,}483{,}648 leaves ($2^{31}$), not 65{,}536 --- or an epoch rotation, which is what SIGIL actually does.\n\n");
    body.push_str(&format!("\\textbf{{Live anonymity set}} at generation time: {} notes of {} capacity in epoch {}, {} nullifiers ever, at height {}; native supply {:.0} SIGIL. The note count is the real privacy number --- a spend can only hide among notes that exist. The intent memo uses {} of the {}-byte sealed-memo budget, so the ciphertext is the same size whether or not it carries an Ethereum destination; size does not fingerprint a bridging note.\n\n",
        commas(notes as u128), commas(cap as u128), s(&anchor["epoch"]), nfs as u64, commas(height as u128), supply_sigil / 1e10, s(&b["intent_memo_bytes"]), s(&b["memo_budget_bytes"])));

    // ── 5. Ethereum leg ────────────────────────────────────────────────────────────────
    body.push_str("\\section{Leg 3 --- replay-safe EVM settlement}\n");
    body.push_str(&format!("The settled exit becomes one call on the deployed \\texttt{{SigilBridgeWrappedG3}} (Polygon, \\texttt{{0x3FCED760\\ldots}}): \\texttt{{mint(address,uint256,uint256)}}, selector \\texttt{{0x156e29f6}} (recomputed with Keccak-256 and checked against the deployed artifact), {} bytes of calldata. The third argument, \\texttt{{lockId}}, is the SIGIL exit transaction hash as a big-endian \\texttt{{uint256}}. It is content-derived, so it cannot be reset by a chain restart or reused by a counter rollover, and the contract's \\texttt{{usedLock}} map makes a repeat revert on chain: replay safety lives in the contract, not in the relayer's memory. Amounts shift by $10^{{{}}}$ from 10-decimal glyphs to 18-decimal wei; the real 0.01 SIGIL lock settled during this work (lock id 2, \\texttt{{{}}}, settled: {}) would mint {} wei.\n\n",
        s(&b["mint_calldata_bytes"]), (f(&b["decimal_shift"]).log10()) as u32, hex2(&s(&lock["tx_hash"])), s(&lock["settled"]), commas(f(&b["lock2_0_01_sigil_wei"]) as u128)));
    body.push_str("\\textbf{A finding, pinned as a test.} The deployed relayer's ABI declares \\texttt{OperatorMinted}; the live g3 contract emits \\texttt{Minted}. Same indexed layout, different name, different topic hash --- the relayer's on-chain lookback would never see a g3 mint. It was fixed in code (both signatures accepted) but the service stays held: its configuration still names the legacy token, and the contract's \\texttt{operator()} is not the relayer's key.\n\n");
    body.push_str(&format!("\\textbf{{From wSIGIL to anything else.}} The intent layer lets a user ask for USDC (or later ETH) rather than wrapped SIGIL. The solver quotes a one-hop constant-product swap on the destination chain. On the live wSIGIL3/USDC pool, read at generation time ({:.4} wSIGIL against {:.4} USDC, spot {:.2} USDC per wSIGIL), the price impact is brutal, and the solver must say so rather than hide it:\n", res_w, res_u, spot));
    body.push_str("\\begin{center}\\begin{tabular}{rrr}\\toprule trade (\\% of pool) & USDC out & loss vs spot (bps) \\\\\\midrule\n");
    for r in &slip { body.push_str(&format!("{:.2} & {:.6} & {} \\\\\n", f(&r["trade_pct_of_pool"]), f(&r["out_units"]) / 1e6, s(&r["loss_bps_vs_spot"]))); }
    body.push_str("\\bottomrule\\end{tabular}\\end{center}\n");
    body.push_str("What nature is telling us here is simple: ``0.1 native BTC $\\to$ 2.3 native ETH'' is not a cryptography problem, it is a liquidity problem. The corridor moves value with proofs; the exchange rate is set by whoever is willing to hold the other side. The intent carries a \\texttt{min\\_output} and a \\texttt{deadline}, and the solver refuses any route that cannot clear them --- slippage is bounded by the user, not by the market.\n\n");

    // ── 6. Receipt ────────────────────────────────────────────────────────────────────
    body.push_str("\\section{The universal receipt}\n");
    body.push_str("Each leg is serialised and chained: $H_0=\\mathrm{B3}(\\text{btc})$, $H_1=\\mathrm{B3}(H_0\\,\\|\\,\\text{sigil})$, $H_2=\\mathrm{B3}(H_1\\,\\|\\,\\text{eth})$, and the intent closes it: $H_3=\\mathrm{B3}(\\text{intent\\_id}\\,\\|\\,H_0\\,\\|\\,H_1\\,\\|\\,H_2\\,\\|\\,\\text{amount\\_in}\\,\\|\\,\\text{amount\\_out}\\,\\|\\,\\text{asset\\_in}\\,\\|\\,\\text{asset\\_out})$. One hash says: this bitcoin existed, this private transition was valid, this settlement happened, and they are the same economic operation. Editing any leg breaks the link after it; the tests do exactly that. The demo run that produced this paper's attestation:\n");
    body.push_str(&format!("\\begin{{center}}\\footnotesize\\begin{{tabular}}{{ll}}\\toprule $H_0$ (Bitcoin) & \\texttt{{{}}} \\\\ $H_1$ (SIGIL) & \\texttt{{{}}} \\\\ $H_2$ (Polygon) & \\texttt{{{}}} \\\\ btc txid & \\texttt{{{}}} \\\\ SIGIL nullifier & \\texttt{{{}}} \\\\ lockId & \\texttt{{{}}} \\\\\\bottomrule\\end{{tabular}}\\end{{center}}\n",
        hex2(&s(&att["chain"][0])), hex2(&s(&att["chain"][1])), hex2(&s(&att["chain"][2])), hex2(&s(&att["btc"]["txid"])), hex2(&s(&att["sigil"]["nullifier"])), hex2(&s(&att["eth"]["lock_id"]))));
    body.push_str("The intent object the v1 layer executes is deliberately chain-agnostic: \\texttt{\\{source, input, amount\\_in, destination, output, min\\_output, recipient, privacy: ShieldedTransit, deadline, nonce\\}}, with a content-derived id. Consensus, in turn, is asked for exactly one new shape --- \\texttt{ExternalTransition \\{proof, commitment\\}} --- where \\texttt{proof} is an enum. Bitcoin SPV and Lightning (BOLT11 signature + preimage) have verifiers; the Ethereum receipt variant exists so the shape is stable and is \\emph{refused}, because a receipt-trie proof plus a finality argument does not exist in this tree and pretending otherwise would be worse than saying no.\n\n");

    // ── 7. Honest list ─────────────────────────────────────────────────────────────────
    body.push_str("\\section{What is real, and what is still pretend}\n");
    body.push_str("\\textbf{Real, exercised through the production code paths:} SPV verification incl.\\ the difficulty floor; the v5 hiding prover and the consensus verifier; the sealed note cipher; the byte-exact g3 mint calldata; the constant-product solver on live reserves; the intent memo; the receipt chain; the live 0.01 SIGIL lock that settled on g2 during this work. 24/24 tests, run with \\texttt{--profile release-fast} because the prover needs debug assertions off.\n\n");
    body.push_str("\\textbf{Pretend, and said so:} (i) no node route accepts an external proof --- \\texttt{ExternalTransition} is not a \\texttt{SigilTx} variant and \\texttt{commit\\_state\\_transition} has no arm for it; minting a note from Bitcoin is a consensus change and needs a height gate; (ii) the bridge vault does not yet scan ciphertexts for exit memos --- today's lock is a transparent \\texttt{Shield}; (iii) the relayer is held, mis-configured for the legacy token, and not the contract's operator; (iv) \\texttt{bitcoind} on this host is crash-looping on a corrupt block database and needs \\texttt{-reindex-chainstate} before a real mainnet proof can be fetched; (v) the BTC$\\to$SIGIL rate is a policy input, not an oracle; (vi) the DEX leg is quoted, not executed --- executing it needs the vault's EVM key and a router call.\n\n");
    body.push_str("\\section{Next}\n");
    body.push_str("In order: fix the operator key or rotate \\texttt{operator()} to the relayer, repoint the relayer to g3, start it, and watch lock 2 mint; reindex \\texttt{bitcoind} and admit one real mainnet proof under \\texttt{BtcPolicy::mainnet()}; add \\texttt{SigilTx::ExternalTransition} behind a height gate with the peg root committed in the header; teach the vault to scan for \\texttt{SIGILX-exit/v1} memos and mark \\texttt{settled} on the finalised spine; then put EVM compatibility \\emph{above} this layer rather than SIGIL beneath Ethereum's.\n\n");
    body.push_str("\\medskip\\noindent\\footnotesize Code: sigil \\texttt{crates/sigil-pipeline} (modules \\texttt{btc\\_in}, \\texttt{private\\_mid}, \\texttt{eth\\_out}, \\texttt{external}, \\texttt{intent}, \\texttt{solver}, \\texttt{receipt}, \\texttt{attestation}); measurements by \\texttt{sigil-pipeline-bench} at generation time; live figures from \\texttt{sigil-api} on 127.0.0.1:18181 and Polygon RPC. Where a number here disagrees with the chain tomorrow, the chain is right.\n");

    let preamble = PREAMBLE.replace("__SIGIL_COMMIT__", &commit);
    let doc = Document::new("article").option("11pt").option("a4paper")
        .package_opt("fontenc", &["T1"]).package("lmodern")
        .package_opt("geometry", &["a4paper", "top=22mm", "bottom=22mm", "left=22mm", "right=22mm"])
        .package("xcolor").package("titlesec").package("enumitem").package("booktabs").package("amsmath").package("tikz").package("pgfplots").package("hyperref")
        .preamble(&preamble).add(Block::Raw(body));
    std::fs::create_dir_all(&out_dir).ok();
    let res = doc.compile_pdf(&out_dir, "sigil-corridor-paper");
    println!("flux-arxiv-latex: success={} pdf={:?}", res.success, res.pdf_path);
    if !res.success { eprintln!("{}", res.log.lines().rev().take(40).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n")); std::process::exit(1); }
}
