//! Host/OS abstractions shared across crates. The one place OS branching lives,
//! so the rest of the tree stays portable.
//!
//! Central piece: **helper-binary discovery** (`find_tool`) that locates
//! `clickhouse`, `7zz`, etc. Resolution order: an explicit env override, a copy
//! next to the running executable, a per-user tools cache, then the system
//! `PATH`.

use std::path::PathBuf;

/// A helper binary could not be located by any [`find_tool`] strategy.
#[derive(Debug, thiserror::Error)]
#[error("helper binary `{name}` not found — put it on PATH, next to the app, or set {env}")]
pub struct MissingTool {
    pub name: String,
    /// The env var that overrides discovery (e.g. `CONCIERGE_CLICKHOUSE`).
    pub env: String,
}

/// The env var that overrides discovery for `name` (e.g. `7zz` -> `CONCIERGE_7ZZ`).
#[must_use]
pub fn override_env(name: &str) -> String {
    let up: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("CONCIERGE_{up}")
}

/// Locate a helper binary without Nix. Order: `CONCIERGE_<NAME>` env override →
/// next to the running executable (bundled) → the per-user tools cache
/// ([`tools_dir`]) → the system `PATH`. Returns the resolved path.
pub fn find_tool(name: &str) -> Result<PathBuf, MissingTool> {
    let env = override_env(name);
    if let Some(v) = std::env::var_os(&env) {
        let p = PathBuf::from(v);
        if p.is_file() {
            return Ok(p);
        }
    }
    let candidates = candidate_names(name);
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            roots.push(dir.to_path_buf());
        }
    }
    if let Some(dir) = tools_dir() {
        roots.push(dir);
    }
    if let Some(path) = std::env::var_os("PATH") {
        roots.extend(std::env::split_paths(&path));
    }
    for dir in roots {
        for c in &candidates {
            let p = dir.join(c);
            if p.is_file() {
                return Ok(p);
            }
        }
    }
    Err(MissingTool {
        name: name.to_owned(),
        env,
    })
}

/// A [`std::process::Command`] for a discovered helper binary — the ergonomic
/// wrapper over [`find_tool`] for the common "run this tool" case.
pub fn tool_command(name: &str) -> Result<std::process::Command, MissingTool> {
    Ok(std::process::Command::new(find_tool(name)?))
}

/// Open a URL or file path with the OS default handler (macOS `open`, Windows
/// `start`, Linux `xdg-open`) — e.g. a `steam://` URL or a mod page.
pub fn open_url(target: &str) -> std::io::Result<std::process::ExitStatus> {
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg(target);
        c
    } else if cfg!(windows) {
        // `start` is a cmd builtin; the empty "" is the window-title arg.
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", "", target]);
        // Without this, a GUI (windows-subsystem) parent spawning `cmd` gets a
        // fresh console allocated — a blank cmd window flashes on every URL open.
        // CREATE_NO_WINDOW (0x0800_0000) suppresses it. Safe API, no `unsafe`.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            c.creation_flags(0x0800_0000);
        }
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(target);
        c
    };
    cmd.status()
}

/// Create a directory symlink `link` → `target`, cross-platform: a POSIX
/// symlink on Unix, a directory symlink on Windows (which needs Developer Mode
/// or admin — callers should surface that if it fails).
pub fn symlink_dir(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(target, link)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, link);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "symlinks unsupported on this platform",
        ))
    }
}

/// Best-effort: is a process whose image/cmdline contains `needle` running?
/// Uses `pgrep -f` on Unix and `tasklist` on Windows. Never errors — a failure
/// to check reads as "not running."
#[must_use]
pub fn process_running(needle: &str) -> bool {
    if cfg!(windows) {
        std::process::Command::new("tasklist")
            .output()
            .is_ok_and(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .to_lowercase()
                    .contains(&needle.to_lowercase())
            })
    } else {
        std::process::Command::new("pgrep")
            .args(["-f", needle])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }
}

