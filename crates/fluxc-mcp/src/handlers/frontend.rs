//! flux_ui_* — frontend / static-UI management tools.
//!
//! Why these exist: q-flux serves `dist-final/*.html` with a hardcoded
//! `cache-control: max-age=86400`. There's no config knob and no
//! cache-flush admin endpoint, so editing a file and reloading the bare URL
//! shows the OLD page for up to 24h. That made every desktop/UI iteration
//! painful — you'd edit, deploy, and not see the change.
//!
//! These tools fix the workflow without touching the q-flux binary:
//! `flux_ui_deploy` writes the file AND hands back a `?v=<epoch>`
//! cache-busted URL that the browser has never seen, so the change is
//! always visible immediately. `flux_ui_preview` mints a fresh URL on
//! demand; `flux_ui_list` shows every surface with size + mtime;
//! `flux_ui_read` reads one back.
//!
//! Root is `$FLUX_UI_ROOT` or the Epsilon production static root by
//! default. All paths are sandboxed to the root — no `..`, no absolute
//! escapes.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::handlers::{ToolDef, ToolRegistry};

/// Default static root — Epsilon's q-flux `[static_files].root`. Override
/// with `FLUX_UI_ROOT` when the MCP server runs elsewhere.
fn ui_root() -> PathBuf {
    std::env::var("FLUX_UI_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/home/orobit/q-narwhalknight/dist-final"))
}

/// Public origin for built URLs. Override with `FLUX_UI_ORIGIN`.
fn ui_origin() -> String {
    std::env::var("FLUX_UI_ORIGIN").unwrap_or_else(|_| "https://quillon.xyz".to_string())
}

/// Resolve `name` inside the root, rejecting traversal + absolute paths.
fn safe_path(name: &str) -> Result<PathBuf, String> {
    let name = name.trim().trim_start_matches('/');
    if name.is_empty() {
        return Err("empty file name".into());
    }
    if name.contains("..") {
        return Err(format!("path traversal rejected: {name:?}"));
    }
    let p = ui_root().join(name);
    // Final containment check on the lexical path.
    if !p.starts_with(ui_root()) {
        return Err(format!("path escapes root: {name:?}"));
    }
    Ok(p)
}

fn epoch() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_ui_list",
            description: "List every static UI surface in the q-flux dist root (HTML/CSS/JS) with byte size, last-modified, and its public + cache-busted URL. Use to see what's deployed before editing.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "ext": {"type": "string", "description": "Filter by extension, e.g. 'html' (default: html,css,js)"}
                }
            }),
        },
        flux_ui_list,
    );
    registry.register(
        ToolDef {
            name: "flux_ui_deploy",
            description: "Write a static UI file into the q-flux dist root and return a CACHE-BUSTED url that bypasses the 24h browser cache, so the change is visible immediately. This is the fix for 'I edited the page but the browser shows the old version'. Sandboxed to the static root.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file": {"type": "string", "description": "File name (e.g. 'desktop.html'). No leading slash, no '..'."},
                    "content": {"type": "string", "description": "Full file content to write (overwrites)."}
                },
                "required": ["file", "content"]
            }),
        },
        flux_ui_deploy,
    );
    registry.register(
        ToolDef {
            name: "flux_ui_preview",
            description: "Mint a fresh cache-busted URL for an already-deployed file (appends ?v=<epoch>). Hand this to a human so they always load the latest version, never a stale cached copy.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file": {"type": "string", "description": "File name to preview (e.g. 'ledger.html')."}
                },
                "required": ["file"]
            }),
        },
        flux_ui_preview,
    );
    registry.register(
        ToolDef {
            name: "flux_ui_read",
            description: "Read a static UI file from the q-flux dist root back into context (e.g. to diff or patch it). Sandboxed to the root.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file": {"type": "string", "description": "File name to read."},
                    "max_bytes": {"type": "integer", "description": "Truncate to this many bytes (default 200000)."}
                },
                "required": ["file"]
            }),
        },
        flux_ui_read,
    );
    registry.register(
        ToolDef {
            name: "flux_launcher_register",
            description: "Wire a new app/surface into desktop.html's launcher in 3 places atomically (APPS registry + quick-launch tile + dock item). Idempotent — re-registering with the same appId updates the existing entry instead of duplicating. Returns the cache-busted URL of the updated desktop.html. Replaces the 5+ Bash invocations the manual path required.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "appId":  {"type": "string", "description": "Short identifier, alphanumeric + dash/underscore (e.g. 'ide', 'verify')."},
                    "title":  {"type": "string", "description": "Window title (e.g. 'Flux Pro IDE — Atelier')."},
                    "icon":   {"type": "string", "description": "Emoji or symbol shown in launcher (e.g. '💻')."},
                    "src":    {"type": "string", "description": "Iframe URL (e.g. '/ide.html')."},
                    "w":      {"type": "integer", "description": "Window width in px, default 1100"},
                    "h":      {"type": "integer", "description": "Window height in px, default 700"},
                    "name":   {"type": "string", "description": "Short display name on the quick-launch tile (default: title up to first ' — ')"},
                    "desc":   {"type": "string", "description": "Tile description line (default empty)"},
                    "add_to_dock":         {"type": "boolean", "description": "Add icon to dock (default true)"},
                    "add_to_quick_launch": {"type": "boolean", "description": "Add tile to quick-launch widget (default true)"}
                },
                "required": ["appId", "title", "icon", "src"]
            }),
        },
        flux_launcher_register,
    );
}

