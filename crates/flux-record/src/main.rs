use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use flux_record::{capture, captions, chapters, demo, ffmpeg, pty, shorts, transcript, vite};

#[derive(Parser)]
#[command(name = "flux-record")]
#[command(version, about = "Cinematic Claude Code session recorder", long_about = None)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start screen+audio capture (writes raw.mkv via ffmpeg x11grab/pulse)
    Start {
        #[arg(long, default_value = ":0.0")]
        display: String,
        #[arg(long, default_value = "1920x1080")]
        size: String,
        #[arg(long, default_value = "30")]
        fps: u32,
        #[arg(long, default_value = "default")]
        audio: String,
        #[arg(long, default_value = "raw.mkv")]
        out: PathBuf,
        #[arg(long, default_value = "/tmp/flux-record.pid")]
        pidfile: PathBuf,
    },
    /// Stop a running capture (reads pidfile)
    Stop {
        #[arg(long, default_value = "/tmp/flux-record.pid")]
        pidfile: PathBuf,
    },
    /// Render cinematic mp4: raw + transcript -> styled YouTube-ready video
    Render {
        #[arg(long)]
        raw: PathBuf,
        #[arg(long)]
        transcript: PathBuf,
        /// Optional flux-events.jsonl produced by the control server (HMR/DOM/console)
        #[arg(long)]
        events: Option<PathBuf>,
        #[arg(long, default_value = "cinematic.mp4")]
        out: PathBuf,
        #[arg(long, default_value = "Claude Code Session")]
        title: String,
        #[arg(long, default_value = "Claude Opus 4.7")]
        subtitle: String,
        #[arg(long, default_value_t = false)]
        no_kenburns: bool,
        #[arg(long, default_value_t = false)]
        no_vignette: bool,
        /// Rescale event timestamps so the last event lands at this video time.
        /// Use this to fit a multi-hour transcript into an N-minute video.
        #[arg(long)]
        compress_to_seconds: Option<f64>,
        /// Speed/quality preset: `fast` (ultrafast, ~5× faster, default) or
        /// `quality` (slow, final-quality, ~5× slower).
        #[arg(long, default_value = "fast")]
        speed: String,
    },
    /// Emit YouTube chapter timestamps from a transcript
    Chapters {
        #[arg(long)]
        transcript: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Detect interesting moments and export 9:16 vertical Shorts clips
    Shorts {
        #[arg(long)]
        raw: PathBuf,
        #[arg(long)]
        transcript: PathBuf,
        #[arg(long, default_value = "shorts")]
        out: PathBuf,
        #[arg(long, default_value_t = 60)]
        max_seconds: u32,
        #[arg(long, default_value_t = 5)]
        max_clips: u32,
    },
    /// Generate ASS karaoke-style captions from assistant text events
    Captions {
        #[arg(long)]
        transcript: PathBuf,
        #[arg(long, default_value = "captions.ass")]
        out: PathBuf,
    },
    /// Dual capture: terminal window + browser window side-by-side (1920×1080)
    Dual {
        #[arg(long, default_value = ":0.0")]
        display: String,
        /// "WxH+X+Y" geometry of the terminal window (use `xdotool getwindowgeometry`)
        #[arg(long, default_value = "1280x1440+0+0")]
        term: String,
        /// "WxH+X+Y" geometry of the Chromium/Firefox window
        #[arg(long, default_value = "1280x1440+1280+0")]
        browser: String,
        #[arg(long, default_value = "30")]
        fps: u32,
        #[arg(long, default_value = "default")]
        audio: String,
        #[arg(long, default_value = "dual.mkv")]
        out: PathBuf,
        #[arg(long, default_value = "/tmp/flux-record.pid")]
        pidfile: PathBuf,
    },
    /// Start the HMR control server. Receives POSTs on /event and appends
    /// to flux-events.jsonl. Pair with the Vite plugin or bookmarklet.
    Control {
        #[arg(long, default_value = "127.0.0.1:9876")]
        bind: String,
        #[arg(long, default_value = "flux-events.jsonl")]
        events: PathBuf,
    },
    /// Print a one-liner JS bookmarklet that streams HMR + console + route
    /// events to the control server. Drop it in your bookmarks bar.
    Bookmarklet {
        #[arg(long, default_value = "http://127.0.0.1:9876/event")]
        endpoint: String,
    },
    /// Print the source of `vite-plugin-flux-record` for you to drop into a
    /// Vite project. Wires `handleHotUpdate` to POST events at the endpoint.
    VitePlugin {
        #[arg(long, default_value = "http://127.0.0.1:9876/event")]
        endpoint: String,
    },
    /// Extract a single styled frame as a YouTube thumbnail PNG
    Thumbnail {
        #[arg(long)]
        raw: PathBuf,
        #[arg(long, default_value_t = 3.0)]
        at_seconds: f64,
        #[arg(long, default_value = "Claude Code Session")]
        title: String,
        #[arg(long, default_value = "Built live with Claude Opus 4.7")]
        subtitle: String,
        #[arg(long, default_value = "thumbnail.png")]
        out: PathBuf,
    },
    /// Synthesize a realistic terminal+preview "raw" video from a transcript
    /// alone (no actual screen capture required). Output feeds into `render`.
    Demo {
        #[arg(long)]
        transcript: PathBuf,
        #[arg(long, default_value = "demo.mkv")]
        out: PathBuf,
        #[arg(long, default_value_t = 180)]
        duration_s: u32,
        #[arg(long, default_value_t = 30)]
        fps: u32,
        #[arg(long, default_value = "~/work/project")]
        cwd_label: String,
    },
    /// Record a real terminal session via `script -t` (PTY capture). Wraps
    /// your $SHELL; exit (Ctrl-D) when done. Produces typescript+timing.
    PtyRec {
        #[arg(long, default_value = "typescript")]
        typescript: PathBuf,
        #[arg(long, default_value = "timing")]
        timing: PathBuf,
        #[arg(long)]
        shell: Option<String>,
    },
    /// Render a PTY-captured session (typescript+timing) into a video that
    /// can feed `flux-record render --raw`.
    PtyRender {
        #[arg(long)]
        typescript: PathBuf,
        #[arg(long)]
        timing: PathBuf,
        #[arg(long, default_value = "pty.mkv")]
        out: PathBuf,
        #[arg(long, default_value_t = 30)]
        fps: u32,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Start { display, size, fps, audio, out, pidfile } => {
            let pid = capture::start(&display, &size, fps, &audio, &out, &pidfile)
                .context("failed to start capture")?;
            println!("flux-record: capture started, pid={pid}, out={}", out.display());
            println!("           : stop with `flux-record stop --pidfile {}`", pidfile.display());
        }
        Cmd::Stop { pidfile } => {
            capture::stop(&pidfile).context("failed to stop capture")?;
            println!("flux-record: capture stopped");
        }
        Cmd::Render { raw, transcript, events, out, title, subtitle, no_kenburns, no_vignette, compress_to_seconds, speed } => {
            let mut tx = transcript::load(&transcript)
                .with_context(|| format!("loading transcript {}", transcript.display()))?;
            if let Some(target) = compress_to_seconds {
                tx = transcript::compress_timeline(tx, target * 0.92);
            }
            let extra = match events {
                Some(p) => vite::load_events(&p).unwrap_or_default(),
                None => Vec::new(),
            };
            let merged = vite::merge(tx, extra);
            let (preset, crf) = match speed.as_str() {
                "quality" => ("slow".to_string(), 18u32),
                "balanced" => ("medium".to_string(), 20u32),
                _ => ("ultrafast".to_string(), 23u32),
            };
            ffmpeg::render(
                &raw,
                &merged,
                &out,
                &ffmpeg::RenderOpts {
                    title,
                    subtitle,
                    kenburns: !no_kenburns,
                    vignette: !no_vignette,
                    preset,
                    crf,
                },
            )?;
            println!("flux-record: rendered {} ({} events)", out.display(), merged.len());
        }
        Cmd::Chapters { transcript, out } => {
            let events = transcript::load(&transcript)?;
            let text = chapters::format_youtube(&events);
            if let Some(path) = out {
                std::fs::write(&path, &text)?;
                println!("flux-record: wrote chapters to {}", path.display());
            } else {
                print!("{text}");
            }
        }
        Cmd::Shorts { raw, transcript, out, max_seconds, max_clips } => {
            let events = transcript::load(&transcript)?;
            let clips = shorts::pick(&events, max_seconds, max_clips);
            std::fs::create_dir_all(&out)?;
            for (i, clip) in clips.iter().enumerate() {
                let path = out.join(format!("short_{:02}.mp4", i + 1));
                shorts::render_vertical(&raw, clip, &path)?;
                println!(
                    "flux-record: short {} -> {} ({}..{}, \"{}\")",
                    i + 1, path.display(), clip.start_s, clip.end_s, clip.caption
                );
            }
            if clips.is_empty() {
                println!("flux-record: no highlight-worthy moments detected");
            }
        }
        Cmd::Captions { transcript, out } => {
            let events = transcript::load(&transcript)?;
            let ass = captions::render_ass(&events);
            std::fs::write(&out, ass)?;
            println!("flux-record: wrote {}", out.display());
        }
        Cmd::Dual { display, term, browser, fps, audio, out, pidfile } => {
            let pid = capture::start_dual(&display, &term, &browser, fps, &audio, &out, &pidfile)
                .context("failed to start dual capture")?;
            println!("flux-record: dual capture started, pid={pid}");
            println!("           : terminal {term}  |  browser {browser}");
            println!("           : out -> {}", out.display());
            println!("           : stop with `flux-record stop --pidfile {}`", pidfile.display());
        }
        Cmd::Control { bind, events } => {
            let server = vite::ControlServer::new(bind, events);
            server.serve()?;
        }
        Cmd::Bookmarklet { endpoint } => {
            print!("{}", bookmarklet_js(&endpoint));
        }
        Cmd::VitePlugin { endpoint } => {
            print!("{}", vite_plugin_js(&endpoint));
        }
        Cmd::Thumbnail { raw, at_seconds, title, subtitle, out } => {
            ffmpeg::thumbnail(&raw, at_seconds, &title, &subtitle, &out)?;
            println!("flux-record: thumbnail -> {}", out.display());
        }
        Cmd::Demo { transcript, out, duration_s, fps, cwd_label } => {
            let events = transcript::load(&transcript)?;
            demo::synthesize(
                &events,
                &out,
                &demo::DemoOpts { duration_s, fps, cwd_label },
            )?;
            println!(
                "flux-record: demo -> {} ({} events, {}s, {}fps)",
                out.display(), events.len(), duration_s, fps
            );
        }
        Cmd::PtyRec { typescript, timing, shell } => {
            let opts = pty::PtyOpts {
                shell: shell.unwrap_or_else(|| std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into())),
                typescript: typescript.clone(),
                timing: timing.clone(),
            };
            println!("flux-record: starting PTY capture in {} — exit shell to stop", opts.shell);
            pty::record(&opts)?;
            println!("flux-record: pty-rec finished. typescript={} timing={}",
                typescript.display(), timing.display());
        }
        Cmd::PtyRender { typescript, timing, out, fps } => {
            let frames = pty::frame(&typescript, &timing, fps)?;
            println!("flux-record: framed {} terminal states", frames.len());
            pty::render(&frames, &out, &pty::RenderOpts { fps, ..Default::default() })?;
            println!("flux-record: pty-render -> {}", out.display());
        }
    }
    Ok(())
}

