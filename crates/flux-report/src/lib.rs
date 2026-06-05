// flux-report — Flux-driven LaTeX project reports.
//
// Composes the Q-NarwhalKnight visual template (qblue/qgreen/qred palette,
// pgfgantt, pgfplots/TikZ radial for SAP) with live data the Flux toolchain
// already tracks: workspace version, swarm history, SAP, flux-ai audit, plus
// digests from a directory of markdown reports rsynced from Beta's docs/.
//
// Entry point: `render_report(opts)` returns `RenderedReport { tex, base_name }`.
// The bin in src/main.rs wraps that with file I/O and optional pdflatex.

pub mod sources;
pub mod state;
pub mod tex;

use crate::sources::{SourceCategory, SourceDigest};
use crate::state::ReportState;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ReportOptions {
    /// Workspace root (Cargo.toml lives here). Drives version + flux-ai.
    pub workspace_root: PathBuf,
    /// Directory of markdown source reports — typically a local rsync of
    /// Beta's `/opt/orobit/shared/q-narwhalknight/docs/`.
    pub sources_dir: PathBuf,
    /// Where to look for swarm state. Defaults to `/tmp/flux-swarm.json`.
    pub swarm_path: PathBuf,
    /// Report title — appears on the cover and in `\pdftitle`.
    pub title: String,
    /// Reporting period label, e.g. "May 2026" or "Q2 2026".
    pub period: String,
    /// Output basename — `<base>.tex`, `<base>.pdf` after pdflatex.
    pub base_name: String,
    /// Optional Gantt phases. If empty, a Gantt is synthesized from swarm
    /// completion history (one bar per agent, one milestone per settled task).
    pub gantt_phases: Vec<GanttPhase>,
    /// Number of weekly columns when synthesizing the Gantt from swarm history.
    pub gantt_weeks: u32,
}

/// One phase / group on the Gantt chart. Mirrors the pgfgantt structure used
/// in Beta's project-report-2026-05.tex: a group line spanning a column
/// range, with N bars + zero-or-more milestones underneath.
#[derive(Debug, Clone)]
pub struct GanttPhase {
    /// Group title (e.g. "March — Foundation").
    pub group: String,
    /// 1-indexed start column for the group line.
    pub group_start: u32,
    /// 1-indexed end column for the group line (inclusive).
    pub group_end: u32,
    pub bars: Vec<GanttBar>,
    pub milestones: Vec<GanttMilestone>,
}

#[derive(Debug, Clone)]
pub struct GanttBar {
    pub label: String,
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone)]
pub struct GanttMilestone {
    pub label: String,
    pub at: u32,
}

impl ReportOptions {
    pub fn default_for(workspace_root: PathBuf, sources_dir: PathBuf, base_name: &str) -> Self {
        let now = chrono::Utc::now();
        let period = now.format("%B %Y").to_string();
        Self {
            workspace_root,
            sources_dir,
            swarm_path: PathBuf::from("/tmp/flux-swarm.json"),
            title: "Quillon Graph — Technical Progress Report".into(),
            period,
            base_name: base_name.into(),
            gantt_phases: Vec::new(),
            gantt_weeks: 12,
        }
    }
}

pub struct RenderedReport {
    pub tex: String,
    pub base_name: String,
    pub state: ReportState,
    pub sources: Vec<SourceDigest>,
}

pub fn render_report(opts: &ReportOptions) -> RenderedReport {
    let state = state::snapshot(&opts.workspace_root, &opts.swarm_path);
    let sources = sources::load_sources(&opts.sources_dir);
    let tex = render_tex(opts, &state, &sources);
    RenderedReport {
        tex,
        base_name: opts.base_name.clone(),
        state,
        sources,
    }
}

fn render_tex(opts: &ReportOptions, state: &ReportState, sources: &[SourceDigest]) -> String {
    let mut s = String::with_capacity(8192);
    s.push_str(PREAMBLE);
    s.push_str(&format!(
        "\\hypersetup{{colorlinks=true, linkcolor=qblue, urlcolor=qblue, citecolor=qblue,\n  pdftitle={{{}}}}}\n",
        tex::escape(&opts.title)
    ));
    s.push_str(&format!("\\rhead{{\\textcolor{{qgray}}{{\\small {} \\textendash\\ {}}}}}\n",
        tex::escape(&opts.title), tex::escape(&opts.period)));
    s.push_str(HEADER_FOOTER);
    s.push_str("\\begin{document}\n\n");

    render_cover(&mut s, opts, state);
    render_executive_summary(&mut s, state, sources);
    render_gantt_landscape(&mut s, opts, state);
    render_swarm_section(&mut s, state);
    render_sap_radial(&mut s, state);
    render_flux_ai_audit(&mut s, state);
    render_source_digests(&mut s, sources);
    render_footer_credits(&mut s, state);

    s.push_str("\n\\end{document}\n");
    s
}