/// Extract `archive` (zip/7z/rar/tar…) into `dest`, which is created if
/// absent. Primary extractor is `bsdtar` (libarchive — reads every format mod
/// archives ship, and is present on macOS, Windows 10+ as `tar.exe`, and Linux
/// via libarchive). If bsdtar is unavailable or fails, falls back to `7zz`
/// when one is on PATH. Errors carry both tools' stderr.
pub fn extract_archive(archive: &std::path::Path, dest: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| format!("create {}: {e}", dest.display()))?;
    let mut errs: Vec<String> = Vec::new();
    if let Ok(mut cmd) = tool_command("bsdtar") {
        match cmd.arg("-xf").arg(archive).arg("-C").arg(dest).output() {
            Ok(out) if out.status.success() => return Ok(()),
            Ok(out) => errs.push(format!(
                "bsdtar: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )),
            Err(e) => errs.push(format!("bsdtar: {e}")),
        }
    } else {
        errs.push("bsdtar: not found".to_owned());
    }
    if let Ok(mut cmd) = tool_command("7zz") {
        match cmd
            .arg("x")
            .arg("-y")
            .arg(format!("-o{}", dest.display()))
            .arg(archive)
            .output()
        {
            Ok(out) if out.status.success() => return Ok(()),
            Ok(out) => errs.push(format!(
                "7zz: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )),
            Err(e) => errs.push(format!("7zz: {e}")),
        }
    }
    Err(errs.join("\n"))
}

/// The filenames to try for `name` on this OS. Adds `.exe` on Windows and knows
/// a few per-OS binary-name aliases (the official 7-Zip console binary is
/// `7zz` on Linux/macOS but `7z`/`7za` on Windows; Windows' system `tar.exe`
/// IS bsdtar).
fn candidate_names(name: &str) -> Vec<String> {
    let mut bases = vec![name.to_owned()];
    if cfg!(windows) && name == "7zz" {
        bases.push("7z".to_owned());
        bases.push("7za".to_owned());
    }
    if cfg!(windows) && name == "bsdtar" {
        bases.push("tar".to_owned());
    }
    let mut out = Vec::new();
    for b in bases {
        let has_exe = std::path::Path::new(&b)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"));
        if cfg!(windows) && !has_exe {
            out.push(format!("{b}.exe"));
        }
        out.push(b);
    }
    out
}

/// The per-user directory where Concierge caches auto-obtained helper binaries
/// (`<data>/concierge/tools/`). `None` only if the OS data dir can't be found.
#[must_use]
pub fn tools_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join("concierge").join("tools"))
}

/// The user's Documents directory for this OS (`~/Documents`; Windows honors the
/// `USERPROFILE` known-folder, Linux honors `XDG_DOCUMENTS_DIR`). `None` if no
/// home is set.
#[must_use]
pub fn documents_dir() -> Option<PathBuf> {
    if !cfg!(windows) {
        if let Some(x) = std::env::var_os("XDG_DOCUMENTS_DIR") {
            return Some(PathBuf::from(x));
        }
    }
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|h| PathBuf::from(h).join("Documents"))
}

/// The NATIVE `Documents/My Games/<folder>` path (where a natively-installed
/// Bethesda game keeps saves/INIs). Returns `None` on OSes/runtimes whose real
/// path lives inside a Wine/Proton/CrossOver prefix — those must be declared in
/// the manifest's `[game.paths]` (the portable, per-profile mechanism).
#[must_use]
pub fn documents_my_games(folder: &str) -> Option<PathBuf> {
    documents_dir().map(|d| d.join("My Games").join(folder))
}

/// The user's home directory, cross-platform: `HOME` (Unix), else `USERPROFILE`
/// (Windows), else the current dir as a last resort. Never panics.
#[must_use]
pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
}

/// The freedesktop `.desktop` entry that makes this exe the `nxm://` handler.
/// A browser "Mod Manager Download" (free-user, one click per file — the same
/// token flow MO2/Vortex/Wabbajack use) then routes to `concierge nxm <url>`.
#[must_use]
pub fn nxm_desktop_entry(exe: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Concierge (nxm handler)\n\
         Exec={exe} nxm %u\n\
         NoDisplay=true\n\
         MimeType=x-scheme-handler/nxm;\n"
    )
}

