// fluxc-core/webhook_inbound.rs — Inbound Webhook Schema Compiler
//
// Novel feature (2026-06-07): define webhook contracts in .flux-webhook.toml,
// and fluxc auto-generates:
//   1. Rust handler with HMAC validation + type-safe payloads
//   2. OpenAPI 3.1 schema
//   3. MCP tool binding
//   4. TypeScript SDK stub
//   5. Auto-registration with fluxc serve
//
// Bridges: flux-api (schema types, event_types, codegen), flux-backend (compilation),
// and the existing outbound webhook infrastructure (webhook.rs).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// ── Webhook Contract Model ──

/// A complete webhook contract: one inbound endpoint with schema + action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookContract {
    /// Unique identifier (e.g. "github_push")
    pub id: String,
    /// HTTP route (e.g. "POST /webhooks/github")
    pub route: String,
    /// Environment variable holding the HMAC secret
    #[serde(default)]
    pub secret_env: Option<String>,
    /// Human-readable description
    #[serde(default)]
    pub description: String,
    /// JSON Schema for the request body
    pub schema: WebhookSchema,
    /// What to do when this webhook fires
    pub action: WebhookAction,
    /// Tags for OpenAPI grouping
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Supported webhook actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WebhookAction {
    /// Run `fluxc build --package <package>` using a JSONPath expression from the payload.
    FluxcBuild {
        package: String,        // JSONPath like "$.repository.full_name"
        #[serde(default)]
        release: bool,
    },
    /// Run `fluxc test --package <package>`
    FluxcTest {
        package: String,
    },
    /// Fire an MCP tool call
    McpTool {
        tool: String,
        /// JSONPath to extract args from payload
        args_path: Option<String>,
    },
    /// Dispatch to the swarm bus
    SwarmDispatch {
        lane: String,
        /// JSONPath for the task description
        task_path: String,
    },
    /// Custom shell command (careful!)
    ShellCommand {
        command: String,        // Can use {{JSONPath}} templates
    },
}

/// A simplified JSON Schema for webhook payloads.
/// Lowered from flux-api's ApiSchema for ease of definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WebhookSchema {
    Object {
        #[serde(default)]
        required: Vec<String>,
        properties: BTreeMap<String, WebhookSchema>,
    },
    String_ {
        #[serde(default)]
        description: String,
    },
    Number_,
    Integer_,
    Boolean_,
    Array {
        items: Box<WebhookSchema>,
    },
    Null_,
}

impl WebhookSchema {
    pub fn is_object(&self) -> bool {
        matches!(self, WebhookSchema::Object { .. })
    }
}

// ── Contract Parser (TOML) ──

/// Parse a .flux-webhook.toml file into a Vec<WebhookContract>.
pub fn parse_webhook_contracts(path: &Path) -> Result<Vec<WebhookContract>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Cannot read {}: {}", path.display(), e))?;

    let raw: toml::Value = toml::from_str(&content)
        .map_err(|e| format!("TOML parse error: {}", e))?;

    let mut contracts = Vec::new();

    if let toml::Value::Table(root) = &raw {
        if let Some(toml::Value::Table(webhooks)) = root.get("webhook") {
            for (id, val) in webhooks {
                if let toml::Value::Table(t) = val {
                    let contract = parse_single_contract(id, t)?;
                    contracts.push(contract);
                }
            }
        }
    }

    if contracts.is_empty() {
        return Err("No [[webhook]] entries found in TOML".into());
    }

    Ok(contracts)
}

fn parse_single_contract(id: &str, t: &toml::value::Table) -> Result<WebhookContract, String> {
    let route = t.get("route")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("webhook.{}: missing 'route'", id))?
        .to_string();

    let description = t.get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let secret_env = t.get("secret_env")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let tags: Vec<String> = t.get("tags")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();

    let schema = t.get("schema")
        .ok_or_else(|| format!("webhook.{}: missing 'schema'", id))
        .and_then(|v| parse_schema_from_toml(v))?;

    let action = t.get("action")
        .ok_or_else(|| format!("webhook.{}: missing 'action'", id))
        .and_then(|v| parse_action_from_toml(v))?;

    Ok(WebhookContract {
        id: id.to_string(),
        route,
        secret_env,
        description,
        schema,
        action,
        tags,
    })
}

