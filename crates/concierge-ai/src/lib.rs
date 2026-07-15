//! Rung 3 — **Synthesize**. The AI orchestrator: it calls the deterministic
//! core (in `tools`) via the Anthropic API (in `client`), the core executes and
//! verifies, and the AI only does what the lower rungs can't (Rule of Least
//! Power). The core is the oracle/guardrail — every AI-proposed artifact is
//! validated by the same checks as hand-written code.
//!
//! Two entry points:
//! - `run_agent` — the autonomous loop (needs a client-side API key).
//! - `apply_decision` — execute + verify a decision produced by the model
//!   (works today; the model's judgment routed through the same tools/oracle).

pub mod agent;
pub mod client;
pub mod tools;

use serde::Serialize;

use concierge::plan::Plan;
use concierge::repo::Repo;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no Anthropic API key (put one in ~/.config/concierge/anthropic-api-key to run the autonomous loop)")]
    NoKey,
    #[error("api: {0}")]
    Api(String),
    #[error("tool: {0}")]
    Tool(String),
}

/// A model decision to escalate an asset conflict by choosing a winner.
#[derive(Debug, Clone)]
pub struct AssetDecision {
    pub path: String,
    pub winner_mod: String,
    /// The model's stated reasoning (recorded in the report).
    pub reason: String,
}

/// The rung-by-rung outcome — what the deterministic rungs handled vs what the
/// AI synthesized vs what remains.
#[derive(Debug, Serialize)]
pub struct Report {
    pub plugins: usize,
    /// Rung 2 handled these (mergeable record conflicts).
    pub reconcilable_record_conflicts: usize,
    /// Rung 2 refused these (danger-class); they escalate.
    pub danger_class: usize,
    /// Asset conflicts the deterministic rung reports but won't merge.
    pub asset_conflicts: usize,
    /// The fixes the AI authored + the oracle verified.
    pub synthesized: Vec<tools::Resolution>,
    /// Instance still vanilla/plan-consistent after synthesis.
    pub oracle_clean: bool,
}

/// Execute + verify one or more model decisions, producing a rung-by-rung
/// report. The escalation (author a fix for a conflict Rung 2 refuses) is the
/// novel step; every fix is verified by the oracle before it counts.
pub fn apply_decision(
    repo: &Repo,
    plan: &Plan,
    decisions: &[AssetDecision],
) -> Result<Report, Error> {
    let landscape = tools::conflict_landscape(repo, plan)?;
    let mut synthesized = Vec::new();
    for d in decisions {
        let res = tools::resolve_asset(repo, plan, &d.path, &d.winner_mod)?;
        // guardrail: only a verified resolution counts
        if res.verified {
            synthesized.push(res);
        }
    }
    let oracle_clean = tools::vanilla_and_state_check(repo, plan)?;
    Ok(Report {
        plugins: landscape.plugins,
        reconcilable_record_conflicts: landscape.mergeable_left,
        danger_class: landscape.danger_class,
        asset_conflicts: landscape.asset_conflicts.len(),
        synthesized,
        oracle_clean,
    })
}

/// System prompt anchoring the agent to the ladder + the oracle discipline.
#[must_use]
pub fn system_prompt() -> String {
    "You are Concierge's Synthesize rung. The deterministic core has already \
     acquired, ordered, and reconciled everything it safely can. Your job is \
     ONLY the residue it refuses: choose winners for asset-path conflicts, and \
     author bespoke fixes for danger-class record conflicts. Rule of Least \
     Power: never redo what the lower rungs did. Every fix you propose is \
     executed and VERIFIED by the core (round-trip, resolves the conflict, \
     vanilla stays clean); an unverified fix does not count. Use the tools; \
     propose, don't assert."
        .to_owned()
}

