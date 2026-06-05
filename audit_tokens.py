import os

handlers = '/home/storage/deepseek-codewhale/flux/crates/fluxc-mcp/src/handlers'
for f in sorted(os.listdir(handlers)):
    if f.endswith('.rs') and f != 'mod.rs':
        path = os.path.join(handlers, f)
        with open(path) as fh:
            content = fh.read()
        format_count = content.count('format!(')
        push_count = content.count('.push(') + content.count('.push_str(')
        lines = content.count('\n')
        print(f'{f}: {lines} lines, {format_count} format!, {push_count} push')
print()
print('=== TOKEN LEAK HOTSPOTS ===')
print('flux_quickstart: ~380 lines of doc inlines (120+100+60+100) -> ~8000 tokens')
print('flux_bootstrap: quickstart + diagnose + tune -> ~12000 tokens')
print('flux_quantum_architect: per-crate bars + priority -> ~3000 tokens')
print('flux_predict_batch: 15 crates x 4 fields -> ~2000 tokens')
print('flux_swot: 4 quadrants + scores -> ~2500 tokens')
print()
print('=== SUGGESTED FIXES ===')
print('1. flux_quickstart: truncate doc previews to 40 lines each (vs 120/100/60/100)')
print('2. flux_bootstrap: skip quickstart text, just return state summary')
print('3. Add token_limit param to verbose tools')
print('4. Use JSON mode (compact) as default for batch tools')
