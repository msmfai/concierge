//! The deterministic capabilities exposed to the AI as **tools**. Each is a
//! plain function over the real engine (catalog, conflict matrices, resolver,
//! vanilla check) returning structured data. The model proposes tool calls; the
//! core executes and — critically — *verifies*. The model never writes game
//! files directly.

use serde::Serialize;

use concierge::plan::Plan;
use concierge::repo::Repo;

use crate::Error;

#[derive(Debug, Serialize, Clone)]
pub struct CatalogHit {
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

/// The white-glove **hard** filters that narrow a catalog search — built from a
/// profile's `[curate]` block. (Soft preferences — preferred categories,
/// lore-friendliness, scope, must-haves — steer the curator, not the SQL.)
#[derive(Debug, Clone, Default)]
pub struct CatalogFilter {
    pub min_endorsements: u64,
    pub max_size_bytes: Option<u64>,
    pub updated_since: Option<String>,
    pub allow_adult: bool,
    pub avoid_categories: Vec<String>,
    /// Names/authors to exclude.
    pub avoid_terms: Vec<String>,
}

impl CatalogFilter {
    /// Derive the hard filters from a profile's `[curate]` block.
    #[must_use]
    pub fn from_curate(c: &concierge::manifest::Curate) -> Self {
        Self {
            min_endorsements: c.min_endorsements,
            max_size_bytes: c.max_size_mb.map(|m| m.saturating_mul(1024 * 1024)),
            updated_since: c.updated_since.clone(),
            allow_adult: c.nsfw,
            avoid_categories: c.categories_avoid.clone(),
            avoid_terms: c.avoid.clone(),
        }
    }

    /// The extra `WHERE` predicates (each begins with ` AND `). SQL values are
    /// escaped; the DB is queried locally read-only.
    pub(crate) fn where_sql(&self) -> String {
        use std::fmt::Write as _;
        let esc = |s: &str| s.replace('\'', "''").to_lowercase();
        let mut w = String::new();
        if self.min_endorsements > 0 {
            let _ = write!(w, " AND endorsements >= {}", self.min_endorsements);
        }
        if let Some(max) = self.max_size_bytes {
            let _ = write!(w, " AND file_size <= {max}");
        }
        if let Some(since) = &self.updated_since {
            let _ = write!(w, " AND updated_at >= '{}'", since.replace('\'', "''"));
        }
        if !self.allow_adult {
            w.push_str(" AND NOT adult");
        }
        if !self.avoid_categories.is_empty() {
            let list: Vec<String> = self
                .avoid_categories
                .iter()
                .map(|c| format!("'{}'", esc(c)))
                .collect();
            let _ = write!(w, " AND lower(category) NOT IN ({})", list.join(","));
        }
        for term in &self.avoid_terms {
            let t = esc(term);
            let _ = write!(
                w,
                " AND lower(name) NOT LIKE '%{t}%' AND lower(author) != '{t}'"
            );
        }
        w
    }
}

/// Search the local Nexus catalog (`ClickHouse`), narrowed by the curation
/// filters. Read-only.
pub fn catalog_search(
    repo: &Repo,
    game: &str,
    query: &str,
    limit: u32,
    filter: &CatalogFilter,
) -> Result<Vec<CatalogHit>, Error> {
    catalog_search_sorted(repo, game, query, limit, filter, None, SortBy::Endorsements)
}

pub use concierge_db::catalog::SortBy;

/// Distinct categories (with counts) for the browse filter, most-populous
/// first. Absent catalog → empty.
#[must_use]
pub fn catalog_categories(repo: &Repo, game: &str) -> Vec<(String, u64)> {
    concierge_db::catalog::Catalog::open(&repo.catalog_path())
        .ok()
        .and_then(|c| c.categories(game).ok())
        .unwrap_or_default()
}

/// Browse search with an explicit category filter and sort order. The plain
/// `catalog_search` is this with (all categories, endorsements-desc).
pub fn catalog_search_sorted(
    repo: &Repo,
    game: &str,
    query: &str,
    limit: u32,
    filter: &CatalogFilter,
    category: Option<&str>,
    sort: SortBy,
) -> Result<Vec<CatalogHit>, Error> {
    let cat = concierge_db::catalog::Catalog::open(&repo.catalog_path())
        .map_err(|e| Error::Tool(e.to_string()))?;
    let hits = cat
        .search_sorted(game, query, &filter.where_sql(), category, sort, limit)
        .map_err(|e| Error::Tool(e.to_string()))?;
    Ok(hits
        .into_iter()
        .map(|h| CatalogHit {
            mod_id: h.mod_id,
            name: h.name,
            endorsements: h.endorsements,
            kb: h.kb,
            author: h.author,
            summary: h.summary,
            category: h.category,
            downloads: h.downloads,
            updated_at: h.updated_at,
        })
        .collect())
}

/// Look up catalog names for a set of Nexus mod ids in one query (for turning a
/// tracked-mods wishlist of bare ids into named suggestions). Read-only.
pub fn catalog_names(repo: &Repo, game: &str, ids: &[u64]) -> Result<Vec<(u64, String)>, Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let cat = concierge_db::catalog::Catalog::open(&repo.catalog_path())
        .map_err(|e| Error::Tool(e.to_string()))?;
    cat.names(game, ids).map_err(|e| Error::Tool(e.to_string()))
}

