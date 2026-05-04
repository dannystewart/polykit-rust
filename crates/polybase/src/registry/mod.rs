//! Entity registration with builder API.
//!
//! Each polybase consumer registers its synced and local-only entities on startup. The registry
//! captures table name, column field-mapping (canonical / local / remote names + types + encryption
//! flags), parent relationships for hierarchy bumping, an optional factory for deserializing
//! remote rows, and — new in v2 — the [`WritePath`] policy that decides whether mutations flow
//! through PostgREST or through an Edge Function.
//!
//! # Registration pattern
//!
//! Build a [`Registry`] once at startup and register every entity the app cares about. The
//! [`crate::sync::Coordinator`] consults it on every persist / delete to decide where to
//! dispatch the mutation.
//!
//! ```no_run
//! use std::sync::Arc;
//! use polybase::{ColumnDef, EntityConfig, ParentRelation, Registry};
//!
//! let registry = Arc::new(Registry::new());
//!
//! // Synced entity routed through an Edge Function (the heavyweight chat path).
//! registry.register(
//!     EntityConfig::synced("messages", "Message")
//!         .columns([
//!             ColumnDef::synced("id", "id", "id", "TEXT", "string", false),
//!             ColumnDef::synced("content", "content", "content", "TEXT", "string", true), // encrypted
//!             ColumnDef::synced("version", "version", "version", "INTEGER", "integer", false),
//!             ColumnDef::synced("deleted", "deleted", "deleted", "INTEGER", "boolean", false),
//!         ])
//!         .parent(ParentRelation::new("conversations", "conversation_id"))
//!         .write_via_edge("messages-write", "send"),
//! );
//!
//! // Synced entity with a composite PK that goes direct via PostgREST. The conflict target
//! // must match an actual unique / exclusion constraint on the Supabase table — see
//! // [`EntityConfig::conflict_target`] and the `kvs` migration in this crate.
//! registry.register(
//!     EntityConfig::synced("kvs", "Kvs")
//!         .columns([
//!             ColumnDef::synced("id", "id", "id", "TEXT", "string", false),
//!             ColumnDef::synced("namespace", "namespace", "namespace", "TEXT", "string", false),
//!             ColumnDef::synced("key", "key", "key", "TEXT", "string", false),
//!             ColumnDef::synced("value", "value", "value", "TEXT", "jsonb", false),
//!         ])
//!         .conflict_target("id,user_id"), // PK is (user_id, namespace, key); unique(id, user_id) backs upserts.
//! );
//! ```
//!
//! [`crate::Kvs::register`] does the second registration for you idempotently — only register
//! `kvs` manually when you want to override columns or write-path for some reason.

use std::collections::HashMap;

use parking_lot::RwLock;
use serde_json::{Map, Value};

use crate::encryption::Encryption;
use crate::errors::RegistryError;

pub mod schema;
pub use schema::{ColumnShape, SchemaSnapshot, TableShape, normalize_type};

/// Whether an entity is synced to Supabase or strictly local-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableScope {
    /// Bidirectional sync with Supabase via PostgREST or Edge Function.
    Synced,
    /// Lives only in the local mirror (e.g. sync metadata, drafts).
    LocalOnly,
}

/// Where mutations for this entity are published to.
///
/// This is the central "hybrid write-path" toggle. Synced chat tables that have `*-write` Edge
/// Functions on the server should use [`WritePath::Edge`]; lightweight tables (KVS, device_tokens)
/// that the user JWT can write to directly via PostgREST use [`WritePath::PostgREST`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WritePath {
    /// PostgREST upsert with the user JWT against `{rest_url}/rest/v1/{table}`.
    PostgREST,
    /// Edge Function call to `{functions_url}/functions/v1/{function}/v1/{op_for(...)}`.
    Edge {
        /// Edge Function name, e.g. `messages-write`.
        function: &'static str,
        /// Default operation when none is specified by the caller (e.g. `create`).
        default_op: &'static str,
    },
}

