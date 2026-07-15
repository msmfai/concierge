//! Declarative acquisition pipelines — the escape hatch for mods that aren't on
//! Nexus. A pipeline is an ordered list of typed steps that produces a single
//! archive file; that file is content-addressed (md5) and cached in the shared
//! store exactly like a Nexus/http download, so the eval→realize model and the
//! plan hash are unchanged: the PLAN holds the pipeline *description* (pure,
//! hashed), and realize *runs* it and TOFU-pins the output.
//!
//! Verbs (all deterministic or TOFU-pinned): `http` GETs a URL to a file
//! (curl-equivalent, optional headers); `git` clones `url@ref` and `git
//! archive`s it to a reproducible tarball; `extract` unpacks the current file
//! into a tree (bsdtar); `pick` descends into a subdirectory; `run` is an impure,
//! gated escape hatch (an argv) honored only for hand-written manifests, never
//! for AI-authored pipelines. An `extract`/`pick` chain ending in a tree is
//! re-archived with a deterministic tar (sorted paths, zeroed metadata) so
//! re-runs hash the same.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::error::{Error, IoCtx, Result};

/// One pipeline step. Exactly one verb field is set (validated on run).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Step {
    #[serde(default)]
    pub http: Option<String>,
    /// Extra request headers as `"Key: Value"` lines.
    #[serde(default)]
    pub headers: Vec<String>,
    /// `url@ref` (ref = tag/branch); `@ref` omitted = default branch.
    #[serde(default)]
    pub git: Option<String>,
    #[serde(default)]
    pub extract: Option<bool>,
    #[serde(default)]
    pub pick: Option<String>,
    /// Impure escape hatch: an argv to run in the work dir. Gated.
    #[serde(default)]
    pub run: Option<Vec<String>>,
}

impl Step {
    /// Does this pipeline use the impure `run` verb anywhere?
    #[must_use]
    pub const fn is_impure(&self) -> bool {
        self.run.is_some()
    }
}

/// The artifact threaded between steps.
enum Artifact {
    File(PathBuf),
    Dir(PathBuf),
}

/// Run `steps` in a fresh working area under `work_root`, returning the final
/// archive file. `allow_run` gates the impure `run` verb (false for
/// AI-authored pipelines). The output file name is `out_name`.
pub fn run(steps: &[Step], work_root: &Path, out_name: &str, allow_run: bool) -> Result<PathBuf> {
    std::fs::create_dir_all(work_root).ctx(work_root)?;
    let mut current: Option<Artifact> = None;

    for (i, step) in steps.iter().enumerate() {
        let stage = work_root.join(format!("s{i}"));
        current = Some(exec_step(step, current, &stage, i, allow_run)?);
    }

    let final_artifact = current.ok_or_else(|| Error::Other("empty pipeline".into()))?;
    let dest = work_root.join(out_name);
    match final_artifact {
        Artifact::File(f) => {
            if f != dest {
                std::fs::rename(&f, &dest)
                    .or_else(|_| std::fs::copy(&f, &dest).map(|_| ()))
                    .ctx(&dest)?;
            }
        }
        Artifact::Dir(d) => deterministic_tar(&d, &dest)?,
    }
    Ok(dest)
}

fn exec_step(
    step: &Step,
    prev: Option<Artifact>,
    stage: &Path,
    idx: usize,
    allow_run: bool,
) -> Result<Artifact> {
    let bad = |m: String| Error::Other(format!("pipeline step {idx}: {m}"));
    match step {
        Step {
            http: Some(url),
            headers,
            ..
        } => Ok(Artifact::File(http_get(url, headers, stage)?)),
        Step {
            git: Some(spec), ..
        } => Ok(Artifact::File(git_archive(spec, stage)?)),
        Step {
            extract: Some(true),
            ..
        } => {
            let Some(Artifact::File(f)) = prev else {
                return Err(bad("extract needs a file from the previous step".into()));
            };
            Ok(Artifact::Dir(extract(&f, stage)?))
        }
        Step {
            pick: Some(sub), ..
        } => {
            let Some(Artifact::Dir(d)) = prev else {
                return Err(bad("pick needs a directory from the previous step".into()));
            };
            let picked = d.join(sub);
            if !picked.is_dir() {
                return Err(bad(format!("pick: '{sub}' is not a directory in the tree")));
            }
            Ok(Artifact::Dir(picked))
        }
        Step {
            run: Some(argv), ..
        } => {
            if !allow_run {
                return Err(bad(
                    "`run` is not permitted here (AI-authored / untrusted pipeline)".into(),
                ));
            }
            run_command(argv, stage)?;
            Ok(Artifact::Dir(stage.to_path_buf()))
        }
        _ => Err(bad(
            "no verb set (need one of http/git/extract/pick/run)".into()
        )),
    }
}

