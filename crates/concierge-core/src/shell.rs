//! The sandboxed agent shell: spawn a shell or agent for a
//! profile inside an OS sandbox whose write-policy is DERIVED FROM THE PLAN.
//! The plan already declares everything a modding session may write — the
//! workspace caches, the profile, the instance, the mounts, the config files —
//! so the sandbox turns that declaration into an OS-enforced boundary. The
//! pristine install is explicitly denied: "vanilla is sacred" holds even
//! against a hostile or confused agent. This is what makes handing an
//! embedded agent to strangers safe: the blast radius is Concierge's own
//! write-set, not the user's machine.
//!
//! Backends: macOS seatbelt (`sandbox-exec`; rule order matters — the LAST
//! matching rule wins, verified empirically) and Linux `bwrap`. Anything else
//! refuses loudly rather than running unsandboxed.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Error, Result};
use crate::plan::Plan;
use crate::repo::Repo;

/// The derived policy: paths a session may write, and paths that stay
/// read-only even inside allowed subtrees (pristine; locked manifests).
#[derive(Debug, Default)]
pub struct WriteSet {
    pub allow: Vec<PathBuf>,
    pub deny: Vec<PathBuf>,
}

/// Derive the write-set from the plan. Everything here is something the
/// modding lifecycle legitimately writes; nothing else on the machine is.
#[must_use]
pub fn write_set(repo: &Repo, plan: &Plan, extra_allow: &[PathBuf]) -> WriteSet {
    let mut allow: Vec<PathBuf> = vec![
        repo.store(),
        repo.builds(),
        repo.workspace.join("state"),
        repo.workspace.join("games"),
        repo.profile.clone(),
    ];
    if let Some(inst) = &plan.game.instance {
        allow.push(PathBuf::from(inst));
    }
    // Mount targets: realize parks the real dir (rename beside it) and plants
    // a symlink, so the real path AND its parent must be writable.
    for (_dir, real) in plan.mounts() {
        let real = PathBuf::from(real);
        if let Some(parent) = real.parent() {
            allow.push(parent.to_path_buf());
        }
        allow.push(real);
    }
    // Rendered config files (plugins.txt, modsettings.lsx, INIs) live at
    // absolute [game.paths] targets — their parent dirs are written.
    for c in plan.configs.iter().chain(plan.config_resets.iter()) {
        if let Some(parent) = Path::new(&c.path).parent() {
            allow.push(parent.to_path_buf());
        }
    }
    // Session plumbing: temp dirs, devices (tty!), and the agent's own state.
    allow.push(PathBuf::from("/dev"));
    allow.push(PathBuf::from("/tmp"));
    allow.push(PathBuf::from("/private/tmp"));
    allow.push(std::env::temp_dir());
    let home = crate::repo::home();
    allow.push(home.join(".claude"));
    allow.push(home.join(".claude.json"));
    allow.push(home.join(".claude.json.backup"));
    allow.push(home.join("Library/Caches/claude-cli-nodejs"));
    allow.extend(extra_allow.iter().cloned());
    allow.sort();
    allow.dedup();

    // Explicit denies OVERRIDE the allows (last-match-wins): the pristine is
    // never writable, even if a user config nests it under an allowed tree;
    // a Locked profile's declaration stays immutable inside the sandbox too.
    let mut deny = vec![PathBuf::from(&plan.game.pristine)];
    let manifest = repo.profile.join("manifest.toml");
    if is_read_only(&manifest) {
        deny.push(manifest);
        deny.push(repo.profile.join("concierge.lock"));
    }
    WriteSet { allow, deny }
}

fn is_read_only(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.permissions().readonly())
}

/// Render the macOS seatbelt profile. Rule order is load-bearing: default
/// deny-writes, then the allowed subtrees, then the hard denies so they win.
pub fn seatbelt_profile(ws: &WriteSet, offline: bool) -> Result<String> {
    use std::fmt::Write as _;
    let mut p = String::from("(version 1)\n(allow default)\n(deny file-write*)\n");
    for path in &ws.allow {
        let _ = writeln!(p, "(allow file-write* (subpath \"{}\"))", sb_escape(path)?);
    }
    for path in &ws.deny {
        let _ = writeln!(p, "(deny file-write* (subpath \"{}\"))", sb_escape(path)?);
    }
    if offline {
        p.push_str("(deny network*)\n");
    }
    Ok(p)
}

/// Seatbelt strings can't safely carry quotes/backslashes — refuse such paths
/// instead of risking a policy that parses differently than intended.
fn sb_escape(path: &Path) -> Result<String> {
    let s = path.to_string_lossy();
    if s.contains('"') || s.contains('\\') {
        return Err(Error::Other(format!(
            "path not sandboxable (quote/backslash): {s}"
        )));
    }
    Ok(s.into_owned())
}

