// flux-api — Stainless-style API Builder (Phase 3a)
// OpenAPI 3.1 spec generation, multi-language SDK stubs, flux-graph endpoint
// discovery, and the discriminated-union event-surface generator that's the
// roadmap moat (see docs/flux-api-roadmap.md, v0.16).
//
// Module layout (v0.12 pre-step, rocky-owned, 2026-05-29):
//   schema      — endpoint / parameter / response / body types
//   discover    — walk a flux-graph WorkspaceGraph → Vec<ApiEndpoint>
//   openapi     — OpenAPI 3.1 JSON emission
//   sdk_ts      — TypeScript REST client codegen
//   sdk_python  — Python (httpx) REST client codegen
//   event_types — WebSocket discriminated-union codegen (TS today)
//   casing      — private case helpers shared by SDK generators

mod casing;
mod codegen;
pub mod discover;
pub mod event_types;
pub mod openapi;
pub mod schema;
pub mod diff;
pub mod docsite;
pub mod middleware;
pub mod publish;
pub mod sdk_chrome;
pub mod sdk_go;
pub mod sdk_kotlin;
pub mod sdk_python;
pub mod sdk_rust;
pub mod sdk_ts;
pub mod webhook_sdk;

pub use discover::{
    discover_endpoints, discover_endpoints_static, discover_schemas, ApiEndpointDescriptor,
};

/// Re-export `inventory` so `flux_api_macros::api` can emit
/// `::flux_api::inventory::submit!` without forcing user crates to take a
/// direct dep on `inventory`.
pub use inventory;

/// Re-export the `#[api]` attribute macro for ergonomic use as
/// `#[flux_api::api(...)]`.
pub use flux_api_macros::api;
pub use event_types::{
    flux_ue_bridge_events, generate_go_event_types, generate_kotlin_event_types,
    generate_python_event_types, generate_rust_event_types, generate_ts_event_types,
    EventVariant,
};
pub use webhook_sdk::{generate_python_webhook_handler, generate_ts_webhook_handler};
pub use openapi::{generate_openapi, generate_openapi_with_schemas};
pub use schema::{
    ApiEndpoint, ApiParameter, ApiResponse, ApiSchema, HttpMethod, ParamLocation,
};
pub use diff::{classify_diff, SchemaChange};
pub use docsite::render_docsite;
pub use middleware::{
    AuthKind, MiddlewareSpec, PaginationStyle, RetryPolicy, StreamKind,
};
pub use publish::{plan_publish, publish_dry_run, Language, PublishPlan, SdkBundle};
pub use sdk_chrome::{generate_chrome_extension, ChromeExtensionBundle};
pub use sdk_go::{generate_go_sdk, generate_go_sdk_with_types};
pub use sdk_kotlin::{generate_kotlin_sdk, generate_kotlin_sdk_with_types};
pub use sdk_python::{generate_python_sdk, generate_python_sdk_with_types};
pub use sdk_rust::{generate_rust_client_sdk, generate_rust_client_sdk_with_types};
pub use sdk_ts::{generate_typescript_sdk, generate_typescript_sdk_with_types};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::casing::camel_case;

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

    #[test]
    fn discovers_flux_ue_bridge_endpoints() {
        let g = ws(&["flux-ue-bridge", "flux-core"]);
        let eps = discover_endpoints(&g);
        let paths: Vec<&str> = eps.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"/v1/webhook"), "missing /v1/webhook: got {paths:?}");
        assert!(paths.contains(&"/v1/workspace"));
        assert!(paths.contains(&"/v1/events"));
        assert!(eps.iter().all(|e| e.crate_name == "flux-ue-bridge"));
        assert!(eps.iter().all(|e| e.tags.contains(&"flux-ue-bridge".to_string())));
    }

    #[test]
    fn unknown_crate_yields_no_endpoints() {
        let g = ws(&["flux-totally-unknown"]);
        assert!(discover_endpoints(&g).is_empty());
    }

    #[test]
    fn openapi_groups_paths_by_method() {
        let g = ws(&["wickes-cms"]);
        let spec = generate_openapi("Wickes", "1.0", &discover_endpoints(&g));
        assert_eq!(spec["openapi"], "3.1.0");
        assert_eq!(spec["info"]["title"], "Wickes");
        let pages = &spec["paths"]["/api/pages"];
        assert!(pages.get("get").is_some(), "GET /api/pages missing: {spec}");
        assert!(pages.get("post").is_some(), "POST /api/pages missing");
        assert!(spec["paths"]["/api/pages/{id}"].get("get").is_some());
    }

    #[test]
    fn ts_sdk_emits_client_class_per_tag() {
        let g = ws(&["flux-ue-bridge"]);
        let eps = discover_endpoints(&g);
        let sdk = generate_typescript_sdk(&eps, "http://localhost:9989");
        assert!(sdk.contains("export class FluxUeBridgeClient"));
        for ep in &eps {
            let fname = camel_case(&ep.operation_id);
            assert!(sdk.contains(&format!("async {fname}")), "missing method {fname} in:\n{sdk}");
        }
        assert!(sdk.contains("http://localhost:9989"));
    }

    #[test]
    fn python_sdk_emits_one_class_per_tag() {
        let g = ws(&["flux-ue-bridge"]);
        let sdk = generate_python_sdk(&discover_endpoints(&g), "http://localhost:9989");
        assert!(sdk.contains("class FluxUeBridgeClient:"));
        assert!(sdk.contains("import httpx"));
        assert!(sdk.contains("http://localhost:9989"));
    }

    #[test]
    fn bridge_event_surface_is_complete() {
        let v = flux_ue_bridge_events();
        let tags: Vec<&str> = v.iter().map(|e| e.tag.as_str()).collect();
        for expected in ["iterate_complete", "test_complete", "build_failed", "hello", "heartbeat"] {
            assert!(tags.contains(&expected), "missing event tag {expected}: got {tags:?}");
        }
    }

    #[test]
    fn ts_event_types_produce_discriminated_union() {
        let ts = generate_ts_event_types("BridgeEvent", &flux_ue_bridge_events());
        assert!(ts.contains("export interface BridgeEvent_IterateComplete"));
        assert!(ts.contains("type: \"iterate_complete\""));
        assert!(ts.contains("compile_ms: number;"));
        assert!(ts.contains("success: boolean;"));
        assert!(ts.contains("error_excerpt: string | null;"));
        assert!(ts.contains("export type BridgeEvent ="));
        assert!(ts.contains("| BridgeEvent_Heartbeat"));
    }
}
