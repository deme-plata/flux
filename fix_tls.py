path = "/home/storage/deepseek-codewhale/flux/crates/fluxmux/Cargo.toml"
with open(path) as f:
    c = f.read()
c = c.replace(
    'reqwest = { version = "0.12", features = ["blocking"] }',
    'reqwest = { version = "0.12", features = ["blocking", "rustls-tls"], default-features = false }'
)
with open(path, 'w') as f:
    f.write(c)
print("OK: Added rustls-tls to reqwest")
