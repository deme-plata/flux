// Endpoint discovery: walks a flux-graph WorkspaceGraph and maps known crate
// names to their REST surface.
//
// v0.12-B (rocky, 2026-05-29) — known_patterns now returns rich `EndpointSpec`s
// with real parameters and request bodies, not bare `(method, path, summary)`
// tuples. Path params (`/pages/{id}`) are auto-extracted; query params and
// JSON bodies are declared explicitly per-pattern. Existing public API
// `discover_endpoints` -> `Vec<ApiEndpoint>` is unchanged.
//
// v0.13 replaces this with `#[flux::api]` + `inventory::submit!` so endpoints
// declare themselves where they're defined.

use crate::schema::{
    ApiEndpoint, ApiParameter, ApiResponse, ApiSchema, HttpMethod, ParamLocation,
};

/// Compile-time descriptor submitted by `#[flux_api_macros::api(...)]`. Each
/// `#[api]`-annotated fn in any crate of the final binary appears in
/// `inventory::iter::<ApiEndpointDescriptor>()`. Fields are `&'static str`
/// because the proc-macro produces string literals.
#[derive(Debug, Clone, Copy)]
pub struct ApiEndpointDescriptor {
    pub method: &'static str,
    pub path: &'static str,
    pub summary: &'static str,
    pub operation_id: &'static str,
    pub crate_name: &'static str,
}

inventory::collect!(ApiEndpointDescriptor);

/// Drain every `#[api]`-registered endpoint into a `Vec<ApiEndpoint>`. Returns
/// the same shape as `discover_endpoints` so callers can mix the two streams
/// during the v0.13 transition (heuristic patterns + macro-registered).
pub fn discover_endpoints_static() -> Vec<ApiEndpoint> {
    inventory::iter::<ApiEndpointDescriptor>()
        .map(descriptor_to_endpoint)
        .collect()
}

pub(crate) fn descriptor_to_endpoint(d: &ApiEndpointDescriptor) -> ApiEndpoint {
    let method = match d.method {
        "GET" => HttpMethod::GET,
        "POST" => HttpMethod::POST,
        "PUT" => HttpMethod::PUT,
        "DELETE" => HttpMethod::DELETE,
        "PATCH" => HttpMethod::PATCH,
        // The macro rejects unknown methods at compile time; if a constructed
        // descriptor sneaks one in, default to GET rather than panicking — the
        // OpenAPI emitter still produces valid (if incorrect) output.
        _ => HttpMethod::GET,
    };
    ApiEndpoint {
        crate_name: d.crate_name.to_string(),
        method,
        path: d.path.to_string(),
        operation_id: d.operation_id.to_string(),
        summary: d.summary.to_string(),
        parameters: extract_path_params(d.path),
        request_body: None,
        responses: vec![ApiResponse {
            status: 200,
            description: "Success".into(),
            schema: None,
        }],
        tags: vec![d.crate_name.to_string()],
        middleware: None,
    }
}

pub fn discover_endpoints(ws: &flux_graph::WorkspaceGraph) -> Vec<ApiEndpoint> {
    ws.crates
        .iter()
        .filter(|ci| {
            let n = ci.name.to_lowercase();
            n.contains("api")
                || n.contains("route")
                || n.contains("wickes")
                || !known_patterns(&ci.name).is_empty()
        })
        .flat_map(heuristic_scan)
        .collect()
}