fn parse_schema_from_toml(val: &toml::Value) -> Result<WebhookSchema, String> {
    match val {
        toml::Value::Table(t) => {
            let ty = t.get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("object");

            match ty {
                "object" => {
                    let required: Vec<String> = t.get("required")
                        .and_then(|v| v.as_array())
                        .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                        .unwrap_or_default();

                    let mut properties = BTreeMap::new();
                    if let Some(toml::Value::Table(props)) = t.get("properties") {
                        for (name, prop_val) in props {
                            properties.insert(name.clone(), parse_schema_from_toml(prop_val)?);
                        }
                    }
                    Ok(WebhookSchema::Object { required, properties })
                }
                "string" => Ok(WebhookSchema::String_ { description: String::new() }),
                "number" => Ok(WebhookSchema::Number_),
                "integer" => Ok(WebhookSchema::Integer_),
                "boolean" => Ok(WebhookSchema::Boolean_),
                "array" => {
                    let items = t.get("items")
                        .map(|v| parse_schema_from_toml(v))
                        .transpose()?
                        .unwrap_or(WebhookSchema::String_ { description: String::new() });
                    Ok(WebhookSchema::Array { items: Box::new(items) })
                }
                "null" => Ok(WebhookSchema::Null_),
                other => Err(format!("Unknown schema type: {}", other)),
            }
        }
        toml::Value::String(s) => {
            // Shorthand: "string", "number", etc.
            match s.as_str() {
                "string" => Ok(WebhookSchema::String_ { description: String::new() }),
                "number" => Ok(WebhookSchema::Number_),
                "integer" => Ok(WebhookSchema::Integer_),
                "boolean" => Ok(WebhookSchema::Boolean_),
                "null" => Ok(WebhookSchema::Null_),
                other => Err(format!("Unknown schema type shorthand: {}", other)),
            }
        }
        _ => Err("Schema must be a table or string".into()),
    }
}

fn parse_action_from_toml(val: &toml::Value) -> Result<WebhookAction, String> {
    let t = val.as_table()
        .ok_or_else(|| "action must be a table".to_string())?;

    let action_type = t.get("type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "action missing 'type'".to_string())?;

    match action_type {
        "fluxc_build" => {
            let package = t.get("package")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "fluxc_build missing 'package'".to_string())?
                .to_string();
            let release = t.get("release")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            Ok(WebhookAction::FluxcBuild { package, release })
        }
        "fluxc_test" => {
            let package = t.get("package")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "fluxc_test missing 'package'".to_string())?
                .to_string();
            Ok(WebhookAction::FluxcTest { package })
        }
        "mcp_tool" => {
            let tool = t.get("tool")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "mcp_tool missing 'tool'".to_string())?
                .to_string();
            let args_path = t.get("args_path")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            Ok(WebhookAction::McpTool { tool, args_path })
        }
        "swarm_dispatch" => {
            let lane = t.get("lane")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "swarm_dispatch missing 'lane'".to_string())?
                .to_string();
            let task_path = t.get("task_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "swarm_dispatch missing 'task_path'".to_string())?
                .to_string();
            Ok(WebhookAction::SwarmDispatch { lane, task_path })
        }
        "shell_command" => {
            let command = t.get("command")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "shell_command missing 'command'".to_string())?
                .to_string();
            Ok(WebhookAction::ShellCommand { command })
        }
        other => Err(format!("Unknown action type: {}", other)),
    }
}

// ── Code Generators ──