fn flux_ui_list(args: &Value) -> String {
    let root = ui_root();
    let origin = ui_origin();
    let filt = arg_str(args, "ext").unwrap_or("html,css,js");
    let exts: Vec<&str> = filt.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    let rd = match std::fs::read_dir(&root) {
        Ok(r) => r,
        Err(e) => return format!("✗ cannot read UI root {}: {}", root.display(), e),
    };
    let mut rows: Vec<(String, u64, u64)> = Vec::new();
    for entry in rd.flatten() {
        let p = entry.path();
        if !p.is_file() { continue; }
        let name = match p.file_name().and_then(|n| n.to_str()) { Some(n) => n.to_string(), None => continue };
        let ext_ok = exts.iter().any(|e| name.ends_with(&format!(".{e}")));
        if !ext_ok { continue; }
        let meta = match entry.metadata() { Ok(m) => m, Err(_) => continue };
        let size = meta.len();
        let mtime = meta.modified().ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs()).unwrap_or(0);
        rows.push((name, size, mtime));
    }
    rows.sort_by(|a, b| b.2.cmp(&a.2)); // newest first
    if rows.is_empty() {
        return format!("(no matching files in {})", root.display());
    }
    let now = epoch();
    let mut out = format!("🖥  {} UI surface(s) in {}\n\n", rows.len(), root.display());
    for (name, size, mtime) in rows {
        let age = now.saturating_sub(mtime);
        let age_s = if age < 90 { format!("{age}s ago") }
            else if age < 5400 { format!("{}m ago", age / 60) }
            else if age < 129600 { format!("{}h ago", age / 3600) }
            else { format!("{}d ago", age / 86400) };
        out.push_str(&format!(
            "  {name:<26} {size:>7}B  {age_s:<9}  {origin}/{name}\n"
        ));
    }
    out
}

fn flux_ui_deploy(args: &Value) -> String {
    let file = match arg_str(args, "file") { Some(f) => f, None => return "✗ missing 'file'".into() };
    let content = match arg_str(args, "content") { Some(c) => c, None => return "✗ missing 'content'".into() };
    let path = match safe_path(file) { Ok(p) => p, Err(e) => return format!("✗ {e}") };

    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return format!("✗ mkdir {}: {}", parent.display(), e);
        }
    }
    if let Err(e) = std::fs::write(&path, content) {
        return format!("✗ write {}: {}", path.display(), e);
    }
    let v = epoch();
    let origin = ui_origin();
    let clean = file.trim_start_matches('/');
    format!(
        "✓ deployed {} ({} bytes)\n  cache-busted (open this — bypasses the 24h cache):\n    {origin}/{clean}?v={v}\n  canonical (cached up to 24h on repeat loads):\n    {origin}/{clean}",
        path.display(), content.len()
    )
}

