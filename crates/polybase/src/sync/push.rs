//! Push pipeline: PostgREST upsert + tombstone UPDATE for non-Edge entities.
//!
//! For entities whose [`WritePath`] is [`crate::registry::WritePath::PostgREST`], the
//! sync coordinator dispatches here. Edge-routed entities go through [`crate::edge::EdgeClient`]
//! instead. Both paths share the [`crate::sync::EchoTracker`] so realtime echoes are suppressed
//! identically.
//!
//! This module also exposes [`is_permanent_push_error_message`], the canonical "should this
//! drop from the offline queue or be retried" classifier, ported verbatim from Swift PolyBase's
//! `polybaseIsPermanentOfflineQueueError` (see `polykit-swift/PolyBase/Sync/SyncCoordinator.swift`).

use serde_json::{Map, Value};

use crate::client::Client;
use crate::errors::{PolyError, PushError};
use crate::sync::echo::EchoTracker;

/// PostgREST pusher.
#[derive(Debug, Clone)]
pub(crate) struct Pusher {
    client: Client,
    echo: EchoTracker,
}

impl Pusher {
    pub(crate) fn new(client: Client, echo: EchoTracker) -> Self {
        Self { client, echo }
    }

    /// Upsert a single row to `{table}` via PostgREST.
    ///
    /// `conflict_target` is the comma-separated column list for PostgREST's `on_conflict`
    /// query parameter and must match an actual unique / exclusion constraint on the table.
    /// Most entities pass `"id"`; entities with a composite primary key (e.g. KVS uses
    /// `"id,user_id"`) override via [`crate::registry::EntityConfig::conflict_target`].
    ///
    /// CRITICAL ORDERING: marks echo BEFORE awaiting the network call so realtime cannot
    /// race ahead and re-deliver our own write into the merge pipeline.
    pub(crate) async fn upsert(
        &self,
        table: &str,
        record: Map<String, Value>,
        conflict_target: &str,
        access_token: &str,
    ) -> Result<(), PolyError> {
        let entity_id = record.get("id").and_then(Value::as_str).ok_or_else(|| {
            PolyError::Push(PushError::Permanent {
                table: table.into(),
                message: "record missing 'id' field".into(),
            })
        })?;

        // Mark echo first — see comment above.
        self.echo.mark_pushed(table, entity_id);

        let url = self.client.rest_url(table);
        let resp = self
            .client
            .http()
            .post(url)
            .header("apikey", &self.client.config().supabase_anon_key)
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Prefer", "resolution=merge-duplicates,return=minimal")
            .query(&[("on_conflict", conflict_target)])
            .json(&vec![record])
            .send()
            .await?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let body = resp.text().await.unwrap_or_default();
        let lower = body.to_ascii_lowercase();
        if lower.contains("same-version mutation") || lower.contains("ignored") {
            return Err(PolyError::Push(PushError::SameVersionMutationIgnored {
                table: table.into(),
            }));
        }
        // Classify by message body, not status code. Swift PolyBase has shown that 5xx responses
        // can carry permanent rejections (e.g. constraint violations surfaced as 500s by triggers)
        // and 4xx responses can be transient (rate limits, transient PostgREST hiccups). The
        // canonical pattern list is the source of truth.
        let message = format!("HTTP {}: {body}", status.as_u16());
        Err(PolyError::Push(classify_push_error_message(table, &message)))
    }

    /// Tombstone update: PATCH only `version`, `deleted`, `updated_at`. Avoids re-writing
    /// NOT NULL columns the caller may not have on hand at delete time.
    pub(crate) async fn update_tombstone(
        &self,
        table: &str,
        entity_id: &str,
        version: i64,
        updated_at: &str,
        access_token: &str,
    ) -> Result<(), PolyError> {
        self.echo.mark_pushed(table, entity_id);

        let url = self.client.rest_url(table);
        let body = serde_json::json!({
            "version": version,
            "deleted": true,
            "updated_at": updated_at,
        });
        let resp = self
            .client
            .http()
            .patch(url)
            .header("apikey", &self.client.config().supabase_anon_key)
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Prefer", "return=minimal")
            .query(&[("id", &format!("eq.{entity_id}"))])
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let body_text = resp.text().await.unwrap_or_default();
        let message = format!("HTTP {}: {body_text}", status.as_u16());
        Err(PolyError::Push(classify_push_error_message(table, &message)))
    }