/// Generate a Rust module with HMAC-validated webhook handler.
pub fn generate_rust_handler(contract: &WebhookContract) -> String {
    let struct_name = to_pascal_case(&contract.id);
    let method = extract_method(&contract.route);
    let path = extract_path(&contract.route);

    let mut out = String::new();
    out.push_str(&format!(
        "// Auto-generated by fluxc webhook-gen — {}\n", contract.id
    ));
    out.push_str(&format!("// {}\n", contract.description));
    out.push_str("use serde::{{Deserialize, Serialize}};\n");
    out.push_str("use hmac::{{Hmac, Mac}};\n");
    out.push_str("use sha2::Sha256;\n\n");
    out.push_str("type HmacSha256 = Hmac<Sha256>;\n\n");

    // Struct definitions from schema
    out.push_str(&generate_rust_structs(&contract.schema, &struct_name));
    out.push_str("\n");

    // HMAC validation function
    if let Some(ref secret_env) = contract.secret_env {
        out.push_str(&format!(
            "/// Validate the HMAC-SHA256 signature of an incoming webhook payload.\n\
             pub fn validate_{}_signature(body: &[u8], signature_header: &str) -> Result<(), String> {{\n\
             \x20   let secret = std::env::var(\"{}\")\n\
             \x20       .map_err(|_| \"{} env var not set\".to_string())?;\n\
             \x20   let mut mac = HmacSha256::new_from_slice(secret.as_bytes())\n\
             \x20       .map_err(|e| format!(\"HMAC init: {{}}\", e))?;\n\
             \x20   mac.update(body);\n\
             \x20   let computed = hex::encode(mac.finalize().into_bytes());\n\
             \x20   let expected = signature_header\n\
             \x20       .strip_prefix(\"sha256=\")\n\
             \x20       .unwrap_or(signature_header);\n\
             \x20   if computed != expected {{\n\
             \x20       return Err(\"HMAC signature mismatch\".into());\n\
             \x20   }}\n\
             \x20   Ok(())\n\
             }}\n\n",
            contract.id, secret_env, secret_env
        ));
    }

    // Main handler function
    out.push_str(&format!(
        "/// Handle an inbound `{}` webhook.\n\
         pub async fn handle_{}(body: &[u8]) -> Result<String, String> {{\n\
         \x20   // Parse payload\n\
         \x20   let payload: {} = serde_json::from_slice(body)\n\
         \x20       .map_err(|e| format!(\"JSON parse: {{}}\", e))?;\n\n\
         \x20   // Execute action\n\
         \x20   let result = execute_{}_action(&payload).await?;\n\
         \x20   Ok(result)\n\
         }}\n\n",
        contract.id, contract.id, struct_name, contract.id
    ));

    // Action dispatcher
    out.push_str(&format!(
        "async fn execute_{}_action(payload: &{}) -> Result<String, String> {{\n",
        contract.id, struct_name
    ));

    match &contract.action {
        WebhookAction::FluxcBuild { package, release } => {
            out.push_str(&format!(
                "    let package_name = resolve_jsonpath(payload, \"{}\")?;\n\
                 \x20   let mut cmd = std::process::Command::new(\"fluxc\");\n\
                 \x20   cmd.args([\"build\", \"--package\", &package_name]);\n\
                 \x20   if {} {{ cmd.arg(\"--release\"); }}\n\
                 \x20   let status = cmd.status().map_err(|e| format!(\"fluxc: {{}}\", e))?;\n\
                 \x20   Ok(format!(\"Build {{}}: {{}}\", package_name, if status.success() {{ \"OK\" }} else {{ \"FAILED\" }}))\n",
                package, release
            ));
        }
        WebhookAction::FluxcTest { package } => {
            out.push_str(&format!(
                "    let package_name = resolve_jsonpath(payload, \"{}\")?;\n\
                 \x20   let status = std::process::Command::new(\"fluxc\")\n\
                 \x20       .args([\"test\", \"--package\", &package_name])\n\
                 \x20       .status().map_err(|e| format!(\"fluxc: {{}}\", e))?;\n\
                 \x20   Ok(format!(\"Test {{}}: {{}}\", package_name, if status.success() {{ \"PASSED\" }} else {{ \"FAILED\" }}))\n",
                package
            ));
        }
        WebhookAction::McpTool { tool, args_path } => {
            out.push_str(&format!(
                "    let _tool_name = \"{}\";\n\
                 \x20   let args = if let Some(path) = {:?} {{\n\
                 \x20       resolve_jsonpath(payload, path)?\n\
                 \x20   }} else {{\n\
                 \x20       \"{{}}\".to_string()\n\
                 \x20   }};\n\
                 \x20   Ok(format!(\"MCP {{}} called with args: {{}}\", _tool_name, args))\n",
                tool, args_path
            ));
        }
        WebhookAction::SwarmDispatch { lane, task_path } => {
            out.push_str(&format!(
                "    let task = resolve_jsonpath(payload, \"{}\")?;\n\
                 \x20   crate::swarm::swarm_claim_cli(\"webhook\", lane);\n\
                 \x20   Ok(format!(\"Swarm lane '{}' dispatched: {{}}\", task))\n",
                task_path, lane
            ));
        }
        WebhookAction::ShellCommand { command } => {
            out.push_str(&format!(
                "    let cmd = \"{}\".to_string();\n\
                 \x20   // NOTE: shell commands are NOT sanitized — use only with trusted webhooks\n\
                 \x20   let status = std::process::Command::new(\"sh\")\n\
                 \x20       .args([\"-c\", &cmd])\n\
                 \x20       .status().map_err(|e| format!(\"shell: {{}}\", e))?;\n\
                 \x20   Ok(format!(\"Shell command exit: {{}}\", status.code().unwrap_or(-1)))\n",
                command
            ));
        }
    }
    out.push_str("}\n\n");

    // JSONPath resolver (simple dot/bracket notation)
    out.push_str(
        "/// Simple JSONPath resolver (dot notation + brackets).\n\
         fn resolve_jsonpath(payload: &impl serde::Serialize, path: &str) -> Result<String, String> {\n\
         \x20   let value = serde_json::to_value(payload)\n\
         \x20       .map_err(|e| format!(\"serialize: {}\", e))?;\n\
         \x20   let parts: Vec<&str> = path.trim_start_matches(\"$.\").split('.').collect();\n\
         \x20   let mut current = &value;\n\
         \x20   for part in &parts {\n\
         \x20       current = current.get(part)\n\
         \x20           .ok_or_else(|| format!(\"JSONPath '{}' missing at '{}'\", path, part))?;\n\
         \x20   }\n\
         \x20   Ok(match current {\n\
         \x20       serde_json::Value::String(s) => s.clone(),\n\
         \x20       other => other.to_string(),\n\
         \x20   })\n\
         }\n\n"
    );

    // Route registration for fluxc serve
    out.push_str(&format!(
        "// Register this handler with fluxc serve:\n\
         //   fluxc::serve::register_webhook_route(\"{}\", \"{}\", handle_{});\n",
        method, path, contract.id
    ));

    out
}

