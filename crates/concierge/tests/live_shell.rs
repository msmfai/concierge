//! Live escape-attempt test for the sandboxed agent shell.
//! Gated behind `CONCIERGE_LIVE=1`. Runs the real binary's `shell` against a
//! real profile and proves the OS boundary: writes inside the plan's write-set
//! succeed; writes to the pristine install and to $HOME fail with EPERM; the
//! pristine is bit-for-bit untouched afterward.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use common::{concierge, game_dirs, pristine_of, profiles, snapshot};

#[test]
fn sandbox_confines_writes_to_the_plan() {
    if std::env::var("CONCIERGE_LIVE").as_deref() != Ok("1") {
        println!("skipped (set CONCIERGE_LIVE=1)");
        return;
    }
    if !cfg!(target_os = "macos") && !cfg!(target_os = "linux") {
        println!("no sandbox backend on this platform — skipped");
        return;
    }
    // Any installed game will do; take the first.
    let Some((game_dir, pristine)) = game_dirs()
        .into_iter()
        .filter_map(|g| pristine_of(&g).map(|p| (g, p)))
        .find(|(_, p)| p.is_dir())
    else {
        println!("no game installs found — skipped");
        return;
    };
    let profile = profiles(&game_dir).into_iter().next().unwrap();
    let name = game_dir.file_name().unwrap().to_string_lossy().into_owned();
    println!("{name}: probing sandbox against {}", pristine.display());
    let before = snapshot(&pristine);

    let probe = profile.join("state/sandbox-probe.txt");
    let home_escape = concierge::repo::home().join("concierge-sandbox-escape.txt");
    let _ = std::fs::remove_file(&probe);
    let _ = std::fs::remove_file(&home_escape);
    let script = format!(
        "echo inside > '{probe}' && echo PROFILE-WRITE-OK; \
         echo x > '{pristine}/concierge-escape.txt' 2>/dev/null && echo PRISTINE-LEAKED || echo PRISTINE-BLOCKED; \
         echo x > '{home}' 2>/dev/null && echo HOME-LEAKED || echo HOME-BLOCKED; \
         ls '{pristine}' >/dev/null 2>&1 && echo PRISTINE-READ-OK || echo PRISTINE-READ-BLOCKED",
        probe = probe.display(),
        pristine = pristine.display(),
        home = home_escape.display(),
    );
    let (ok, out) = concierge(&profile, &["shell", "/bin/sh", "-c", &script], None);
    println!("{out}");
    assert!(ok, "sandboxed shell run failed:\n{out}");
    assert!(
        out.contains("PROFILE-WRITE-OK"),
        "plan write-set must be writable:\n{out}"
    );
    assert!(
        out.contains("PRISTINE-BLOCKED"),
        "pristine write must fail:\n{out}"
    );
    assert!(
        out.contains("HOME-BLOCKED"),
        "$HOME write must fail:\n{out}"
    );
    assert!(!out.contains("LEAKED"), "sandbox leaked:\n{out}");

    assert!(probe.exists(), "the allowed write really happened");
    let _ = std::fs::remove_file(&probe);
    assert!(!home_escape.exists(), "$HOME escape file must not exist");
    assert_eq!(before, snapshot(&pristine), "{name}: pristine changed");
}

/// A Locked profile refuses edits everywhere — realize refuses, and
/// inside the sandbox the manifest can't be written even with the chmod's
/// permissions notwithstanding (the write-set drops it).
#[test]
fn locked_profile_refuses_edits_everywhere() {
    if std::env::var("CONCIERGE_LIVE").as_deref() != Ok("1") {
        println!("skipped (set CONCIERGE_LIVE=1)");
        return;
    }
    if !cfg!(target_os = "macos") && !cfg!(target_os = "linux") {
        println!("no sandbox backend on this platform — skipped");
        return;
    }
    let Some((game_dir, pristine)) = game_dirs()
        .into_iter()
        .filter_map(|g| pristine_of(&g).map(|p| (g, p)))
        .find(|(_, p)| p.is_dir())
    else {
        println!("no game installs found — skipped");
        return;
    };
    let profile = profiles(&game_dir).into_iter().next().unwrap();
    let manifest = profile.join("manifest.toml");
    let original = std::fs::read_to_string(&manifest).unwrap();

    let (ok, out) = concierge(&profile, &["lock"], None);
    assert!(ok, "lock failed:\n{out}");
    // realize refuses politely.
    let (ok, out) = concierge(&profile, &["realize"], None);
    assert!(
        !ok && out.contains("LOCKED"),
        "realize must refuse on locked:\n{out}"
    );
    // Inside the sandbox: appending and rename-over must both fail.
    let script = format!(
        "echo x >> '{m}' 2>/dev/null && echo APPEND-LEAKED || echo APPEND-BLOCKED; \
         echo y > /tmp/concierge-lock-probe && mv /tmp/concierge-lock-probe '{m}' 2>/dev/null \
         && echo RENAME-LEAKED || echo RENAME-BLOCKED",
        m = manifest.display()
    );
    let (ok, out) = concierge(&profile, &["shell", "/bin/sh", "-c", &script], None);
    assert!(ok, "sandboxed probe failed:\n{out}");
    assert!(
        out.contains("APPEND-BLOCKED") && out.contains("RENAME-BLOCKED"),
        "leak:\n{out}"
    );

    let (ok, out) = concierge(&profile, &["unlock"], None);
    assert!(ok, "unlock failed:\n{out}");
    assert_eq!(
        std::fs::read_to_string(&manifest).unwrap(),
        original,
        "manifest survived intact"
    );
    // Editable again after unlock.
    std::fs::write(&manifest, &original).unwrap();
    let _ = pristine; // pristine untouched is covered by the sibling test
}
