//! The mod catalog as an embedded `SQLite` database, compiled into
//! the binary (`rusqlite` `bundled`), so the catalog needs no external program
//! and works identically on Windows, Linux, and macOS — unlike `clickhouse
//! local`, which ships no Windows binary. This is the cross-platform catalog
//! backend; [`crate::ch`] (`ClickHouse`) exists only to migrate legacy stores.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension as _};

use crate::error::Result;

const DDL: &str = "\
CREATE TABLE IF NOT EXISTS mods (
    game_domain TEXT NOT NULL,
    mod_id      INTEGER NOT NULL,
    name        TEXT NOT NULL DEFAULT '',
    summary     TEXT NOT NULL DEFAULT '',
    author      TEXT NOT NULL DEFAULT '',
    version     TEXT NOT NULL DEFAULT '',
    category    TEXT NOT NULL DEFAULT '',
    endorsements INTEGER NOT NULL DEFAULT 0,
    downloads   INTEGER NOT NULL DEFAULT 0,
    file_size   INTEGER NOT NULL DEFAULT 0,
    adult       INTEGER NOT NULL DEFAULT 0,
    updated_at  TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (game_domain, mod_id)
);
CREATE INDEX IF NOT EXISTS mods_domain_endorse ON mods(game_domain, endorsements DESC);
CREATE TABLE IF NOT EXISTS sync_watermarks (
    game_domain    TEXT NOT NULL,
    provider       TEXT NOT NULL DEFAULT 'nexus',
    synced_at      TEXT NOT NULL DEFAULT '',
    max_updated_at TEXT NOT NULL DEFAULT '',
    rows_synced    INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (game_domain, provider)
);";

/// One catalog mod (last-write-wins per `(game_domain, mod_id)`).
#[derive(Debug, Clone, Default)]
pub struct Row {
    pub game_domain: String,
    pub mod_id: u64,
    pub name: String,
    pub summary: String,
    pub author: String,
    pub version: String,
    pub category: String,
    pub endorsements: u64,
    pub downloads: u64,
    pub file_size: u64,
    pub adult: bool,
    pub updated_at: String,
}

/// Browse sort order. Endorsements-descending is the default everywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortBy {
    #[default]
    Endorsements,
    Downloads,
    Updated,
    Name,
}

impl SortBy {
    /// The `ORDER BY` body. Static strings only (no user input) — safe to
    /// interpolate; the name filter and category are still bound params.
    #[must_use]
    pub const fn order_sql(self) -> &'static str {
        match self {
            Self::Endorsements => "endorsements DESC",
            Self::Downloads => "downloads DESC",
            Self::Updated => "updated_at DESC",
            Self::Name => "name COLLATE NOCASE ASC",
        }
    }
    /// Map a [`label`](Self::label) back to its variant (for projected sort ids).
    #[must_use]
    pub fn from_label(s: &str) -> Option<Self> {
        Self::all().into_iter().find(|o| o.label() == s)
    }

    /// Human label for the sort control.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Endorsements => "Most endorsed",
            Self::Downloads => "Most downloaded",
            Self::Updated => "Recently updated",
            Self::Name => "Name (A–Z)",
        }
    }
    /// The controls' option list, default first.
    #[must_use]
    pub const fn all() -> [Self; 4] {
        [
            Self::Endorsements,
            Self::Downloads,
            Self::Updated,
            Self::Name,
        ]
    }
}

/// A search hit.
#[derive(Debug, Clone)]
pub struct Hit {
    pub mod_id: u64,
    pub name: String,
    pub endorsements: u64,
    pub kb: u64,
    pub author: String,
    pub summary: String,
    pub category: String,
    pub downloads: u64,
    pub updated_at: String,
}

/// The embedded catalog. Opening creates the schema if absent.
#[derive(Debug)]
pub struct Catalog {
    conn: Connection,
}

