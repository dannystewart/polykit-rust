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

use crate::errors::RegistryError;

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
}