/// Per-domain catalog status for the browser's sync affordance: how many mods
/// are cached and when they were last synced (Unix seconds, 0 = never). Absent
/// catalog file reads as (0, 0) — "never synced".
#[must_use]
pub fn catalog_status(repo: &Repo, game: &str) -> (u64, u64) {
    let path = repo.catalog_path();
    if !path.exists() {
        return (0, 0);
    }
    concierge_db::catalog::Catalog::open(&path).map_or((0, 0), |cat| {
        (
            cat.count_domain(game).unwrap_or(0),
            cat.synced_at_epoch(game).unwrap_or(0),
        )
    })
}

#[derive(Debug, Serialize)]
pub struct ConflictSummary {
    pub plugins: usize,
    pub record_conflicts: usize,
    pub danger_class: usize,
    pub mergeable_left: usize,
    pub asset_conflicts: Vec<AssetConflictInfo>,
}

#[derive(Debug, Serialize, Clone)]
pub struct AssetConflictInfo {
    pub path: String,
    pub providers: Vec<String>,
    pub winner_by_load_order: String,
    pub benign: bool,
}

/// The conflict landscape: record matrix summary + the asset conflicts the
/// deterministic rung reports but will NOT merge (these feed the AI).
pub fn conflict_landscape(repo: &Repo, plan: &Plan) -> Result<ConflictSummary, Error> {
    let (plugins, matrix) =
        concierge_pluginorder::conflict_matrix(plan).map_err(|e| Error::Tool(e.to_string()))?;
    let danger = matrix.conflicts.iter().filter(|c| c.danger).count();
    let assets = concierge_pluginorder::assets::asset_conflicts(repo, plan)
        .map_err(|e| Error::Tool(e.to_string()))?;
    Ok(ConflictSummary {
        plugins,
        record_conflicts: matrix.conflicts.len(),
        danger_class: danger,
        mergeable_left: matrix.conflicts.len().saturating_sub(danger),
        asset_conflicts: assets
            .into_iter()
            .map(|c| AssetConflictInfo {
                path: c.path,
                providers: c.providers,
                winner_by_load_order: c.winner,
                benign: c.benign,
            })
            .collect(),
    })
}

#[derive(Debug, Serialize)]
pub struct Resolution {
    pub path: String,
    pub chosen_winner: String,
    pub deployed: String,
    pub winner_hash: String,
    /// The oracle's verdict: the deployed bytes are the chosen winner's.
    pub verified: bool,
}

/// Author + verify a resolution for an asset conflict: make `winner_mod` win by
/// deploying its bytes loose, then confirm via the oracle. This is the Rung-3
/// action — a judgment the deterministic layer refuses to make.
pub fn resolve_asset(
    repo: &Repo,
    plan: &Plan,
    path: &str,
    winner_mod: &str,
) -> Result<Resolution, Error> {
    let (deployed, hash) =
        concierge_pluginorder::assets::resolve_by_winner(repo, plan, path, winner_mod)
            .map_err(|e| Error::Tool(e.to_string()))?;
    let verified = concierge_pluginorder::assets::verify_resolution(&deployed, hash)
        .map_err(|e| Error::Tool(e.to_string()))?;
    Ok(Resolution {
        path: path.to_owned(),
        chosen_winner: winner_mod.to_owned(),
        deployed: deployed.display().to_string(),
        winner_hash: format!("{hash:016x}"),
        verified,
    })
}