impl Catalog {
    /// Open (or create) the catalog at `path` (a single `.sqlite` file).
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        // WAL so a reader (the browser's catalog search) can read already-synced
        // rows *while* a sync is still writing later pages — without this the
        // default rollback journal locks the DB and browsing is gated on the
        // whole (tens-of-thousands-of-rows) sync finishing. busy_timeout keeps a
        // read that races a page-commit waiting briefly instead of erroring.
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
        conn.execute_batch(DDL)?;
        Ok(Self { conn })
    }

    /// Total mod count (across games).
    pub fn count(&self) -> Result<u64> {
        let n: i64 = self
            .conn
            .query_row("SELECT count(*) FROM mods", [], |r| r.get(0))?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    /// Rows for one game domain — what the browser reports as "N mods synced".
    pub fn count_domain(&self, game_domain: &str) -> Result<u64> {
        let n: i64 = self.conn.query_row(
            "SELECT count(*) FROM mods WHERE game_domain=?1",
            rusqlite::params![game_domain],
            |r| r.get(0),
        )?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    /// When this domain was last synced (Unix seconds, 0 = never) — drives the
    /// browser's "synced N ago / never" affordance.
    pub fn synced_at_epoch(&self, game_domain: &str) -> Result<u64> {
        let e: Option<i64> = self.conn.query_row(
            "SELECT CAST(strftime('%s', max(synced_at)) AS INTEGER) FROM sync_watermarks \
             WHERE game_domain=?1 AND provider='nexus'",
            rusqlite::params![game_domain],
            |r| r.get(0),
        )?;
        Ok(e.map_or(0, |v| u64::try_from(v).unwrap_or(0)))
    }

    /// Current UTC timestamp as an ISO-8601 string (from `SQLite`).
    pub fn now_utc(&self) -> Result<String> {
        Ok(self
            .conn
            .query_row("SELECT strftime('%Y-%m-%dT%H:%M:%SZ','now')", [], |r| {
                r.get(0)
            })?)
    }

    /// The sync watermark for `game_domain` as Unix seconds (0 if none) — used
    /// to fetch only mods updated since the last sync.
    pub fn watermark_epoch(&self, game_domain: &str) -> Result<u64> {
        let e: Option<i64> = self.conn.query_row(
            "SELECT CAST(strftime('%s', max(max_updated_at)) AS INTEGER) FROM sync_watermarks \
             WHERE game_domain=?1 AND provider='nexus' AND rows_synced>0",
            rusqlite::params![game_domain],
            |r| r.get(0),
        )?;
        Ok(e.map_or(0, |v| u64::try_from(v).unwrap_or(0)))
    }

    /// Record a sync watermark (last-write-wins per `(game_domain, provider)`).
    pub fn set_watermark(
        &mut self,
        game_domain: &str,
        provider: &str,
        synced_at: &str,
        max_updated_at: &str,
        rows_synced: u64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO sync_watermarks (game_domain, provider, synced_at, max_updated_at, rows_synced) \
             VALUES (?1,?2,?3,?4,?5) \
             ON CONFLICT(game_domain, provider) DO UPDATE SET synced_at=excluded.synced_at, \
             max_updated_at=excluded.max_updated_at, rows_synced=excluded.rows_synced",
            rusqlite::params![
                game_domain,
                provider,
                synced_at,
                max_updated_at,
                i64::try_from(rows_synced).unwrap_or(0)
            ],
        )?;
        Ok(())
    }

    /// Upsert rows (last-write-wins on the `(game_domain, mod_id)` key).
    pub fn upsert(&mut self, rows: &[Row]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO mods (game_domain, mod_id, name, summary, author, version, \
                     category, endorsements, downloads, file_size, adult, updated_at) \
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12) \
                     ON CONFLICT(game_domain, mod_id) DO UPDATE SET \
                     name=excluded.name, summary=excluded.summary, author=excluded.author, \
                     version=excluded.version, category=excluded.category, \
                     endorsements=excluded.endorsements, downloads=excluded.downloads, \
                     file_size=excluded.file_size, adult=excluded.adult, \
                     updated_at=excluded.updated_at",
            )?;
            for r in rows {
                stmt.execute(rusqlite::params![
                    r.game_domain,
                    i64::try_from(r.mod_id).unwrap_or(0),
                    r.name,
                    r.summary,
                    r.author,
                    r.version,
                    r.category,
                    i64::try_from(r.endorsements).unwrap_or(0),
                    i64::try_from(r.downloads).unwrap_or(0),
                    i64::try_from(r.file_size).unwrap_or(0),
                    i64::from(r.adult),
                    r.updated_at,
                ])?;
            }
        }
        tx.commit()?;
        Ok(rows.len())
    }

    /// Search `game_domain` for `query` (substring, case-insensitive), narrowed
    /// by `extra_where` (SQL fragments, each starting ` AND `; caller-escaped),
    /// ranked by endorsements. Read-only.
    /// Endorsement-descending, name-filtered — the default browse/agent search.
    pub fn search(
        &self,
        game_domain: &str,
        query: &str,
        extra_where: &str,
        limit: u32,
    ) -> Result<Vec<Hit>> {
        self.search_sorted(
            game_domain,
            query,
            extra_where,
            None,
            SortBy::Endorsements,
            limit,
        )
    }

    /// Full browse search: name filter + hard-filter `extra_where` + an exact
    /// `category` (None = all) + a `sort` column (default endorsements desc).
    pub fn search_sorted(
        &self,
        game_domain: &str,
        query: &str,
        extra_where: &str,
        category: Option<&str>,
        sort: SortBy,
        limit: u32,
    ) -> Result<Vec<Hit>> {
        let cat_clause = if category.is_some() {
            " AND category=?4"
        } else {
            ""
        };
        let query_sql = format!(
            "SELECT mod_id, name, endorsements, file_size/1024 AS kb, author, summary, \
             category, downloads, updated_at FROM mods \
             WHERE game_domain=?1 AND lower(name) LIKE ?2{extra_where}{cat_clause} \
             ORDER BY {} LIMIT ?3",
            sort.order_sql()
        );
        let like = format!("%{}%", query.to_lowercase());
        let mut stmt = self.conn.prepare(&query_sql)?;
        let map_row = |r: &rusqlite::Row| {
            Ok(Hit {
                mod_id: u64::try_from(r.get::<_, i64>(0)?).unwrap_or(0),
                name: r.get(1)?,
                endorsements: u64::try_from(r.get::<_, i64>(2)?).unwrap_or(0),
                kb: u64::try_from(r.get::<_, i64>(3)?).unwrap_or(0),
                author: r.get(4)?,
                summary: r.get(5)?,
                category: r.get(6)?,
                downloads: u64::try_from(r.get::<_, i64>(7)?).unwrap_or(0),
                updated_at: r.get(8)?,
            })
        };
        let rows = if let Some(cat) = category {
            stmt.query_map(
                rusqlite::params![game_domain, like, limit.min(50), cat],
                map_row,
            )?
        } else {
            stmt.query_map(rusqlite::params![game_domain, like, limit.min(50)], map_row)?
        };
        let mut out = Vec::new();
        for h in rows {
            out.push(h?);
        }
        Ok(out)
    }

    /// Distinct categories for a domain with their mod counts, most-populous
    /// first — powers the first-class category filter.
    pub fn categories(&self, game_domain: &str) -> Result<Vec<(String, u64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT category, count(*) AS n FROM mods \
             WHERE game_domain=?1 AND category<>'' \
             GROUP BY category ORDER BY n DESC",
        )?;
        let rows = stmt.query_map(rusqlite::params![game_domain], |r| {
            Ok((
                r.get::<_, String>(0)?,
                u64::try_from(r.get::<_, i64>(1)?).unwrap_or(0),
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Names for a set of mod ids in one query.
    pub fn names(&self, game_domain: &str, ids: &[u64]) -> Result<Vec<(u64, String)>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let list = ids.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
        let names_sql = format!(
            "SELECT mod_id, name FROM mods WHERE game_domain=?1 AND mod_id IN ({list}) \
             ORDER BY endorsements DESC"
        );
        let mut stmt = self.conn.prepare(&names_sql)?;
        let rows = stmt.query_map(rusqlite::params![game_domain], |r| {
            Ok((
                u64::try_from(r.get::<_, i64>(0)?).unwrap_or(0),
                r.get::<_, String>(1)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// The `version` column for one mod — which for a Modrinth-synced catalog
    /// holds the project **slug**, the key needed to resolve a free download.
    /// `None` if the mod isn't in the catalog.
    pub fn version_of(&self, game_domain: &str, mod_id: u64) -> Result<Option<String>> {
        let v = self
            .conn
            .query_row(
                "SELECT version FROM mods WHERE game_domain=?1 AND mod_id=?2",
                rusqlite::params![game_domain, i64::try_from(mod_id).unwrap_or(0)],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        Ok(v)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::{Catalog, Row};

    fn row(domain: &str, id: u64, name: &str, endorse: u64, size: u64) -> Row {
        Row {
            game_domain: domain.to_owned(),
            mod_id: id,
            name: name.to_owned(),
            endorsements: endorse,
            file_size: size,
            ..Row::default()
        }
    }

    #[test]
    fn upsert_search_and_names_roundtrip() {
        let dir = std::env::temp_dir().join(format!("cg-cat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut c = Catalog::open(&dir.join("catalog.sqlite")).unwrap();
        c.upsert(&[
            row(
                "skyrimspecialedition",
                1,
                "Immersive Armors",
                200_000,
                1_000_000,
            ),
            row("skyrimspecialedition", 2, "Some Weather", 50_000, 2000),
            row("fallout4", 3, "Power Armor", 40_000, 5000),
        ])
        .unwrap();
        assert_eq!(c.count().unwrap(), 3);
        // search is domain-scoped, substring, ranked by endorsements
        let hits = c.search("skyrimspecialedition", "armor", "", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "Immersive Armors");
        // extra_where filter (min endorsements) narrows
        assert!(
            c.search(
                "skyrimspecialedition",
                "",
                " AND endorsements >= 100000",
                10
            )
            .unwrap()
            .len()
                == 1
        );
        // last-write-wins on re-upsert
        c.upsert(&[row(
            "skyrimspecialedition",
            1,
            "Immersive Armors v2",
            210_000,
            1_000_000,
        )])
        .unwrap();
        assert_eq!(c.count().unwrap(), 3);
        let names = c.names("skyrimspecialedition", &[1, 2]).unwrap();
        assert_eq!(names.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // The browse-while-syncing guarantee: a second connection (the browser's
    // catalog search) must see rows a first connection (the running sync) has
    // already written, page by page — not be locked out until the sync ends.
    #[test]
    fn partial_rows_are_visible_to_a_second_reader_mid_sync() {
        let dir = std::env::temp_dir().join(format!("cg-cat-mid-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("catalog.sqlite");
        let mut writer = Catalog::open(&path).unwrap();
        // WAL is what makes the concurrent read possible.
        let mode: String = writer
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal", "catalog must open in WAL mode");

        // "page 1" of a sync lands.
        writer
            .upsert(&[row(
                "cyberpunk2077",
                1,
                "Cyber Engine Tweaks",
                227_000,
                17_000_000,
            )])
            .unwrap();
        // A separate reader opens mid-sync and can already search page 1.
        let reader = Catalog::open(&path).unwrap();
        assert_eq!(
            reader
                .search("cyberpunk2077", "engine", "", 10)
                .unwrap()
                .len(),
            1
        );
        // "page 2" lands on the writer; the same reader now sees it too.
        writer
            .upsert(&[row("cyberpunk2077", 2, "redscript", 158_000, 11_000_000)])
            .unwrap();
        assert_eq!(reader.count_domain("cyberpunk2077").unwrap(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sorted_search_and_category_filter() {
        use super::SortBy;
        let dir = std::env::temp_dir().join(format!("cg-cat-sort-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut c = Catalog::open(&dir.join("catalog.sqlite")).unwrap();
        let r = |id, name, cat, endorse, downloads| {
            let mut row = row("fallout4", id, name, endorse, 1000);
            row.category = String::from(cat);
            row.downloads = downloads;
            row
        };
        c.upsert(&[
            r(1, "Alpha Armor", "Armour", 100, 50),
            r(2, "Beta Blade", "Weapons", 500, 10),
            r(3, "Gamma Gun", "Weapons", 50, 900),
        ])
        .unwrap();

        // Default: endorsements desc across all categories.
        let all = c.search("fallout4", "", "", 10).unwrap();
        assert_eq!(
            all.iter().map(|h| h.mod_id).collect::<Vec<_>>(),
            vec![2, 1, 3]
        );

        // Category filter narrows to Weapons; sort by downloads desc.
        let weap = c
            .search_sorted("fallout4", "", "", Some("Weapons"), SortBy::Downloads, 10)
            .unwrap();
        assert_eq!(
            weap.iter().map(|h| h.mod_id).collect::<Vec<_>>(),
            vec![3, 2]
        );

        // Sort by name ascending.
        let by_name = c
            .search_sorted("fallout4", "", "", None, SortBy::Name, 10)
            .unwrap();
        assert_eq!(by_name.first().unwrap().name, "Alpha Armor");

        // Categories, most-populous first.
        let cats = c.categories("fallout4").unwrap();
        assert_eq!(cats[0], ("Weapons".to_owned(), 2));
        assert!(cats.iter().any(|(n, _)| n == "Armour"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn per_domain_status_counts_and_watermark() {
        let dir = std::env::temp_dir().join(format!("cg-cat-status-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut c = Catalog::open(&dir.join("catalog.sqlite")).unwrap();
        c.upsert(&[
            row("fallout4", 1, "A", 1, 1),
            row("fallout4", 2, "B", 1, 1),
            row("skyrimspecialedition", 3, "C", 1, 1),
        ])
        .unwrap();
        assert_eq!(c.count_domain("fallout4").unwrap(), 2);
        assert_eq!(c.count_domain("skyrimspecialedition").unwrap(), 1);
        assert_eq!(c.count_domain("bg3").unwrap(), 0);
        // never-synced domain reads epoch 0; after a watermark it's nonzero.
        assert_eq!(c.synced_at_epoch("fallout4").unwrap(), 0);
        let now = c.now_utc().unwrap();
        c.set_watermark("fallout4", "nexus", &now, &now, 2).unwrap();
        assert!(c.synced_at_epoch("fallout4").unwrap() > 0);
        assert_eq!(c.synced_at_epoch("skyrimspecialedition").unwrap(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
