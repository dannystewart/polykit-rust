//! Remote read / write protocols.
//!
//! Direct port of Swift PolyBase's `Sync/RemoteWriter.swift` and `Sync/RemoteReader.swift`.
//!
//! Two narrow traits:
//! - [`RemoteWriter`]: upsert single / batch records, update specific fields by id.
//! - [`RemoteReader`]: fetch full records (optionally scoped by `user_id` plus extra equality
//!   filters) and fetch the lightweight `(version, deleted)` projection used by reconcile.
//!
//! ## Why two traits, not one
//!
//! Swift PolyBase split read and write so consumers can wire memory test impls for one path
//! while still hitting the live PostgREST surface on the other. Tauri Prism's earlier `SyncRemote`
//! collapsed both into a single push trait, which forced reconcile pulls to bypass the trait and
//! reach into PostgREST directly. The split here removes that asymmetry.
//!
//! ## Auth model
//!
//! Methods accept `access_token: &str` per call, mirroring the rest of the polybase HTTP surface
//! (e.g. [`crate::sync::push::Pusher::upsert`]). The coordinator owns session lifecycle and
//! threads the token through; impls do not hold session state.
//!
//! ## Filter model
//!
//! Swift's `RemoteFilter` is a closure over a typed PostgREST builder. The Rust port uses a
//! structured [`RemoteFilter`] type — a list of equality predicates — because we do not have a
//! type-safe PostgREST builder. This is enough for every current PolyBase consumer; richer
//! predicates (range, `in`, etc.) can be added when first needed without breaking the trait.

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::client::Client;
use crate::errors::{PolyError, PullError, PushError};
use crate::sync::echo::EchoTracker;
use crate::sync::push::classify_push_error_message;

/// Optional structured predicate filter for [`RemoteReader::fetch_records`].
///
/// Each entry becomes an additional `column=eq.value` query parameter on top of the
/// `user_id_column = user_id` scope (when `user_id` is provided).
#[derive(Default, Debug, Clone)]
pub struct RemoteFilter {
    /// `column = value` equality predicates (case-sensitive).
    pub eq: Vec<(String, String)>,
}

impl RemoteFilter {
    /// Construct a filter from a slice of borrowed `(column, value)` pairs.
    pub fn from_pairs<C: Into<String>, V: Into<String>>(pairs: Vec<(C, V)>) -> Self {
        Self { eq: pairs.into_iter().map(|(c, v)| (c.into(), v.into())).collect() }
    }
}

/// Write-side remote operations.
///
/// All methods are blocking from the caller's perspective; the implementation is responsible
/// for any underlying transport (HTTP, in-memory, etc.). Errors should be classified using
/// [`crate::sync::push::classify_push_error_message`] so the offline-queue retry decision is
/// uniform across implementations.
#[async_trait]
pub trait RemoteWriter: Send + Sync {
    /// Upsert a single record into `table`.
    async fn upsert_record(
        &self,
        table: &str,
        record: Map<String, Value>,
        access_token: &str,
    ) -> Result<(), PolyError>;

    /// Upsert a batch of records into `table`. Implementations may reject heterogeneous
    /// shapes; the coordinator never mixes entity types within a single call.
    async fn upsert_records(
        &self,
        table: &str,
        records: Vec<Map<String, Value>>,
        access_token: &str,
    ) -> Result<(), PolyError>;

    /// Update only `fields` for the row matching `id`. Used for tombstones (`version`,
    /// `deleted`, `updated_at`) and any other partial update where re-writing NOT NULL
    /// columns at delete time is undesirable.
    async fn update_fields(
        &self,
        table: &str,
        id: &str,
        fields: Map<String, Value>,
        access_token: &str,
    ) -> Result<(), PolyError>;
}

/// Read-side remote operations.
#[async_trait]
pub trait RemoteReader: Send + Sync {
    /// Fetch all rows from `table`, optionally scoped by `user_id_column = user_id` and any
    /// extra equality predicates in `filter`.
    async fn fetch_records(
        &self,
        table: &str,
        user_id_column: &str,
        user_id: Option<&str>,
        filter: Option<&RemoteFilter>,
        access_token: &str,
    ) -> Result<Vec<Map<String, Value>>, PolyError>;

    /// Fetch the `(version, deleted)` projection for every row, keyed by `id`. Used by reconcile
    /// to plan pulls / pushes / tombstone adoptions without dragging full rows over the wire.
    async fn fetch_versions(
        &self,
        table: &str,
        user_id_column: &str,
        user_id: Option<&str>,
        access_token: &str,
    ) -> Result<HashMap<String, (i64, bool)>, PolyError>;
}