fn flux_ui_preview(args: &Value) -> String {
    let file = match arg_str(args, "file") { Some(f) => f, None => return "✗ missing 'file'".into() };
    let path = match safe_path(file) { Ok(p) => p, Err(e) => return format!("✗ {e}") };
    if !path.exists() {
        return format!("✗ not deployed: {} (use flux_ui_deploy first)", path.display());
    }
    let v = epoch();
    let clean = file.trim_start_matches('/');
    format!("{}/{}?v={}", ui_origin(), clean, v)
}

fn flux_ui_read(args: &Value) -> String {
    let file = match arg_str(args, "file") { Some(f) => f, None => return "✗ missing 'file'".into() };
    let max = args.get("max_bytes").and_then(|v| v.as_u64()).unwrap_or(200_000) as usize;
    let path = match safe_path(file) { Ok(p) => p, Err(e) => return format!("✗ {e}") };
    match std::fs::read_to_string(&path) {
        Ok(s) => {
            if s.len() > max {
                format!("{}\n\n… (truncated at {} of {} bytes)", &s[..max], max, s.len())
            } else { s }
        }
        Err(e) => format!("✗ read {}: {}", path.display(), e),
    }
}

fn flux_launcher_register(args: &Value) -> String {
    let app_id = match arg_str(args, "appId") { Some(s) => s.to_string(), None => return "✗ missing 'appId'".into() };
    let title  = match arg_str(args, "title") { Some(s) => s.to_string(), None => return "✗ missing 'title'".into() };
    let icon   = match arg_str(args, "icon")  { Some(s) => s.to_string(), None => return "✗ missing 'icon'".into() };
    let src    = match arg_str(args, "src")   { Some(s) => s.to_string(), None => return "✗ missing 'src'".into() };
    let w = args.get("w").and_then(|v| v.as_u64()).unwrap_or(1100);
    let h = args.get("h").and_then(|v| v.as_u64()).unwrap_or(700);
    let name = arg_str(args, "name").map(String::from)
        .unwrap_or_else(|| title.split(" — ").next().unwrap_or(&title).to_string());
    let desc = arg_str(args, "desc").unwrap_or("").to_string();
    let add_to_dock  = args.get("add_to_dock").and_then(|v| v.as_bool()).unwrap_or(true);
    let add_to_quick = args.get("add_to_quick_launch").and_then(|v| v.as_bool()).unwrap_or(true);

    if !app_id.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') || app_id.is_empty() {
        return format!("✗ invalid appId {app_id:?} — alphanumeric + dash/underscore only");
    }
    // Defend against quote injection in JS string literals
    for (k, v) in [("title", &title), ("icon", &icon), ("src", &src), ("name", &name), ("desc", &desc)] {
        if v.contains('\'') || v.contains('\\') || v.contains('\n') {
            return format!("✗ {k} contains forbidden char (quote/backslash/newline)");
        }
    }

    let path = match safe_path("desktop.html") { Ok(p) => p, Err(e) => return format!("✗ {e}") };
    let mut html = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => return format!("✗ read {}: {}", path.display(), e),
    };

    let mut steps: Vec<&'static str> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    // (1) APPS registry — upsert
    let new_apps_entry = format!(
        "  {app_id}: {{ title: '{title}', icon: '{icon}', kind: 'iframe', src: '{src}', w: {w}, h: {h} }},"
    );
    match upsert_apps_entry(&html, &app_id, &new_apps_entry) {
        Ok((updated, was_update)) => {
            html = updated;
            steps.push(if was_update { "APPS updated" } else { "APPS inserted" });
        }
        Err(e) => errors.push(format!("APPS: {e}")),
    }

    // (2) Quick-launch tile — upsert
    if add_to_quick {
        let tile = format!(
            "      <button class=\"launch-tile\" onclick=\"launch('{app_id}')\">\n        <div class=\"ic\">{icon}</div>\n        <div class=\"name\">{name}</div>\n        <div class=\"desc\">{desc}</div>\n      </button>"
        );
        match upsert_launch_tile(&html, &app_id, &tile) {
            Ok((updated, was_update)) => {
                html = updated;
                steps.push(if was_update { "tile updated" } else { "tile inserted" });
            }
            Err(e) => errors.push(format!("tile: {e}")),
        }
    } else {
        steps.push("tile skipped");
    }

    // (3) Dock item — upsert
    if add_to_dock {
        let dock = format!(
            "  <div class=\"dock-item\" onclick=\"launch('{app_id}')\"><span>{icon}</span><span class=\"tip\">{name}</span></div>"
        );
        match upsert_dock_item(&html, &app_id, &dock) {
            Ok((updated, was_update)) => {
                html = updated;
                steps.push(if was_update { "dock updated" } else { "dock inserted" });
            }
            Err(e) => errors.push(format!("dock: {e}")),
        }
    } else {
        steps.push("dock skipped");
    }

    if !errors.is_empty() {
        return format!("✗ launcher_register failed: {}", errors.join("; "));
    }

    if let Err(e) = std::fs::write(&path, &html) {
        return format!("✗ write {}: {}", path.display(), e);
    }
    let v = epoch();
    let origin = ui_origin();
    format!(
        "✓ launcher '{app_id}' registered\n  steps: {}\n  open this URL (cache-busted, defeats 24h cache):\n    {origin}/desktop.html?v={v}",
        steps.join(" · ")
    )
}

