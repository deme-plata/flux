//! What a local answer costs, and who gets paid for it.
//!
//! # The thing to be honest about first
//!
//! flux-moe runs the model on **the user's own GPU and CPU**. Nobody's electricity but
//! theirs is spent, and no chain is touched by the act of answering. So a "price" here is
//! not a payment for compute in the way a cloud bill is — it is a **meter reading**: how
//! much work was actually done, priced by a published rate, so that a node offering that
//! work to OTHERS has something to invoice, and so the person who wrote the software can
//! take a share the way a mining pool takes a dev fee.
//!
//! Three consequences, all of them baked in below:
//!
//! 1. **The token counts are self-reported.** They come from the inference server's own
//!    reply (`prompt_eval_count` / `eval_count`). The network does not and cannot verify
//!    them: reproducing the count means re-running the model. Every record therefore
//!    carries `attested: false`, and any consumer that treats it as proof is wrong.
//!    Making it verifiable is a real research problem, not a TODO — see [`Charge::caveat`].
//! 2. **Nothing is ever paid automatically.** This module computes a
//!    [`SettlementProposal`] and stops. Signing and sending is a separate, human-gated
//!    step, and a proposal with no destination address is explicitly `payable: false`
//!    rather than a zero-address transfer.
//! 3. **Per-answer settlement is absurd** on a chain whose fee exceeds the charge. Usage
//!    accrues in a local [`Ledger`] and settles in batches, so the on-chain footprint is
//!    one transfer per batch instead of one per sentence.
//!
//! # The split (operator ruling, 2026-09-06)
//!
//! A node operator who has configured an **admin wallet** earns **0.1 % (10 bps)** of what
//! their node meters — the *IOU FÆLLED* share, paid for keeping a node up and behaving.
//! Everything else goes to the **master dev bank fee account**. An operator who set no
//! admin wallet earns nothing and the whole charge goes to master; that is not a penalty,
//! it is simply that there is nowhere to send it, and inventing a destination would be
//! worse than paying none.
//!
//! 10 bps is the same operator-pool share SIGIL's mining coinbase already uses
//! (`sigil-node/src/coinbase.rs`), so there is one fee vocabulary on this chain, not two.
//!
//! One honest note on the word *profit*: nothing here measures cost. Electricity, hardware
//! amortisation and the operator's time are not inputs to any of this, so the 0.1 % is
//! 0.1 % of **metered revenue**, not of profit. Calling it profit would be a claim the
//! code cannot support.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// SIGIL's base unit. 1 SIGIL = 10^10 glyphs (`sigil-state::SIGIL_DECIMALS` = 10).
pub const GLYPHS_PER_SIGIL: u128 = 10_000_000_000;

/// The node operator's share for good behaviour — **IOU FÆLLED**, 0.1 % — paid ONLY to an
/// operator who configured an admin wallet. Same 10 bps as the mining coinbase's operator
/// pool.
pub const IOU_FAELLED_BPS: u32 = 10;

/// Where the work actually ran. Recorded because it is the honest answer to "what did
/// this cost?" — a GPU answer and a CPU answer are the same tokens and very different
/// electricity, and a future rate table may want to say so.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Device {
    Cpu,
    /// GPU with its reported name, e.g. `"NVIDIA GeForce RTX 2080"`.
    Gpu(String),
    /// The server did not say. Never guessed.
    Unknown,
}

impl Default for Device {
    fn default() -> Self {
        Device::Unknown
    }
}

/// One measured generation. Every field is reported by the inference server; nothing here
/// is inferred, and `None` means "the server did not say" rather than zero.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub model: String,
    /// Tokens in the prompt (ollama: `prompt_eval_count`).
    pub prompt_tokens: u64,
    /// Tokens generated (ollama: `eval_count`).
    pub completion_tokens: u64,
    /// Wall-clock nanoseconds spent generating (ollama: `eval_duration`), when reported.
    pub eval_duration_ns: Option<u64>,
    pub device: Device,
    /// Unix milliseconds when this was recorded.
    pub at_ms: u128,
}

impl Usage {
    pub fn total_tokens(&self) -> u64 {
        self.prompt_tokens.saturating_add(self.completion_tokens)
    }

    /// Decode tokens per second from what the server reported. `None` when it reported no
    /// duration, or a zero one — never a fabricated rate.
    pub fn tokens_per_second(&self) -> Option<f64> {
        match self.eval_duration_ns {
            Some(ns) if ns > 0 && self.completion_tokens > 0 => {
                Some(self.completion_tokens as f64 / (ns as f64 / 1e9))
            }
            _ => None,
        }
    }

