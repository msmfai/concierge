//! Catalog sync: paginate the public GraphQL endpoint into the local store.
//!
//! First run = full sweep (createdAt-ordered); later runs filter on the
//! stored `updatedAt` watermark. `ReplacingMergeTree` makes re-inserts
//! idempotent.

use std::time::Duration;

use crate::catalog::{Catalog, Row};
use crate::error::{Error, Result};
use crate::gql::{self, GqlMod, PAGE_SIZE};

const THROTTLE: Duration = Duration::from_millis(300);

#[derive(Debug, Default)]
pub struct SyncReport {
    pub total_count: u64,
    pub rows_synced: u64,
    pub pages: u64,
    pub full_sweep: bool,
    pub max_updated_at: String,
}

/// One GraphQL mod -> one catalog [`Row`]. Pure (unit-tested).
#[must_use]
pub fn mod_to_row(game_domain: &str, m: &GqlMod) -> Row {
    Row {
        game_domain: game_domain.to_owned(),
        mod_id: m.mod_id,
        name: m.name.clone(),
        summary: m.summary.clone().unwrap_or_default(),
        author: m.author.clone().unwrap_or_default(),
        version: m.version.clone().unwrap_or_default(),
        category: m
            .mod_category
            .as_ref()
            .and_then(|c| c.name.clone())
            .unwrap_or_default(),
        endorsements: m.endorsements,
        downloads: m.downloads,
        file_size: m.file_size.unwrap_or(0),
        adult: m.adult_content,
        updated_at: m.updated_at.clone(),
    }
}

/// Sync a game's Nexus catalog into the embedded `SQLite` [`Catalog`]. Full
/// sweep on first run; incremental (`updatedAt >= watermark`) afterwards.
pub fn sync_game(
    cat: &mut Catalog,
    game_domain: &str,
    progress: &mut dyn FnMut(&str),
) -> Result<SyncReport> {
    let wm_epoch = cat.watermark_epoch(game_domain)?;
    let updated_gte = (wm_epoch > 0).then(|| wm_epoch.to_string());
    let full_sweep = updated_gte.is_none();

    let synced_at = cat.now_utc()?;
    let mut report = SyncReport {
        full_sweep,
        ..SyncReport::default()
    };
    let mut offset: u64 = 0;
    loop {
        let body = gql::build_query(game_domain, PAGE_SIZE, offset, updated_gte.as_deref());
        let page = gql::fetch_page(&body)?;
        report.total_count = page.total_count;
        report.pages += 1;

        let mut rows = Vec::with_capacity(page.nodes.len());
        for m in &page.nodes {
            if m.updated_at.as_str() > report.max_updated_at.as_str() {
                report.max_updated_at.clone_from(&m.updated_at);
            }
            rows.push(mod_to_row(game_domain, m));
        }
        report.rows_synced += u64::try_from(cat.upsert(&rows)?).unwrap_or(0);
        progress(&format!(
            "  page {:>4}  {:>6}/{} rows",
            report.pages, report.rows_synced, report.total_count
        ));

        match gql::next_offset(offset, page.nodes.len(), page.total_count) {
            Some(next) => offset = next,
            None => break,
        }
        std::thread::sleep(THROTTLE);
    }

    // Fail loudly on an empty full sweep — almost always the wrong domain
    // (e.g. the game KIND `skyrimse` instead of the Nexus domain
    // `skyrimspecialedition`) — a silent "synced 0" would hide it.
    if full_sweep && report.total_count == 0 {
        return Err(Error::GraphQl(format!(
            "Nexus returned 0 mods for gameDomainName '{game_domain}' — is that the \
             Nexus domain? (e.g. skyrimspecialedition, not skyrimse; fallout4; baldursgate3)"
        )));
    }
    let max_updated = if report.max_updated_at.is_empty() {
        updated_gte.unwrap_or_else(|| synced_at.clone())
    } else {
        report.max_updated_at.clone()
    };
    cat.set_watermark(
        game_domain,
        "nexus",
        &synced_at,
        &max_updated,
        report.rows_synced,
    )?;
    Ok(report)
}

/// Modrinth sync: top projects by downloads (search offset caps at 10k --
/// reported, not silent). Full re-sync each run; the catalog upsert dedups.
pub fn sync_modrinth(
    cat: &mut Catalog,
    game_domain: &str,
    progress: &mut dyn FnMut(&str),
) -> Result<SyncReport> {
    use crate::modrinth;

    let synced_at = cat.now_utc()?;
    let mut report = SyncReport {
        full_sweep: true,
        ..SyncReport::default()
    };
    let mut offset: u64 = 0;
    loop {
        let page = modrinth::fetch_page(offset)?;
        report.total_count = page.total_hits;
        report.pages += 1;
        let mut rows = Vec::with_capacity(page.hits.len());
        for h in &page.hits {
            if h.date_modified.as_str() > report.max_updated_at.as_str() {
                report.max_updated_at.clone_from(&h.date_modified);
            }
            rows.push(modrinth::hit_to_row(game_domain, h));
        }
        report.rows_synced += u64::try_from(cat.upsert(&rows)?).unwrap_or(0);
        progress(&format!(
            "  page {:>4}  {:>6}/{} rows",
            report.pages, report.rows_synced, report.total_count
        ));
        let next = offset + u64::try_from(page.hits.len()).unwrap_or(0);
        if page.hits.is_empty() || next >= report.total_count {
            break;
        }
        if next >= modrinth::OFFSET_CAP {
            progress(&format!(
                "  NOTE: modrinth search caps offset at {} -- synced top {} of {} projects",
                modrinth::OFFSET_CAP,
                report.rows_synced,
                report.total_count
            ));
            break;
        }
        offset = next;
        std::thread::sleep(THROTTLE);
    }
    let max_updated = if report.max_updated_at.is_empty() {
        synced_at.clone()
    } else {
        report.max_updated_at.clone()
    };
    cat.set_watermark(
        game_domain,
        "modrinth",
        &synced_at,
        &max_updated,
        report.rows_synced,
    )?;
    Ok(report)
}
