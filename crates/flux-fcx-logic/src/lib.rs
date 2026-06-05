//! FCX-LOGIC — run **arbitrary JavaScript** FCX handlers via embedded QuickJS.
//!
//! v0.1–v0.5 of FCX only supported `setX(expr)` handlers (transpiled to a
//! single Slint assignment). Real apps need real logic — conditionals, loops,
//! `Math`, string work, multiple statements. This crate embeds **QuickJS**
//! (via `rquickjs`) so an FCX handler body can be *any* JavaScript:
//!
//! ```
//! # use std::collections::BTreeMap;
//! # use flux_fcx_logic::{eval_handler, JsVal};
//! let mut state = BTreeMap::from([("count".to_string(), JsVal::Num(0.0))]);
//! eval_handler("for (let i = 0; i < 5; i++) count += i;", &mut state).unwrap();
//! assert_eq!(state["count"], JsVal::Num(10.0)); // real JS ran
//! ```
//!
//! The engine loads each state variable as a JS global, runs the handler in a
//! fresh QuickJS context, then reads the (possibly mutated) globals back into
//! `state`. The FCX transpiler rewrites `setX(e)` → `x = e` before handing the
//! body here, so handlers compose naturally with component state.
//!
//! **What this crate is:** the headlessly-testable logic engine. **The
//! integration step** (not here): the generated Slint app's `clicked => {}`
//! callback calls into Rust → `eval_handler` → applies the returned state to
//! the Slint properties. That binding needs a running window to verify.

use anyhow::{anyhow, Result};
use std::collections::BTreeMap;

/// A value carried between FCX state and the JS engine.
#[derive(Debug, Clone, PartialEq)]
pub enum JsVal {
    Num(f64),
    Bool(bool),
    Str(String),
}

/// Run a JavaScript handler body against `state`. Each entry is exposed to the
/// script as a global of its type; after the script runs, the globals are read
/// back into `state` (preserving each entry's original type). The script may be
/// any JavaScript QuickJS accepts — full expressions, statements, control flow.
pub fn eval_handler(js: &str, state: &mut BTreeMap<String, JsVal>) -> Result<()> {
    use rquickjs::{Context, Runtime};

    let rt = Runtime::new().map_err(|e| anyhow!("quickjs runtime: {e}"))?;
    let ctx = Context::full(&rt).map_err(|e| anyhow!("quickjs context: {e}"))?;

    ctx.with(|ctx| -> Result<()> {
        let g = ctx.globals();
        // inject FCX state as JS globals
        for (k, v) in state.iter() {
            match v {
                JsVal::Num(n) => g.set(k.as_str(), *n),
                JsVal::Bool(b) => g.set(k.as_str(), *b),
                JsVal::Str(s) => g.set(k.as_str(), s.as_str()),
            }
            .map_err(|e| anyhow!("set global `{k}`: {e}"))?;
        }
        // run the arbitrary JS handler (value, if any, is ignored)
        ctx.eval::<rquickjs::Value, _>(js)
            .map_err(|e| anyhow!("javascript error: {e}"))?;
        // read mutations back, preserving each entry's declared type
        for (k, v) in state.iter_mut() {
            *v = match v {
                JsVal::Num(_) => {
                    JsVal::Num(g.get::<_, f64>(k.as_str()).map_err(|e| anyhow!("read `{k}`: {e}"))?)
                }
                JsVal::Bool(_) => {
                    JsVal::Bool(g.get::<_, bool>(k.as_str()).map_err(|e| anyhow!("read `{k}`: {e}"))?)
                }
                JsVal::Str(_) => JsVal::Str(
                    g.get::<_, String>(k.as_str()).map_err(|e| anyhow!("read `{k}`: {e}"))?,
                ),
            };
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(pairs: &[(&str, JsVal)]) -> BTreeMap<String, JsVal> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    #[test]
    fn arithmetic_assignment() {
        let mut s = state(&[("count", JsVal::Num(0.0))]);
        eval_handler("count = count + 5;", &mut s).unwrap();
        assert_eq!(s["count"], JsVal::Num(5.0));
    }

    #[test]
    fn loops_and_conditionals_run() {
        // sum of even numbers 0..10 = 0+2+4+6+8 = 20 — not expressible as setX(expr)
        let mut s = state(&[("n", JsVal::Num(0.0))]);
        eval_handler("for (let i = 0; i < 10; i++) { if (i % 2 == 0) n += i; }", &mut s).unwrap();
        assert_eq!(s["n"], JsVal::Num(20.0));
    }

    #[test]
    fn boolean_toggle() {
        let mut s = state(&[("on", JsVal::Bool(false))]);
        eval_handler("on = !on;", &mut s).unwrap();
        assert_eq!(s["on"], JsVal::Bool(true));
    }

    #[test]
    fn string_ops() {
        let mut s = state(&[("msg", JsVal::Str("a".into()))]);
        eval_handler("msg = msg + 'bc'.toUpperCase();", &mut s).unwrap();
        assert_eq!(s["msg"], JsVal::Str("aBC".into()));
    }

    #[test]
    fn real_js_builtins_available() {
        // Math proves it's a real JS engine, not a toy expression evaluator
        let mut s = state(&[("x", JsVal::Num(0.0))]);
        eval_handler("x = Math.max(3, 7) + Math.floor(2.9);", &mut s).unwrap();
        assert_eq!(s["x"], JsVal::Num(9.0));
    }

    #[test]
    fn multi_statement_handler() {
        let mut s = state(&[("count", JsVal::Num(4.0)), ("on", JsVal::Bool(false))]);
        eval_handler("count = count + 1; on = count > 3;", &mut s).unwrap();
        assert_eq!(s["count"], JsVal::Num(5.0));
        assert_eq!(s["on"], JsVal::Bool(true));
    }

    #[test]
    fn syntax_error_is_reported_not_panicked() {
        let mut s = state(&[("x", JsVal::Num(0.0))]);
        assert!(eval_handler("this is ((( not js", &mut s).is_err());
    }
}
