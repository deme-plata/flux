"""Add Gemma4 chat tab to fluxmux — free local AI in the TUI."""
path = "/home/storage/deepseek-codewhale/flux/crates/fluxmux/src/main.rs"
with open(path) as f:
    c = f.read()

# 1. Add Gemma4 to Tab enum
c = c.replace(
    "enum Tab { Build, P2P, SelfHeal, Stats, Journal, Tasks }",
    "enum Tab { Build, P2P, SelfHeal, Stats, Journal, Tasks, Gemma4 }"
)

# 2. Add Gemma4 title
c = c.replace(
    'Tab::Tasks => "📋 Tasks"',
    'Tab::Tasks => "📋 Tasks", Tab::Gemma4 => "🤖 Gemma4"'
)

# 3. Add to all()
c = c.replace(
    'vec![Tab::Build, Tab::P2P, Tab::SelfHeal, Tab::Stats, Tab::Journal, Tab::Tasks]',
    'vec![Tab::Build, Tab::P2P, Tab::SelfHeal, Tab::Stats, Tab::Journal, Tab::Tasks, Tab::Gemma4]'
)

# 4. Add gemma state fields to App
c = c.replace(
    '    wallet_balance: String,\n    // Button states',
    '    wallet_balance: String,\n    // Gemma4 chat\n    gemma_input: String,\n    gemma_response: String,\n    gemma_loading: bool,\n    gemma_history: Vec<(String, String)>,\n    // Button states'
)

# 5. Initialize gemma fields
c = c.replace(
    '            wallet_balance: "—".into(),\n            selected_button: 0,',
    '            wallet_balance: "—".into(),\n            gemma_input: String::new(),\n            gemma_response: String::new(),\n            gemma_loading: false,\n            gemma_history: Vec::new(),\n            selected_button: 0,'
)

# 6. Add render_gemma4 match arm
c = c.replace(
    '        Tab::Tasks => render_tasks(frame, content_area, app),\n    }',
    '        Tab::Tasks => render_tasks(frame, content_area, app),\n        Tab::Gemma4 => render_gemma4(frame, content_area, app),\n    }'
)

# 7. Add '7' key binding
c = c.replace(
    "KeyCode::Char('6') => app.tab = Tab::Tasks,",
    "KeyCode::Char('6') => app.tab = Tab::Tasks,\n                        KeyCode::Char('7') => app.tab = Tab::Gemma4,"
)

# 8. Add text input handling in Gemma4 tab (before existing Char match)
old_char = '''                        KeyCode::Up => {
                            app.selected_button = app.selected_button.saturating_sub(1);'''
new_char = '''                        // Gemma4 chat input
                        KeyCode::Char(c) if app.tab == Tab::Gemma4 && !app.gemma_loading => {
                            app.gemma_input.push(c);
                        }
                        KeyCode::Backspace if app.tab == Tab::Gemma4 => {
                            app.gemma_input.pop();
                        }
                        KeyCode::Up => {
                            app.selected_button = app.selected_button.saturating_sub(1);'''
c = c.replace(old_char, new_char)

# 9. Add Enter handler for Gemma4 tab
old_enter = '''                        KeyCode::Enter => {
                            app.log(&format!("ACTION: button {} pressed on tab {:?}", app.selected_button, app.tab));
                            match (&app.tab, app.selected_button) {'''
new_enter = '''                        KeyCode::Enter => {
                            if app.tab == Tab::Gemma4 && !app.gemma_input.is_empty() && !app.gemma_loading {
                                let prompt = app.gemma_input.clone();
                                app.gemma_input.clear();
                                app.gemma_response = "⏳ Thinking...".into();
                                app.gemma_loading = true;
                                app.log(&format!("🤖 Gemma4: {}", &prompt[..prompt.len().min(60)]));
                                // Spawn background Ollama call
                                let app_clone = std::sync::Arc::new(std::sync::Mutex::new(&mut app)); // can't share — use thread
                                std::thread::spawn(move || {
                                    let resp = gemma4_chat(&prompt);
                                    let _ = std::fs::write("/tmp/flux-gemma.response", resp);
                                    let _ = std::fs::write("/tmp/flux-gemma.done", "1");
                                });
                            } else {
                            app.log(&format!("ACTION: button {} pressed on tab {:?}", app.selected_button, app.tab));
                            match (&app.tab, app.selected_button) {'''
