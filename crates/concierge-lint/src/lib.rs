//! Pre-launch validators — each game's crash-causing modding invariants, so
//! `concierge-cli realize` / `concierge-cli doctor` fail with a specific, cited reason
//! instead of a runtime crash. The LOOT / MO2 / SMAPI / BG3MM role, unified
//! behind one [`validate`] entry point.
//!
//! Dispatch is **adapter-first**: a game contributes its invariants via
//! `GameAdapter::lints` (the Bethesda plugin family does, covering every
//! plugin-order game). A game may instead keep a validator module in this
//! crate, called from the `match` below; such a module can move into its
//! adapter crate at any time. The lint result types live in `concierge::lint`
//! so adapter crates can produce them without a dependency cycle.

use concierge::error::Result;
use concierge::game::try_adapter;
use concierge::plan::Plan;

pub use concierge::lint::{partition, Severity, Violation};

mod bg3;
mod kotor;
mod minecraft;
mod rimworld;
mod stardew;
mod valheim;

/// Run the validators for `plan`'s game: the adapter's own lints, plus this
/// crate's per-game validators. Unknown games return nothing (simply not
/// linted yet).
pub fn validate(plan: &Plan) -> Result<Vec<Violation>> {
    // Adapter-owned lints — the Bethesda plugin family returns the missing-master
    // / plugin-limit / master-graph checks for ALL seven of its kinds; every
    // other adapter returns none by default.
    let mut out = try_adapter(&plan.game.kind)
        .map(|a| a.lints(plan))
        .transpose()?
        .unwrap_or_default();
    // Per-game validators kept in this crate; the Bethesda family's lints come
    // from its adapter above.
    out.extend(match plan.game.kind.as_str() {
        "stardew" => stardew::validate(plan)?,
        "rimworld" => rimworld::validate(plan)?,
        "minecraft" => minecraft::validate(plan)?,
        "bg3" => bg3::validate(plan)?,
        "valheim" => valheim::validate(plan)?,
        "kotor2" | "kotor" => kotor::validate(plan)?,
        _ => Vec::new(),
    });
    Ok(out)
}
