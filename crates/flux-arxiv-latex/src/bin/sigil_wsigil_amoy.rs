//! sigil_wsigil_amoy — a Flux Science note on WHERE SIGIL's testnet coin
//! (wSIGIL, the ERC-20 that the SIGIL bridge mints) should live. Decided
//! 2026-11-18 by the operator: Polygon Amoy (Polygon's testnet) now, real
//! Polygon after SIGIL mainnet.
//!
//! Every chain figure in the paper is MEASURED when this program runs:
//! read-only JSON-RPC calls (eth_chainId, eth_blockNumber, eth_gasPrice,
//! eth_getBalance, eth_call of totalSupply / decimals / balanceOf) made with
//! `curl` against public RPC endpoints, plus the local SIGIL node's own API.
//! The raw answers are written to `evidence.json` beside the PDF and its
//! BLAKE3 is printed in the paper. Nothing is signed, sent or deployed. A
//! call that fails is reported as "not measured", never filled in.
//!
//! The drain table is pure arithmetic (constant-product pool, 0.3 % fee).
//!
//! Usage: sigil_wsigil_amoy [out_dir]

use flux_arxiv_latex::doc::{Block, Document};
use serde_json::{json, Value};
use std::process::Command;

/// The bridge's operator address (owner + operator of wSIGIL3 on Polygon).
const OP: &str = "0x39d1d26d59eebcf1b4b7b4863ad38a6d226f6840";
const WSIGIL3: &str = "0x3FCED760b0DE6d57F96835C6110b1227941Ec2e9";
const LEGACY: &str = "0xc224602C32F5c7f68d3Ef002aE4C99e4C7Df25B7";
/// Native USDC on Polygon PoS (Circle).
const USDC_POLYGON: &str = "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359";
const SIGIL_API: &str = "http://127.0.0.1:18181";
/// Gas for deploying the wSIGIL ERC-20, from the flux-soneium deploy plan of
/// 2026-11-09 (`plan-base-usdc.json`). A plan estimate, not a measured deploy.
const DEPLOY_GAS: u128 = 883_752;
/// Alchemy's faucet eligibility floor: 0.001 ETH on Ethereum mainnet (in wei).
const ALCHEMY_FLOOR_WEI: u128 = 1_000_000_000_000_000;

struct Net {
    key: &'static str,
    name: &'static str,
    chain_id: u128,
    rpc: &'static str,
    unit: &'static str,
    test: bool,
}

const NETS: [Net; 5] = [
    Net { key: "ethereum", name: "Ethereum", chain_id: 1, rpc: "https://ethereum-rpc.publicnode.com", unit: "ETH", test: false },
    Net { key: "base", name: "Base", chain_id: 8453, rpc: "https://mainnet.base.org", unit: "ETH", test: false },
    Net { key: "base-sepolia", name: "Base Sepolia", chain_id: 84532, rpc: "https://sepolia.base.org", unit: "ETH", test: true },
    Net { key: "polygon", name: "Polygon PoS", chain_id: 137, rpc: "https://polygon-bor-rpc.publicnode.com", unit: "POL", test: false },
    Net { key: "polygon-amoy", name: "Polygon Amoy", chain_id: 80002, rpc: "https://polygon-amoy-bor-rpc.publicnode.com", unit: "POL", test: true },
];

struct Row<'a> {
    net: &'a Net,
    chain: Option<u128>,
    block: Option<u128>,
    gas: Option<u128>,
    bal: Option<u128>,
}

fn raw(s: &str) -> Block {
    Block::Raw(format!("{s}\n\n"))
}

/// One HTTP call through curl, three tries (public endpoints drop requests).
fn curl_json(url: &str, body: Option<&Value>) -> Option<Value> {
    for _ in 0..3 {
        let mut c = Command::new("curl");
        c.args(["-s", "-m", "12", "-A", "flux-science-rpc", "-H", "content-type: application/json"]);
        if let Some(b) = body {
            c.arg("--data").arg(b.to_string());
        }
        c.arg(url);
        if let Ok(out) = c.output() {
            if out.status.success() {
                if let Ok(v) = serde_json::from_slice::<Value>(&out.stdout) {
                    return Some(v);
                }
            }
        }
    }
    None
}

fn rpc(url: &str, method: &str, params: Value) -> Option<String> {
    let v = curl_json(url, Some(&json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})))?;
    v.get("result")?.as_str().map(str::to_string)
}