fn heuristic_scan(ci: &flux_graph::CrateInfo) -> Vec<ApiEndpoint> {
    known_patterns(&ci.name)
        .into_iter()
        .enumerate()
        .map(|(i, spec)| {
            // Merge auto-extracted path params with explicitly-declared ones.
            // Explicit declarations win on name collision so callers can
            // override the auto-string type with a typed schema.
            let mut params = extract_path_params(&spec.path);
            for p in spec.parameters {
                if let Some(slot) = params.iter_mut().find(|x| x.name == p.name) {
                    *slot = p;
                } else {
                    params.push(p);
                }
            }
            ApiEndpoint {
                crate_name: ci.name.clone(),
                method: spec.method,
                path: spec.path,
                operation_id: format!("{}_{}", ci.name.replace('-', "_"), i),
                summary: spec.summary,
                parameters: params,
                request_body: spec.request_body,
                responses: vec![ApiResponse {
                    status: 200,
                    description: "Success".into(),
                    schema: None,
                }],
                tags: vec![ci.name.clone()],
                middleware: None,
            }
        })
        .collect()
}

/// Internal endpoint descriptor used by `known_patterns`. Builder-style so
/// the pattern table reads top-to-bottom and rarely needs intermediate vars.
pub(crate) struct EndpointSpec {
    pub method: HttpMethod,
    pub path: String,
    pub summary: String,
    pub parameters: Vec<ApiParameter>,
    pub request_body: Option<ApiSchema>,
}

impl EndpointSpec {
    pub(crate) fn get(path: &str, summary: &str) -> Self {
        Self::new(HttpMethod::GET, path, summary)
    }
    pub(crate) fn post(path: &str, summary: &str) -> Self {
        Self::new(HttpMethod::POST, path, summary)
    }
    fn new(method: HttpMethod, path: &str, summary: &str) -> Self {
        Self {
            method,
            path: path.into(),
            summary: summary.into(),
            parameters: vec![],
            request_body: None,
        }
    }
    pub(crate) fn with_body(mut self, body: ApiSchema) -> Self {
        self.request_body = Some(body);
        self
    }
    pub(crate) fn with_query(
        mut self,
        name: &str,
        schema: ApiSchema,
        required: bool,
        description: &str,
    ) -> Self {
        self.parameters.push(ApiParameter {
            name: name.into(),
            location: ParamLocation::Query,
            required,
            schema,
            description: description.into(),
        });
        self
    }
}

/// Pull `{name}` segments from a path and lift each into a required Path
/// parameter of type `string`. Patterns are free to override via `with_query`
/// or by adding an ApiParameter with the same name (in which case the
/// explicit declaration wins).
fn extract_path_params(path: &str) -> Vec<ApiParameter> {
    let mut out = vec![];
    let mut chars = path.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '{' {
            continue;
        }
        let mut name = String::new();
        for c in chars.by_ref() {
            if c == '}' {
                break;
            }
            name.push(c);
        }
        if !name.is_empty() {
            out.push(ApiParameter {
                name,
                location: ParamLocation::Path,
                required: true,
                schema: ApiSchema::string(),
                description: String::new(),
            });
        }
    }
    out
}

/// Named schema definitions for a known crate — the concrete shape behind the
/// `Ref { name }`s that `known_patterns` attaches as request/response bodies.
/// `generate_openapi_with_schemas` lowers these into `components/schemas`
/// instead of the empty `{}` stubs the plain emitter produces.
pub(crate) fn known_schemas(name: &str) -> Vec<(String, ApiSchema)> {
    use ApiSchema as S;
    let status_enum = |values: &[&str]| S::Enum {
        ty: crate::schema::PrimType::String,
        values: values.iter().map(|v| serde_json::Value::String((*v).into())).collect(),
    };
    match name {
        "wickes-cms" => vec![(
            "Page".to_string(),
            S::object()
                .req_prop("id", S::string_with_format("uuid"))
                .req_prop("title", S::string())
                .prop("status", status_enum(&["draft", "published", "archived"]))
                .prop("body", S::string().nullable())
                .build(),
        )],
        "wickes-erp" => vec![(
            "Order".to_string(),
            S::object()
                .req_prop("id", S::string_with_format("uuid"))
                .req_prop("total", S::number())
                .prop("currency", S::string())
                .prop("items", S::array_of(S::ref_to("LineItem")))
                .build(),
        ), (
            "LineItem".to_string(),
            S::object()
                .req_prop("sku", S::string())
                .req_prop("qty", S::integer())
                .build(),
        )],
        "wickes-finance" => vec![(
            "Payment".to_string(),
            S::object()
                .req_prop("invoice_id", S::string_with_format("uuid"))
                .req_prop("amount", S::number())
                .prop("method", status_enum(&["card", "bank", "crypto"]))
                .build(),
        )],
        _ => vec![],
    }
}

