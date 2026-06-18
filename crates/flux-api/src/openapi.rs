// OpenAPI 3.1 emitter.
//
// v0.12-C (rocky, 2026-05-29) — rewrote the paths-only stub to a real
// OpenAPI 3.1 document:
//   * `components/schemas` populated from every `ApiSchema::Ref { name }`
//     reachable from the endpoint list (empty `{}` schemas until v0.13
//     supplies real bodies via `#[flux::api]`).
//   * Operation-level `parameters: [...]` with `in` + `required` + lowered
//     `schema`.
//   * `requestBody: { content: { application/json: { schema } } }` when
//     `ApiEndpoint::request_body` is set.
//   * `responses` with `application/json` content when the response has a
//     schema, plain description otherwise.
//
// JSON Schema lowering follows draft 2020-12 / OpenAPI 3.1 conventions:
//   * Nullable lowers to `oneOf: [<inner>, { type: "null" }]` so it stays
//     valid even when `<inner>` is a Ref or compound shape.
//   * OneOf passes through unchanged.

use crate::schema::*;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Emit an OpenAPI 3.1 document with **stub** component schemas — every
/// `Ref { name }` reachable from the endpoints becomes an empty `{}` schema.
///
/// This is the back-compat entry point: callers that hand-build endpoint specs
/// with named refs but no definitions (flux-0x, flux-cmc, the MCP `openapi`
/// format) get exactly the document they got before. To emit **real** schema
/// definitions, supply a registry via [`generate_openapi_with_schemas`].
pub fn generate_openapi(
    title: &str,
    version: &str,
    endpoints: &[ApiEndpoint],
) -> Value {
    generate_openapi_with_schemas(title, version, endpoints, &BTreeMap::new())
}

/// Emit an OpenAPI 3.1 document, resolving `Ref { name }` against `defs`
/// (a `name -> ApiSchema` registry, e.g. from [`crate::discover::discover_schemas`]).
///
/// Resolution is a transitive closure: a definition that itself references
/// another schema pulls that one in too (e.g. an `Order` whose `items` are
/// `Ref("LineItem")` causes `LineItem` to be emitted). Any ref with no entry in
/// `defs` falls back to a permissive `{}` stub so the document stays valid
/// rather than dangling.
pub fn generate_openapi_with_schemas(
    title: &str,
    version: &str,
    endpoints: &[ApiEndpoint],
    defs: &BTreeMap<String, ApiSchema>,
) -> Value {
    let mut paths = Map::new();
    for ep in endpoints {
        let pe = paths
            .entry(ep.path.clone())
            .or_insert_with(|| json!({}));
        let pe_obj = pe.as_object_mut().expect("path entry is an object");
        let mk = method_key(&ep.method);
        pe_obj.insert(mk.into(), build_operation(ep));
    }

    // Seed the worklist with every ref reachable from the endpoint surface.
    let mut queue: Vec<String> = Vec::new();
    {
        let mut seed = BTreeSet::new();
        for ep in endpoints {
            for p in &ep.parameters {
                collect_refs(&p.schema, &mut seed);
            }
            if let Some(body) = &ep.request_body {
                collect_refs(body, &mut seed);
            }
            for r in &ep.responses {
                if let Some(s) = &r.schema {
                    collect_refs(s, &mut seed);
                }
            }
        }
        queue.extend(seed);
    }

    // Transitive closure: resolve each name against `defs`, lowering its real
    // definition and queueing any nested refs it introduces.
    let mut components_schemas = Map::new();
    let mut done = BTreeSet::new();
    while let Some(name) = queue.pop() {
        if !done.insert(name.clone()) {
            continue;
        }
        match defs.get(&name) {
            Some(def) => {
                components_schemas.insert(name.clone(), schema_to_json_schema(def));
                let mut nested = BTreeSet::new();
                collect_refs(def, &mut nested);
                for r in nested {
                    if !done.contains(&r) {
                        queue.push(r);
                    }
                }
            }
            None => {
                // No definition supplied — keep the v0.12 permissive stub so the
                // $ref still resolves to a (vacuous) schema instead of dangling.
                components_schemas.insert(name.clone(), json!({}));
            }
        }
    }

    let mut doc = json!({
        "openapi": "3.1.0",
        "info": { "title": title, "version": version },
        "paths": paths,
    });
    if !components_schemas.is_empty() {
        doc["components"] = json!({ "schemas": components_schemas });
    }
    doc
}