/// One column definition with field-mapping metadata.
#[derive(Debug, Clone)]
pub struct ColumnDef {
    /// Stable canonical name used internally and across platforms (e.g. `created_at`).
    pub canonical_name: &'static str,
    /// Local mirror column name. Same as canonical unless there is intentional drift.
    pub local_name: &'static str,
    /// Remote PostgREST column name. `None` means the column is local-only.
    pub remote_name: Option<&'static str>,
    /// Local SQL column type (e.g. `TEXT`, `INTEGER`).
    pub sql_type: &'static str,
    /// Remote PostgREST column type (e.g. `string`, `integer`, `boolean`).
    pub remote_type: Option<&'static str>,
    /// True when this column should be transparently encrypted/decrypted at the registry boundary.
    pub encrypted: bool,
}

impl ColumnDef {
    /// Define a column that is both stored locally AND synced to Supabase.
    pub const fn synced(
        canonical_name: &'static str,
        local_name: &'static str,
        remote_name: &'static str,
        sql_type: &'static str,
        remote_type: &'static str,
        encrypted: bool,
    ) -> Self {
        Self {
            canonical_name,
            local_name,
            remote_name: Some(remote_name),
            sql_type,
            remote_type: Some(remote_type),
            encrypted,
        }
    }

    /// Define a column that lives only in the local mirror.
    pub const fn local_only(name: &'static str, sql_type: &'static str) -> Self {
        Self {
            canonical_name: name,
            local_name: name,
            remote_name: None,
            sql_type,
            remote_type: None,
            encrypted: false,
        }
    }

    /// Resolve the effective remote name, falling back to canonical when not overridden.
    pub fn effective_remote_name(&self) -> &'static str {
        self.remote_name.unwrap_or(self.canonical_name)
    }
}

/// Parent-child link enabling hierarchy bumping (e.g. message changes bump conversation).
#[derive(Debug, Clone)]
pub struct ParentRelation {
    /// Parent table name.
    pub parent_table: &'static str,
    /// Local column on the child row that holds the parent id.
    pub parent_id_column: &'static str,
}

impl ParentRelation {
    /// Build a parent-child relation.
    pub const fn new(parent_table: &'static str, parent_id_column: &'static str) -> Self {
        Self { parent_table, parent_id_column }
    }
}

/// Factory function that turns a remote-shaped record into a JSON entity.
pub type FactoryFn = fn(&Map<String, Value>) -> Result<Value, RegistryError>;

/// Configuration for one registered entity.
#[derive(Debug, Clone)]
pub struct EntityConfig {
    /// Remote table name (also used as local mirror table name unless overridden).
    pub table_name: &'static str,
    /// Stable Rust-side type name. Required for synced entities.
    pub entity_type_name: Option<&'static str>,
    /// Column definitions in declaration order.
    pub columns: Vec<ColumnDef>,
    /// Optional parent relation for hierarchy bumping.
    pub parent_relation: Option<ParentRelation>,
    /// Column on the remote row that holds the active user's id (default `user_id`).
    pub user_id_column: Option<&'static str>,
    /// True when push payloads should automatically include `user_id`.
    pub include_user_id: bool,
    /// Optional factory for converting a remote row map into a typed JSON entity.
    pub factory: Option<FactoryFn>,
    /// Synced or local-only.
    pub scope: TableScope,
    /// PostgREST or Edge Function for mutations.
    pub write_path: WritePath,
    /// Comma-separated column list to use as the `on_conflict=` target for PostgREST upserts.
    ///
    /// Defaults to `"id"` because every chat-style entity (Conversation, Message, Persona,
    /// etc.) has a single-column `id` primary key. Entities with a composite primary key —
    /// notably the KVS table whose PK is `(user_id, namespace, key)` — must override this so
    /// the upsert lines up with an actual unique / exclusion constraint. Otherwise PostgREST
    /// returns Postgres error `42P10` ("there is no unique or exclusion constraint matching
    /// the ON CONFLICT specification").
    ///
    /// Ignored for entities routed through [`WritePath::Edge`] (Edge Functions own their own
    /// conflict logic).
    pub conflict_target: &'static str,
}

impl EntityConfig {
    /// Builder entry for a synced entity (default write-path: PostgREST).
    pub fn synced(table: &'static str, entity_type: &'static str) -> Self {
        Self {
            table_name: table,
            entity_type_name: Some(entity_type),
            columns: Vec::new(),
            parent_relation: None,
            user_id_column: Some("user_id"),
            include_user_id: true,
            factory: None,
            scope: TableScope::Synced,
            write_path: WritePath::PostgREST,
            conflict_target: "id",
        }
    }

