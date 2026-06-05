//! flux-fcx — the **FCX → Slint transpiler**: the Vite/TS authoring → custom
//! Slint rendering bridge that lets Flux beat Electron.
//!
//! Electron ships a 150 MB Chromium + Node runtime to render UI. The Flux way:
//! author UI in **FCX** (a constrained TSX dialect — `function` component +
//! `useState` + a JSX tree + `onClick`), transpile it to **Slint** markup at
//! build time, and render native via flux-gui's Slint runtime — cross-compiled
//! by fluxc to a ~5–12 MB binary with no Chromium and no bundled JS runtime.
//!
//! This crate is the MVP of that bridge: **FCX-PARSE** ([`parse`]) +
//! **FCX-SLINTGEN** ([`slintgen`]). The follow-up lanes are FCX-LOGIC (an
//! embedded QuickJS for dynamic JS expressions), FCX-PACK (`fluxc build --app`
//! cross-building win/mac/linux binaries), and FCX-HMR (Vite HMR → Slint
//! hot-swap). Arbitrary npm-React is *deliberately out of scope* — that is the
//! unbounded compatibility trap; Flux wins on size/speed for apps authored the
//! FCX way, not by re-implementing the JS ecosystem.
//!
//! ```
//! let slint = flux_fcx::transpile_fcx(r#"
//!   export function Hi() {
//!     const [n, setN] = useState(0);
//!     return ( <button onClick={() => setN(n + 1)}>{n}</button> );
//!   }
//! "#).unwrap();
//! assert!(slint.contains("export component Hi inherits Window"));
//! ```

pub mod dev;
pub mod emit;
pub mod ir;
pub mod parse;
pub mod slintgen;

pub use emit::{emit_app, write_app, AppProject};
pub use ir::*;
pub use parse::{parse_component, parse_program};
pub use slintgen::{root_component, to_slint, to_slint_program};

/// Transpile FCX source text to `.slint` markup in one call. Handles a file
/// with one or many `export function` components.
pub fn transpile_fcx(src: &str) -> anyhow::Result<String> {
    let components = parse_program(src)?;
    Ok(to_slint_program(&components))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End-to-end golden: the bundled counter example transpiles to Slint that
    /// carries every construct the arc needs — state property, mapped widgets,
    /// interpolation, and an event handler that mutates state.
    #[test]
    fn counter_example_transpiles_end_to_end() {
        let fcx = include_str!("../examples/counter.fcx");
        let slint = transpile_fcx(fcx).expect("counter.fcx must transpile");

        for needle in [
            "export component Counter inherits Window {",
            "property <int> count: 0;",
            "import { Button, VerticalBox } from \"std-widgets.slint\";",
            r#"text: "Count: \{count}";"#,
            "clicked => { count = count + 1; }",
        ] {
            assert!(slint.contains(needle), "missing `{needle}` in:\n{slint}");
        }
    }

    #[test]
    fn bad_source_is_an_error_not_a_panic() {
        assert!(transpile_fcx("not fcx at all").is_err());
    }

    /// v0.4 golden: multi-component + lists + conditionals + array state.
    #[test]
    fn showcase_uses_lists_multicomponent_conditionals() {
        let fcx = include_str!("../examples/showcase.fcx");
        let slint = transpile_fcx(fcx).expect("showcase.fcx must transpile");
        for needle in [
            "component Banner {",                     // sub-component (referenced)
            "export component App inherits Window {",  // app root
            "property <[string]> items:",              // array state
            "for item in items : Text {",              // list rendering
            r#"text: "\{item}";"#,                     // loop interpolation
            "if count > 0 : Text {",                   // conditional
            "clicked => { count = count + 1; }",
        ] {
            assert!(slint.contains(needle), "missing `{needle}` in:\n{slint}");
        }
    }

    /// v0.3 golden: conditional rendering + Bool state + `!on` handler.
    #[test]
    fn toggle_example_transpiles_with_conditional() {
        let fcx = include_str!("../examples/toggle.fcx");
        let slint = transpile_fcx(fcx).expect("toggle.fcx must transpile");
        for needle in [
            "export component Toggle inherits Window {",
            "property <bool> on: false;",
            "clicked => { on = !on; }",
            "if on : Text {",
            r#"text: "Now visible";"#,
        ] {
            assert!(slint.contains(needle), "missing `{needle}` in:\n{slint}");
        }
    }
}