    /// Read an ollama `/api/chat` or `/api/generate` response.
    ///
    /// Returns `None` when the reply carries no token counts at all — a streaming chunk,
    /// or an error body. That is deliberate: a charge of zero and "we do not know what
    /// this cost" are different answers, and only one of them is honest.
    pub fn from_ollama(model: &str, v: &Value) -> Option<Self> {
        let prompt = v.get("prompt_eval_count").and_then(Value::as_u64);
        let completion = v.get("eval_count").and_then(Value::as_u64);
        if prompt.is_none() && completion.is_none() {
            return None;
        }
        Some(Usage {
            model: model.to_string(),
            prompt_tokens: prompt.unwrap_or(0),
            completion_tokens: completion.unwrap_or(0),
            eval_duration_ns: v.get("eval_duration").and_then(Value::as_u64),
            device: Device::Unknown,
            at_ms: now_ms(),
        })
    }

    pub fn with_device(mut self, d: Device) -> Self {
        self.device = d;
        self
    }
}

/// The published price, in glyphs per 1000 tokens. Generation is dearer than reading a
/// prompt because it is the part that is memory-bandwidth bound and actually slow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rates {
    pub glyphs_per_1k_prompt: u128,
    pub glyphs_per_1k_completion: u128,
}

impl Default for Rates {
    /// A deliberately tiny default: 0.00001 SIGIL per 1k prompt tokens and 0.00004 per 1k
    /// generated. At those rates a 1000-token answer costs 0.00005 SIGIL — a rounding
    /// error, on purpose. **This number is a placeholder, not a market price.** Nobody has
    /// paid it, no demand curve has been measured, and the right rate cannot be derived
    /// from an empty market. It exists so the plumbing is exercised by a real figure
    /// instead of a zero that hides arithmetic bugs.
    fn default() -> Self {
        Rates { glyphs_per_1k_prompt: 100_000, glyphs_per_1k_completion: 400_000 }
    }
}

impl Rates {
    /// Charge for one generation, in glyphs. Integer maths throughout — a price computed
    /// in floating point and then rounded is how a ledger acquires a slow leak.
    pub fn charge_glyphs(&self, u: &Usage) -> u128 {
        let p = u.prompt_tokens as u128 * self.glyphs_per_1k_prompt / 1000;
        let c = u.completion_tokens as u128 * self.glyphs_per_1k_completion / 1000;
        p + c
    }
}

/// How a charge is divided: the operator's IOU FÆLLED share, and the master dev bank fee
/// account which takes everything else.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Split {
    /// The operator's share in basis points, earned only with an admin wallet configured.
    pub iou_faelled_bps: u32,
}

impl Default for Split {
    fn default() -> Self {
        Split { iou_faelled_bps: IOU_FAELLED_BPS }
    }
}

impl Split {
    /// Rejects a share larger than the whole charge. A fee past 10 000 bps is not a
    /// rounding question, it is a bug that would mint value.
    pub fn validate(&self) -> Result<(), String> {
        if self.iou_faelled_bps > 10_000 {
            return Err(format!(
                "IOU FÆLLED share is {} bps of 10000 — more than the whole charge",
                self.iou_faelled_bps
            ));
        }
        Ok(())
    }

    /// Divide `gross` into `(master, operator)`.
    ///
    /// `operator_has_admin_wallet` is the whole rule: without one the operator's share is
    /// zero and master takes everything, because there is no address to pay. Master's cut
    /// is the REMAINDER, computed by subtraction, so the two parts always re-sum to
    /// exactly `gross` however the integer division rounded.
    pub fn apply(&self, gross: u128, operator_has_admin_wallet: bool) -> (u128, u128) {
        let operator = if operator_has_admin_wallet {
            gross * self.iou_faelled_bps as u128 / 10_000
        } else {
            0
        };
        (gross - operator, operator)
    }
}

/// One priced generation: what was measured, what it comes to, and who it is owed to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Charge {
    pub usage: Usage,
    pub rates: Rates,
    pub split: Split,
    pub gross_glyphs: u128,
    /// The master dev bank fee account's share: everything that is not IOU FÆLLED.
    pub master_glyphs: u128,
    /// The operator's IOU FÆLLED share — zero unless an admin wallet was configured.
    pub iou_faelled_glyphs: u128,
    /// Whether an admin wallet was configured when this was metered. Recorded per charge,
    /// because it is the fact that decided the split and a later config change must not
    /// silently rewrite history.
    pub operator_admin_wallet: bool,
    /// Always `false`. The token counts come from the inference server's own reply and
    /// nothing on the network re-derives them.
    pub attested: bool,
}

