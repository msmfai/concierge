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

/// Strip the Windows extended-length `\\?\` (and `\\?\UNC\`) prefix that
/// `canonicalize` adds. No-op off Windows.
#[allow(clippy::missing_const_for_fn)] // const-able only on the non-Windows body
fn strip_verbatim(p: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(s) = p.to_str() {
            if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
                return PathBuf::from(format!(r"\\{rest}"));
            }
            if let Some(rest) = s.strip_prefix(r"\\?\") {
                return PathBuf::from(rest);
            }
        }
    }
    p
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
    // On Windows `canonicalize` returns an extended-length `\\?\` path. Used as a
    // process working directory it breaks PowerShell (it can't set that as its
    // location and exits before running anything), and it confuses many tools —
    // so strip the prefix. No-op elsewhere (canonicalize never adds it).
    let profile = strip_verbatim(profile);
    let repo = &Repo::at(&profile);
    let ws = write_set(repo, plan, extra_allow);
    let mut extra_env: Vec<(String, String)> = Vec::new();
    let program: Vec<String> = if !cmd.is_empty() {
        cmd.to_vec()
    } else if let Some(a) = agent {
        vec![a.to_owned()]
    } else {
        let (prog, env) = interactive_shell();
        extra_env = env;
        prog
    };
    let mut c = sandboxed(&ws, offline, &program)?;
    c.current_dir(&repo.profile)
        .env("CONCIERGE_REPO", &repo.profile)
        .env("CONCIERGE_SANDBOX", "1")
        .env("CONCIERGE_MOTD", sandbox_motd());
    for (k, v) in extra_env {
        c.env(k, v);
    }
    Ok(c)
}

/// Program + extra env for the custom sandboxed shell: print the MOTD, then hand
/// off to the user's interactive shell. The sandbox home is READ-ONLY, so zsh /
/// bash can't lock their history file there ("zsh: locking failed"); redirect
/// history to a writable per-session dir (under the temp allow-set) while still
/// loading the user's own config. `$CONCIERGE_MOTD` is set by the caller.
#[cfg(not(windows))]
fn interactive_shell() -> (Vec<String>, Vec<(String, String)>) {
    let sh = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
    let base = Path::new(&sh)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let state = std::env::temp_dir().join("concierge-shell");
    let _ = std::fs::create_dir_all(&state);
    let motd = "printf '%s\\n' \"$CONCIERGE_MOTD\"";
    let mut env: Vec<(String, String)> = Vec::new();
    let program = match base {
        // ZDOTDIR redirects zsh's dotfiles; source the real ~/.zshrc, then point
        // HISTFILE at the writable state dir so it wins over any rc setting.
        "zsh" => {
            let zdot = state.join("zdot");
            let _ = std::fs::create_dir_all(&zdot);
            let _ = std::fs::write(
                zdot.join(".zshrc"),
                format!(
                    "[[ -f \"$HOME/.zshrc\" ]] && source \"$HOME/.zshrc\"\n\
                     export HISTFILE=\"{s}/.zsh_history\"\n",
                    s = state.display()
                ),
            );
            env.push(("ZDOTDIR".to_owned(), zdot.display().to_string()));
            vec![
                "/bin/sh".to_owned(),
                "-c".to_owned(),
                format!("{motd}; exec {sh} -i"),
            ]
        }
        "bash" => {
            let rc = state.join("bashrc");
            let _ = std::fs::write(
                &rc,
                format!(
                    "[ -f \"$HOME/.bashrc\" ] && source \"$HOME/.bashrc\"\n\
                     export HISTFILE=\"{s}/.bash_history\"\n",
                    s = state.display()
                ),
            );
            vec![
                "/bin/sh".to_owned(),
                "-c".to_owned(),
                format!("{motd}; exec {sh} --rcfile '{}' -i", rc.display()),
            ]
        }
        _ => {
            env.push((
                "HISTFILE".to_owned(),
                state.join(".sh_history").display().to_string(),
            ));
            vec![
                "/bin/sh".to_owned(),
                "-c".to_owned(),
                format!("{motd}; exec {sh} -i"),
            ]
        }
    };
    (program, env)
}