/// Register `exe` as the OS handler for `nxm://` links — the missing link that
/// makes free-account Nexus downloads work end to end: click "Mod Manager
/// Download" on a mod page and the signed handoff routes here to be fetched +
/// hash-verified. Best-effort per OS; returns a human status line.
///
/// This is a *routing* registration only — it never downloads without a click,
/// so it stays within Nexus's AUP (no automation).
///
/// # Errors
/// Returns a message if the handler files/keys couldn't be written, or if the
/// current OS isn't wired up yet.
pub fn register_nxm_handler(exe: &std::path::Path) -> Result<String, String> {
    let exe = exe.display().to_string();
    if cfg!(target_os = "linux") {
        let apps = home_dir().join(".local").join("share").join("applications");
        std::fs::create_dir_all(&apps).map_err(|e| e.to_string())?;
        let file = apps.join("concierge-nxm.desktop");
        std::fs::write(&file, nxm_desktop_entry(&exe)).map_err(|e| e.to_string())?;
        let _ = std::process::Command::new("xdg-mime")
            .args(["default", "concierge-nxm.desktop", "x-scheme-handler/nxm"])
            .status();
        let _ = std::process::Command::new("update-desktop-database")
            .arg(&apps)
            .status();
        Ok(format!(
            "1-click downloads enabled ({}). Now click \"Mod Manager Download\" on any \
             Nexus mod page and it lands here.",
            file.display()
        ))
    } else if cfg!(windows) {
        let cmd = format!("\"{exe}\" nxm \"%1\"");
        let reg = |args: &[&str]| std::process::Command::new("reg").args(args).status();
        reg(&[
            "add",
            r"HKCU\Software\Classes\nxm",
            "/ve",
            "/d",
            "URL:nxm Protocol",
            "/f",
        ])
        .map_err(|e| e.to_string())?;
        reg(&[
            "add",
            r"HKCU\Software\Classes\nxm",
            "/v",
            "URL Protocol",
            "/d",
            "",
            "/f",
        ])
        .map_err(|e| e.to_string())?;
        reg(&[
            "add",
            r"HKCU\Software\Classes\nxm\shell\open\command",
            "/ve",
            "/d",
            &cmd,
            "/f",
        ])
        .map_err(|e| e.to_string())?;
        Ok(
            "1-click downloads enabled. Now click \"Mod Manager Download\" on any Nexus \
            mod page and it lands here."
                .to_owned(),
        )
    } else if cfg!(target_os = "macos") {
        register_nxm_macos(&exe)
    } else {
        Err("Automatic nxm:// registration isn't wired up on this OS yet.".to_owned())
    }
}

/// macOS has no per-user scheme registration for a bare binary — the established
/// trick is a tiny `AppleScript` applet with an `on open location` handler that
/// shells out to `concierge nxm <url>`. We compile one into `~/Applications`,
/// declare the `nxm` scheme in its `Info.plist`, and register it with
/// `LaunchServices`.
#[cfg(target_os = "macos")]
fn register_nxm_macos(exe: &str) -> Result<String, String> {
    use std::process::Command;
    let apps = home_dir().join("Applications");
    std::fs::create_dir_all(&apps).map_err(|e| e.to_string())?;
    let app = apps.join("Concierge nxm.app");
    let _ = std::fs::remove_dir_all(&app);

    // `on open location` receives the URL as an Apple Event — the one way a
    // scriptable handler (not a full Cocoa app) can catch a scheme.
    let script = format!(
        "on open location this_URL\n\
         \tdo shell script \"{exe} nxm \" & quoted form of this_URL\n\
         end open location\n"
    );
    let src = std::env::temp_dir().join("concierge-nxm.applescript");
    std::fs::write(&src, script).map_err(|e| e.to_string())?;
    let ok = Command::new("osacompile")
        .args(["-o".as_ref(), app.as_os_str(), src.as_os_str()])
        .status()
        .map_err(|e| e.to_string())?
        .success();
    if !ok {
        return Err("couldn't build the nxm helper app (osacompile failed)".to_owned());
    }

    // Declare the nxm scheme in the compiled app's Info.plist.
    let plist = app.join("Contents").join("Info.plist");
    let pb = |args: &[&str]| {
        Command::new("/usr/libexec/PlistBuddy")
            .arg("-c")
            .args(args)
            .arg(&plist)
            .status()
    };
    let _ = pb(&["Add :CFBundleURLTypes array"]);
    let _ = pb(&["Add :CFBundleURLTypes:0 dict"]);
    let _ = pb(&["Add :CFBundleURLTypes:0:CFBundleURLName string com.msmfai.concierge.nxm"]);
    let _ = pb(&["Add :CFBundleURLTypes:0:CFBundleURLSchemes array"]);
    let _ = pb(&["Add :CFBundleURLTypes:0:CFBundleURLSchemes:0 string nxm"]);

    // Make LaunchServices aware of it so nxm:// routes here.
    let lsregister = "/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/\
                      LaunchServices.framework/Versions/A/Support/lsregister";
    let _ = Command::new(lsregister).arg("-f").arg(&app).status();
    Ok(format!(
        "1-click downloads enabled ({}). Click \"Mod Manager Download\" on a Nexus page; \
         if macOS asks which app opens nxm links, pick Concierge nxm.",
        app.display()
    ))
}

