// End-to-end test for v0.13 macro pipeline.
//
// Proves that `#[flux_api::api(METHOD, "/path"[, summary = "..."])]` applied to
// a handler fn results in a fully-formed `ApiEndpoint` appearing in
// `discover_endpoints_static()` at runtime. Path params auto-extract; unknown
// methods are rejected at compile time (covered separately by trybuild-style
// tests in v0.14+).
//
// Note: this is an integration test (under `tests/`), so it builds its own
// binary that links flux-api. The inventory submissions made by the `#[api]`s
// in this file land in THIS binary's static slice — `discover_endpoints_static`
// reads them at runtime.

// The handler fns are never called directly — they exist only so the
// `#[flux_api::api]` attribute can run and submit their descriptors into
// inventory at compile time. Silence the (correct) dead-code lint.
#![allow(dead_code)]

use flux_api::{discover_endpoints_static, HttpMethod, ParamLocation};

#[flux_api::api(GET, "/test/ping")]
fn handler_ping() {}

#[flux_api::api(POST, "/test/echo", summary = "Echo the request body")]
fn handler_echo() {}

#[flux_api::api(GET, "/test/items/{id}")]
fn handler_get_item() {}

#[flux_api::api(DELETE, "/test/items/{id}")]
fn handler_delete_item() {}

#[flux_api::api(PUT, "/test/orgs/{org_id}/users/{user_id}")]
fn handler_put_user() {}

#[test]
fn macro_registered_endpoints_are_discovered() {
    let eps = discover_endpoints_static();
    let paths: Vec<&str> = eps.iter().map(|e| e.path.as_str()).collect();

    // The static inventory is binary-scoped; in this test binary it contains
    // exactly the five we registered above (plus possibly any submissions made
    // by other in-binary code — there are none today).
    for expected in [
        "/test/ping",
        "/test/echo",
        "/test/items/{id}",
        "/test/orgs/{org_id}/users/{user_id}",
    ] {
        assert!(
            paths.contains(&expected),
            "missing {expected} from registered endpoints; got {paths:?}"
        );
    }
}

#[test]
fn macro_carries_method_and_summary_through() {
    let eps = discover_endpoints_static();

    let echo = eps
        .iter()
        .find(|e| e.path == "/test/echo")
        .expect("echo not registered");
    assert_eq!(echo.method, HttpMethod::POST);
    assert_eq!(echo.summary, "Echo the request body");
    assert_eq!(echo.operation_id, "handler_echo");
    assert_eq!(echo.crate_name, "flux-api"); // CARGO_PKG_NAME at expansion site

    let ping = eps
        .iter()
        .find(|e| e.path == "/test/ping")
        .expect("ping not registered");
    assert_eq!(ping.method, HttpMethod::GET);
    // No explicit summary → falls back to operation_id (the fn name).
    assert_eq!(ping.summary, "handler_ping");
}

#[test]
fn macro_path_params_auto_extract() {
    let eps = discover_endpoints_static();

    let get_item = eps
        .iter()
        .find(|e| e.path == "/test/items/{id}" && e.method == HttpMethod::GET)
        .expect("get_item not registered");
    let id = get_item
        .parameters
        .iter()
        .find(|p| p.name == "id")
        .expect("expected `id` path param");
    assert_eq!(id.location, ParamLocation::Path);
    assert!(id.required);
}

#[test]
fn macro_multiple_path_params_are_both_extracted() {
    let eps = discover_endpoints_static();

    let put_user = eps
        .iter()
        .find(|e| e.path == "/test/orgs/{org_id}/users/{user_id}")
        .expect("put_user not registered");
    let names: Vec<&str> = put_user.parameters.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"org_id"), "missing org_id: {names:?}");
    assert!(names.contains(&"user_id"), "missing user_id: {names:?}");
    assert!(put_user.parameters.iter().all(|p| p.location == ParamLocation::Path));
}

#[test]
fn distinct_methods_on_same_path_both_register() {
    let eps = discover_endpoints_static();

    // /test/items/{id} appears twice: once for GET, once for DELETE.
    let item_endpoints: Vec<_> = eps
        .iter()
        .filter(|e| e.path == "/test/items/{id}")
        .collect();
    assert_eq!(item_endpoints.len(), 2, "expected GET + DELETE on /test/items/{{id}}");
    assert!(item_endpoints.iter().any(|e| e.method == HttpMethod::GET));
    assert!(item_endpoints.iter().any(|e| e.method == HttpMethod::DELETE));
}