/// Live PostgREST writer.
#[derive(Debug, Clone)]
pub struct SupabaseRemoteWriter {
    client: Client,
    echo: EchoTracker,
}

impl SupabaseRemoteWriter {
    /// Build a writer over the given client and echo tracker. Pass the same `EchoTracker`
    /// instance the rest of the sync layer uses so realtime echo suppression stays coherent.
    pub fn new(client: Client, echo: EchoTracker) -> Self {
        Self { client, echo }
    }

    fn rest_url(&self, table: &str) -> String {
        self.client.rest_url(table)
    }
}

#[async_trait]
impl RemoteWriter for SupabaseRemoteWriter {
    async fn upsert_record(
        &self,
        table: &str,
        record: Map<String, Value>,
        access_token: &str,
    ) -> Result<(), PolyError> {
        // Mark echo before awaiting so realtime cannot race ahead of the response and re-deliver
        // our own write. Skipped silently when the record lacks an `id` (caller error path will
        // surface below).
        if let Some(id) = record.get("id").and_then(Value::as_str) {
            self.echo.mark_pushed(table, id);
        }
        let resp = self
            .client
            .http()
            .post(self.rest_url(table))
            .header("apikey", &self.client.config().supabase_anon_key)
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Prefer", "resolution=merge-duplicates,return=minimal")
            .query(&[("on_conflict", "id")])
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
        Err(PolyError::Push(classify_push_error_message(
            table,
            &format!("HTTP {}: {body}", status.as_u16()),
        )))
    }

    async fn upsert_records(
        &self,
        table: &str,
        records: Vec<Map<String, Value>>,
        access_token: &str,
    ) -> Result<(), PolyError> {
        if records.is_empty() {
            return Ok(());
        }
        for record in &records {
            if let Some(id) = record.get("id").and_then(Value::as_str) {
                self.echo.mark_pushed(table, id);
            }
        }
        let resp = self
            .client
            .http()
            .post(self.rest_url(table))
            .header("apikey", &self.client.config().supabase_anon_key)
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Prefer", "resolution=merge-duplicates,return=minimal")
            .query(&[("on_conflict", "id")])
            .json(&records)
            .send()
            .await?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let body = resp.text().await.unwrap_or_default();
        Err(PolyError::Push(classify_push_error_message(
            table,
            &format!("HTTP {}: {body}", status.as_u16()),
        )))
    }

    async fn update_fields(
        &self,
        table: &str,
        id: &str,
        fields: Map<String, Value>,
        access_token: &str,
    ) -> Result<(), PolyError> {
        self.echo.mark_pushed(table, id);
        let resp = self
            .client
            .http()
            .patch(self.rest_url(table))
            .header("apikey", &self.client.config().supabase_anon_key)
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Prefer", "return=minimal")
            .query(&[("id", &format!("eq.{id}"))])
            .json(&fields)
            .send()
            .await?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let body = resp.text().await.unwrap_or_default();
        Err(PolyError::Push(classify_push_error_message(
            table,
            &format!("HTTP {}: {body}", status.as_u16()),
        )))
    }
}

/// Live PostgREST reader.
#[derive(Debug, Clone)]
pub struct SupabaseRemoteReader {
    client: Client,
}

impl SupabaseRemoteReader {
    /// Build a reader over the given client.
    pub fn new(client: Client) -> Self {
        Self { client }
    }
}

