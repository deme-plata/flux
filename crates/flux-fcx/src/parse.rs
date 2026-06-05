//! FCX parser — a small hand-written recursive-descent parser for the
//! constrained FCX dialect. It is **not** a general TS/JSX parser (that is the
//! unbounded trap we explicitly refuse); it accepts exactly:
//!
//! ```tsx
//! export function Counter() {
//!   const [count, setCount] = useState(0);
//!   return (
//!     <vbox>
//!       <text>Count: {count}</text>
//!       <button onClick={() => setCount(count + 1)}>Increment</button>
//!     </vbox>
//!   );
//! }
//! ```
//!
//! Supported: one function component, zero+ `useState` states, a JSX element
//! tree, text with `{interp}`, string attrs, and `onClick={() => setX(expr)}`.

use crate::ir::*;
use anyhow::{anyhow, bail, Result};

pub fn parse_component(src: &str) -> Result<Component> {
    let mut p = Parser::new(src);
    p.parse_component()
}

/// Parse a whole FCX file — one or more `export function` components.
pub fn parse_program(src: &str) -> Result<Vec<Component>> {
    let mut p = Parser::new(src);
    let mut comps = Vec::new();
    loop {
        p.ws();
        if p.peek().is_none() {
            break;
        }
        comps.push(p.parse_component()?);
    }
    if comps.is_empty() {
        bail!("no FCX components found");
    }
    Ok(comps)
}

/// Parse a single node from a fragment (used for conditional bodies).
fn parse_node_str(src: &str) -> Result<Node> {
    let mut p = Parser::new(src);
    p.parse_node()?
        .ok_or_else(|| anyhow!("conditional body has no element: {src:?}"))
}

struct Parser {
    s: Vec<char>,
    i: usize,
}