    /// Builder entry for a local-only entity.
    pub fn local_only(table: &'static str) -> Self {
        Self {
            table_name: table,
            entity_type_name: None,
            columns: Vec::new(),
            parent_relation: None,
            user_id_column: None,
            include_user_id: false,
            factory: None,
            scope: TableScope::LocalOnly,
            write_path: WritePath::PostgREST, // unused for local-only
            conflict_target: "id",
        }
    }

    /// Append a single column definition.
    pub fn column(mut self, column: ColumnDef) -> Self {
        self.columns.push(column);
        self
    }

    /// Append multiple column definitions.
    pub fn columns(mut self, columns: impl IntoIterator<Item = ColumnDef>) -> Self {
        self.columns.extend(columns);
        self
    }

    /// Attach a parent relation (used for hierarchy bumping).
    pub fn parent(mut self, relation: ParentRelation) -> Self {
        self.parent_relation = Some(relation);
        self
    }

    /// Attach a factory for converting remote rows into typed JSON entities.
    pub fn factory(mut self, factory: FactoryFn) -> Self {
        self.factory = Some(factory);
        self
    }

    /// Override the default `user_id` column name for this entity.
    pub fn user_id_column(mut self, column: &'static str) -> Self {
        self.user_id_column = Some(column);
        self
    }

    /// Toggle whether push payloads automatically include the active user id.
    pub fn include_user_id(mut self, include: bool) -> Self {
        self.include_user_id = include;
        self
    }

    /// Route writes for this entity through an Edge Function instead of direct PostgREST.
    pub fn write_via_edge(mut self, function: &'static str, default_op: &'static str) -> Self {
        self.write_path = WritePath::Edge { function, default_op };
        self
    }

    /// Override the comma-separated `on_conflict=` columns sent on PostgREST upserts. Required
    /// for entities whose primary key is composite (e.g. KVS uses `"id,user_id"`). Defaults to
    /// `"id"` for chat-style entities with a single-column primary key.
    pub fn conflict_target(mut self, target: &'static str) -> Self {
        self.conflict_target = target;
        self
    }

    // -- Convenience accessors -----------------------------------------------------------------

