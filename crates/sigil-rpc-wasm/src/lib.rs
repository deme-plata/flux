//! sigil-rpc-wasm — the SIGIL money keystone running as WASM in the browser tab.
//!
//! A single in-tab `SigilState` (thread-local; wasm is single-threaded) holding
//! a bootstrapped USDS/wQUG pool + a funded "you" wallet. `swap` and `mine`
//! execute the REAL keystone fns (`sigil_rpc::execute_swap` / `submit_share`),
//! every write through `commit_state_transition` — so money moves are settled,
//! committed in the 4 roots, and 21M-cap-guarded, all CLIENT-SIDE with no server.
//!
//! C-ABI, raw (no wasm-bindgen). Ops return a packed `(ptr<<32)|len` to a JSON
//! verdict string (same convention as flux-fcx-wasm / sigil-tip-wasm).

use std::cell::RefCell;

use sigil_dex::SwapDirection;
use sigil_rpc::{execute_swap, submit_share};
use sigil_state::{
    commit_state_transition, PoolState, SigilState, StateMutation, StateTransition, TokenId,
    WalletId, NATIVE,
};

// SIGIL uses 8 decimals — bootstrap amounts are in base units so the wallet UI
// (which sends `amount * 1e8`) lines up with real pool depth. Shallow raw
// reserves (100k) tripped the DEX reserve floor on any real swap.
const DEC: u128 = 100_000_000;

const USDS: TokenId = [0xAA; 32];
const WQUG: TokenId = [0xBB; 32];
const POOL: WalletId = [0xCC; 32];
const ME: WalletId = [0x11; 32];
const MASTER: WalletId = [0xFF; 32];

struct Node {
    state: SigilState,
    height: u64,
}

fn bootstrap() -> Node {
    let mut state = SigilState::new();
    let t = StateTransition {
        at_height: 0,
        mutations: vec![
            StateMutation::SetMasterWallet { wallet: MASTER },
            StateMutation::SetBalance { wallet: ME, token: USDS, amount: 1_000_000 * DEC },
            StateMutation::SetPool {
                pool: POOL,
                state: PoolState {
                    token_a: USDS, token_b: WQUG,
                    reserve_a: 100_000 * DEC, reserve_b: 100_000 * DEC,
                    lp_shares: 100_000 * DEC, fee_bps: 30, accrued_fees: 0,
                },
            },
        ],
    };
    commit_state_transition(&mut state, &t, 0).expect("genesis");
    Node { state, height: 1 }
}

thread_local! {
    static NODE: RefCell<Node> = RefCell::new(bootstrap());
}

fn pack(s: String) -> u64 {
    let b = s.into_bytes();
    let l = b.len() as u64;
    let p = b.as_ptr() as u64;
    std::mem::forget(b);
    (p << 32) | l
}

fn err(e: String) -> String {
    format!("{{\"ok\":false,\"error\":\"{}\"}}", e.replace('"', "'"))
}

/// Swap `amount_in` USDS → wQUG through the in-tab pool. Settled + root-committed.
#[no_mangle]
pub extern "C" fn rpc_swap(amount_in: u64) -> u64 {
    NODE.with(|n| {
        let mut n = n.borrow_mut();
        let h = n.height;
        match execute_swap(&mut n.state, h, ME, POOL, SwapDirection::AtoB, amount_in as u128, 1) {
            Ok(o) => {
                n.height += 1;
                pack(format!(
                    "{{\"ok\":true,\"amount_out\":{},\"protocol_fee\":{}}}",
                    o.amount_out, o.protocol_fee
                ))
            }
            Err(e) => pack(err(e.to_string())),
        }
    })
}

/// Submit a mining share → coinbase `reward` SIGIL credited (cap-enforced).
#[no_mangle]
pub extern "C" fn rpc_mine(nonce: u64, reward: u64) -> u64 {
    NODE.with(|n| {
        let mut n = n.borrow_mut();
        let h = n.height;
        match submit_share(&mut n.state, h, ME, b"sigil-os-block", nonce, 0, reward as u128) {
            Ok(bal) => {
                n.height += 1;
                pack(format!("{{\"ok\":true,\"sigil\":{}}}", bal))
            }
            Err(e) => pack(err(e.to_string())),
        }
    })
}

/// Your three balances (base units, 8 decimals).
#[no_mangle]
pub extern "C" fn rpc_balances() -> u64 {
    NODE.with(|n| {
        let n = n.borrow();
        pack(format!(
            "{{\"sigil\":{},\"usds\":{},\"wqug\":{}}}",
            n.state.balance_of(&ME, &NATIVE),
            n.state.balance_of(&ME, &USDS),
            n.state.balance_of(&ME, &WQUG)
        ))
    })
}
