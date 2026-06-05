"""Fix fluxmux: use .text() + serde_json instead of .json() on blocking response."""
path = "/home/storage/deepseek-codewhale/flux/crates/fluxmux/src/main.rs"
with open(path) as f:
    c = f.read()

old = '''    let body_str = serde_json::to_string(&body).unwrap_or_default();
    match client.post("http://localhost:11434/api/generate")
        .header("Content-Type", "application/json")
        .body(body_str).send().and_then(|r| r.json::<serde_json::Value>()) {
        Ok(v) => v.get("response").and_then(|r| r.as_str()).unwrap_or("(no response)").to_string(),
        Err(e) => format!("Ollama error: {} (is it running?)", e),
    }'''

new = '''    let body_str = serde_json::to_string(&body).unwrap_or_default();
    let resp = match client.post("http://localhost:11434/api/generate")
        .header("Content-Type", "application/json")
        .body(body_str).send() {
        Ok(r) => r,
        Err(e) => return format!("Ollama error: {} (is it running?)", e),
    };
    let text = resp.text().unwrap_or_default();
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(v) => v.get("response").and_then(|r| r.as_str()).unwrap_or("(no response)").to_string(),
        Err(_) => format!("Parse error: {}", &text[..text.len().min(100)]),
    }'''

c = c.replace(old, new)
with open(path, 'w') as f:
    f.write(c)
print("OK: Fixed reqwest .json() -> .text() + serde_json")