fn hex_u128(s: &str) -> Option<u128> {
    let t = s.trim_start_matches("0x").trim_start_matches('0');
    if t.is_empty() {
        return Some(0);
    }
    u128::from_str_radix(t, 16).ok()
}

fn rpc_u128(url: &str, method: &str, params: Value) -> Option<u128> {
    rpc(url, method, params).and_then(|r| hex_u128(&r))
}

fn call_u128(url: &str, to: &str, data: &str) -> Option<u128> {
    rpc_u128(url, "eth_call", json!([{ "to": to, "data": data }, "latest"]))
}

fn balance_of_data(addr: &str) -> String {
    format!("0x70a08231{:0>64}", addr.trim_start_matches("0x").to_lowercase())
}

/// Thousands separators that LaTeX prints as commas in text mode.
fn group(n: u128) -> String {
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

/// Fixed-point rendering of an integer amount with `dec` decimals, `shown` places.
fn units(v: u128, dec: u32, shown: u32) -> String {
    let base = 10u128.pow(dec);
    let whole = v / base;
    if shown == 0 || dec == 0 {
        return group(whole);
    }
    let frac = format!("{:0>width$}", v % base, width = dec as usize);
    format!("{}.{}", group(whole), &frac[..shown.min(dec) as usize])
}

fn opt_s(v: Option<String>) -> String {
    v.unwrap_or_else(|| "\\textit{not measured}".into())
}

fn s_opt(v: Option<u128>) -> Option<String> {
    v.map(|x| x.to_string())
}

fn main() {
    let out_dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/home/storage/sigil-scratch/sigil-amoy-paper-1118/out".into());
    std::fs::create_dir_all(&out_dir).expect("out dir");
    let measured_at = Command::new("date")
        .args(["-u", "+%Y-%m-%d %H:%M UTC"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown time".into());

    // ── measure: five EVM networks ──────────────────────────────────────────
    let mut ev = serde_json::Map::new();
    ev.insert("measured_at".into(), json!(measured_at));
    ev.insert("operator_address".into(), json!(OP));
    let mut rows: Vec<Row> = Vec::new();
    for n in NETS.iter() {
        let chain = rpc_u128(n.rpc, "eth_chainId", json!([]));
        let block = rpc_u128(n.rpc, "eth_blockNumber", json!([]));
        let gas = rpc_u128(n.rpc, "eth_gasPrice", json!([]));
        let bal = rpc_u128(n.rpc, "eth_getBalance", json!([OP, "latest"]));
        ev.insert(
            n.key.into(),
            json!({
                "rpc": n.rpc,
                "chain_id": s_opt(chain),
                "block": s_opt(block),
                "gas_price_wei": s_opt(gas),
                "operator_balance_wei": s_opt(bal),
            }),
        );
        rows.push(Row { net: n, chain, block, gas, bal });
    }

    // ── measure: tokens on Polygon PoS ──────────────────────────────────────
    let p = NETS[3].rpc;
    let usdc_op = call_u128(p, USDC_POLYGON, &balance_of_data(OP));
    let w3_supply = call_u128(p, WSIGIL3, "0x18160ddd");
    let w3_dec = call_u128(p, WSIGIL3, "0x313ce567");
    let lg_supply = call_u128(p, LEGACY, "0x18160ddd");
    let lg_dec = call_u128(p, LEGACY, "0x313ce567");
    ev.insert(
        "polygon_tokens".into(),
        json!({
            "usdc_native": USDC_POLYGON, "operator_usdc_base_units": s_opt(usdc_op),
            "wsigil3": WSIGIL3, "wsigil3_total_supply": s_opt(w3_supply), "wsigil3_decimals": s_opt(w3_dec),
            "legacy": LEGACY, "legacy_total_supply": s_opt(lg_supply), "legacy_decimals": s_opt(lg_dec),
        }),
    );

    // ── measure: the SIGIL side (local node API, read-only) ─────────────────
    let cert = curl_json(&format!("{SIGIL_API}/v1/finality/certificate"), None);
    let bridge = curl_json(&format!("{SIGIL_API}/v1/bridge/status"), None);
    let c = cert.as_ref().and_then(|v| v.get("certificate"));
    let s_height = c.and_then(|c| c.get("height")).and_then(Value::as_u64);
    let s_committee = c.and_then(|c| c.get("committee_size")).and_then(Value::as_u64);
    let s_gate = c.and_then(|c| c.get("gate")).and_then(Value::as_str).map(str::to_string);
    let s_bft = c.and_then(|c| c.get("bft")).and_then(Value::as_bool);
    let b = bridge.as_ref().and_then(|v| v.get("data"));
    let b_locks = b.and_then(|b| b.get("lock_count")).and_then(Value::as_u64);
    let b_no_relayer = b.map(|b| b.get("relayer").map_or(true, Value::is_null));
    ev.insert("sigil".into(), json!({ "api": SIGIL_API, "finality_certificate": cert, "bridge_status": bridge }));

    let ev_bytes = serde_json::to_vec_pretty(&Value::Object(ev)).expect("evidence json");
    std::fs::write(format!("{out_dir}/evidence.json"), &ev_bytes).expect("write evidence");
    let ev_hash = blake3::hash(&ev_bytes).to_hex().to_string();

    // ── derived figures ─────────────────────────────────────────────────────
    let eth_rows: Vec<&Row> = rows.iter().filter(|r| r.net.unit == "ETH").collect();
    let eth_all_measured = eth_rows.iter().all(|r| r.bal.is_some());
    let eth_total: u128 = eth_rows.iter().filter_map(|r| r.bal).sum();
    let eth_mainnet = rows[0].bal;
    let alchemy_gate = eth_mainnet.map(|b| b >= ALCHEMY_FLOOR_WEI);
    let pol_bal = rows[3].bal;
    let amoy_bal = rows[4].bal;

    let eth_sentence = if !eth_all_measured {
        "Its ETH balances could not all be read in this run.".to_string()
    } else if eth_total == 0 {
        "It holds no ETH at all: zero on Ethereum, zero on Base and zero on Base Sepolia.".to_string()
    } else {
        format!("It holds {} ETH in total across Ethereum, Base and Base Sepolia.", units(eth_total, 18, 6))
    };
    let gate_sentence = match (eth_mainnet, alchemy_gate) {
        (Some(b), Some(false)) => format!(
            "With {} ETH on Ethereum mainnet, the operator address is below that floor, so this faucet cannot \
             fund it.",
            units(b, 18, 6)
        ),
        (Some(b), Some(true)) => format!(
            "With {} ETH on Ethereum mainnet, the operator address meets the balance floor; the activity \
             requirement was not checked.",
            units(b, 18, 6)
        ),
        _ => "The Ethereum mainnet balance could not be read, so this gate is undetermined.".to_string(),
    };

    // ── Table 1: the five networks ──────────────────────────────────────────
    let mut t1 = String::from(
        "\\begin{table}[h]\\centering\\footnotesize\\setlength{\\tabcolsep}{4pt}\n\\begin{tabular}{lrrrrl}\\toprule\n\
         Network & Chain id & Block & Gas (gwei) & Operator balance & Pays one deploy?\\\\\\midrule\n",
    );
    for r in &rows {
        let name = if r.net.test { format!("{} \\textit{{(test)}}", r.net.name) } else { r.net.name.to_string() };
        let chain = match r.chain {
            Some(c) if c == r.net.chain_id => format!("{} \\checkmark", c),
            Some(c) => format!("{} (expected {})", c, r.net.chain_id),
            None => "\\textit{not measured}".into(),
        };
        let block = opt_s(r.block.map(group));
        let gas = opt_s(r.gas.map(|g| units(g, 9, 3)));
        let bal = opt_s(r.bal.map(|b| format!("{} {}", units(b, 18, 4), r.net.unit)));
        let pays = match (r.gas, r.bal) {
            (Some(g), Some(b)) => {
                let cost = g.saturating_mul(DEPLOY_GAS);
                format!("{} (needs {} {})", if b >= cost { "yes" } else { "no" }, units(cost, 18, 6), r.net.unit)
            }
            _ => "\\textit{not measured}".into(),
        };
        t1.push_str(&format!("{} & {} & {} & {} & {} & {}\\\\\n", name, chain, block, gas, bal, pays));
    }
    t1.push_str(&format!(
        "\\bottomrule\\end{{tabular}}\n\\caption{{Measured {measured_at}. Operator address \\texttt{{{}}} (in full in \\texttt{{evidence.json}}). \
         ``Pays one deploy'' compares the balance with the measured gas price times {} gas, the deploy estimate \
         from the 2026-11-09 plan (an estimate, not a measured deploy). Gas prices move; the column is a \
         snapshot.}}\\label{{tab:nets}}\n\\end{{table}}\n",
        format!("{}\\ldots{}", &OP[..10], &OP[38..]),
        group(DEPLOY_GAS)
    ));

    // ── Table 2: what already exists on Polygon PoS and on SIGIL ────────────
    let tok = |sup: Option<u128>, dec: Option<u128>| -> String {
        match (sup, dec) {
            (Some(s), Some(d)) if d <= 30 => format!("{} ({} decimals)", units(s, d as u32, 4), d),
            (Some(s), _) => format!("{} base units", group(s)),
            _ => "\\textit{not measured}".into(),
        }
    };
    let t2 = format!(
        "\\begin{{table}}[h]\\centering\\small\n\\begin{{tabularx}}{{\\linewidth}}{{lX}}\\toprule\n\
         Item & Measured value\\\\\\midrule\n\
         Operator's USDC on Polygon PoS & {usdc}\\\\\n\
         wSIGIL3 total supply (\\texttt{{{w3a}\\ldots}}) & {w3}\\\\\n\
         Older ``SigilGraph'' token total supply (\\texttt{{{lga}\\ldots}}) & {lg}\\\\\n\
         SIGIL testnet height & {h}\\\\\n\
         SIGIL finality committee size, gate, BFT & {cm}, {gate}, {bft}\\\\\n\
         SIGIL bridge locks recorded & {locks}\\\\\n\
         SIGIL bridge relayer configured & {rel}\\\\\n\
         \\bottomrule\\end{{tabularx}}\n\\caption{{Measured {measured_at}. The two Polygon tokens are left over from \
         the September bridge; SIGIL's testnet was reset on 19 October, so neither is backed by anything \
         today.}}\\label{{tab:exists}}\n\\end{{table}}\n",
        usdc = opt_s(usdc_op.map(|u| format!("{} USDC", units(u, 6, 4)))),
        w3a = &WSIGIL3[..10],
        w3 = tok(w3_supply, w3_dec),
        lga = &LEGACY[..10],
        lg = tok(lg_supply, lg_dec),
        h = opt_s(s_height.map(|h| group(h as u128))),
        cm = opt_s(s_committee.map(|c| c.to_string())),
        gate = opt_s(s_gate.clone()),
        bft = opt_s(s_bft.map(|b| if b { "yes".to_string() } else { "no".to_string() })),
        locks = opt_s(b_locks.map(|l| l.to_string())),
        rel = opt_s(b_no_relayer.map(|n| if n { "no".to_string() } else { "yes".to_string() })),
    );

    // ── Table 3: how much of a pool one minting key can take (pure math) ────
    let gamma = 0.997_f64;
    let mut t3 = String::from(
        "\\begin{table}[h]\\centering\\small\n\\begin{tabular}{rrr}\\toprule\n\
         $k$ & Share of the pool's USDC taken & USDC left of 1{,}000\\\\\\midrule\n",
    );
    for k in [0.1_f64, 1.0, 10.0, 100.0, 1000.0] {
        let share = gamma * k / (1.0 + gamma * k);
        t3.push_str(&format!(
            "{} & {:.2}\\,\\% & {:.2}\\\\\n",
            if k < 1.0 { format!("{k:.1}") } else { format!("{k:.0}") },
            share * 100.0,
            1000.0 * (1.0 - share)
        ));
    }
    t3.push_str(
        "\\bottomrule\\end{tabular}\n\\caption{Computed from Eq.~\\eqref{eq:drain} with a 0.3\\,\\% swap fee \
         ($\\gamma = 0.997$); $k$ is the number of minted coins sold, as a multiple of the coins in the pool. The last column is an example pool holding 1{,}000 USDC; the shares do not depend \
         on the pool's size.}\\label{tab:drain}\n\\end{table}\n",
    );

    // ── the document ────────────────────────────────────────────────────────
    let mut doc = Document::new("article")
        .option("11pt")
        .option("a4paper")
        .package_opt("inputenc", &["utf8"])
        .package_opt("geometry", &["margin=1.05in"])
        .package("amsmath")
        .package("amssymb")
        .package("booktabs")
        .package("tabularx")
        .package("xcolor")
        .package("tcolorbox")
        .package_opt("enumitem", &[])
        .package_opt("hyperref", &["hidelinks"])
        .preamble(concat!(
            "\\definecolor{sigilcyan}{HTML}{0E7C86}\n",
            "\\definecolor{panelbg}{HTML}{F6F7F9}\n",
            "\\newcommand{\\brief}[3]{\\begin{tcolorbox}[colback=panelbg,colframe=#1,boxrule=0.8pt,arc=2pt,",
            "title=\\textbf{#2},coltitle=white,colbacktitle=#1]#3\\end{tcolorbox}}\n",
            "\\hypersetup{pdftitle={Where a Testnet Coin Should Live},",
            "pdfsubject={Network choice for SIGIL's bridged testnet coin, measured},",
            "pdfkeywords={SIGIL, bridge, ERC-20, Polygon Amoy, faucet, constant-product AMM, testnet}}\n",
            "\\title{\\textbf{Where a Testnet Coin Should Live}\\\\[6pt]",
            "\\large Choosing the network for SIGIL's bridged coin from what the operator actually holds\\\\[4pt]",
            "\\normalsize\\itshape Polygon Amoy now, real Polygon after SIGIL mainnet}\n",
            "\\author{Flux Science\\\\\\small research note by Rocky (Claude Opus 5.5), requested by Viktor,",
            "\\\\\\small written on the day the decision was made, 18 November 2026}\n",
            "\\date{\\today}"
        ))
        .add(Block::Raw("\\maketitle".into()));

    doc = doc.add(Block::Raw(format!(
        "\\begin{{abstract}}\\noindent\n\
         SIGIL is a privacy-first chain that is still a testnet. Its bridge can lock SIGIL on the chain and \
         issue the same amount as an ordinary ERC-20 token, wSIGIL, on an Ethereum-style network, where it can \
         trade against dollars. This note asks which network that coin should live on before SIGIL's mainnet, \
         and answers it from measurements taken when the note was generated, not from preference. Three facts \
         decide it. The operator's money is on Polygon, and the bridge's operator address holds \
         {pol} POL there and no ETH anywhere. The free faucets for Base's testnet require real ETH on Ethereum \
         first, so Base is closed to us. And a coin backed by a testnet has a known end: if real money can buy \
         it, buyers carry a loss the project chose for them, while one bridge key can mint without limit and \
         empty any pool it trades in (Table~\\ref{{tab:drain}}). The decision that follows is Polygon Amoy, \
         Polygon's testnet, until SIGIL mainnet, then real Polygon with real USDC once the bridge no longer \
         rests on one key. Every chain figure here is in \\texttt{{evidence.json}}, BLAKE3 \
         \\texttt{{{h12}}}\\ldots\n\\end{{abstract}}\n\n",
        pol = opt_s(pol_bal.map(|b| units(b, 18, 4))),
        h12 = &ev_hash[..12],
    )));

    doc = doc.add(raw(&format!(
        "\\brief{{sigilcyan}}{{The decision, in one paragraph}}{{SIGIL's testnet coin goes on \\textbf{{Polygon Amoy}} \
         now: test USDC is free there, so nobody can lose real money on it. It moves to \\textbf{{real Polygon}} \
         with real USDC after SIGIL mainnet (planned for 18 December 2026; the date moves if the quality is not \
         there), and only after the bridge's minting power is split across several keys with a cap and a pause. \
         Base is dropped for now: the operator has no ETH (\\S\\ref{{sec:measure}}), and a coin on Base would \
         need the USDC moved there first. Measured {measured_at}.}}"
    )));

    doc = doc
        .add(Block::Section("What the bridge does, and what it trusts".into()))
        .add(raw(
            "The bridge has two halves. On SIGIL, a wallet locks coins into a vault note. A relay service watches \
             the SIGIL node, sees the lock, and calls \\texttt{mint(to, amount, lockId)} on the ERC-20 contract \\cite{eip20} on \
             the other network; the contract refuses a lock id it has already used. Going back, a holder burns the \
             token with a SIGIL destination, the contract emits an event, and the relay asks the SIGIL node to \
             release the same amount from the vault. Nothing in this code is specific to one network: it speaks \
             plain Ethereum JSON-RPC, so the same contract and relay run on Polygon, Base or their testnets \
             \\cite{sigilbase}.",
        ))
        .add(raw(
            "What the bridge trusts is the part that matters for this note. The contract has one operator key, and \
             that key may mint any amount. The relay trusts one SIGIL node's word that a lock has settled, and SIGIL \
             itself is finalised by a committee of one key today (Table~\\ref{tab:exists}). For a test, that is \
             acceptable. For a coin that real money can buy, it means the coin is worth exactly as much as the \
             promise of one key holder, which \\S\\ref{sec:drain} makes concrete.",
        ));

    doc = doc
        .add(Block::Section("What was measured".into()))
        .add(Block::Raw("\\label{sec:measure}\n".into()))
        .add(raw(
            "The program that generated this paper asked five networks for their chain id, latest block, gas price \
             and the operator address's balance, and asked Polygon for the operator's USDC and the supply of the two \
             tokens the September bridge left behind. It then asked the local SIGIL node for its finality \
             certificate and bridge status. A chain id that matches the expected value is marked with a tick; it is \
             the check that each public endpoint is the network it claims to be. Every call is read-only.",
        ))
        .add(Block::Raw(t1))
        .add(Block::Raw(t2))
        .add(raw(&format!(
            "{eth_sentence} Its POL sits on Polygon PoS ({pol} POL), and on Amoy it holds {amoy} POL, so the very \
             first step on Amoy is a faucet for test POL to pay gas. The USDC that will later fund a real pool is \
             the operator's own and sits on Polygon PoS; the bridge address itself holds only dust (Table~\\ref{{tab:exists}}).",
            pol = opt_s(pol_bal.map(|b| units(b, 18, 4))),
            amoy = opt_s(amoy_bal.map(|b| units(b, 18, 4))),
        )));

    doc = doc
        .add(Block::Section("Three constraints that decide the network".into()))
        .add(Block::Subsection("Where the money already is".into()))
        .add(raw(
            "A token can only trade against dollars that sit on the same network. The operator's USDC is on Polygon \
             PoS, and the bridge ran there before, so on Polygon the coin and its future liquidity meet without any \
             extra step. On Base, the USDC would first have to be moved across networks, and the operator would \
             need ETH there to pay gas. The cheapest network is the one the money is already on.",
        ))
        .add(Block::Subsection("Who will give you test money".into()))
        .add(raw(&format!(
            "A testnet is free only if a faucet will fund you. Alchemy's documentation states that its faucet \
             requires the receiving wallet to hold at least 0.001 ETH on Ethereum mainnet and to have a history of \
             real transactions there; Base Sepolia is not among its listed exceptions \\cite{{alchemyfaucet}}. \
             {gate_sentence} Circle's public faucet, by contrast, lists Polygon PoS Amoy among its networks and \
             hands out test USDC \\cite{{circlefaucet}}. The remaining gate on Amoy is test POL for gas; which \
             faucet will fund the operator address with it was not checked for this note."
        )))
        .add(Block::Subsection("What a buyer can lose".into()))
        .add(Block::Raw("\\label{sec:drain}\n".into()))
        .add(raw(
            "On a testnet the answer is zero by construction: every token in play came from a faucet for free, so no \
             one can pay real money for the coin. On a real network the coin's backing chain is still a testnet that \
             will be reset at mainnet, so anyone who pays real USDC for it before then holds a claim with a known \
             expiry. A second risk is larger and does not depend on the reset. A constant-product pool with $x$ \
             coins and $y$ USDC pays, for $\\Delta x$ coins sold into it,",
        ))
        .add(Block::Raw(
            "\\begin{equation}\\label{eq:drain}\n\\Delta y = \\frac{\\gamma\\,\\Delta x\\,y}{x + \\gamma\\,\\Delta x},\
             \\qquad \\frac{\\Delta y}{y} = \\frac{\\gamma k}{1 + \\gamma k}\\quad\\text{with } \\Delta x = k\\,x,\n\
             \\end{equation}\n\n"
                .into(),
        ))
        .add(raw(
            "where $\\gamma$ is one minus the swap fee \\cite{uniswapv2}. Whoever holds the minting key can create \
             $\\Delta x$ at no cost, so the share of the pool's dollars they can take approaches one as $k$ grows. \
             Table~\\ref{tab:drain} shows how fast: minting ten times the pool's coins takes about 91\\,\\% of its \
             USDC, a hundred times takes 99\\,\\%. The pool's real dollars are exactly as safe as one key.",
        ))
        .add(Block::Raw(t3));

    doc = doc
        .add(Block::Section("The plan".into()))
        .add(raw(
            "\\textbf{Phase A, now: Polygon Amoy.} Fund the operator address with test POL and test USDC from \
             faucets; deploy a coin whose name says testnet; open the bridge to testers or only show the coin, as the \
             team decides; add a pool if a constant-product router is deployed on Amoy (not checked yet). The \
             exposure of real money in this phase is zero.",
        ))
        .add(raw(
            "\\textbf{Gate between the phases.} SIGIL mainnet runs, and the bridge's minting power is split: several \
             operator keys that must agree, a daily cap, a pause switch and a delay on owner changes, as listed in the \
             9 November assessment \\cite{sigilbase}. Until then, Table~\\ref{tab:drain} applies in full to any real \
             liquidity.",
        ))
        .add(raw(
            "\\textbf{Phase B, after mainnet: real Polygon.} Deploy a fresh coin with zero supply, backed by the \
             mainnet vault, and seed a SIGIL/USDC pool with the operator's USDC on Polygon PoS. The two old tokens of \
             Table~\\ref{tab:exists} are not reused; they stay as they are, unbacked, and should be described that \
             way wherever they are mentioned.",
        ));

    doc = doc
        .add(Block::Section("What is still pretend".into()))
        .add(raw(
            "\\begin{itemize}[leftmargin=1.4em,itemsep=2pt]\n\
             \\item No coin has been deployed on Amoy yet; the gas figure in Table~\\ref{tab:nets} is a plan's \
             estimate, not a receipt.\n\
             \\item The faucet rules are the providers' own documents, read on 18 November 2026; neither faucet was \
             used for this note.\n\
             \\item Whether a constant-product router exists on Amoy was not checked.\n\
             \\item The bridge has one minting key and SIGIL finality has one signer (Table~\\ref{tab:exists}); \
             neither is fixed by choosing a network.\n\
             \\item The mainnet date of 18 December 2026 is a plan, and it moves if the quality is not there.\n\
             \\end{itemize}",
        ));

    doc = doc
        .add(Block::Section("Conclusion".into()))
        .add(raw(
            "The right network for a testnet coin is the one where it can do no harm and where its future \
             liquidity already sits. Measured today, that is Polygon: its testnet, Amoy, until SIGIL is real, and \
             Polygon PoS with the operator's USDC after that. Base was not rejected on its merits; it was rejected \
             because the money is elsewhere and its faucets ask for ETH the operator does not hold.",
        ));

    doc = doc.add(Block::Raw(
        "\\begin{thebibliography}{9}\\raggedright\n\
         \\bibitem{sigilbase} Rocky (Flux Science): \\emph{wSIGIL on Base via Uniswap: assessment}, 9 November 2026. \
         \\url{https://sigilgraph.org/downloads/sigil-wsigil-base-uniswap-assessment-2026-11-09.md}\n\
         \\bibitem{alchemyfaucet} Alchemy: \\emph{Testnet faucets}, documentation, read 18 November 2026. \
         \\url{https://www.alchemy.com/docs/faucets}\n\
         \\bibitem{circlefaucet} Circle: \\emph{Testnet faucets}, developer documentation, read 18 November 2026. \
         \\url{https://developers.circle.com/w3s/docs/developer-console-faucet}\n\
         \\bibitem{uniswapv2} Hayden Adams, Noah Zinsmeister, Dan Robinson: \\emph{Uniswap v2 Core}, 2020. \
         \\url{https://uniswap.org/whitepaper.pdf}\n\
         \\bibitem{eip20} Fabian Vogelsteller, Vitalik Buterin: \\emph{EIP-20: Token Standard}, 2015. \
         \\url{https://eips.ethereum.org/EIPS/eip-20}\n\
         \\end{thebibliography}\n"
            .into(),
    ));

    let res = doc.compile_pdf(&out_dir, "SIGIL_WSIGIL_AMOY_v0");
    if res.success {
        println!("OK {}", res.pdf_path.unwrap());
        println!("evidence {out_dir}/evidence.json blake3 {ev_hash}");
    } else {
        let tail: String = res.log.lines().rev().take(60).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
        eprintln!("FAILED\n{tail}");
        std::process::exit(1);
    }
}
