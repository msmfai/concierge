//! Public Nexus GraphQL v2 client — the endpoint that powers the website's
//! own search; no API key. We identify ourselves and throttle between pages.

use serde::Deserialize;

use crate::error::{Error, Result};

pub const ENDPOINT: &str = "https://api.nexusmods.com/v2/graphql";
pub const PAGE_SIZE: u32 = 100;

const MOD_SELECTION: &str = "modId uid name summary author version status \
     endorsements downloads fileSize adultContent supportsVortex \
     createdAt updatedAt modCategory { name }";

/// Build the paged catalog query. Pure (unit-tested).
/// Full sweeps sort by `createdAt ASC` — a stable order under pagination
/// (new mods only append). Incremental syncs add an `updatedAt >=` filter.
pub fn build_query(
    game_domain: &str,
    count: u32,
    offset: u64,
    updated_gte: Option<&str>,
) -> serde_json::Value {
    let updated_filter = updated_gte.map_or_else(String::new, |ts| {
        format!(r#", updatedAt: [{{value: "{ts}", op: GTE}}]"#)
    });
    let query = format!(
        r#"query {{ mods(
             filter: {{gameDomainName: [{{value: "{game_domain}", op: EQUALS}}]{updated_filter}}},
             sort: [{{createdAt: {{direction: ASC}}}}],
             count: {count}, offset: {offset}
           ) {{ totalCount nodes {{ {MOD_SELECTION} }} }} }}"#
    );
    serde_json::json!({ "query": query })
}

/// Next page offset, or `None` when the sweep is complete.
///
/// The API silently clamps `count` (observed: 100 -> 80 nodes), so a short
/// page does NOT mean the end — we advance by the nodes actually received
/// and stop only at `total_count` (or on an empty page, which would
/// otherwise loop forever). Pure (unit-tested).
pub fn next_offset(offset: u64, page_len: usize, total_count: u64) -> Option<u64> {
    if page_len == 0 {
        return None;
    }
    let next = offset + u64::try_from(page_len).unwrap_or(0);
    (next < total_count).then_some(next)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlMod {
    pub mod_id: u64,
    #[serde(default)]
    pub uid: Option<String>,
    pub name: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub endorsements: u64,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub file_size: Option<u64>,
    #[serde(default)]
    pub adult_content: bool,
    #[serde(default)]
    pub supports_vortex: Option<bool>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub mod_category: Option<Category>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Category {
    pub name: Option<String>,
}

#[derive(Debug)]
pub struct Page {
    pub total_count: u64,
    pub nodes: Vec<GqlMod>,
}

pub fn fetch_page(body: &serde_json::Value) -> Result<Page> {
    let resp: serde_json::Value = ureq::post(ENDPOINT)
        .set("Content-Type", "application/json")
        .set(
            "User-Agent",
            "concierge-prototype/0.1 (local metadata cache)",
        )
        .send_json(body.clone())?
        .into_json()
        .map_err(|e| Error::GraphQl(e.to_string()))?;
    if let Some(errors) = resp.get("errors") {
        return Err(Error::GraphQl(errors.to_string()));
    }
    let mods = resp
        .get("data")
        .and_then(|d| d.get("mods"))
        .ok_or_else(|| Error::GraphQl("missing data.mods".into()))?;
    let total_count = mods
        .get("totalCount")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| Error::GraphQl("missing totalCount".into()))?;
    let nodes: Vec<GqlMod> = serde_json::from_value(
        mods.get("nodes")
            .cloned()
            .ok_or_else(|| Error::GraphQl("missing nodes".into()))?,
    )?;
    Ok(Page { total_count, nodes })
}