#[cfg(not(target_os = "macos"))]
fn register_nxm_macos(_exe: &str) -> Result<String, String> {
    Err("not macOS".to_owned())
}

/// The per-user config directory, `$XDG_CONFIG_HOME/concierge` or
/// `~/.config/concierge`. Where API keys live.
#[must_use]
pub fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map_or_else(|| home_dir().join(".config"), PathBuf::from)
        .join("concierge")
}

/// Path to a named config file under [`config_dir`]. If it isn't there yet but a
/// legacy `~/.config/fo4nix/<name>` exists, return the legacy path so existing
/// keys keep working after the rename.
#[must_use]
pub fn config_file(name: &str) -> PathBuf {
    let current = config_dir().join(name);
    if current.exists() {
        return current;
    }
    let legacy = std::env::var_os("XDG_CONFIG_HOME")
        .map_or_else(|| home_dir().join(".config"), PathBuf::from)
        .join("fo4nix")
        .join(name);
    if legacy.exists() {
        return legacy;
    }
    current
}

/// The per-user data directory for this OS (`%LOCALAPPDATA%`, macOS
/// `~/Library/Application Support`, or XDG `~/.local/share`).
#[must_use]
pub fn data_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Library").join("Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share"))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{find_tool, nxm_desktop_entry, override_env, tools_dir};

    #[test]
    fn nxm_desktop_entry_routes_the_scheme_to_the_exe() {
        let d = nxm_desktop_entry("/opt/concierge/concierge");
        assert!(d.contains("MimeType=x-scheme-handler/nxm;"));
        assert!(d.contains("Exec=/opt/concierge/concierge nxm %u"));
        assert!(d.contains("[Desktop Entry]"));
    }

    #[test]
    fn override_env_is_sanitized_and_prefixed() {
        assert_eq!(override_env("clickhouse"), "CONCIERGE_CLICKHOUSE");
        assert_eq!(override_env("7zz"), "CONCIERGE_7ZZ");
        assert_eq!(override_env("some-tool"), "CONCIERGE_SOME_TOOL");
    }

    #[test]
    fn tools_dir_is_under_a_concierge_path() {
        // On any host with HOME/LOCALAPPDATA set (CI included) this resolves.
        if let Some(d) = tools_dir() {
            assert!(d.ends_with("concierge/tools") || d.ends_with("concierge\\tools"));
        }
    }

    #[test]
    fn documents_my_games_hangs_off_documents() {
        if let Some(mg) = super::documents_my_games("Fallout4") {
            assert!(mg.ends_with("My Games/Fallout4") || mg.ends_with("My Games\\Fallout4"));
        }
    }

    #[test]
    fn home_dir_is_absolute_when_home_is_set() {
        // On the test host HOME is set; the result should be a real, non-root dir.
        if std::env::var_os("HOME").is_some() {
            let h = super::home_dir();
            assert!(h.is_absolute());
            assert_ne!(h, std::path::Path::new("/"));
        }
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn config_file_prefers_concierge_then_falls_back_to_fo4nix() {
        use super::config_file;
        // Isolate HOME/XDG so we don't touch the real config.
        let tmp = std::env::temp_dir().join(format!("cg-cfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let prev_x = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", &tmp);
        // neither exists -> returns the concierge path
        assert!(config_file("k").ends_with("concierge/k"));
        // only legacy fo4nix exists -> returns legacy
        std::fs::create_dir_all(tmp.join("fo4nix")).unwrap();
        std::fs::write(tmp.join("fo4nix/k"), "x").unwrap();
        assert!(config_file("k").ends_with("fo4nix/k"));
        // once concierge has it, prefer concierge
        std::fs::create_dir_all(tmp.join("concierge")).unwrap();
        std::fs::write(tmp.join("concierge/k"), "y").unwrap();
        assert!(config_file("k").ends_with("concierge/k"));
        match prev_x {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn missing_tool_reports_the_override_env() {
        // A name that cannot exist anywhere → error naming its override env.
        let Err(err) = find_tool("definitely-not-a-real-binary-xyzzy") else {
            unreachable!("nonexistent tool must not resolve");
        };
        assert_eq!(err.env, "CONCIERGE_DEFINITELY_NOT_A_REAL_BINARY_XYZZY");
    }
}
