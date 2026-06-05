//! ffmpeg orchestration: cinematic filtergraph builder + render runner.
//!
//! Filter chain we assemble for `render`:
//!   [0:v] -> scale=1920:1080
//!         -> (optional) zoompan ken-burns
//!         -> (optional) vignette
//!         -> drawtext title card (fades in 0–4s)
//!         -> drawtext subtitle (lower-third, 0.8–4s)
//!         -> drawtext clock HUD (top-right, persistent)
//!         -> drawtext tool-call cards (one per event, ~3s each, fade in/out)
//!         -> ass=captions.ass (karaoke captions, burned-in)

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

use crate::overlay;
use crate::transcript::Event;

pub struct RenderOpts {
    pub title: String,
    pub subtitle: String,
    pub kenburns: bool,
    pub vignette: bool,
    /// ffmpeg x264 preset: ultrafast (default, ~5× faster) ... slow (final quality).
    pub preset: String,
    /// CRF — 18 = visually lossless, 23 = default, 28 = small but soft.
    pub crf: u32,
}

impl Default for RenderOpts {
    fn default() -> Self {
        Self {
            title: "Claude Code Session".into(),
            subtitle: "Claude Opus 4.7".into(),
            kenburns: true,
            vignette: true,
            preset: "ultrafast".into(),
            crf: 23,
        }
    }
}

/// Grab a single frame at `at_seconds`, scale to 1920x1080, then overlay a
/// big title + a subtle gradient bar at the bottom. Saves as PNG suitable for
/// a YouTube cover (≥ 1280x720, < 2 MB JPEG/PNG).
pub fn thumbnail(raw: &Path, at_seconds: f64, title: &str, subtitle: &str, out: &Path) -> Result<()> {
    let title = esc(title);
    let subtitle = esc(subtitle);
    let filter = format!(
        "scale=1920:1080:force_original_aspect_ratio=decrease,\
         pad=1920:1080:(ow-iw)/2:(oh-ih)/2:color=0x0c0e14,\
         vignette=PI/4,\
         eq=contrast=1.08:saturation=1.12:gamma=0.96,\
         drawbox=x=0:y=ih-260:w=iw:h=260:color=0x0b1020@0.78:t=fill,\
         drawtext=text='{title}':fontsize=104:fontcolor=white:\
             x=80:y=h-220:box=1:boxcolor=0x4f46e5@0.0:boxborderw=0,\
         drawtext=text='{subtitle}':fontsize=44:fontcolor=0xc7d1ff:\
             x=80:y=h-90,\
         drawtext=text='flux-record':fontsize=30:fontcolor=0x9aa9ff:\
             x=w-tw-60:y=h-70:alpha=0.85"
    );

    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-hide_banner",
            "-loglevel", "warning",
            "-ss", &format!("{:.3}", at_seconds),
            "-i", &raw.display().to_string(),
            "-frames:v", "1",
            "-update", "1",
            "-vf", &filter,
            &out.display().to_string(),
        ])
        .status()
        .context("failed to spawn ffmpeg for thumbnail")?;

    if !status.success() {
        anyhow::bail!("ffmpeg thumbnail exited with {status}");
    }
    Ok(())
}

pub fn render(raw: &Path, events: &[Event], out: &Path, opts: &RenderOpts) -> Result<()> {
    let filter = build_filtergraph(events, opts);

    // Use -filter_complex_script (filtergraph from a file) instead of
    // -filter_complex (filtergraph from a CLI arg). The script parser does
    // NOT split on raw commas inside option values, so expressions like
    // alpha='if(lt(t,3),0,...)' survive intact. The -filter_complex parser
    // would otherwise treat each comma as a filter separator and try to
    // instantiate filters named "3", "0", etc.
    let script = write_filter_script(&filter)?;

    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-hide_banner",
            "-loglevel", "warning",
            "-i", &raw.display().to_string(),
            "-filter_complex_script", &script.display().to_string(),
            "-map", "[vout]",
            "-map", "0:a?",
            "-c:v", "libx264",
            "-preset", &opts.preset,
            "-crf", &opts.crf.to_string(),
            "-pix_fmt", "yuv420p",
            "-c:a", "aac",
            "-b:a", "192k",
            "-movflags", "+faststart",
            &out.display().to_string(),
        ])
        .status()
        .context("failed to spawn ffmpeg for render")?;

    // Cleanup only on success — leave the script behind on failure for debug.
    if status.success() {
        let _ = std::fs::remove_file(&script);
    } else {
        eprintln!("flux-record: filter script preserved at {} for debug", script.display());
    }

    if !status.success() {
        anyhow::bail!("ffmpeg render exited with {status}");
    }
    Ok(())
}

/// Write the filtergraph to a tempfile so we can pass it via
/// -filter_complex_script. Returns the path; caller deletes when done.
fn write_filter_script(filter: &str) -> Result<std::path::PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "flux-record-filter-{}.txt",
        std::process::id()
    ));
    std::fs::write(&path, filter).context("writing filtergraph script")?;
    Ok(path)
}

