//! Embedded PTY terminal: the agent view IS a terminal running the user's
//! real interactive agent inside the `concierge-cli shell` sandbox. The session
//! logic here is deliberately egui-INDEPENDENT and unit-testable — spawn a
//! command in a PTY, pump its output through a vt100 parser, expose the screen
//! grid and an input sink. `main.rs` renders that grid and forwards
//! keystrokes; nothing about the agent protocol is reimplemented — permission
//! prompts, plan mode, and skills all come from the real harness.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

/// A live PTY session: a child process whose output feeds a `vt100` screen. The
/// PTY backend is portable-pty on Unix and the dedicated `conpty` crate on
/// Windows (portable-pty's `ConPTY` delivers no output there — verified in CI).
pub struct PtyTerminal {
    parser: Arc<Mutex<vt100::Parser>>,
    writer: Box<dyn Write + Send>,
    rows: u16,
    cols: u16,
    /// Bumped by the reader thread on new output so the UI knows to repaint.
    dirty: Arc<std::sync::atomic::AtomicU64>,
    #[cfg(not(windows))]
    master: Box<dyn portable_pty::MasterPty + Send>,
    #[cfg(not(windows))]
    child: Arc<Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
    #[cfg(windows)]
    proc: Arc<Mutex<conpty::Process>>,
}

