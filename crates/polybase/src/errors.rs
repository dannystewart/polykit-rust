//! Top-level and per-subsystem error types.

use thiserror::Error;

use crate::contract::{is_same_version_mutation_message, is_version_regression_message};
use crate::encryption::EncryptionError;

/// Top-level error covering anything PolyBase can fail at.
#[derive(Debug, Error)]
pub enum PolyError {
    /// Configuration is incomplete or invalid.
    #[error("polybase not configured: {0}")]
    NotConfigured(String),

    /// No active session — caller must sign in (or hand off a session).
    #[error("no active session")]
    NoSession,

    /// Underlying HTTP transport failure.
    #[error(transparent)]
    Http(#[from] reqwest::Error),

    /// JSON serialization/deserialization failure.
    #[error(transparent)]
    Json(#[from] serde_json::Error),

    /// Edge Function call failed.
    #[error(transparent)]
    Edge(#[from] EdgeError),

    /// Push to remote failed.
    #[error(transparent)]
    Push(#[from] PushError),

    /// Pull from remote failed.
    #[error(transparent)]
    Pull(#[from] PullError),

    /// Registry / field-mapping problem.
    #[error(transparent)]
    Registry(#[from] RegistryError),

    /// Storage bucket operation failed.
    #[error(transparent)]
    Storage(#[from] StorageError),

    /// Offline queue operation failed.
    #[error(transparent)]
    OfflineQueue(#[from] OfflineQueueError),

    /// Local persistence (LocalStore) failed.
    #[error("local store error: {0}")]
    Local(String),

    /// Encryption failure (key derivation, AES-GCM, or base64).
    #[error(transparent)]
    Encryption(#[from] EncryptionError),

    /// Catch-all for app-level callbacks failing.
    #[error("{0}")]
    Other(String),
}

impl PolyError {
    /// Convenience: build a `PolyError::Other` from any displayable error.
    pub fn other(message: impl Into<String>) -> Self {
        Self::Other(message.into())
    }
}

/// Error class for Edge Function calls. Mirrors the structured error contract used by all
/// `*-write` Supabase Edge Functions in Tauri Prism: `{ success: false, error: { code, message } }`.
#[derive(Debug, Error, Clone)]
pub enum EdgeError {
    /// Validation failure on the server (HTTP 4xx). Should NOT be retried.
    #[error("validation error from {function}: {code}: {message}")]
    Validation {
        /// Edge Function name that produced the error.
        function: String,
        /// Server-supplied error code.
        code: String,
        /// Human-readable message.
        message: String,
    },

    /// Conflict: version regression, idempotency mismatch, etc. Should NOT be retried.
    #[error("conflict from {function}: {code}: {message}")]
    Conflict {
        /// Edge Function name that produced the error.
        function: String,
        /// Server-supplied error code (e.g. `version_conflict`).
        code: String,
        /// Human-readable message.
        message: String,
    },

    /// Authorization failure (HTTP 401/403). Caller should refresh session and retry.
    #[error("forbidden from {function}: {message}")]
    Forbidden {
        /// Edge Function name that produced the error.
        function: String,
        /// Human-readable message.
        message: String,
    },

    /// Transient failure — network, 5xx, or rate-limited. Retry with backoff.
    #[error("transient error from {function}: {message}")]
    Transient {
        /// Edge Function name that produced the error.
        function: String,
        /// Human-readable message.
        message: String,
    },

    /// Permanent server-side failure (HTTP 5xx with no retry). Drop from queue.
    #[error("permanent error from {function}: {message}")]
    Permanent {
        /// Edge Function name that produced the error.
        function: String,
        /// Human-readable message.
        message: String,
    },

    /// Decode failure — server returned non-conforming response.
    #[error("decode failed for {function}: {message}")]
    Decode {
        /// Edge Function name whose response failed to decode.
        function: String,
        /// Decoder error detail.
        message: String,
    },
}

impl EdgeError {
    /// Should the offline queue retry this?
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Transient { .. } | Self::Forbidden { .. })
    }

    /// Borrow the human-readable message body, regardless of variant. Useful for caller-side
    /// classification — e.g. distinguishing a benign same-version mutation surfaced through
    /// an Edge Function from a real `Conflict` that should propagate to the user.
    pub fn message(&self) -> &str {
        match self {
            Self::Validation { message, .. }
            | Self::Conflict { message, .. }
            | Self::Forbidden { message, .. }
            | Self::Transient { message, .. }
            | Self::Permanent { message, .. }
            | Self::Decode { message, .. } => message,
        }
    }

    /// Borrow the Edge Function name that produced the error.
    pub fn function(&self) -> &str {
        match self {
            Self::Validation { function, .. }
            | Self::Conflict { function, .. }
            | Self::Forbidden { function, .. }
            | Self::Transient { function, .. }
            | Self::Permanent { function, .. }
            | Self::Decode { function, .. } => function,
        }
    }

    /// True when this Edge error wraps the trigger's benign same-version mutation rejection.
    /// Caller should treat the operation as a no-op success — the push already happened
    /// (typically from a concurrent task on the same client). Mirrors Swift `PolyBase`'s
    /// `isSameVersionMutationError` check.
    pub fn is_benign_same_version_mutation(&self) -> bool {
        is_same_version_mutation_message(self.message())
    }

    /// True when this Edge error wraps a version regression rejection. Caller should drop the
    /// operation rather than retry, and ideally trigger a reconcile pull so the local mirror
    /// catches up with the newer remote state.
    pub fn is_version_regression(&self) -> bool {
        is_version_regression_message(self.message())
    }
}

/// Error class for push operations (PostgREST upserts, tombstone updates).
#[derive(Debug, Error, Clone)]
pub enum PushError {
    /// Network or server-side transient failure.
    #[error("transient push failure on {table}: {message}")]
    Transient {
        /// Table name being pushed.
        table: String,
        /// Human-readable message.
        message: String,
    },

    /// Version regression, constraint violation, or invalid UUID — do not retry.
    #[error("permanent push failure on {table}: {message}")]
    Permanent {
        /// Table name being pushed.
        table: String,
        /// Human-readable message.
        message: String,
    },

    /// Same-version mutation; treated as benign no-op by callers.
    #[error("same-version mutation ignored on {table}")]
    SameVersionMutationIgnored {
        /// Table name being pushed.
        table: String,
    },
}

impl PushError {
    /// True for transport-level failures the offline queue should retry.
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Transient { .. })
    }

    /// True for failures that should drop the operation rather than retry.
    pub fn is_permanent(&self) -> bool {
        matches!(self, Self::Permanent { .. })
    }

    /// True when a `Permanent` push failure carries the version regression marker. The
    /// coordinator uses this to fire a `PolyEvent::VersionRegressionDetected` so consumers
    /// can trigger a targeted pull to converge with the newer remote state.
    pub fn is_version_regression(&self) -> bool {
        match self {
            Self::Permanent { message, .. } => is_version_regression_message(message),
            _ => false,
        }
    }
}

/// Error class for pull / merge operations.
#[derive(Debug, Error, Clone)]
pub enum PullError {
    /// Pull request failed (HTTP error or transport failure).
    #[error("pull failed on {table}: {message}")]
    Failed {
        /// Table name being pulled.
        table: String,
        /// Human-readable message.
        message: String,
    },

    /// Pull succeeded but the response failed to decode.
    #[error("decode failed on {table}: {message}")]
    Decode {
        /// Table name being pulled.
        table: String,
        /// Decoder error detail.
        message: String,
    },
}

/// Error class for registry / field mapping.
#[derive(Debug, Error, Clone)]
pub enum RegistryError {
    /// Lookup by table name failed because the table was never registered.
    #[error("table not registered: {0}")]
    TableNotRegistered(String),

    /// Lookup by entity type name failed because the type was never registered.
    #[error("entity type not registered: {0}")]
    TypeNotRegistered(String),

    /// Encryption failed for a registered encrypted column.
    #[error("encryption failed for {table}.{column}")]
    EncryptionFailed {
        /// Table name owning the column.
        table: String,
        /// Column name that failed to encrypt.
        column: String,
    },

    /// Required field was missing from a write payload.
    #[error("missing required field {field} on table {table}")]
    MissingField {
        /// Table name owning the field.
        table: String,
        /// Missing field name.
        field: String,
    },

    /// Active user id was missing when constructing a write payload.
    #[error("write payload missing active user id on table {0}")]
    MissingUserId(String),

    /// A registered factory function failed to deserialize a remote-shaped row. Distinct from
    /// [`Self::TableNotRegistered`] / [`Self::TypeNotRegistered`] (which are lookup failures)
    /// — this fires when the factory IS present but its body returned an error (typically a
    /// `serde` deserialization issue or a `from_remote_map` validation failure).
    #[error("factory failed for {table}: {message}")]
    FactoryFailed {
        /// Table name whose factory failed.
        table: String,
        /// Human-readable detail from the underlying deserializer or validator.
        message: String,
    },

    /// Remote schema introspection (e.g. PostgREST OpenAPI) returned a payload that didn't
    /// match the expected shape. Used by [`crate::registry::schema::SchemaSnapshot::from_openapi`]
    /// when the JSON document is missing the `components.schemas` envelope or otherwise
    /// malformed.
    #[error("invalid remote schema: {0}")]
    InvalidRemoteSchema(String),
}

/// Error class for Storage bucket operations.
#[derive(Debug, Error, Clone)]
pub enum StorageError {
    /// Storage REST request failed.
    #[error("storage error on {bucket}/{path}: {message}")]
    Failed {
        /// Bucket name.
        bucket: String,
        /// Object path within the bucket.
        path: String,
        /// Human-readable message.
        message: String,
    },

    /// Storage object did not exist (HTTP 404).
    #[error("storage object not found at {bucket}/{path}")]
    NotFound {
        /// Bucket name.
        bucket: String,
        /// Object path within the bucket.
        path: String,
    },
}

/// Error class for offline queue persistence/replay.
#[derive(Debug, Error, Clone)]
pub enum OfflineQueueError {
    /// File-system or other I/O error reading/writing the queue.
    #[error("queue io error: {0}")]
    Io(String),

    /// Queue persistence file failed to deserialize.
    #[error("queue decode failed: {0}")]
    Decode(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge_conflict_with(message: &str) -> EdgeError {
        EdgeError::Conflict {
            function: "conversations-write".into(),
            code: "version_conflict".into(),
            message: message.into(),
        }
    }

    fn edge_permanent_with(message: &str) -> EdgeError {
        EdgeError::Permanent { function: "conversations-write".into(), message: message.into() }
    }

    #[test]
    fn edge_error_message_accessor_covers_all_variants() {
        let cases: [(EdgeError, &str); 6] = [
            (
                EdgeError::Validation {
                    function: "f".into(),
                    code: "c".into(),
                    message: "v_msg".into(),
                },
                "v_msg",
            ),
            (
                EdgeError::Conflict {
                    function: "f".into(),
                    code: "c".into(),
                    message: "c_msg".into(),
                },
                "c_msg",
            ),
            (EdgeError::Forbidden { function: "f".into(), message: "f_msg".into() }, "f_msg"),
            (EdgeError::Transient { function: "f".into(), message: "t_msg".into() }, "t_msg"),
            (EdgeError::Permanent { function: "f".into(), message: "p_msg".into() }, "p_msg"),
            (EdgeError::Decode { function: "f".into(), message: "d_msg".into() }, "d_msg"),
        ];
        for (err, expected) in cases {
            assert_eq!(err.message(), expected, "message mismatch for {err:?}");
            assert_eq!(err.function(), "f", "function mismatch for {err:?}");
        }
    }

    #[test]
    fn edge_error_classifies_benign_same_version_mutation() {
        let err = edge_conflict_with("same-version mutation is not allowed");
        assert!(err.is_benign_same_version_mutation());
        assert!(!err.is_version_regression());
    }

    #[test]
    fn edge_error_classifies_version_regression() {
        let err = edge_permanent_with("ERROR: version regression rejected");
        assert!(err.is_version_regression());
        assert!(!err.is_benign_same_version_mutation());
    }

    #[test]
    fn edge_error_unknown_message_is_neither() {
        let err = edge_conflict_with("rate limited");
        assert!(!err.is_benign_same_version_mutation());
        assert!(!err.is_version_regression());
    }

    #[test]
    fn push_error_is_version_regression_only_for_permanent_with_marker() {
        let regression = PushError::Permanent {
            table: "conversations".into(),
            message: "HTTP 500: version regression detected".into(),
        };
        assert!(regression.is_version_regression());

        let other_permanent = PushError::Permanent {
            table: "conversations".into(),
            message: "HTTP 500: violates check constraint".into(),
        };
        assert!(!other_permanent.is_version_regression());

        let transient = PushError::Transient {
            table: "conversations".into(),
            message: "version regression".into(),
        };
        assert!(
            !transient.is_version_regression(),
            "version regression markers on transient errors should not pretend to be regressions",
        );

        let same_version = PushError::SameVersionMutationIgnored { table: "conversations".into() };
        assert!(!same_version.is_version_regression());
    }
}
