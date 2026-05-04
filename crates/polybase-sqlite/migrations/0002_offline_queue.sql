-- PolyBase offline queue — local SQLite mirror.
--
-- Persistent backing store for `polybase::offline_queue::OfflineQueue`. Each row is one
-- pending mutation waiting to be replayed against Supabase (Edge Function or PostgREST).
--
-- The composite primary key on `(table_name, entity_id)` enforces dedupe at the schema
-- level: a fresh `enqueue` for the same key is a single `INSERT OR REPLACE` that overwrites
-- the prior pending op. The contract dedupe semantics live in `polybase::contract` and the
-- impl in `polybase-sqlite::SqliteOfflineQueue` matches them.
--
-- `kind` carries a JSON-serialized `polybase::offline_queue::QueuedOperationKind` (tagged
-- enum: `write` / `tombstone` / `hard_delete`) so the same column shape covers Edge Function
-- payloads, PostgREST upsert rows, and tombstone updates.
--
-- Embedded into `polybase-sqlite` and applied automatically when a per-user database is
-- opened via `DbManager::with_polybase_migrator`.

create table if not exists polybase_queue (
    table_name        text    not null,
    entity_id         text    not null,
    kind              text    not null,
    queued_at_micros  integer not null,
    retry_count       integer not null default 0,
    last_error        text,

    primary key (table_name, entity_id)
) without rowid;

create index if not exists polybase_queue_queued_at_idx
    on polybase_queue (queued_at_micros);
