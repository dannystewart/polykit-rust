-- PolyBase KVS table — local SQLite mirror.
--
-- Mirrors `crates/polybase/migrations/0001_kvs.sql` (Supabase) for the local-first cache.
-- The `id` column is the synthetic `{namespace}::{key}` primary key used by the polybase
-- coordinator's per-row APIs; `(namespace, key)` is also unique for direct lookup.
--
-- Embedded into `polybase-sqlite` and applied automatically when a per-user database is opened.

create table if not exists kvs (
    id          text primary key,
    namespace   text not null,
    key         text not null,
    value       text not null,                   -- serialized JSON (sqlite has no native jsonb)

    version     integer not null default 1,
    deleted     integer not null default 0,
    updated_at  text not null default (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),

    unique (namespace, key)
);

create index if not exists kvs_namespace_idx on kvs (namespace);
create index if not exists kvs_updated_at_idx on kvs (updated_at desc);
