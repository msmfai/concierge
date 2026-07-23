#!/usr/bin/env bash
# Proof that Concierge's core loop runs on real Linux with NO Nix and NO external
# database: builds in a stock rust:latest container and searches the embedded
# SQLite catalog (state/catalog.sqlite) with clickhouse absent. Needs Docker.
set -euo pipefail
cd "$(dirname "$0")/.."
[ -f state/catalog.sqlite ] || { echo "no state/catalog.sqlite (run: concierge-cli db migrate, or db sync)"; exit 2; }
IN="${PWD}/.cg-lin-in.sh"; trap 'rm -f "${IN}"' EXIT
cat > "${IN}" <<'SCRIPT'
set -e
echo "== no nix / no clickhouse =="; uname -srm
command -v nix >/dev/null 2>&1 && { echo FAIL; exit 1; } || echo "  no nix"
command -v clickhouse >/dev/null 2>&1 && echo "  (clickhouse present, unused)" || echo "  no clickhouse (catalog is embedded SQLite)"
mkdir -p /work/src && cp -r /src/crates /src/Cargo.toml /work/src/; [ -f /src/Cargo.lock ] && cp /src/Cargo.lock /work/src/ || true
export CARGO_TARGET_DIR=/work/target; cd /work/src && cargo build -q -p concierge && echo "  built"
mkdir -p /work/ws/state /work/ws/games/skyrimse/profiles/t
cp /src/state/catalog.sqlite /work/ws/state/catalog.sqlite
cp /src/games/skyrimse/profiles/modpack/manifest.toml /work/ws/games/skyrimse/profiles/t/manifest.toml 2>/dev/null || printf '[game]\nkind="skyrimse"\npristine="/tmp"\nversion="1.6"\n' > /work/ws/games/skyrimse/profiles/t/manifest.toml
echo "== catalog search via embedded SQLite (no clickhouse) =="
PATH=/usr/bin:/bin CONCIERGE_REPO=/work/ws/games/skyrimse/profiles/t /work/target/debug/concierge-cli ai --catalog "armor" 2>&1 | head -4
cargo test -q -p concierge-db -p concierge-platform 2>&1 | grep -E "test result: ok" | tail -2
echo "== LINUX (no nix, no clickhouse) PASSED =="
SCRIPT
docker run --rm -v "${PWD}:/src:ro" -v cg-cargo:/usr/local/cargo/registry -v cg-target:/work/target rust:latest bash /src/.cg-lin-in.sh