impl Charge {
    pub fn new(usage: Usage, rates: Rates, split: Split, operator_admin_wallet: bool) -> Result<Self, String> {
        split.validate()?;
        let gross = rates.charge_glyphs(&usage);
        let (master, operator) = split.apply(gross, operator_admin_wallet);
        Ok(Charge {
            usage,
            rates,
            split,
            gross_glyphs: gross,
            master_glyphs: master,
            iou_faelled_glyphs: operator,
            operator_admin_wallet,
            attested: false,
        })
    }

    /// The sentence any UI showing this number must be able to show beside it.
    pub fn caveat(&self) -> &'static str {
        "token counts are self-reported by the machine that ran the model; the network does \
         not verify them, so this is a meter reading, not a proof of work done"
    }

    /// Glyphs as a decimal SIGIL string, for display only.
    pub fn gross_sigil(&self) -> String {
        format_glyphs(self.gross_glyphs)
    }
}

/// Render glyphs as SIGIL with all 10 decimals, trailing zeros trimmed to at least one.
pub fn format_glyphs(g: u128) -> String {
    let whole = g / GLYPHS_PER_SIGIL;
    let frac = g % GLYPHS_PER_SIGIL;
    let mut s = format!("{whole}.{frac:010}");
    while s.ends_with('0') && !s.ends_with(".0") {
        s.pop();
    }
    s
}

pub fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Where the three shares are to be sent. Absent addresses are the normal state today and
/// are what makes a proposal `payable: false` — never a transfer to a zero address.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Payees {
    /// The master dev bank fee account — the lead developer's wallet. Everything that is
    /// not IOU FÆLLED goes here, so without it there is nothing to settle.
    pub master: Option<String>,
    /// The node operator's `--admin-wallet`. Its PRESENCE is what earns the 0.1 %.
    pub admin_wallet: Option<String>,
}

impl Payees {
    /// The operator earns IOU FÆLLED exactly when an admin wallet is configured.
    pub fn earns_iou_faelled(&self) -> bool {
        self.admin_wallet.is_some()
    }

    /// A SIGIL address is 64 hex characters. Anything else is refused here rather than at
    /// signing time, where the failure is expensive and confusing.
    pub fn validate(&self) -> Result<(), String> {
        for (who, a) in [("master", &self.master), ("admin_wallet", &self.admin_wallet)] {
            if let Some(a) = a {
                if a.len() != 64 || !a.bytes().all(|c| c.is_ascii_hexdigit()) {
                    return Err(format!("{who} address is not 64 hex characters"));
                }
            }
        }
        Ok(())
    }
}

/// The local, append-only record of what has been metered and not yet settled.
///
/// Kept as JSON lines so a crash truncates at most the last line, and so a human can read
/// what their machine thinks it is owed without any tooling.
#[derive(Clone, Debug, Default)]
pub struct Ledger {
    path: Option<PathBuf>,
    entries: Vec<Charge>,
}

impl Ledger {
    pub fn in_memory() -> Self {
        Ledger::default()
    }

    /// Open (creating nothing yet) a ledger backed by `path`, loading whatever is already
    /// there. A corrupt line is SKIPPED, not fatal: a half-written last record must not
    /// cost the user the rest of their history.
    pub fn open(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        let entries = std::fs::read_to_string(&path)
            .map(|s| s.lines().filter_map(|l| serde_json::from_str::<Charge>(l).ok()).collect())
            .unwrap_or_default();
        Ledger { path: Some(path), entries }
    }