fn build_filtergraph(events: &[Event], opts: &RenderOpts) -> String {
    let mut chain = String::from("[0:v]scale=1920:1080:force_original_aspect_ratio=decrease,pad=1920:1080:(ow-iw)/2:(oh-ih)/2:color=0x0c0e14");

    if opts.kenburns {
        // Gentle 1.0 -> 1.06 zoom over 25s — subtle ken-burns. Commas inside
        // min() must be backslash-escaped because the filtergraph parser uses
        // commas as filter separators even inside single-quoted strings.
        chain.push_str(
            ",zoompan=z='min(1.0+0.0024*on\\,1.06)':d=1:s=1920x1080:fps=30",
        );
    }

    if opts.vignette {
        chain.push_str(",vignette=PI/5");
    }

    // Subtle filmic grade (gamma + slight cool shadows).
    chain.push_str(",eq=contrast=1.04:saturation=1.06:gamma=0.98");

    // Title card: big, fades in 0–1s, holds, fades out by 4s.
    chain.push_str(&drawtext_title(&opts.title, &opts.subtitle));

    // Persistent HUD (top-right): elapsed clock.
    chain.push_str(&drawtext_clock_hud());

    // Tool-call lower-third cards from event stream.
    let cards = overlay::build_cards(events);
    for c in &cards {
        chain.push_str(&drawtext_card(c));
    }

    // Watermark in bottom-right corner (always visible).
    chain.push_str(&drawtext_watermark());

    chain.push_str("[vout]");
    chain
}

fn esc(s: &str) -> String {
    // Escape characters that break ffmpeg's drawtext parser.
    //
    // ASCII apostrophe (') gets converted to U+2019 (right single quotation
    // mark) rather than backslash-escaped. Inside a single-quoted option
    // value, ffmpeg's parser treats `\'` as ending the quote (it has no
    // in-quote escape sequence), which then breaks every subsequent quoted
    // option in the filter chain. The typographic apostrophe is visually
    // identical and bypasses the issue.
    s.replace('\\', "\\\\")
        .replace('\'', "\u{2019}")
        .replace(':', "\\:")
        .replace('%', "\\%")
        .replace(',', "\\,")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('=', "\\=")
        .replace(';', "\\;")
}

/// Backslash-escape only the commas in an ffmpeg expression. Single quotes
/// do NOT protect commas inside filtergraph option values — the chain parser
/// will treat each unescaped comma as a filter separator no matter the quotes.
fn esce(s: &str) -> String {
    s.replace(',', "\\,")
}

fn drawtext_title(title: &str, subtitle: &str) -> String {
    let t = esc(title);
    let s = esc(subtitle);
    // alpha curve: 0 -> 1 over (0..1)s, hold to 3s, 1 -> 0 over (3..4)s.
    let alpha = esce("if(lt(t,1),t,if(lt(t,3),1,if(lt(t,4),4-t,0)))");
    format!(
        ",drawtext=text='{t}':fontsize=88:fontcolor=white:x=(w-tw)/2:y=h/2-100:\
         box=1:boxcolor=0x0b1020@0.55:boxborderw=24:alpha='{alpha}'\
         ,drawtext=text='{s}':fontsize=36:fontcolor=0xc7d1ff:x=(w-tw)/2:y=h/2+10:\
         box=1:boxcolor=0x0b1020@0.55:boxborderw=14:alpha='{alpha}'"
    )
}

fn drawtext_clock_hud() -> String {
    // pts in seconds -> HH:MM:SS via gmtime(). The %{pts:hms} expansion is
    // drawtext's built-in; the inner colon is already escaped as \:
    ",drawtext=text='%{pts\\:hms}':fontsize=28:fontcolor=0xb8c7ff:\
     x=w-tw-32:y=28:box=1:boxcolor=0x0b1020@0.55:boxborderw=12"
        .to_string()
}

fn drawtext_watermark() -> String {
    ",drawtext=text='flux-record':fontsize=22:fontcolor=0x6a7ab8:\
     x=w-tw-32:y=h-th-28:alpha=0.7"
        .to_string()
}

fn drawtext_card(c: &overlay::Card) -> String {
    let label = esc(&c.label);
    let badge = esc(&c.badge);
    let color = c.color;
    let start = c.start_s;
    let end = c.start_s + c.dur_s;

    // Fade in 0.2s, fade out 0.4s. Commas inside if()/between() must be
    // backslash-escaped — single quotes don't preserve commas at the
    // filtergraph level.
    let alpha = esce(&format!(
        "if(lt(t,{start}),0,if(lt(t,{start}+0.2),(t-{start})/0.2,if(lt(t,{end}-0.4),1,if(lt(t,{end}),({end}-t)/0.4,0))))"
    ));
    let enable = esce(&format!("between(t,{start},{end})"));

    // Single-quotes terminate the option value (so the chain-separator comma
    // is recognized); backslash-escaped commas inside survive intact so the
    // expression keeps its semantics.
    format!(
        ",drawtext=text='{badge}':fontsize=34:fontcolor=white:\
         x=80:y=h-208:box=1:boxcolor={color}@0.85:boxborderw=14:\
         enable='{enable}':alpha='{alpha}'\
         ,drawtext=text='{label}':fontsize=34:fontcolor=0xe6ecff:\
         x=320:y=h-208:box=1:boxcolor=0x0b1020@0.78:boxborderw=14:\
         enable='{enable}':alpha='{alpha}'"
    )
}
