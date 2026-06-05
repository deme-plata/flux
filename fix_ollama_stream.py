"""Fix Gemma4: streaming Ollama API + partial display in TUI."""
path = "/home/storage/deepseek-codewhale/flux/crates/fluxmux/src/main.rs"
with open(path) as f:
    c = f.read()

# 1. Replace gemma4_chat with streaming version using std::io::Read
old_fn = '''/// Gemma4 chat: send prompt to Ollama, return response.
fn gemma4_chat(prompt: &str) -> String {
    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60)).build() {
        Ok(c) => c, Err(e) => return format!("Client error: {}", e),
    };
    let body = serde_json::json!({
        "model": "gemma4:latest",
        "prompt": prompt,
        "stream": false,
        "options": {"temperature": 0.7, "num_predict": 300}
    });
    let body_str = serde_json::to_string(&body).unwrap_or_default();
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
    }
}'''

new_fn = '''/// Gemma4 chat: stream response from Ollama, write to /tmp/flux-gemma.response.
/// The TUI poll loop reads partial responses for live display.
fn gemma4_chat_stream(prompt: &str) {
    use std::io::{BufRead, BufReader, Write};
    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300)).build() {
        Ok(c) => c,
        Err(e) => { let _ = std::fs::write("/tmp/flux-gemma.response", format!("Error: {}", e)); let _ = std::fs::write("/tmp/flux-gemma.done", "1"); return; }
    };
    let body = serde_json::json!({
        "model": "gemma4:latest",
        "prompt": prompt,
        "stream": true,
        "options": {"temperature": 0.7, "num_predict": 300}
    });
    let body_str = serde_json::to_string(&body).unwrap_or_default();
    let resp = match client.post("http://localhost:11434/api/generate")
        .header("Content-Type", "application/json")
        .body(body_str).send() {
        Ok(r) => r,
        Err(e) => { let _ = std::fs::write("/tmp/flux-gemma.response", format!("Ollama: {}", e)); let _ = std::fs::write("/tmp/flux-gemma.done", "1"); return; }
    };
    
    // Stream: read JSON lines, extract "response" field, append to file
    let reader = BufReader::new(resp);
    let mut file = match std::fs::File::create("/tmp/flux-gemma.response") {
        Ok(f) => f,
        Err(_) => { let _ = std::fs::write("/tmp/flux-gemma.done", "1"); return; }
    };
    writeln!(file, "🤖 Gemma4:").ok();
    for line in reader.lines() {
        match line {
            Ok(l) if !l.trim().is_empty() => {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&l) {
                    if let Some(token) = v.get("response").and_then(|r| r.as_str()) {
                        write!(file, "{}", token).ok();
                        file.flush().ok();
                    }
                    if v.get("done").and_then(|d| d.as_bool()).unwrap_or(false) {
                        break;
                    }
                }
            }
            _ => break,
        }
    }
    let _ = std::fs::write("/tmp/flux-gemma.done", "1");
}'''

c = c.replace(old_fn, new_fn)

# 2. Update the thread spawn to use the streaming version
old_spawn = '''                                std::thread::spawn(move || {
                                    let resp = gemma4_chat(&prompt);
                                    let _ = std::fs::write("/tmp/flux-gemma.response", resp);
                                    let _ = std::fs::write("/tmp/flux-gemma.done", "1");
                                });'''

new_spawn = '''                                std::thread::spawn(move || {
                                    gemma4_chat_stream(&prompt);
                                });'''

c = c.replace(old_spawn, new_spawn)

# 3. Update tick() to show partial streaming response
old_tick = '''        // Check for Gemma4 response completion
        if self.gemma_loading {
            if std::fs::metadata("/tmp/flux-gemma.done").is_ok() {
                if let Ok(resp) = std::fs::read_to_string("/tmp/flux-gemma.response") {
                    self.gemma_response = resp;
                    self.gemma_history.push(("You".into(), self.gemma_response.clone()));
                    self.gemma_loading = false;
                    let _ = std::fs::remove_file("/tmp/flux-gemma.done");
                    let _ = std::fs::remove_file("/tmp/flux-gemma.response");
                }
            }
        }'''

new_tick = '''        // Check for Gemma4 response (streaming: show partial, finalize on done)
        if self.gemma_loading {
            if std::fs::metadata("/tmp/flux-gemma.done").is_ok() {
                if let Ok(resp) = std::fs::read_to_string("/tmp/flux-gemma.response") {
                    self.gemma_response = resp;
                    self.gemma_history.push(("You".into(), self.gemma_response.clone()));
                    self.gemma_loading = false;
                    let _ = std::fs::remove_file("/tmp/flux-gemma.done");
                    let _ = std::fs::remove_file("/tmp/flux-gemma.response");
                }
            } else if let Ok(partial) = std::fs::read_to_string("/tmp/flux-gemma.response") {
                self.gemma_response = partial; // live streaming update
            }
        }'''

c = c.replace(old_tick, new_tick)

with open(path, 'w') as f:
    f.write(c)
print("OK: Gemma4 streaming + live partial display")