    #[allow(dead_code)]
    pub(crate) fn echo_tracker(&self) -> &EchoTracker {
        &self.echo
    }
}

/// Patterns that mean a push rejection is permanent and the offline queue should drop the
/// operation rather than retry it. Lifted verbatim from Swift PolyBase's
/// `polybaseIsPermanentOfflineQueueError` (see `polykit-swift/PolyBase/Sync/SyncCoordinator.swift`).
///
/// Notably absent: `violates foreign key constraint`. A FK violation usually means the parent row
/// is en route via realtime/pull and a retry will succeed once the parent lands. Treating it as
/// permanent would drop legitimate writes during catch-up; Swift's lesson here is "let it retry".
const PERMANENT_PUSH_ERROR_PATTERNS: &[&str] = &[
    "version regression",
    "is immutable",
    "same-version mutation",
    "undelete requires version",
    "string is too long for tsvector",
    "value too long",
    "violates check constraint",
    "violates not-null constraint",
    "violates unique constraint",
    "invalid input syntax",
];

/// True when the (case-insensitive) message body contains any of the known permanent-rejection
/// patterns. Use this from any push path — Edge functions, PostgREST upserts, custom executors —
/// to decide whether a remote rejection should drop the operation from the offline queue or be
/// retried.
///
/// The patterns are the canonical battle-tested list from Swift PolyBase. Any new pattern added
/// here must be backed by real production evidence (server-side trigger message, Postgres error
/// class, etc.) — speculative additions will silently drop legitimate writes.
pub fn is_permanent_push_error_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    PERMANENT_PUSH_ERROR_PATTERNS.iter().any(|pattern| lower.contains(pattern))
}

/// Classify a free-form error message into a [`PushError`]. Convenience wrapper around
/// [`is_permanent_push_error_message`] for call sites that already need a typed error.
pub(crate) fn classify_push_error_message(table: &str, message: &str) -> PushError {
    if is_permanent_push_error_message(message) {
        PushError::Permanent { table: table.into(), message: message.into() }
    } else {
        PushError::Transient { table: table.into(), message: message.into() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifier_matches_swift_polybase_list() {
        for pattern in PERMANENT_PUSH_ERROR_PATTERNS {
            assert!(
                is_permanent_push_error_message(pattern),
                "expected pattern to match itself: {pattern}",
            );
        }
    }

    #[test]
    fn classifier_is_case_insensitive() {
        assert!(is_permanent_push_error_message("ERROR: VERSION REGRESSION detected"));
        assert!(is_permanent_push_error_message("Field IS IMMUTABLE for personas/123"));
    }

    #[test]
    fn fk_violations_are_intentionally_transient() {
        // Swift PolyBase deliberately omits FK violations from the permanent list — parent rows
        // arrive via realtime/pull during catch-up and a retry will succeed. The Rust port had a
        // regression that classified FK as permanent; this test pins the Swift behavior.
        assert!(!is_permanent_push_error_message(
            "insert or update on table \"messages\" violates foreign key constraint",
        ));
    }

    #[test]
    fn classify_uuid_error_is_permanent() {
        assert!(matches!(
            classify_push_error_message("t", "invalid input syntax for type uuid"),
            PushError::Permanent { .. }
        ));
    }

    #[test]
    fn classify_unknown_is_transient() {
        assert!(matches!(
            classify_push_error_message("t", "connection reset"),
            PushError::Transient { .. }
        ));
    }

    #[test]
    fn classify_undelete_threshold_is_permanent() {
        assert!(matches!(
            classify_push_error_message("t", "undelete requires version >= 1005"),
            PushError::Permanent { .. }
        ));
    }

    #[test]
    fn classify_constraint_violations_are_permanent() {
        for body in [
            "value too long for type character varying(255)",
            "duplicate key violates unique constraint \"messages_pkey\"",
            "null value in column \"id\" violates not-null constraint",
            "string is too long for tsvector",
            "new row for relation \"messages\" violates check constraint \"role_check\"",
        ] {
            assert!(
                is_permanent_push_error_message(body),
                "expected permanent classification for: {body}",
            );
        }
    }
}
