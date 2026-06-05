// Schema model for flux-api: endpoints, parameters, responses, request bodies.
//
// v0.12-A (rocky, 2026-05-29) — replaced the flat `ApiSchema { type_name, format,
// nullable }` record with a proper sum type. Old callsites that just used
// `ApiSchema` as a leaf primitive build via `ApiSchema::string()` / etc; richer
// callsites can now describe objects, arrays, enums, refs, and oneOf shapes.
// OpenAPI 3.1 emission (v0.12-C) will lower these into `components/schemas`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiEndpoint {
    pub crate_name: String,
    pub method: HttpMethod,
    pub path: String,
    pub operation_id: String,
    pub summary: String,
    pub parameters: Vec<ApiParameter>,
    pub request_body: Option<ApiSchema>,
    pub responses: Vec<ApiResponse>,
    pub tags: Vec<String>,
    /// v0.15: declarative middleware (auth/retry/pagination/streaming) lowered
    /// by SDK generators. `None` keeps the v0.14 emission path (no middleware).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub middleware: Option<crate::middleware::MiddlewareSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum HttpMethod { GET, POST, PUT, DELETE, PATCH }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiParameter {
    pub name: String,
    pub location: ParamLocation,
    pub required: bool,
    pub schema: ApiSchema,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ParamLocation { Path, Query, Header }

/// The schema sum type. Maps onto JSON Schema 2020-12 / OpenAPI 3.1 shapes:
/// `Primitive` → `{ "type": "X", "format": "..." }`,
/// `Object` → `{ "type": "object", "properties": {...}, "required": [...] }`,
/// `Array` → `{ "type": "array", "items": <ApiSchema> }`,
/// `Enum` → `{ "type": "X", "enum": [...] }`,
/// `OneOf` → `{ "oneOf": [...] }`,
/// `Ref` → `{ "$ref": "#/components/schemas/<name>" }`,
/// `Nullable` → JSON Schema `{ "type": ["X", "null"] }` (lowered by emitter).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApiSchema {
    Primitive {
        ty: PrimType,
        #[serde(skip_serializing_if = "Option::is_none")]
        format: Option<String>,
    },
    Object {
        properties: BTreeMap<String, ApiSchema>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        required: Vec<String>,
    },
    Array {
        items: Box<ApiSchema>,
    },
    Enum {
        ty: PrimType,
        values: Vec<serde_json::Value>,
    },
    OneOf {
        variants: Vec<ApiSchema>,
    },
    Ref {
        /// Name of a schema entry in `components/schemas`; the emitter
        /// produces `{ "$ref": "#/components/schemas/<name>" }`.
        name: String,
    },
    /// Wrap a schema in a nullable shell. OpenAPI 3.1 lowers this into
    /// `type: ["X", "null"]`; older 3.0 emitters set `nullable: true`.
    ///
    /// Struct variant (not tuple) because `#[serde(tag = "kind")]` rejects
    /// tuple variants whose inner is a recursive instance of the same enum
    /// — Serde's trait resolution loops on it (E0275 overflow).
    Nullable { inner: Box<ApiSchema> },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PrimType {
    String,
    Integer,
    Number,
    Boolean,
}

impl PrimType {
    pub fn as_str(self) -> &'static str {
        match self {
            PrimType::String => "string",
            PrimType::Integer => "integer",
            PrimType::Number => "number",
            PrimType::Boolean => "boolean",
        }
    }
}

impl ApiSchema {
    pub fn string() -> Self {
        Self::Primitive { ty: PrimType::String, format: None }
    }
    pub fn integer() -> Self {
        Self::Primitive { ty: PrimType::Integer, format: None }
    }
    pub fn number() -> Self {
        Self::Primitive { ty: PrimType::Number, format: None }
    }
    pub fn boolean() -> Self {
        Self::Primitive { ty: PrimType::Boolean, format: None }
    }
    pub fn string_with_format(fmt: impl Into<String>) -> Self {
        Self::Primitive { ty: PrimType::String, format: Some(fmt.into()) }
    }
    pub fn array_of(items: ApiSchema) -> Self {
        Self::Array { items: Box::new(items) }
    }
    pub fn ref_to(name: impl Into<String>) -> Self {
        Self::Ref { name: name.into() }
    }
    pub fn one_of(variants: Vec<ApiSchema>) -> Self {
        Self::OneOf { variants }
    }
    pub fn nullable(self) -> Self {
        match self {
            already @ ApiSchema::Nullable { .. } => already,
            other => ApiSchema::Nullable { inner: Box::new(other) },
        }
    }
    /// Start an object schema builder. Use `.prop()` / `.req_prop()` and
    /// finish with `.build()`.
    pub fn object() -> ObjectBuilder {
        ObjectBuilder { properties: BTreeMap::new(), required: vec![] }
    }
}

pub struct ObjectBuilder {
    properties: BTreeMap<String, ApiSchema>,
    required: Vec<String>,
}

