//! Screen + audio capture wrapper.
//!
//! Spawns ffmpeg with x11grab + pulse, writes pidfile, returns child PID.
//! `stop` reads pidfile and sends SIGINT (so ffmpeg flushes mux trailer).

use anyhow::{anyhow, Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};

/// Dual-input capture: terminal X11 region + browser X11 region, composited
/// side-by-side with `hstack` and recorded to a single mkv. Coordinates are
/// given as `WxH+X+Y` strings (the same convention x11grab uses for `-i`).
pub fn start_dual(
    display: &str,
    term_geom: &str,
    browser_geom: &str,
    fps: u32,
    audio: &str,
    out: &Path,
    pidfile: &Path,
) -> Result<u32> {
    if pidfile.exists() {
        return Err(anyhow!(
            "pidfile {} already exists — stop the previous capture or remove it",
            pidfile.display()
        ));
    }

    // Split "WxH+X+Y" into video_size "WxH" and screen+offset "DISPLAY+X+Y".
    let (term_size, term_offset) = split_geom(term_geom)?;
    let (browser_size, browser_offset) = split_geom(browser_geom)?;

    // The filtergraph stacks the two inputs horizontally, drops a vertical
    // divider, and a small "TERMINAL" / "PREVIEW" label above each.
    let filter = format!(
        "[0:v]scale={term_w}:{h},drawtext=text='TERMINAL':fontsize=28:fontcolor=white:\
            x=24:y=18:box=1:boxcolor=0x111827@0.7:boxborderw=10[term];\
         [1:v]scale={browser_w}:{h},drawtext=text='PREVIEW':fontsize=28:fontcolor=white:\
            x=24:y=18:box=1:boxcolor=0x4f46e5@0.85:boxborderw=10[prev];\
         [term][prev]hstack=inputs=2,drawbox=x=iw/2-1:y=0:w=2:h=ih:color=0x4f46e5@0.6:t=fill[vout]",
        term_w = 960,
        browser_w = 960,
        h = 1080,
    );

    let mut cmd = Command::new("ffmpeg");
    cmd.args([
        "-y",
        "-hide_banner",
        "-loglevel", "warning",
        "-f", "x11grab",
        "-video_size", &term_size,
        "-framerate", &fps.to_string(),
        "-i", &format!("{display}{term_offset}"),
        "-f", "x11grab",
        "-video_size", &browser_size,
        "-framerate", &fps.to_string(),
        "-i", &format!("{display}{browser_offset}"),
    ]);

    if !audio.eq_ignore_ascii_case("none") {
        cmd.args(["-f", "pulse", "-i", audio]);
    }

    cmd.args([
        "-filter_complex", &filter,
        "-map", "[vout]",
    ]);
    if !audio.eq_ignore_ascii_case("none") {
        cmd.args(["-map", "2:a"]);
    }
    cmd.args([
        "-c:v", "libx264",
        "-preset", "veryfast",
        "-crf", "20",
        "-pix_fmt", "yuv420p",
    ]);
    if !audio.eq_ignore_ascii_case("none") {
        cmd.args(["-c:a", "aac", "-b:a", "192k"]);
    }
    cmd.arg(out.as_os_str());
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());

    let child = cmd.spawn().context(
        "failed to spawn ffmpeg for dual capture — is ffmpeg installed?",
    )?;
    let pid = child.id();
    std::mem::forget(child);
    std::fs::write(pidfile, pid.to_string())?;
    Ok(pid)
}

/// "1920x1080+0+0" -> ("1920x1080", "+0+0")
fn split_geom(g: &str) -> Result<(String, String)> {
    if let Some(plus) = g.find('+') {
        let (size, off) = g.split_at(plus);
        Ok((size.to_string(), off.to_string()))
    } else {
        Ok((g.to_string(), "+0+0".to_string()))
    }
}

pub fn start(
    display: &str,
    size: &str,
    fps: u32,
    audio: &str,
    out: &Path,
    pidfile: &Path,
) -> Result<u32> {
    if pidfile.exists() {
        return Err(anyhow!(
            "pidfile {} already exists — stop the previous capture or remove it",
            pidfile.display()
        ));
    }

    // ffmpeg -y -f x11grab -video_size W x H -framerate FPS -i :0.0
    //        -f pulse -i default
    //        -c:v libx264 -preset veryfast -crf 18 -pix_fmt yuv420p
    //        -c:a aac -b:a 192k raw.mkv
    let mut cmd = Command::new("ffmpeg");
    cmd.args([
        "-y",
        "-hide_banner",
        "-loglevel", "warning",
        "-f", "x11grab",
        "-video_size", size,
        "-framerate", &fps.to_string(),
        "-i", display,
    ]);

    if !audio.eq_ignore_ascii_case("none") {
        cmd.args(["-f", "pulse", "-i", audio]);
    }

    cmd.args([
        "-c:v", "libx264",
        "-preset", "veryfast",
        "-crf", "18",
        "-pix_fmt", "yuv420p",
    ]);

    if !audio.eq_ignore_ascii_case("none") {
        cmd.args(["-c:a", "aac", "-b:a", "192k"]);
    }

    cmd.arg(out.as_os_str());
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());

    let child = cmd.spawn().context(
        "failed to spawn ffmpeg — is it installed? `apt-get install -y ffmpeg`",
    )?;
    let pid = child.id();
    // Detach: ffmpeg is now ours-but-orphaned. We deliberately drop the handle.
    std::mem::forget(child);

    std::fs::write(pidfile, pid.to_string())?;
    Ok(pid)
}

pub fn stop(pidfile: &Path) -> Result<()> {
    let pid_str = std::fs::read_to_string(pidfile)
        .with_context(|| format!("reading pidfile {}", pidfile.display()))?;
    let pid: i32 = pid_str.trim().parse().context("pidfile is not an integer")?;

    // SIGINT → ffmpeg flushes the mux trailer cleanly.
    let status = Command::new("kill")
        .args(["-INT", &pid.to_string()])
        .status()
        .context("kill -INT failed")?;
    if !status.success() {
        return Err(anyhow!("kill -INT {pid} returned {}", status));
    }
    let _ = std::fs::remove_file(pidfile);
    Ok(())
}