    /// Canonical names of every registered column.
    pub fn column_names(&self) -> Vec<&'static str> {
        self.columns.iter().map(|c| c.canonical_name).collect()
    }

    /// Local mirror names of every registered column.
    pub fn local_column_names(&self) -> Vec<&'static str> {
        self.columns.iter().map(|c| c.local_name).collect()
    }

    /// Remote column names of every synced column, plus `user_id` if `include_user_id` is set.
    pub fn remote_column_names(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> =
            self.columns.iter().filter_map(|c| c.remote_name).collect();
        if self.include_user_id
            && let Some(user_id_column) = self.user_id_column
        {
            names.push(user_id_column);
        }
        names
    }

    /// Canonical names of every encrypted column.
    pub fn encrypted_columns(&self) -> Vec<&'static str> {
        self.columns.iter().filter(|c| c.encrypted).map(|c| c.canonical_name).collect()
    }

    /// True when this entity participates in remote sync.
    pub fn is_synced(&self) -> bool {
        self.scope == TableScope::Synced
    }

    /// Translate a record keyed by remote column names into a record keyed by local column
    /// names, dropping fields that are not registered. Used after pulling a row from
    /// PostgREST or receiving one over realtime so the result can be handed straight to a
    /// [`crate::persistence::LocalStore::upsert_record`] call.
    pub fn map_remote_to_local(&self, remote: &Map<String, Value>) -> Map<String, Value> {
        let mut local = Map::with_capacity(remote.len());
        for column in &self.columns {
            let Some(remote_name) = column.remote_name else { continue };
            if let Some(value) = remote.get(remote_name) {
                local.insert(column.local_name.into(), value.clone());
            }
        }
        local
    }

    /// Translate a record keyed by local column names into a record keyed by remote column
    /// names. Local-only columns are dropped. Used when promoting a row from the local
    /// mirror into a push payload (e.g. queued tombstone replays where only local columns
    /// are available).
    pub fn map_local_to_remote(&self, local: &Map<String, Value>) -> Map<String, Value> {
        let mut remote = Map::with_capacity(local.len());
        for column in &self.columns {
            let Some(remote_name) = column.remote_name else { continue };
            if let Some(value) = local.get(column.local_name) {
                remote.insert(remote_name.into(), value.clone());
            }
        }
        remote
    }

    /// True when this entity has a factory registered (i.e. callers can use
    /// [`Self::create_entity`] to deserialize remote-shaped rows).
    pub fn has_factory(&self) -> bool {
        self.factory.is_some()
    }

    /// Run the registered factory against a remote-shaped row, producing a typed JSON
    /// entity. Returns [`RegistryError::FactoryFailed`] when no factory is registered, or
    /// when the factory returns an error.
    pub fn create_entity(&self, record: &Map<String, Value>) -> Result<Value, RegistryError> {
        let factory = self.factory.ok_or_else(|| RegistryError::FactoryFailed {
            table: self.table_name.into(),
            message: "no factory registered".into(),
        })?;
        factory(record)
    }

    /// Prepare a remote-shaped payload for transport: encrypt every encrypted column (when
    /// an [`Encryption`] is provided) AND inject the active user id when
    /// [`Self::include_user_id`] is set. Returns the augmented payload as a fresh map.
    ///
    /// This is the single canonical way to prepare a remote-shaped payload for PostgREST
    /// upserts, Edge Function bodies, or queued replays. The [`crate::sync::Coordinator`]
    /// uses this internally; consumers running their own push path should use
    /// [`crate::registry::Registry::serialize_remote_payload`] (which resolves the
    /// [`EntityConfig`] for them by table name).
    ///
    /// Behavior:
    /// - Columns that are not flagged `encrypted` pass through unchanged.
    /// - When `encryption` is `None`, encrypted columns ALSO pass through unchanged. This
    ///   is the right behavior for unit tests, lightweight apps without an encryption
    ///   secret, or public-data buckets.
    /// - When `encryption` is `Some`, encrypted-flagged string values are encrypted in
    ///   place. Values already carrying the canonical `enc:` prefix are left as-is
    ///   (idempotent over already-encrypted payloads). Non-string values are skipped.
    /// - When [`Self::include_user_id`] is true and [`Self::user_id_column`] is set, the
    ///   user id is inserted under that column name (overwriting any prior value to
    ///   prevent a caller from accidentally pushing on behalf of a different user).
    pub fn serialize_for_remote(
        &self,
        encryption: Option<&Encryption>,
        user_id: &str,
        payload: &Map<String, Value>,
    ) -> Result<Map<String, Value>, RegistryError> {
        let mut serialized = payload.clone();

        if let Some(enc) = encryption {
            let user_uuid = Encryption::key_user_uuid(user_id);
            for column in &self.columns {
                if !column.encrypted {
                    continue;
                }
                let key = column.effective_remote_name();
                let Some(Value::String(current_value)) = serialized.get(key).cloned() else {
                    continue;
                };
                if enc.is_encrypted(&current_value) {
                    continue;
                }
                let encrypted = enc.encrypt(&current_value, user_uuid).map_err(|_| {
                    RegistryError::EncryptionFailed {
                        table: self.table_name.into(),
                        column: column.canonical_name.into(),
                    }
                })?;
                serialized.insert(key.into(), Value::String(encrypted));
            }
        }

        if self.include_user_id
            && let Some(user_id_column) = self.user_id_column
        {
            serialized.insert(user_id_column.into(), Value::String(user_id.to_string()));
        }

        Ok(serialized)
    }
}

/// Registry of all entities for an app. Built once at startup, then read-only.
#[derive(Debug, Default)]
pub struct Registry {
    inner: RwLock<RegistryInner>,
}

#[derive(Debug, Default)]
struct RegistryInner {
    by_type: HashMap<String, EntityConfig>,
    by_table: HashMap<String, EntityConfig>,
    table_to_type: HashMap<String, String>,
}

impl Registry {
    /// Build an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register one entity. Synced entities require an `entity_type_name`.
    pub fn register(&self, config: EntityConfig) {
        let mut inner = self.inner.write();
        let table = config.table_name.to_string();
        if config.is_synced() {
            let entity_type = config
                .entity_type_name
                .expect("synced entities require entity_type_name")
                .to_string();
            inner.table_to_type.insert(table.clone(), entity_type.clone());
            inner.by_type.insert(entity_type, config.clone());
        }
        inner.by_table.insert(table, config);
    }

