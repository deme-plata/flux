//! UI-IR — the platform-neutral intermediate between FCX (the TS authoring
//! dialect) and a render target. Today the only target is Slint
//! ([`crate::slintgen`]); the IR is kept render-agnostic so a future target
//! (web, UE, TUI) can consume the same tree.
//!
//! The IR is deliberately small: it models exactly the FCX subset the MVP
//! supports — a single function component with `useState` reactive state, a
//! JSX-ish element tree, text with `{interpolation}`, and `onClick` handlers
//! of the form `() => setX(expr)`.

use serde::{Deserialize, Serialize};

/// A single UI component (one FCX function component).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Component {
    pub name: String,
    pub states: Vec<State>,
    pub root: Node,
}

impl Component {
    /// Resolve the state name a `setX` setter mutates, e.g. `setCount` → `count`.
    pub fn state_for_setter(&self, setter: &str) -> Option<&State> {
        self.states.iter().find(|s| s.setter == setter)
    }
}

/// A `const [name, setter] = useState(init)` declaration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub name: String,
    pub setter: String,
    pub ty: StateTy,
    /// The initial value as source text (`"0"`, `"true"`, `"\"hi\""`).
    pub init: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StateTy {
    Int,
    Float,
    Bool,
    Str,
    /// `useState(["a","b"])` → Slint `[string]`.
    StrList,
    /// `useState([1,2,3])` → Slint `[int]`.
    IntList,
}

impl StateTy {
    /// Infer the state type from the `useState(..)` initial-value text.
    pub fn infer(init: &str) -> StateTy {
        let t = init.trim();
        if t.starts_with('[') {
            // array literal — infer element type from the first element
            let first = t[1..].trim_start();
            if first.starts_with('"') || first.starts_with('\'') || first.starts_with('`') {
                StateTy::StrList
            } else {
                StateTy::IntList
            }
        } else if t == "true" || t == "false" {
            StateTy::Bool
        } else if t.starts_with('"') || t.starts_with('\'') || t.starts_with('`') {
            StateTy::Str
        } else if t.contains('.') && t.trim_matches(|c: char| c.is_ascii_digit() || c == '.' || c == '-').is_empty() {
            StateTy::Float
        } else {
            StateTy::Int
        }
    }

    /// The Slint type keyword for this state.
    pub fn slint_ty(&self) -> &'static str {
        match self {
            StateTy::Int => "int",
            StateTy::Float => "float",
            StateTy::Bool => "bool",
            StateTy::Str => "string",
            StateTy::StrList => "[string]",
            StateTy::IntList => "[int]",
        }
    }
}

/// A node in the element tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Node {
    Element(Element),
    /// Text content, possibly with `{interpolations}`.
    Text(Text),
    /// `{cond && <el/>}` conditional rendering → Slint `if cond : El`.
    If { cond: String, body: Box<Node> },
    /// `{list.map(item => <el/>)}` list rendering → Slint `for item in list : El`.
    For { item: String, list: String, body: Box<Node> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Element {
    /// FCX tag — `vbox`, `hbox`, `text`, `button`, …
    pub tag: String,
    pub props: Vec<Prop>,
    pub children: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Prop {
    pub name: String,
    pub value: PropValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PropValue {
    /// A double/single-quoted string literal (quotes stripped).
    Literal(String),
    /// `{ () => setX(expr) }` — an event handler.
    Handler(Handler),
    /// `{ expr }` — a one-way binding expression.
    Bind(String),
}

/// An `onClick={() => setCount(count + 1)}` handler.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Handler {
    pub setter: String,
    /// The new-value expression, e.g. `count + 1`.
    pub expr: String,
}

/// Text content as alternating literal / interpolated parts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Text {
    pub parts: Vec<TextPart>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TextPart {
    Lit(String),
    /// The expression inside `{ }`.
    Interp(String),
}
