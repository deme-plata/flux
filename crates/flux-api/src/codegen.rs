// Shared, language-agnostic SDK-codegen helpers.
//
// Before this module the five SDK generators each emitted zero-argument methods
// and pasted the raw path into the URL — so a route like `/api/pages/{id}`
// produced `fetch(`${baseUrl}/api/pages/{id}`)` with the `{id}` placeholder
// never substituted, query params silently dropped, and POST bodies never sent.
// The generated clients were syntactically valid but semantically useless for
// any real endpoint.
//
// These helpers give every generator one consistent view of an endpoint's call
// shape — which params are path vs query, whether there's a body, the primitive
// type of each param, and a path-template renderer — so the per-language
// emitters only differ in syntax, not in semantics.

use crate::schema::{ApiEndpoint, ApiParameter, ApiSchema, ParamLocation, PrimType};
use std::collections::BTreeMap;

/// A `name -> definition` schema registry, as produced by
/// `crate::discover::discover_schemas` and consumed by the OpenAPI emitter and
/// the typed-SDK generators.
pub(crate) type SchemaDefs = BTreeMap<String, ApiSchema>;

/// If `schema` is an object, borrow its properties + required list.
pub(crate) fn as_object(schema: &ApiSchema) -> Option<(&BTreeMap<String, ApiSchema>, &Vec<String>)> {
    match schema {
        ApiSchema::Object { properties, required } => Some((properties, required)),
        _ => None,
    }
}

/// The schema name a request body refers to, iff it's a direct `Ref { name }`
/// present in `defs`. Inline objects, primitives, and unknown refs return
/// `None` — those keep the generator's open body type (`any`/`Value`/…).
pub(crate) fn body_type_name<'a>(
    body: Option<&'a ApiSchema>,
    defs: &SchemaDefs,
) -> Option<&'a str> {
    match body {
        Some(ApiSchema::Ref { name }) if defs.contains_key(name) => Some(name.as_str()),
        _ => None,
    }
}

/// Path parameters in path order (extraction order == path order; explicit
/// query params are appended after, so a `location == Path` filter preserves
/// the order they appear in the URL template).
pub(crate) fn path_params(ep: &ApiEndpoint) -> Vec<&ApiParameter> {
    ep.parameters
        .iter()
        .filter(|p| p.location == ParamLocation::Path)
        .collect()
}

/// Query parameters, in declaration order.
pub(crate) fn query_params(ep: &ApiEndpoint) -> Vec<&ApiParameter> {
    ep.parameters
        .iter()
        .filter(|p| p.location == ParamLocation::Query)
        .collect()
}

/// Does this endpoint accept a JSON request body?
pub(crate) fn has_body(ep: &ApiEndpoint) -> bool {
    ep.request_body.is_some()
}

/// The primitive type of a (param) schema, peeling `Nullable` and reading the
/// element type of an `Enum`. Compound shapes (Object/Array/OneOf/Ref) return
/// `None` — the generators map those to the language's open/string type.
pub(crate) fn prim_of(schema: &ApiSchema) -> Option<PrimType> {
    match schema {
        ApiSchema::Primitive { ty, .. } => Some(*ty),
        ApiSchema::Enum { ty, .. } => Some(*ty),
        ApiSchema::Nullable { inner } => prim_of(inner),
        _ => None,
    }
}

/// Replace each `{name}` segment in `path` with `interp(name)` — the calling
/// language's way of interpolating the bound argument (e.g. `${id}` in TS,
/// `{id}` in a Python f-string, `%v` for Go's `fmt.Sprintf`). Text outside the
/// braces is copied verbatim. Unterminated `{` (no closing `}`) consumes to end
/// of string, matching `discover::extract_path_params`.
pub(crate) fn render_path(path: &str, mut interp: impl FnMut(&str) -> String) -> String {
    let mut out = String::with_capacity(path.len());
    let mut chars = path.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            let mut name = String::new();
            for c in chars.by_ref() {
                if c == '}' {
                    break;
                }
                name.push(c);
            }
            out.push_str(&interp(&name));
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{ApiResponse, HttpMethod};

    fn ep(path: &str, params: Vec<ApiParameter>, body: Option<ApiSchema>) -> ApiEndpoint {
        ApiEndpoint {
            crate_name: "demo".into(),
            method: HttpMethod::GET,
            path: path.into(),
            operation_id: "demo".into(),
            summary: "".into(),
            parameters: params,
            request_body: body,
            responses: vec![ApiResponse { status: 200, description: "".into(), schema: None }],
            tags: vec!["demo".into()],
            middleware: None,
        }
    }
    fn pp(name: &str) -> ApiParameter {
        ApiParameter {
            name: name.into(),
            location: ParamLocation::Path,
            required: true,
            schema: ApiSchema::string(),
            description: "".into(),
        }
    }
    fn qp(name: &str, schema: ApiSchema) -> ApiParameter {
        ApiParameter {
            name: name.into(),
            location: ParamLocation::Query,
            required: false,
            schema,
            description: "".into(),
        }
    }

    #[test]
    fn splits_path_and_query_in_order() {
        let e = ep(
            "/orgs/{org_id}/users/{user_id}",
            vec![pp("org_id"), pp("user_id"), qp("limit", ApiSchema::integer())],
            None,
        );
        let names: Vec<&str> = path_params(&e).iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["org_id", "user_id"]);
        let q: Vec<&str> = query_params(&e).iter().map(|p| p.name.as_str()).collect();
        assert_eq!(q, vec!["limit"]);
        assert!(!has_body(&e));
    }

    #[test]
    fn has_body_tracks_request_body() {
        assert!(has_body(&ep("/x", vec![], Some(ApiSchema::ref_to("X")))));
        assert!(!has_body(&ep("/x", vec![], None)));
    }

    #[test]
    fn prim_of_peels_nullable_and_enum() {
        assert_eq!(prim_of(&ApiSchema::integer()), Some(PrimType::Integer));
        assert_eq!(prim_of(&ApiSchema::string().nullable()), Some(PrimType::String));
        assert_eq!(
            prim_of(&ApiSchema::Enum { ty: PrimType::String, values: vec![] }),
            Some(PrimType::String)
        );
        assert_eq!(prim_of(&ApiSchema::ref_to("X")), None);
        assert_eq!(prim_of(&ApiSchema::array_of(ApiSchema::string())), None);
    }

    #[test]
    fn render_path_substitutes_each_segment() {
        let out = render_path("/api/pages/{id}", |n| format!("${{{n}}}"));
        assert_eq!(out, "/api/pages/${id}");
        let multi = render_path("/orgs/{org}/users/{user}", |n| format!(":{n}"));
        assert_eq!(multi, "/orgs/:org/users/:user");
    }

    #[test]
    fn render_path_leaves_static_paths_untouched() {
        assert_eq!(render_path("/v1/webhook", |n| format!("${{{n}}}")), "/v1/webhook");
    }
}
