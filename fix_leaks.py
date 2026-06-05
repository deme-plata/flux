# Fix token leaks in flux_quickstart: reduce doc preview lines
import sys
path = "/home/storage/deepseek-codewhale/flux/crates/fluxc-mcp/src/handlers/session.rs"
with open(path) as f:
    content = f.read()
orig = content

# Reduce doc previews: 120→40, 100→40, 60→30, 100→40
content = content.replace(
    "let instructions_preview: String = instructions.lines().take(120).collect::<Vec<_>>().join(\"\\n\");",
    "let instructions_preview: String = instructions.lines().take(40).collect::<Vec<_>>().join(\"\\n\");"
)
content = content.replace(
    "let handoff_preview: String = handoff.lines().take(100).collect::<Vec<_>>().join(\"\\n\");",
    "let handoff_preview: String = handoff.lines().take(40).collect::<Vec<_>>().join(\"\\n\");"
)
content = content.replace(
    "let agents_preview: String = agents.lines().take(60).collect::<Vec<_>>().join(\"\\n\");",
    "let agents_preview: String = agents.lines().take(30).collect::<Vec<_>>().join(\"\\n\");"
)
content = content.replace(
    "let ai_rules_preview: String = ai_rules.lines().take(100).collect::<Vec<_>>().join(\"\\n\");",
    "let ai_rules_preview: String = ai_rules.lines().take(40).collect::<Vec<_>>().join(\"\\n\");"
)

# Also add token savings note to quickstart output
content = content.replace(
    'qs.push("⚡ Ready. Full docs loaded. Run flux_diagnose or flux_fullcheck to verify state.".to_string());',
    'qs.push("⚡ Ready. Docs loaded (compact preview). For full docs use read_file. Run flux_diagnose or flux_architect_predict.".to_string());'
)

if content == orig:
    print("ERROR: No changes")
    sys.exit(1)
with open(path, 'w') as f:
    f.write(content)
print("OK: Reduced flux_quickstart doc previews ~380→~150 lines (~60% token savings)")
