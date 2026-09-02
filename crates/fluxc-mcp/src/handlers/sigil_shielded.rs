//! `flux_sigil_shielded_*` — the SHIELDED money surface with the client-side
//! cryptography actually performed **in this process**.
//!
//! ## Why this module exists (2026-08-26)
//!
//! Transparent sends have been dead network-wide since 2026-08-23
//! (`sigil_tx::SHIELDED_ONLY_HEIGHT == 0`, `POST /api/v1/send` refuses
//! unconditionally), so the shielded family is the ONLY way value moves on
//! SIGIL. But the shielded tools in [`crate::handlers::sigil_wallet`] were thin
//! HTTP passthroughs: `flux_sigil_shielded_send` demanded a caller-supplied
//! `anchor` / `nullifier` / `cm_outs` / `proof`, and its own description said
//! *"Build the proof with sigil-shield's wallet::build_spend — NOT implemented
//! by this MCP tool"*. An AI agent driving the MCP has no way to do that: the
//! STARK prover, the note store, the pool-tree reconstruction and the note
//! ciphertexts all live in `sigil-shield`, which the MCP did not link. The
//! practical effect was that **no shielded transfer could be initiated from
//! Claude Code at all** — the only working path was a hand-run
//! `cargo run --example live_shielded_test` in the sigil tree.
//!
//! This module links `sigil-shield` and closes that gap end to end:
//!
//! ```text
//!   flux_sigil_shielded_keys    seed        → wallet / pk_shield / pk_encrypt / sigil1s: address
//!   flux_sigil_shielded_notes   seed        → the notes this seed can actually spend, + balance
//!   flux_sigil_shielded_send    seed,to,amt → scan → select → PROVE → seal → submit
//!   flux_sigil_unshield_from_seed seed,to,amt → same, exiting to a transparent wallet
//!   flux_sigil_shield_from_seed seed,amt    → derives the commitment itself (no hand-computed `cm`)
//! ```
//!
//! The low-level passthroughs in `sigil_wallet.rs` are kept as an escape hatch
//! for a caller that already has a proof from elsewhere; these are the tools an
//! agent should reach for.
//!
//! ## Statelessness, and the one piece of local state
//!
//! An MCP tool call keeps nothing between invocations, so every call rebuilds
//! the wallet from the chain:
//!   * **received notes** — trial-decrypt every ciphertext in
//!     `GET /v1/shielded/leaves` (a successful AEAD open IS the ownership proof);
//!   * **self-shielded deposits** — re-derive `note(index, denomination)` over a
//!     bounded index × denomination grid and match commitments against the pool
//!     (shield amounts are always denominations, so this grid is complete);
//!   * **mining rewards** — `coinbase_commitment_wire(height, pk_shield, amount)`
//!     over a window of recent heights (a coinbase blinding is publicly derivable
//!     by design, so a miner finds its own reward with no bookkeeping);
//!   * **our own change** — this module seals a note ciphertext to the SENDER's
//!     own address for the change output, so change is recoverable by the same
//!     trial-decryption path as any received note. The reference
//!     `live_shielded_test` example sent `null` there, which made change
//!     unrecoverable after a restart. That is a deliberate improvement, not a
//!     copy of the example.
//!
//! The one thing the chain cannot tell us is which of our notes are already
//! SPENT: `sigil-api` exposes a nullifier *count* but no nullifier *list*. So a
//! small journal (`FLUX_SIGIL_SHIELDED_JOURNAL`, default
//! `/home/storage/flux-state/sigil-shielded.json`) records the nullifiers this
//! MCP has spent, keyed by the wallet's PUBLIC `pk_shield`. It never stores a
//! seed. It is a convenience only — the chain is the authority and rejects a
//! double spend regardless; losing the journal costs a rejected submission, not
//! money.
//!
//! ## Safety
//!
//! `broadcast` defaults to **false** on every value-moving tool. A dry run stops
//! before the (expensive) STARK proof and returns the plan — which note it would
//! spend, the fee, the change — so an agent can show its work before an operator
//! authorizes the real thing. Nothing here ever stores or transmits a seed: the
//! seed is used to derive keys in-process and the proof/ciphertexts are all that
//! leave.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

use sigil_shield::note_cipher::{
    enc_identity_from_seed, seal_note, try_open_note, NoteCiphertext, NotePlaintext, ShieldedAddress,
};
use sigil_shield::note_v1::{
    coinbase_blinding, coinbase_commitment_wire, padding_leaf_wire, to_wire,
};
use sigil_shield::wallet::{build_spend, NoteStore, ShieldedAccount, SpendBundle};

use crate::handlers::sigil_wallet::{rpc_get, rpc_post};
use crate::handlers::{ToolDef, ToolRegistry};

/// Mirrors `sigil_state::shielded::SHIELDED_FEE`. Duplicated rather than
/// depended on: pulling `sigil-state` in would drag the whole chain into the MCP
/// build for one constant, and the chokepoint re-checks the value anyway — a
/// drift here produces a rejected transaction, never an accepted bad one.
/// 2026-08-29: 1_000 → 100_000, tracking the sigil-g2 fee bump (the drift the
/// comment above predicted — every s2s send died with WrongFee until this).
const SHIELDED_FEE: u64 = 100_000;

/// How many derivation indices to sweep when reconstructing self-shielded notes.
const DEFAULT_INDEX_SCAN: u64 = 64;
/// How many recent heights to sweep for coinbase (mining-reward) notes.
const DEFAULT_COINBASE_WINDOW: u64 = 512;

// ─── small arg helpers ──────────────────────────────────────────────────────