fn render_cover(s: &mut String, opts: &ReportOptions, state: &ReportState) {
    s.push_str("\\begin{center}\n");
    s.push_str(&format!(
        "{{\\Huge\\bfseries\\color{{qblue}} {}}}\\\\[6pt]\n",
        tex::escape(&opts.title)
    ));
    s.push_str(&format!(
        "{{\\Large\\color{{qgray}} {} \\textbullet\\ Flux v{}}}\\\\[2pt]\n",
        tex::escape(&opts.period),
        tex::escape(&state.workspace_version)
    ));
    s.push_str(&format!(
        "{{\\small\\color{{qgray}} Generated {}}}\n",
        tex::escape(&state.generated_at_utc)
    ));
    s.push_str("\\end{center}\n\\vspace{6pt}\n\n");
}

fn render_executive_summary(s: &mut String, state: &ReportState, sources: &[SourceDigest]) {
    let n_tr = sources
        .iter()
        .filter(|d| d.category == SourceCategory::TechnicalReview)
        .count();
    let n_inc = sources
        .iter()
        .filter(|d| d.category == SourceCategory::IncidentReport)
        .count();
    let n_plans = sources
        .iter()
        .filter(|d| d.category == SourceCategory::Plan)
        .count();
    let n_handoffs = sources
        .iter()
        .filter(|d| d.category == SourceCategory::Handoff)
        .count();
    let n_settled = state.swarm.completed.len();
    let total_qug = state.swarm.total_qug_paid;
    let agents: Vec<String> = state
        .swarm
        .agents
        .iter()
        .map(|a| tex::escape(&a.id))
        .collect();

    s.push_str("\\section*{Executive summary}\n");
    s.push_str(&format!(
        "Reporting period \\textbf{{{}}}. Workspace at \\texttt{{v{}}}. The swarm settled \\textbf{{{} task(s)}} totalling \\textbf{{{:.2} QUG}}, across {} registered agent(s){}.\n\n",
        tex::escape(&state.generated_at_utc),
        tex::escape(&state.workspace_version),
        n_settled,
        total_qug,
        state.swarm.agents.len(),
        if agents.is_empty() {
            String::new()
        } else {
            format!(" (\\,{}\\,)", agents.join(", "))
        }
    ));
    s.push_str(&format!(
        "Source corpus on Beta: \\textbf{{{}}} technical review(s), \\textbf{{{}}} incident report(s), \\textbf{{{}}} plan(s), \\textbf{{{}}} handoff(s).\n\n",
        n_tr, n_inc, n_plans, n_handoffs
    ));
}

fn render_swarm_section(s: &mut String, state: &ReportState) {
    s.push_str("\\section{Swarm \\& settlements}\n");
    if state.swarm.agents.is_empty() {
        s.push_str("\\textit{Swarm state file not present at generation time.}\n\n");
        return;
    }
    s.push_str("\\subsection*{Registered agents}\n");
    s.push_str("\\begin{tabular}{l l r l}\n\\toprule\n");
    s.push_str("\\textbf{Agent} & \\textbf{Status} & \\textbf{QUG earned} & \\textbf{Current crates}\\\\\n");
    s.push_str("\\midrule\n");
    for a in &state.swarm.agents {
        s.push_str(&format!(
            "{} & {} & {:.2} & {}\\\\\n",
            tex::escape(&a.id),
            tex::escape(&a.status),
            a.total_earned_qug,
            tex::escape(&a.current_crates.join(", "))
        ));
    }
    s.push_str("\\bottomrule\n\\end{tabular}\n\n");

    if !state.swarm.completed.is_empty() {
        s.push_str("\\subsection*{Recently settled (most recent 12)}\n");
        s.push_str("\\begin{tabular}{l l p{7cm} r}\n\\toprule\n");
        s.push_str("\\textbf{task\\_id} & \\textbf{agent} & \\textbf{crates} & \\textbf{QUG}\\\\\n");
        s.push_str("\\midrule\n");
        let n = state.swarm.completed.len();
        let start = n.saturating_sub(12);
        for c in &state.swarm.completed[start..] {
            s.push_str(&format!(
                "{} & {} & {} & {:.2}\\\\\n",
                tex::escape(&c.task_id),
                tex::escape(&c.agent),
                tex::escape(&c.crates.join(", ")),
                c.qug_earned
            ));
        }
        s.push_str("\\bottomrule\n\\end{tabular}\n\n");
    }
}

