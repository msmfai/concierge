//! The dendritic module system's metamorphic + domain laws are enforced BY THE
//! NIX INTERPRETER: `mkDendritic` asserts them, so a violating tree fails to
//! evaluate. These tests pin that contract. Gated on a Nix evaluator.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::process::Command;

const DENDRITIC: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../nix/dendritic.nix");
const LIB: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../nix/lib.nix");

fn nix_present() -> bool {
    Command::new("nix")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn nix_eval(expr: &str) -> std::process::Output {
    Command::new("nix")
        .args(["eval", "--impure", "--json", "--expr", expr])
        .output()
        .expect("spawn nix")
}

#[test]
fn interpreter_computes_all_laws_true_for_a_valid_tree() {
    if !nix_present() {
        eprintln!("nix not present; skipping");
        return;
    }
    let expr = format!(
        r#"let cl = import {LIB}; d = import {DENDRITIC}; in
        d.laws {{ game = {{}}; modules = [
          {{ mods = [ (cl.mkMod {{ name="a"; version="1"; source=cl.github "u/a@v1"; }}) ]; }}
          {{ mods = [ (cl.mkMod {{ name="b"; version="1"; source=cl.github "u/b@v1"; }}) ]; }}
        ]; }}"#
    );
    let out = nix_eval(&expr);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json = String::from_utf8_lossy(&out.stdout);
    for law in ["orderIndependentSet", "identity", "uniqueNames"] {
        assert!(
            json.contains(&format!("\"{law}\":true")),
            "law {law} not true in {json}"
        );
    }
}

#[test]
fn interpreter_rejects_a_law_violating_tree() {
    if !nix_present() {
        eprintln!("nix not present; skipping");
        return;
    }
    // two modules claim the same mod name -> uniqueNames law fails -> eval error
    let expr = format!(
        r#"let cl = import {LIB}; d = import {DENDRITIC}; in
        d.mkDendritic {{ game = {{}}; modules = [
          {{ mods = [ (cl.mkMod {{ name="dup"; version="1"; source=cl.github "u/a@v1"; }}) ]; }}
          {{ mods = [ (cl.mkMod {{ name="dup"; version="2"; source=cl.github "u/b@v1"; }}) ]; }}
        ]; }}"#
    );
    let out = nix_eval(&expr);
    assert!(
        !out.status.success(),
        "a duplicate-name tree must fail to evaluate"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("uniqueNames"),
        "expected uniqueNames assertion, got: {err}"
    );
}
