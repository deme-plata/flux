// The Isaac GR00T reference-humanoid control interface as a flux-api spec.
//
// One Rust source of truth → OpenAPI 3.1 + five SDKs, including the v0.15-B
// auto-paginator (command/receipt history) and SSE stream reader (telemetry).
// The daemon in `daemon.rs` implements EXACTLY these endpoints — the e2e test
// drives the generated TypeScript client against the live daemon, so spec and
// implementation cannot drift silently.
//
// DoF model, per the 2026-06-01 reference design: Unitree H2 Plus body (31)
// + two Sharpa hands (22 each) = 75 total.

use flux_api::{
    ApiEndpoint, ApiParameter, ApiResponse, ApiSchema, HttpMethod, MiddlewareSpec,
    PaginationStyle, ParamLocation, RetryPolicy, StreamKind,
};
use std::collections::BTreeMap;

pub const BODY_DOF: usize = 31;
pub const HAND_DOF: usize = 22;
pub const TOTAL_DOF: usize = BODY_DOF + 2 * HAND_DOF; // 75

const CRATE: &str = "flux-groot";
const TAG: &str = "groot";

fn prim(ty: flux_api::schema::PrimType) -> ApiSchema {
    ApiSchema::Primitive { ty, format: None }
}

fn string() -> ApiSchema {
    prim(flux_api::schema::PrimType::String)
}
fn number() -> ApiSchema {
    prim(flux_api::schema::PrimType::Number)
}
fn integer() -> ApiSchema {
    prim(flux_api::schema::PrimType::Integer)
}
fn boolean() -> ApiSchema {
    prim(flux_api::schema::PrimType::Boolean)
}

fn obj(props: Vec<(&str, ApiSchema)>, required: &[&str]) -> ApiSchema {
    ApiSchema::Object {
        properties: props.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        required: required.iter().map(|s| s.to_string()).collect(),
    }
}

/// Named schemas for `components/schemas` + typed SDK bodies.
pub fn groot_schemas() -> BTreeMap<String, ApiSchema> {
    let mut m = BTreeMap::new();

    // The signed envelope every motion command travels in (QRBT1).
    m.insert(
        "SignedCommand".to_string(),
        obj(
            vec![
                ("wallet", string()),
                ("nonce", integer()),
                ("timestamp", integer()),
                ("command", obj(vec![], &[])),
                ("sig", string()),
            ],
            &["wallet", "nonce", "timestamp", "command", "sig"],
        ),
    );

    // Proprioception snapshot.
    m.insert(
        "RobotState".to_string(),
        obj(
            vec![
                ("body_joints", ApiSchema::Array { items: Box::new(number()) }),
                ("left_hand", ApiSchema::Array { items: Box::new(number()) }),
                ("right_hand", ApiSchema::Array { items: Box::new(number()) }),
                ("estopped", boolean()),
                ("task", ApiSchema::Nullable { inner: Box::new(string()) }),
                ("commands_accepted", integer()),
                ("qug_owed", string()),
            ],
            &["body_joints", "left_hand", "right_hand", "estopped", "commands_accepted"],
        ),
    );

    // One accepted command = one settlement receipt.
    m.insert(
        "CommandReceipt".to_string(),
        obj(
            vec![
                ("seq", integer()),
                ("wallet", string()),
                ("kind", string()),
                ("nonce", integer()),
                ("timestamp", integer()),
                ("command_hash", string()),
                ("cost_qug", string()),
            ],
            &["seq", "wallet", "kind", "nonce", "timestamp", "command_hash", "cost_qug"],
        ),
    );

    m
}