impl PtyTerminal {
    /// Spawn `cmd` (argv) in a PTY of `rows`x`cols`, with env and cwd applied.
    /// When `log` is set, every byte the child prints is also appended there —
    /// so a terminal that renders blank or closes instantly can still be
    /// diagnosed from the transcript afterwards.
    #[allow(clippy::type_complexity)] // the backend tuple is documented inline
    pub fn spawn(
        cmd: &[String],
        cwd: &std::path::Path,
        env: &[(String, String)],
        rows: u16,
        cols: u16,
        log: Option<std::path::PathBuf>,
    ) -> std::io::Result<Self> {
        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 2000)));
        let dirty = Arc::new(std::sync::atomic::AtomicU64::new(0));

        #[cfg(not(windows))]
        let (reader, writer, master, child): (
            Box<dyn Read + Send>,
            Box<dyn Write + Send>,
            Box<dyn portable_pty::MasterPty + Send>,
            Arc<Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
        ) = {
            use portable_pty::{CommandBuilder, PtySize};
            let pair = portable_pty::native_pty_system()
                .openpty(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(std::io::Error::other)?;
            let mut builder = CommandBuilder::new(cmd.first().map_or("/bin/sh", |s| s));
            for a in cmd.iter().skip(1) {
                builder.arg(a);
            }
            builder.cwd(cwd);
            for (k, v) in env {
                builder.env(k, v);
            }
            let child = pair
                .slave
                .spawn_command(builder)
                .map_err(std::io::Error::other)?;
            drop(pair.slave); // we hold only the master end
            let reader = pair
                .master
                .try_clone_reader()
                .map_err(std::io::Error::other)?;
            let writer = pair.master.take_writer().map_err(std::io::Error::other)?;
            (reader, writer, pair.master, Arc::new(Mutex::new(child)))
        };

        #[cfg(windows)]
        let (reader, writer, proc): (
            Box<dyn Read + Send>,
            Box<dyn Write + Send>,
            Arc<Mutex<conpty::Process>>,
        ) = {
            let mut command =
                std::process::Command::new(cmd.first().map_or("cmd.exe", String::as_str));
            for a in cmd.iter().skip(1) {
                command.arg(a);
            }
            command.current_dir(cwd);
            // conpty passes ONLY the Command's explicit env to CreateProcess (it
            // does NOT inherit the parent environment), so seed the full current
            // environment first — otherwise PowerShell launches without
            // SystemRoot/PATH and the .NET CLR fails to load ("error 8009001d").
            command.envs(std::env::vars_os());
            for (k, v) in env {
                command.env(k, v);
            }
            let mut proc = conpty::Process::spawn(command)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            let _ = proc.resize(
                i16::try_from(cols).unwrap_or(80),
                i16::try_from(rows).unwrap_or(24),
            );
            let reader = proc
                .output()
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            let writer = proc
                .input()
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            (
                Box::new(reader),
                Box::new(writer),
                Arc::new(Mutex::new(proc)),
            )
        };

        Self::pump(reader, &parser, &dirty, log, cmd.join(" "));

        Ok(Self {
            parser,
            writer,
            rows,
            cols,
            dirty,
            #[cfg(not(windows))]
            master,
            #[cfg(not(windows))]
            child,
            #[cfg(windows)]
            proc,
        })
    }

    /// Reader thread: pump child output into the vt100 screen (+ optional
    /// transcript), bumping `dirty` so the UI repaints. Backend-agnostic.
    fn pump(
        mut reader: Box<dyn Read + Send>,
        parser: &Arc<Mutex<vt100::Parser>>,
        dirty: &Arc<std::sync::atomic::AtomicU64>,
        log: Option<std::path::PathBuf>,
        cmdline: String,
    ) {
        let p = Arc::clone(parser);
        let d = Arc::clone(dirty);
        let mut transcript = log.and_then(|path| {
            std::fs::File::create(&path)
                .map(|mut f| {
                    let _ = writeln!(f, "$ {cmdline}\n");
                    f
                })
                .ok()
        });
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                if let (Ok(mut parser), Some(chunk)) = (p.lock(), buf.get(..n)) {
                    parser.process(chunk);
                    if let (Some(f), Some(chunk)) = (transcript.as_mut(), buf.get(..n)) {
                        let _ = f.write_all(chunk);
                        let _ = f.flush();
                    }
                }
                d.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            if let Some(f) = transcript.as_mut() {
                let _ = writeln!(f, "\n[child exited]");
            }
            d.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        });
    }

    /// Forward user input bytes to the child.
    pub fn send(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    /// A monotonically-increasing counter that changes whenever new output
    /// arrived — cheap repaint trigger for the UI.
    #[must_use]
    pub fn dirty_tick(&self) -> u64 {
        self.dirty.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Resize the PTY and the parser to `rows`x`cols`.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        if rows == self.rows && cols == self.cols {
            return;
        }
        self.rows = rows;
        self.cols = cols;
        #[cfg(not(windows))]
        {
            let _ = self.master.resize(portable_pty::PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
        #[cfg(windows)]
        if let (Ok(mut p), Ok(x), Ok(y)) =
            (self.proc.lock(), i16::try_from(cols), i16::try_from(rows))
        {
            let _ = p.resize(x, y);
        }
        if let Ok(mut p) = self.parser.lock() {
            p.set_size(rows, cols);
        }
    }

    /// Snapshot the visible grid as rows of plain text (no styling) — the
    /// testable view of what the terminal shows.
    #[must_use]
    pub fn text_rows(&self) -> Vec<String> {
        self.parser.lock().map_or_else(
            |_| Vec::new(),
            |p| {
                let screen = p.screen();
                (0..self.rows)
                    .map(|r| {
                        (0..self.cols)
                            .map(|c| {
                                screen
                                    .cell(r, c)
                                    .map(vt100::Cell::contents)
                                    .filter(|s| !s.is_empty())
                                    .unwrap_or_else(|| " ".to_owned())
                            })
                            .collect::<String>()
                            .trim_end()
                            .to_owned()
                    })
                    .collect()
            },
        )
    }

    /// Has the child exited?
    #[must_use]
    pub fn finished(&self) -> bool {
        #[cfg(not(windows))]
        {
            self.child
                .lock()
                .ok()
                .and_then(|mut c| c.try_wait().ok())
                .is_some_and(|s| s.is_some())
        }
        #[cfg(windows)]
        {
            self.proc.lock().map_or(true, |p| !p.is_alive())
        }
    }

    /// Kill the child (the interrupt affordance).
    pub fn kill(&self) {
        #[cfg(not(windows))]
        if let Ok(mut c) = self.child.lock() {
            let _ = c.kill();
        }
        #[cfg(windows)]
        if let Ok(mut p) = self.proc.lock() {
            let _ = p.exit(0);
        }
    }
}

impl Drop for PtyTerminal {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait_for(term: &PtyTerminal, needle: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if term.text_rows().iter().any(|r| r.contains(needle)) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn pty_runs_a_command_and_shows_its_output() {
        let term = PtyTerminal::spawn(
            &[
                "/bin/sh".into(),
                "-c".into(),
                "printf 'concierge-terminal-ok'".into(),
            ],
            std::path::Path::new("/tmp"),
            &[],
            24,
            80,
            None,
        )
        .unwrap();
        assert!(
            wait_for(&term, "concierge-terminal-ok"),
            "grid: {:?}",
            term.text_rows()
        );
    }

    // The embedded terminal MUST render child output on Windows (a real user
    // reported an empty grid there). Reproduce it in CI: spawn cmd.exe and assert
    // its output reaches the grid. If this fails on the Windows runner, the PTY
    // backend is the culprit and we can iterate on it here, not via a user.
    #[test]
    #[cfg(windows)]
    fn pty_windows_renders_cmd_output() {
        let term = PtyTerminal::spawn(
            &[
                "cmd.exe".into(),
                "/c".into(),
                "echo CONCIERGE-WIN-MARKER".into(),
            ],
            std::path::Path::new("."),
            &[],
            24,
            80,
            None,
        )
        .unwrap();
        assert!(
            wait_for(&term, "CONCIERGE-WIN-MARKER"),
            "cmd output never reached the grid: {:?}",
            term.text_rows()
        );
    }

    #[test]
    #[cfg(windows)]
    fn pty_windows_renders_interactive_powershell() {
        // Pass env vars like the real GUI does — conpty builds the child env from
        // ONLY the explicit vars, so this reproduces the "SystemRoot missing → CLR
        // fails to load (8009001d)" bug unless the full env is seeded first.
        let term = PtyTerminal::spawn(
            &[
                "powershell.exe".into(),
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-NoExit".into(),
                "-Command".into(),
                "Write-Host CONCIERGE-PS-MARKER".into(),
            ],
            std::path::Path::new("."),
            &[("CONCIERGE_TEST".to_owned(), "1".to_owned())],
            24,
            80,
            None,
        )
        .unwrap();
        let shown = wait_for(&term, "CONCIERGE-PS-MARKER");
        let alive = !term.finished();
        term.kill();
        assert!(shown, "interactive PS output never reached the grid");
        assert!(
            alive,
            "interactive PS exited instead of staying at a prompt"
        );
    }

    // The whole goal on a real Windows runner: the INTERACTIVE sandboxed shell
    // must (1) render the MOTD, (2) survive self-lowering to Low integrity and
    // stay at a prompt, (3) accept typed input, and (4) be write-confined — a
    // write inside the write-set lands, a write outside it (the user profile,
    // Medium) is blocked by MIC.
    #[test]
    #[cfg(windows)]
    fn pty_windows_interactive_sandbox_confines_and_renders() {
        use std::time::Duration;
        concierge_games::register();
        let pid = std::process::id();
        let base = std::env::temp_dir().join(format!("cg-int-sb-{pid}"));
        let profile = base.join("games/fallout4/profiles/default");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(base.join(".concierge-workspace"), "").unwrap();
        let toml = concat!(
            "[game]\nkind = \"fallout4\"\npristine = \"C:/cg-x\"\nversion = \"1\"\n",
            "[game.paths]\nplugins_txt = \"C:/cg-x/p.txt\"\nmy_games = \"C:/cg-x/mg\"\n",
        );
        let manifest = concierge::manifest::Manifest::parse(toml).unwrap();
        let plan = concierge::plan::eval(&manifest).unwrap();
        let repo = concierge::repo::Repo::at(&profile);
        let c = concierge::shell::shell_command(&repo, &plan, None, false, &[], &[]).unwrap();
        let program: Vec<String> = std::iter::once(c.get_program())
            .chain(c.get_args())
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let cwd = c
            .get_current_dir()
            .map_or_else(|| profile.clone(), std::path::Path::to_path_buf);
        let env: Vec<(String, String)> = c
            .get_envs()
            .filter_map(|(k, v)| {
                Some((
                    k.to_string_lossy().into_owned(),
                    v?.to_string_lossy().into_owned(),
                ))
            })
            .collect();

        let mut term = PtyTerminal::spawn(&program, &cwd, &env, 40, 120, None).unwrap();
        assert!(
            wait_for(&term, "Concierge sandbox"),
            "MOTD never rendered: {:?}",
            term.text_rows()
        );
        std::thread::sleep(Duration::from_millis(800));
        assert!(
            !term.finished(),
            "interactive shell exited after DropToLow instead of staying at a prompt"
        );

        // Type a write to an allowed path (the profile, relabeled Low) and to a
        // denied path (the user-profile root, Medium). Confinement = only the
        // first lands.
        let allowed = cwd.join(format!("sb-in-{pid}.txt"));
        let denied = concierge::repo::home().join(format!("sb-out-{pid}.txt"));
        let _ = std::fs::remove_file(&allowed);
        let _ = std::fs::remove_file(&denied);
        let line = format!(
            "Set-Content -LiteralPath '{}' -Value ok -EA SilentlyContinue; \
             Set-Content -LiteralPath '{}' -Value bad -EA SilentlyContinue; \
             Write-Host SB-DONE\r\n",
            allowed.display(),
            denied.display(),
        );
        term.send(line.as_bytes());
        // SB-DONE prints only after both writes ran (same line, sequential), so
        // the file outcomes are settled once it appears. The write to the
        // Medium user-profile root can only be blocked if the shell is Low — so
        // `denied_blocked` is the confinement proof; no fragile text-scraping.
        let done = wait_for(&term, "SB-DONE");
        std::thread::sleep(Duration::from_millis(300));
        let allowed_ok = allowed.exists();
        let denied_blocked = !denied.exists();
        term.kill();
        let _ = std::fs::remove_file(&allowed);
        let _ = std::fs::remove_file(&denied);
        let _ = std::fs::remove_dir_all(&base);

        assert!(done, "typed command never executed (input not forwarded?)");
        assert!(
            allowed_ok,
            "write inside the write-set was blocked — shell unusable"
        );
        assert!(
            denied_blocked,
            "write OUTSIDE the write-set landed — the interactive shell is NOT confined"
        );
    }

    #[test]
    fn transcript_captures_child_output() {
        // The diagnostic transcript must hold what the child printed, so a blank
        // terminal on a machine we can't see is still readable after the fact.
        let log = std::env::temp_dir().join(format!("cg-term-log-{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&log);
        let term = PtyTerminal::spawn(
            &[
                "/bin/sh".into(),
                "-c".into(),
                "printf 'transcript-marker'".into(),
            ],
            std::path::Path::new("/tmp"),
            &[],
            24,
            80,
            Some(log.clone()),
        )
        .unwrap();
        assert!(wait_for(&term, "transcript-marker"), "grid missing marker");
        std::thread::sleep(Duration::from_millis(150));
        let contents = std::fs::read_to_string(&log).unwrap_or_default();
        assert!(
            contents.contains("transcript-marker"),
            "transcript did not capture output: {contents:?}"
        );
        let _ = std::fs::remove_file(&log);
    }

    #[test]
    fn pty_forwards_typed_input() {
        // `cat` echoes stdin back through the pty; typing must reach the grid.
        let mut term = PtyTerminal::spawn(
            &["/bin/cat".into()],
            std::path::Path::new("/tmp"),
            &[],
            24,
            80,
            None,
        )
        .unwrap();
        term.send(b"hello-from-keystrokes\n");
        assert!(
            wait_for(&term, "hello-from-keystrokes"),
            "grid: {:?}",
            term.text_rows()
        );
        term.kill();
    }

    #[test]
    fn env_and_cwd_reach_the_child() {
        // Canonicalized so $PWD matches verbatim (/tmp is a symlink to
        // /private/tmp on macOS). Short path so `pwd` doesn't wrap at 80 cols.
        let canon = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("cg-term-{}", std::process::id()));
        std::fs::create_dir_all(&canon).unwrap();
        let term = PtyTerminal::spawn(
            &[
                "/bin/sh".into(),
                "-c".into(),
                "echo SBX=$CONCIERGE_SANDBOX; pwd".into(),
            ],
            &canon,
            &[("CONCIERGE_SANDBOX".into(), "1".into())],
            24,
            80,
            None,
        )
        .unwrap();
        assert!(wait_for(&term, "SBX=1"), "env: {:?}", term.text_rows());
        let leaf = canon.file_name().unwrap().to_string_lossy().into_owned();
        assert!(wait_for(&term, &leaf), "cwd: {:?}", term.text_rows());
        let _ = std::fs::remove_dir_all(&canon);
    }
}
