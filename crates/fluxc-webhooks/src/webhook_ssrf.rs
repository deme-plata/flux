// fluxc webhook ssrf — Extracted SSRF guards for webhook URLs
//
// Split from webhook.rs as part of fluxc-core refactor (legacy_plan rank 14, low effort ~40min).
// This reduces god-file size and improves modularity for the 35% score crate.

use std::net::{IpAddr, ToSocketAddrs};

/// SEC-006 (SSRF guard): validate that a webhook URL is safe to dispatch to.
pub fn ssrf_check(url: &str) -> Result<(), String> {
    let allow_private = std::env::var("FLUX_WEBHOOK_ALLOW_PRIVATE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    ssrf_check_with(url, allow_private)
}

/// Pure SSRF policy — `allow_private` injected so it's testable without touching
/// the process-global `FLUX_WEBHOOK_ALLOW_PRIVATE` (which would race parallel tests).
pub fn ssrf_check_with(url: &str, allow_private: bool) -> Result<(), String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .ok_or_else(|| format!("webhook URL must be http(s): {url}"))?;
    // authority = up to the first '/', '?' or '#'; strip any userinfo before '@'
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    if authority.is_empty() {
        return Err(format!("webhook URL has no host: {url}"));
    }
    // host:port — handle bracketed IPv6 [::1]:port
    let (host, port): (String, u16) = if let Some(h) = authority.strip_prefix('[') {
        let (h6, tail) = h.split_once(']').ok_or_else(|| format!("bad IPv6 authority: {authority}"))?;
        let port = tail.strip_prefix(':').and_then(|p| p.parse().ok()).unwrap_or(443);
        (h6.to_string(), port)
    } else if let Some((h, p)) = authority.rsplit_once(':') {
        // only treat trailing ':NNN' as a port if it parses as a number
        match p.parse::<u16>() {
            Ok(port) => (h.to_string(), port),
            Err(_) => (authority.to_string(), 443),
        }
    } else {
        (authority.to_string(), 443)
    };

    let addrs: Vec<IpAddr> = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| format!("webhook host '{host}' does not resolve: {e}"))?
        .map(|sa| sa.ip())
        .collect();
    if addrs.is_empty() {
        return Err(format!("webhook host '{host}' resolved to no addresses"));
    }
    for ip in addrs {
        if is_metadata_ip(&ip) {
            return Err(format!("webhook target {ip} is the cloud metadata endpoint — refused"));
        }
        if !allow_private && is_private_ip(&ip) {
            return Err(format!(
                "webhook target {ip} is loopback/private/link-local — refused \
                 (set FLUX_WEBHOOK_ALLOW_PRIVATE=1 to allow same-host webhooks)"
            ));
        }
    }
    Ok(())
}

/// Dispatch-time guard for an endpoint that is ALREADY in the 0600 webhook
/// config (it passed `ssrf_check` at registration). A literal loopback IP
/// (`127.0.0.0/8`, `[::1]`) cannot DNS-rebind, which is what the dispatch
/// re-check exists to catch — so it is allowed here even when the current
/// process lacks FLUX_WEBHOOK_ALLOW_PRIVATE. Without this, a same-host
/// receiver (fluxc serve's /api/build_event) only ever heard from builds the
/// MCP server spawned, and silently nothing from a `fluxc build` in a shell.
/// Hostnames (even `localhost`), private ranges and metadata stay checked.
pub fn ssrf_check_dispatch(url: &str) -> Result<(), String> {
    if is_literal_loopback_url(url) {
        return Ok(());
    }
    ssrf_check(url)
}

fn is_literal_loopback_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("http://").or_else(|| url.strip_prefix("https://")) else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    if authority.contains('@') {
        return false; // userinfo games: fall back to the full check
    }
    let host = if let Some(h) = authority.strip_prefix('[') {
        h.split(']').next().unwrap_or("")
    } else {
        authority.rsplit_once(':').map(|(h, _)| h).unwrap_or(authority)
    };
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => v4.is_loopback(),
        Ok(IpAddr::V6(v6)) => v6.is_loopback(),
        Err(_) => false,
    }
}

/// The cloud-metadata SSRF target — blocked unconditionally.
pub fn is_metadata_ip(ip: &IpAddr) -> bool {
    matches!(ip, IpAddr::V4(v4) if v4.octets() == [169, 254, 169, 254])
}

/// Loopback / private / link-local / unspecified, v4 and v6.
pub fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified() || v4.is_broadcast()
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() {
                return true;
            }
            // IPv4-mapped (::ffff:a.b.c.d) — re-check as v4
            if let Some(v4) = v6.to_ipv4_mapped() {
                return v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified();
            }
            let seg = v6.segments();
            (seg[0] & 0xfe00) == 0xfc00 // fc00::/7 unique-local
                || (seg[0] & 0xffc0) == 0xfe80 // fe80::/10 link-local
        }
    }
}



#[cfg(test)]
mod dispatch_tests {
    use super::*;

    #[test]
    fn literal_loopback_passes_dispatch_but_nothing_else_does() {
        assert!(is_literal_loopback_url("http://127.0.0.1:8084/api/build_event"));
        assert!(is_literal_loopback_url("http://[::1]:8084/x"));
        assert!(!is_literal_loopback_url("http://localhost:8084/x"), "names can rebind");
        assert!(!is_literal_loopback_url("http://10.0.0.5/x"));
        assert!(!is_literal_loopback_url("http://169.254.169.254/latest"));
        assert!(!is_literal_loopback_url("http://evil.com@127.0.0.1/x"));
        assert!(!is_literal_loopback_url("file:///etc/passwd"));
        // metadata still refused at dispatch, regardless of env
        assert!(ssrf_check_dispatch("http://169.254.169.254/").is_err());
    }
}
