//! Typed Edge Function client.
//!
//! Mirrors the contract established in Tauri Prism's `supabase/functions/_shared/mutation/`:
//! - URL shape: `{base}/functions/v1/{function}/v1/{op}` (op-style routing)
//! - Auth: `Authorization: Bearer {jwt}` + `apikey: {anon}`
//! - Response: `{ success, data | error: { code, message }, request_id }`
//! - Idempotency: callers may set or auto-generate an `Idempotency-Key` header (UUIDv7)
//!
//! Errors are classified into [`crate::errors::EdgeError`] variants so the offline queue can
//! decide which calls to retry, drop, or surface.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::client::Client;
use crate::errors::{EdgeError, PolyError};

/// Header name for caller-supplied idempotency keys.
pub const IDEMPOTENCY_HEADER: &str = "Idempotency-Key";
/// Header name for caller-supplied request id (echoed in error responses).
pub const REQUEST_ID_HEADER: &str = "X-Request-Id";

/// Inputs for a single Edge Function invocation.
#[derive(Debug, Clone, Serialize)]
pub struct EdgeRequest<P: Serialize> {
    /// Edge Function name, e.g. `messages-write` (no leading slash).
    pub function: String,
    /// Operation under `/v1/{op}` (e.g. `send`, `edit`, `delete`). Empty string means no `/v1/{op}` suffix.
    #[serde(default)]
    pub op: Option<String>,
    /// Caller-supplied payload that will be JSON-serialized into the request body.
    pub payload: P,
    /// Optional caller-supplied idempotency key. Auto-generated if omitted.
    #[serde(default)]
    pub idempotency_key: Option<String>,
    /// Optional caller-supplied request id (echoed in error responses).
    #[serde(default)]
    pub request_id: Option<String>,
}

impl<P: Serialize> EdgeRequest<P> {
    /// Build a request with no `op` suffix and auto-generated idempotency / request ids.
    pub fn new(function: impl Into<String>, payload: P) -> Self {
        Self {
            function: function.into(),
            op: None,
            payload,
            idempotency_key: None,
            request_id: None,
        }
    }

    /// Set the `/v1/{op}` suffix.
    pub fn with_op(mut self, op: impl Into<String>) -> Self {
        self.op = Some(op.into());
        self
    }

    /// Override the auto-generated idempotency key.
    pub fn with_idempotency_key(mut self, key: impl Into<String>) -> Self {
        self.idempotency_key = Some(key.into());
        self
    }

    /// Override the auto-generated request id.
    pub fn with_request_id(mut self, id: impl Into<String>) -> Self {
        self.request_id = Some(id.into());
        self
    }
}

/// Decoded successful response. `data` is the per-function payload (caller types it).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeOk<R> {
    /// Typed payload returned by the Edge Function.
    pub data: R,
    /// Server-echoed request id for log correlation.
    #[serde(default)]
    pub request_id: Option<String>,
}

/// Server-side error envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeErrorBody {
    /// Error code identifying the failure class (e.g. `version_conflict`, `validation`).
    pub code: String,
    /// Human-readable error message.
    pub message: String,
    /// Optional additional details (validator output, conflict context, etc.).
    #[serde(default)]
    pub details: Option<Value>,
}

/// Decoded server envelope before the typed `data` payload is extracted.
///
/// Kept untyped (`serde_json::Value`) so the derived `Deserialize` impl does not
/// impose unwanted bounds on the generic response type used by `EdgeClient::call`.
#[derive(Debug, Deserialize)]
struct RawEnvelope {
    success: bool,
    #[serde(default)]
    data: Option<Value>,
    #[serde(default)]
    error: Option<EdgeErrorBody>,
    #[serde(default)]
    request_id: Option<String>,
}

/// Edge Function client.
#[derive(Debug, Clone)]
pub struct EdgeClient {
    client: Client,
}