/// Idempotent upsert of an APPS-registry entry. Searches the `const APPS = {` block
/// for an existing `<appId>:` line and replaces it, otherwise inserts before the `};`.
/// Returns (updated_html, was_existing_entry).
fn upsert_apps_entry(html: &str, app_id: &str, new_entry: &str) -> Result<(String, bool), String> {
    let start = html.find("const APPS = {").ok_or("const APPS block not found")?;
    let after_start = start + "const APPS = {".len();
    let rel_end = html[after_start..].find("};").ok_or("APPS closing }; not found")?;
    let end = after_start + rel_end;

    let line_marker = format!("\n  {app_id}:");
    if let Some(line_start_rel) = html[after_start..end].find(&line_marker) {
        // Replace the existing line
        let line_start = after_start + line_start_rel + 1; // skip leading \n
        let line_end_rel = html[line_start..].find('\n').ok_or("APPS entry line end not found")?;
        let line_end = line_start + line_end_rel;
        let mut out = String::with_capacity(html.len() + new_entry.len());
        out.push_str(&html[..line_start]);
        out.push_str(new_entry);
        out.push_str(&html[line_end..]);
        Ok((out, true))
    } else {
        // Insert before the closing `};`
        let mut out = String::with_capacity(html.len() + new_entry.len() + 2);
        out.push_str(&html[..end]);
        if !html[..end].ends_with('\n') {
            out.push('\n');
        }
        out.push_str(new_entry);
        out.push('\n');
        out.push_str(&html[end..]);
        Ok((out, false))
    }
}

