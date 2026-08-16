#!/bin/sh
# Build SESH on the Pi. Run as your normal user from the repo root:
#     sh deploy/build.sh
#
# Deliberately NOT run by install.sh: building as root leaves root-owned
# artifacts in target/ and ~/.cargo, which then break the next user build.
set -eu

fail() {
    echo "error: $1" >&2
    exit 1
}

[ -f Cargo.toml ] && [ -d crates/seshd ] || fail "run this from the repo root"
[ "$(id -u)" -ne 0 ] || fail "do not run this as root — build as your normal user"

command -v cargo >/dev/null 2>&1 || fail "cargo not found. Install Rust:
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
then re-open your shell and try again."

command -v npm >/dev/null 2>&1 || fail "npm not found. Install Node 20 or newer:
  sudo apt-get install -y nodejs npm
If that gives you Node 18 or older, use nodesource or nvm instead —
the surface build requires Node 20+."

NODE_MAJOR="$(node -p 'process.versions.node.split(".")[0]' 2>/dev/null || echo 0)"
[ "$NODE_MAJOR" -ge 20 ] || fail "Node 20+ required, found $(node --version 2>/dev/null || echo none).
Use nodesource or nvm; Raspberry Pi OS Bookworm's apt Node is too old."

echo "==> Building seshd (release). First build takes a few minutes on a Pi."
cargo build --release -p seshd

echo "==> Building the surface bundle"
cd surfaces
npm ci
npm run build
cd ..

echo
echo "Built:"
echo "  target/release/seshd"
echo "  surfaces/dist/"
echo
echo "Next: sudo sh deploy/install.sh"
