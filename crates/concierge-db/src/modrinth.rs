//! Modrinth catalog provider (Minecraft ecosystem). Open REST API, no key.
//!
//! Modrinth ids are strings (slugs/project ids) — they live in `uid`, with a
//! stable hash filling the numeric `mod_id` ordering key. The search API
//! caps `offset` at `10_000`, so a sync covers the top ~10k projects by
//! downloads; the cap is reported, never silent.

use md5::{Digest, Md5};
use serde::Deserialize;

use crate::error::{Error, Result};

pub const ENDPOINT: &str = "https://api.modrinth.com/v2/search";
pub const PAGE_SIZE: u32 = 100;
pub const OFFSET_CAP: u64 = 10_000;

#[derive(Debug, Deserialize)]
pub struct SearchHit {
    pub project_id: String,
    pub slug: Option<String>,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub follows: u64,
    pub date_created: String,
    pub date_modified: String,
}

#[derive(Debug)]
pub struct Page {
    pub total_hits: u64,
    pub hits: Vec<SearchHit>,
}

pub fn fetch_page(offset: u64) -> Result<Page> {
    let url = format!(
        "{ENDPOINT}?limit={PAGE_SIZE}&offset={offset}&index=downloads&facets=%5B%5B%22project_type%3Amod%22%5D%5D"
    );
    let resp: serde_json::Value = ureq::get(&url)
        .set(
            "User-Agent",
            "concierge-prototype/0.1 (local metadata cache)",
        )
        .call()?
        .into_json()
        .map_err(|e| Error::GraphQl(e.to_string()))?;
    let total_hits = resp
        .get("total_hits")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| Error::GraphQl("missing total_hits".into()))?;
    let hits: Vec<SearchHit> = serde_json::from_value(
        resp.get("hits")
            .cloned()
            .ok_or_else(|| Error::GraphQl("missing hits".into()))?,
    )?;
    Ok(Page { total_hits, hits })
}

/// Stable numeric ordering key from the string project id. Pure (unit-tested).
pub fn numeric_id(project_id: &str) -> u64 {
    let digest = Md5::digest(project_id.as_bytes());
    let bytes: [u8; 8] = digest
        .as_slice()
        .get(..8)
        .and_then(|s| s.try_into().ok())
        .unwrap_or_default();
    u64::from_le_bytes(bytes)
}

/// One hit -> one catalog [`crate::catalog::Row`]. Pure (unit-tested).
#[must_use]
pub fn hit_to_row(game_domain: &str, h: &SearchHit) -> crate::catalog::Row {
    crate::catalog::Row {
        game_domain: game_domain.to_owned(),
        mod_id: numeric_id(&h.project_id),
        name: h.title.clone(),
        summary: h.description.clone(),
        author: h.author.clone(),
        version: h.slug.clone().unwrap_or_default(),
        category: h.categories.join(","),
        endorsements: h.follows,
        downloads: h.downloads,
        file_size: 0,
        adult: false,
        updated_at: h.date_modified.clone(),
    }
}