fn arg_str(a: &Value, k: &str, d: &str) -> String {
    a.get(k).and_then(|v| v.as_str()).unwrap_or(d).to_string()
}
fn arg_u64(a: &Value, k: &str, d: u64) -> u64 {
    a.get(k)
        .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        .unwrap_or(d)
}
fn arg_bool(a: &Value, k: &str, d: bool) -> bool {
    a.get(k).and_then(|v| v.as_bool()).unwrap_or(d)
}
fn err(msg: impl Into<String>) -> String {
    json!({"ok": false, "error": msg.into()}).to_string()
}
fn hex32(s: &str) -> Option<[u8; 32]> {
    hex::decode(s.trim().trim_start_matches("0x"))
        .ok()
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
}
fn seed_of(a: &Value) -> Result<[u8; 32], String> {
    let s = arg_str(a, "seed", "");
    if s.is_empty() {
        return Err("seed required (32-byte hex — the same seed the wallet was created with)".into());
    }
    hex32(&s).ok_or_else(|| "seed must be 32-byte hex".to_string())
}

/// The ramp ladder, mirrored from `sigil_state::shielded::DENOMINATIONS`
/// (1/2/5 × 10^0..10^15). Same duplication rationale as [`SHIELDED_FEE`].
fn denominations() -> Vec<u64> {
    let mut v = Vec::with_capacity(48);
    let mut p: u64 = 1;
    for _ in 0..16 {
        for d in [1u64, 2, 5] {
            v.push(d * p);
        }
        p = p.saturating_mul(10);
    }
    v.sort_unstable();
    v
}

// ─── chain reads ────────────────────────────────────────────────────────────

struct Pool {
    leaves: Vec<[u8; 32]>,
    ciphertexts: Vec<Option<String>>,
    capacity: usize,
    anchor: String,
}

