#!/usr/bin/env bash
# Install Concierge on macOS: build release, bundle the GUI as a launchable
# Concierge.app in ~/Applications (Spotlight/Launchpad), and put the
# `concierge-cli` agent CLI on PATH. Re-runnable -- this IS the build+deploy
# step; run it after changes instead of `cargo run`.
set -euo pipefail
cd "$(dirname "$0")/.."
REPO="${PWD}"
APP="${CONCIERGE_APP_DIR:-${HOME}/Applications}/Concierge.app"
CLAUDE_DIR="$(dirname "$(command -v claude 2>/dev/null || echo /usr/local/bin/claude)")"

echo "==> building release (gui + cli + daemon)"
cargo build --release -p concierge-gui -p concierge -p concierge-daemon

echo "==> bundling ${APP}"
rm -rf "${APP}"
mkdir -p "${APP}/Contents/MacOS" "${APP}/Contents/Resources"
# The GUI binary is `concierge` now (bare launch = GUI + daemon, Vortex-style).
# Inside the bundle it keeps the `concierge-gui` name: the launcher script below
# is `Concierge`, and on a case-insensitive filesystem (macOS default) a sibling
# named `concierge` would BE the same file.
cp target/release/concierge "${APP}/Contents/MacOS/concierge-gui"
# The agent CLI rides in the bundle so the GUI's forwarding + sandbox shim and
# the daemon all find it beside the running exe.
cp target/release/concierge-cli "${APP}/Contents/MacOS/concierge-cli"
# The background download daemon lives beside the GUI so spawn-or-connect finds
# it (concierge_daemon::daemon_exe looks next to the running executable).
cp target/release/concierge-daemon "${APP}/Contents/MacOS/concierge-daemon"

# A Finder-launched .app gets a bare PATH and no repo context. The bundle's
# executable is a wrapper that sets CONCIERGE_REPO (so it finds games/) and a
# real PATH (so the AI panel's `claude` + the `concierge-cli` tool resolve),
# then execs the actual GUI binary.
cat > "${APP}/Contents/MacOS/Concierge" <<WRAP
#!/bin/bash
export CONCIERGE_REPO="${REPO}"
export PATH="\${HOME}/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:${CLAUDE_DIR}:\${PATH}"
exec "\$(dirname "\$0")/concierge-gui" "\$@"
WRAP
chmod +x "${APP}/Contents/MacOS/Concierge"

cat > "${APP}/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>Concierge</string>
  <key>CFBundleDisplayName</key><string>Concierge</string>
  <key>CFBundleIdentifier</key><string>consultancy.solomonoff.concierge</string>
  <key>CFBundleExecutable</key><string>Concierge</string>
  <key>CFBundleVersion</key><string>0.1.0</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>10.15</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>CFBundleURLTypes</key>
  <array><dict>
    <key>CFBundleURLName</key><string>Nexus Mod Manager Protocol</string>
    <key>CFBundleURLSchemes</key><array><string>nxm</string></array>
  </dict></array>
</dict></plist>
PLIST

echo "==> installing concierge-cli to ~/.cargo/bin"
mkdir -p "${HOME}/.cargo/bin"
cp target/release/concierge-cli "${HOME}/.cargo/bin/concierge-cli"

# Concierge discovers helper binaries at runtime via concierge-platform::find_tool
# (PATH / next-to-app / ~/.cargo/bin), NOT via Nix. Make sure the two it needs
# are reachable: clickhouse (catalog search) and 7zz (archive extraction).
# Prefer a real install (PATH, then Homebrew); fall back to the nix store only if
# that's the only copy present, so an existing Nix box keeps working.
provide_tool() {
  local tool="$1" found=""
  found="$(command -v "${tool}" 2>/dev/null || true)"
  [ -z "${found}" ] && [ -x "/opt/homebrew/bin/${tool}" ] && found="/opt/homebrew/bin/${tool}"
  [ -z "${found}" ] && [ -x "/usr/local/bin/${tool}" ] && found="/usr/local/bin/${tool}"
  [ -z "${found}" ] && found="$(ls -d /nix/store/*"${tool}"*/bin/"${tool}" 2>/dev/null | sort | tail -1 || true)"
  if [ -n "${found}" ] && [ -x "${found}" ]; then
    ln -sf "${found}" "${HOME}/.cargo/bin/${tool}"
    echo "    ${tool} -> ${found}"
  else
    echo "    WARNING: ${tool} not found — install it (brew install ${tool}) for full function"
  fi
}
provide_tool 7zz
# clickhouse is NOT provisioned by default — the catalog is embedded SQLite now.
# It is only needed for a one-time legacy migration (--features clickhouse-migrate).
echo "    catalog: embedded SQLite (no clickhouse needed)"

# Ad-hoc sign so Gatekeeper allows a locally-built bundle to launch cleanly.
codesign --force --deep --sign - "${APP}" >/dev/null 2>&1 || true
# Refresh Launch Services so Spotlight/Launchpad pick it up.
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "${APP}" >/dev/null 2>&1 || true

echo "OK: Concierge installed"
echo "    app: ${APP}"
echo "         launch from Spotlight/Launchpad, or:  open \"${APP}\""
echo "    cli: ~/.cargo/bin/concierge-cli   (CONCIERGE_REPO=${REPO} baked into the app)"