/// The AI's proposed acquisition pipeline, validated by the deterministic core:
/// the pin plus proof it produced a real, usable tree.
#[derive(Debug, serde::Serialize)]
pub struct PipelineValidation {
    pub md5: String,
    pub file_count: usize,
    /// A few top-level entries — evidence it's a mod tree, not an error page.
    pub sample: Vec<String>,
    pub usable: bool,
}

/// The AI proposes an acquisition pipeline (as step JSON); the core RUNS it with
/// the impure `run` verb forbidden, hashes the output, and extracts it to
/// confirm a usable tree — all before anything is pinned. The model proposes;
/// the deterministic core executes and verifies.
pub fn validate_pipeline(
    steps_json: &serde_json::Value,
    name: &str,
    work_root: &std::path::Path,
) -> Result<PipelineValidation, Error> {
    let steps: Vec<concierge::pipeline::Step> = serde_json::from_value(steps_json.clone())
        .map_err(|e| Error::Tool(format!("bad pipeline steps: {e}")))?;
    if steps.iter().any(concierge::pipeline::Step::is_impure) {
        return Err(Error::Tool(
            "AI-authored pipelines may not use the impure `run` verb".into(),
        ));
    }
    let work = work_root.join(format!("ai-pipe-{name}"));
    let _ = std::fs::remove_dir_all(&work);
    // allow_run = false: the safety gate the AI cannot bypass.
    let archive = concierge::pipeline::run(&steps, &work, &format!("{name}.archive"), false)
        .map_err(|e| Error::Tool(e.to_string()))?;
    let md5 = concierge::repo::md5_file(&archive).map_err(|e| Error::Tool(e.to_string()))?;

    // Extract to prove it's a usable tree (non-empty, real files).
    let tree = work.join("inspect");
    std::fs::create_dir_all(&tree).map_err(|e| Error::Tool(e.to_string()))?;
    let ok = concierge_platform::extract_archive(&archive, &tree).is_ok();
    let mut files = Vec::new();
    if ok {
        walk_files(&tree, &tree, &mut files);
    }
    files.sort();
    let sample = files.iter().take(8).cloned().collect();
    let validation = PipelineValidation {
        md5,
        file_count: files.len(),
        sample,
        usable: ok && !files.is_empty(),
    };
    let _ = std::fs::remove_dir_all(&work);
    Ok(validation)
}

fn walk_files(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk_files(root, &p, out);
        } else if let Ok(rel) = p.strip_prefix(root) {
            out.push(rel.to_string_lossy().into_owned());
        }
    }
}

/// The oracle: is the instance still consistent with vanilla + the plan?
pub fn vanilla_and_state_check(repo: &Repo, plan: &Plan) -> Result<bool, Error> {
    let drift =
        concierge::check::check(repo, plan, false).map_err(|e| Error::Tool(e.to_string()))?;
    Ok(drift.is_empty())
}