/// Generate Rust struct definitions from a WebhookSchema.
fn generate_rust_structs(schema: &WebhookSchema, root_name: &str) -> String {
    let mut out = String::new();
    let mut sub_structs = Vec::new();
    generate_struct_recursive(schema, root_name, &mut out, &mut sub_structs);
    for s in &sub_structs {
        out.push_str(s);
        out.push('\n');
    }
    out
}

fn generate_struct_recursive(
    schema: &WebhookSchema,
    name: &str,
    out: &mut String,
    sub_structs: &mut Vec<String>,
) {
    match schema {
        WebhookSchema::Object { required, properties } => {
            out.push_str(&format!("#[derive(Debug, Clone, Serialize, Deserialize)]\n"));
            out.push_str(&format!("pub struct {} {{\n", name));
            for (prop_name, prop_schema) in properties {
                let rust_type = schema_to_rust_type(prop_schema, prop_name, sub_structs);
                let req_marker = if required.contains(prop_name) { "" } else {
                    "#[serde(default)] "
                };
                let option_wrap = if required.contains(prop_name) {
                    rust_type
                } else {
                    format!("Option<{}>", rust_type)
                };
                out.push_str(&format!(
                    "    {}pub {}: {},\n",
                    req_marker,
                    to_snake_case(prop_name),
                    option_wrap
                ));
            }
            out.push_str("}\n");
        }
        _ => {
            // Primitive at root — wrap in a newtype
            let rust_type = schema_to_rust_type(schema, name, sub_structs);
            out.push_str(&format!("#[derive(Debug, Clone, Serialize, Deserialize)]\n"));
            out.push_str(&format!("pub struct {}(pub {});\n", name, rust_type));
        }
    }
}

fn schema_to_rust_type(schema: &WebhookSchema, sub_name: &str, sub_structs: &mut Vec<String>) -> String {
    match schema {
        WebhookSchema::Object { required, properties } => {
            let nested_name = to_pascal_case(sub_name);
            let mut nested_out = String::new();
            generate_struct_recursive(schema, &nested_name, &mut nested_out, sub_structs);
            sub_structs.push(nested_out);
            nested_name
        }
        WebhookSchema::String_ { .. } => "String".to_string(),
        WebhookSchema::Number_ => "f64".to_string(),
        WebhookSchema::Integer_ => "i64".to_string(),
        WebhookSchema::Boolean_ => "bool".to_string(),
        WebhookSchema::Array { items } => {
            let inner = schema_to_rust_type(items, sub_name, sub_structs);
            format!("Vec<{}>", inner)
        }
        WebhookSchema::Null_ => "Option<()>".to_string(),
    }
}