impl Parser {
    fn new(src: &str) -> Self {
        Self { s: src.chars().collect(), i: 0 }
    }

    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }
    fn peek2(&self) -> Option<char> {
        self.s.get(self.i + 1).copied()
    }
    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.i += 1;
        }
        c
    }
    /// Skip whitespace AND comments (`// line` and `/* block */`) — FCX is a
    /// TS dialect, so comments are expected anywhere a token boundary is.
    fn ws(&mut self) {
        loop {
            let before = self.i;
            while let Some(c) = self.peek() {
                if c.is_whitespace() {
                    self.i += 1;
                } else {
                    break;
                }
            }
            if self.peek() == Some('/') && self.peek2() == Some('/') {
                while let Some(c) = self.peek() {
                    self.i += 1;
                    if c == '\n' {
                        break;
                    }
                }
            } else if self.peek() == Some('/') && self.peek2() == Some('*') {
                self.i += 2;
                while let Some(c) = self.peek() {
                    if c == '*' && self.peek2() == Some('/') {
                        self.i += 2;
                        break;
                    }
                    self.i += 1;
                }
            }
            if self.i == before {
                break;
            }
        }
    }
    fn rest(&self) -> String {
        self.s[self.i..].iter().collect()
    }
    /// If the upcoming text equals `kw` (after the cursor), consume it.
    fn eat(&mut self, kw: &str) -> bool {
        let chars: Vec<char> = kw.chars().collect();
        if self.s[self.i..].starts_with(&chars[..]) {
            self.i += chars.len();
            true
        } else {
            false
        }
    }
    fn expect(&mut self, kw: &str) -> Result<()> {
        self.ws();
        if self.eat(kw) {
            Ok(())
        } else {
            bail!("expected `{}` near: {:?}", kw, self.rest().chars().take(40).collect::<String>())
        }
    }
    fn ident(&mut self) -> Result<String> {
        self.ws();
        let start = self.i;
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                self.i += 1;
            } else {
                break;
            }
        }
        if self.i == start {
            bail!("expected identifier near: {:?}", self.rest().chars().take(30).collect::<String>());
        }
        Ok(self.s[start..self.i].iter().collect())
    }

    fn parse_component(&mut self) -> Result<Component> {
        // `export`? `function` Name `(` `)` `{`
        self.ws();
        self.eat("export");
        self.ws();
        if !self.eat("function") {
            bail!("FCX must declare `function <Name>()`");
        }
        let name = self.ident()?;
        self.expect("(")?;
        self.expect(")")?;
        self.expect("{")?;

        let states = self.parse_states()?;

        self.ws();
        self.expect("return")?;
        self.expect("(")?;
        let root = self.parse_node()?.ok_or_else(|| anyhow!("return () has no root element"))?;

        // consume the trailing `) ; }` of `return (...);` + the fn body close,
        // so parse_program can find the next component.
        self.ws();
        self.eat(")");
        self.ws();
        self.eat(";");
        self.ws();
        self.eat("}");

        Ok(Component { name, states, root })
    }

    /// Parse zero or more `const [a, setA] = useState(x);` up to `return`.
    fn parse_states(&mut self) -> Result<Vec<State>> {
        let mut states = Vec::new();
        loop {
            self.ws();
            // stop when we hit the `return`
            if self.s[self.i..].starts_with(&['r', 'e', 't', 'u', 'r', 'n']) {
                break;
            }
            if !self.eat("const") {
                // Unknown statement — for the MVP we only allow const useState
                // lines before `return`. Anything else is a parse error rather
                // than silently skipped (fail loud).
                bail!("only `const [..]=useState(..)` statements are allowed before `return`; got: {:?}",
                      self.rest().chars().take(40).collect::<String>());
            }
            self.expect("[")?;
            let name = self.ident()?;
            self.expect(",")?;
            let setter = self.ident()?;
            self.expect("]")?;
            self.expect("=")?;
            self.expect("useState")?;
            self.expect("(")?;
            let init = self.take_balanced_until(')')?;
            self.expect(")")?;
            self.ws();
            self.eat(";");
            let ty = StateTy::infer(&init);
            states.push(State { name, setter, ty, init: init.trim().to_string() });
        }
        Ok(states)
    }

    /// Collect text until the matching close `delim`, honoring nested ()/{}.
    fn take_balanced_until(&mut self, delim: char) -> Result<String> {
        let start = self.i;
        let mut depth = 0i32;
        while let Some(c) = self.peek() {
            match c {
                // the matching delimiter at depth 0 ends the scan (checked BEFORE the decrement so a
                // nested closer that returns depth to 0 doesn't false-trigger an early break)
                ')' | '}' | ']' if depth == 0 && c == delim => break,
                '(' | '{' | '[' => depth += 1,
                ')' | '}' | ']' => depth -= 1,
                _ => {}
            }
            self.i += 1;
        }
        Ok(self.s[start..self.i].iter().collect())
    }

    /// Parse one node. Returns None if the next thing is a closing tag `</`.
    fn parse_node(&mut self) -> Result<Option<Node>> {
        self.ws();
        match self.peek() {
            Some('<') if self.peek2() == Some('/') => Ok(None),
            Some('<') => Ok(Some(Node::Element(self.parse_element()?))),
            Some('{') => Ok(Some(self.parse_brace_child()?)),
            Some(')') | None => Ok(None),
            _ => Ok(Some(Node::Text(self.parse_text()?))),
        }
    }

    /// A `{...}` appearing as an element child: either a conditional
    /// `{cond && <el/>}` or a bare interpolation `{expr}`.
    fn parse_brace_child(&mut self) -> Result<Node> {
        self.expect("{")?;
        let inner = self.take_balanced_until('}')?;
        self.expect("}")?;
        let t = inner.trim();
        if let Some(idx) = t.find(".map(") {
            // `list.map(item => <el/>)`  → For
            let list = t[..idx].trim().to_string();
            let after = t[idx + 5..].trim();
            let after = after.strip_suffix(')').unwrap_or(after);
            let (param, body_src) = after
                .split_once("=>")
                .ok_or_else(|| anyhow!("list .map needs an arrow fn: {t:?}"))?;
            let item = param.trim().trim_start_matches('(').trim_end_matches(')').trim().to_string();
            let body = parse_node_str(body_src.trim())?;
            Ok(Node::For { item, list, body: Box::new(body) })
        } else if let Some((cond, rhs)) = t.split_once("&&") {
            // allow `{cond && ( <el/> )}` — strip a single wrapping paren pair around the body
            let mut r = rhs.trim();
            if r.starts_with('(') && r.ends_with(')') { r = r[1..r.len() - 1].trim(); }
            let body = parse_node_str(r)?;
            Ok(Node::If { cond: cond.trim().to_string(), body: Box::new(body) })
        } else {
            Ok(Node::Text(Text { parts: vec![TextPart::Interp(t.to_string())] }))
        }
    }

    fn parse_element(&mut self) -> Result<Element> {
        self.expect("<")?;
        let tag = self.ident()?;
        let props = self.parse_props()?;
        self.ws();
        if self.eat("/>") {
            return Ok(Element { tag, props, children: vec![] });
        }
        self.expect(">")?;
        let mut children = Vec::new();
        while let Some(node) = self.parse_node()? {
            children.push(node);
        }
        // closing </tag>
        self.expect("</")?;
        let close = self.ident()?;
        if close != tag {
            bail!("mismatched closing tag: <{tag}> closed by </{close}>");
        }
        self.expect(">")?;
        Ok(Element { tag, props, children })
    }

    fn parse_props(&mut self) -> Result<Vec<Prop>> {
        let mut props = Vec::new();
        loop {
            self.ws();
            match self.peek() {
                Some('>') | Some('/') | None => break,
                _ => {}
            }
            let name = self.ident()?;
            self.expect("=")?;
            self.ws();
            let value = match self.peek() {
                Some('"') | Some('\'') => {
                    let q = self.bump().unwrap();
                    let start = self.i;
                    while let Some(c) = self.peek() {
                        if c == q {
                            break;
                        }
                        self.i += 1;
                    }
                    let lit: String = self.s[start..self.i].iter().collect();
                    self.bump(); // closing quote
                    PropValue::Literal(lit)
                }
                Some('{') => {
                    self.bump(); // {
                    let inner = self.take_balanced_until('}')?;
                    self.expect("}")?;
                    classify_brace(&inner)?
                }
                other => bail!("attribute `{name}` has unsupported value start {:?}", other),
            };
            props.push(Prop { name, value });
        }
        Ok(props)
    }

    /// Parse a text run up to the next `<`, splitting out `{interp}` segments.
    fn parse_text(&mut self) -> Result<Text> {
        let mut parts = Vec::new();
        let mut lit = String::new();
        while let Some(c) = self.peek() {
            match c {
                '<' => break,
                '{' => {
                    if !lit.trim().is_empty() {
                        parts.push(TextPart::Lit(lit.trim().to_string()));
                    }
                    lit.clear();
                    self.bump(); // {
                    let inner = self.take_balanced_until('}')?;
                    self.expect("}")?;
                    parts.push(TextPart::Interp(inner.trim().to_string()));
                }
                _ => {
                    lit.push(c);
                    self.bump();
                }
            }
        }
        if !lit.trim().is_empty() {
            parts.push(TextPart::Lit(lit.trim().to_string()));
        }
        Ok(Text { parts })
    }
}

