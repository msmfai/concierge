//! Bethesda plugin invariants — the crash-causing rules the Bethesda adapter
//! enforces via `GameAdapter::lints` for every plugin-order game, resolved from
//! the adapter's data. Lives beside the adapter so it can produce lints without
//! a dependency cycle.
//!
//! Encoded here (Error unless noted):
//! - **missing master** — delegated to [`crate::missing_masters`].
//! - **255 full-plugin limit** — at most 254 non-light plugins may be active
//!   (indices 00–FD; FE = light pool, FF reserved). `.esl` / ESL-flagged
//!   plugins don't consume a full slot.
//! - **self / circular masters** — the master graph must be a DAG.

use std::collections::{HashMap, HashSet};

use concierge::error::Result;
use concierge::lint::Violation;
use concierge::plan::Plan;

const FULL_PLUGIN_LIMIT: usize = 254;

/// The plugin facts the invariants need — extracted from the parsed load order
/// so the checks are testable with synthetic data.
struct PluginInfo {
    name: String,
    is_light: bool,
    masters: Vec<String>,
}

/// Run the Bethesda plugin invariants against `plan`'s deployed load order.
///
/// # Errors
/// Propagates load-order parse errors.
pub fn bethesda_lints(plan: &Plan) -> Result<Vec<Violation>> {
    let mut out = Vec::new();
    for m in crate::missing_masters(plan)? {
        out.push(Violation::error(
            m.plugin,
            "missing-master",
            format!(
                "absent master(s): {} — add the dependency mod or disable this plugin",
                m.missing.join(", ")
            ),
        ));
    }
    let plugins: Vec<PluginInfo> = crate::read_load_order(plan)?
        .iter()
        .map(|p| PluginInfo {
            name: p.meta.name.clone(),
            is_light: p.meta.is_esl || is_esl_ext(&p.meta.name),
            masters: p.meta.masters.clone(),
        })
        .collect();
    out.extend(check_full_plugin_limit(&plugins));
    out.extend(check_master_graph(&plugins));
    Ok(out)
}

fn is_esl_ext(name: &str) -> bool {
    std::path::Path::new(name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("esl"))
}

fn check_full_plugin_limit(plugins: &[PluginInfo]) -> Vec<Violation> {
    let full = plugins.iter().filter(|p| !p.is_light).count();
    if full > FULL_PLUGIN_LIMIT {
        return vec![Violation::error(
            format!("{full} full plugins"),
            "plugin-limit",
            format!(
                "{full} active non-light plugins exceeds the {FULL_PLUGIN_LIMIT} limit \
                 (CTD on load) — flag some as ESL/light or remove plugins"
            ),
        )];
    }
    Vec::new()
}

/// Self-reference is always an error; a true cycle among loaded plugins is too.
/// Masters that aren't loaded plugins (vanilla/DLC) are handled by the
/// missing-master check and ignored here.
fn check_master_graph(plugins: &[PluginInfo]) -> Vec<Violation> {
    let mut out = Vec::new();
    let loaded: HashSet<String> = plugins.iter().map(|p| p.name.to_lowercase()).collect();
    for p in plugins {
        if p.masters.iter().any(|m| m.eq_ignore_ascii_case(&p.name)) {
            out.push(Violation::error(
                p.name.clone(),
                "self-master",
                "plugin lists itself as a master (won't launch / CTD)",
            ));
        }
    }
    let mut state: HashMap<String, u8> = HashMap::new(); // 0=unseen,1=in-progress,2=done
    for p in plugins {
        if visit_cycle(&p.name.to_lowercase(), plugins, &loaded, &mut state) {
            out.push(Violation::error(
                p.name.clone(),
                "circular-master",
                "circular master dependency among plugins (won't launch / CTD)",
            ));
            break; // one report is enough; the whole order is unloadable
        }
    }
    out
}

fn visit_cycle(
    name: &str,
    plugins: &[PluginInfo],
    loaded: &HashSet<String>,
    state: &mut HashMap<String, u8>,
) -> bool {
    match state.get(name) {
        Some(1) => return true,  // back-edge → cycle
        Some(2) => return false, // fully explored, clean
        _ => {}
    }
    state.insert(name.to_owned(), 1);
    if let Some(p) = plugins.iter().find(|p| p.name.eq_ignore_ascii_case(name)) {
        for m in &p.masters {
            let ml = m.to_lowercase();
            if ml != name && loaded.contains(&ml) && visit_cycle(&ml, plugins, loaded, state) {
                return true;
            }
        }
    }
    state.insert(name.to_owned(), 2);
    false
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn p(name: &str, light: bool, masters: &[&str]) -> PluginInfo {
        PluginInfo {
            name: name.to_owned(),
            is_light: light,
            masters: masters.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    #[test]
    fn full_plugin_limit_flags_over_254_non_light() {
        let many: Vec<PluginInfo> = (0..255)
            .map(|i| p(&format!("m{i}.esp"), false, &[]))
            .collect();
        assert_eq!(check_full_plugin_limit(&many).len(), 1);
        let light: Vec<PluginInfo> = (0..255)
            .map(|i| p(&format!("m{i}.esp"), true, &[]))
            .collect();
        assert!(
            check_full_plugin_limit(&light).is_empty(),
            "light plugins don't count"
        );
    }

    #[test]
    fn self_and_circular_masters_flagged_clean_dag_passes() {
        assert_eq!(
            check_master_graph(&[p("A.esp", false, &["A.esp"])]).len(),
            1,
            "self"
        );
        let cyc = [p("A.esp", false, &["B.esp"]), p("B.esp", false, &["A.esp"])];
        assert!(!check_master_graph(&cyc).is_empty(), "cycle flagged");
        let dag = [p("A.esp", false, &[]), p("B.esp", false, &["A.esp"])];
        assert!(check_master_graph(&dag).is_empty(), "clean DAG passes");
    }
}