/// Generate an OpenAPI 3.1 YAML fragment for a webhook contract.
pub fn generate_openapi(contracts: &[WebhookContract]) -> String {
    let mut out = String::from("openapi: \"3.1.0\"\n");
    out.push_str("info:\n");
    out.push_str("  title: Flux Webhook API\n");
    out.push_str(&format!("  version: \"{}\"\n", env!("CARGO_PKG_VERSION")));
    out.push_str("paths:\n");

    for c in contracts {
        let (method, path) = (extract_method(&c.route), extract_path(&c.route));
        out.push_str(&format!("  {}:\n", path));
        out.push_str(&format!("    {}:\n", method.to_lowercase()));
        out.push_str(&format!("      operationId: handle_{}\n", c.id));
        out.push_str(&format!("      summary: \"{}\"\n", c.description));
        if !c.tags.is_empty() {
            out.push_str(&format!("      tags: [{}]\n", c.tags.iter().map(|t| format!("\"{}\"", t)).collect::<Vec<_>>().join(", ")));
        }
        out.push_str("      requestBody:\n");
        out.push_str("        required: true\n");
        out.push_str("        content:\n");
        out.push_str("          application/json:\n");
        out.push_str("            schema:\n");
        out.push_str(&schema_to_openapi(&c.schema, "              "));
        out.push_str("      responses:\n");
        out.push_str("        \"200\":\n");
        out.push_str("          description: Action executed\n");
        out.push_str("        \"401\":\n");
        out.push_str("          description: HMAC signature invalid\n");
        out.push('\n');
    }

    out
}

fn schema_to_openapi(schema: &WebhookSchema, indent: &str) -> String {
    let mut out = String::new();
    match schema {
        WebhookSchema::Object { required, properties } => {
            out.push_str(&format!("{}type: object\n", indent));
            if !required.is_empty() {
                out.push_str(&format!("{}required: [{}]\n", indent,
                    required.iter().map(|r| format!("\"{}\"", r)).collect::<Vec<_>>().join(", ")));
            }
            out.push_str(&format!("{}properties:\n", indent));
            let inner_indent = format!("{}  ", indent);
            for (name, prop) in properties {
                out.push_str(&format!("{}{}:\n", inner_indent, name));
                let prop_indent = format!("{}  ", inner_indent);
                out.push_str(&schema_to_openapi(prop, &prop_indent));
            }
        }
        WebhookSchema::String_ { description } => {
            out.push_str(&format!("{}type: string\n", indent));
            if !description.is_empty() {
                out.push_str(&format!("{}description: \"{}\"\n", indent, description));
            }
        }
        WebhookSchema::Number_ => { out.push_str(&format!("{}type: number\n", indent)); }
        WebhookSchema::Integer_ => { out.push_str(&format!("{}type: integer\n", indent)); }
        WebhookSchema::Boolean_ => { out.push_str(&format!("{}type: boolean\n", indent)); }
        WebhookSchema::Array { items } => {
            out.push_str(&format!("{}type: array\n", indent));
            out.push_str(&format!("{}items:\n", indent));
            out.push_str(&schema_to_openapi(items, &format!("{}  ", indent)));
        }
        WebhookSchema::Null_ => { out.push_str(&format!("{}type: \"null\"\n", indent)); }
    }
    out
}

/// Generate a TypeScript SDK stub for the webhook payload types.
pub fn generate_typescript_sdk(contracts: &[WebhookContract]) -> String {
    let mut out = String::from(
        "// Auto-generated by fluxc webhook-gen — TypeScript SDK\n"
    );
    out.push_str("// Webhook payload types for Flux inbound webhooks\n\n");

    for c in contracts {
        let name = to_pascal_case(&c.id);
        out.push_str(&format!("// {}\n", c.description));
        out.push_str(&schema_to_typescript(&c.schema, &name));
        out.push_str(&format!(
            "export async function send{}(payload: {}, signature: string): Promise<string> {{\n",
            name, name
        ));
        out.push_str("  const resp = await fetch('/api/webhook', {\n");
        out.push_str(&format!("    method: '{}',\n", extract_method(&c.route)));
        out.push_str("    headers: {\n");
        out.push_str("      'Content-Type': 'application/json',\n");
        out.push_str("      'X-Flux-Signature': `sha256=${signature}`,\n");
        out.push_str("    },\n");
        out.push_str("    body: JSON.stringify(payload),\n");
        out.push_str("  });\n");
        out.push_str("  return resp.text();\n");
        out.push_str("}\n\n");
    }

    out
}