fn build_operation(ep: &ApiEndpoint) -> Value {
    let mut op = json!({
        "operationId": ep.operation_id,
        "summary": ep.summary,
        "tags": ep.tags,
    });
    let op_obj = op.as_object_mut().expect("operation is an object");

    if !ep.parameters.is_empty() {
        let params: Vec<Value> = ep
            .parameters
            .iter()
            .map(|p| {
                json!({
                    "name": p.name,
                    "in": param_location_key(&p.location),
                    "required": p.required,
                    "description": p.description,
                    "schema": schema_to_json_schema(&p.schema),
                })
            })
            .collect();
        op_obj.insert("parameters".into(), Value::Array(params));
    }

    if let Some(body) = &ep.request_body {
        op_obj.insert(
            "requestBody".into(),
            json!({
                "required": true,
                "content": {
                    "application/json": { "schema": schema_to_json_schema(body) }
                }
            }),
        );
    }

    let mut responses = Map::new();
    if ep.responses.is_empty() {
        responses.insert(
            "200".into(),
            json!({ "description": "Success" }),
        );
    } else {
        for r in &ep.responses {
            let mut entry = json!({ "description": r.description });
            if let Some(s) = &r.schema {
                entry["content"] = json!({
                    "application/json": { "schema": schema_to_json_schema(s) }
                });
            }
            responses.insert(r.status.to_string(), entry);
        }
    }
    op_obj.insert("responses".into(), Value::Object(responses));

    op
}

fn method_key(m: &HttpMethod) -> &'static str {
    match m {
        HttpMethod::GET => "get",
        HttpMethod::POST => "post",
        HttpMethod::PUT => "put",
        HttpMethod::DELETE => "delete",
        HttpMethod::PATCH => "patch",
    }
}

fn param_location_key(l: &ParamLocation) -> &'static str {
    match l {
        ParamLocation::Path => "path",
        ParamLocation::Query => "query",
        ParamLocation::Header => "header",
    }
}

/// Lower an ApiSchema to JSON Schema (draft 2020-12 / OpenAPI 3.1 dialect).
pub(crate) fn schema_to_json_schema(s: &ApiSchema) -> Value {
    match s {
        ApiSchema::Primitive { ty, format } => {
            let mut v = json!({ "type": ty.as_str() });
            if let Some(f) = format {
                v["format"] = json!(f);
            }
            v
        }
        ApiSchema::Object { properties, required } => {
            let props: Map<String, Value> = properties
                .iter()
                .map(|(k, v)| (k.clone(), schema_to_json_schema(v)))
                .collect();
            let mut v = json!({
                "type": "object",
                "properties": Value::Object(props),
            });
            if !required.is_empty() {
                v["required"] = json!(required);
            }
            v
        }
        ApiSchema::Array { items } => {
            json!({
                "type": "array",
                "items": schema_to_json_schema(items),
            })
        }
        ApiSchema::Enum { ty, values } => {
            json!({
                "type": ty.as_str(),
                "enum": values,
            })
        }
        ApiSchema::OneOf { variants } => {
            let vs: Vec<Value> = variants.iter().map(schema_to_json_schema).collect();
            json!({ "oneOf": vs })
        }
        ApiSchema::Ref { name } => {
            json!({ "$ref": format!("#/components/schemas/{name}") })
        }
        ApiSchema::Nullable { inner } => {
            // 2020-12 union via oneOf — works for any inner shape including
            // refs + compound types, where `type: ["X", "null"]` doesn't.
            json!({
                "oneOf": [ schema_to_json_schema(inner), { "type": "null" } ]
            })
        }
    }
}