    /// Look up the config for an entity by its Rust type name.
    pub fn config_for_type(&self, entity_type_name: &str) -> Option<EntityConfig> {
        self.inner.read().by_type.get(entity_type_name).cloned()
    }

    /// Look up the config for an entity by its table name.
    pub fn config_for_table(&self, table_name: &str) -> Option<EntityConfig> {
        self.inner.read().by_table.get(table_name).cloned()
    }

    /// True if a synced entity with this type name has been registered.
    pub fn is_registered_type(&self, entity_type_name: &str) -> bool {
        self.inner.read().by_type.contains_key(entity_type_name)
    }

    /// True if a synced entity for this table has been registered.
    pub fn is_registered_table(&self, table_name: &str) -> bool {
        self.inner.read().table_to_type.contains_key(table_name)
    }

    /// Names of every registered synced table.
    pub fn registered_tables(&self) -> Vec<String> {
        self.inner.read().table_to_type.keys().cloned().collect()
    }

    /// Names of every registered synced entity type.
    pub fn registered_types(&self) -> Vec<String> {
        self.inner.read().by_type.keys().cloned().collect()
    }

    /// Encrypt encrypted-flagged columns (when an [`Encryption`] is provided) and inject
    /// the active user id on a remote-shaped payload. Returns
    /// [`RegistryError::TableNotRegistered`] when the table is unknown. Delegates to
    /// [`EntityConfig::serialize_for_remote`] under the hood.
    pub fn serialize_remote_payload(
        &self,
        table_name: &str,
        payload: &Map<String, Value>,
        encryption: Option<&Encryption>,
        user_id: &str,
    ) -> Result<Map<String, Value>, RegistryError> {
        let config = self
            .config_for_table(table_name)
            .ok_or_else(|| RegistryError::TableNotRegistered(table_name.to_string()))?;
        config.serialize_for_remote(encryption, user_id, payload)
    }

    /// Compare a [`SchemaSnapshot`] of the local SQLite mirror against registry expectations.
    /// Returns a list of human-readable issue strings; empty means no drift.
    ///
    /// Validates EVERY registered table (synced AND local-only) against the snapshot using
    /// each column's `local_name` + `sql_type`.
    pub fn validate_local_schema_snapshot(&self, snapshot: &SchemaSnapshot) -> Vec<String> {
        self.validate_schema(snapshot, SchemaLocation::Local)
    }

    /// Compare a [`SchemaSnapshot`] of the remote schema (e.g. parsed from PostgREST's
    /// OpenAPI) against registry expectations. Returns a list of human-readable issue
    /// strings; empty means no drift.
    ///
    /// Validates only synced tables, using each column's `remote_name` + `remote_type`,
    /// plus an additional `user_id` column when [`EntityConfig::include_user_id`] is set.
    pub fn validate_remote_schema_snapshot(&self, snapshot: &SchemaSnapshot) -> Vec<String> {
        self.validate_schema(snapshot, SchemaLocation::Remote)
    }

    /// Derive a [`SchemaSnapshot`] of the EXPECTED remote shape from the registry's own
    /// column definitions. Useful for self-consistency checks, golden-file tests, or
    /// validating one app's expectations against another's snapshot.
    pub fn build_remote_snapshot_from_registry(&self) -> SchemaSnapshot {
        let inner = self.inner.read();
        let tables = inner
            .by_table
            .values()
            .filter(|config| config.is_synced())
            .map(|config| {
                let mut columns = config
                    .columns
                    .iter()
                    .filter_map(|column| {
                        Some(ColumnShape::new(
                            column.remote_name?,
                            column.remote_type.unwrap_or("unknown"),
                        ))
                    })
                    .collect::<Vec<_>>();
                if config.include_user_id
                    && let Some(user_id_column) = config.user_id_column
                {
                    columns.push(ColumnShape::new(user_id_column, "string"));
                }
                (config.table_name.to_string(), TableShape::new(columns))
            })
            .collect();
        SchemaSnapshot::new(tables)
    }

