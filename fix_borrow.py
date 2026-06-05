path = "/home/storage/deepseek-codewhale/flux/crates/fluxmux/src/main.rs"
with open(path) as f:
    c = f.read()
old = '                    self.update_available = true;\n                    self.update_version = ver;\n                    self.log(&format!("🔔 Update available: v{}", ver));'
new = '                    self.update_available = true;\n                    self.update_version = ver.clone();\n                    self.log(&format!("🔔 Update available: v{}", self.update_version));'
c = c.replace(old, new)
with open(path, 'w') as f:
    f.write(c)
print("OK")
