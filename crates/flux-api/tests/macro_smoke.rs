// v0.13-A integration smoke test: annotate stub handlers with #[flux_api::api]
// here in the test binary's own crate, then verify both
// `inventory::iter::<ApiEndpointDescriptor>` AND
// `flux_api::discover_endpoints_static()` see the submission.
//
// The handlers themselves do nothing — the macro's whole job is the sibling
// submit!() block. The fn body just keeps the macro pointed at a real fn item.

// The annotated fns are intentionally unused — the macro's `inventory::submit!`
// is the load-bearing side effect, not the body.
#![allow(dead_code)]

use flux_api::{discover_endpoints_static, ApiEndpointDescriptor, HttpMethod};

#[flux_api::api(GET, "/v1/test/widgets", summary = "List widgets (test stub)")]
fn list_widgets() {}

#[flux_api::api(POST, "/v1/test/widgets/{id}", summary = "Update a widget (test stub)")]
fn update_widget() {}

#[flux_api::api(DELETE, "/v1/test/widgets/{id}")]
fn delete_widget() {}

#[test]
fn macro_emits_inventory_submissions() {
    let descriptors: Vec<&'static ApiEndpointDescriptor> =
        flux_api::inventory::iter::<ApiEndpointDescriptor>().collect();
    assert!(
        descriptors.iter().any(|d| d.path == "/v1/test/widgets" && d.method == "GET"),
        "GET /v1/test/widgets missing — got {:?}",
        descriptors.iter().map(|d| (d.method, d.path)).collect::<Vec<_>>()
    );
    assert!(descriptors
        .iter()
        .any(|d| d.path == "/v1/test/widgets/{id}" && d.method == "POST"));
    assert!(descriptors
        .iter()
        .any(|d| d.path == "/v1/test/widgets/{id}" && d.method == "DELETE"));
}

#[test]
fn discover_endpoints_static_lowers_inventory_to_api_endpoints() {
    let endpoints = discover_endpoints_static();

    let list = endpoints
        .iter()
        .find(|e| e.path == "/v1/test/widgets" && e.method == HttpMethod::GET)
        .expect("GET /v1/test/widgets missing from discover_endpoints_static");
    assert_eq!(list.operation_id, "list_widgets");
    assert_eq!(list.summary, "List widgets (test stub)");
    // No path params on a static path.
    assert!(list.parameters.is_empty());

    let update = endpoints
        .iter()
        .find(|e| e.path == "/v1/test/widgets/{id}" && e.method == HttpMethod::POST)
        .expect("POST /v1/test/widgets/{id} missing");
    // Auto-extracted path param "id" should be present and required.
    let id = update
        .parameters
        .iter()
        .find(|p| p.name == "id")
        .expect("id path param missing");
    assert!(id.required);

    let del = endpoints
        .iter()
        .find(|e| e.path == "/v1/test/widgets/{id}" && e.method == HttpMethod::DELETE)
        .expect("DELETE /v1/test/widgets/{id} missing");
    // When `summary = "..."` is omitted, the macro defaults to the fn name
    // (operation_id). Matches the linter's macro implementation.
    assert_eq!(del.summary, "delete_widget");

    // crate_name is set by env!("CARGO_PKG_NAME") at user-crate compile
    // time. For an integration test, the env points at the host crate
    // ("flux-api") — which is what we expect.
    for ep in [&list, &update, &del] {
        assert_eq!(ep.crate_name, "flux-api");
        assert_eq!(ep.tags, vec!["flux-api".to_string()]);
    }
}

#[test]
fn unknown_method_would_fail_compile() {
    // The macro rejects unknown methods at parse time. This test is here as
    // documentation; trybuild coverage of compile-fail cases is v0.13-D.
    let _doc = "see trybuild tests in v0.13-D";
}