/// Windows interactive shell: PowerShell ships with every Windows 10/11 and,
/// unlike zsh under a read-only home, has no history-lock problem. The Windows
/// sandbox bootstrap prints the MOTD and confines the process, so here we just
/// drop into an interactive prompt (the user's PowerShell profile still loads).
#[cfg(windows)]
// Mirrors the non-Windows signature (which isn't const), so allow the nursery
// lint rather than split the callers on constness.
#[allow(clippy::missing_const_for_fn)]
fn interactive_shell() -> (Vec<String>, Vec<(String, String)>) {
    // An empty program means "become the interactive guarded shell": the Windows
    // bootstrap lowers THIS PowerShell session to Low integrity and -NoExit keeps
    // you at the prompt — no nested process, so it stays attached to the terminal.
    (Vec::new(), Vec::new())
}

/// The greeting an interactive sandboxed shell prints on start — what the
/// sandbox is, and that you can run your own AI assistant inside it. Kept
/// plain-ASCII-safe so it renders in any terminal.
fn sandbox_motd() -> String {
    "\
========================================================================
  Concierge sandbox
========================================================================
  This shell can only write to THIS modpack — its profile folder and
  the shared download cache. The pristine game and the rest of your
  machine are read-only; nothing you do here can escape Concierge.

  Run your AI coding agent right here, inside the sandbox:
    opencode    codex    claude
  Whichever you have installed — start it and it builds the pack for
  you, confined to this modpack. The profile carries CLAUDE.md and
  AGENTS.md plus the slash-commands /health /curate /sort /conflicts
  /audit-ids, so any agent already knows the tools. CONCIERGE_REPO
  points at this profile. Type 'exit' to leave.
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

/// The Windows sandbox: a single PowerShell session that grants Low-integrity
/// write on each allowed path (built-in `icacls`), prints the MOTD, then lowers
/// ITS OWN integrity to Low and continues — interactively (`-NoExit`) or running
/// the given command. Windows' Mandatory Integrity Control then blocks that
/// session (and anything it launches — your agent) from writing "up" to any
/// Medium object, so everything NOT relabeled — the pristine game, your files,
/// the whole machine — is read-only for free, mirroring the seatbelt/bwrap
/// write-policy. Self-lowering (never a nested `CreateProcessAsUser`) keeps the
/// session attached to the terminal the GUI spawned it in. Nothing to install
/// (PowerShell + icacls ship with Windows); no Rust `unsafe` (the one privileged
/// call lives in the embedded C#, shelled out to like sandbox-exec / bwrap).
#[cfg(windows)]
fn sandboxed(ws: &WriteSet, offline: bool, program: &[String]) -> Result<Command> {
    if offline {
        // MIC does not gate network, so we can't honor an offline request here
        // yet — refuse rather than silently run online.
        return Err(Error::Other(
            "offline sandbox is not supported on Windows yet (Low-integrity confinement \
             does not cut off network)"
                .into(),
        ));
    }
    let boot = windows_bootstrap();
    let script = std::env::temp_dir().join("concierge-sandbox.ps1");
    std::fs::write(&script, &boot)
        .map_err(|e| Error::Other(format!("write sandbox bootstrap: {e}")))?;
    // Drop the exact script beside the session trace so a failure can be read
    // against what actually ran.
    crate::diag::artifact("sandbox.ps1", &boot);
    // Only existing paths can be relabeled; a missing allow simply isn't
    // writable, matching the seatbelt/bwrap treatment of absent paths.
    let allow = ws
        .allow
        .iter()
        .filter(|p| p.exists())
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let interactive = program.is_empty();
    crate::diag::event(
        "sandbox",
        "build",
        &format!(
            "windows Low-integrity shell · interactive={interactive} · allow_paths={} · script={}",
            allow.lines().count(),
            script.display()
        ),
    );
    let mut c = Command::new("powershell.exe");
    c.arg("-NoLogo")
        .arg("-NoProfile")
        .arg("-ExecutionPolicy")
        .arg("Bypass");
    // Empty program = the interactive guarded shell: stay in the session after
    // the bootstrap runs. A given command runs Low and the session exits.
    if interactive {
        c.arg("-NoExit");
    }
    c.arg("-File")
        .arg(&script)
        .args(program)
        .env("CONCIERGE_SB_ALLOW", allow);
    Ok(c)
}

/// The fixed PowerShell bootstrap for the Windows sandbox. Values arrive via the
/// environment (`CONCIERGE_SB_ALLOW`, `CONCIERGE_MOTD`) and as the script's own
/// arguments (an optional command to run), so the text needs no interpolation —
/// which keeps it free of path-escaping bugs and lets it be unit-tested verbatim.
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_bootstrap() -> String {
    // S-1-16-4096 is the Low mandatory-integrity SID. A process may always LOWER
    // its own integrity (never raise), so this needs no privilege and no nested
    // process — the session the GUI spawned simply drops to Low and carries on,
    // and every agent it launches inherits Low.
    r#"$ErrorActionPreference = 'Continue'

# Tagged one-line tracing into the session trace so a silent failure is readable
# after the fact. We relabel the LOG DIR to Low up front (below) so these keep
# writing even after this session drops to Low integrity.
function CgTrace($tag, $msg) {
  if ($env:CONCIERGE_LOG_DIR) {
    try {
      $ts = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
      Add-Content -LiteralPath (Join-Path $env:CONCIERGE_LOG_DIR 'trace.log') `
        -Value ('{0} {1,-9} {2,-12} {3}' -f $ts, 'bootstrap', $tag, $msg) -ErrorAction SilentlyContinue
    } catch {}
  }
}

CgTrace 'start' ('begin; args=' + $args.Count + '; ps=' + $PSVersionTable.PSVersion + '; host=' + $Host.Name)

try {
  # Make the log dir itself Low-writable so post-drop traces still land.
  if ($env:CONCIERGE_LOG_DIR) { & icacls $env:CONCIERGE_LOG_DIR /setintegritylevel '(OI)(CI)L' > $null 2>&1 }

  # Grant Low-integrity write on each allowed path. Everything NOT relabeled
  # stays Medium; MIC's no-write-up rule keeps it read-only for this Low session.
  CgTrace 'relabel' 'granting Low-integrity write on allow paths'
  if ($env:CONCIERGE_SB_ALLOW) {
    foreach ($p in ($env:CONCIERGE_SB_ALLOW -split "`n")) {
      if ($p -and (Test-Path -LiteralPath $p)) {
        & icacls $p /setintegritylevel '(OI)(CI)L' > $null 2>&1
      }
    }
  }

  CgTrace 'motd' 'printing MOTD'
  if ($env:CONCIERGE_MOTD) { Write-Host $env:CONCIERGE_MOTD }

  CgTrace 'addtype' 'compiling integrity helper'
  $src = @'
using System;
using System.Runtime.InteropServices;
public static class ConciergeSB {
  const int TokenIntegrityLevel = 25;
  const uint SE_GROUP_INTEGRITY = 0x00000020;
  const uint TOKEN_ADJUST_DEFAULT = 0x0080, TOKEN_QUERY = 0x0008;
  [StructLayout(LayoutKind.Sequential)] struct SID_AND_ATTRIBUTES { public IntPtr Sid; public uint Attributes; }
  [StructLayout(LayoutKind.Sequential)] struct TOKEN_MANDATORY_LABEL { public SID_AND_ATTRIBUTES Label; }
  [DllImport("kernel32.dll", SetLastError=true)] static extern IntPtr GetCurrentProcess();
  [DllImport("advapi32.dll", SetLastError=true)] static extern bool OpenProcessToken(IntPtr h, uint acc, out IntPtr tok);
  [DllImport("advapi32.dll", SetLastError=true, CharSet=CharSet.Unicode)] static extern bool ConvertStringSidToSid(string s, out IntPtr sid);
  [DllImport("advapi32.dll", SetLastError=true)] static extern bool SetTokenInformation(IntPtr tok, int cls, IntPtr info, int len);
  [DllImport("advapi32.dll")] static extern uint GetLengthSid(IntPtr sid);
  [DllImport("kernel32.dll")] static extern IntPtr LocalFree(IntPtr h);
  // Lower THIS process to Low integrity. Allowed without privilege; every child
  // (your agent) then inherits Low and stays confined to the granted paths.
  public static void DropToLow() {
    IntPtr tok, sid;
    if (!OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_DEFAULT | TOKEN_QUERY, out tok)) throw new Exception("OpenProcessToken " + Marshal.GetLastWin32Error());
    if (!ConvertStringSidToSid("S-1-16-4096", out sid)) throw new Exception("ConvertStringSidToSid " + Marshal.GetLastWin32Error());
    TOKEN_MANDATORY_LABEL tml = new TOKEN_MANDATORY_LABEL();
    tml.Label.Sid = sid; tml.Label.Attributes = SE_GROUP_INTEGRITY;
    int len = Marshal.SizeOf(typeof(TOKEN_MANDATORY_LABEL)) + (int)GetLengthSid(sid);
    IntPtr p = Marshal.AllocHGlobal(len);
    Marshal.StructureToPtr(tml, p, false);
    if (!SetTokenInformation(tok, TokenIntegrityLevel, p, len)) throw new Exception("SetTokenInformation " + Marshal.GetLastWin32Error());
    Marshal.FreeHGlobal(p); LocalFree(sid);
  }
}
'@
  Add-Type -TypeDefinition $src -Language CSharp
} catch {
  CgTrace 'error' ('bootstrap failed: ' + $_.Exception.Message)
  Write-Host ('Concierge sandbox error: ' + $_.Exception.Message)
}

# Lower THIS session to Low integrity for BOTH paths — the interactive prompt and
# any agent it launches are then confined to the granted paths. (The console is a
# conpty pseudoconsole whose handles this process already holds, so the drop keeps
# working after; portable-pty's ConPTY was the thing that used to break here.)
try { [ConciergeSB]::DropToLow(); CgTrace 'droptolow' 'lowered' }
catch { CgTrace 'error' ('DropToLow failed: ' + $_.Exception.Message); Write-Host ('Concierge sandbox error: ' + $_.Exception.Message) }

# A given command runs Low, then the session exits with its code.
if ($args.Count -gt 0) {
  CgTrace 'run' ('running: ' + ($args -join ' '))
  if ($args.Count -gt 1) { & $args[0] @($args[1..($args.Count - 1)]) }
  else { & $args[0] }
  CgTrace 'done' ('exit ' + $LASTEXITCODE)
  exit $LASTEXITCODE
}
# Interactive Low-integrity prompt; -NoExit keeps it open.
CgTrace 'interactive' 'interactive Low prompt'
"#
    .to_owned()
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
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
        let args: Vec<String> = c
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        // The program wraps the shell: print $CONCIERGE_MOTD, then run it.
        #[cfg(not(windows))]
        assert!(
            args.iter()
                .any(|a| a.contains("CONCIERGE_MOTD") && a.contains("exec")),
            "the shell wrapper prints the MOTD then execs the shell: {args:?}"
        );
        // On Windows the wrapper is the PowerShell sandbox bootstrap, which
        // prints $CONCIERGE_MOTD and confines the shell at Low integrity.
        #[cfg(windows)]
        assert!(
            args.iter().any(|a| a.contains("concierge-sandbox.ps1")),
            "the shell runs inside the PowerShell sandbox bootstrap: {args:?}"
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
            motd.contains("opencode") && motd.contains("codex") && motd.contains("claude"),
            "MOTD invites running any of opencode/codex/claude"
        );
        assert!(
            motd.contains("read-only") || motd.contains("only write"),
            "MOTD explains the write confinement"
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn interactive_shell_redirects_history_off_the_read_only_home() {
        // The sandbox home is read-only, so pointing shell history there fails
        // to lock ("zsh: locking failed"). Whatever the user's $SHELL, the
        // wrapper must steer history at the writable per-session state dir.
        let (program, env) = interactive_shell();
        let state = std::env::temp_dir().join("concierge-shell");
        let refs: Vec<String> = program
            .iter()
            .cloned()
            .chain(env.iter().map(|(_, v)| v.clone()))
            .collect();
        assert!(
            refs.iter()
                .any(|r| r.contains(&state.display().to_string())),
            "history/config is redirected under the writable state dir: {refs:?}"
        );
        // Never a bare `exec $SHELL -i` that would let zsh lock ~/.zsh_history:
        // history must be steered via ZDOTDIR / --rcfile / HISTFILE.
        let steered = env.iter().any(|(k, _)| k == "ZDOTDIR" || k == "HISTFILE")
            || program.iter().any(|a| a.contains("--rcfile"));
        assert!(
            steered,
            "history is steered explicitly: env={env:?} prog={program:?}"
        );
    }

    #[test]
    fn explicit_agent_runs_directly_without_the_shell_wrapper() {
        let (repo, plan) = fixture();
        let c = shell_command(&repo, &plan, Some("claude"), false, &[], &[]).unwrap();
        let args: Vec<String> = c
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.iter().any(|a| a == "claude"),
            "an explicit --agent runs directly, not wrapped: {args:?}"
        );
        assert!(
            !args.iter().any(|a| a.contains("CONCIERGE_MOTD")),
            "no MOTD wrapper for an explicit agent"
        );
    }

    // The Windows bootstrap is a fixed string, so its policy shape can be
    // asserted on any host (the actual confinement is verified by the Windows CI
    // smoke test, which spawns it on a real runner).
    #[test]
    fn windows_bootstrap_confines_via_low_integrity() {
        let s = windows_bootstrap();
        // Grants Low-integrity write only on the allowed paths…
        assert!(
            s.contains("CONCIERGE_SB_ALLOW") && s.contains("setintegritylevel"),
            "relabels the allowed paths to Low integrity"
        );
        assert!(s.contains("(OI)(CI)L"), "Low label, inherited by children");
        // …lowers THIS session to the Low SID (no privilege, no nested process)…
        assert!(s.contains("S-1-16-4096"), "Low mandatory-integrity SID");
        assert!(
            s.contains("DropToLow") && s.contains("SetTokenInformation"),
            "self-lowers this session's integrity (no CreateProcessAsUser)"
        );
        assert!(
            !s.contains("CreateProcessAsUser"),
            "no nested process — stays attached to the terminal"
        );
        // …and greets with the MOTD.
        assert!(s.contains("CONCIERGE_MOTD"), "prints the sandbox MOTD");
    }

    #[test]
    #[cfg(windows)]
    fn windows_shell_runs_inside_the_low_integrity_bootstrap() {
        let (repo, plan) = fixture();
        let c = shell_command(&repo, &plan, None, false, &[], &[]).unwrap();
        assert_eq!(
            c.get_program().to_string_lossy().to_ascii_lowercase(),
            "powershell.exe",
            "the Windows sandbox shells out to PowerShell"
        );
        let args: Vec<String> = c
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.iter().any(|a| a.contains("concierge-sandbox.ps1")),
            "runs the sandbox bootstrap script: {args:?}"
        );
        // Interactive = stay in the (self-lowered) session, no nested program.
        assert!(
            args.iter().any(|a| a == "-NoExit"),
            "the interactive guarded shell stays open: {args:?}"
        );
        // The allowed write-set is handed to the bootstrap via the environment.
        assert!(
            c.get_envs()
                .any(|(k, _)| k == std::ffi::OsStr::new("CONCIERGE_SB_ALLOW")),
            "the write-set is passed to the bootstrap"
        );
    }

    // The real proof, run on a Windows CI runner: spawn the sandbox and confirm
    // the Low child can write inside the write-set but is blocked from writing
    // outside it. If the sandbox were broken (child at Medium integrity), the
    // "forbidden" write would land and this fails.
    #[test]
    #[cfg(windows)]
    fn windows_low_integrity_blocks_writes_outside_the_write_set() {
        let (repo, plan) = fixture();
        let pid = std::process::id();
        // Allowed: inside the profile (granted + relabeled to Low).
        let allowed = repo.profile.join(format!("sb-allowed-{pid}.txt"));
        // Forbidden: the user-profile root — Medium integrity, not in the write-set.
        let forbidden = crate::repo::home().join(format!("sb-forbidden-{pid}.txt"));
        let _ = std::fs::remove_file(&allowed);
        let _ = std::fs::remove_file(&forbidden);
        let cmd = vec![
            "powershell.exe".to_owned(),
            "-NoProfile".to_owned(),
            "-NoLogo".to_owned(),
            "-Command".to_owned(),
            format!(
                "Set-Content -LiteralPath '{a}' -Value ok -ErrorAction SilentlyContinue; \
                 Set-Content -LiteralPath '{f}' -Value bad -ErrorAction SilentlyContinue",
                a = allowed.display(),
                f = forbidden.display()
            ),
        ];
        let status = shell_command(&repo, &plan, None, false, &[], &cmd)
            .unwrap()
            .status()
            .unwrap();
        let allowed_written = allowed.exists();
        let forbidden_written = forbidden.exists();
        let _ = std::fs::remove_file(&allowed);
        let _ = std::fs::remove_file(&forbidden); // clean up even if it leaked out
        assert!(
            allowed_written,
            "the Low child can write inside the write-set (bootstrap status {status:?})"
        );
        assert!(
            !forbidden_written,
            "the Low child is blocked from writing outside the write-set"
        );
    }

    #[test]
    #[cfg(windows)]
    fn windows_offline_is_refused_rather_than_run_online() {
        // MIC does not gate network; an offline request must fail loudly, not
        // silently run online.
        let (repo, plan) = fixture();
        assert!(
            shell_command(&repo, &plan, None, true, &[], &[]).is_err(),
            "offline sandbox is refused on Windows"
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
    // Seatbelt is the macOS backend, and its path-escaper rejects the
    // backslashes in a Windows temp path — so this only applies off-Windows.
    #[cfg(not(windows))]
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