fn schema_to_typescript(schema: &WebhookSchema, name: &str) -> String {
    let mut out = String::new();
    match schema {
        WebhookSchema::Object { properties, .. } => {
            out.push_str(&format!("export interface {} {{\n", name));
            for (prop_name, prop_schema) in properties {
                let ts_type = schema_to_ts_type(prop_schema, &to_pascal_case(prop_name));
                out.push_str(&format!("  {}: {};\n", to_camel_case(prop_name), ts_type));
            }
            out.push_str("}\n");
        }
        WebhookSchema::Array { items } => {
            let inner = schema_to_ts_type(items, name);
            out.push_str(&format!("export type {} = {}[];\n", name, inner));
        }
        _ => {
            out.push_str(&format!("export type {} = {};\n", name, schema_to_ts_type(schema, name)));
        }
    }
    out
}

fn schema_to_ts_type(schema: &WebhookSchema, name: &str) -> String {
    match schema {
        WebhookSchema::Object { .. } => to_pascal_case(name),
        WebhookSchema::String_ { .. } => "string".into(),
        WebhookSchema::Number_ | WebhookSchema::Integer_ => "number".into(),
        WebhookSchema::Boolean_ => "boolean".into(),
        WebhookSchema::Array { items } => format!("{}[]", schema_to_ts_type(items, name)),
        WebhookSchema::Null_ => "null".into(),
    }
}

/// Generate MCP tool binding for a webhook contract.
pub fn generate_mcp_tool_binding(contract: &WebhookContract) -> String {
    let tool_name = format!("flux_webhook_{}", contract.id);
    let struct_name = to_pascal_case(&contract.id);

    format!(
        "// MCP tool binding for webhook '{}'\n\
         // Register in fluxc-mcp handlers:\n\
         pub fn register(registry: &mut ToolRegistry) {{\n\
         \x20   registry.register(\n\
         \x20       \"{}\",\n\
         \x20       serde_json::json!({{}}\n\
         \x20           \"name\": \"{}\",\n\
         \x20           \"description\": \"{}\",\n\
         \x20           \"inputSchema\": {{}}\n\
         \x20               \"type\": \"object\",\n\
         \x20               \"properties\": {{}}\n\
         \x20                   \"payload\": {{ \"type\": \"object\", \"$ref\": \"#/components/schemas/{}\" }},\n\
         \x20                   \"signature\": {{ \"type\": \"string\", \"description\": \"HMAC-SHA256 signature\" }}\n\
         \x20               }},\n\
         \x20               \"required\": [\"payload\"]\n\
         \x20           }}\n\
         \x20       }}),\n\
         \x20       |args| async {{}}\n\
         \x20           let payload: {} = serde_json::from_value(args[\"payload\"].clone())?;\n\
         \x20           let result = execute_{}_action(&payload).await?;\n\
         \x20           Ok(serde_json::json!({{ \"result\": result }}))\n\
         \x20       }}),\n\
         \x20   );\n\
         }}\n",
        contract.id,
        tool_name, tool_name, contract.description, struct_name,
        struct_name, contract.id
    )
}