#[async_trait]
impl RemoteReader for SupabaseRemoteReader {
    async fn fetch_records(
        &self,
        table: &str,
        user_id_column: &str,
        user_id: Option<&str>,
        filter: Option<&RemoteFilter>,
        access_token: &str,
    ) -> Result<Vec<Map<String, Value>>, PolyError> {
        let mut query: Vec<(String, String)> = vec![("select".into(), "*".into())];
        if let Some(uid) = user_id {
            query.push((user_id_column.into(), format!("eq.{uid}")));
        }
        if let Some(extra) = filter {
            for (column, value) in &extra.eq {
                query.push((column.clone(), format!("eq.{value}")));
            }
        }
        let resp = self
            .client
            .http()
            .get(self.client.rest_url(table))
            .header("apikey", &self.client.config().supabase_anon_key)
            .header("Authorization", format!("Bearer {access_token}"))
            .query(&query)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(PolyError::Pull(PullError::Failed {
                table: table.into(),
                message: format!("HTTP {}: {body}", status.as_u16()),
            }));
        }
        let value: Value = resp.json().await?;
        Ok(value
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| match v {
                Value::Object(map) => Some(map),
                _ => None,
            })
            .collect())
    }

    async fn fetch_versions(
        &self,
        table: &str,
        user_id_column: &str,
        user_id: Option<&str>,
        access_token: &str,
    ) -> Result<HashMap<String, (i64, bool)>, PolyError> {
        let mut query: Vec<(String, String)> = vec![("select".into(), "id,version,deleted".into())];
        if let Some(uid) = user_id {
            query.push((user_id_column.into(), format!("eq.{uid}")));
        }
        let resp = self
            .client
            .http()
            .get(self.client.rest_url(table))
            .header("apikey", &self.client.config().supabase_anon_key)
            .header("Authorization", format!("Bearer {access_token}"))
            .query(&query)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(PolyError::Pull(PullError::Failed {
                table: table.into(),
                message: format!("HTTP {}: {body}", status.as_u16()),
            }));
        }
        let rows: Vec<Value> = resp.json().await?;
        let mut versions = HashMap::with_capacity(rows.len());
        for row in rows {
            let Value::Object(obj) = row else { continue };
            let Some(id) = obj.get("id").and_then(Value::as_str) else { continue };
            let Some(version) = obj.get("version").and_then(Value::as_i64) else { continue };
            let deleted = obj.get("deleted").and_then(Value::as_bool).unwrap_or(false);
            versions.insert(id.to_string(), (version, deleted));
        }
        Ok(versions)
    }
}

// -- Memory test impls -------------------------------------------------------------------------

/// In-memory writer for tests. Records every call into a shared log; never fails.
#[derive(Debug, Default, Clone)]
pub struct MemoryWriter {
    inner: std::sync::Arc<parking_lot::Mutex<MemoryWriterState>>,
}

#[derive(Debug, Default)]
struct MemoryWriterState {
    upserts: Vec<(String, Vec<Map<String, Value>>)>,
    updates: Vec<(String, String, Map<String, Value>)>,
}

impl MemoryWriter {
    /// New empty writer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot all upserts logged so far, as `(table, records)` tuples in call order.
    pub fn upserts(&self) -> Vec<(String, Vec<Map<String, Value>>)> {
        self.inner.lock().upserts.clone()
    }

    /// Snapshot all field updates logged so far, as `(table, id, fields)` in call order.
    pub fn updates(&self) -> Vec<(String, String, Map<String, Value>)> {
        self.inner.lock().updates.clone()
    }
}

#[async_trait]
impl RemoteWriter for MemoryWriter {
    async fn upsert_record(
        &self,
        table: &str,
        record: Map<String, Value>,
        _access_token: &str,
    ) -> Result<(), PolyError> {
        self.inner.lock().upserts.push((table.into(), vec![record]));
        Ok(())
    }

    async fn upsert_records(
        &self,
        table: &str,
        records: Vec<Map<String, Value>>,
        _access_token: &str,
    ) -> Result<(), PolyError> {
        self.inner.lock().upserts.push((table.into(), records));
        Ok(())
    }

    async fn update_fields(
        &self,
        table: &str,
        id: &str,
        fields: Map<String, Value>,
        _access_token: &str,
    ) -> Result<(), PolyError> {
        self.inner.lock().updates.push((table.into(), id.into(), fields));
        Ok(())
    }
}

/// In-memory reader for tests. Returns rows the test seeded into a per-table store.
#[derive(Debug, Default, Clone)]
pub struct MemoryReader {
    inner: std::sync::Arc<parking_lot::Mutex<HashMap<String, Vec<Map<String, Value>>>>>,
}

impl MemoryReader {
    /// New empty reader with no rows seeded.
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed `rows` for `table`, replacing any previously seeded rows for that table.
    pub fn seed(&self, table: impl Into<String>, rows: Vec<Map<String, Value>>) {
        self.inner.lock().insert(table.into(), rows);
    }
}

#[async_trait]
impl RemoteReader for MemoryReader {
    async fn fetch_records(
        &self,
        table: &str,
        user_id_column: &str,
        user_id: Option<&str>,
        filter: Option<&RemoteFilter>,
        _access_token: &str,
    ) -> Result<Vec<Map<String, Value>>, PolyError> {
        let guard = self.inner.lock();
        let rows = guard.get(table).cloned().unwrap_or_default();
        let filtered = rows.into_iter().filter(|row| {
            if let Some(uid) = user_id
                && row.get(user_id_column).and_then(Value::as_str) != Some(uid)
            {
                return false;
            }
            if let Some(extra) = filter {
                for (column, value) in &extra.eq {
                    if row.get(column).and_then(Value::as_str) != Some(value.as_str()) {
                        return false;
                    }
                }
            }
            true
        });
        Ok(filtered.collect())
    }

