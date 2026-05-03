-- PolyBase KVS table — Supabase-side schema.
--
-- Drop-in replacement for iCloud KVS / NSUbiquitousKeyValueStore-style preferences sync.
-- Each row is keyed by `(user_id, namespace, key)`; `value` carries the JSON payload and the
-- usual `version` / `deleted` / `updated_at` columns participate in the same conflict
-- resolution that every other PolyBase entity uses.
--
-- Apply with:
--     supabase migration up
-- after copying this file into your Supabase project's `supabase/migrations/` directory.

create extension if not exists "pgcrypto";

create table if not exists public.kvs (
    -- Synthetic primary key used by clients; encoded as `{namespace}::{key}` for the local
    -- mirror's single-column id slot. The composite uniqueness contract still lives on
    -- `(user_id, namespace, key)` below so admin tools can address rows the same way the
    -- client does.
    id           text not null,

    user_id      uuid not null references auth.users(id) on delete cascade,
    namespace    text not null,
    key          text not null,
    value        jsonb not null,

    version      bigint not null default 1,
    deleted      boolean not null default false,
    updated_at   timestamptz not null default now(),

    primary key (user_id, namespace, key),
    unique (id, user_id)
);

create index if not exists kvs_user_namespace_idx
    on public.kvs (user_id, namespace);

create index if not exists kvs_updated_at_idx
    on public.kvs (updated_at desc);

-- ---------------------------------------------------------------------------------------------
-- RLS: each user can read / write only their own rows. The `service_role` lockdown that the
-- chat tables use is intentionally NOT applied here — KVS writes are lightweight enough to
-- run via the user JWT direct against PostgREST (the polybase coordinator dispatches KVS as
-- `WritePath::PostgREST`).
-- ---------------------------------------------------------------------------------------------

alter table public.kvs enable row level security;

drop policy if exists kvs_select_own on public.kvs;
create policy kvs_select_own on public.kvs
    for select using (auth.uid() = user_id);

drop policy if exists kvs_insert_own on public.kvs;
create policy kvs_insert_own on public.kvs
    for insert with check (auth.uid() = user_id);

drop policy if exists kvs_update_own on public.kvs;
create policy kvs_update_own on public.kvs
    for update using (auth.uid() = user_id) with check (auth.uid() = user_id);

drop policy if exists kvs_delete_own on public.kvs;
create policy kvs_delete_own on public.kvs
    for delete using (auth.uid() = user_id);

-- ---------------------------------------------------------------------------------------------
-- Realtime: publish the table so the polybase realtime subscriber can pick up cross-device
-- changes the way Swift PolyBase does for chat data.
-- ---------------------------------------------------------------------------------------------

alter publication supabase_realtime add table public.kvs;