fn render_sap_radial(s: &mut String, state: &ReportState) {
    let sap = &state.sap;
    // Five SAP axes as a 5-point radial built directly in TikZ (no pgfplots
    // dependency — the existing template doesn't load it and pdflatex on
    // some Beta nodes is missing the package).
    let axes = [
        ("Compile velocity", sap.compile_velocity),
        ("Cache health", sap.cache_health),
        ("Swarm utilization", sap.swarm_utilization),
        ("Agent diversity", sap.agent_diversity),
        ("Settlement throughput", sap.settlement_throughput),
    ];
    s.push_str("\\section{SAP project chart}\n");
    s.push_str("Synthesized SAP (Score-Adjusted Priority) axes derived from the live Flux toolchain state at generation time. Each axis is normalized 0.0\\,--\\,1.0.\n\n");
    s.push_str("\\begin{center}\n\\begin{tikzpicture}[scale=1.0]\n");

    // Concentric rings + spokes.
    s.push_str("\\foreach \\r in {0.5,1,1.5,2} { \\draw[gray!30] (0,0) circle (\\r); }\n");
    for (i, _) in axes.iter().enumerate() {
        let angle = 90.0 - 72.0 * i as f64; // start at top, go clockwise
        s.push_str(&format!(
            "\\draw[gray!50] (0,0) -- ({}:2);\n",
            angle.round() as i64
        ));
    }
    // Data polygon.
    s.push_str("\\draw[qblue, fill=qblue, fill opacity=0.18, line width=1pt] ");
    for (i, (_label, v)) in axes.iter().enumerate() {
        let angle = 90.0 - 72.0 * i as f64;
        let r = (v.max(0.0).min(1.0)) * 2.0;
        let sep = if i == 0 { "" } else { " -- " };
        s.push_str(&format!("{}({}:{:.3})", sep, angle.round() as i64, r));
    }
    s.push_str(" -- cycle;\n");

    // Labels.
    for (i, (label, v)) in axes.iter().enumerate() {
        let angle = 90.0 - 72.0 * i as f64;
        s.push_str(&format!(
            "\\node[font=\\small] at ({}:2.5) {{{}\\,({:.2})}};\n",
            angle.round() as i64,
            tex::escape(label),
            v
        ));
    }
    s.push_str("\\end{tikzpicture}\n\\end{center}\n\n");
}

fn render_flux_ai_audit(s: &mut String, state: &ReportState) {
    let a = &state.flux_ai;
    s.push_str("\\section{Flux\\,AI audit summary}\n");
    s.push_str("Static-analysis pass over the Flux workspace via \\texttt{flux\\_ai::full\\_ai\\_audit}: lifetime inference, Send/Sync coverage, race detection, unsafe verification, ownership wrapper hints, deadlock freedom.\n\n");
    s.push_str("\\begin{tabular}{l r l}\n\\toprule\n");
    s.push_str("\\textbf{Dimension} & \\textbf{Hints} & \\textbf{Status}\\\\\n\\midrule\n");
    for (label, count) in [
        ("Lifetime inference", a.lifetime_hints),
        ("Send/Sync coverage", a.send_sync_hints),
        ("Race detection", a.race_hints),
        ("Unsafe verification", a.unsafe_hints),
        ("Ownership wrappers", a.ownership_hints),
        ("Deadlock freedom", a.deadlock_hints),
    ] {
        let badge = if count == 0 {
            "\\statusclosed"
        } else {
            "\\statuspending"
        };
        s.push_str(&format!("{} & {} & {}\\\\\n", label, count, badge));
    }
    s.push_str("\\bottomrule\n\\end{tabular}\n\n");
}