/// Build the sandboxed command: `cmd` if given, else `agent`, else the user's
/// shell — confined to the plan's write-set, cwd'd to the profile.
pub fn shell_command(
    repo: &Repo,
    plan: &Plan,
    agent: Option<&str>,
    offline: bool,
    extra_allow: &[PathBuf],
    cmd: &[String],
) -> Result<Command> {
    // The session's cwd becomes the profile, so every path the sandbox and
    // the inner process see must be absolute — a relative CONCIERGE_REPO
    // would re-resolve against the new cwd (and relative seatbelt subpaths
    // never match).
    let profile = std::fs::canonicalize(&repo.profile).map_err(|e| {
        Error::Other(format!(
            "profile not resolvable: {}: {e}",
            repo.profile.display()
        ))
    })?;
    let repo = &Repo::at(&profile);
    let ws = write_set(repo, plan, extra_allow);
    let program: Vec<String> = if !cmd.is_empty() {
        cmd.to_vec()
    } else if let Some(a) = agent {
        vec![a.to_owned()]
    } else {
        // A custom sandboxed shell: print the MOTD (what the sandbox is + that you
        // can run claude/codex here), then hand off to the user's interactive
        // shell. `$CONCIERGE_MOTD` is set on the command below.
        let sh = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
        vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            format!("printf '%s\\n' \"$CONCIERGE_MOTD\"; exec {sh} -i"),
        ]
    };
    let mut c = sandboxed(&ws, offline, &program)?;
    c.current_dir(&repo.profile)
        .env("CONCIERGE_REPO", &repo.profile)
        .env("CONCIERGE_SANDBOX", "1")
        .env("CONCIERGE_MOTD", sandbox_motd());
    Ok(c)
}

/// The greeting an interactive sandboxed shell prints on start — what the
/// sandbox is, and that you can run your own AI assistant (claude/codex) inside
/// it. Kept plain-ASCII-safe so it renders in any terminal.
fn sandbox_motd() -> String {
    "\
========================================================================
  Concierge sandbox
========================================================================
  This shell can only write to THIS modpack — its profile folder and
  the shared download cache. The pristine game and the rest of your
  machine are read-only; nothing you do here can escape Concierge.

  Run an AI assistant right here, inside the sandbox:
    claude    start Claude Code in this modpack
    codex     start Codex CLI in this modpack
  The profile already carries CLAUDE.md and the slash-commands
  /health /curate /sort /conflicts /audit-ids, so the assistant knows
  the tools. CONCIERGE_REPO points at this profile. Type 'exit' to leave.
========================================================================
"
    .to_owned()
}

#[cfg(target_os = "macos")]
fn sandboxed(ws: &WriteSet, offline: bool, program: &[String]) -> Result<Command> {
    let profile = seatbelt_profile(ws, offline)?;
    let mut c = Command::new("/usr/bin/sandbox-exec");
    c.arg("-p").arg(profile);
    c.args(program);
    Ok(c)
}

