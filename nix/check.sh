#!/usr/bin/env bash
# Enforce the dendritic Nix style: linters + interpreter-evaluated metamorphic
# asserts. One command; non-zero exit on any violation.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
echo "==> statix (anti-patterns)";   statix check "$here"
echo "==> deadnix (dead code)";      deadnix --fail "$here" >/dev/null
echo "==> alejandra (format check)"; alejandra --check "$here" >/dev/null 2>&1
echo "==> metamorphic laws (interpreter-enforced)"
nix eval --impure --raw --expr "
  let d = import $here/dendritic.nix; cl = import $here/lib.nix;
      l = d.laws { game = {}; modules = [
        { mods = [ (cl.mkMod { name=\"a\"; version=\"1\"; source=cl.github \"u/a@v1\"; }) ]; }
        { mods = [ (cl.mkMod { name=\"b\"; version=\"1\"; source=cl.github \"u/b@v1\"; }) ]; }
      ]; };
  in if l.orderIndependentSet && l.identity && l.uniqueNames
     then \"  laws: orderIndependentSet + identity + uniqueNames OK\"
     else throw \"metamorphic law violated\""
echo ""
echo "dendritic check: OK"
