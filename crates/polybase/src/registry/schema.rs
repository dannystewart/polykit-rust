//! Schema introspection: snapshots of an actual local SQLite database or a remote
//! PostgREST OpenAPI document, plus drift validation against the registered
//! [`super::EntityConfig`] expectations.
//!
//! The shapes here are deliberately minimal — name + data-type pairs — because they are
//! produced by two very different sources (`PRAGMA table_info` rows on the local side; the
//! `components.schemas` block of `/rest/v1/?` on the remote side) and have to compare cleanly
//! against one another via [`super::Registry::validate_local_schema_snapshot`] /
//! [`super::Registry::validate_remote_schema_snapshot`].

use std::collections::HashMap;

use serde_json::Value;

use crate::errors::RegistryError;

/// Single column entry in a [`TableShape`]. Both fields are stored as owned strings because
/// snapshots come from runtime introspection (SQLite pragma rows, OpenAPI JSON), where data
/// type names are not `&'static`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnShape {
    /// Column name as reported by the source (SQLite or PostgREST).
    pub name: String,
    /// Data type as reported by the source. Compared with [`normalize_type`] so superficial
    /// differences (`int8` vs `integer`, `varchar` vs `text`) don't trigger false drift.
    pub data_type: String,
}

impl ColumnShape {
    /// Convenience constructor accepting any string-like input.
    pub fn new(name: impl Into<String>, data_type: impl Into<String>) -> Self {
        Self { name: name.into(), data_type: data_type.into() }
    }
}

/// Set of [`ColumnShape`]s for one table.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TableShape {
    /// Columns in source-reported order. Order is informational; validation is set-based.
    pub columns: Vec<ColumnShape>,
}

impl TableShape {
    /// Build a table shape from a column list.
    pub fn new(columns: Vec<ColumnShape>) -> Self {
        Self { columns }
    }

    /// Indexed view by column name — used internally by validation to compare snapshot
    /// columns against registry expectations. Public so consumers can run their own checks
    /// without re-walking [`Self::columns`].
    pub fn column_map(&self) -> HashMap<&str, &str> {
        self.columns
            .iter()
            .map(|column| (column.name.as_str(), column.data_type.as_str()))
            .collect()
    }
}

/// Snapshot of every table in a database (local SQLite mirror) or remote schema
/// (PostgREST OpenAPI document). The keys in [`Self::tables`] are table names exactly as
/// reported by the source.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SchemaSnapshot {
    /// Per-table column shapes. Construct with [`Self::new`] from a pre-built map, or
    /// [`Self::from_openapi`] when fed a Supabase / PostgREST root document.
    pub tables: HashMap<String, TableShape>,
}

impl SchemaSnapshot {
    /// Build a snapshot from a pre-assembled table map. Typically used by callers that just
    /// finished walking the local database (e.g. via SQLite's `PRAGMA table_info`).
    pub fn new(tables: HashMap<String, TableShape>) -> Self {
        Self { tables }
    }

    /// Parse a Supabase / PostgREST OpenAPI root document into a [`SchemaSnapshot`].
    ///
    /// Looks for the `components.schemas.*` block and treats each entry as a table. Each
    /// table's `properties.*` becomes a column; the column's `type` (or `format` as
    /// fallback) becomes the data type. Tables without a `properties` block are skipped.
    ///
    /// Returns [`RegistryError::InvalidRemoteSchema`] when the document doesn't contain a
    /// `components.schemas` object.
    pub fn from_openapi(value: &Value) -> Result<Self, RegistryError> {
        let schemas = value
            .get("components")
            .and_then(|value| value.get("schemas"))
            .and_then(Value::as_object)
            .ok_or_else(|| {
                RegistryError::InvalidRemoteSchema("missing components.schemas".to_string())
            })?;

        let mut tables = HashMap::new();
        for (table_name, schema_value) in schemas {
            let Some(properties) = schema_value.get("properties").and_then(Value::as_object) else {
                continue;
            };

            let mut columns = Vec::new();
            for (column_name, property) in properties {
                let data_type = property
                    .get("type")
                    .and_then(Value::as_str)
                    .or_else(|| property.get("format").and_then(Value::as_str))
                    .unwrap_or("unknown");
                columns.push(ColumnShape::new(column_name.clone(), data_type));
            }
            tables.insert(table_name.clone(), TableShape::new(columns));
        }

        Ok(Self { tables })
    }
}