fn render_source_digests(s: &mut String, sources: &[SourceDigest]) {
    if sources.is_empty() {
        return;
    }
    s.push_str("\\section{Source reports on Beta}\n");
    s.push_str(&format!(
        "{} markdown source(s) digested from the corpus, grouped by category.\n\n",
        sources.len()
    ));
    let mut current_cat: Option<SourceCategory> = None;
    for d in sources {
        if Some(d.category) != current_cat {
            s.push_str(&format!("\\subsection*{{{}}}\n", d.category.label()));
            current_cat = Some(d.category);
        }
        let lead = sources::truncate_to_words(&d.lead_paragraph, 260);
        s.push_str("\\noindent\\textbf{");
        s.push_str(&tex::escape(&d.title));
        s.push_str("}\\,\\textcolor{qgray}{\\small ");
        s.push_str(&tex::mono(&d.relative_path));
        s.push_str("}\\\\\n");
        s.push_str(&tex::escape(&lead));
        s.push_str("\\\\[6pt]\n");
    }
}

/// Landscape Gantt page in the same style as Beta's
/// `project-report-2026-05.tex`: month titles on top, weekly column titles
/// under them, group lines per phase, bars per task, milestones for
/// inflection points. If `opts.gantt_phases` is empty, synthesizes one phase
/// per agent from completed swarm tasks.
fn render_gantt_landscape(s: &mut String, opts: &ReportOptions, state: &ReportState) {
    let phases: Vec<GanttPhase> = if opts.gantt_phases.is_empty() {
        synthesize_phases_from_swarm(state, opts.gantt_weeks)
    } else {
        opts.gantt_phases.clone()
    };
    if phases.is_empty() {
        return;
    }
    let weeks = opts.gantt_weeks.max(4);

    s.push_str("\\begin{landscape}\n");
    s.push_str("\\section{Development Gantt}\n");
    s.push_str(&format!(
        "{}-week milestone view. Blue bars = work shipped, diamonds = settled milestones, grouped by phase / agent.\n\n",
        weeks
    ));
    s.push_str("\\noindent\\begin{ganttchart}[\n");
    s.push_str("  hgrid, vgrid,\n");
    s.push_str("  x unit=0.6cm, y unit chart=0.55cm, y unit title=0.6cm,\n");
    s.push_str("  bar/.append style={fill=qblue!70, draw=qblue!90},\n");
    s.push_str("  bar label font=\\small,\n");
    s.push_str("  group/.append style={fill=qblue!30, draw=qblue!60},\n");
    s.push_str("  group label font=\\bfseries\\small,\n");
    s.push_str("  milestone/.append style={fill=qred!80, draw=qred!90},\n");
    s.push_str("  milestone label font=\\small,\n");
    s.push_str("  title/.append style={fill=qblue!15, draw=qblue!40},\n");
    s.push_str("  title label font=\\small\\bfseries,\n");
    s.push_str(&format!("  ]{{1}}{{{weeks}}}\n"));
    // Weekly column titles.
    let title_strip: String = (1..=weeks)
        .map(|w| format!("  \\gantttitle{{W{w:02}}}{{1}}"))
        .collect::<Vec<_>>()
        .join("\n");
    s.push_str(&format!("{title_strip} \\\\\n"));

    for (i, phase) in phases.iter().enumerate() {
        s.push_str(&format!(
            "  \\ganttgroup{{{}}}{{{}}}{{{}}} \\\\\n",
            tex::escape(&phase.group),
            phase.group_start.max(1).min(weeks),
            phase.group_end.max(phase.group_start).min(weeks)
        ));
        for bar in &phase.bars {
            s.push_str(&format!(
                "  \\ganttbar{{{}}}{{{}}}{{{}}} \\\\\n",
                tex::escape(&bar.label),
                bar.start.max(1).min(weeks),
                bar.end.max(bar.start).min(weeks)
            ));
        }
        for ms in &phase.milestones {
            s.push_str(&format!(
                "  \\ganttmilestone{{{}}}{{{}}} \\\\\n",
                tex::escape(&ms.label),
                ms.at.max(1).min(weeks)
            ));
        }
        // Visual breathing room between groups, unless last.
        if i + 1 < phases.len() {
            s.push_str("  \\ganttnewline\n");
        }
    }
    s.push_str("\\end{ganttchart}\n");
    s.push_str("\\vspace{0.5em}\n");
    s.push_str("{\\small\\textit{Each column $\\approx 1$ week. Red diamonds = milestones; blue bars = work shipped; darker blue groups = phase headers.}}\n");
    s.push_str("\\end{landscape}\n\\clearpage\n\n");
}

