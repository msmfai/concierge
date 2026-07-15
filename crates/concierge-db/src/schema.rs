//! Schema DDL. `ReplacingMergeTree(synced_at)` gives last-write-wins per
//! (game, mod) so re-syncs and incremental updates are idempotent.

/// Existing stores get new columns via ALTER (idempotent).
pub const MIGRATIONS: &[&str] = &[
    "ALTER TABLE mods ADD COLUMN IF NOT EXISTS provider String DEFAULT 'nexus'",
    "ALTER TABLE sync_watermarks ADD COLUMN IF NOT EXISTS provider String DEFAULT 'nexus'",
];

pub const DDL: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS games (
        domain String,
        name String,
        synced_at DateTime('UTC')
    ) ENGINE = ReplacingMergeTree(synced_at) ORDER BY domain",
    "CREATE TABLE IF NOT EXISTS mods (
        game_domain String,
        provider String DEFAULT 'nexus',
        mod_id UInt64,
        uid String,
        name String,
        summary String,
        author String,
        version String,
        category String,
        status String,
        endorsements UInt64,
        downloads UInt64,
        file_size UInt64,
        adult Bool,
        supports_vortex Bool,
        created_at DateTime('UTC'),
        updated_at DateTime('UTC'),
        synced_at DateTime('UTC')
    ) ENGINE = ReplacingMergeTree(synced_at) ORDER BY (game_domain, mod_id)",
    "CREATE TABLE IF NOT EXISTS mod_files (
        game_domain String,
        mod_id UInt64,
        file_id UInt64,
        file_name String,
        category String,
        version String,
        size_bytes UInt64,
        md5 String,
        synced_at DateTime('UTC')
    ) ENGINE = ReplacingMergeTree(synced_at) ORDER BY (game_domain, mod_id, file_id)",
    "CREATE TABLE IF NOT EXISTS sync_watermarks (
        game_domain String,
        provider String DEFAULT 'nexus',
        synced_at DateTime('UTC'),
        max_updated_at DateTime('UTC'),
        rows_synced UInt64,
        full_sweep Bool
    ) ENGINE = MergeTree ORDER BY (game_domain, synced_at)",
    // Concierge's own product data — populated by the resolver work later.
    "CREATE TABLE IF NOT EXISTS conflict_findings (
        game_domain String,
        plan_hash String,
        record_signature String,
        form_id UInt64,
        winner_plugin String,
        loser_plugin String,
        field_path String,
        severity String,
        found_at DateTime('UTC')
    ) ENGINE = MergeTree ORDER BY (game_domain, plan_hash, form_id)",
    "CREATE TABLE IF NOT EXISTS resolver_decisions (
        game_domain String,
        plan_hash String,
        form_id UInt64,
        decision String,
        rationale String,
        decided_at DateTime('UTC')
    ) ENGINE = MergeTree ORDER BY (game_domain, plan_hash, form_id)",
];