#[cfg(target_os = "linux")]
#[allow(clippy::unnecessary_wraps)] // signature shared with the fallible macOS impl
fn sandboxed(ws: &WriteSet, offline: bool, program: &[String]) -> Result<Command> {
    // bwrap: read-only root, then rw-bind each allowed path that exists.
    // (bwrap can't bind a nonexistent path; missing allows simply aren't
    // writable, matching seatbelt's behavior for absent paths.)
    let mut c = Command::new("bwrap");
    c.args(["--ro-bind", "/", "/", "--dev", "/dev", "--proc", "/proc"]);
    for p in &ws.allow {
        if p.exists() && p != Path::new("/dev") {
            c.arg("--bind").arg(p).arg(p);
        }
    }
    for p in &ws.deny {
        if p.exists() {
            c.arg("--ro-bind").arg(p).arg(p);
        }
    }
    if offline {
        c.arg("--unshare-net");
    }
    c.arg("--");
    c.args(program);
    Ok(c)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn sandboxed(_ws: &WriteSet, _offline: bool, _program: &[String]) -> Result<Command> {
    Err(Error::Other(
        "no sandbox backend on this platform — refusing to run an agent shell unsandboxed".into(),
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn fixture() -> (Repo, Plan) {
        let base = std::env::temp_dir().join(format!("concierge-shell-{}", std::process::id()));
        let profile = base.join("games/bg3/profiles/default");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(base.join(".concierge-workspace"), "").unwrap();
        let repo = Repo::at(&profile);
        // The custom kind needs no adapter registry — the manifest is the shape.
        let m = crate::manifest::Manifest::parse(&concierge_manifest()).unwrap();
        let plan = crate::plan::eval(&m).unwrap();
        (repo, plan)
    }

    fn concierge_manifest() -> String {
        "[game]\nkind = \"custom\"\npristine = \"/tmp/sacred-pristine\"\n\
         instance = \"/tmp/concierge-shell-test/instance\"\nversion = \"1\"\n\
         [game.paths]\nmods = \"/tmp/concierge-shell-test/external/Mods\"\n\
         [game.custom]\ndefault_root = \"mods\"\nlaunch = [\"X.app\"]\n\
         [[game.custom.root]]\nname = \"game\"\ndir = \"\"\n\
         [[game.custom.root]]\nname = \"mods\"\npath_key = \"mods\"\n\
         [[game.custom.config]]\npath = \"/tmp/concierge-shell-test/cfg/order.txt\"\n\
         line_prefix = \"\"\ntemplate = \"{{plugins}}\"\n"
            .to_owned()
    }

    #[test]
    fn interactive_shell_greets_with_the_sandbox_motd() {
        let (repo, plan) = fixture();
        let c = shell_command(&repo, &plan, None, false, &[], &[]).unwrap();
        // The program wraps the shell: print $CONCIERGE_MOTD, then exec it.
        let args: Vec<String> = c.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(
            args.iter()
                .any(|a| a.contains("CONCIERGE_MOTD") && a.contains("exec")),
            "the shell wrapper prints the MOTD then execs the shell: {args:?}"
        );
        // …and the MOTD explains the sandbox + that you can run claude/codex.
        let motd = c
            .get_envs()
            .find(|(k, _)| *k == std::ffi::OsStr::new("CONCIERGE_MOTD"))
            .and_then(|(_, v)| v)
            .map(|v| v.to_string_lossy().into_owned())
            .unwrap_or_default();
        assert!(motd.contains("Concierge sandbox"), "MOTD names the sandbox");
        assert!(
            motd.contains("claude") && motd.contains("codex"),
            "MOTD invites running claude/codex"
        );
        assert!(
            motd.contains("read-only") || motd.contains("only write"),
            "MOTD explains the write confinement"
        );
    }

    #[test]
    fn explicit_agent_runs_directly_without_the_shell_wrapper() {
        let (repo, plan) = fixture();
        let c = shell_command(&repo, &plan, Some("claude"), false, &[], &[]).unwrap();
        let args: Vec<String> = c.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(
            args.iter().any(|a| a == "claude"),
            "an explicit --agent runs directly, not wrapped: {args:?}"
        );
        assert!(
            !args.iter().any(|a| a.contains("CONCIERGE_MOTD")),
            "no MOTD wrapper for an explicit agent"
        );
    }

    #[test]
    fn write_set_is_the_plan_and_nothing_else() {
        let (repo, plan) = fixture();
        let ws = write_set(&repo, &plan, &[]);
        let has = |s: &str| ws.allow.iter().any(|p| p.to_string_lossy().contains(s));
        assert!(has("store"), "shared store is writable");
        assert!(has("games"), "profiles are writable");
        assert!(has("concierge-shell-test/instance"), "instance is writable");
        assert!(has("external/Mods"), "mount target is writable");
        assert!(has("cfg"), "config parent is writable");
        assert!(
            ws.deny.iter().any(|p| p.ends_with("sacred-pristine")),
            "pristine is explicitly denied"
        );
        assert!(
            !ws.allow
                .iter()
                .any(|p| p.to_string_lossy().contains("sacred-pristine")),
            "pristine never in the allow list"
        );
    }

    #[test]
    fn locked_manifest_joins_the_deny_list() {
        let (repo, plan) = fixture();
        let manifest = repo.profile.join("manifest.toml");
        std::fs::write(&manifest, "x").unwrap();
        let mut perms = std::fs::metadata(&manifest).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&manifest, perms.clone()).unwrap();
        let ws = write_set(&repo, &plan, &[]);
        assert!(ws.deny.iter().any(|p| p.ends_with("manifest.toml")));
        #[allow(clippy::permissions_set_readonly_false)]
        {
            perms.set_readonly(false);
            std::fs::set_permissions(&manifest, perms).unwrap();
        }
        let ws = write_set(&repo, &plan, &[]);
        assert!(!ws.deny.iter().any(|p| p.ends_with("manifest.toml")));
    }

    #[test]
    fn seatbelt_profile_orders_rules_correctly() {
        let (repo, plan) = fixture();
        let ws = write_set(&repo, &plan, &[]);
        let p = seatbelt_profile(&ws, true).unwrap();
        let deny_default = p.find("(deny file-write*)\n").unwrap();
        let first_allow = p.find("(allow file-write* ").unwrap();
        let pristine_deny = p.find("sacred-pristine").unwrap();
        assert!(deny_default < first_allow, "default deny precedes allows");
        assert!(
            first_allow < pristine_deny,
            "hard denies come last (they win)"
        );
        assert!(p.contains("(deny network*)"), "offline denies network");
        assert!(!seatbelt_profile(&ws, false).unwrap().contains("network"));
    }

    #[test]
    fn hostile_paths_are_refused() {
        let ws = WriteSet {
            allow: vec![PathBuf::from("/tmp/has\"quote")],
            deny: Vec::new(),
        };
        assert!(seatbelt_profile(&ws, false).is_err());
    }
}