/// Build a coarse Gantt when no phases were supplied. Strategy:
///   - One group per registered agent in the swarm.
///   - One bar per completed task assigned roughly to the week-of-month
///     derived from `completed_at` (we lack the original claim time, so
///     each completed task becomes a one-week bar in the assigned week).
///   - One milestone marker for each agent's most recent settled task.
fn synthesize_phases_from_swarm(state: &ReportState, weeks: u32) -> Vec<GanttPhase> {
    if state.swarm.agents.is_empty() {
        return vec![];
    }
    let mut phases = Vec::new();
    let weeks = weeks.max(4);
    // Distribute agents evenly across the weekly axis: each gets a slot
    // proportional to its share of completions. Idle agents get a thin
    // slot at week 1 so they still appear on the chart.
    let total_done = state.swarm.completed.len().max(1) as f64;
    let mut col_cursor: f64 = 1.0;
    let span_per_completion: f64 = (weeks - 1) as f64 / total_done;

    for agent in &state.swarm.agents {
        let agent_done: Vec<&state::SwarmCompletedRow> = state
            .swarm
            .completed
            .iter()
            .filter(|c| c.agent == agent.id)
            .collect();
        if agent_done.is_empty() {
            phases.push(GanttPhase {
                group: format!("{} — idle this period", agent.id),
                group_start: 1,
                group_end: 2,
                bars: vec![],
                milestones: vec![],
            });
            continue;
        }
        let group_start = col_cursor.round() as u32;
        let group_end =
            (col_cursor + agent_done.len() as f64 * span_per_completion).round() as u32;
        col_cursor = group_end as f64;

        let mut bars = Vec::with_capacity(agent_done.len());
        for (i, task) in agent_done.iter().enumerate() {
            let bar_start = group_start + (i as u32);
            let bar_end = bar_start; // 1-week bar
            let label = if task.crates.is_empty() {
                task.task_id.clone()
            } else {
                format!("{} ({})", task.task_id, task.crates.join(", "))
            };
            bars.push(GanttBar {
                label,
                start: bar_start.min(weeks),
                end: bar_end.min(weeks),
            });
        }
        let milestones = vec![GanttMilestone {
            label: format!("{:.2} QUG", agent.total_earned_qug),
            at: group_end.min(weeks),
        }];
        phases.push(GanttPhase {
            group: format!("{} — {:.2} QUG", agent.id, agent.total_earned_qug),
            group_start: group_start.min(weeks),
            group_end: group_end.min(weeks),
            bars,
            milestones,
        });
    }
    phases
}

fn render_footer_credits(s: &mut String, state: &ReportState) {
    s.push_str("\\vfill\n\\begin{center}\\small\\color{qgray}\n");
    s.push_str(&format!(
        "Built with flux-report \\textbullet\\ Flux v{} \\textbullet\\ rocky (Claude Opus 4.7) on Epsilon \\textbullet\\ {}\n",
        tex::escape(&state.workspace_version),
        tex::escape(&state.generated_at_utc)
    ));
    s.push_str("\\end{center}\n");
}

// --- LaTeX preamble: lifted from Beta's project-report-2026-05.tex so the
//     visual style stays consistent across machine-generated reports.

const PREAMBLE: &str = r#"\documentclass[11pt,a4paper]{article}
\usepackage[margin=2cm]{geometry}
\usepackage[T1]{fontenc}
\usepackage{lmodern}
\usepackage{microtype}
\usepackage{graphicx}
\usepackage{booktabs}
\usepackage{longtable}
\usepackage{tabularx}
\usepackage{array}
\usepackage{xcolor}
\usepackage{colortbl}
\usepackage{hyperref}
\usepackage{tikz}
\usepackage{pgfgantt}
\usepackage{pdflscape}
\usepackage{rotating}
\usepackage{enumitem}
\usepackage{fancyhdr}
\usepackage{titlesec}
\usepackage{amsmath}
\usepackage{amssymb}
\usepackage{caption}
\usepackage{float}
\usepackage[utf8]{inputenc}
\usepackage{textgreek}
\usepackage{textcomp}
\usepackage{eurosym}

\definecolor{qblue}{RGB}{30,136,229}
\definecolor{qgreen}{RGB}{67,160,71}
\definecolor{qred}{RGB}{229,57,53}
\definecolor{qorange}{RGB}{251,140,0}
\definecolor{qgray}{RGB}{96,125,139}
\definecolor{rowgray}{RGB}{245,245,245}

