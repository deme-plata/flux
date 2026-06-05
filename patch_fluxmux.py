import re

path = "/home/storage/deepseek-codewhale/flux/crates/fluxmux/src/main.rs"
with open(path) as f:
    content = f.read()
orig = content

# 1. Remove eprintln!("DEBUG: thread started") from check_for_update_bg
content = content.replace(
    'std::thread::spawn(|| { eprintln!("DEBUG: thread started");\n        let client',
    'std::thread::spawn(|| {\n        let client'
)

# 2. Remove eprintln from spawn_webhook_server
content = content.replace(
    'std::thread::spawn(|| { eprintln!("DEBUG: thread started");\n        if let Ok(server)',
    'std::thread::spawn(|| {\n        if let Ok(server)'
)

# 3. Add update_available and update_version fields to App struct
content = content.replace(
    '    health_score: f64,\n    last_heal_event: String,\n    // Stats',
    '    health_score: f64,\n    last_heal_event: String,\n    update_available: bool,\n    update_version: String,\n    // Stats'
)

# 4. Initialize new fields in App::new()
content = content.replace(
    '            last_heal_event: "No events yet — monitoring active".into(),\n            uptime_secs: 0,',
    '            last_heal_event: "No events yet — monitoring active".into(),\n            update_available: false,\n            update_version: String::new(),\n            uptime_secs: 0,'
)

# 5. Add update indicator in header
old_header = '        Span::styled(format!("{} peers", app.peer_count), Style::default().fg(Color::Green)),\n        Span::styled("  │  🏥 ", Style::default().fg(Color::DarkGray)),'
new_header = '        Span::styled(format!("{} peers", app.peer_count), Style::default().fg(Color::Green)),\n    ];\n    if app.update_available {\n        header_spans.push(Span::styled(format!("  │  🔔 UPDATE v{} ", app.update_version), Style::default().bg(Color::Rgb(180, 120, 0)).fg(Color::Black).add_modifier(Modifier::BOLD)));\n    }\n    header_spans.append(&mut vec![\n        Span::styled("  │  🏥 ", Style::default().fg(Color::DarkGray)),'
content = content.replace(old_header, new_header)

# 5b. Fix vec close
old_close = '        Span::styled(format!("{} builds", app.build_count), Style::default().fg(Color::Rgb(6, 182, 212))),\n    ]))\n    .block(Block::default().borders(Borders::BOTTOM)'
new_close = '        Span::styled(format!("{} builds", app.build_count), Style::default().fg(Color::Rgb(6, 182, 212))),\n    ]);\n    let header = Paragraph::new(Line::from(header_spans))\n        .block(Block::default().borders(Borders::BOTTOM)'
content = content.replace(old_close, new_close)

# 5c. Fix vec start  
old_start = '    let header = Paragraph::new(Line::from(vec![\n        Span::styled("⚡ FluxMux "'
new_start = '    let mut header_spans = vec![\n        Span::styled("⚡ FluxMux "'
content = content.replace(old_start, new_start)

# 6. Add poll_update
old_tick = '\n    fn tick(&mut self) {'
new_tick = '''
    /// Periodic update poll: check for ready flag from background thread.
    fn poll_update(&mut self) {
        if !self.update_available {
            if let Ok(ver) = std::fs::read_to_string("/tmp/flux-update.ready") {
                let ver = ver.trim().to_string();
                if !ver.is_empty() {
                    self.update_available = true;
                    self.update_version = ver;
                    self.log(&format!("🔔 Update available: v{}", ver));
                }
            }
        }
    }

    fn tick(&mut self) {
        self.poll_update();'''
content = content.replace(old_tick, new_tick)

# 7. Add periodic update checker function
old_section = '// ═══════════════════════════════════════════════════════════════\n// UI rendering'
new_section = '''/// Spawn periodic background update checker (runs every 5 min).
fn spawn_periodic_update_check() {
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_secs(300));
        check_for_update_bg();
    });
}

// ═══════════════════════════════════════════════════════════════
// UI rendering'''
content = content.replace(old_section, new_section)

# 8. Add periodic check call in main
content = content.replace(
    'app.log("🔄 Auto-update: checking in background...");\n',
    'app.log("🔄 Auto-update: checking in background...");\n    spawn_periodic_update_check();\n    app.log("🔄 Periodic update check: every 5 min");\n'
)

if content == orig:
    print("ERROR: No changes made!")
else:
    with open(path, 'w') as f:
        f.write(content)
    print("OK: Patched fluxmux/src/main.rs")
