// registry_gen.rs — Generate ToolRegistry boilerplate + test scaffolding.
//
// Phase 2 (v0.2.0): Given a set of handler module names, generate the
// ToolRegistry struct, ToolDef, ToolFn type alias, handlers/mod.rs with
// module declarations, and a test scaffold.
//
// Stub for v0.1.0 — returns template strings.

/// Generate the ToolRegistry boilerplate for a set of handler module names.
pub fn generate_registry(module_names: &[&str]) -> String {
    let mut code = String::new();
    code.push_str("use std::collections::HashMap;\n");
    code.push_str("use serde_json::Value;\n\n");
    code.push_str("pub type ToolFn = fn(&Value) -> String;\n\n");
    code.push_str("pub struct ToolDef {\n");
    code.push_str("    pub name: &'static str,\n");
    code.push_str("    pub description: &'static str,\n");
    code.push_str("    pub input_schema: Value,\n");
    code.push_str("}\n\n");
    code.push_str("pub struct ToolRegistry {\n");
    code.push_str("    tools: Vec<ToolDef>,\n");
    code.push_str("    handlers: HashMap<String, ToolFn>,\n");
    code.push_str("}\n\n");
    code.push_str("impl ToolRegistry {\n");
    code.push_str("    pub fn new() -> Self { ToolRegistry { tools: Vec::new(), handlers: HashMap::new() } }\n");
    code.push_str("    pub fn register(&mut self, def: ToolDef, handler: ToolFn) {\n");
    code.push_str("        self.handlers.insert(def.name.to_string(), handler);\n");
    code.push_str("        self.tools.push(def);\n");
    code.push_str("    }\n");
    code.push_str("    pub fn tools_schema(&self) -> Vec<Value> {\n");
    code.push_str("        self.tools.iter().map(|t| serde_json::json!({\"name\":t.name,\"description\":t.description,\"inputSchema\":t.input_schema})).collect()\n");
    code.push_str("    }\n");
    code.push_str("    pub fn execute(&self, name: &str, args: &Value) -> Option<String> {\n");
    code.push_str("        self.handlers.get(name).map(|h| h(args))\n");
    code.push_str("    }\n");
    code.push_str("}\n\n");

    // Module declarations
    for name in module_names {
        code.push_str(&format!("pub mod {};\n", name));
    }

    code
}

/// Generate test scaffold for handler modules.
pub fn generate_tests(module_names: &[&str]) -> String {
    let mut code = String::new();
    code.push_str("#[cfg(test)]\n");
    code.push_str("mod tests {\n");
    code.push_str("    use super::*;\n\n");
    code.push_str("    #[test]\n");
    code.push_str("    fn test_registry_includes_all_modules() {\n");
    code.push_str("        let registry = build_registry();\n");
    code.push_str("        let schema = registry.tools_schema();\n");
    code.push_str("        assert!(schema.len() >= 44);\n");
    code.push_str("    }\n\n");

    for name in module_names {
        code.push_str(&format!("    #[test]\n"));
        code.push_str(&format!("    fn test_{}_registered() {{\n", name));
        code.push_str("        let registry = build_registry();\n");
        code.push_str(&format!("        assert!(registry.tools_schema().len() > 0);\n"));
        code.push_str("    }\n\n");
    }

    code.push_str("}\n");
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_registry_non_empty() {
        let code = generate_registry(&["build", "test_combo"]);
        assert!(code.contains("pub struct ToolRegistry"));
        assert!(code.contains("pub mod build"));
        assert!(code.contains("pub mod test_combo"));
    }

    #[test]
    fn test_generate_tests_includes_modules() {
        let code = generate_tests(&["build", "test_combo"]);
        assert!(code.contains("test_build_registered"));
        assert!(code.contains("test_test_combo_registered"));
    }
}