    fn validate_schema(&self, snapshot: &SchemaSnapshot, location: SchemaLocation) -> Vec<String> {
        let inner = self.inner.read();
        let mut issues = Vec::new();

        let configs: Vec<&EntityConfig> = inner
            .by_table
            .values()
            .filter(|config| match location {
                SchemaLocation::Local => true,
                SchemaLocation::Remote => config.is_synced(),
            })
            .collect();

        for config in configs {
            let Some(actual_table) = snapshot.tables.get(config.table_name) else {
                issues.push(format!("missing table: {}", config.table_name));
                continue;
            };

            let actual_columns = actual_table.column_map();
            let expected_columns: Vec<(&str, &str)> = match location {
                SchemaLocation::Local => config
                    .columns
                    .iter()
                    .map(|column| (column.local_name, column.sql_type))
                    .collect(),
                SchemaLocation::Remote => {
                    let mut columns: Vec<(&str, &str)> = config
                        .columns
                        .iter()
                        .filter_map(|column| {
                            Some((column.remote_name?, column.remote_type.unwrap_or("unknown")))
                        })
                        .collect();
                    if config.include_user_id
                        && let Some(user_id_column) = config.user_id_column
                    {
                        columns.push((user_id_column, "string"));
                    }
                    columns
                }
            };

            for (column_name, expected_type) in expected_columns {
                let Some(actual_type) = actual_columns.get(column_name) else {
                    issues.push(format!(
                        "table '{}' missing column: {}",
                        config.table_name, column_name
                    ));
                    continue;
                };

                if normalize_type(expected_type) != normalize_type(actual_type) {
                    issues.push(format!(
                        "table '{}' column '{}' type mismatch: expected {}, found {}",
                        config.table_name, column_name, expected_type, actual_type
                    ));
                }
            }
        }

        issues
    }
}

#[derive(Debug, Clone, Copy)]
enum SchemaLocation {
    Local,
    Remote,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn messages_config() -> EntityConfig {
        EntityConfig::synced("messages", "Message")
            .columns([
                ColumnDef::synced("id", "id", "id", "TEXT", "string", false),
                ColumnDef::synced("content", "content", "content", "TEXT", "string", true),
                ColumnDef::synced("version", "version", "version", "INTEGER", "integer", false),
                ColumnDef::synced("deleted", "deleted", "deleted", "INTEGER", "boolean", false),
            ])
            .parent(ParentRelation::new("conversations", "conversation_id"))
            .write_via_edge("messages-write", "send")
    }

    #[test]
    fn register_and_lookup_by_type_and_table() {
        let reg = Registry::new();
        reg.register(messages_config());
        let by_type = reg.config_for_type("Message").unwrap();
        let by_table = reg.config_for_table("messages").unwrap();
        assert_eq!(by_type.table_name, by_table.table_name);
        assert!(matches!(by_type.write_path, WritePath::Edge { function: "messages-write", .. }));
    }

    #[test]
    fn encrypted_columns_filtered() {
        let cfg = messages_config();
        assert_eq!(cfg.encrypted_columns(), vec!["content"]);
    }

    #[test]
    fn synced_default_conflict_target_is_id() {
        let cfg = EntityConfig::synced("conversations", "Conversation");
        assert_eq!(cfg.conflict_target, "id");
    }

    #[test]
    fn conflict_target_builder_overrides_default() {
        let cfg = EntityConfig::synced("kvs", "Kvs").conflict_target("id,user_id");
        assert_eq!(cfg.conflict_target, "id,user_id");
    }

    #[test]
    fn remote_columns_include_user_id_when_requested() {
        let cfg = messages_config();
        let names = cfg.remote_column_names();
        assert!(names.contains(&"user_id"));
    }

    #[test]
    fn local_only_entities_register_but_dont_appear_in_synced_lookups() {
        let reg = Registry::new();
        reg.register(
            EntityConfig::local_only("sync_metadata")
                .column(ColumnDef::local_only("table_name", "TEXT")),
        );
        assert!(reg.config_for_table("sync_metadata").is_some());
        assert!(reg.config_for_type("sync_metadata").is_none());
        assert!(!reg.is_registered_table("sync_metadata"));
    }

    #[test]
    fn map_remote_to_local_renames_and_drops_unknown_fields() {
        let cfg = EntityConfig::synced("conversations", "Conversation").columns([
            ColumnDef::synced("id", "id_local", "id", "TEXT", "string", false),
            ColumnDef::synced("name", "name_local", "name", "TEXT", "string", false),
        ]);

        let remote = serde_json::json!({
            "id": "c1",
            "name": "alpha",
            "extra_remote_only": "ignored",
        });
        let local = cfg.map_remote_to_local(remote.as_object().unwrap());
        assert_eq!(local["id_local"], serde_json::json!("c1"));
        assert_eq!(local["name_local"], serde_json::json!("alpha"));
        assert!(local.get("extra_remote_only").is_none());
        assert_eq!(local.len(), 2);
    }

