"""Fix Gemma4 chat compilation errors in fluxmux."""
path = "/home/storage/deepseek-codewhale/flux/crates/fluxmux/src/main.rs"
with open(path) as f:
    c = f.read()

# Fix 1: Replace .json() on blocking RequestBuilder with manual JSON body
old_json = '''    match client.post("http://localhost:11434/api/generate")
        .json(&body).send().and_then(|r| r.json::<serde_json::Value>()) {'''
new_json = '''    let body_str = serde_json::to_string(&body).unwrap_or_default();
    match client.post("http://localhost:11434/api/generate")
        .header("Content-Type", "application/json")
        .body(body_str).send().and_then(|r| r.json::<serde_json::Value>()) {'''
c = c.replace(old_json, new_json)

# Fix 2: Fix if let Ok(_) type ambiguity in tick()
old_ok = '''        if self.gemma_loading {
            if let Ok(_) = std::fs::read_to_string("/tmp/flux-gemma.done") {'''
new_ok = '''        if self.gemma_loading {
            if std::fs::metadata("/tmp/flux-gemma.done").is_ok() {'''
c = c.replace(old_ok, new_ok)

# Fix 3: Remove unused app_clone
old_clone = '''                                app.log(&format!("🤖 Gemma4: {}", &prompt[..prompt.len().min(60)]));
                                // Spawn background Ollama call
                                let app_clone = std::sync::Arc::new(std::sync::Mutex::new(&mut app)); // can't share — use thread
                                std::thread::spawn(move || {'''
new_clone = '''                                app.log(&format!("🤖 Gemma4: {}", &prompt[..prompt.len().min(60)]));
                                // Spawn background Ollama call
                                std::thread::spawn(move || {'''
c = c.replace(old_clone, new_clone)

with open(path, 'w') as f:
    f.write(c)
print("OK: Fixed Gemma4 compilation errors")