\titleformat{\section}{\large\bfseries\color{qblue}}{}{0em}{}[\titlerule]
\titleformat{\subsection}{\normalsize\bfseries\color{qgray}}{}{0em}{}

\setlength{\parskip}{4pt}
\setlength{\parindent}{0pt}

\newcommand{\statusopen}{\textcolor{qred}{\bfseries Open}}
\newcommand{\statusclosed}{\textcolor{qgreen}{\bfseries $\checkmark$~Closed}}
\newcommand{\statuspending}{\textcolor{qorange}{\bfseries Pending}}
"#;

const HEADER_FOOTER: &str = r#"
\lhead{\textcolor{qgray}{\small CONFIDENTIAL}}
\cfoot{\textcolor{qgray}{\thepage}}
\pagestyle{fancy}
\fancyhf{}
\rhead{\textcolor{qgray}{\small Q-NarwhalKnight}}
\lhead{\textcolor{qgray}{\small CONFIDENTIAL}}
\cfoot{\textcolor{qgray}{\thepage}}
\renewcommand{\headrulewidth}{0.4pt}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn opts() -> ReportOptions {
        ReportOptions {
            workspace_root: PathBuf::from("/tmp/__no_ws__"),
            sources_dir: PathBuf::from("/tmp/__no_sources__"),
            swarm_path: PathBuf::from("/tmp/__no_swarm__"),
            title: "Quillon Graph — Test".into(),
            period: "May 2026".into(),
            base_name: "test-report".into(),
            gantt_phases: vec![],
            gantt_weeks: 12,
        }
    }

    #[test]
    fn explicit_gantt_phases_emit_pgfgantt() {
        let mut o = opts();
        o.gantt_phases = vec![GanttPhase {
            group: "March — Foundation".into(),
            group_start: 1,
            group_end: 6,
            bars: vec![
                GanttBar { label: "v9.x close-out".into(), start: 1, end: 3 },
                GanttBar { label: "v10.0.x Windows fix".into(), start: 3, end: 5 },
            ],
            milestones: vec![GanttMilestone { label: "v10.1 ship".into(), at: 6 }],
        }];
        let r = render_report(&o);
        assert!(r.tex.contains("\\begin{ganttchart}"));
        assert!(r.tex.contains("\\ganttgroup{March \u{2014} Foundation}"));
        assert!(r.tex.contains("\\ganttbar{v9.x close-out}{1}{3}"));
        assert!(r.tex.contains("\\ganttmilestone{v10.1 ship}{6}"));
        assert!(r.tex.contains("\\begin{landscape}"));
    }

    #[test]
    fn render_emits_documentclass_and_begin_end() {
        let r = render_report(&opts());
        assert!(r.tex.contains("\\documentclass"));
        assert!(r.tex.contains("\\begin{document}"));
        assert!(r.tex.contains("\\end{document}"));
    }

    #[test]
    fn render_uses_q_palette() {
        let r = render_report(&opts());
        // qblue is the canonical title color; if it drifts we'd notice here.
        assert!(r.tex.contains("\\definecolor{qblue}{RGB}{30,136,229}"));
    }

    #[test]
    fn render_includes_sap_radial() {
        let r = render_report(&opts());
        // The radial is built with TikZ — confirm at least the structure
        // (concentric rings) lands so a refactor doesn't silently drop the
        // chart and leave a section without its visual.
        assert!(r.tex.contains("SAP project chart"));
        assert!(r.tex.contains("\\begin{tikzpicture}"));
        assert!(r.tex.contains("circle (0.5)") || r.tex.contains("circle (\\r)"));
    }

    #[test]
    fn render_escapes_dangerous_title() {
        let mut o = opts();
        o.title = "Quillon & Co_2026".into();
        let r = render_report(&o);
        // `&` and `_` must be escaped or pdflatex crashes.
        assert!(r.tex.contains("Quillon \\& Co\\_2026"));
        assert!(!r.tex.contains("Quillon & Co_2026"));
    }

    #[test]
    fn render_survives_missing_state_files() {
        // All input paths above point at nowhere — render still produces a
        // complete document with cover + exec summary placeholders.
        let r = render_report(&opts());
        assert!(r.tex.contains("Executive summary"));
        // Empty swarm collapses to the placeholder line.
        assert!(r.tex.contains("Swarm state file not present"));
    }
}