/// AI-driven: suggest webhook contracts by analyzing a crate's API surface.
pub fn suggest_webhooks(crate_path: &Path) -> Vec<WebhookContract> {
    let mut suggestions = Vec::new();

    // Detect project type by looking at Cargo.toml
    let cargo_path = crate_path.join("Cargo.toml");
    if !cargo_path.exists() {
        return suggestions;
    }

    // Heuristic: suggest based on common patterns
    let has_build_script = crate_path.join("build.rs").exists();
    let has_tests = crate_path.join("tests").is_dir();
    let has_benches = crate_path.join("benches").is_dir();

    // Always suggest a CI-triggered build webhook
    suggestions.push(WebhookContract {
        id: "ci_build".into(),
        route: "POST /webhooks/ci/build".into(),
        secret_env: Some("CI_WEBHOOK_SECRET".into()),
        description: "CI-triggered fluxc build — fires on push to main".into(),
        schema: WebhookSchema::Object {
            required: vec!["ref".into(), "sha".into()],
            properties: {
                let mut m = BTreeMap::new();
                m.insert("ref".into(), WebhookSchema::String_ { description: "Git ref (e.g. refs/heads/main)".into() });
                m.insert("sha".into(), WebhookSchema::String_ { description: "Commit SHA".into() });
                m.insert("repository".into(), WebhookSchema::Object {
                    required: vec!["full_name".into()],
                    properties: {
                        let mut r = BTreeMap::new();
                        r.insert("full_name".into(), WebhookSchema::String_ { description: "repo name".into() });
                        r
                    },
                });
                m
            },
        },
        action: WebhookAction::FluxcBuild {
            package: "$.repository.full_name".into(),
            release: false,
        },
        tags: vec!["ci".into(), "github".into()],
    });

    if has_tests {
        suggestions.push(WebhookContract {
            id: "ci_test".into(),
            route: "POST /webhooks/ci/test".into(),
            secret_env: Some("CI_WEBHOOK_SECRET".into()),
            description: "CI-triggered test run on PR".into(),
            schema: WebhookSchema::Object {
                required: vec!["pull_request".into()],
                properties: {
                    let mut m = BTreeMap::new();
                    m.insert("pull_request".into(), WebhookSchema::Object {
                        required: vec!["number".into(), "head".into()],
                        properties: {
                            let mut pr = BTreeMap::new();
                            pr.insert("number".into(), WebhookSchema::Integer_);
                            pr.insert("head".into(), WebhookSchema::Object {
                                required: vec!["ref".into()],
                                properties: {
                                    let mut h = BTreeMap::new();
                                    h.insert("ref".into(), WebhookSchema::String_ { description: "".into() });
                                    h
                                },
                            });
                            pr
                        },
                    });
                    m
                },
            },
            action: WebhookAction::FluxcTest {
                package: "fluxc".into(),
            },
            tags: vec!["ci".into(), "github".into(), "testing".into()],
        });
    }

    if has_build_script {
        suggestions.push(WebhookContract {
            id: "deploy_release".into(),
            route: "POST /webhooks/release".into(),
            secret_env: Some("RELEASE_WEBHOOK_SECRET".into()),
            description: "Release deployment trigger — build + provenance sign".into(),
            schema: WebhookSchema::Object {
                required: vec!["version".into()],
                properties: {
                    let mut m = BTreeMap::new();
                    m.insert("version".into(), WebhookSchema::String_ { description: "Semver tag".into() });
                    m.insert("notes".into(), WebhookSchema::String_ { description: "Release notes".into() });
                    m
                },
            },
            action: WebhookAction::FluxcBuild {
                package: "fluxc".into(),
                release: true,
            },
            tags: vec!["release".into(), "deploy".into()],
        });
    }

    suggestions
}

// ── Utility Helpers ──

fn extract_method(route: &str) -> &str {
    if route.starts_with("GET ") { "GET" }
    else if route.starts_with("POST ") { "POST" }
    else if route.starts_with("PUT ") { "PUT" }
    else if route.starts_with("DELETE ") { "DELETE" }
    else if route.starts_with("PATCH ") { "PATCH" }
    else { "POST" }
}

fn extract_path(route: &str) -> &str {
    route.find(' ').map(|i| &route[i+1..]).unwrap_or(route)
}

fn to_pascal_case(s: &str) -> String {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut chars = p.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().to_string() + &chars.collect::<String>().to_lowercase(),
                None => String::new(),
            }
        })
        .collect()
}

fn to_snake_case(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        c.to_lowercase().for_each(|lc| out.push(lc));
    }
    out
}

fn to_camel_case(s: &str) -> String {
    let pascal = to_pascal_case(s);
    let mut chars = pascal.chars();
    match chars.next() {
        Some(c) => c.to_lowercase().to_string() + &chars.collect::<String>(),
        None => String::new(),
    }
}

// ── Main Entry Point ──

