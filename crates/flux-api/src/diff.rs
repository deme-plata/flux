// v0.17-C — OpenAPI schema diff classifier.
//
// Walks two OpenAPI documents and classifies the change as None < Patch <
// Minor < Major. Drives the workspace `[workspace.package] version` bump on
// every regeneration:
//
//   classify_diff(old, new)
//     None  → no version bump
//     Patch → docs / summary text only
//     Minor → additive: new path, new method, new optional param/field
//     Major → breaking: removed path/method, removed required field, type
//             change on an existing field/param, required→param flipped on
//
// Heuristic but useful: catches the changes that would actually break a
// generated client. v0.18+ may tighten with full $ref-resolved schema diff.

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SchemaChange {
    None = 0,
    Patch = 1,
    Minor = 2,
    Major = 3,
}

impl SchemaChange {
    pub fn label(self) -> &'static str {
        match self {
            SchemaChange::None => "none",
            SchemaChange::Patch => "patch",
            SchemaChange::Minor => "minor",
            SchemaChange::Major => "major",
        }
    }
}

pub fn classify_diff(old: &Value, new: &Value) -> SchemaChange {
    let mut worst = SchemaChange::None;
    let bump = |w: &mut SchemaChange, c: SchemaChange| {
        if c > *w {
            *w = c;
        }
    };

    let old_paths = old["paths"].as_object();
    let new_paths = new["paths"].as_object();

    // Removed paths → Major.
    if let Some(op) = old_paths {
        for path in op.keys() {
            if new_paths.map(|np| !np.contains_key(path)).unwrap_or(true) {
                bump(&mut worst, SchemaChange::Major);
            }
        }
    }

    // Added paths → Minor.
    if let Some(np) = new_paths {
        for path in np.keys() {
            if old_paths.map(|op| !op.contains_key(path)).unwrap_or(true) {
                bump(&mut worst, SchemaChange::Minor);
            }
        }
    }

    // Path-by-path operation diff.
    if let (Some(op), Some(np)) = (old_paths, new_paths) {
        for (path, ops_old) in op {
            let Some(ops_new) = np.get(path) else { continue };
            let Some(ops_old_obj) = ops_old.as_object() else { continue };
            let Some(ops_new_obj) = ops_new.as_object() else { continue };

            // Removed methods → Major.
            for verb in ops_old_obj.keys() {
                if !is_http_verb(verb) {
                    continue;
                }
                if !ops_new_obj.contains_key(verb) {
                    bump(&mut worst, SchemaChange::Major);
                }
            }
            // Added methods → Minor.
            for verb in ops_new_obj.keys() {
                if !is_http_verb(verb) {
                    continue;
                }
                if !ops_old_obj.contains_key(verb) {
                    bump(&mut worst, SchemaChange::Minor);
                }
            }

            // Per-operation diff.
            for (verb, op_old) in ops_old_obj {
                if !is_http_verb(verb) {
                    continue;
                }
                let Some(op_new) = ops_new_obj.get(verb) else { continue };
                bump(&mut worst, diff_operation(op_old, op_new));
            }
        }
    }

    worst
}

fn is_http_verb(s: &str) -> bool {
    matches!(s, "get" | "post" | "put" | "delete" | "patch")
}