    pub fn record(&mut self, c: Charge) -> Result<(), String> {
        if let Some(p) = &self.path {
            if let Some(dir) = p.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("ledger dir: {e}"))?;
            }
            let line = serde_json::to_string(&c).map_err(|e| format!("ledger encode: {e}"))?;
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .map_err(|e| format!("ledger open: {e}"))?;
            writeln!(f, "{line}").map_err(|e| format!("ledger write: {e}"))?;
        }
        self.entries.push(c);
        Ok(())
    }

    pub fn entries(&self) -> &[Charge] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Totals across everything recorded: `(gross, master, iou_faelled)` in glyphs.
    pub fn totals(&self) -> (u128, u128, u128) {
        self.entries.iter().fold((0, 0, 0), |(g, m, o), e| {
            (g + e.gross_glyphs, m + e.master_glyphs, o + e.iou_faelled_glyphs)
        })
    }

    pub fn total_tokens(&self) -> u64 {
        self.entries.iter().map(|e| e.usage.total_tokens()).sum()
    }

    /// Turn everything accrued into a proposal. Nothing is signed, nothing is sent, and
    /// the ledger is NOT cleared — a proposal that was never settled must not silently
    /// erase the record it was made from.
    pub fn propose(&self, payees: &Payees, min_glyphs: u128) -> Result<SettlementProposal, String> {
        payees.validate()?;
        let (gross, master, iou_faelled) = self.totals();
        let payable = gross >= min_glyphs && gross > 0 && payees.master.is_some();
        let reason = if gross == 0 {
            "nothing metered yet".to_string()
        } else if gross < min_glyphs {
            format!(
                "accrued {} SIGIL is below the {} SIGIL batch minimum — settling now would \
                 likely cost more in fees than it moves",
                format_glyphs(gross),
                format_glyphs(min_glyphs)
            )
        } else if payees.master.is_none() {
            "no master dev bank fee account is configured, so there is nowhere to settle to"
                .to_string()
        } else {
            "ready for a human to review and sign".to_string()
        };
        // An IOU FÆLLED share with no admin wallet to receive it would be money addressed
        // to nobody. Say so rather than quietly folding it into master at settlement time.
        let orphaned_iou = iou_faelled > 0 && payees.admin_wallet.is_none();
        Ok(SettlementProposal {
            generations: self.entries.len(),
            tokens: self.total_tokens(),
            gross_glyphs: gross,
            master_glyphs: master,
            iou_faelled_glyphs: iou_faelled,
            orphaned_iou,
            payees: payees.clone(),
            payable: payable && !orphaned_iou,
            reason: if orphaned_iou {
                "charges were metered with an admin wallet that is no longer configured — \
                 fix the admin wallet before settling, or that share has no payee"
                    .to_string()
            } else {
                reason
            },
            auto_paid: false,
            note: "PROPOSAL ONLY — this module never signs and never sends. Token counts are \
                   self-reported by the machine that ran the model and are not verified by any \
                   consensus."
                .into(),
        })
    }
}

/// What a human is asked to approve. Deliberately inert: there is no `send()` on it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettlementProposal {
    pub generations: usize,
    pub tokens: u64,
    pub gross_glyphs: u128,
    /// To the master dev bank fee account.
    pub master_glyphs: u128,
    /// To the node operator, for good behaviour.
    pub iou_faelled_glyphs: u128,
    /// An IOU FÆLLED share was metered but no admin wallet is configured to receive it.
    pub orphaned_iou: bool,
    pub payees: Payees,
    /// Whether this is worth signing at all. `false` is the normal state today.
    pub payable: bool,
    /// Why `payable` is what it is, in a sentence a person can act on.
    pub reason: String,
    /// Always `false`, and there is no code path that sets it true.
    pub auto_paid: bool,
    pub note: String,
}

impl SettlementProposal {
    /// A few lines fit for a terminal or a receipt pane.
    pub fn human(&self) -> String {
        let mut s = format!(
            "flux-moe meter — {} generation(s), {} tokens\n  gross            {} SIGIL\n  IOU FÆLLED       {} SIGIL ({} bps, node operator)\n  master dev bank  {} SIGIL\n",
            self.generations,
            self.tokens,
            format_glyphs(self.gross_glyphs),
            format_glyphs(self.iou_faelled_glyphs),
            IOU_FAELLED_BPS,
            format_glyphs(self.master_glyphs),
        );
        s.push_str(&format!(
            "  status     {}\n  {}\n",
            if self.payable { "ready to sign" } else { "NOT payable" },
            self.reason
        ));
        s
    }
}

