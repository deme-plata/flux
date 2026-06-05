// flux-graph — Dependency graph resolution engine
//
// Phase 2a: Intra-workspace dependency resolution without cargo.
// Discovers workspace members, parses their Cargo.toml manifests,
// builds a dependency DAG, topologically sorts, and emits rustc flags.
//
// Phase 2b (future): crates.io + git dependency resolution.

pub mod workspace;
pub mod manifest;
pub mod graph;
pub mod resolver;
pub mod build_order;
pub mod agility;

use std::path::PathBuf;

/// Cache-aligned: hot fields grouped. A single crate, parsed from its Cargo.toml.
#[derive(Debug, Clone)]
#[repr(C, align(64))]
pub struct CrateInfo {
    pub name: String,
    pub path: PathBuf,
    pub dependencies: Vec<Dependency>,
    pub edition: String,
    pub crate_type: CrateType,
    pub features: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CrateType {
    Lib,
    Bin,
    ProcMacro,
}

/// A dependency (workspace path dep or external crates.io/git dep).
#[derive(Debug, Clone)]
pub struct Dependency {
    pub name: String,
    pub path: Option<PathBuf>,
    pub kind: DepKind,
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DepKind {
    Path,
    CratesIo,
    Git,
}

/// The full resolved workspace: all crates + their dependency graph.
#[derive(Debug, Clone)]
pub struct WorkspaceGraph {
    pub root: PathBuf,
    pub crates: Vec<CrateInfo>,
    /// Build order: each inner Vec is a parallel batch (crates with no inter-dependencies).
    pub batches: Vec<Vec<usize>>, // indices into `crates`
}

/// Discover and resolve the entire workspace.
pub fn resolve_workspace(root: &PathBuf) -> Result<WorkspaceGraph, String> {
    let member_paths = workspace::discover_members(root)?;
    let mut crates = Vec::new();
    for mp in &member_paths {
        // Resilient: a single half-written / unreadable crate must not sink the whole graph.
        // (A mid-create member with no Cargo.toml yet used to hard-fail flux_api_generate.)
        if let Ok(ci) = manifest::parse_crate(mp) {
            crates.push(ci);
        }
    }
    let dag = graph::build_dag(&crates)?;
    let batches = graph::topological_batches(&dag);
    Ok(WorkspaceGraph { root: root.clone(), crates, batches })
}