    #[test]
    fn map_local_to_remote_skips_local_only_columns() {
        let cfg = EntityConfig::synced("conversations", "Conversation").columns([
            ColumnDef::synced("id", "id", "id", "TEXT", "string", false),
            ColumnDef::local_only("draft_text", "TEXT"),
        ]);

        let local = serde_json::json!({
            "id": "c1",
            "draft_text": "in-progress",
        });
        let remote = cfg.map_local_to_remote(local.as_object().unwrap());
        assert_eq!(remote["id"], serde_json::json!("c1"));
        assert!(remote.get("draft_text").is_none());
        assert_eq!(remote.len(), 1);
    }

    fn enc() -> Encryption {
        Encryption::new("test-secret").unwrap()
    }

    #[test]
    fn serialize_for_remote_injects_user_id_when_configured() {
        let cfg = messages_config();
        let payload = serde_json::json!({ "id": "m1", "version": 1 });
        let serialized =
            cfg.serialize_for_remote(None, "user-42", payload.as_object().unwrap()).unwrap();
        assert_eq!(serialized["user_id"], serde_json::json!("user-42"));
    }

    #[test]
    fn serialize_for_remote_overwrites_caller_supplied_user_id() {
        let cfg = messages_config();
        let payload = serde_json::json!({ "id": "m1", "user_id": "spoofed" });
        let serialized =
            cfg.serialize_for_remote(None, "real-user", payload.as_object().unwrap()).unwrap();
        assert_eq!(
            serialized["user_id"],
            serde_json::json!("real-user"),
            "active session id must win over caller-supplied value"
        );
    }

    #[test]
    fn serialize_for_remote_skips_user_id_when_not_configured() {
        let cfg = EntityConfig::synced("public", "Public")
            .columns([ColumnDef::synced("id", "id", "id", "TEXT", "string", false)])
            .include_user_id(false);
        let payload = serde_json::json!({ "id": "p1" });
        let serialized =
            cfg.serialize_for_remote(None, "user-1", payload.as_object().unwrap()).unwrap();
        assert!(!serialized.contains_key("user_id"));
    }

    #[test]
    fn serialize_for_remote_encrypts_only_flagged_columns() {
        let cfg = messages_config();
        let payload = serde_json::json!({
            "id": "m1",
            "content": "hello world",
            "version": 7,
        });
        let serialized =
            cfg.serialize_for_remote(Some(&enc()), "user-1", payload.as_object().unwrap()).unwrap();
        assert!(serialized["content"].as_str().unwrap().starts_with("enc:"));
        assert_eq!(serialized["id"], serde_json::json!("m1"));
        assert_eq!(serialized["version"], serde_json::json!(7));
    }

    #[test]
    fn serialize_for_remote_is_idempotent_over_already_encrypted() {
        let cfg = messages_config();
        let engine = enc();
        let user = "user-1";
        let pre = engine.encrypt("payload", Encryption::key_user_uuid(user)).unwrap();
        let payload = serde_json::json!({ "id": "m1", "content": pre.clone() });
        let serialized =
            cfg.serialize_for_remote(Some(&engine), user, payload.as_object().unwrap()).unwrap();
        assert_eq!(
            serialized["content"],
            serde_json::json!(pre),
            "already-encrypted values are left untouched"
        );
    }

    #[test]
    fn serialize_for_remote_is_pass_through_without_encryption() {
        let cfg = messages_config();
        let payload = serde_json::json!({ "id": "m1", "content": "plain", "version": 1 });
        let serialized =
            cfg.serialize_for_remote(None, "user-1", payload.as_object().unwrap()).unwrap();
        assert_eq!(serialized["content"], serde_json::json!("plain"));
    }

    #[test]
    fn create_entity_returns_factory_failed_when_no_factory() {
        let cfg = EntityConfig::synced("messages", "Message");
        let payload = serde_json::Map::new();
        let err = cfg.create_entity(&payload).expect_err("no factory");
        match err {
            RegistryError::FactoryFailed { table, .. } => assert_eq!(table, "messages"),
            other => panic!("expected FactoryFailed, got {other:?}"),
        }
    }