/// Collect the named schema definitions for every crate in `ws` into one
/// registry, suitable for [`crate::openapi::generate_openapi_with_schemas`].
pub fn discover_schemas(ws: &flux_graph::WorkspaceGraph) -> std::collections::BTreeMap<String, ApiSchema> {
    let mut out = std::collections::BTreeMap::new();
    for ci in &ws.crates {
        for (name, schema) in known_schemas(&ci.name) {
            out.entry(name).or_insert(schema);
        }
    }
    out
}

pub(crate) fn known_patterns(name: &str) -> Vec<EndpointSpec> {
    match name {
        "wickes-cms" => vec![
            EndpointSpec::get("/api/pages", "List pages")
                .with_query("limit", ApiSchema::integer(), false, "Max rows to return"),
            EndpointSpec::post("/api/pages", "Create page")
                .with_body(ApiSchema::ref_to("Page")),
            EndpointSpec::get("/api/pages/{id}", "Get page"),
        ],
        "wickes-erp" => vec![
            EndpointSpec::get("/api/inventory", "List inventory"),
            EndpointSpec::post("/api/orders", "Create order")
                .with_body(ApiSchema::ref_to("Order")),
        ],
        "wickes-finance" => vec![
            EndpointSpec::get("/api/invoices", "List invoices"),
            EndpointSpec::post("/api/invoices/{id}/pay", "Pay invoice")
                .with_body(ApiSchema::ref_to("Payment")),
        ],
        "flux-ue-bridge" => vec![
            EndpointSpec::post(
                "/v1/webhook",
                "Receive a fluxc-mcp webhook event and broadcast to subscribers",
            )
            .with_body(
                ApiSchema::object()
                    .req_prop("event", ApiSchema::string())
                    .prop("data", ApiSchema::object().build())
                    .build(),
            ),
            EndpointSpec::get(
                "/v1/workspace",
                "Workspace topology snapshot (crate list with score/loc)",
            ),
            EndpointSpec::get(
                "/v1/events",
                "WebSocket: live event stream from the bridge",
            ),
        ],
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_param_extraction_single() {
        let params = extract_path_params("/api/pages/{id}");
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "id");
        assert_eq!(params[0].location, ParamLocation::Path);
        assert!(params[0].required);
    }

    #[test]
    fn path_param_extraction_multiple() {
        let params = extract_path_params("/api/orgs/{org_id}/users/{user_id}");
        let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["org_id", "user_id"]);
    }

    #[test]
    fn path_param_extraction_none_for_static_path() {
        assert!(extract_path_params("/api/pages").is_empty());
        assert!(extract_path_params("/v1/webhook").is_empty());
    }

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
    fn wickes_cms_get_pages_carries_query_param() {
        let eps = discover_endpoints(&ws(&["wickes-cms"]));
        let list = eps
            .iter()
            .find(|e| e.method == HttpMethod::GET && e.path == "/api/pages")
            .expect("missing GET /api/pages");
        let limit = list
            .parameters
            .iter()
            .find(|p| p.name == "limit")
            .expect("missing limit query param");
        assert_eq!(limit.location, ParamLocation::Query);
        assert!(!limit.required);
        assert!(matches!(limit.schema, ApiSchema::Primitive { .. }));
    }

    #[test]
    fn wickes_cms_get_page_id_has_path_param() {
        let eps = discover_endpoints(&ws(&["wickes-cms"]));
        let get_one = eps
            .iter()
            .find(|e| e.path == "/api/pages/{id}")
            .expect("missing GET /api/pages/{id}");
        let id = get_one
            .parameters
            .iter()
            .find(|p| p.name == "id")
            .expect("missing id path param");
        assert_eq!(id.location, ParamLocation::Path);
        assert!(id.required);
    }

    #[test]
    fn wickes_cms_post_has_ref_body() {
        let eps = discover_endpoints(&ws(&["wickes-cms"]));
        let create = eps
            .iter()
            .find(|e| e.method == HttpMethod::POST && e.path == "/api/pages")
            .expect("missing POST /api/pages");
        match &create.request_body {
            Some(ApiSchema::Ref { name }) => assert_eq!(name, "Page"),
            other => panic!("expected Ref body, got {other:?}"),
        }
    }

    #[test]
    fn flux_ue_bridge_webhook_body_is_object() {
        let eps = discover_endpoints(&ws(&["flux-ue-bridge"]));
        let webhook = eps
            .iter()
            .find(|e| e.path == "/v1/webhook")
            .expect("missing POST /v1/webhook");
        match &webhook.request_body {
            Some(ApiSchema::Object { properties, required }) => {
                assert!(properties.contains_key("event"));
                assert!(properties.contains_key("data"));
                assert_eq!(required, &vec!["event".to_string()]);
            }
            other => panic!("expected Object body, got {other:?}"),
        }
    }

    #[test]
    fn flux_ue_bridge_get_endpoints_have_no_params() {
        let eps = discover_endpoints(&ws(&["flux-ue-bridge"]));
        for path in ["/v1/workspace", "/v1/events"] {
            let ep = eps.iter().find(|e| e.path == path).expect(path);
            assert!(ep.parameters.is_empty(), "expected no params on GET {path}");
            assert!(ep.request_body.is_none(), "expected no body on GET {path}");
        }
    }

    #[test]
    fn descriptor_to_endpoint_lowers_method_string() {
        let d = ApiEndpointDescriptor {
            method: "POST",
            path: "/api/x/{id}",
            summary: "create x",
            operation_id: "create_x",
            crate_name: "test-crate",
        };
        let ep = descriptor_to_endpoint(&d);
        assert_eq!(ep.method, HttpMethod::POST);
        assert_eq!(ep.path, "/api/x/{id}");
        assert_eq!(ep.operation_id, "create_x");
        assert_eq!(ep.tags, vec!["test-crate".to_string()]);
        // path-param auto-extraction kicks in here too
        assert!(ep.parameters.iter().any(|p| p.name == "id" && p.location == ParamLocation::Path));
    }

    #[test]
    fn descriptor_with_unknown_method_defaults_to_get() {
        // Defensive: this can't happen through the macro (it validates), but
        // a hand-constructed descriptor with garbage shouldn't panic.
        let d = ApiEndpointDescriptor {
            method: "TEAPOT",
            path: "/teapot",
            summary: "",
            operation_id: "brew",
            crate_name: "kettle",
        };
        let ep = descriptor_to_endpoint(&d);
        assert_eq!(ep.method, HttpMethod::GET);
    }

    #[test]
    fn discover_endpoints_static_returns_a_vec() {
        // We don't register any #[api] handlers from inside the unit-test
        // binary, so the iter is typically empty — but the call must compile
        // and return a Vec<ApiEndpoint> without panicking.
        let _eps: Vec<ApiEndpoint> = discover_endpoints_static();
    }

    #[test]
    fn wickes_finance_pay_endpoint_has_id_param_and_body() {
        let eps = discover_endpoints(&ws(&["wickes-finance"]));
        let pay = eps
            .iter()
            .find(|e| e.path == "/api/invoices/{id}/pay")
            .expect("missing POST /api/invoices/{id}/pay");
        assert!(pay.parameters.iter().any(|p| p.name == "id" && p.location == ParamLocation::Path));
        assert!(matches!(pay.request_body, Some(ApiSchema::Ref { .. })));
    }
}
