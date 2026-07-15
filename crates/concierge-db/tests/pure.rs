//! Tests for the pure parts: query building, pagination math, row
//! serialization, watermark SQL, and schema DDL sanity.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use concierge_db::gql::{build_query, next_offset, Category, GqlMod, PAGE_SIZE};
use concierge_db::schema::DDL;
use concierge_db::sync::mod_to_row;

fn sample_mod() -> GqlMod {
    serde_json::from_value(serde_json::json!({
        "modId": 42,
        "uid": "7318624808234",
        "name": "Test Mod",
        "summary": "A test",
        "author": "someone",
        "version": "1.0",
        "status": "published",
        "endorsements": 10,
        "downloads": 100,
        "fileSize": 12345,
        "adultContent": false,
        "supportsVortex": true,
        "createdAt": "2020-01-02T03:04:05Z",
        "updatedAt": "2021-06-07T08:09:10Z",
        "modCategory": {"name": "Weapons"}
    }))
    .unwrap()
}

#[test]
fn build_query_full_sweep() {
    let q = build_query("fallout4", 100, 200, None);
    let s = q["query"].as_str().unwrap();
    assert!(s.contains(r#"value: "fallout4""#));
    assert!(s.contains("count: 100"));
    assert!(s.contains("offset: 200"));
    assert!(s.contains("createdAt: {direction: ASC}"));
    assert!(
        !s.contains("updatedAt: ["),
        "no watermark filter on full sweep"
    );
}

#[test]
fn build_query_incremental() {
    // watermark is unix seconds: ISO timestamps break the backend's Lucene layer
    let q = build_query("fallout4", 50, 0, Some("1751400000"));
    let s = q["query"].as_str().unwrap();
    assert!(s.contains(r#"updatedAt: [{value: "1751400000", op: GTE}]"#));
}

#[test]
fn next_offset_math() {
    // advance by nodes actually received (API clamps count: 100 -> 80)
    assert_eq!(next_offset(0, 80, 74776), Some(80));
    assert_eq!(next_offset(80, 80, 74776), Some(160));
    // short page mid-catalog still advances
    assert_eq!(next_offset(160, 3, 74776), Some(163));
    // stop at total_count
    assert_eq!(next_offset(74700, 76, 74776), None);
    assert_eq!(next_offset(74700, 100, 74776), None, "overshoot also stops");
    // empty page must not loop forever
    assert_eq!(next_offset(500, 0, 74776), None);
    let _ = PAGE_SIZE; // request size stays 100; the server clamps as it likes
}

#[test]
fn mod_row_shape() {
    let row = mod_to_row("fallout4", &sample_mod());
    assert_eq!(row.game_domain, "fallout4");
    assert_eq!(row.mod_id, 42);
    assert_eq!(row.category, "Weapons");
    assert_eq!(row.updated_at, "2021-06-07T08:09:10Z");
}

#[test]
fn mod_row_defaults_for_missing_fields() {
    let sparse: GqlMod = serde_json::from_value(serde_json::json!({
        "modId": 7,
        "name": "Sparse",
        "createdAt": "2020-01-01T00:00:00Z",
        "updatedAt": "2020-01-01T00:00:00Z"
    }))
    .unwrap();
    let row = mod_to_row("fallout4", &sparse);
    assert!(row.summary.is_empty());
    assert!(row.category.is_empty());
    assert_eq!(row.endorsements, 0);
    assert!(!row.adult);
}

#[test]
fn category_deserializes_null_name() {
    let c: Category = serde_json::from_value(serde_json::json!({"name": null})).unwrap();
    assert!(c.name.is_none());
}

#[test]
fn ddl_creates_expected_tables() {
    let joined = DDL.join("\n");
    for table in [
        "games",
        "mods",
        "mod_files",
        "sync_watermarks",
        "conflict_findings",
        "resolver_decisions",
    ] {
        assert!(
            joined.contains(&format!("CREATE TABLE IF NOT EXISTS {table}")),
            "missing table {table}"
        );
    }
    for ddl in DDL {
        assert!(ddl.contains("ENGINE ="), "every table needs an engine");
    }
}

#[test]
fn modrinth_numeric_id_stable_and_distinct() {
    use concierge_db::modrinth::numeric_id;
    assert_eq!(numeric_id("AANobbMI"), numeric_id("AANobbMI"));
    assert_ne!(numeric_id("AANobbMI"), numeric_id("P7dR8mSH"));
}

#[test]
fn modrinth_hit_row_shape() {
    use concierge_db::modrinth::{hit_to_row, SearchHit};
    let h: SearchHit = serde_json::from_value(serde_json::json!({
        "project_id": "AANobbMI",
        "slug": "sodium",
        "title": "Sodium",
        "description": "A modern rendering engine",
        "author": "jellysquid3",
        "categories": ["optimization"],
        "downloads": 40_000_000u64,
        "follows": 20000,
        "date_created": "2020-01-01T00:00:00Z",
        "date_modified": "2026-06-01T00:00:00Z"
    }))
    .unwrap();
    let row = hit_to_row("minecraft", &h);
    assert_eq!(row.game_domain, "minecraft");
    assert_eq!(row.name, "Sodium");
    assert_eq!(row.endorsements, 20000);
    assert_eq!(row.downloads, 40_000_000);
}