fn collect_refs(s: &ApiSchema, out: &mut BTreeSet<String>) {
    match s {
        ApiSchema::Ref { name } => {
            out.insert(name.clone());
        }
        ApiSchema::Object { properties, .. } => {
            for v in properties.values() {
                collect_refs(v, out);
            }
        }
        ApiSchema::Array { items } => collect_refs(items, out),
        ApiSchema::OneOf { variants } => {
            for v in variants {
                collect_refs(v, out);
            }
        }
        ApiSchema::Nullable { inner } => collect_refs(inner, out),
        ApiSchema::Primitive { .. } | ApiSchema::Enum { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::{discover_endpoints, discover_schemas};

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
    fn primitive_lowering() {
        let v = schema_to_json_schema(&ApiSchema::string_with_format("uuid"));
        assert_eq!(v["type"], "string");
        assert_eq!(v["format"], "uuid");
    }

    #[test]
    fn object_lowering_emits_required() {
        let s = ApiSchema::object()
            .req_prop("id", ApiSchema::string())
            .prop("nick", ApiSchema::string())
            .build();
        let v = schema_to_json_schema(&s);
        assert_eq!(v["type"], "object");
        assert_eq!(v["required"], json!(["id"]));
        assert_eq!(v["properties"]["id"]["type"], "string");
    }

    #[test]
    fn ref_lowering_uses_components_path() {
        let v = schema_to_json_schema(&ApiSchema::ref_to("Page"));
        assert_eq!(v["$ref"], "#/components/schemas/Page");
    }

    #[test]
    fn nullable_lowers_to_one_of_with_null() {
        let v = schema_to_json_schema(&ApiSchema::string().nullable());
        let alts = v["oneOf"].as_array().expect("oneOf array");
        assert_eq!(alts.len(), 2);
        assert!(alts.iter().any(|a| a["type"] == "string"));
        assert!(alts.iter().any(|a| a["type"] == "null"));
    }

    #[test]
    fn collect_refs_walks_nested() {
        let s = ApiSchema::object()
            .req_prop("page", ApiSchema::ref_to("Page"))
            .prop("orders", ApiSchema::array_of(ApiSchema::ref_to("Order")))
            .build();
        let mut refs = BTreeSet::new();
        collect_refs(&s, &mut refs);
        assert!(refs.contains("Page"));
        assert!(refs.contains("Order"));
        assert_eq!(refs.len(), 2);
    }

    #[test]
    fn openapi_for_wickes_cms_has_components_and_params() {
        let eps = discover_endpoints(&ws(&["wickes-cms"]));
        let doc = generate_openapi("Wickes CMS", "1.0", &eps);
        assert_eq!(doc["openapi"], "3.1.0");
        assert!(
            doc["components"]["schemas"].get("Page").is_some(),
            "expected Page in components.schemas: {doc}"
        );

        let list = &doc["paths"]["/api/pages"]["get"];
        let params = list["parameters"].as_array().expect("parameters array");
        assert!(params.iter().any(|p| p["name"] == "limit" && p["in"] == "query"));

        let get_one = &doc["paths"]["/api/pages/{id}"]["get"];
        let params = get_one["parameters"].as_array().expect("parameters array");
        assert!(
            params
                .iter()
                .any(|p| p["name"] == "id" && p["in"] == "path" && p["required"] == true)
        );

        let create = &doc["paths"]["/api/pages"]["post"];
        assert_eq!(
            create["requestBody"]["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/Page"
        );
    }

    #[test]
    fn openapi_for_flux_ue_bridge_inlines_webhook_body() {
        let eps = discover_endpoints(&ws(&["flux-ue-bridge"]));
        let doc = generate_openapi("flux-ue-bridge", "0.11.0", &eps);
        let webhook = &doc["paths"]["/v1/webhook"]["post"];
        let body = &webhook["requestBody"]["content"]["application/json"]["schema"];
        assert_eq!(body["type"], "object");
        assert!(body["properties"]["event"].get("type").is_some());
        assert_eq!(body["required"], json!(["event"]));
    }

    #[test]
    fn openapi_doc_round_trips_through_json() {
        let eps = discover_endpoints(&ws(&["wickes-cms", "flux-ue-bridge"]));
        let doc = generate_openapi("Mix", "0.1.0", &eps);
        let s = serde_json::to_string(&doc).expect("serialize");
        let v: Value = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(v["openapi"], "3.1.0");
    }

    #[test]
    fn plain_emitter_still_stubs_refs() {
        // Back-compat: with no registry, every ref is the permissive `{}` stub.
        let g = ws(&["wickes-cms"]);
        let doc = generate_openapi("Wickes", "1.0", &discover_endpoints(&g));
        assert_eq!(doc["components"]["schemas"]["Page"], json!({}));
    }

    #[test]
    fn with_schemas_emits_real_definition() {
        let g = ws(&["wickes-cms"]);
        let doc = generate_openapi_with_schemas(
            "Wickes",
            "1.0",
            &discover_endpoints(&g),
            &discover_schemas(&g),
        );
        let page = &doc["components"]["schemas"]["Page"];
        assert_eq!(page["type"], "object", "Page should be a real object schema: {doc}");
        assert_eq!(page["properties"]["id"]["type"], "string");
        assert_eq!(page["properties"]["id"]["format"], "uuid");
        assert_eq!(page["required"], json!(["id", "title"]));
        // The enum field lowered its allowed values.
        let status = &page["properties"]["status"];
        assert_eq!(status["type"], "string");
        assert!(status["enum"].as_array().unwrap().iter().any(|v| v == "published"));
        // Nullable body lowered to oneOf[…, null].
        assert!(page["properties"]["body"]["oneOf"].is_array());
    }

    #[test]
    fn with_schemas_resolves_transitive_refs() {
        // Order.items is an array of Ref("LineItem") — LineItem must be pulled
        // into components even though no endpoint references it directly.
        let g = ws(&["wickes-erp"]);
        let doc = generate_openapi_with_schemas(
            "ERP",
            "1.0",
            &discover_endpoints(&g),
            &discover_schemas(&g),
        );
        let schemas = doc["components"]["schemas"].as_object().unwrap();
        assert!(schemas.contains_key("Order"), "Order missing: {doc}");
        assert!(schemas.contains_key("LineItem"), "transitive LineItem missing: {doc}");
        assert_eq!(
            doc["components"]["schemas"]["Order"]["properties"]["items"]["items"]["$ref"],
            "#/components/schemas/LineItem"
        );
        assert_eq!(doc["components"]["schemas"]["LineItem"]["type"], "object");
    }

    #[test]
    fn with_schemas_falls_back_to_stub_for_unknown_ref() {
        // A ref the registry doesn't define still resolves to a `{}` stub.
        let eps = vec![ApiEndpoint {
            crate_name: "x".into(),
            method: HttpMethod::POST,
            path: "/x".into(),
            operation_id: "x".into(),
            summary: "".into(),
            parameters: vec![],
            request_body: Some(ApiSchema::ref_to("Mystery")),
            responses: vec![],
            tags: vec!["x".into()],
            middleware: None,
        }];
        let doc = generate_openapi_with_schemas("X", "1.0", &eps, &std::collections::BTreeMap::new());
        assert_eq!(doc["components"]["schemas"]["Mystery"], json!({}));
    }
}