/// Normalize column data-type spellings so comparison is symmetric across SQLite, PostgREST,
/// and the registry's own `&'static str` type identifiers.
///
/// Examples:
/// - `INT`, `Integer`, `int8`, `BIGINT` all collapse to `"integer"`
/// - `REAL`, `float`, `double`, `double precision` all collapse to `"real"`
/// - `bool`, `BOOLEAN` collapse to `"boolean"`
/// - `TEXT`, `varchar`, `character varying`, `string` collapse to `"string"`
/// - Anything else passes through lowercase, trimmed.
pub fn normalize_type(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "int" | "integer" | "int8" | "bigint" => "integer".to_string(),
        "real" | "float" | "double" | "double precision" => "real".to_string(),
        "bool" | "boolean" => "boolean".to_string(),
        "text" | "varchar" | "character varying" | "string" => "string".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn from_openapi_parses_well_formed_document() {
        let doc = json!({
            "components": {
                "schemas": {
                    "personas": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "name": { "type": "string" },
                            "version": { "type": "integer" }
                        }
                    },
                    "messages": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "content": { "type": "string" }
                        }
                    }
                }
            }
        });

        let snapshot = SchemaSnapshot::from_openapi(&doc).expect("valid doc");
        assert_eq!(snapshot.tables.len(), 2);

        let personas = &snapshot.tables["personas"];
        let columns = personas.column_map();
        assert_eq!(columns.get("id"), Some(&"string"));
        assert_eq!(columns.get("name"), Some(&"string"));
        assert_eq!(columns.get("version"), Some(&"integer"));
    }

    #[test]
    fn from_openapi_uses_format_when_type_missing() {
        // PostgREST sometimes reports nullable / nested column types via `format`.
        let doc = json!({
            "components": {
                "schemas": {
                    "things": {
                        "properties": {
                            "id": { "format": "uuid" }
                        }
                    }
                }
            }
        });
        let snapshot = SchemaSnapshot::from_openapi(&doc).expect("valid doc");
        let columns = snapshot.tables["things"].column_map();
        assert_eq!(columns.get("id"), Some(&"uuid"));
    }

    #[test]
    fn from_openapi_falls_back_to_unknown_when_neither_type_nor_format() {
        let doc = json!({
            "components": { "schemas": { "t": { "properties": { "c": {} } } } }
        });
        let snapshot = SchemaSnapshot::from_openapi(&doc).expect("valid doc");
        let columns = snapshot.tables["t"].column_map();
        assert_eq!(columns.get("c"), Some(&"unknown"));
    }

    #[test]
    fn from_openapi_skips_schemas_without_properties() {
        let doc = json!({
            "components": {
                "schemas": {
                    "personas": { "properties": { "id": { "type": "string" } } },
                    "rpc.do_something": { "type": "object" }
                }
            }
        });
        let snapshot = SchemaSnapshot::from_openapi(&doc).expect("valid doc");
        assert!(snapshot.tables.contains_key("personas"));
        assert!(!snapshot.tables.contains_key("rpc.do_something"));
    }

    #[test]
    fn from_openapi_rejects_documents_without_components_schemas() {
        let doc = json!({ "openapi": "3.0.0", "info": { "title": "missing schemas" } });
        let err = SchemaSnapshot::from_openapi(&doc).expect_err("missing block");
        assert!(matches!(err, RegistryError::InvalidRemoteSchema(_)));
    }

    #[test]
    fn normalize_type_collapses_common_aliases() {
        assert_eq!(normalize_type("INTEGER"), "integer");
        assert_eq!(normalize_type("bigint"), "integer");
        assert_eq!(normalize_type("int8"), "integer");
        assert_eq!(normalize_type(" Real "), "real");
        assert_eq!(normalize_type("DOUBLE PRECISION"), "real");
        assert_eq!(normalize_type("Bool"), "boolean");
        assert_eq!(normalize_type("varchar"), "string");
        assert_eq!(normalize_type("TEXT"), "string");
        assert_eq!(normalize_type("custom_type"), "custom_type");
    }

    #[test]
    fn column_map_indexes_by_name() {
        let shape = TableShape::new(vec![
            ColumnShape::new("id", "string"),
            ColumnShape::new("count", "integer"),
        ]);
        let map = shape.column_map();
        assert_eq!(map.get("id"), Some(&"string"));
        assert_eq!(map.get("count"), Some(&"integer"));
        assert_eq!(map.len(), 2);
    }
}
