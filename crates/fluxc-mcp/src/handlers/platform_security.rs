//! Shared validation for platform MCP combos (DeepSeek security step 4).

/// Sanitize search queries — no shell metacharacters.
pub fn sanitize_search_query(q: &str) -> Result<String, String> {
    let q = q.trim();
    if q.is_empty() {
        return Err("query must not be empty".into());
    }
    if q.len() > 1024 {
        return Err("query exceeds 1024 char cap".into());
    }
    if !q
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || " ._-:/".contains(c))
    {
        return Err("query contains disallowed characters".into());
    }
    Ok(q.to_string())
}

/// Validate 64-char hex content_root (32 bytes).
pub fn validate_content_root_hex(h: &str) -> Result<[u8; 32], String> {
    let h = h.trim().trim_start_matches("0x");
    if h.len() != 64 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("content_root must be 64 hex chars (32 bytes)".into());
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = u8::from_str_radix(&h[i * 2..i * 2 + 2], 16)
            .map_err(|_| "invalid hex in content_root".to_string())?;
    }
    Ok(out)
}

/// Decode base64 with byte cap.
pub fn decode_base64_capped(b64: &str, cap: usize) -> Result<Vec<u8>, String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|e| format!("base64 decode failed: {e}"))?;
    if bytes.len() > cap {
        return Err(format!("payload exceeds {cap} byte cap (got {})", bytes.len()));
    }
    Ok(bytes)
}

/// Fleet node alias — alphanumeric + hyphen only.
pub fn validate_node_name(n: &str) -> Result<(), String> {
    if n.is_empty() || n.len() > 32 {
        return Err("invalid node name length".into());
    }
    if !n
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("node name contains disallowed characters".into());
    }
    Ok(())
}

/// Reject unknown JSON fields at top level (strict schema).
pub fn reject_unknown_fields(args: &serde_json::Value, allowed: &[&str]) -> Result<(), String> {
    let Some(obj) = args.as_object() else {
        return Ok(());
    };
    for k in obj.keys() {
        if !allowed.contains(&k.as_str()) {
            return Err(format!("unknown field '{k}'"));
        }
    }
    Ok(())
}