/// Generate through the user's own ollama AND meter it in one call.
///
/// This is the wiring, not a helper: [`crate::generate`] throws the token counts away
/// (it deserializes only `response`), so a caller who wants a priced answer had no way to
/// get one without re-running the model. Here the full reply is read, the counts are taken
/// from the same JSON that carried the text — so the meter can never disagree with the
/// answer it charged for — and the charge is appended to `ledger` before returning.
///
/// The model runs on the caller's own GPU/CPU. Nothing is sent anywhere, and no payment
/// happens: settle by calling [`Ledger::propose`] and handing the result to a human.
pub fn generate_metered(
    endpoint: &str,
    model: &str,
    prompt: &str,
    ledger: &mut Ledger,
    rates: Rates,
    split: Split,
    payees: &Payees,
    timeout_s: u64,
) -> Result<(String, Charge), String> {
    split.validate()?;
    payees.validate()?;
    let body = serde_json::json!({ "model": model, "prompt": prompt, "stream": false });
    let v: Value = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_s))
        .build()
        .map_err(|e| e.to_string())?
        .post(format!("{endpoint}/api/generate"))
        .json(&body)
        .send()
        .map_err(|e| format!("connect {endpoint}: {e}"))?
        .error_for_status()
        .map_err(|e| format!("http: {e}"))?
        .json()
        .map_err(|e| format!("decode: {e}"))?;
    let text = v
        .get("response")
        .and_then(Value::as_str)
        .ok_or_else(|| "reply carried no `response` field".to_string())?
        .to_string();
    // No counts ⇒ no charge. Returning the text with a zero charge would be a lie about
    // what it cost; the caller gets the answer and an explicit absence.
    let usage = Usage::from_ollama(model, &v)
        .ok_or_else(|| "the server returned no token counts, so this answer cannot be metered".to_string())?;
    let charge = Charge::new(usage, rates, split, payees.earns_iou_faelled())?;
    ledger.record(charge.clone())?;
    Ok((text, charge))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn usage(p: u64, c: u64) -> Usage {
        Usage { model: "qwen3.5:9b".into(), prompt_tokens: p, completion_tokens: c, ..Default::default() }
    }
    fn addr(c: char) -> String {
        std::iter::repeat(c).take(64).collect()
    }

    #[test]
    fn ollama_reply_without_counts_is_unknown_not_zero() {
        // A streaming chunk or an error body must not be priced as a free answer.
        assert!(Usage::from_ollama("qwen3.5:9b", &json!({"message": {"content": "hi"}})).is_none());
        let u = Usage::from_ollama(
            "qwen3.5:9b",
            &json!({"prompt_eval_count": 20, "eval_count": 180, "eval_duration": 2_000_000_000u64}),
        )
        .expect("counts present");
        assert_eq!(u.total_tokens(), 200);
        assert_eq!(u.tokens_per_second(), Some(90.0));
    }

    #[test]
    fn tokens_per_second_is_none_when_unmeasurable() {
        let mut u = usage(10, 0);
        u.eval_duration_ns = Some(0);
        assert_eq!(u.tokens_per_second(), None);
        u.eval_duration_ns = None;
        assert_eq!(u.tokens_per_second(), None);
    }

    #[test]
    fn an_admin_wallet_earns_one_tenth_of_a_percent_and_master_takes_the_rest() {
        let r = Rates::default();
        // 1000 prompt + 1000 completion = 100_000 + 400_000 glyphs
        let c = Charge::new(usage(1000, 1000), r, Split::default(), true).unwrap();
        assert_eq!(c.gross_glyphs, 500_000);
        assert_eq!(c.iou_faelled_glyphs, 500_000 * 10 / 10_000, "0.1 % to the operator");
        assert_eq!(c.master_glyphs, 500_000 - 500);
        assert_eq!(c.master_glyphs + c.iou_faelled_glyphs, c.gross_glyphs);
        assert!(c.operator_admin_wallet);
        assert!(!c.attested, "a self-reported count is never attested");
    }

    #[test]
    fn without_an_admin_wallet_the_operator_earns_nothing() {
        let c = Charge::new(usage(1000, 1000), Rates::default(), Split::default(), false).unwrap();
        assert_eq!(c.iou_faelled_glyphs, 0);
        assert_eq!(c.master_glyphs, c.gross_glyphs, "all of it goes to master");
        assert!(!c.operator_admin_wallet);
    }

    #[test]
    fn the_two_parts_always_re_sum_at_every_amount() {
        let s = Split::default();
        for gross in [0u128, 1, 2, 3, 7, 999, 1000, 9_999, 10_001, 123_456_789] {
            for admin in [true, false] {
                let (m, o) = s.apply(gross, admin);
                assert_eq!(m + o, gross, "parts must re-sum exactly at {gross} (admin={admin})");
                if !admin {
                    assert_eq!(o, 0);
                }
            }
        }
    }

    #[test]
    fn a_share_over_one_hundred_percent_is_refused() {
        assert!(Split { iou_faelled_bps: 10_001 }.validate().is_err());
        assert!(Split { iou_faelled_bps: 10_000 }.validate().is_ok());
        assert!(Charge::new(usage(1, 1), Rates::default(), Split { iou_faelled_bps: 99_999 }, true).is_err());
    }

    #[test]
    fn a_charge_too_small_to_split_gives_the_operator_zero_not_a_rounding_gift() {
        // 0.1 % of 500 glyphs is 0.05 — integer division floors it to nothing, and master
        // must then receive the whole amount rather than 499.
        let c = Charge::new(usage(1, 1), Rates::default(), Split::default(), true).unwrap();
        assert_eq!(c.gross_glyphs, 500);
        assert_eq!(c.iou_faelled_glyphs, 0);
        assert_eq!(c.master_glyphs, 500);
    }

    #[test]
    fn ledger_totals_and_proposal_is_never_payable_without_a_master_account() {
        let mut l = Ledger::in_memory();
        for _ in 0..4 {
            l.record(Charge::new(usage(500, 500), Rates::default(), Split::default(), true).unwrap()).unwrap();
        }
        let (gross, master, iou) = l.totals();
        assert_eq!(gross, 4 * 250_000);
        assert_eq!(master + iou, gross);
        assert_eq!(l.total_tokens(), 4000);

        // No master account → not payable, and the reason says why.
        let p = l.propose(&Payees { admin_wallet: Some(addr('b')), ..Default::default() }, 0).unwrap();
        assert!(!p.payable);
        assert!(p.reason.contains("master dev bank"), "{}", p.reason);
        assert!(!p.auto_paid);

        // Master set but under the batch minimum → still not payable.
        let payees = Payees { master: Some(addr('a')), admin_wallet: Some(addr('b')) };
        let p = l.propose(&payees, 10 * GLYPHS_PER_SIGIL).unwrap();
        assert!(!p.payable && p.reason.contains("below"));

        // Master + over the minimum → ready for a human, and STILL not auto-paid.
        let p = l.propose(&payees, 1).unwrap();
        assert!(p.payable && !p.auto_paid && !p.orphaned_iou);
        assert!(p.human().contains("IOU FÆLLED"));
        assert!(p.human().contains("master dev bank"));
    }

    #[test]
    fn an_iou_share_metered_without_an_admin_wallet_to_receive_it_blocks_settlement() {
        // Metered while an admin wallet was configured, then the operator removed it.
        let mut l = Ledger::in_memory();
        l.record(Charge::new(usage(100_000, 100_000), Rates::default(), Split::default(), true).unwrap()).unwrap();
        let p = l.propose(&Payees { master: Some(addr('a')), admin_wallet: None }, 1).unwrap();
        assert!(p.iou_faelled_glyphs > 0);
        assert!(p.orphaned_iou && !p.payable, "money addressed to nobody must not settle");
        assert!(p.reason.contains("admin wallet"));
    }

    #[test]
    fn a_bad_address_is_refused_before_anything_is_proposed() {
        let l = Ledger::in_memory();
        assert!(l.propose(&Payees { master: Some("nope".into()), ..Default::default() }, 0).is_err());
        assert!(
            l.propose(&Payees { master: Some(addr('g')), ..Default::default() }, 0).is_err(),
            "non-hex must be refused"
        );
        assert!(l.propose(&Payees { master: Some(addr('a')), admin_wallet: Some("short".into()) }, 0).is_err());
    }

    #[test]
    fn ledger_survives_a_truncated_last_line() {
        let dir = std::env::temp_dir().join(format!("flux-moe-meter-{}", now_ms()));
        let path = dir.join("meter.jsonl");
        let mut l = Ledger::open(&path);
        l.record(Charge::new(usage(100, 100), Rates::default(), Split::default(), true).unwrap()).unwrap();
        // Simulate a crash mid-write.
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        write!(f, "{{\"usage\":{{\"model\":\"trunc").unwrap();
        drop(f);
        let reopened = Ledger::open(&path);
        assert_eq!(reopened.entries().len(), 1, "the good record survives the torn one");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn glyph_formatting_is_exact() {
        assert_eq!(format_glyphs(0), "0.0");
        assert_eq!(format_glyphs(GLYPHS_PER_SIGIL), "1.0");
        assert_eq!(format_glyphs(GLYPHS_PER_SIGIL / 2), "0.5");
        assert_eq!(format_glyphs(1), "0.0000000001");
    }
}
