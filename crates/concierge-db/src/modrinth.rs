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
pub const PROJECT: &str = "https://api.modrinth.com/v2/project";
pub const PAGE_SIZE: u32 = 100;
pub const OFFSET_CAP: u64 = 10_000;
const UA: &str = "concierge-prototype/0.1 (local metadata cache)";

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

// ── Version resolution: project -> a concrete, free-to-download file ─────────

/// One published version of a project (the fields we pick a download from).
#[derive(Debug, Deserialize)]
pub struct Version {
    pub version_number: String,
    #[serde(default)]
    pub game_versions: Vec<String>,
    #[serde(default)]
    pub loaders: Vec<String>,
    pub date_published: String,
    #[serde(default)]
    pub files: Vec<VersionFile>,
}

/// One downloadable file of a version. Modrinth serves these over its open CDN
/// with **no key** — a plain HTTPS download, which is exactly why the free path
/// works for Modrinth where it can't for Nexus.
#[derive(Debug, Deserialize)]
pub struct VersionFile {
    pub url: String,
    pub filename: String,
    #[serde(default)]
    pub primary: bool,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub hashes: Hashes,
}

#[derive(Debug, Deserialize, Default)]
pub struct Hashes {
    #[serde(default)]
    pub sha1: String,
    #[serde(default)]
    pub sha512: String,
}

/// A resolved, directly-downloadable Modrinth file.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub url: String,
    pub filename: String,
    pub sha1: String,
    pub sha512: String,
    pub size: u64,
    pub version_number: String,
}

/// Fetch every published version of a project (by slug or id).
///
/// # Errors
/// Network/HTTP failure, or a non-JSON response.
pub fn fetch_versions(project: &str) -> Result<Vec<Version>> {
    let url = format!("{PROJECT}/{project}/version");
    ureq::get(&url)
        .set("User-Agent", UA)
        .call()?
        .into_json()
        .map_err(|e| Error::Other(format!("modrinth versions: {e}")))
}

/// Pick the best downloadable file from a version list: the newest version that
/// matches `game_version` and `loader` (each an optional filter), then its
/// primary file (or the first). Pure over the list — unit-tested.
#[must_use]
pub fn pick_file(
    versions: &[Version],
    game_version: Option<&str>,
    loader: Option<&str>,
) -> Option<Resolved> {
    let mut matches: Vec<&Version> = versions
        .iter()
        .filter(|v| {
            game_version.is_none_or(|g| v.game_versions.iter().any(|x| x == g))
                && loader.is_none_or(|l| v.loaders.iter().any(|x| x.eq_ignore_ascii_case(l)))
        })
        .collect();
    // ISO-8601 dates sort lexicographically; newest first.
    matches.sort_by(|a, b| b.date_published.cmp(&a.date_published));
    let v = matches.first()?;
    let f = v
        .files
        .iter()
        .find(|f| f.primary)
        .or_else(|| v.files.first())?;
    Some(Resolved {
        url: f.url.clone(),
        filename: f.filename.clone(),
        sha1: f.hashes.sha1.clone(),
        sha512: f.hashes.sha512.clone(),
        size: f.size,
        version_number: v.version_number.clone(),
    })
}

/// Resolve a project to a concrete free download, filtered by game version and
/// mod loader. Network.
///
/// # Errors
/// Network/HTTP failure, or no version matching the filters.
pub fn resolve(
    project: &str,
    game_version: Option<&str>,
    loader: Option<&str>,
) -> Result<Resolved> {
    let versions = fetch_versions(project)?;
    pick_file(&versions, game_version, loader).ok_or_else(|| {
        Error::Other(format!(
            "no Modrinth version of '{project}' matches game {game_version:?} / loader {loader:?}"
        ))
    })
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::{pick_file, Version};

    const VERSIONS: &str = r#"[
      {"version_number":"1.0","game_versions":["1.20.1"],"loaders":["fabric"],
       "date_published":"2024-01-01T00:00:00Z",
       "files":[{"url":"https://cdn/old.jar","filename":"old.jar","primary":true,"size":100,
                 "hashes":{"sha1":"aaa","sha512":"a2"}}]},
      {"version_number":"2.0","game_versions":["1.20.1"],"loaders":["fabric"],
       "date_published":"2024-06-01T00:00:00Z",
       "files":[{"url":"https://cdn/secondary.jar","filename":"secondary.jar","primary":false,
                 "size":5,"hashes":{}},
                {"url":"https://cdn/new.jar","filename":"new.jar","primary":true,"size":200,
                 "hashes":{"sha1":"bbb","sha512":"b2"}}]},
      {"version_number":"forge-only","game_versions":["1.20.1"],"loaders":["forge"],
       "date_published":"2025-01-01T00:00:00Z",
       "files":[{"url":"https://cdn/forge.jar","filename":"forge.jar","primary":true,"size":300,
                 "hashes":{"sha1":"ccc","sha512":"c2"}}]}
    ]"#;

    #[test]
    fn pick_file_takes_newest_match_and_primary_file() {
        let versions: Vec<Version> = serde_json::from_str(VERSIONS).unwrap();
        // fabric + 1.20.1: newest matching is 2.0; its PRIMARY file is new.jar.
        let r = pick_file(&versions, Some("1.20.1"), Some("fabric")).unwrap();
        assert_eq!(r.filename, "new.jar");
        assert_eq!(r.sha1, "bbb");
        assert_eq!(r.version_number, "2.0");
        // Loader filter keeps forge-only out even though it's newest overall.
        // With no loader filter, forge-only (newest) wins.
        assert_eq!(
            pick_file(&versions, Some("1.20.1"), None)
                .unwrap()
                .version_number,
            "forge-only"
        );
        // No version matches an unknown game version.
        assert!(pick_file(&versions, Some("1.99"), None).is_none());
    }
}