fn diff_operation(old: &Value, new: &Value) -> SchemaChange {
    let mut worst = SchemaChange::None;
    let bump = |w: &mut SchemaChange, c: SchemaChange| {
        if c > *w {
            *w = c;
        }
    };

    // Summary / description / tags → Patch only.
    for field in ["summary", "description"] {
        if old[field] != new[field] {
            bump(&mut worst, SchemaChange::Patch);
        }
    }

    // Parameters by name.
    let empty = serde_json::Value::Array(vec![]);
    let old_params = old["parameters"].as_array().unwrap_or_else(|| {
        let _ = &empty;
        const E: &Vec<Value> = &Vec::new();
        E
    });
    let new_params = new["parameters"].as_array().unwrap_or_else(|| {
        const E: &Vec<Value> = &Vec::new();
        E
    });
    let old_by_name = params_by_name(old_params);
    let new_by_name = params_by_name(new_params);

    // Removed required param → Major; removed optional → Minor.
    for (name, p_old) in &old_by_name {
        if !new_by_name.contains_key(name) {
            let was_required = p_old["required"].as_bool().unwrap_or(false);
            bump(
                &mut worst,
                if was_required { SchemaChange::Major } else { SchemaChange::Minor },
            );
        }
    }
    // Added required param → Major (breaking — old clients won't send it);
    // added optional → Minor.
    for (name, p_new) in &new_by_name {
        if !old_by_name.contains_key(name) {
            let is_required = p_new["required"].as_bool().unwrap_or(false);
            bump(
                &mut worst,
                if is_required { SchemaChange::Major } else { SchemaChange::Minor },
            );
        }
    }
    // Type / required flip on a param that exists in both.
    for (name, p_old) in &old_by_name {
        let Some(p_new) = new_by_name.get(name) else { continue };
        if p_old["schema"]["type"] != p_new["schema"]["type"] {
            bump(&mut worst, SchemaChange::Major);
        }
        let was_req = p_old["required"].as_bool().unwrap_or(false);
        let is_req = p_new["required"].as_bool().unwrap_or(false);
        if was_req != is_req {
            // Loosening (required→optional) is Minor; tightening is Major.
            bump(
                &mut worst,
                if is_req { SchemaChange::Major } else { SchemaChange::Minor },
            );
        }
    }

    // Request body: type ref change or required flip → Major; added → Minor.
    let old_body = old.get("requestBody");
    let new_body = new.get("requestBody");
    match (old_body, new_body) {
        (None, Some(_)) => bump(&mut worst, SchemaChange::Minor),
        (Some(_), None) => bump(&mut worst, SchemaChange::Major),
        (Some(a), Some(b)) => {
            if a["content"]["application/json"]["schema"]
                != b["content"]["application/json"]["schema"]
            {
                bump(&mut worst, SchemaChange::Major);
            }
        }
        _ => {}
    }

    worst
}

