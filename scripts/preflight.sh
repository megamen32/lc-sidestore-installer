#!/usr/bin/env bash
# Non-destructive check: is the phone visible over usbmuxd and does remote pairing work?
# Produces a pairing file but does NOT sign or install anything, and never asks for a password.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
# DRIVER_BIN=<path> overrides the binary (used for tests and for a relocated build).
bin="${DRIVER_BIN:-$root/driver/target/release/lc-sidestore-driver}"
[[ -x "$bin" ]] || bin="$root/driver/target/debug/lc-sidestore-driver"

if [[ ! -x "$bin" ]]; then
  echo "driver binary not found; building (this takes a few minutes the first time)"
  (cd "$root/driver" && cargo build --release -j 4)
  bin="$root/driver/target/release/lc-sidestore-driver"
fi

echo "connected devices (usbmuxd):"
if command -v idevice_id >/dev/null 2>&1; then
  idevice_id -l
else
  echo "  (install libimobiledevice for a nicer listing; the driver will still work)"
fi

DRIVER_MODE=preflight \
TARGET_UDID="${TARGET_UDID:-}" \
PAIRING_OUT="${PAIRING_OUT:-$root/pairing_merged.plist}" \
  "$bin" "preflight@local" "/dev/null"

cat <<'EOF'

PREFLIGHT OK means:
  * the iPhone is unlocked, trusted by this Mac and reachable over usbmuxd;
  * Developer Mode is on (Settings > Privacy & Security > Developer Mode), which iOS 17.4+
    requires for the remote-pairing half of the file.
If it failed on remote pairing, turn Developer Mode on, reboot, unlock the phone, run again.
EOF