impl EdgeClient {
    /// Build an Edge Function client wrapping a configured [`Client`].
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Invoke an Edge Function with a typed payload and decode a typed response.
    pub async fn call<P, R>(
        &self,
        request: EdgeRequest<P>,
        access_token: &str,
    ) -> Result<EdgeOk<R>, PolyError>
    where
        P: Serialize,
        R: for<'de> Deserialize<'de>,
    {
        let function_name = request.function.clone();
        let url = self.build_url(&request.function, request.op.as_deref());
        let idempotency_key =
            request.idempotency_key.clone().unwrap_or_else(|| Uuid::now_v7().to_string());
        let request_id = request.request_id.clone().unwrap_or_else(|| Uuid::now_v7().to_string());

        let resp = self
            .client
            .http()
            .post(&url)
            .header("apikey", &self.client.config().supabase_anon_key)
            .header("Authorization", format!("Bearer {access_token}"))
            .header(IDEMPOTENCY_HEADER, idempotency_key)
            .header(REQUEST_ID_HEADER, request_id.clone())
            .json(&request.payload)
            .send()
            .await
            .map_err(|err| {
                PolyError::Edge(EdgeError::Transient {
                    function: function_name.clone(),
                    message: err.to_string(),
                })
            })?;

        let status = resp.status();
        let body = resp.text().await.map_err(|err| {
            PolyError::Edge(EdgeError::Transient {
                function: function_name.clone(),
                message: format!("read body: {err}"),
            })
        })?;

        if status.is_success() {
            let envelope: RawEnvelope = serde_json::from_str(&body).map_err(|err| {
                PolyError::Edge(EdgeError::Decode {
                    function: function_name.clone(),
                    message: err.to_string(),
                })
            })?;
            if envelope.success {
                let Some(data_value) = envelope.data else {
                    return Err(PolyError::Edge(EdgeError::Decode {
                        function: function_name,
                        message: "envelope.success was true but data was missing".into(),
                    }));
                };
                let data: R = serde_json::from_value(data_value).map_err(|err| {
                    PolyError::Edge(EdgeError::Decode {
                        function: function_name.clone(),
                        message: err.to_string(),
                    })
                })?;
                return Ok(EdgeOk { data, request_id: envelope.request_id });
            }
            // Server returned 200 with success: false. Map to error class via code.
            let error = envelope.error.unwrap_or(EdgeErrorBody {
                code: "unknown".into(),
                message: body,
                details: None,
            });
            return Err(classify_error(&function_name, status, &error));
        }

        // Non-2xx: try to decode the envelope; otherwise synthesize.
        let error = match serde_json::from_str::<RawEnvelope>(&body)
            .ok()
            .and_then(|envelope| envelope.error)
        {
            Some(error) => error,
            None => {
                EdgeErrorBody { code: status.as_u16().to_string(), message: body, details: None }
            }
        };
        Err(classify_error(&function_name, status, &error))
    }

    fn build_url(&self, function: &str, op: Option<&str>) -> String {
        let base = self.client.functions_url(function);
        match op {
            Some(op) if !op.is_empty() => format!("{base}/v1/{op}"),
            _ => base,
        }
    }
}

fn classify_error(
    function_name: &str,
    status: reqwest::StatusCode,
    error: &EdgeErrorBody,
) -> PolyError {
    use reqwest::StatusCode;

    let function = function_name.to_string();
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return PolyError::Edge(EdgeError::Forbidden { function, message: error.message.clone() });
    }
    if status.is_server_error() {
        // 5xx without an explicit "do not retry" hint is transient by default.
        if error.code.eq_ignore_ascii_case("permanent") {
            return PolyError::Edge(EdgeError::Permanent {
                function,
                message: error.message.clone(),
            });
        }
        return PolyError::Edge(EdgeError::Transient { function, message: error.message.clone() });
    }
    // Otherwise we have a 4xx (or 200 success:false). Use the code to disambiguate.
    let code = error.code.to_ascii_lowercase();
    if code.contains("conflict") || code.contains("version") || code.contains("idempotency") {
        return PolyError::Edge(EdgeError::Conflict {
            function,
            code: error.code.clone(),
            message: error.message.clone(),
        });
    }
    PolyError::Edge(EdgeError::Validation {
        function,
        code: error.code.clone(),
        message: error.message.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::ClientConfig;

    fn make_client() -> Client {
        Client::new(ClientConfig {
            supabase_url: "https://example.supabase.co".into(),
            supabase_anon_key: "anon".into(),
            encryption_secret: None,
            storage_bucket: None,
        })
        .unwrap()
    }

    #[test]
    fn url_with_op_appends_v1_op_suffix() {
        let edge = EdgeClient::new(make_client());
        let url = edge.build_url("messages-write", Some("send"));
        assert_eq!(url, "https://example.supabase.co/functions/v1/messages-write/v1/send");
    }

    #[test]
    fn url_without_op_omits_suffix() {
        let edge = EdgeClient::new(make_client());
        let url = edge.build_url("notify-new-message", None);
        assert_eq!(url, "https://example.supabase.co/functions/v1/notify-new-message");
    }

    #[test]
    fn classify_401_is_forbidden() {
        let err = classify_error(
            "x",
            reqwest::StatusCode::UNAUTHORIZED,
            &EdgeErrorBody { code: "auth".into(), message: "no jwt".into(), details: None },
        );
        match err {
            PolyError::Edge(EdgeError::Forbidden { .. }) => {}
            other => panic!("wrong: {other:?}"),
        }
    }

    #[test]
    fn classify_5xx_is_transient_by_default() {
        let err = classify_error(
            "x",
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            &EdgeErrorBody { code: "boom".into(), message: "oops".into(), details: None },
        );
        match err {
            PolyError::Edge(EdgeError::Transient { .. }) => {}
            other => panic!("wrong: {other:?}"),
        }
    }

    #[test]
    fn classify_conflict_code_routes_to_conflict() {
        let err = classify_error(
            "x",
            reqwest::StatusCode::BAD_REQUEST,
            &EdgeErrorBody {
                code: "version_conflict".into(),
                message: "stale".into(),
                details: None,
            },
        );
        match err {
            PolyError::Edge(EdgeError::Conflict { .. }) => {}
            other => panic!("wrong: {other:?}"),
        }
    }
}
