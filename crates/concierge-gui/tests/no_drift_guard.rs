//! Architectural guard — enforces the model / view-model / two-view split *by
//! construction*: the human GUI may trigger an action ONLY by dispatching an id
//! from the shared view-model vocabulary (`self.dispatch_intent(...)` or a queued
//! `clicks.push(...)` that is dispatched). The machine/headless view drives those
//! same ids. So a control a human can operate but the agent cannot — drift — is
//! caught here mechanically.
//!
//! Every `.clicked()` in the GUI must route to dispatch. This is a RATCHET: the
//! baseline is the number of legacy hand-rendered controls still to migrate; it
//! may only ever DECREASE. A new direct-action control pushes the count over the
//! baseline and fails CI, making drift impossible to merge unnoticed.

#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

/// Direct-action `.clicked()` sites not yet routed through `dispatch_intent`.
/// LOWER this as surfaces migrate into the view-model; NEVER raise it.
const BASELINE_BYPASS: usize = 30;

fn count_bypass() -> usize {
    let files = ["src/main.rs", "src/downloads.rs", "src/updates.rs"];
    let mut bypass = 0;
    for f in files {
        let path = format!("{}/{f}", env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        for (i, l) in lines.iter().enumerate() {
            if l.contains(".clicked()") {
                let end = (i + 7).min(lines.len());
                let window = lines[i..end].join("\n");
                let routed = window.contains("dispatch_intent(") || window.contains("clicks.push(");
                if !routed {
                    bypass += 1;
                }
            }
        }
    }
    bypass
}

#[test]
fn no_new_hand_rendered_actions_bypass_the_view_model() {
    let bypass = count_bypass();
    assert!(
        bypass <= BASELINE_BYPASS,
        "{bypass} direct-action controls bypass the view-model (baseline {BASELINE_BYPASS}). \
         A human control must trigger a view-model action id via self.dispatch_intent(...) so the \
         machine/agent view can drive it too. Route the new control through dispatch, or the two \
         views will drift."
    );
    if bypass < BASELINE_BYPASS {
        // A gain to lock in — not a failure, just a nudge.
        eprintln!(
            "note: bypass dropped to {bypass}; lower BASELINE_BYPASS to {bypass} to ratchet it."
        );
    }
}
