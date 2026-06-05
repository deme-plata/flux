//! flux-uint — vendored fixed-width big integers (U256 / U512) + exact money (Amount).
//! Zero deps, `const fn` throughout, no_std-ready (uses only `core`). The foundation
//! for MandatPilot's credit-ledger + accumulator root. Built + chronos-tested under fluxc.
pub mod u256;
pub mod u512;
pub mod amount;

pub use amount::Amount;
pub use u256::U256;
pub use u512::{widening_mul, U512};

/// CONST-EVAL PROOF: this total is computed at COMPILE time — zero runtime cost.
/// 1_000_000_000_000 øre = 10 mia. øre = 100 mio. DKK.
pub const GENESIS_TOTAL: Amount = Amount::from_ore(1_000_000_000_000);