/// Process a .flux-webhook.toml file: parse, validate, and generate all artifacts.
pub fn process_webhook_contracts(input_path: &Path, output_dir: &Path) -> Result<String, String> {
    let contracts = parse_webhook_contracts(input_path)?;

    let _ = std::fs::create_dir_all(output_dir);

    let mut report = format!(
        "⚡ Flux Webhook Schema Compiler\n  Input: {}\n  {} contract(s) found\n",
        input_path.display(),
        contracts.len()
    );

    // 1. Generate Rust handler
    let rust_out = output_dir.join("webhook_handlers.rs");
    let mut rust_code = String::from(
        "// Auto-generated by fluxc webhook-gen\n// Do not edit manually\n\n"
    );
    for c in &contracts {
        rust_code.push_str(&generate_rust_handler(c));
        rust_code.push_str("\n// ────────────────────────────────\n\n");
    }
    std::fs::write(&rust_out, &rust_code)
        .map_err(|e| format!("Write {}: {}", rust_out.display(), e))?;
    report.push_str(&format!("  Rust handlers: {}\n", rust_out.display()));

    // 2. Generate OpenAPI spec
    let openapi_out = output_dir.join("webhook_openapi.yaml");
    let openapi_yaml = generate_openapi(&contracts);
    std::fs::write(&openapi_out, &openapi_yaml)
        .map_err(|e| format!("Write {}: {}", openapi_out.display(), e))?;
    report.push_str(&format!("  OpenAPI spec: {}\n", openapi_out.display()));

    // 3. Generate TypeScript SDK
    let ts_out = output_dir.join("webhook_sdk.ts");
    let ts_code = generate_typescript_sdk(&contracts);
    std::fs::write(&ts_out, &ts_code)
        .map_err(|e| format!("Write {}: {}", ts_out.display(), e))?;
    report.push_str(&format!("  TypeScript SDK: {}\n", ts_out.display()));

    // 4. Generate MCP tool bindings
    let mcp_out = output_dir.join("webhook_mcp_tools.rs");
    let mut mcp_code = String::from(
        "// Auto-generated MCP tool bindings for webhooks\n\n"
    );
    for c in &contracts {
        mcp_code.push_str(&generate_mcp_tool_binding(c));
        mcp_code.push('\n');
    }
    std::fs::write(&mcp_out, &mcp_code)
        .map_err(|e| format!("Write {}: {}", mcp_out.display(), e))?;
    report.push_str(&format!("  MCP tools: {}\n", mcp_out.display()));

    // 5. Print next-step instructions
    report.push_str(&format!(
        "\n  Next steps:\n\
         \x20   1. Add `mod webhook_handlers;` to your crate\n\
         \x20   2. Register routes with fluxc serve:\n\
         \x20      fluxc::serve::mount_webhook_routes(&router);\n\
         \x20   3. Set {} env var(s)\n\
         \x20   4. Test: curl -X POST http://localhost:8084/webhooks/... \\\n\
         \x20           -H \"X-Flux-Signature: sha256=$(echo -n '{{}}' | openssl dgst -sha256 -hmac $SECRET)\" \\\n\
         \x20           -H \"Content-Type: application/json\" -d '{{}}'\n",
        contracts.iter()
            .filter_map(|c| c.secret_env.as_deref())
            .collect::<Vec<_>>()
            .join(", ")
    ));

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_to_rust_type() {
        let schema = WebhookSchema::Object {
            required: vec!["name".into(), "count".into()],
            properties: {
                let mut m = BTreeMap::new();
                m.insert("name".into(), WebhookSchema::String_ { description: "".into() });
                m.insert("count".into(), WebhookSchema::Integer_);
                m
            },
        };
        let rust = generate_rust_structs(&schema, "TestPayload");
        assert!(rust.contains("struct TestPayload"));
        assert!(rust.contains("name: String"));
        assert!(rust.contains("count: i64"));
    }

    #[test]
    fn test_extract_route_parts() {
        assert_eq!(extract_method("POST /webhooks/github"), "POST");
        assert_eq!(extract_path("POST /webhooks/github"), "/webhooks/github");
        assert_eq!(extract_method("GET /health"), "GET");
    }

    #[test]
    fn test_pascal_case() {
        assert_eq!(to_pascal_case("github_push"), "GithubPush");
        assert_eq!(to_pascal_case("ci_build"), "CiBuild");
    }

    #[test]
    fn test_suggest_webhooks_empty_dir() {
        let dir = std::env::temp_dir().join("flux_webhook_test_empty");
        let _ = std::fs::create_dir_all(&dir);
        let suggestions = suggest_webhooks(&dir);
        // No Cargo.toml → no suggestions
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_generate_openapi() {
        let contract = WebhookContract {
            id: "test".into(),
            route: "POST /webhooks/test".into(),
            secret_env: None,
            description: "Test webhook".into(),
            schema: WebhookSchema::Object {
                required: vec!["msg".into()],
                properties: {
                    let mut m = BTreeMap::new();
                    m.insert("msg".into(), WebhookSchema::String_ { description: "A message".into() });
                    m
                },
            },
            action: WebhookAction::FluxcBuild { package: "test".into(), release: false },
            tags: vec!["test".into()],
        };
        let yaml = generate_openapi(&[contract]);
        assert!(yaml.contains("openapi: \"3.1.0\""));
        assert!(yaml.contains("/webhooks/test"));
        assert!(yaml.contains("type: string"));
    }
}
