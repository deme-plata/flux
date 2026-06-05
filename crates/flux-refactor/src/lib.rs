// flux-refactor — AI-Assisted Refactoring Engine
//
// v0.1.0 — First delivery: API index + mismatch detection.
// Crate 16 in the Flux workspace.
//
// Modules:
//   api_index.rs       — Scan all flux crate pub exports into a JSON index
//   mismatch.rs        — Compare source code API calls against index, flag mismatches
//   handler_extract.rs — Auto-extract match arms into handler modules
//   score_model.rs     — Predict architecture score delta of refactors
//   registry_gen.rs    — Generate ToolRegistry boilerplate + test scaffolding

pub mod api_index;
pub mod mismatch;
pub mod handler_extract;
pub mod score_model;
pub mod registry_gen;

/// Refactor result: what the engine found or produced.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RefactorAudit {
    pub crate_name: String,
    pub api_calls: Vec<ApiCall>,
    pub mismatches: Vec<ApiMismatch>,
    pub suggested_fixes: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ApiCall {
    pub file: String,
    pub line: usize,
    pub function: String,
    pub arguments: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ApiMismatch {
    pub file: String,
    pub line: usize,
    pub call_expression: String,
    pub actual_function: String,
    pub expected_args: usize,
    pub actual_args: usize,
    pub missing_fields: Vec<String>,
    pub wrong_field_names: Vec<(String, String)>, // (used, correct)
}
