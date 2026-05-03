//! Push pipeline: PostgREST upsert + tombstone UPDATE for non-Edge entities.
//!
//! For entities whose [`WritePath`] is [`crate::registry::WritePath::PostgREST`], the
//! sync coordinator dispatches here. Edge-routed entities go through [`crate::edge::EdgeClient`]
//! instead. Both paths share the [`crate::sync::EchoTracker`] so realtime echoes are suppressed
//! identically.

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
        if status.is_server_error() {
            return Err(PolyError::Push(PushError::Transient {
                table: table.into(),
                message: format!("HTTP {}: {body}", status.as_u16()),
            }));
        }
        Err(PolyError::Push(PushError::Permanent {
            table: table.into(),
            message: format!("HTTP {}: {body}", status.as_u16()),
        }))
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
        if status.is_server_error() {
            return Err(PolyError::Push(PushError::Transient {
                table: table.into(),
                message: format!("HTTP {}: {body_text}", status.as_u16()),
            }));
        }
        Err(PolyError::Push(PushError::Permanent {
            table: table.into(),
            message: format!("HTTP {}: {body_text}", status.as_u16()),
        }))
    }

    #[allow(dead_code)]
    pub(crate) fn echo_tracker(&self) -> &EchoTracker {
        &self.echo
    }
}

/// Classify a free-form error message into a [`PushError`] for callers that catch lower-level
/// errors and need to decide whether to enqueue for retry. Mirrors the heuristic from Swift
/// PolyBase's `polybaseIsPermanentOfflineQueueError`.
#[allow(dead_code)]
pub(crate) fn classify_push_error_message(table: &str, message: &str) -> PushError {
    let lower = message.to_ascii_lowercase();
    if lower.contains("undelete requires version")
        || lower.contains("invalid input syntax for type uuid")
        || lower.contains("violates foreign key constraint")
        || lower.contains("violates check constraint")
        || lower.contains("version regression")
    {
        return PushError::Permanent { table: table.into(), message: message.into() };
    }
    PushError::Transient { table: table.into(), message: message.into() }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