/// Idempotent upsert of a quick-launch tile inside `class="launch-grid"`.
/// Returns (updated_html, was_existing_entry).
fn upsert_launch_tile(html: &str, app_id: &str, new_tile: &str) -> Result<(String, bool), String> {
    let grid_anchor = html.find("class=\"launch-grid\"").ok_or("launch-grid not found")?;
    let grid_open = html[grid_anchor..].find('>').map(|i| grid_anchor + i + 1)
        .ok_or("launch-grid open tag malformed")?;
    let close_rel = html[grid_open..].find("</div>\n  </div>")
        .or_else(|| html[grid_open..].find("</div>\n    </div>"))
        .ok_or("launch-grid closing </div></div> not found")?;
    let grid_close = grid_open + close_rel;

    let marker = format!("onclick=\"launch('{app_id}')\"");
    if let Some(rel) = html[grid_open..grid_close].find(&marker) {
        let abs = grid_open + rel;
        let btn_start = html[..abs].rfind("<button").ok_or("<button> for existing tile not found")?;
        let btn_close_rel = html[abs..].find("</button>").ok_or("</button> for existing tile not found")?;
        let btn_end = abs + btn_close_rel + "</button>".len();
        let mut out = String::with_capacity(html.len() + new_tile.len());
        out.push_str(&html[..btn_start]);
        out.push_str(new_tile.trim_start_matches(' '));
        out.push_str(&html[btn_end..]);
        Ok((out, true))
    } else {
        let mut out = String::with_capacity(html.len() + new_tile.len() + 2);
        out.push_str(&html[..grid_close]);
        if !html[..grid_close].ends_with('\n') {
            out.push('\n');
        }
        out.push_str(new_tile);
        out.push('\n');
        out.push_str(&html[grid_close..]);
        Ok((out, false))
    }
}