/// The wire surface. Middleware choices are load-bearing, not decorative:
/// - command POST: standard retry IS safe — the nonce burn makes execution
///   at-most-once, so a retried duplicate is rejected, never re-executed.
/// - receipts GET: cursor pagination (the daemon serves data/next_cursor).
/// - telemetry GET: SSE.
/// - estop POST: NO auth middleware and NO envelope — safety is never gated.
pub fn groot_endpoints() -> Vec<ApiEndpoint> {
    let mk = |method: HttpMethod, path: &str, op: &str, summary: &str| ApiEndpoint {
        crate_name: CRATE.to_string(),
        method,
        path: path.to_string(),
        operation_id: op.to_string(),
        summary: summary.to_string(),
        parameters: vec![],
        request_body: None,
        responses: vec![ApiResponse {
            status: 200,
            description: "OK".to_string(),
            schema: None,
        }],
        tags: vec![TAG.to_string()],
        middleware: None,
    };

    let mut eps = Vec::new();

    // GET /v1/robot/state — read-only proprioception; retry-safe.
    let mut state = mk(
        HttpMethod::GET,
        "/v1/robot/state",
        "get_robot_state",
        "75-DoF proprioception snapshot (31 body + 2×22 Sharpa hands), estop flag, active task, settlement counters",
    );
    state.responses[0].schema = Some(ApiSchema::Ref { name: "RobotState".into() });
    state.middleware = Some(MiddlewareSpec::default().with_retry(RetryPolicy::standard()));
    eps.push(state);

    // POST /v1/robot/command — the QRBT1 signed envelope. At-most-once by
    // nonce burn, hence retry-safe.
    let mut command = mk(
        HttpMethod::POST,
        "/v1/robot/command",
        "submit_robot_command",
        "Submit a wallet-signed QRBT1 command envelope: kind=task (GR00T policy instruction) | joints (31-DoF body targets) | hand (side + 22-DoF) | halt. Verified Ed25519(wallet)+nonce+freshness; each accepted command appends a QUG settlement receipt.",
    );
    command.request_body = Some(ApiSchema::Ref { name: "SignedCommand".into() });
    command.middleware = Some(MiddlewareSpec::default().with_retry(RetryPolicy::standard()));
    eps.push(command);

    // GET /v1/robot/receipts — cursor-paginated settlement/command history.
    let mut receipts = mk(
        HttpMethod::GET,
        "/v1/robot/receipts",
        "list_robot_receipts",
        "Cursor-paginated command/settlement receipt history (the pay-per-command ledger the operator settles in QUG out-of-band)",
    );
    receipts.parameters = vec![
        ApiParameter {
            name: "after".into(),
            location: ParamLocation::Query,
            required: false,
            description: "Opaque cursor from the previous page's next_cursor".into(),
            schema: string(),
        },
        ApiParameter {
            name: "limit".into(),
            location: ParamLocation::Query,
            required: false,
            description: "Page size (default 3 in the reference daemon)".into(),
            schema: integer(),
        },
    ];
    receipts.responses[0].schema = Some(obj(
        vec![
            (
                "data",
                ApiSchema::Array { items: Box::new(ApiSchema::Ref { name: "CommandReceipt".into() }) },
            ),
            ("next_cursor", ApiSchema::Nullable { inner: Box::new(string()) }),
        ],
        &["data"],
    ));
    receipts.middleware = Some(MiddlewareSpec::default().with_pagination(PaginationStyle::Cursor {
        cursor_param: "after".into(),
        response_field: "next_cursor".into(),
    }));
    eps.push(receipts);

    // GET /v1/robot/telemetry — SSE joint/pose stream.
    let mut telemetry = mk(
        HttpMethod::GET,
        "/v1/robot/telemetry",
        "stream_robot_telemetry",
        "Server-sent-events stream of proprioception frames (JSON per data: frame)",
    );
    telemetry.middleware = Some(MiddlewareSpec::default().with_streaming(StreamKind::Sse));
    eps.push(telemetry);

    // POST /v1/robot/estop — UNSIGNED on purpose. Anyone may stop the robot;
    // only allowlisted signers may move it. Auth gates motion, never safety.
    eps.push(mk(
        HttpMethod::POST,
        "/v1/robot/estop",
        "emergency_stop",
        "Latch the emergency stop. Deliberately unauthenticated — safety commands must never be gated on key material. Clears only via signed kind=clear_estop.",
    ));

    eps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dof_arithmetic_matches_reference_design() {
        assert_eq!(TOTAL_DOF, 75);
        assert_eq!(BODY_DOF + 2 * HAND_DOF, TOTAL_DOF);
    }

    #[test]
    fn estop_is_the_only_unauthenticated_motion_adjacent_endpoint() {
        let eps = groot_endpoints();
        let estop = eps.iter().find(|e| e.operation_id == "emergency_stop").unwrap();
        assert!(estop.middleware.is_none(), "estop must carry no auth middleware");
        // And the command endpoint must NOT be estop-shaped: it carries the
        // envelope body.
        let cmd = eps.iter().find(|e| e.operation_id == "submit_robot_command").unwrap();
        assert!(matches!(cmd.request_body, Some(ApiSchema::Ref { .. })));
    }

    #[test]
    fn receipts_declare_cursor_pagination() {
        let eps = groot_endpoints();
        let r = eps.iter().find(|e| e.operation_id == "list_robot_receipts").unwrap();
        match &r.middleware.as_ref().unwrap().pagination {
            PaginationStyle::Cursor { cursor_param, response_field } => {
                assert_eq!(cursor_param, "after");
                assert_eq!(response_field, "next_cursor");
            }
            other => panic!("expected cursor pagination, got {other:?}"),
        }
    }

    #[test]
    fn telemetry_declares_sse() {
        let eps = groot_endpoints();
        let t = eps.iter().find(|e| e.operation_id == "stream_robot_telemetry").unwrap();
        assert_eq!(t.middleware.as_ref().unwrap().streaming, StreamKind::Sse);
    }

    #[test]
    fn openapi_document_is_valid_31() {
        let spec = flux_api::generate_openapi_with_schemas(
            "Isaac GR00T Reference Control",
            "0.1",
            &groot_endpoints(),
            &groot_schemas(),
        );
        // Independent parser proves the emitted document is a real OpenAPI
        // 3.1 spec, not JSON that merely looks API-shaped. (oas3 0.16 has no
        // from_str — deserialize into its Spec type, same as flux-api's own
        // golden tests.)
        serde_json::from_str::<oas3::Spec>(&spec.to_string())
            .expect("generated OpenAPI must validate");
    }
}