/// Fetch the pool once: real leaves, positionally-aligned delivery ciphertexts,
/// and the capacity the padding must be computed to.
fn fetch_pool() -> Result<Pool, String> {
    let raw = rpc_get("/v1/shielded/leaves");
    let v: Value = serde_json::from_str(&raw).map_err(|e| format!("/v1/shielded/leaves: {e} (body: {raw})"))?;
    if !v.get("ok").and_then(|o| o.as_bool()).unwrap_or(false) {
        return Err(format!("/v1/shielded/leaves returned {v}"));
    }
    let leaves: Vec<[u8; 32]> = v["leaves"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| x.as_str().and_then(hex32)).collect())
        .unwrap_or_default();
    let ciphertexts: Vec<Option<String>> = v["ciphertexts"]
        .as_array()
        .map(|a| a.iter().map(|x| x.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();
    let capacity = v["capacity"].as_u64().unwrap_or(32_768) as usize;
    let anchor = serde_json::from_str::<Value>(&rpc_get("/v1/shielded/anchor"))
        .ok()
        .and_then(|a| a["anchor"].as_str().map(|s| s.to_string()))
        .unwrap_or_default();
    Ok(Pool { leaves, ciphertexts, capacity, anchor })
}

/// The FULL padded leaf set the chain's anchor is computed over. A proof is
/// against one specific tree, so a differently-padded view yields a proof that
/// cannot verify — this must match the server's padding exactly.
fn padded(pool: &Pool) -> Vec<[u8; 32]> {
    let mut v = pool.leaves.clone();
    for i in v.len()..pool.capacity {
        v.push(padding_leaf_wire(i as u64));
    }
    v
}

/// Current chain height, for bounding the coinbase sweep. `/api/v1/status` is
/// 404 on sigil-api; the mining challenge is the live route that carries it.
fn tip_height() -> u64 {
    serde_json::from_str::<Value>(&rpc_get("/api/v1/mining/challenge"))
        .ok()
        .and_then(|v| v["height"].as_u64())
        .unwrap_or(0)
}

// ─── the spent-note journal ─────────────────────────────────────────────────

fn journal_path() -> String {
    std::env::var("FLUX_SIGIL_SHIELDED_JOURNAL")
        .unwrap_or_else(|_| "/home/storage/flux-state/sigil-shielded.json".to_string())
}

fn journal_load() -> HashMap<String, Vec<String>> {
    std::fs::read_to_string(journal_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Record a nullifier we just spent. Atomic (tmp-write + rename), same contract
/// as the mandate store, so a crash mid-write cannot corrupt the file.
fn journal_record(pk_shield: &str, nullifier: &str) {
    let mut j = journal_load();
    let e = j.entry(pk_shield.to_string()).or_default();
    if !e.iter().any(|n| n == nullifier) {
        e.push(nullifier.to_string());
    }
    let path = journal_path();
    if let Some(dir) = std::path::Path::new(&path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = format!("{path}.tmp");
    if serde_json::to_string_pretty(&j)
        .ok()
        .and_then(|s| std::fs::write(&tmp, s).ok())
        .is_some()
    {
        let _ = std::fs::rename(&tmp, &path);
    }
}

// ─── wallet reconstruction ──────────────────────────────────────────────────

struct Wallet {
    acct: ShieldedAccount,
    store: NoteStore,
    /// Highest derivation index observed on chain — new outputs start above it,
    /// so a stateless rebuild never re-issues an index already in use.
    max_index: u64,
    /// Origin label per note, parallel to `store.notes`, purely for reporting.
    origins: Vec<&'static str>,
    spent: HashSet<String>,
}

struct ScanOpts {
    index_scan: u64,
    coinbase_window: u64,
    height: u64,
}

impl ScanOpts {
    fn from(a: &Value) -> Self {
        Self {
            index_scan: arg_u64(a, "index_scan", DEFAULT_INDEX_SCAN),
            coinbase_window: arg_u64(a, "coinbase_window", DEFAULT_COINBASE_WINDOW),
            height: arg_u64(a, "height", 0),
        }
    }
}

/// Rebuild everything this seed owns from the chain alone.
fn recover(seed: [u8; 32], pool: &Pool, opts: &ScanOpts) -> Wallet {
    let acct = ShieldedAccount::from_seed(seed);
    let enc = enc_identity_from_seed(&seed);
    let mut store = NoteStore::new();
    let mut origins: Vec<&'static str> = Vec::new();
    let mut max_index = 0u64;

    // 1. Payments (and our own change) delivered as sealed ciphertexts. A
    //    successful open is the ownership proof; nothing marks a ciphertext as
    //    ours, which is exactly why an observer cannot tell who was paid.
    for c in pool.ciphertexts.iter().flatten() {
        if c.is_empty() {
            continue;
        }
        if let Ok(pt) = try_open_note(&NoteCiphertext(c.clone()), &enc) {
            let memo = (!pt.memo.is_empty()).then(|| pt.memo.text());
            if store.receive_with_memo(pt.value, pt.blinding, memo) {
                origins.push("received");
            }
        }
    }

    let leafset: HashSet<[u8; 32]> = pool.leaves.iter().copied().collect();
    let denoms = denominations();

    // 2. Deposits we shielded ourselves: deterministic blinding per index, and a
    //    shield amount is always a denomination, so this grid is complete.
    for idx in 0..opts.index_scan {
        for d in &denoms {
            let Ok(note) = acct.note(idx, *d) else { continue };
            if leafset.contains(&to_wire(note.commitment())) {
                if store.receive(*d, acct.blinding(idx)) {
                    origins.push("self-shielded");
                }
                max_index = max_index.max(idx);
            }
        }
    }

    // 3. Mining rewards: a coinbase blinding is publicly derivable from
    //    (height, pk_shield), so the miner needs no bookkeeping at all.
    let tip = if opts.height > 0 { opts.height } else { tip_height() };
    if tip > 0 && opts.coinbase_window > 0 {
        let pk_wire = to_wire(acct.public_key());
        let lo = tip.saturating_sub(opts.coinbase_window);
        for h in lo..=tip {
            for d in &denoms {
                if let Some(cm) = coinbase_commitment_wire(h, &pk_wire, *d as u128) {
                    if leafset.contains(&cm) {
                        if store.receive(*d, coinbase_blinding(h, acct.public_key())) {
                            origins.push("coinbase");
                        }
                    }
                }
            }
        }
    }

    // 4. Resolve leaf positions — required before anything can be spent.
    let full = padded(pool);
    store.scan_owned(&acct, &full);

    let spent: HashSet<String> = journal_load()
        .get(&hex::encode(to_wire(acct.public_key())))
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect();

    origins.resize(store.notes.len(), "unknown");
    Wallet { acct, store, max_index, origins, spent }
}

impl Wallet {
    fn pk_shield_hex(&self) -> String {
        hex::encode(to_wire(self.acct.public_key()))
    }

    /// Is this note already spent, per the local journal? (The chain is the
    /// authority; this only avoids a guaranteed-rejected submission.)
    fn is_spent(&self, i: usize) -> bool {
        let Some(pos) = self.store.notes[i].position else { return false };
        self.spent.contains(&hex::encode(to_wire(self.acct.nullifier_at(pos))))
    }

    fn notes_json(&self) -> Vec<Value> {
        self.store
            .notes
            .iter()
            .enumerate()
            .map(|(i, n)| {
                json!({
                    "store_position": i,
                    "value": n.value,
                    "leaf_position": n.position,
                    "spendable": n.position.is_some() && !n.spent && !self.is_spent(i),
                    "origin": self.origins.get(i).copied().unwrap_or("unknown"),
                    "memo": n.memo,
                })
            })
            .collect()
    }

    /// Smallest unspent, landed note covering `need` — smallest-first so large
    /// notes stay whole for larger payments.
    fn select(&self, need: u64) -> Option<usize> {
        let mut best: Option<(usize, u64)> = None;
        for (i, n) in self.store.notes.iter().enumerate() {
            if n.position.is_none() || n.spent || self.is_spent(i) || n.value < need {
                continue;
            }
            if best.map(|(_, v)| n.value < v).unwrap_or(true) {
                best = Some((i, n.value));
            }
        }
        best.map(|(i, _)| i)
    }

    fn spendable_balance(&self) -> u128 {
        self.store
            .notes
            .iter()
            .enumerate()
            .filter(|(i, n)| n.position.is_some() && !n.spent && !self.is_spent(*i))
            .map(|(_, n)| n.value as u128)
            .sum()
    }

    /// Advance the derivation counter past every index already on chain, WITHOUT
    /// disturbing `store.notes` (whose ordering `build_spend`'s `store_position`
    /// argument indexes into). A fresh `NoteStore` starts at index 0, so without
    /// this a change note would re-use an index an earlier deposit already used —
    /// same value at the same index means the same commitment.
    fn advance_indices(&mut self) {
        let keep = self.store.notes.len();
        for _ in 0..=self.max_index {
            self.store.allocate_with(&self.acct, 0);
        }
        self.store.notes.truncate(keep);
    }
}


/// Build + PROVE the spend and seal one delivery ciphertext per output.
///
/// The single code path behind both a shielded send and an unshield — they differ
/// only in which slot the public value occupies. `to_amount` goes to `recipient`;
/// everything left after `public_value` returns to us as change. For an unshield,
/// `recipient` is our OWN address and `to_amount` is 0, so both outputs stay ours.
///
/// Output 1 (the change) is always sealed to the sender's own address. The
/// reference `live_shielded_test` example sent `null` there, which left change
/// unrecoverable once the process exited — a stateless MCP cannot afford that.
fn prove_and_seal(
    w: &mut Wallet,
    seed: &[u8; 32],
    pool: &Pool,
    pos: usize,
    public_value: u64,
    to_amount: u64,
    recipient: &ShieldedAddress,
    memo: &str,
) -> Result<(SpendBundle, Vec<Value>), String> {
    let recipient_pk = recipient
        .shield_key()
        .map_err(|e| format!("recipient pk_shield is not a valid field element: {e}"))?;
    let value = w.store.notes.get(pos).map(|n| n.value).ok_or("no such note")?;
    let change = value
        .checked_sub(public_value)
        .and_then(|v| v.checked_sub(to_amount))
        .ok_or_else(|| format!("note of {value} cannot cover {to_amount} + {public_value}"))?;

    w.advance_indices();
    let outs = [(to_amount, recipient_pk), (change, w.acct.public_key())];
    let full = padded(pool);
    let bundle = build_spend(&w.acct, &mut w.store, &full, pos, public_value, &outs)
        .map_err(|e| format!("build_spend: {e}"))?;

    let self_addr = w.acct.address(seed);
    let targets = [recipient, &self_addr];
    let cts: Vec<Value> = (0..2)
        .map(|i| {
            let (v, b) = bundle.out_preimages[i];
            // The memo goes to the recipient only; the change note back to ourselves
            // carries none (we wrote it, we do not need to be told).
            let pt = if i == 0 {
                match NotePlaintext::new(v, b).with_memo(memo) {
                    Ok(p) => p,
                    Err(_) => return Value::Null,
                }
            } else {
                NotePlaintext::new(v, b)
            };
            match seal_note(&pt, targets[i]) {
                Ok(ct) => json!(ct.0),
                Err(_) => Value::Null,
            }
        })
        .collect();
    if cts[0].is_null() {
        return Err(format!(
            "could not seal the recipient's note ciphertext — check pk_encrypt, and that the \
             memo is at most {} bytes",
            sigil_shield::note_cipher::MEMO_LEN
        ));
    }
    Ok((bundle, cts))
}

// ─── recipient resolution ───────────────────────────────────────────────────

/// Accepts either a `sigil1s:<pk_shield>:<pk_encrypt>` address, an explicit
/// `pk_shield`+`pk_encrypt` pair, or a transparent 64-hex wallet id whose owner
/// has published a shielded address via `flux_sigil_shielded_register`.
fn resolve_recipient(a: &Value) -> Result<ShieldedAddress, String> {
    let addr = arg_str(a, "to_address", "");
    if !addr.is_empty() {
        return ShieldedAddress::decode(addr.trim())
            .map_err(|e| format!("to_address is not a sigil1s address: {e}"));
    }
    let (pks, pke) = (arg_str(a, "pk_shield", ""), arg_str(a, "pk_encrypt", ""));
    if !pks.is_empty() && !pke.is_empty() {
        return Ok(ShieldedAddress { pk_shield: pks, pk_enc: pke });
    }
    let wallet = arg_str(a, "to", "");
    if wallet.is_empty() {
        return Err("need one of: to_address (sigil1s:...), pk_shield+pk_encrypt, or to (64-hex wallet)".into());
    }
    let raw = rpc_get(&format!("/v1/shielded/address?wallet={wallet}"));
    let v: Value = serde_json::from_str(&raw).map_err(|e| format!("shielded/address: {e}"))?;
    if !v["ok"].as_bool().unwrap_or(false) {
        return Err(format!(
            "wallet {wallet} has no published shielded address ({}). It must call \
             flux_sigil_shielded_register first, or pay it via to_address instead.",
            v["error"].as_str().unwrap_or("lookup failed")
        ));
    }
    let (Some(pks), Some(pke)) = (v["pk_shield"].as_str(), v["pk_encrypt"].as_str()) else {
        return Err("published address is missing pk_shield or pk_encrypt".into());
    };
    Ok(ShieldedAddress { pk_shield: pks.to_string(), pk_enc: pke.to_string() })
}

// ─── tools ──────────────────────────────────────────────────────────────────

pub fn register(registry: &mut ToolRegistry) {
    registry.register(ToolDef {
        name: "flux_sigil_shielded_keys",
        description: "Derive every SIGIL shielded identity from one seed: the transparent \
                      wallet id (ed25519 pubkey), pk_shield (the circuit key a note \
                      commitment binds to), pk_encrypt (the X25519 key note ciphertexts are \
                      sealed to), and the single copy-pasteable `sigil1s:` address. Args: \
                      seed (32-byte hex; omit to GENERATE a fresh one — save it, it is the \
                      only way to ever spend the notes). Nothing is stored or transmitted.",
        input_schema: json!({"type":"object","properties":{"seed":{"type":"string"}}}),
    }, shielded_keys);

    registry.register(ToolDef {
        name: "flux_sigil_shielded_notes",
        description: "What this seed can actually SPEND. Rebuilds the wallet from the chain \
                      alone: trial-decrypts every delivery ciphertext in the pool (received \
                      payments and our own change), re-derives self-shielded deposits over an \
                      index x denomination grid, and reconstructs mining rewards from \
                      (height, pk_shield). Returns each note with its leaf position and \
                      whether it is spendable, plus the spendable balance and the current \
                      anchor. Args: seed (hex), index_scan (default 64), coinbase_window \
                      (default 512 heights), height (default: live tip).",
        input_schema: json!({"type":"object","properties":{"seed":{"type":"string"},"index_scan":{"type":"integer"},"coinbase_window":{"type":"integer"},"height":{"type":"integer"}},"required":["seed"]}),
    }, shielded_notes);

    registry.register(ToolDef {
        name: "flux_sigil_shielded_send",
        description: "Send SIGIL PRIVATELY, end to end, from this process — the whole \
                      ceremony, not a passthrough: scan the pool for a spendable note, build \
                      and PROVE the spend (real STARK via sigil-shield::wallet::build_spend), \
                      seal a note ciphertext to the recipient AND a change ciphertext back to \
                      yourself (so the change is recoverable later), then submit to \
                      POST /v1/shielded_send, which relays via Dandelion++. Carries no sender, \
                      no recipient and no amount on chain. Args: seed (hex, the SENDER), \
                      amount (integer raw base units), and ONE of: to_address \
                      (sigil1s:<pk_shield>:<pk_encrypt>) | pk_shield+pk_encrypt | to (64-hex \
                      wallet that has registered a shielded address). memo (optional, \
                      max 512 UTF-8 bytes) rides sealed inside the recipient's note. broadcast (default \
                      FALSE) — a dry run returns the plan and stops BEFORE proving; pass true \
                      to actually move value. The fee is fixed at 100000 raw and is not \
                      selectable (a chosen fee is a fingerprint).",
        input_schema: json!({"type":"object","properties":{
            "seed":{"type":"string"},"amount":{"type":"integer"},
            "to":{"type":"string"},"to_address":{"type":"string"},
            "pk_shield":{"type":"string"},"pk_encrypt":{"type":"string"},
            "broadcast":{"type":"boolean","default":false},
            "memo":{"type":"string","description":"Optional private message (UTF-8, max 512 bytes) sealed to the recipient with the note; only they can read it, and every ciphertext is padded so its presence leaks nothing."},
            "index_scan":{"type":"integer"},"coinbase_window":{"type":"integer"},"height":{"type":"integer"}
        },"required":["seed","amount"]}),
    }, shielded_send_full);

    registry.register(ToolDef {
        name: "flux_sigil_unshield_from_seed",
        description: "Exit the shielded pool to a transparent wallet, proof built here. Same \
                      scan+prove path as flux_sigil_shielded_send, with the withdrawn amount \
                      in the circuit's public-value slot. The amount MUST be a ramp \
                      denomination (1/2/5 x powers of ten) — call flux_sigil_shield_plan to \
                      split an arbitrary sum. Args: seed (hex), to (64-hex transparent \
                      wallet), amount (integer raw), broadcast (default false).",
        input_schema: json!({"type":"object","properties":{
            "seed":{"type":"string"},"to":{"type":"string"},"amount":{"type":"integer"},
            "broadcast":{"type":"boolean","default":false},
            "index_scan":{"type":"integer"},"coinbase_window":{"type":"integer"},"height":{"type":"integer"}
        },"required":["seed","to","amount"]}),
    }, unshield_full);

    registry.register(ToolDef {
        name: "flux_sigil_shield_from_seed",
        description: "Enter the shielded pool WITHOUT hand-computing a commitment: derives \
                      the next free note index for this seed, computes cm locally (the server \
                      never learns the blinding — that is what makes the note private), signs \
                      the transparent debit with the wallet key and submits POST /v1/shield. \
                      The amount must be a ramp denomination. Args: seed (hex — this is BOTH \
                      the ed25519 wallet key and the shielded seed), amount (integer raw), \
                      broadcast (default false).",
        input_schema: json!({"type":"object","properties":{
            "seed":{"type":"string"},"amount":{"type":"integer"},
            "broadcast":{"type":"boolean","default":false},"index_scan":{"type":"integer"}
        },"required":["seed","amount"]}),
    }, shield_from_seed);

    registry.register(ToolDef {
        name: "flux_sigil_shielded_register_from_seed",
        description: "Publish this seed's shielded address so (a) block rewards mint straight \
                      into the shielded pool and (b) others can pay it by transparent wallet \
                      id alone. Derives wallet/pk_shield/pk_encrypt from the seed and signs \
                      the request locally. Args: seed (hex), broadcast (default false).",
        input_schema: json!({"type":"object","properties":{
            "seed":{"type":"string"},"broadcast":{"type":"boolean","default":false}
        },"required":["seed"]}),
    }, register_from_seed);
}

fn shielded_keys(a: &Value) -> String {
    use rand::RngCore;
    let (seed, generated) = match arg_str(a, "seed", "") {
        s if s.is_empty() => {
            let mut b = [0u8; 32];
            rand::rngs::OsRng.fill_bytes(&mut b);
            (b, true)
        }
        s => match hex32(&s) {
            Some(b) => (b, false),
            None => return err("seed must be 32-byte hex"),
        },
    };
    let acct = ShieldedAccount::from_seed(seed);
    let addr = acct.address(&seed);
    let wallet = {
        use ed25519_dalek::SigningKey;
        hex::encode(SigningKey::from_bytes(&seed).verifying_key().to_bytes())
    };
    let mut out = json!({
        "ok": true,
        "wallet": wallet,
        "pk_shield": addr.pk_shield,
        "pk_encrypt": addr.pk_enc,
        "address": addr.encode(),
        "next": "flux_sigil_shielded_register_from_seed publishes this address on chain; \
                 flux_sigil_shielded_notes shows what it can spend",
    });
    if generated {
        out["seed"] = json!(hex::encode(seed));
        out["warning"] = json!(
            "GENERATED seed — this is the ONLY copy and the only way to ever spend notes \
             paid to this address. Store it before using it. The MCP does not keep it."
        );
    }
    out.to_string()
}

fn shielded_notes(a: &Value) -> String {
    let seed = match seed_of(a) {
        Ok(s) => s,
        Err(e) => return err(e),
    };
    let pool = match fetch_pool() {
        Ok(p) => p,
        Err(e) => return err(e),
    };
    let opts = ScanOpts::from(a);
    let w = recover(seed, &pool, &opts);
    json!({
        "ok": true,
        "wallet_pk_shield": w.pk_shield_hex(),
        "address": w.acct.address(&seed).encode(),
        "spendable_balance": w.spendable_balance().to_string(),
        "notes": w.notes_json(),
        "pool": {"real_leaves": pool.leaves.len(), "capacity": pool.capacity, "anchor": pool.anchor},
        "scanned": {"index_scan": opts.index_scan, "coinbase_window": opts.coinbase_window},
        "note": "a note with leaf_position=null has not landed on chain yet and cannot be \
                 spent. If a reward is missing, widen coinbase_window; if an old deposit is \
                 missing, widen index_scan.",
    })
    .to_string()
}

fn shielded_send_full(a: &Value) -> String {
    let seed = match seed_of(a) {
        Ok(s) => s,
        Err(e) => return err(e),
    };
    let amount = arg_u64(a, "amount", 0);
    if amount == 0 {
        return err("amount must be > 0 (raw base units)");
    }
    let recipient = match resolve_recipient(a) {
        Ok(r) => r,
        Err(e) => return err(e),
    };
    let memo = arg_str(a, "memo", "");
    if memo.len() > sigil_shield::note_cipher::MEMO_LEN {
        return err(format!(
            "memo is {} bytes; the limit is {} (UTF-8 bytes, not characters)",
            memo.len(),
            sigil_shield::note_cipher::MEMO_LEN
        ));
    }
    let pool = match fetch_pool() {
        Ok(p) => p,
        Err(e) => return err(e),
    };
    let opts = ScanOpts::from(a);
    let mut w = recover(seed, &pool, &opts);

    let need = amount + SHIELDED_FEE;
    let Some(pos) = w.select(need) else {
        return json!({
            "ok": false,
            "error": format!("no single spendable note covers {need} (amount {amount} + fee {SHIELDED_FEE})"),
            "spendable_balance": w.spendable_balance().to_string(),
            "notes": w.notes_json(),
            "hint": "a spend consumes exactly ONE input note. Shield a larger denomination, \
                     or send an amount a single note covers.",
        })
        .to_string();
    };
    let value = w.store.notes[pos].value;
    let change = value - need;

    if !arg_bool(a, "broadcast", false) {
        return json!({
            "ok": true, "dry_run": true,
            "plan": {
                "input_note": {"store_position": pos, "value": value,
                               "leaf_position": w.store.notes[pos].position},
                "to_amount": amount, "fee": SHIELDED_FEE, "change_back_to_self": change,
                "recipient_address": recipient.encode(),
                "anchor": pool.anchor,
            },
            "next": "re-call with broadcast:true to build the STARK proof and submit. \
                     Proving takes a few seconds and only happens on the real call.",
        })
        .to_string();
    }

    let (bundle, cts) = match prove_and_seal(&mut w, &seed, &pool, pos, SHIELDED_FEE, amount, &recipient, &memo) {
        Ok(x) => x,
        Err(e) => return err(e),
    };

    let body = json!({
        "anchor": hex::encode(bundle.anchor),
        "nullifier": hex::encode(bundle.nullifier),
        "cm_outs": bundle.cm_outs.iter().map(hex::encode).collect::<Vec<_>>(),
        "fee": SHIELDED_FEE.to_string(),
        "proof": hex::encode(&bundle.proof),
        "note_ciphertexts": cts,
    });
    submit("/v1/shielded_send", &body, &w, &bundle.nullifier, json!({
        "sent": amount, "fee": SHIELDED_FEE, "change": change,
        "recipient_address": recipient.encode(),
    }))
}

fn unshield_full(a: &Value) -> String {
    let seed = match seed_of(a) {
        Ok(s) => s,
        Err(e) => return err(e),
    };
    let to = arg_str(a, "to", "");
    if hex32(&to).is_none() {
        return err("to must be a 64-hex transparent wallet id");
    }
    let amount = arg_u64(a, "amount", 0);
    if !denominations().contains(&amount) {
        return err(format!(
            "amount {amount} is not a ramp denomination (1/2/5 x powers of ten). The chain \
             rejects any other exit amount — use flux_sigil_shield_plan to split it."
        ));
    }
    let pool = match fetch_pool() {
        Ok(p) => p,
        Err(e) => return err(e),
    };
    let opts = ScanOpts::from(a);
    let mut w = recover(seed, &pool, &opts);

    let Some(pos) = w.select(amount) else {
        return json!({
            "ok": false,
            "error": format!("no single spendable note covers {amount}"),
            "spendable_balance": w.spendable_balance().to_string(),
            "notes": w.notes_json(),
        })
        .to_string();
    };
    let value = w.store.notes[pos].value;
    let change = value - amount;

    if !arg_bool(a, "broadcast", false) {
        return json!({
            "ok": true, "dry_run": true,
            "plan": {"input_note": {"store_position": pos, "value": value},
                     "withdraw": amount, "change_back_to_pool": change, "to": to,
                     "anchor": pool.anchor},
            "next": "re-call with broadcast:true to prove and submit",
        })
        .to_string();
    }

    // Both outputs stay ours: the change, and a zero note that keeps the output
    // count fixed at N_OUTS (a variable output count would itself leak).
    let self_addr = w.acct.address(&seed);
    let (bundle, cts) = match prove_and_seal(&mut w, &seed, &pool, pos, amount, 0, &self_addr, "") {
        Ok(x) => x,
        Err(e) => return err(e),
    };
    let body = json!({
        "to": to,
        "amount": amount.to_string(),
        "anchor": hex::encode(bundle.anchor),
        "nullifier": hex::encode(bundle.nullifier),
        "cm_outs": bundle.cm_outs.iter().map(hex::encode).collect::<Vec<_>>(),
        "proof": hex::encode(&bundle.proof),
        "fee": "0",
        "note_ciphertexts": cts,
    });
    submit("/v1/unshield", &body, &w, &bundle.nullifier, json!({
        "withdrawn": amount, "to": to, "change_back_to_pool": change,
    }))
}

/// POST a proof-carrying spend and, on success, journal its nullifier so a later
/// stateless rescan does not offer the same note again.
fn submit(path: &str, body: &Value, w: &Wallet, nullifier: &[u8; 32], summary: Value) -> String {
    match rpc_post(path, &body.to_string()) {
        Ok(raw) => {
            let v: Value = serde_json::from_str(&raw).unwrap_or_else(|_| json!({"raw": raw}));
            if v["ok"].as_bool().unwrap_or(false) {
                journal_record(&w.pk_shield_hex(), &hex::encode(nullifier));
            }
            json!({
                "ok": v["ok"].as_bool().unwrap_or(false),
                "endpoint": path,
                "response": v,
                "summary": summary,
                "note": "queued for the next braid block — a txid here is an ACCEPTED \
                         submission, not yet a settled one. Re-run flux_sigil_shielded_notes \
                         after a block to see the new note.",
            })
            .to_string()
        }
        Err(e) => json!({"ok": false, "error": e, "endpoint": path}).to_string(),
    }
}

fn shield_from_seed(a: &Value) -> String {
    use ed25519_dalek::{Signer, SigningKey};
    let seed = match seed_of(a) {
        Ok(s) => s,
        Err(e) => return err(e),
    };
    let amount = arg_u64(a, "amount", 0);
    if !denominations().contains(&amount) {
        return err(format!(
            "amount {amount} is not a ramp denomination (1/2/5 x powers of ten) — the chain \
             rejects it. Use flux_sigil_shield_plan to split an arbitrary sum."
        ));
    }
    let pool = match fetch_pool() {
        Ok(p) => p,
        Err(e) => return err(e),
    };
    let acct = ShieldedAccount::from_seed(seed);
    let leafset: HashSet<[u8; 32]> = pool.leaves.iter().copied().collect();
    // Next index whose commitment for THIS amount is not already on chain —
    // deterministic and stateless, and it keeps two equal deposits distinct
    // (same index + same value would produce the same commitment twice).
    let scan = arg_u64(a, "index_scan", DEFAULT_INDEX_SCAN).max(1);
    let mut chosen: Option<(u64, [u8; 32])> = None;
    for idx in 0..scan {
        let Ok(note) = acct.note(idx, amount) else { continue };
        let cm = to_wire(note.commitment());
        if !leafset.contains(&cm) {
            chosen = Some((idx, cm));
            break;
        }
    }
    let Some((index, cm)) = chosen else {
        return err(format!("every derivation index below {scan} already holds a note of {amount} — raise index_scan"));
    };

    let sk = SigningKey::from_bytes(&seed);
    let from = hex::encode(sk.verifying_key().to_bytes());
    let cm_hex = hex::encode(cm);
    let fee = "0";
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    if !arg_bool(a, "broadcast", false) {
        return json!({
            "ok": true, "dry_run": true,
            "plan": {"from": from, "amount": amount, "note_index": index, "cm": cm_hex},
            "next": "re-call with broadcast:true to sign and submit POST /v1/shield",
        })
        .to_string();
    }

    let msg = format!("sigil-rpc/v1|shield|{from}|{amount}|{cm_hex}|{fee}|nonce={nonce}");
    let sig = hex::encode(sk.sign(msg.as_bytes()).to_bytes());
    let body = json!({
        "from": from, "amount": amount.to_string(), "cm": cm_hex,
        "fee": fee, "sig": sig, "req_nonce": nonce,
    });
    match rpc_post("/v1/shield", &body.to_string()) {
        Ok(raw) => {
            let v: Value = serde_json::from_str(&raw).unwrap_or_else(|_| json!({"raw": raw}));
            json!({
                "ok": v["ok"].as_bool().unwrap_or(false), "response": v,
                "summary": {"shielded": amount, "note_index": index, "cm": cm_hex},
                "note": "the blinding for this note is derived from your seed at this index — \
                         the seed alone recovers it, nothing extra to back up.",
            })
            .to_string()
        }
        Err(e) => json!({"ok": false, "error": e, "endpoint": "/v1/shield"}).to_string(),
    }
}

fn register_from_seed(a: &Value) -> String {
    use ed25519_dalek::{Signer, SigningKey};
    let seed = match seed_of(a) {
        Ok(s) => s,
        Err(e) => return err(e),
    };
    let acct = ShieldedAccount::from_seed(seed);
    let addr = acct.address(&seed);
    let sk = SigningKey::from_bytes(&seed);
    let wallet = hex::encode(sk.verifying_key().to_bytes());
    let (pk_shield, pk_encrypt) = (addr.pk_shield.clone(), addr.pk_enc.clone());
    let fee = "0";
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    if !arg_bool(a, "broadcast", false) {
        return json!({
            "ok": true, "dry_run": true,
            "plan": {"wallet": wallet, "pk_shield": pk_shield, "pk_encrypt": pk_encrypt,
                     "address": addr.encode()},
            "next": "re-call with broadcast:true to sign and submit POST /v1/shielded/register",
        })
        .to_string();
    }
    let msg = format!("sigil-rpc/v1|shield-register|{wallet}|{pk_shield}|{pk_encrypt}|{fee}|nonce={nonce}");
    let sig = hex::encode(sk.sign(msg.as_bytes()).to_bytes());
    let body = json!({
        "wallet": wallet, "pk_shield": pk_shield, "pk_encrypt": pk_encrypt,
        "fee": fee, "sig": sig, "req_nonce": nonce,
    });
    match rpc_post("/v1/shielded/register", &body.to_string()) {
        Ok(raw) => raw,
        Err(e) => json!({"ok": false, "error": e, "endpoint": "/v1/shielded/register"}).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ladder this module enforces must be the one the chain enforces —
    /// 1/2/5 x 10^0..10^15, 48 rungs, reaching both extremes. A drift here means
    /// every shield/unshield we plan gets rejected at the chokepoint.
    #[test]
    fn denomination_ladder_matches_the_chain() {
        let d = denominations();
        assert_eq!(d.len(), 48);
        assert_eq!(d[0], 1);
        assert_eq!(*d.last().unwrap(), 5_000_000_000_000_000);
        assert!(d.contains(&1_000), "the fixed fee must itself be a rung");
        assert!(d.windows(2).all(|w| w[0] < w[1]), "must be sorted and unique");
    }

    /// A round-trip through the real crypto: an account seals a note to its own
    /// address and recovers `(value, blinding)` — this is exactly the mechanism
    /// that makes CHANGE recoverable after this process exits, which the
    /// reference example did not do.
    #[test]
    fn change_sealed_to_self_reopens() {
        let seed = [9u8; 32];
        let acct = ShieldedAccount::from_seed(seed);
        let enc = enc_identity_from_seed(&seed);
        let pt = NotePlaintext::new(4_242, acct.blinding(3));
        let ct = seal_note(&pt, &acct.address(&seed)).expect("seal to self");
        let back = try_open_note(&ct, &enc).expect("own ciphertext must reopen");
        assert_eq!(back, pt);
    }

    /// A ciphertext addressed to someone else must NOT open — the AEAD tag is
    /// the whole ownership test during a scan, so if this ever passes vacuously
    /// the note-discovery path is theater.
    #[test]
    fn foreign_ciphertext_does_not_open() {
        let mine = ShieldedAccount::from_seed([1u8; 32]);
        let theirs = ShieldedAccount::from_seed([2u8; 32]);
        let pt = NotePlaintext::new(7, theirs.blinding(0));
        let ct = seal_note(&pt, &theirs.address(&[2u8; 32])).expect("seal");
        assert!(try_open_note(&ct, &enc_identity_from_seed(&[1u8; 32])).is_err());
        let _ = mine;
    }

    /// `advance_indices` must move the derivation counter past everything on
    /// chain WITHOUT disturbing `store.notes` — `build_spend` indexes into that
    /// vector by position, so a stray entry would make it prove the wrong note.
    #[test]
    fn advance_indices_preserves_note_ordering() {
        let seed = [5u8; 32];
        let acct = ShieldedAccount::from_seed(seed);
        let mut store = NoteStore::new();
        store.receive(10, acct.blinding(0));
        store.receive(20, acct.blinding(1));
        let mut w = Wallet {
            acct: acct.clone(),
            store,
            max_index: 4,
            origins: vec!["received", "received"],
            spent: HashSet::new(),
        };
        w.advance_indices();
        assert_eq!(w.store.notes.len(), 2, "no dummy notes may survive");
        assert_eq!(w.store.notes[0].value, 10);
        assert_eq!(w.store.notes[1].value, 20);
        // The next allocation must land above every index already on chain.
        let next = w.store.allocate_with(&w.acct, 1);
        assert!(next > 4, "new outputs must not re-use an on-chain index (got {next})");
    }

    /// Build a synthetic pool the way the chain would see it.
    fn pool_of(leaves: Vec<[u8; 32]>, cts: Vec<Option<String>>) -> Pool {
        Pool { leaves, ciphertexts: cts, capacity: 32_768, anchor: String::new() }
    }
    fn offline() -> ScanOpts {
        // height>0 + coinbase_window=0 keeps `recover` entirely off the network.
        ScanOpts { index_scan: 8, coinbase_window: 0, height: 1 }
    }

    /// THE ACCEPTANCE GATE for this module. Alice shields a note, this module
    /// reconstructs her wallet from the pool alone, proves a spend to Bob, and the
    /// resulting proof is checked with `note_v1::verify_spend_wire` — the EXACT
    /// function `sigil-api`'s `precheck_proof` and the chokepoint's
    /// `verify_spend_proof` call. If this passes, a proof produced by
    /// `flux_sigil_shielded_send` is one the live node accepts; if it ever passes
    /// vacuously, this whole surface is theater.
    ///
    /// It then re-runs recovery as BOB against the post-spend pool, proving the
    /// payment is actually findable by its recipient — a payment nobody can find
    /// is value burned, and that is the failure the delivery ciphertext exists to
    /// prevent.
    #[test]
    fn alice_pays_bob_and_the_chain_verifier_accepts() {
        std::env::set_var("FLUX_SIGIL_SHIELDED_JOURNAL", "/tmp/flux-sigil-shielded-test.json");
        let (alice_seed, bob_seed) = ([11u8; 32], [22u8; 32]);
        let alice = ShieldedAccount::from_seed(alice_seed);
        let bob = ShieldedAccount::from_seed(bob_seed);

        // Alice's shielded deposit of 1,000,000 at derivation index 0 — exactly what
        // `flux_sigil_shield_from_seed` publishes.
        let deposit = alice.note(0, 1_000_000).expect("note").commitment();
        let pool = pool_of(vec![to_wire(deposit)], vec![None]);

        // Stateless reconstruction: no local state, just the pool.
        let mut w = recover(alice_seed, &pool, &offline());
        assert_eq!(w.spendable_balance(), 1_000_000, "the deposit must be re-derivable from the seed");
        let pos = w.select(500 + SHIELDED_FEE).expect("a note covering amount+fee");

        let bob_addr = bob.address(&bob_seed);
        let (bundle, cts) =
            prove_and_seal(&mut w, &alice_seed, &pool, pos, SHIELDED_FEE, 500, &bob_addr, "")
                .expect("prove");

        // The node's own door check. Same signature, same arguments.
        sigil_shield::note_v1::verify_spend_wire(
            &bundle.anchor,
            &bundle.nullifier,
            SHIELDED_FEE as u128,
            &bundle.cm_outs,
            &bundle.proof,
        )
        .expect("the live node's verifier must accept a proof this module produced");

        // Value conservation, stated explicitly: 500 out, 1000 fee, rest as change.
        assert_eq!(bundle.out_preimages[0].0, 500);
        assert_eq!(bundle.out_preimages[1].0, 1_000_000 - 500 - SHIELDED_FEE);

        // Bob's side: the pool now holds the two outputs with their ciphertexts.
        let after = pool_of(
            bundle.cm_outs.clone(),
            cts.iter().map(|c| c.as_str().map(|s| s.to_string())).collect(),
        );
        let bob_w = recover(bob_seed, &after, &offline());
        assert_eq!(bob_w.spendable_balance(), 500, "Bob must be able to FIND what he was paid");

        // And Alice finds her change the same way — the property the reference
        // example's `null` change ciphertext did not have.
        let alice_after = recover(alice_seed, &after, &offline());
        assert_eq!(alice_after.spendable_balance(), (1_000_000 - 500 - SHIELDED_FEE) as u128);
    }

    /// A tampered proof must be REJECTED by that same verifier. Without this the
    /// test above could pass against a verifier that accepts anything.
    #[test]
    fn tampered_proof_is_rejected() {
        std::env::set_var("FLUX_SIGIL_SHIELDED_JOURNAL", "/tmp/flux-sigil-shielded-test.json");
        let seed = [33u8; 32];
        let acct = ShieldedAccount::from_seed(seed);
        let pool = pool_of(vec![to_wire(acct.note(0, 100_000).unwrap().commitment())], vec![None]);
        let mut w = recover(seed, &pool, &offline());
        let pos = w.select(SHIELDED_FEE).expect("note");
        let addr = acct.address(&seed);
        let (bundle, _) = prove_and_seal(&mut w, &seed, &pool, pos, SHIELDED_FEE, 0, &addr, "").unwrap();

        let mut bad = bundle.proof.clone();
        let n = bad.len() / 2;
        bad[n] ^= 0xff;
        assert!(
            sigil_shield::note_v1::verify_spend_wire(
                &bundle.anchor, &bundle.nullifier, SHIELDED_FEE as u128, &bundle.cm_outs, &bad,
            ).is_err(),
            "SECURITY: a tampered proof must not verify"
        );
    }
}
