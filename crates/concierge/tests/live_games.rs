//! Live-games suite: runs the REAL binary against the machine's actual game
//! installs and proves the pristine copies stay untouched. Opt-in and
//! machine-bound — gated behind `CONCIERGE_LIVE=1` so plain `cargo test`
//! stays hermetic; games whose pristine path doesn't resolve are skipped.
//!
//! Setup (once per game, hashes the whole install):
//!   `CONCIERGE_LIVE=1 cargo test -p concierge --test live_games -- \
//!       --ignored setup_vanilla_inventories --nocapture`
//!
//! Run:
//!   `CONCIERGE_LIVE=1 cargo test -p concierge --test live_games -- --nocapture`
//!
//! Per installed game the suite: snapshots the pristine tree (path/size/mtime),
//! `eval`s every profile, asserts the headless health check is GO, verifies
//! the pristine against the committed vanilla inventory (existence + size;
//! full md5 re-hash with `CONCIERGE_LIVE_FULL=1` via `check --vanilla`), runs
//! drift `check` for every profile, then re-snapshots and asserts the pristine
//! is bit-for-bit identical — the suite itself never wrote a byte there.
//! Filter games with `CONCIERGE_LIVE_GAMES="bg3,kotor2"`.
//!
//! For the mutating tier (real fetch→realize→undeploy round-trips against
//! disposable targets) see `live_mutate.rs`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use std::path::Path;

use common::{concierge, game_dirs, pristine_of, profiles, snapshot, snapshot_diff};

fn live() -> bool {
    std::env::var("CONCIERGE_LIVE").as_deref() == Ok("1")
}

/// Inventory quick sweep: every recorded file exists with the recorded size.
/// (Content hashes are re-verified by `check --vanilla` in FULL mode.)
fn quick_sweep(inventory: &Path, pristine: &Path) -> Vec<String> {
    let mut bad = Vec::new();
    let text = std::fs::read_to_string(inventory).expect("read inventory");
    for line in text.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(_md5), Some(size), Some(rel)) = (parts.next(), parts.next(), parts.next()) else {
            bad.push(format!("bad inventory line: {line}"));
            continue;
        };
        let p = pristine.join(rel);
        match std::fs::metadata(&p) {
            Err(_) => bad.push(format!("pristine file missing: {rel}")),
            Ok(m) if m.len().to_string() != size => bad.push(format!(
                "pristine file resized: {rel} ({} != {size})",
                m.len()
            )),
            Ok(_) => {}
        }
    }
    bad
}

/// One-time setup: hash each installed game's pristine into its
/// vanilla-inventory.tsv (skips games that already have one). Ignored by
/// default — it reads hundreds of GB; run it deliberately.
#[test]
#[ignore = "one-time setup: hashes entire game installs; run with --ignored"]
fn setup_vanilla_inventories() {
    if !live() {
        println!("skipped (set CONCIERGE_LIVE=1)");
        return;
    }
    for game_dir in game_dirs() {
        let name = game_dir.file_name().unwrap().to_string_lossy().into_owned();
        let Some(pristine) = pristine_of(&game_dir) else {
            println!("{name}: no loadable profile — skipped");
            continue;
        };
        if !pristine.is_dir() {
            println!(
                "{name}: pristine not installed at {} — skipped",
                pristine.display()
            );
            continue;
        }
        if game_dir.join("vanilla-inventory.tsv").exists() {
            println!("{name}: inventory already present — kept (re-bless via `concierge inventory --force`)");
            continue;
        }
        let profile = profiles(&game_dir).into_iter().next().unwrap();
        println!("{name}: hashing {} ...", pristine.display());
        let (ok, out) = concierge(&profile, &["inventory"], None);
        assert!(ok, "{name}: inventory failed:\n{out}");
        print!("{out}");
    }
}

/// The suite: everything read-only works against the real installs, and the
/// pristine copies come out bit-for-bit untouched.
#[test]
fn live_games_work_with_untouched_copies() {
    if !live() {
        println!("skipped (set CONCIERGE_LIVE=1)");
        return;
    }
    let full = std::env::var("CONCIERGE_LIVE_FULL").as_deref() == Ok("1");
    let mut failures: Vec<String> = Vec::new();
    let mut covered = 0usize;

    for game_dir in game_dirs() {
        let name = game_dir.file_name().unwrap().to_string_lossy().into_owned();
        let Some(pristine) = pristine_of(&game_dir) else {
            failures.push(format!("{name}: first profile's manifest failed to load"));
            continue;
        };
        if !pristine.is_dir() {
            println!(
                "{name}: pristine not installed at {} — skipped",
                pristine.display()
            );
            continue;
        }
        covered += 1;
        println!("{name}: pristine {}", pristine.display());
        let before = snapshot(&pristine);
        assert!(!before.is_empty(), "{name}: empty pristine snapshot");

        for profile in profiles(&game_dir) {
            let pname = format!("{name}/{}", profile.file_name().unwrap().to_string_lossy());

            let (ok, out) = concierge(&profile, &["eval"], None);
            if ok {
                println!("  {pname}: eval ok");
            } else {
                failures.push(format!("{pname}: eval failed:\n{out}"));
                continue; // downstream steps need a plan
            }

            let (ok, out) = concierge(
                &profile,
                &["tui", "--live", "--script", "-"],
                Some("assert health go\n"),
            );
            if ok {
                println!("  {pname}: health GO");
            } else {
                failures.push(format!("{pname}: health NO-GO:\n{out}"));
            }

            let (ok, out) = concierge(&profile, &["check"], None);
            if ok {
                println!("  {pname}: check clean");
            } else {
                failures.push(format!("{pname}: drift check failed:\n{out}"));
            }
        }

        // Pristine vs the committed baseline.
        let inventory = game_dir.join("vanilla-inventory.tsv");
        if inventory.exists() {
            let bad = quick_sweep(&inventory, &pristine);
            if bad.is_empty() {
                println!("  {name}: inventory quick sweep ok");
            } else {
                failures.extend(bad.into_iter().map(|b| format!("{name}: {b}")));
            }
            if full {
                let profile = profiles(&game_dir).into_iter().next().unwrap();
                let (ok, out) = concierge(&profile, &["check", "--vanilla"], None);
                if ok {
                    println!("  {name}: full vanilla re-hash ok");
                } else {
                    failures.push(format!("{name}: check --vanilla failed:\n{out}"));
                }
            }
        } else {
            failures.push(format!(
                "{name}: no vanilla-inventory.tsv — run the setup test:\n  \
                 CONCIERGE_LIVE=1 cargo test -p concierge --test live_games -- \
                 --ignored setup_vanilla_inventories --nocapture"
            ));
        }

        // The whole suite must not have written a byte into the pristine.
        let after = snapshot(&pristine);
        if before == after {
            println!("  {name}: pristine untouched ({} files)", after.len());
        } else {
            failures.push(format!(
                "{name}: PRISTINE TOUCHED: {}",
                snapshot_diff(&before, &after).join(", ")
            ));
        }
    }

    assert!(covered > 0, "no game installs found on this machine");
    assert!(
        failures.is_empty(),
        "live suite failures:\n{}",
        failures.join("\n")
    );
}