    #[test]
    fn create_entity_invokes_registered_factory() {
        let cfg = EntityConfig::synced("messages", "Message")
            .factory(|record| Ok(serde_json::Value::Object(record.clone())));
        let mut payload = serde_json::Map::new();
        payload.insert("id".into(), serde_json::Value::String("m1".into()));
        let value = cfg.create_entity(&payload).unwrap();
        assert_eq!(value["id"], serde_json::json!("m1"));
    }

    #[test]
    fn registry_serialize_remote_payload_resolves_table() {
        let reg = Registry::new();
        reg.register(messages_config());
        let payload = serde_json::json!({ "id": "m1", "content": "hi" });
        let serialized = reg
            .serialize_remote_payload(
                "messages",
                payload.as_object().unwrap(),
                Some(&enc()),
                "user-1",
            )
            .unwrap();
        assert!(serialized["content"].as_str().unwrap().starts_with("enc:"));
        assert_eq!(serialized["user_id"], serde_json::json!("user-1"));
    }

    #[test]
    fn registry_serialize_remote_payload_rejects_unknown_table() {
        let reg = Registry::new();
        let err = reg
            .serialize_remote_payload("ghosts", &serde_json::Map::new(), Some(&enc()), "user-1")
            .expect_err("unknown table");
        assert!(matches!(err, RegistryError::TableNotRegistered(t) if t == "ghosts"));
    }

    #[test]
    fn build_remote_snapshot_from_registry_skips_local_only_tables() {
        let reg = Registry::new();
        reg.register(messages_config());
        reg.register(
            EntityConfig::local_only("drafts").columns([ColumnDef::local_only("id", "TEXT")]),
        );
        let snap = reg.build_remote_snapshot_from_registry();
        assert!(snap.tables.contains_key("messages"));
        assert!(!snap.tables.contains_key("drafts"));
        assert!(snap.tables["messages"].column_map().contains_key("user_id"));
    }

    #[test]
    fn validate_remote_schema_snapshot_reports_missing_table_and_column_drift() {
        let reg = Registry::new();
        reg.register(messages_config());

        let mut tables = HashMap::new();
        tables.insert(
            "messages".to_string(),
            TableShape::new(vec![
                ColumnShape::new("id", "string"),
                ColumnShape::new("content", "integer"), // wrong type
                ColumnShape::new("version", "integer"),
                ColumnShape::new("deleted", "boolean"),
                ColumnShape::new("user_id", "string"),
            ]),
        );
        let snap = SchemaSnapshot::new(tables);
        let issues = reg.validate_remote_schema_snapshot(&snap);
        assert!(
            issues.iter().any(|i| i.contains("content") && i.contains("type mismatch")),
            "expected type-mismatch issue for content; got {issues:?}"
        );
    }

    #[test]
    fn validate_remote_schema_snapshot_passes_self_consistent_snapshot() {
        let reg = Registry::new();
        reg.register(messages_config());
        let snap = reg.build_remote_snapshot_from_registry();
        let issues = reg.validate_remote_schema_snapshot(&snap);
        assert!(issues.is_empty(), "self-consistent snapshot should validate cleanly: {issues:?}");
    }

    #[test]
    fn validate_local_schema_snapshot_includes_local_only_tables() {
        let reg = Registry::new();
        reg.register(messages_config());
        reg.register(
            EntityConfig::local_only("drafts").columns([ColumnDef::local_only("id", "TEXT")]),
        );

        let mut tables = HashMap::new();
        // Provide messages but skip drafts → expect a missing-table issue.
        tables.insert(
            "messages".to_string(),
            TableShape::new(vec![
                ColumnShape::new("id", "TEXT"),
                ColumnShape::new("content", "TEXT"),
                ColumnShape::new("version", "INTEGER"),
                ColumnShape::new("deleted", "INTEGER"),
            ]),
        );
        let snap = SchemaSnapshot::new(tables);
        let issues = reg.validate_local_schema_snapshot(&snap);
        assert!(
            issues.iter().any(|i| i == "missing table: drafts"),
            "expected missing-table issue for drafts; got {issues:?}"
        );
    }
}