fn http_get(url: &str, headers: &[String], stage: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(stage).ctx(stage)?;
    let name = url
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("download");
    let out = stage.join(name);
    eprintln!("  pipeline  http {}", url.split('?').next().unwrap_or(url));
    let mut req = ureq::get(url).set("User-Agent", "concierge-prototype/0.1");
    for h in headers {
        if let Some((k, v)) = h.split_once(':') {
            req = req.set(k.trim(), v.trim());
        }
    }
    let resp = req.call().map_err(|e| Error::Other(format!("http: {e}")))?;
    let mut buf = Vec::new();
    resp.into_reader().read_to_end(&mut buf).ctx(&out)?;
    std::fs::write(&out, &buf).ctx(&out)?;
    Ok(out)
}

/// Clone `url@ref` shallowly and `git archive` it to a reproducible tarball.
fn git_archive(spec: &str, stage: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(stage).ctx(stage)?;
    let (url, git_ref) = spec
        .rsplit_once('@')
        .map_or((spec, None), |(u, r)| (u, Some(r)));
    let checkout = stage.join("repo");
    eprintln!(
        "  pipeline  git clone {url}{}",
        git_ref.map(|r| format!(" @ {r}")).unwrap_or_default()
    );
    let mut clone = Command::new("git");
    clone.args(["clone", "--depth", "1"]);
    if let Some(r) = git_ref {
        clone.args(["--branch", r]);
    }
    clone.arg(url).arg(&checkout);
    run_ok(&mut clone, "git clone")?;
    // git archive HEAD -> deterministic tar (no mtimes, no .git)
    let tar = stage.join("repo.tar");
    let out = std::fs::File::create(&tar).ctx(&tar)?;
    let status = Command::new("git")
        .args(["-C"])
        .arg(&checkout)
        .args(["archive", "--format=tar", "HEAD"])
        .stdout(std::process::Stdio::from(out))
        .status()
        .map_err(|e| Error::Other(format!("git archive: {e}")))?;
    if !status.success() {
        return Err(Error::Other("git archive failed".into()));
    }
    Ok(tar)
}

fn extract(archive: &Path, stage: &Path) -> Result<PathBuf> {
    let out = stage.join("tree");
    concierge_platform::extract_archive(archive, &out)
        .map_err(|e| Error::Other(format!("extract {}: {e}", archive.display())))?;
    Ok(out)
}

fn run_command(argv: &[String], stage: &Path) -> Result<()> {
    std::fs::create_dir_all(stage).ctx(stage)?;
    let (program, rest) = argv
        .split_first()
        .ok_or_else(|| Error::Other("empty run argv".into()))?;
    let mut cmd = Command::new(program);
    cmd.args(rest).current_dir(stage);
    run_ok(&mut cmd, "run")
}

