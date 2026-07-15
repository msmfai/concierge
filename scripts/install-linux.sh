#!/usr/bin/env bash
# Install Concierge on Linux: build release, put a launcher on PATH, install a
# desktop entry, and make sure the helper binaries are reachable. No Nix.
# Re-runnable -- this IS the build+deploy step on Linux.
set -euo pipefail
cd "$(dirname "$0")/.."
REPO="${PWD}"
BINDIR="${HOME}/.local/bin"
APPS="${XDG_DATA_HOME:-${HOME}/.local/share}/applications"

echo "==> building release (gui + cli)"
cargo build --release -p concierge-gui -p concierge

echo "==> installing binaries to ${BINDIR}"
mkdir -p "${BINDIR}" "${APPS}"
cp target/release/concierge "${BINDIR}/concierge"

# A menu-launched app has no repo cwd, so a tiny wrapper bakes CONCIERGE_REPO in.
# (Tools are found by concierge-platform::find_tool via PATH — no PATH hack.)
cat > "${BINDIR}/concierge-gui" <<WRAP
#!/usr/bin/env bash
export CONCIERGE_REPO="${REPO}"
exec "${REPO}/target/release/concierge-gui" "\$@"
WRAP
chmod +x "${BINDIR}/concierge-gui"

# Helper binaries: Concierge discovers clickhouse/7zz on PATH (or ~/.local/bin).
# Prefer a distro package; symlink whatever is found so find_tool locates it.
for tool in clickhouse 7zz; do
  found="$(command -v "${tool}" 2>/dev/null || true)"
  if [ -n "${found}" ]; then
    ln -sf "${found}" "${BINDIR}/${tool}"
    echo "    ${tool} -> ${found}"
  else
    echo "    NOTE: ${tool} not found — install it (e.g. apt install p7zip-full for 7zz;"
    echo "          see clickhouse.com/docs/install for clickhouse) for full function"
  fi
done

cat > "${APPS}/concierge.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Concierge
Comment=Declarative mod manager
Exec=${BINDIR}/concierge-gui
Terminal=false
Categories=Game;Utility;
DESKTOP
update-desktop-database "${APPS}" >/dev/null 2>&1 || true

echo "OK: Concierge installed"
echo "    launcher: ${BINDIR}/concierge-gui  (also in your app menu)"
echo "    cli:      ${BINDIR}/concierge       (ensure ${BINDIR} is on PATH)"
