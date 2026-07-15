//! Thin wrapper around `clickhouse local`: serverless, data persists under a
//! directory. Every call is a fresh process; `ClickHouse`'s own storage layer
//! handles durability and merges.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{Error, Result};

#[derive(Debug, Clone)]
pub struct Db {
    path: PathBuf,
    /// The resolved `clickhouse` binary (found without Nix at open time).
    bin: PathBuf,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let bin = concierge_platform::find_tool("clickhouse")
            .map_err(|e| Error::NoClickHouse(e.to_string()))?;
        std::fs::create_dir_all(path)?;
        let db = Self {
            path: path.to_path_buf(),
            bin,
        };
        for ddl in crate::schema::DDL {
            db.exec(ddl)?;
        }
        for m in crate::schema::MIGRATIONS {
            db.exec(m)?;
        }
        Ok(db)
    }

    /// Open for **read-only** queries: resolve the binary, but do NOT run the
    /// DDL or migrations. Those `CREATE`/`ALTER … IF NOT EXISTS` statements are
    /// only needed before a write (sync); running them on every read mutates the
    /// stored schema and, on some `ClickHouse` versions, disturbs a
    /// `ReplacingMergeTree`'s `FINAL` visibility so a following `SELECT` returns
    /// nothing. Searches use this; `open` (with schema setup) is for sync.
    pub fn open_ro(path: &Path) -> Result<Self> {
        let bin = concierge_platform::find_tool("clickhouse")
            .map_err(|e| Error::NoClickHouse(e.to_string()))?;
        Ok(Self {
            path: path.to_path_buf(),
            bin,
        })
    }

    fn command(&self) -> Command {
        let mut c = Command::new(&self.bin);
        c.arg("local").arg("--path").arg(&self.path);
        c
    }

    /// Run a statement, discarding output.
    pub fn exec(&self, sql: &str) -> Result<()> {
        self.run(sql, "TSV", None).map(|_| ())
    }

    /// Run a query, returning raw output in the given format.
    pub fn query(&self, sql: &str, format: &str) -> Result<String> {
        self.run(sql, format, None)
    }

    /// Insert `JSONEachRow` lines into a table.
    pub fn insert_json_rows(&self, table: &str, rows: &str) -> Result<usize> {
        if rows.is_empty() {
            return Ok(0);
        }
        let n = rows.lines().count();
        self.run(
            &format!("INSERT INTO {table} FORMAT JSONEachRow"),
            "TSV",
            Some(rows),
        )?;
        Ok(n)
    }

    fn run(&self, sql: &str, format: &str, stdin: Option<&str>) -> Result<String> {
        // clickhouse-local holds an exclusive dir lock per process; retry
        // briefly so queries during a sync don't fail outright.
        let mut delay = std::time::Duration::from_millis(250);
        for _ in 0..6 {
            match self.run_once(sql, format, stdin) {
                Err(Error::ClickHouse(msg)) if msg.contains("CANNOT_OPEN_FILE") => {
                    std::thread::sleep(delay);
                    delay *= 2;
                }
                other => return other,
            }
        }
        self.run_once(sql, format, stdin)
    }

    fn run_once(&self, sql: &str, format: &str, stdin: Option<&str>) -> Result<String> {
        let mut cmd = self.command();
        cmd.arg("--query")
            .arg(sql)
            .arg("--format")
            .arg(format)
            .arg("--date_time_input_format")
            .arg("best_effort")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            });
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::NoClickHouse(format!(
                    "clickhouse binary vanished at {}: {e}",
                    self.bin.display()
                ))
            } else {
                Error::Io(e)
            }
        })?;
        if let (Some(data), Some(mut pipe)) = (stdin, child.stdin.take()) {
            pipe.write_all(data.as_bytes())?;
            drop(pipe);
        }
        let out = child.wait_with_output()?;
        if !out.status.success() {
            return Err(Error::ClickHouse(
                String::from_utf8_lossy(&out.stderr).into_owned(),
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}
