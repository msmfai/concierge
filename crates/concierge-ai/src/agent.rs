//! Quick-action shortcuts for the UI.
//!
//! The GUI's agent view is an embedded terminal running the user's real
//! interactive agent, sandboxed by `concierge-cli shell` — never a bespoke
//! headless chat loop, which would hide permission denials from the transcript
//! and degrade the model to guessing.
//!
//! This module is pure data: the quick-action labels the UI projects. Their
//! behaviour lives in the profile's provisioned slash-commands
//! (`.claude/commands/*.md`), so the real agent runs them in the real
//! harness.

/// A fixed task/skill shortcut the UI shows as a button. The matching
/// behaviour is provisioned as a `/`-command in each profile.
#[derive(Debug, Clone, Copy)]
pub struct QuickAction {
    pub label: &'static str,
    /// The slash-command a user (or the embedded terminal) invokes to run it.
    pub command: &'static str,
}

/// The built-in quick-action buttons. Labels the UI renders; each maps to a
/// provisioned slash-command the agent terminal can run.
#[must_use]
pub const fn quick_actions() -> &'static [QuickAction] {
    &[
        QuickAction {
            label: "Health check",
            command: "/health",
        },
        QuickAction {
            label: "Curate a modpack",
            command: "/curate",
        },
        QuickAction {
            label: "Audit mod ids",
            command: "/audit-ids",
        },
        QuickAction {
            label: "Sort load order",
            command: "/sort",
        },
        QuickAction {
            label: "Explain conflicts",
            command: "/conflicts",
        },
        QuickAction {
            label: "Diagnose last launch",
            command: "/diagnose-launch",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_actions_are_populated_and_map_to_commands() {
        assert!(quick_actions().len() >= 4);
        assert!(quick_actions()
            .iter()
            .all(|a| !a.label.is_empty() && a.command.starts_with('/')));
    }
}
