// Integration tests for v0.12-D — proves every doc `generate_openapi` emits
// validates against an independent OpenAPI 3.1 parser (`oas3` crate) and that
// the structural shape of one stable example (flux-ue-bridge) doesn't regress
// between releases.
//
// These run via the test binary at `target/debug/deps/openapi_golden-<hash>` —
// distinct from the in-tree `lib.rs` tests; see
// [[feedback-flux-combo-zero-tests-means-test-build-failed]] for why running
// the binary directly is the only way to trust test counts.

use flux_api::{
    discover_endpoints, discover_schemas, generate_openapi_with_schemas,
};
use serde_json::Value;

fn ci(name: &str) -> flux_graph::CrateInfo {
    flux_graph::CrateInfo {
        name: name.into(),
        path: std::path::PathBuf::from("/tmp"),
        dependencies: vec![],
        edition: "2021".into(),
        crate_type: flux_graph::CrateType::Lib,
        features: vec![],
    }
}
fn ws(names: &[&str]) -> flux_graph::WorkspaceGraph {
    flux_graph::WorkspaceGraph {
        root: std::path::PathBuf::from("/tmp"),
        crates: names.iter().map(|n| ci(n)).collect(),
        batches: vec![],
    }
}

fn doc_for(crates: &[&str]) -> Value {
    let g = ws(crates);
    let eps = discover_endpoints(&g);
    // Thread the real schema registry so component schemas are populated, not
    // stubbed — and prove oas3 still accepts the richer document.
    generate_openapi_with_schemas(
        &format!("{} suite", crates.join("+")),
        "0.17.0",
        &eps,
        &discover_schemas(&g),
    )
}

fn assert_oas3_parses(doc: &Value, label: &str) {
    let s = serde_json::to_string(doc).expect("serialize doc");
    // oas3 0.16 exposes `Spec` (their typed root). Deserialize into it via
    // serde_json — succeeds iff every field shape matches the OpenAPI 3.1
    // schema oas3 knows about.
    match serde_json::from_str::<oas3::Spec>(&s) {
        Ok(_) => {}
        Err(e) => panic!("{label}: oas3 rejected the generated doc: {e}\n\n{s}"),
    }
}

#[test]
fn wickes_cms_doc_validates() {
    let doc = doc_for(&["wickes-cms"]);
    assert_oas3_parses(&doc, "wickes-cms");
}

#[test]
fn wickes_erp_doc_validates() {
    let doc = doc_for(&["wickes-erp"]);
    assert_oas3_parses(&doc, "wickes-erp");
}

#[test]
fn wickes_finance_doc_validates() {
    let doc = doc_for(&["wickes-finance"]);
    assert_oas3_parses(&doc, "wickes-finance");
}

#[test]
fn flux_ue_bridge_doc_validates() {
    let doc = doc_for(&["flux-ue-bridge"]);
    assert_oas3_parses(&doc, "flux-ue-bridge");
}

#[test]
fn mixed_crates_doc_validates() {
    let doc = doc_for(&["wickes-cms", "wickes-finance", "flux-ue-bridge"]);
    assert_oas3_parses(&doc, "mixed");
}

/// Stable structural snapshot for flux-ue-bridge. If something accidentally
/// drops parameters/body or renames the operationId scheme, this fires loud.
#[test]
fn flux_ue_bridge_doc_structural_snapshot() {
    let doc = doc_for(&["flux-ue-bridge"]);

    // Top level.
    assert_eq!(doc["openapi"], "3.1.0");

    // Paths present.
    for path in ["/v1/webhook", "/v1/workspace", "/v1/events"] {
        assert!(
            doc["paths"].get(path).is_some(),
            "missing path {path} in doc:\n{doc}"
        );
    }

    // Webhook is POST with object body that has a required `event` field.
    let webhook = &doc["paths"]["/v1/webhook"]["post"];
    assert_eq!(webhook["operationId"], "flux_ue_bridge_0");
    let body = &webhook["requestBody"]["content"]["application/json"]["schema"];
    assert_eq!(body["type"], "object");
    assert!(body["properties"]["event"]["type"] == "string");
    assert_eq!(body["required"], serde_json::json!(["event"]));

    // Workspace + events GETs have no params + no body.
    for path in ["/v1/workspace", "/v1/events"] {
        let op = &doc["paths"][path]["get"];
        assert!(op.get("requestBody").is_none(), "{path} should have no body");
        assert!(
            op.get("parameters").map_or(true, |p| {
                p.as_array().map_or(true, |a| a.is_empty())
            }),
            "{path} should have no parameters"
        );
    }
}

/// Ensure the components/schemas block carries REAL definitions for every Ref
/// reachable from the wickes payments use-case (Page + Order + Payment), plus
/// the transitive LineItem that Order pulls in — not just stub `{}`s.
#[test]
fn wickes_full_suite_lists_all_refs_in_components() {
    let doc = doc_for(&["wickes-cms", "wickes-erp", "wickes-finance"]);
    let schemas = doc["components"]["schemas"]
        .as_object()
        .expect("components.schemas object");
    for name in ["Page", "Order", "Payment", "LineItem"] {
        assert!(
            schemas.contains_key(name),
            "missing {name} in components.schemas: {:?}",
            schemas.keys().collect::<Vec<_>>()
        );
        // Real object definition, not the empty `{}` stub.
        assert_eq!(
            schemas[name]["type"], "object",
            "{name} should be a populated object schema, got {}",
            schemas[name]
        );
    }
    // Payment's enum field lowered its allowed values.
    let methods = &schemas["Payment"]["properties"]["method"]["enum"];
    assert!(
        methods.as_array().map_or(false, |a| a.iter().any(|v| v == "crypto")),
        "Payment.method enum not lowered: {}",
        schemas["Payment"]
    );
}