/// Idempotent upsert of a dock item inside `<div id="dock">`. Inserts before
/// the first `<div class="dock-sep">` (so new items appear in the primary
/// section), or before the closing `</div>` if no separator present.
fn upsert_dock_item(html: &str, app_id: &str, new_item: &str) -> Result<(String, bool), String> {
    let dock_open = html.find("<div id=\"dock\">").ok_or("<div id=\"dock\"> not found")?;
    let dock_inner = dock_open + "<div id=\"dock\">".len();
    let close_rel = html[dock_inner..].find("\n</div>").ok_or("dock closing </div> not found")?;
    let dock_close = dock_inner + close_rel;

    let marker = format!("onclick=\"launch('{app_id}')\"");
    if let Some(rel) = html[dock_inner..dock_close].find(&marker) {
        let abs = dock_inner + rel;
        let item_start = html[..abs].rfind("<div class=\"dock-item\"")
            .ok_or("existing dock-item <div> not found")?;
        let item_close_rel = html[abs..].find("</div>").ok_or("existing dock-item </div> not found")?;
        let item_end = abs + item_close_rel + "</div>".len();
        let mut out = String::with_capacity(html.len() + new_item.len());
        out.push_str(&html[..item_start]);
        out.push_str(new_item.trim_start_matches(' '));
        out.push_str(&html[item_end..]);
        Ok((out, true))
    } else {
        let insert_pos = html[dock_inner..dock_close]
            .find("<div class=\"dock-sep\">")
            .map(|i| dock_inner + i)
            .unwrap_or(dock_close);
        // The dock-sep line typically has leading whitespace; find the actual
        // newline before it so our insertion lines up.
        let line_start = html[..insert_pos].rfind('\n').map(|i| i + 1).unwrap_or(insert_pos);
        let mut out = String::with_capacity(html.len() + new_item.len() + 2);
        out.push_str(&html[..line_start]);
        out.push_str(new_item.trim_start_matches(' '));
        out.push('\n');
        out.push_str(&html[line_start..]);
        Ok((out, false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_path_rejects_traversal() {
        assert!(safe_path("../etc/passwd").is_err());
        assert!(safe_path("a/../../x").is_err());
        assert!(safe_path("").is_err());
    }

    #[test]
    fn safe_path_accepts_simple_names() {
        assert!(safe_path("desktop.html").is_ok());
        assert!(safe_path("/ledger.html").is_ok()); // leading slash stripped
    }

    #[test]
    fn deploy_requires_args() {
        assert!(flux_ui_deploy(&json!({})).starts_with("✗"));
        assert!(flux_ui_deploy(&json!({"file":"x.html"})).starts_with("✗"));
    }

    // ── flux_launcher_register tests (state-free, pure string ops on a synthetic html)

    fn fake_desktop() -> String {
        r#"<html><head></head><body>
<div id="w-launch">
<h3>quick launch</h3>
<div class="launch-grid">
      <button class="launch-tile" onclick="launch('terminal')">
        <div class="ic">⌨️</div>
        <div class="name">Terminal</div>
        <div class="desc">shell</div>
      </button>
    </div>
  </div>
<div id="dock">
  <div class="dock-item" onclick="launch('terminal')"><span>⌨️</span><span class="tip">Terminal</span></div>
  <div class="dock-sep"></div>
  <div class="dock-item" onclick="launch('about')"><span>ℹ️</span><span class="tip">About</span></div>
</div>
<script>
const APPS = {
  terminal: { title: 'Terminal', icon: '⌨️', kind: 'shell', w: 760, h: 480 },
  about:    { title: 'About', icon: 'ℹ️', kind: 'about', w: 460, h: 380 },
};
</script>
</body></html>"#.to_string()
    }

    #[test]
    fn apps_inserts_new_entry() {
        let (h, was_update) = upsert_apps_entry(&fake_desktop(), "ide", "  ide: { title: 'IDE', icon: '💻', kind: 'iframe', src: '/ide.html', w: 1280, h: 820 },").unwrap();
        assert!(!was_update);
        assert!(h.contains("ide: { title: 'IDE'"));
        assert!(h.contains("terminal: { title: 'Terminal'"));
    }

    #[test]
    fn apps_updates_existing_entry() {
        let (h, was_update) = upsert_apps_entry(&fake_desktop(), "terminal", "  terminal: { title: 'Terminal V2', icon: '⌨️', kind: 'shell', w: 800, h: 500 },").unwrap();
        assert!(was_update);
        assert!(h.contains("Terminal V2"));
        assert!(!h.contains("w: 760")); // old width gone
    }

    #[test]
    fn tile_inserts_then_updates_idempotently() {
        let (h1, w1) = upsert_launch_tile(&fake_desktop(), "ide",
            "      <button class=\"launch-tile\" onclick=\"launch('ide')\">\n        <div class=\"ic\">💻</div>\n        <div class=\"name\">IDE</div>\n        <div class=\"desc\">v0.18</div>\n      </button>").unwrap();
        assert!(!w1);
        assert!(h1.contains("launch('ide')"));
        let (h2, w2) = upsert_launch_tile(&h1, "ide",
            "      <button class=\"launch-tile\" onclick=\"launch('ide')\">\n        <div class=\"ic\">💻</div>\n        <div class=\"name\">IDE v2</div>\n        <div class=\"desc\">v0.19</div>\n      </button>").unwrap();
        assert!(w2);
        assert!(h2.contains("IDE v2"));
        // Only one IDE tile present
        assert_eq!(h2.matches("launch('ide')").count(), 1);
    }

    #[test]
    fn dock_inserts_before_sep() {
        let (h, _) = upsert_dock_item(&fake_desktop(), "ide",
            "  <div class=\"dock-item\" onclick=\"launch('ide')\"><span>💻</span><span class=\"tip\">IDE</span></div>").unwrap();
        // The new item should sit BEFORE the dock-sep, after the existing terminal item
        let ide_pos  = h.find("launch('ide')").unwrap();
        let sep_pos  = h.find("dock-sep").unwrap();
        let about_pos = h.find("launch('about')").unwrap();
        assert!(ide_pos < sep_pos);
        assert!(sep_pos < about_pos);
    }

    #[test]
    fn launcher_register_rejects_bad_app_id() {
        assert!(flux_launcher_register(&json!({
            "appId": "bad id with spaces",
            "title": "X", "icon": "💻", "src": "/x.html"
        })).starts_with("✗ invalid appId"));
    }

    #[test]
    fn launcher_register_rejects_quote_injection() {
        assert!(flux_launcher_register(&json!({
            "appId": "evil", "title": "X' + alert(1) + '", "icon": "💻", "src": "/x.html"
        })).starts_with("✗"));
    }
}