    async fn fetch_versions(
        &self,
        table: &str,
        user_id_column: &str,
        user_id: Option<&str>,
        _access_token: &str,
    ) -> Result<HashMap<String, (i64, bool)>, PolyError> {
        let guard = self.inner.lock();
        let rows = guard.get(table).cloned().unwrap_or_default();
        let mut versions = HashMap::new();
        for row in rows {
            if let Some(uid) = user_id
                && row.get(user_id_column).and_then(Value::as_str) != Some(uid)
            {
                continue;
            }
            let Some(id) = row.get("id").and_then(Value::as_str) else { continue };
            let Some(version) = row.get("version").and_then(Value::as_i64) else { continue };
            let deleted = row.get("deleted").and_then(Value::as_bool).unwrap_or(false);
            versions.insert(id.to_string(), (version, deleted));
        }
        Ok(versions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obj(pairs: &[(&str, Value)]) -> Map<String, Value> {
        pairs.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect()
    }

    #[tokio::test]
    async fn memory_writer_logs_upserts_in_order() {
        let writer = MemoryWriter::new();
        writer.upsert_record("messages", obj(&[("id", json!("a"))]), "tok").await.unwrap();
        writer
            .upsert_records(
                "messages",
                vec![obj(&[("id", json!("b"))]), obj(&[("id", json!("c"))])],
                "tok",
            )
            .await
            .unwrap();

        let upserts = writer.upserts();
        assert_eq!(upserts.len(), 2);
        assert_eq!(upserts[0].0, "messages");
        assert_eq!(upserts[0].1.len(), 1);
        assert_eq!(upserts[1].1.len(), 2);
    }

    #[tokio::test]
    async fn memory_writer_logs_updates() {
        let writer = MemoryWriter::new();
        writer
            .update_fields("messages", "abc", obj(&[("deleted", json!(true))]), "tok")
            .await
            .unwrap();
        let updates = writer.updates();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].1, "abc");
    }

    #[tokio::test]
    async fn memory_reader_filters_by_user_id_and_extra_eq() {
        let reader = MemoryReader::new();
        reader.seed(
            "messages",
            vec![
                obj(&[
                    ("id", json!("1")),
                    ("user_id", json!("alice")),
                    ("conversation_id", json!("X")),
                    ("version", json!(1)),
                    ("deleted", json!(false)),
                ]),
                obj(&[
                    ("id", json!("2")),
                    ("user_id", json!("bob")),
                    ("conversation_id", json!("X")),
                    ("version", json!(1)),
                    ("deleted", json!(false)),
                ]),
                obj(&[
                    ("id", json!("3")),
                    ("user_id", json!("alice")),
                    ("conversation_id", json!("Y")),
                    ("version", json!(2)),
                    ("deleted", json!(false)),
                ]),
            ],
        );

        let alice_only =
            reader.fetch_records("messages", "user_id", Some("alice"), None, "tok").await.unwrap();
        assert_eq!(alice_only.len(), 2);

        let alice_in_x = reader
            .fetch_records(
                "messages",
                "user_id",
                Some("alice"),
                Some(&RemoteFilter::from_pairs(vec![("conversation_id", "X")])),
                "tok",
            )
            .await
            .unwrap();
        assert_eq!(alice_in_x.len(), 1);
        assert_eq!(alice_in_x[0].get("id").and_then(Value::as_str), Some("1"));
    }

    #[tokio::test]
    async fn memory_reader_returns_versions_keyed_by_id() {
        let reader = MemoryReader::new();
        reader.seed(
            "personas",
            vec![
                obj(&[
                    ("id", json!("p1")),
                    ("user_id", json!("alice")),
                    ("version", json!(7)),
                    ("deleted", json!(false)),
                ]),
                obj(&[
                    ("id", json!("p2")),
                    ("user_id", json!("alice")),
                    ("version", json!(3)),
                    ("deleted", json!(true)),
                ]),
            ],
        );
        let versions =
            reader.fetch_versions("personas", "user_id", Some("alice"), "tok").await.unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions.get("p1"), Some(&(7, false)));
        assert_eq!(versions.get("p2"), Some(&(3, true)));
    }
}
