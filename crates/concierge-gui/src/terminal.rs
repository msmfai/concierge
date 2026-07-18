//! Embedded PTY terminal: the agent view IS a terminal running the user's
//! real interactive agent inside the `concierge shell` sandbox. The session
//! logic here is deliberately egui-INDEPENDENT and unit-testable — spawn a
//! command in a PTY, pump its output through a vt100 parser, expose the screen
//! grid and an input sink. `main.rs` renders that grid and forwards
//! keystrokes; nothing about the agent protocol is reimplemented — permission
//! prompts, plan mode, and skills all come from the real harness.

use std::io::{Read as _, Write};
use std::sync::{Arc, Mutex};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize};

/// A live PTY session: a child process whose output feeds a vt100 screen.
pub struct PtyTerminal {
    parser: Arc<Mutex<vt100::Parser>>,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    rows: u16,
    cols: u16,
    /// Bumped by the reader thread on new output so the UI knows to repaint.
    dirty: Arc<std::sync::atomic::AtomicU64>,
}

impl PtyTerminal {
    /// Spawn `cmd` (argv) in a PTY of `rows`x`cols`, with env and cwd applied.
    /// When `log` is set, every byte the child prints is also appended there —
    /// so a terminal that renders blank or closes instantly can still be
    /// diagnosed from the transcript afterwards.
    pub fn spawn(
        cmd: &[String],
        cwd: &std::path::Path,
        env: &[(String, String)],
        rows: u16,
        cols: u16,
        log: Option<std::path::PathBuf>,
    ) -> std::io::Result<Self> {
        let pty = portable_pty::native_pty_system();
        let pair = pty
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

        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 2000)));
        let writer = pair.master.take_writer().map_err(std::io::Error::other)?;
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(std::io::Error::other)?;
        let dirty = Arc::new(std::sync::atomic::AtomicU64::new(0));

        let p = Arc::clone(&parser);
        let d = Arc::clone(&dirty);
        // Optional transcript: truncate on open so it holds only this session.
        let mut transcript = log.and_then(|path| {
            std::fs::File::create(&path)
                .map(|mut f| {
                    let _ = writeln!(f, "$ {}\n", cmd.join(" "));
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

        Ok(Self {
            parser,
            writer,
            master: pair.master,
            child: Arc::new(Mutex::new(child)),
            rows,
            cols,
            dirty,
        })
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
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
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
    pub fn finished(&self) -> bool {
        self.child
            .lock()
            .ok()
            .and_then(|mut c| c.try_wait().ok())
            .is_some_and(|s| s.is_some())
    }

    /// Kill the child (the interrupt affordance).
    pub fn kill(&self) {
        if let Ok(mut c) = self.child.lock() {
            let _ = c.kill();
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
