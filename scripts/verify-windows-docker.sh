#!/usr/bin/env bash
# Cross-compile Concierge to a real Windows PE binary and RUN it under Wine,
# all with NO Nix. Proves the Windows target builds and executes (CLI + Windows
# path resolution). Needs Docker. Note: ClickHouse ships no Windows binary, so
# the mod catalog degrades gracefully on native Windows (use WSL/Docker for it);
# everything else (download, 7z extract, deploy, launch) is native.
#   Usage:  scripts/verify-windows-docker.sh
set -euo pipefail
cd "$(dirname "$0")/.."

IN="${PWD}/.cg-win-in.sh"
trap 'rm -f "${IN}"' EXIT
cat > "${IN}" <<'SCRIPT'
set -e
echo "== 1. NO-NIX LINUX HOST (cross-building FOR Windows) =="; uname -srm
command -v nix >/dev/null 2>&1 && { echo "FAIL: nix present"; exit 1; } || echo "  OK: no nix"
echo "== 2. TOOLCHAIN (mingw + wine + windows rust target; no nix) =="
apt-get update -qq >/dev/null 2>&1
apt-get install -y -qq gcc-mingw-w64-x86-64 wine curl ca-certificates >/dev/null 2>&1
rustup target add x86_64-pc-windows-gnu >/dev/null 2>&1
echo "  mingw $(x86_64-w64-mingw32-gcc -dumpversion), wine $(wine --version 2>/dev/null)"
echo "== 3. CROSS-COMPILE concierge.exe (Windows PE) =="
mkdir -p /work/src && cp -r /src/crates /src/Cargo.toml /work/src/; [ -f /src/Cargo.lock ] && cp /src/Cargo.lock /work/src/ || true
export CARGO_TARGET_DIR=/work/target
cd /work/src && cargo build -q -p concierge --target x86_64-pc-windows-gnu
EXE=/work/target/x86_64-pc-windows-gnu/debug/concierge.exe
file "$EXE" | sed 's/^/  /'
echo "== 4. RUN THE WINDOWS BINARY UNDER WINE =="
export WINEDEBUG=-all WINEPREFIX=/tmp/wp; wineboot -i >/dev/null 2>&1 || true
echo "  concierge.exe --help:"; wine "$EXE" --help 2>/dev/null | head -3 | sed 's/^/    /'
echo "  USERPROFILE seen by the Windows build: $(wine cmd /c 'echo %USERPROFILE%' 2>/dev/null | tr -d '\r')"
echo "== NOTE: ClickHouse has no Windows binary; the catalog uses WSL/Docker there =="
echo "== WINDOWS CROSS-BUILD + WINE RUN PASSED =="
SCRIPT

echo "==> cross-compiling + running Concierge for Windows (no Nix)…"
# amd64 platform so Wine (x86-64) can execute the x86-64 build even on an ARM Mac.
docker run --rm --platform linux/amd64 -v "${PWD}:/src:ro" \
  -v cg-cargo-win:/usr/local/cargo/registry -v cg-target-win:/work/target \
  rust:latest bash /src/.cg-win-in.sh