/// Anthropic tool-use schemas for the tools above (the model calls these names).
#[must_use]
pub fn tool_specs() -> serde_json::Value {
    serde_json::json!([
        {
            "name": "catalog_search",
            "description": "Search the local Nexus mod catalog by name substring. Returns mods with endorsements and size.",
            "input_schema": {"type": "object", "properties": {
                "game": {"type": "string"}, "query": {"type": "string"}, "limit": {"type": "integer"}
            }, "required": ["game", "query"]}
        },
        {
            "name": "conflict_landscape",
            "description": "Summarize the current profile's conflicts: record matrix (counts + danger-class) and the asset-path conflicts the deterministic rung reports but will not merge.",
            "input_schema": {"type": "object", "properties": {}}
        },
        {
            "name": "resolve_asset",
            "description": "Make winner_mod win an asset-path conflict by deploying its bytes loose, then verify. Use for asset conflicts the deterministic rung refuses to merge.",
            "input_schema": {"type": "object", "properties": {
                "path": {"type": "string"}, "winner_mod": {"type": "string"}
            }, "required": ["path", "winner_mod"]}
        },
        {
            "name": "vanilla_check",
            "description": "Verify the instance is still consistent with vanilla and the plan (no drift). The safety oracle.",
            "input_schema": {"type": "object", "properties": {}}
        },
        {
            "name": "validate_pipeline",
            "description": "For a mod NOT on Nexus, propose a declarative acquisition pipeline; the core runs it (impure `run` forbidden), hashes the output, and confirms a usable tree before pinning. Steps: [{\"http\":\"url\"},{\"git\":\"url@ref\"},{\"extract\":true},{\"pick\":\"subdir\"}]. Returns the md5 pin, file_count, and sample entries.",
            "input_schema": {"type": "object", "properties": {
                "name": {"type": "string"},
                "steps": {"type": "array", "items": {"type": "object"}}
            }, "required": ["name", "steps"]}
        }
    ])
}

#[cfg(test)]
mod curate_filter_tests {
    use super::CatalogFilter;
    use concierge::manifest::Curate;

    #[test]
    fn where_sql_reflects_hard_filters() {
        let f = CatalogFilter {
            min_endorsements: 5000,
            max_size_bytes: Some(50 * 1024 * 1024),
            updated_since: Some("2020-01-01".to_owned()),
            allow_adult: false,
            avoid_categories: vec!["Cheats".to_owned()],
            avoid_terms: vec!["SomeAuthor".to_owned()],
        };
        let w = f.where_sql();
        assert!(w.contains("endorsements >= 5000"));
        assert!(w.contains("file_size <= 52428800"));
        assert!(w.contains("updated_at >= '2020-01-01'"));
        assert!(w.contains("NOT adult"));
        assert!(w.contains("category) NOT IN ('cheats')"));
        assert!(w.contains("someauthor"));
    }

    #[test]
    fn default_excludes_adult_and_allow_adult_drops_the_clause() {
        let mut f = CatalogFilter::default();
        // nsfw is off by default → the only default predicate excludes adult
        assert_eq!(f.where_sql(), " AND NOT adult", "default excludes adult");
        f.allow_adult = true;
        assert!(
            !f.where_sql().contains("adult"),
            "allow_adult drops the clause"
        );
    }

    #[test]
    fn from_curate_maps_fields() {
        let c = Curate {
            min_endorsements: 100,
            max_size_mb: Some(10),
            nsfw: true,
            ..Curate::default()
        };
        let f = CatalogFilter::from_curate(&c);
        assert_eq!(f.min_endorsements, 100);
        assert_eq!(f.max_size_bytes, Some(10 * 1024 * 1024));
        assert!(f.allow_adult);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod catalog_status_tests {
    use super::catalog_status;
    use concierge::repo::Repo;
    use concierge_db::catalog::{Catalog, Row};

    #[test]
    fn status_reports_rows_and_never_synced() {
        let base = std::env::temp_dir().join(format!("cg-status-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let profile = base.join("games/fallout4/profiles/default");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::create_dir_all(base.join("state")).unwrap();
        std::fs::write(base.join(".concierge-workspace"), "").unwrap();
        let repo = Repo::at(&profile);

        // No catalog file yet → never synced, zero rows.
        assert_eq!(catalog_status(&repo, "fallout4"), (0, 0));

        let mut cat = Catalog::open(&repo.catalog_path()).unwrap();
        cat.upsert(&[
            Row {
                game_domain: "fallout4".into(),
                mod_id: 1,
                name: "A".into(),
                ..Row::default()
            },
            Row {
                game_domain: "fallout4".into(),
                mod_id: 2,
                name: "B".into(),
                ..Row::default()
            },
        ])
        .unwrap();
        let (rows, synced) = catalog_status(&repo, "fallout4");
        assert_eq!(rows, 2);
        assert_eq!(synced, 0, "rows present but no watermark yet");
        let now = cat.now_utc().unwrap();
        cat.set_watermark("fallout4", "nexus", &now, &now, 2)
            .unwrap();
        assert!(
            catalog_status(&repo, "fallout4").1 > 0,
            "watermark surfaces as synced-at"
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