/// Classify a `{...}` attribute body into a handler or a binding.
fn classify_brace(inner: &str) -> Result<PropValue> {
    let t = inner.trim();
    if let Some(rhs) = t.split_once("=>") {
        // `() => setCount(count + 1)`  → setter + expr
        let body = rhs.1.trim();
        let open = body.find('(').ok_or_else(|| anyhow!("handler body has no call: {body}"))?;
        let setter = body[..open].trim().to_string();
        let expr = body[open + 1..]
            .trim_end()
            .trim_end_matches(')')
            .trim()
            .to_string();
        if setter.is_empty() {
            bail!("handler has no setter: {body}");
        }
        Ok(PropValue::Handler(Handler { setter, expr }))
    } else {
        Ok(PropValue::Bind(t.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const COUNTER: &str = r#"
export function Counter() {
  const [count, setCount] = useState(0);
  return (
    <vbox>
      <text>Count: {count}</text>
      <button onClick={() => setCount(count + 1)}>Increment</button>
    </vbox>
  );
}
"#;

    #[test]
    fn parses_counter_name_and_state() {
        let c = parse_component(COUNTER).unwrap();
        assert_eq!(c.name, "Counter");
        assert_eq!(c.states.len(), 1);
        assert_eq!(c.states[0].name, "count");
        assert_eq!(c.states[0].setter, "setCount");
        assert_eq!(c.states[0].ty, StateTy::Int);
        assert_eq!(c.states[0].init, "0");
    }

    #[test]
    fn parses_tree_structure() {
        let c = parse_component(COUNTER).unwrap();
        let Node::Element(root) = &c.root else { panic!("root not element") };
        assert_eq!(root.tag, "vbox");
        assert_eq!(root.children.len(), 2);
        let Node::Element(btn) = &root.children[1] else { panic!("child 1 not element") };
        assert_eq!(btn.tag, "button");
    }

    #[test]
    fn parses_text_interpolation() {
        let c = parse_component(COUNTER).unwrap();
        let Node::Element(root) = &c.root else { unreachable!() };
        let Node::Element(text_el) = &root.children[0] else { panic!() };
        assert_eq!(text_el.tag, "text");
        let Node::Text(t) = &text_el.children[0] else { panic!("not text") };
        assert_eq!(t.parts, vec![TextPart::Lit("Count:".into()), TextPart::Interp("count".into())]);
    }

    #[test]
    fn parses_onclick_handler() {
        let c = parse_component(COUNTER).unwrap();
        let Node::Element(root) = &c.root else { unreachable!() };
        let Node::Element(btn) = &root.children[1] else { panic!() };
        let h = btn.props.iter().find(|p| p.name == "onClick").unwrap();
        match &h.value {
            PropValue::Handler(Handler { setter, expr }) => {
                assert_eq!(setter, "setCount");
                assert_eq!(expr, "count + 1");
            }
            other => panic!("onClick not a handler: {other:?}"),
        }
    }

    #[test]
    fn type_inference() {
        assert_eq!(StateTy::infer("0"), StateTy::Int);
        assert_eq!(StateTy::infer("3.14"), StateTy::Float);
        assert_eq!(StateTy::infer("true"), StateTy::Bool);
        assert_eq!(StateTy::infer("\"hi\""), StateTy::Str);
    }

    #[test]
    fn string_attr_literal() {
        let src = r#"export function B() { return ( <button label="Go"/> ); }"#;
        let c = parse_component(src).unwrap();
        let Node::Element(b) = &c.root else { unreachable!() };
        let p = &b.props[0];
        assert_eq!(p.name, "label");
        assert_eq!(p.value, PropValue::Literal("Go".into()));
    }

    #[test]
    fn rejects_non_function() {
        assert!(parse_component("const x = 1;").is_err());
    }

    const TOGGLE: &str = r#"
export function Toggle() {
  const [on, setOn] = useState(false);
  return (
    <vbox>
      <button onClick={() => setOn(!on)}>Toggle</button>
      {on && <text>Now visible</text>}
    </vbox>
  );
}
"#;

    #[test]
    fn parses_conditional_child() {
        let c = parse_component(TOGGLE).unwrap();
        assert_eq!(c.states[0].ty, StateTy::Bool);
        let Node::Element(root) = &c.root else { unreachable!() };
        assert_eq!(root.children.len(), 2);
        match &root.children[1] {
            Node::If { cond, body } => {
                assert_eq!(cond, "on");
                let Node::Element(e) = body.as_ref() else { panic!("if body not element") };
                assert_eq!(e.tag, "text");
            }
            other => panic!("second child not a conditional: {other:?}"),
        }
    }

    #[test]
    fn parses_negation_handler() {
        let c = parse_component(TOGGLE).unwrap();
        let Node::Element(root) = &c.root else { unreachable!() };
        let Node::Element(btn) = &root.children[0] else { panic!() };
        let h = btn.props.iter().find(|p| p.name == "onClick").unwrap();
        let PropValue::Handler(Handler { setter, expr }) = &h.value else { panic!() };
        assert_eq!(setter, "setOn");
        assert_eq!(expr, "!on");
    }

    #[test]
    fn parses_list_map() {
        let src = r#"export function L() {
          const [xs, setXs] = useState(["a","b"]);
          return ( <vbox>{xs.map(x => <text>{x}</text>)}</vbox> );
        }"#;
        let c = parse_component(src).unwrap();
        assert_eq!(c.states[0].ty, StateTy::StrList);
        let Node::Element(root) = &c.root else { unreachable!() };
        match &root.children[0] {
            Node::For { item, list, body } => {
                assert_eq!(item, "x");
                assert_eq!(list, "xs");
                let Node::Element(e) = body.as_ref() else { panic!() };
                assert_eq!(e.tag, "text");
            }
            other => panic!("not a list: {other:?}"),
        }
    }

    #[test]
    fn parses_multiple_components() {
        let src = r#"
          export function A() { return ( <text>a</text> ); }
          export function B() { return ( <vbox><A/></vbox> ); }
        "#;
        let comps = parse_program(src).unwrap();
        assert_eq!(comps.len(), 2);
        assert_eq!(comps[0].name, "A");
        assert_eq!(comps[1].name, "B");
        // B references A as a component instance
        let Node::Element(root) = &comps[1].root else { unreachable!() };
        let Node::Element(inst) = &root.children[0] else { panic!() };
        assert_eq!(inst.tag, "A");
    }
}
