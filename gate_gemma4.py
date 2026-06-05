"""Gate Gemma4 behind FLUX_G4_ENABLE=1 env var, hide tab when disabled."""
path = "/home/storage/deepseek-codewhale/flux/crates/fluxmux/src/main.rs"
with open(path) as f:
    c = f.read()

# 1. Add gemma_enabled field to App struct
c = c.replace(
    '    gemma_input: String,\n    gemma_response: String,\n    gemma_loading: bool,\n    gemma_history: Vec<(String, String)>,\n    // Button states',
    '    gemma_input: String,\n    gemma_response: String,\n    gemma_loading: bool,\n    gemma_history: Vec<(String, String)>,\n    gemma_enabled: bool,\n    // Button states'
)

# 2. Init gemma_enabled from env
c = c.replace(
    '            gemma_history: Vec::new(),\n            selected_button: 0,',
    '            gemma_history: Vec::new(),\n            gemma_enabled: std::env::var("FLUX_G4_ENABLE").map(|v| v == "1").unwrap_or(false),\n            selected_button: 0,'
)

# 3. Filter Gemma4 from all() when disabled
old_all = "fn all() -> Vec<Tab> { vec![Tab::Build, Tab::P2P, Tab::SelfHeal, Tab::Stats, Tab::Journal, Tab::Tasks, Tab::Gemma4] }"
new_all = '''fn all() -> Vec<Tab> {
        let mut tabs = vec![Tab::Build, Tab::P2P, Tab::SelfHeal, Tab::Stats, Tab::Journal, Tab::Tasks];
        if std::env::var("FLUX_G4_ENABLE").map(|v| v == "1").unwrap_or(false) {
            tabs.push(Tab::Gemma4);
        }
        tabs
    }'''
c = c.replace(old_all, new_all)

# 4. Guard Enter handler for Gemma4
old_enter = 'if app.tab == Tab::Gemma4 && !app.gemma_input.is_empty() && !app.gemma_loading {'
new_enter = 'if app.tab == Tab::Gemma4 && app.gemma_enabled && !app.gemma_input.is_empty() && !app.gemma_loading {'
c = c.replace(old_enter, new_enter)

# 5. Guard Gemma4 text input
old_char_input = 'KeyCode::Char(c) if app.tab == Tab::Gemma4 && !app.gemma_loading => {'
new_char_input = 'KeyCode::Char(c) if app.tab == Tab::Gemma4 && app.gemma_enabled && !app.gemma_loading => {'
c = c.replace(old_char_input, new_char_input)

old_bs = 'KeyCode::Backspace if app.tab == Tab::Gemma4 => {'
new_bs = 'KeyCode::Backspace if app.tab == Tab::Gemma4 && app.gemma_enabled => {'
c = c.replace(old_bs, new_bs)

# 6. Show disabled message in render_gemma4 when not enabled
old_render_start = "lines.push(Line::from(vec![Span::styled(\"🤖 Gemma4 Chat — free local AI ($0.00)\", Style::default().fg(Color::Rgb(212, 175, 55)).add_modifier(Modifier::BOLD))]));"
new_render_start = '''    if !app.gemma_enabled {
        lines.push(Line::from(vec![Span::styled("🤖 Gemma4 — DISABLED", Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD))]));
        lines.push(Line::from(vec![Span::styled("Set FLUX_G4_ENABLE=1 to enable free local AI chat", Style::default().fg(Color::DarkGray))]));
        lines.push(Line::from(vec![Span::styled("Requires: ollama serve && ollama pull gemma4:latest", Style::default().fg(Color::DarkGray))]));
    } else {
    lines.push(Line::from(vec![Span::styled("🤖 Gemma4 Chat — free local AI ($0.00)", Style::default().fg(Color::Rgb(212, 175, 55)).add_modifier(Modifier::BOLD))]));'''
c = c.replace(old_render_start, new_render_start)

# Add closing brace for the else block - find the next lines.push after the title
# The pattern: after "Gemma4 Chat" line, there's lines.push("") then the history loop
# We need to close the else block after the history section
# Find: lines.push(Line::from("")); after the title, before "for (q, a)"
old_after_title = '    } else {\n    lines.push(Line::from(vec![Span::styled("🤖 Gemma4 Chat — free local AI ($0.00)", Style::default().fg(Color::Rgb(212, 175, 55)).add_modifier(Modifier::BOLD))]));\n    lines.push(Line::from(""));'
new_after_title = '    } else {\n    lines.push(Line::from(vec![Span::styled("🤖 Gemma4 Chat — free local AI ($0.00)", Style::default().fg(Color::Rgb(212, 175, 55)).add_modifier(Modifier::BOLD))]));\n    }'
# Actually this won't work because we're closing the else before the history. Let me restructure.

# Simpler: just add a closing brace before the history loop, and wrap the loop in the else
# Find the exact pattern in the file
# Actually, let me just wrap the rest of the function in the else block

# The render_gemma4 function currently starts with:
# lines.push("🤖 Gemma4 Chat...")
# lines.push("")
# for (q, a) in ...
# ...loading check...
# ...more rendering...

# I'll just add a close brace before the final rendering. Let me find a good anchor.
# After the loading check, there's a frame.render_widget for the response area.
# Let me find: "Paragraph::new(lines).block(Block::default()... Gemma4 Response"

# Actually, the simplest fix: move the close-brace to just before the frame.render_widget call
old_render_close = 'frame.render_widget(\n        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" Gemma4 Response ")),'
new_render_close = '    }\n    frame.render_widget(\n        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" Gemma4 Response ")),'
c = c.replace(old_render_close, new_render_close)

with open(path, 'w') as f:
    f.write(c)
print("OK: Gemma4 gated behind FLUX_G4_ENABLE=1")