impl ObjectBuilder {
    /// Add an optional property.
    pub fn prop(mut self, name: impl Into<String>, schema: ApiSchema) -> Self {
        self.properties.insert(name.into(), schema);
        self
    }
    /// Add a required property (= insert + mark required in one step).
    pub fn req_prop(mut self, name: impl Into<String>, schema: ApiSchema) -> Self {
        let n: String = name.into();
        self.properties.insert(n.clone(), schema);
        self.required.push(n);
        self
    }
    pub fn build(self) -> ApiSchema {
        ApiSchema::Object {
            properties: self.properties,
            required: self.required,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResponse {
    pub status: u16,
    pub description: String,
    pub schema: Option<ApiSchema>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitive_constructors() {
        assert!(matches!(ApiSchema::string(),
            ApiSchema::Primitive { ty: PrimType::String, format: None }));
        assert!(matches!(ApiSchema::integer(),
            ApiSchema::Primitive { ty: PrimType::Integer, .. }));
        assert!(matches!(ApiSchema::boolean(),
            ApiSchema::Primitive { ty: PrimType::Boolean, .. }));
    }

    #[test]
    fn string_with_format_carries_format() {
        match ApiSchema::string_with_format("uuid") {
            ApiSchema::Primitive { ty, format } => {
                assert_eq!(ty, PrimType::String);
                assert_eq!(format.as_deref(), Some("uuid"));
            }
            other => panic!("expected Primitive, got {other:?}"),
        }
    }

    #[test]
    fn object_builder_required_props() {
        let obj = ApiSchema::object()
            .req_prop("id", ApiSchema::string())
            .prop("nickname", ApiSchema::string())
            .build();
        match obj {
            ApiSchema::Object { properties, required } => {
                assert!(properties.contains_key("id"));
                assert!(properties.contains_key("nickname"));
                assert_eq!(required, vec!["id".to_string()]);
            }
            _ => panic!("expected Object"),
        }
    }

    #[test]
    fn nested_array_of_object() {
        let s = ApiSchema::array_of(
            ApiSchema::object()
                .req_prop("name", ApiSchema::string())
                .build()
        );
        match s {
            ApiSchema::Array { items } => match *items {
                ApiSchema::Object { ref required, .. } => {
                    assert_eq!(required, &vec!["name".to_string()])
                }
                _ => panic!("inner not Object"),
            },
            _ => panic!("not Array"),
        }
    }

    #[test]
    fn enum_carries_values() {
        let s = ApiSchema::Enum {
            ty: PrimType::String,
            values: vec![
                serde_json::Value::String("draft".into()),
                serde_json::Value::String("published".into()),
            ],
        };
        match s {
            ApiSchema::Enum { ty, values } => {
                assert_eq!(ty, PrimType::String);
                assert_eq!(values.len(), 2);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn one_of_chains_variants() {
        let s = ApiSchema::one_of(vec![
            ApiSchema::string(),
            ApiSchema::integer(),
        ]);
        if let ApiSchema::OneOf { variants } = s {
            assert_eq!(variants.len(), 2);
        } else {
            panic!("not OneOf");
        }
    }

    #[test]
    fn ref_keeps_name() {
        let s = ApiSchema::ref_to("Page");
        if let ApiSchema::Ref { name } = s {
            assert_eq!(name, "Page");
        } else {
            panic!("not Ref");
        }
    }

    #[test]
    fn nullable_wraps_inner_schema() {
        let s = ApiSchema::string().nullable();
        match s {
            ApiSchema::Nullable { inner } => match *inner {
                ApiSchema::Primitive { ty: PrimType::String, .. } => {}
                _ => panic!("inner not String primitive"),
            },
            _ => panic!("not Nullable"),
        }
    }

    #[test]
    fn nullable_is_idempotent() {
        // Wrapping a nullable in another nullable would mean the same thing
        // — keep it flat so downstream emitters don't need to peel layers.
        let once = ApiSchema::integer().nullable();
        let twice = once.clone().nullable();
        // PartialEq isn't derived (sum types via serde tag can't be Eq when
        // they contain serde_json::Value); compare via JSON.
        let j1 = serde_json::to_string(&once).unwrap();
        let j2 = serde_json::to_string(&twice).unwrap();
        assert_eq!(j1, j2);
    }

    #[test]
    fn schema_round_trips_through_json() {
        let s = ApiSchema::object()
            .req_prop("id", ApiSchema::string_with_format("uuid"))
            .req_prop("price", ApiSchema::number())
            .prop("tags", ApiSchema::array_of(ApiSchema::string()))
            .prop("status", ApiSchema::Enum {
                ty: PrimType::String,
                values: vec![
                    serde_json::Value::String("active".into()),
                    serde_json::Value::String("archived".into()),
                ],
            })
            .build();
        let j = serde_json::to_string(&s).expect("serialize");
        let s2: ApiSchema = serde_json::from_str(&j).expect("deserialize");
        // tag layout survives a round trip
        if let ApiSchema::Object { properties, required } = s2 {
            assert!(properties.contains_key("id"));
            assert!(required.contains(&"price".to_string()));
        } else {
            panic!("round trip not Object");
        }
    }
}
