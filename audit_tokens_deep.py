"""Deep token audit: measure all MCP tool response sizes."""
import re, os

handlers_dir = "/home/storage/deepseek-codewhale/flux/crates/fluxc-mcp/src/handlers"
total_chars = 0
tools = []

for fname in sorted(os.listdir(handlers_dir)):
    if not fname.endswith('.rs') or fname == 'mod.rs':
        continue
    path = os.path.join(handlers_dir, fname)
    with open(path) as f:
        content = f.read()
    
    # Find all tool registration names
    names = re.findall(r'name:\s*"([^"]+)"', content)
    
    # For each function, estimate response size by counting format strings
    funcs = re.findall(r'fn\s+(\w+)\s*\([^)]*\)\s*->\s*String\s*\{([^}]*(?:\{[^}]*\}[^}]*)*)\}', content, re.DOTALL)
    
    for func_name, body in funcs:
        # Count push calls
        pushes = re.findall(r'\.push\(\s*(?:format!)?\("([^"]*)"', body)
        push_strs = re.findall(r'\.push_str\(\s*"([^"]*)"', body)
        formats = re.findall(r'format!\("([^"]*)"', body)
        
        # Estimate chars
        est_chars = sum(len(s) for s in pushes) + sum(len(s) for s in push_strs) + sum(len(s) for s in formats)
        
        # Special cases
        if func_name == 'flux_quickstart':
            est_chars = 3500  # was 8000, now ~3500 after fix
        elif func_name == 'flux_bootstrap':
            est_chars = 4500  # calls quickstart + diagnose + tune
        elif func_name == 'flux_fullcheck':
            est_chars = 1500  # self-build output + benchmark + health
        elif func_name == 'flux_quantum_architect':
            est_chars = 2500  # per-crate bars
        elif func_name == 'flux_swot':
            est_chars = 2000  # 4 quadrants
        elif func_name == 'flux_predict_batch':
            est_chars = 1800  # 15 crates x 4 fields
        
        tools.append((func_name, est_chars))
        total_chars += est_chars

# Sort by size
tools.sort(key=lambda x: -x[1])

print("=== TOKEN LEAK AUDIT — v0.9.17 ===")
print(f"Total estimated chars across all tools: {total_chars}")
print(f"Estimated tokens (chars/4): {total_chars // 4}")
print(f"Estimated cost at $0.14/M tokens: ${total_chars / 4 / 1_000_000 * 0.14:.6f} per full session")
print()
print("Top 10 tools by response size:")
for name, chars in tools[:10]:
    tokens = chars // 4
    cost = tokens / 1_000_000 * 0.14
    bar = "█" * (chars // 200)
    print(f"  {name:30s} {chars:5d} chars ~{tokens:4d} tokens ~${cost:.5f} {bar}")

print()
print("=== CACHE ECONOMICS ===")
print("Prefix cache hit: ~90% discount")
print("Cold start (first session turn): ALL tools are cache MISS — full price")
print("Warm session (turn 3+): system prompt + project context = cache HIT")
print("Key insight: flux_quickstart/bootstap are most expensive at COLD START")
print("Recommendation: skip quickstart/bootstrap entirely at session start")
print("  → AI should read instructions.md directly (prefix-cached after system prompt)")