c = c.replace(old_enter, new_enter)

# 10. Fix: close the new else branch properly
c = c.replace(
    '                                _ => { app.log("ℹ️ Button pressed — no action bound"); }\n                            }\n                        }',
    '                                _ => { app.log("ℹ️ Button pressed — no action bound"); }\n                            }\n                            } // close else\n                        }'
)

# 11. Add gemma4_chat function before spawn_periodic_update_check
gemma_fn = '''/// Gemma4 chat: send prompt to Ollama, return response.
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
    match client.post("http://localhost:11434/api/generate")
        .json(&body).send().and_then(|r| r.json::<serde_json::Value>()) {
        Ok(v) => v.get("response").and_then(|r| r.as_str()).unwrap_or("(no response)").to_string(),
        Err(e) => format!("Ollama error: {} (is it running?)", e),
    }
}

/// Spawn periodic background update checker (runs every 5 min).'''
c = c.replace(
    '/// Spawn periodic background update checker (runs every 5 min).',
    gemma_fn
)

# 12. Add gemma poll to tick()
old_tick_poll = '    fn tick(&mut self) {\n        self.poll_update();'
new_tick_poll = '''    fn tick(&mut self) {
        self.poll_update();
        // Check for Gemma4 response completion
        if self.gemma_loading {
            if let Ok(_) = std::fs::read_to_string("/tmp/flux-gemma.done") {
                if let Ok(resp) = std::fs::read_to_string("/tmp/flux-gemma.response") {
                    self.gemma_response = resp;
                    self.gemma_history.push(("You".into(), self.gemma_response.clone()));
                    self.gemma_loading = false;
                    let _ = std::fs::remove_file("/tmp/flux-gemma.done");
                    let _ = std::fs::remove_file("/tmp/flux-gemma.response");
                }
            }
        }'''
c = c.replace(old_tick_poll, new_tick_poll)

# 13. Add render_gemma4 function before render_journal
render_gemma4 = '''fn render_gemma4(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let chunks = Layout::default().direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)]).split(area);
    
    // Response/history area
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![Span::styled("🤖 Gemma4 Chat — free local AI ($0.00)", Style::default().fg(Color::Rgb(212, 175, 55)).add_modifier(Modifier::BOLD))]));
    lines.push(Line::from(""));
    for (q, a) in app.gemma_history.iter().rev().take(5) {
        lines.push(Line::from(vec![Span::styled("> ", Style::default().fg(Color::Cyan)), Span::raw(q)]));
        for aline in a.lines().take(4) {
            lines.push(Line::from(vec![Span::raw(aline)]));
        }
        lines.push(Line::from(""));
    }
    if app.gemma_loading {
        lines.push(Line::from(vec![Span::styled("⏳ ", Style::default().fg(Color::Yellow)), Span::raw(&app.gemma_response)]));
    }
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" Gemma4 Response ")),
        chunks[0],
    );
    
    // Input area
    let prompt = format!("> {}", app.gemma_input);
    let input = Paragraph::new(Line::from(vec![
        Span::styled(if app.gemma_loading { "⏳ " } else { "💬 " }, Style::default().fg(Color::Green)),
        Span::raw(&prompt),
    ])).block(Block::default().borders(Borders::ALL).title(" Prompt (type + Enter to send) "));
    frame.render_widget(input, chunks[1]);
}

fn render_journal(frame: &mut ratatui::Frame, area: Rect, app: &App) {'''

c = c.replace(
    'fn render_journal(frame: &mut ratatui::Frame, area: Rect, app: &App) {',
    render_gemma4
)

with open(path, 'w') as f:
    f.write(c)
print("OK: Added Gemma4 chat tab to fluxmux")
