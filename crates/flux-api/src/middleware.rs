// v0.15-A — middleware type model.
//
// `MiddlewareSpec` is the declarative description an `ApiEndpoint` carries to
// tell SDK generators which auth header to attach, which response codes to
// retry, which pagination shape to expose, and whether the response is a
// streaming surface. v0.15-B/C lower these into emitted client code; v0.15-D
// proves the wire behaviour against a mock server.
//
// Today every endpoint defaults to `None` for every middleware concern —
// existing v0.14 SDKs keep emitting bare fetch/reqwest/httpx calls. Adding a
// middleware spec is opt-in per endpoint.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MiddlewareSpec {
    pub auth: AuthKind,
    pub retry: RetryPolicy,
    pub pagination: PaginationStyle,
    pub streaming: StreamKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuthKind {
    #[default]
    None,
    /// `Authorization: Bearer <token>` — the SDK constructor accepts a token.
    Bearer,
    /// Custom header, e.g. `X-API-Key: <key>`.
    ApiKey { header: String },
    /// HTTP basic, base64(user:pass).
    Basic,
}

/// Exponential-backoff retry. `max_attempts = 1` means "no retry, only the
/// initial request"; v0.15 SDKs hard-code 0 retries when this is the default
/// so test surfaces don't introduce hidden latency.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
    pub retry_on_status: Vec<u16>,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 1,
            initial_backoff_ms: 0,
            max_backoff_ms: 0,
            retry_on_status: vec![],
        }
    }
}

impl RetryPolicy {
    /// A sensible "standard" retry policy: 3 attempts, exp-backoff 200ms→2s,
    /// retry on 429 + 5xx.
    pub fn standard() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff_ms: 200,
            max_backoff_ms: 2_000,
            retry_on_status: vec![429, 500, 502, 503, 504],
        }
    }
    pub fn is_active(&self) -> bool {
        self.max_attempts > 1
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PaginationStyle {
    #[default]
    None,
    /// `?cursor=<>` request, response carries `<response_field>` for the next
    /// cursor (e.g. Stripe `data + has_more + last_id`).
    Cursor { cursor_param: String, response_field: String },
    /// `?page=N` / `?page=N&per_page=M`.
    Page { page_param: String },
    /// `?offset=N&limit=M` style.
    Offset { offset_param: String, limit_param: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    #[default]
    None,
    /// Server-Sent Events; SDKs emit an `async for chunk in client.stream(...)`
    /// surface backed by `EventSource`/`sseclient`.
    Sse,
    /// WebSocket; SDKs emit a typed event-union consumer (see
    /// `event_types::generate_ts_event_types`).
    WebSocket,
}

impl MiddlewareSpec {
    pub fn bearer_auth() -> Self {
        Self {
            auth: AuthKind::Bearer,
            ..Default::default()
        }
    }
    pub fn with_retry(mut self, policy: RetryPolicy) -> Self {
        self.retry = policy;
        self
    }
    pub fn with_pagination(mut self, style: PaginationStyle) -> Self {
        self.pagination = style;
        self
    }
    pub fn with_streaming(mut self, kind: StreamKind) -> Self {
        self.streaming = kind;
        self
    }
    /// Cheap "is this anything other than the default no-op?" probe used by
    /// SDK generators to decide whether to emit middleware scaffolding.
    pub fn is_active(&self) -> bool {
        !matches!(self.auth, AuthKind::None)
            || self.retry.is_active()
            || !matches!(self.pagination, PaginationStyle::None)
            || !matches!(self.streaming, StreamKind::None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_spec_is_inactive() {
        assert!(!MiddlewareSpec::default().is_active());
    }

    #[test]
    fn bearer_auth_is_active() {
        let m = MiddlewareSpec::bearer_auth();
        assert_eq!(m.auth, AuthKind::Bearer);
        assert!(m.is_active());
    }

    #[test]
    fn standard_retry_marks_active() {
        let p = RetryPolicy::standard();
        assert!(p.is_active());
        assert_eq!(p.max_attempts, 3);
        assert!(p.retry_on_status.contains(&429));
        assert!(p.retry_on_status.contains(&503));
    }

    #[test]
    fn builder_chains_auth_retry_pagination_streaming() {
        let m = MiddlewareSpec::bearer_auth()
            .with_retry(RetryPolicy::standard())
            .with_pagination(PaginationStyle::Page { page_param: "page".into() })
            .with_streaming(StreamKind::Sse);
        assert_eq!(m.auth, AuthKind::Bearer);
        assert!(m.retry.is_active());
        assert!(matches!(m.pagination, PaginationStyle::Page { .. }));
        assert_eq!(m.streaming, StreamKind::Sse);
    }

    #[test]
    fn api_key_header_preserved() {
        let a = AuthKind::ApiKey { header: "X-Quillon-Key".into() };
        match a {
            AuthKind::ApiKey { header } => assert_eq!(header, "X-Quillon-Key"),
            _ => panic!(),
        }
    }

    #[test]
    fn cursor_pagination_carries_field_names() {
        let p = PaginationStyle::Cursor {
            cursor_param: "after".into(),
            response_field: "has_more".into(),
        };
        if let PaginationStyle::Cursor { cursor_param, response_field } = p {
            assert_eq!(cursor_param, "after");
            assert_eq!(response_field, "has_more");
        } else {
            panic!();
        }
    }

    #[test]
    fn spec_round_trips_through_json() {
        let m = MiddlewareSpec::bearer_auth()
            .with_retry(RetryPolicy::standard())
            .with_pagination(PaginationStyle::Cursor {
                cursor_param: "cursor".into(),
                response_field: "next_cursor".into(),
            })
            .with_streaming(StreamKind::Sse);
        let s = serde_json::to_string(&m).expect("serialize");
        let m2: MiddlewareSpec = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(m, m2);
    }
}