fn run_ok(cmd: &mut Command, what: &str) -> Result<()> {
    let status = cmd
        .status()
        .map_err(|e| Error::Other(format!("{what}: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::Other(format!("{what} failed ({status})")))
    }
}

/// Write a byte-deterministic uncompressed tar of `dir` (POSIX ustar; sorted
/// paths, zeroed mtime/uid/gid/mode-noise) so identical trees hash identically.
pub(crate) fn deterministic_tar(dir: &Path, dest: &Path) -> Result<()> {
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    collect(dir, dir, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = std::fs::File::create(dest).ctx(dest)?;
    for (rel, path) in &files {
        let data = std::fs::read(path).ctx(path)?;
        out.write_all(&ustar_header(rel, data.len())).ctx(dest)?;
        out.write_all(&data).ctx(dest)?;
        let pad = (512 - (data.len() % 512)) % 512;
        out.write_all(&vec![0u8; pad]).ctx(dest)?;
    }
    out.write_all(&[0u8; 1024]).ctx(dest)?; // two zero blocks = EOF
    Ok(())
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<()> {
    for e in std::fs::read_dir(dir).ctx(dir)?.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(root, &p, out)?;
        } else if let Ok(rel) = p.strip_prefix(root) {
            out.push((rel.to_string_lossy().replace('\\', "/"), p.clone()));
        }
    }
    Ok(())
}

fn ustar_header(name: &str, size: usize) -> [u8; 512] {
    let mut h = [0u8; 512];
    let nb = name.as_bytes();
    let n = nb.len().min(100);
    h.get_mut(..n)
        .unwrap_or_default()
        .copy_from_slice(nb.get(..n).unwrap_or_default());
    // mode 0644, uid/gid 0, mtime 0 (all octal, NUL/space-terminated)
    write_octal(&mut h, 100, 8, 0o644);
    write_octal(&mut h, 108, 8, 0);
    write_octal(&mut h, 116, 8, 0);
    write_octal(&mut h, 124, 12, u64::try_from(size).unwrap_or(0));
    write_octal(&mut h, 136, 12, 0); // mtime
    if let Some(t) = h.get_mut(156) {
        *t = b'0'; // typeflag: regular file
    }
    if let Some(m) = h.get_mut(257..263) {
        m.copy_from_slice(b"ustar\0");
    }
    // checksum: spaces during computation, then octal sum
    if let Some(c) = h.get_mut(148..156) {
        c.fill(b' ');
    }
    let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
    write_octal(&mut h, 148, 7, u64::from(sum));
    if let Some(b) = h.get_mut(155) {
        *b = 0;
    }
    h
}

fn write_octal(h: &mut [u8; 512], off: usize, width: usize, val: u64) {
    let s = format!("{val:0>width$o}", width = width.saturating_sub(1));
    let bytes = s.as_bytes();
    if let Some(slot) = h.get_mut(off..off + bytes.len().min(width.saturating_sub(1))) {
        let n = slot.len();
        slot.copy_from_slice(bytes.get(..n).unwrap_or_default());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;

    fn write_tree(root: &Path, files: &[(&str, &[u8])]) {
        for (rel, data) in files {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, data).unwrap();
        }
    }

    #[test]
    fn deterministic_tar_is_byte_stable_and_order_independent() {
        let base = std::env::temp_dir().join(format!("cc-pipe-{}", std::process::id()));
        let a = base.join("a");
        let b = base.join("b");
        // same content, written in a different order + nesting
        write_tree(
            &a,
            &[("z.txt", b"zed"), ("d/a.txt", b"aaa"), ("m.txt", b"emm")],
        );
        write_tree(
            &b,
            &[("m.txt", b"emm"), ("d/a.txt", b"aaa"), ("z.txt", b"zed")],
        );
        let ta = base.join("a.tar");
        let tb = base.join("b.tar");
        deterministic_tar(&a, &ta).unwrap();
        deterministic_tar(&b, &tb).unwrap();
        let (ba, bb) = (std::fs::read(&ta).unwrap(), std::fs::read(&tb).unwrap());
        assert_eq!(ba, bb, "identical trees must tar to identical bytes");
        // idempotent: re-tarring yields the same bytes
        deterministic_tar(&a, &ta).unwrap();
        assert_eq!(std::fs::read(&ta).unwrap(), ba);
        // and it is a valid tar the system tool can read
        let listed = Command::new("tar").arg("tf").arg(&ta).output().unwrap();
        assert!(listed.status.success());
        let names = String::from_utf8_lossy(&listed.stdout);
        assert!(names.contains("d/a.txt") && names.contains("z.txt"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn ai_authored_pipeline_cannot_run_shell() {
        let step = Step {
            run: Some(vec!["echo".into(), "hi".into()]),
            ..blank()
        };
        let work = std::env::temp_dir().join(format!("cc-pipe-run-{}", std::process::id()));
        // allow_run = false (the AI path) must refuse
        let err = run(&[step], &work, "out.tar", false).unwrap_err();
        assert!(err.to_string().contains("not permitted"));
        let _ = std::fs::remove_dir_all(&work);
    }

    fn blank() -> Step {
        Step {
            http: None,
            headers: vec![],
            git: None,
            extract: None,
            pick: None,
            run: None,
        }
    }
}