fn bookmarklet_js(endpoint: &str) -> String {
    // One-liner bookmarklet. Hooks Vite's HMR client, captures console.error,
    // and posts route changes. Paste into a bookmark's URL field.
    let raw = format!(
        "(()=>{{const E='{endpoint}';const t0=Date.now();\
        const send=(o)=>fetch(E,{{method:'POST',mode:'no-cors',headers:{{'Content-Type':'application/json'}},body:JSON.stringify({{...o,t:(Date.now()-t0)/1000}})}}).catch(()=>{{}});\
        try{{if(import.meta&&import.meta.hot){{import.meta.hot.on('vite:beforeUpdate',(p)=>p.updates.forEach(u=>send({{type:'hmr',file:u.path}})))}}}}catch(e){{}}\
        const ce=console.error;console.error=function(...a){{send({{type:'console',level:'error',text:a.join(' ')}});return ce.apply(this,a)}};\
        const ps=history.pushState;history.pushState=function(...a){{send({{type:'route',to:location.pathname+location.search}});return ps.apply(this,a)}};\
        send({{type:'console',level:'info',text:'flux-record bookmarklet armed'}});}})();"
    );
    format!("javascript:{}\n", raw)
}

fn vite_plugin_js(endpoint: &str) -> String {
    format!(
        r#"// vite-plugin-flux-record.mjs — drop this into your Vite project and add
// to vite.config.ts plugins:
//   import flux from './vite-plugin-flux-record.mjs';
//   export default {{ plugins: [react(), flux()] }};
//
// Streams HMR + module-graph events to flux-record's control server so the
// cinematic render shows badges synced to the moment the browser hot-reloads.
export default function fluxRecord(opts = {{}}) {{
  const endpoint = opts.endpoint || '{endpoint}';
  const t0 = Date.now();
  const post = (body) => {{
    try {{
      fetch(endpoint, {{
        method: 'POST',
        headers: {{ 'Content-Type': 'application/json' }},
        body: JSON.stringify({{ ...body, t: (Date.now() - t0) / 1000 }}),
      }}).catch(() => {{}});
    }} catch (_) {{}}
  }};
  return {{
    name: 'vite-plugin-flux-record',
    apply: 'serve',
    configureServer(server) {{
      server.ws.on('connection', () => post({{ type: 'console', level: 'info', text: 'HMR client connected' }}));
      post({{ type: 'console', level: 'info', text: 'flux-record vite plugin armed' }});
    }},
    handleHotUpdate(ctx) {{
      post({{ type: 'hmr', file: ctx.file }});
      return ctx.modules;
    }},
  }};
}}
"#
    )
}