fn params_by_name(arr: &[Value]) -> std::collections::BTreeMap<String, &Value> {
    arr.iter()
        .filter_map(|p| {
            p["name"]
                .as_str()
                .map(|n| (n.to_string(), p))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn doc(paths: Value) -> Value {
        json!({
            "openapi": "3.1.0",
            "info": { "title": "t", "version": "0" },
            "paths": paths
        })
    }

    #[test]
    fn identical_docs_are_none() {
        let d = doc(json!({
            "/a": { "get": { "summary": "s", "responses": { "200": { "description": "ok" } } } }
        }));
        assert_eq!(classify_diff(&d, &d), SchemaChange::None);
    }

    #[test]
    fn summary_change_only_is_patch() {
        let a = doc(json!({
            "/a": { "get": { "summary": "old", "responses": { "200": { "description": "ok" } } } }
        }));
        let b = doc(json!({
            "/a": { "get": { "summary": "new", "responses": { "200": { "description": "ok" } } } }
        }));
        assert_eq!(classify_diff(&a, &b), SchemaChange::Patch);
    }

    #[test]
    fn added_path_is_minor() {
        let a = doc(json!({
            "/a": { "get": { "responses": { "200": { "description": "ok" } } } }
        }));
        let b = doc(json!({
            "/a": { "get": { "responses": { "200": { "description": "ok" } } } },
            "/b": { "get": { "responses": { "200": { "description": "ok" } } } }
        }));
        assert_eq!(classify_diff(&a, &b), SchemaChange::Minor);
    }

    #[test]
    fn removed_path_is_major() {
        let a = doc(json!({
            "/a": { "get": { "responses": {} } },
            "/b": { "get": { "responses": {} } }
        }));
        let b = doc(json!({
            "/a": { "get": { "responses": {} } }
        }));
        assert_eq!(classify_diff(&a, &b), SchemaChange::Major);
    }

    #[test]
    fn added_method_on_existing_path_is_minor() {
        let a = doc(json!({ "/a": { "get": { "responses": {} } } }));
        let b = doc(json!({
            "/a": { "get": { "responses": {} }, "post": { "responses": {} } }
        }));
        assert_eq!(classify_diff(&a, &b), SchemaChange::Minor);
    }

    #[test]
    fn removed_method_is_major() {
        let a = doc(json!({
            "/a": { "get": { "responses": {} }, "post": { "responses": {} } }
        }));
        let b = doc(json!({ "/a": { "get": { "responses": {} } } }));
        assert_eq!(classify_diff(&a, &b), SchemaChange::Major);
    }

    #[test]
    fn adding_required_param_is_major() {
        let a = doc(json!({
            "/a": { "get": { "parameters": [], "responses": {} } }
        }));
        let b = doc(json!({
            "/a": { "get": { "parameters": [
                { "name": "id", "in": "query", "required": true, "schema": { "type": "string" } }
            ], "responses": {} } }
        }));
        assert_eq!(classify_diff(&a, &b), SchemaChange::Major);
    }

    #[test]
    fn adding_optional_param_is_minor() {
        let a = doc(json!({
            "/a": { "get": { "parameters": [], "responses": {} } }
        }));
        let b = doc(json!({
            "/a": { "get": { "parameters": [
                { "name": "limit", "in": "query", "required": false, "schema": { "type": "integer" } }
            ], "responses": {} } }
        }));
        assert_eq!(classify_diff(&a, &b), SchemaChange::Minor);
    }

    #[test]
    fn changing_param_type_is_major() {
        let a = doc(json!({
            "/a": { "get": { "parameters": [
                { "name": "x", "in": "query", "required": false, "schema": { "type": "string" } }
            ], "responses": {} } }
        }));
        let b = doc(json!({
            "/a": { "get": { "parameters": [
                { "name": "x", "in": "query", "required": false, "schema": { "type": "integer" } }
            ], "responses": {} } }
        }));
        assert_eq!(classify_diff(&a, &b), SchemaChange::Major);
    }

    #[test]
    fn loosening_required_to_optional_is_minor() {
        let a = doc(json!({
            "/a": { "get": { "parameters": [
                { "name": "x", "in": "query", "required": true, "schema": { "type": "string" } }
            ], "responses": {} } }
        }));
        let b = doc(json!({
            "/a": { "get": { "parameters": [
                { "name": "x", "in": "query", "required": false, "schema": { "type": "string" } }
            ], "responses": {} } }
        }));
        assert_eq!(classify_diff(&a, &b), SchemaChange::Minor);
    }

    #[test]
    fn added_request_body_is_minor() {
        let a = doc(json!({ "/a": { "post": { "responses": {} } } }));
        let b = doc(json!({
            "/a": { "post": {
                "requestBody": { "content": { "application/json": { "schema": { "$ref": "#/components/schemas/X" } } } },
                "responses": {}
            } }
        }));
        assert_eq!(classify_diff(&a, &b), SchemaChange::Minor);
    }

    #[test]
    fn changing_request_body_schema_is_major() {
        let a = doc(json!({
            "/a": { "post": {
                "requestBody": { "content": { "application/json": { "schema": { "$ref": "#/components/schemas/X" } } } },
                "responses": {}
            } }
        }));
        let b = doc(json!({
            "/a": { "post": {
                "requestBody": { "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Y" } } } },
                "responses": {}
            } }
        }));
        assert_eq!(classify_diff(&a, &b), SchemaChange::Major);
    }

    #[test]
    fn ordering_is_none_less_patch_less_minor_less_major() {
        assert!(SchemaChange::None < SchemaChange::Patch);
        assert!(SchemaChange::Patch < SchemaChange::Minor);
        assert!(SchemaChange::Minor < SchemaChange::Major);
    }
}