/// The autonomous loop: drive the model with the tools until it stops, executing
/// each tool call against the real core. Needs an API key.
pub fn run_agent(repo: &Repo, plan: &Plan, goal: &str, model: &str) -> Result<Report, Error> {
    let key = client::api_key()?;
    let tool_specs = tools::tool_specs();
    let mut messages = serde_json::json!([
        {"role": "user", "content": format!("Goal: {goal}\nStart by calling conflict_landscape.")}
    ]);
    // bounded loop (safety); each turn either calls tools or ends
    for _ in 0..12 {
        let resp = client::send(&key, model, &system_prompt(), &messages, &tool_specs)?;
        let content = resp.get("content").cloned().unwrap_or_default();
        // record the assistant turn
        if let Some(arr) = messages.as_array_mut() {
            arr.push(serde_json::json!({"role": "assistant", "content": content.clone()}));
        }
        let stop = resp
            .get("stop_reason")
            .and_then(|s| s.as_str())
            .unwrap_or("");
        if stop != "tool_use" {
            break;
        }
        // execute each tool_use block, collect tool_results
        let mut results = Vec::new();
        if let Some(blocks) = content.as_array() {
            for b in blocks {
                if b.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                    let name = b.get("name").and_then(|n| n.as_str()).unwrap_or_default();
                    let input = b.get("input").cloned().unwrap_or_default();
                    let id = b.get("id").and_then(|i| i.as_str()).unwrap_or_default();
                    let out = dispatch(repo, plan, name, &input)
                        .unwrap_or_else(|e| serde_json::json!({"error": e.to_string()}));
                    results.push(serde_json::json!({
                        "type": "tool_result", "tool_use_id": id, "content": out.to_string()
                    }));
                }
            }
        }
        if let Some(arr) = messages.as_array_mut() {
            arr.push(serde_json::json!({"role": "user", "content": results}));
        }
    }
    // summarize the resulting landscape
    let landscape = tools::conflict_landscape(repo, plan)?;
    let oracle_clean = tools::vanilla_and_state_check(repo, plan)?;
    Ok(Report {
        plugins: landscape.plugins,
        reconcilable_record_conflicts: landscape.mergeable_left,
        danger_class: landscape.danger_class,
        asset_conflicts: landscape.asset_conflicts.len(),
        synthesized: Vec::new(),
        oracle_clean,
    })
}

/// Route a model tool call to the real deterministic tool.
fn dispatch(
    repo: &Repo,
    plan: &Plan,
    name: &str,
    input: &serde_json::Value,
) -> Result<serde_json::Value, Error> {
    let s = |k: &str| {
        input
            .get(k)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned()
    };
    match name {
        "catalog_search" => {
            let limit = input
                .get("limit")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(8);
            let filter = concierge::manifest::Manifest::load(&repo.profile)
                .ok()
                .map_or_else(tools::CatalogFilter::default, |m| {
                    tools::CatalogFilter::from_curate(&m.curate)
                });
            let hits = tools::catalog_search(
                repo,
                &s("game"),
                &s("query"),
                u32::try_from(limit).unwrap_or(8),
                &filter,
            )?;
            serde_json::to_value(hits).map_err(|e| Error::Tool(e.to_string()))
        }
        "conflict_landscape" => serde_json::to_value(tools::conflict_landscape(repo, plan)?)
            .map_err(|e| Error::Tool(e.to_string())),
        "resolve_asset" => serde_json::to_value(tools::resolve_asset(
            repo,
            plan,
            &s("path"),
            &s("winner_mod"),
        )?)
        .map_err(|e| Error::Tool(e.to_string())),
        "vanilla_check" => {
            Ok(serde_json::json!({"clean": tools::vanilla_and_state_check(repo, plan)?}))
        }
        "validate_pipeline" => {
            let steps = input
                .get("steps")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let v = tools::validate_pipeline(&steps, &s("name"), &repo.store())?;
            serde_json::to_value(v).map_err(|e| Error::Tool(e.to_string()))
        }
        other => Err(Error::Tool(format!("unknown tool: {other}"))),
    }
}
