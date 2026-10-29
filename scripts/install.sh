#!/usr/bin/env bash
# Sign + install LiveContainer+SideStore, then place the remote pairing file inside the container.
#
#   scripts/install.sh <apple-id> [path/to/LiveContainer+SideStore.ipa]
#
# Environment:
#   EXPECTED_TEAM=<TEAMID>  abort unless the signing team matches; REQUIRED when the
#                           LiveContainer is already installed and holds apps you care about
#   TEAM_INDEX=<n>          pick the n-th team non-interactively
#   TARGET_UDID=<udid>      refuse to touch any other device
#   PAIRING_OUT=<path>      where the merged pairing file is written (default ./pairing_merged.plist)
#   DRIVER_BIN=<path>       use a prebuilt driver binary instead of target/{release,debug}
#   DRIVER_LOG=debug        verbose driver logs
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
app_id="${1:-}"
ipa="${2:-$root/LiveContainer+SideStore.ipa}"
[[ -n "$app_id" ]] || { echo "usage: $0 <apple-id> [ipa]" >&2; exit 2; }
[[ -f "$ipa" ]] || { echo "IPA not found: $ipa (fetch it with scripts/latest-builds.sh)" >&2; exit 2; }

bin="${DRIVER_BIN:-$root/driver/target/release/lc-sidestore-driver}"
[[ -x "$bin" ]] || bin="$root/driver/target/debug/lc-sidestore-driver"
if [[ ! -x "$bin" ]]; then
  echo "building driver (cargo build --release -j 4)"
  (cd "$root/driver" && cargo build --release -j 4)
  bin="$root/driver/target/release/lc-sidestore-driver"
fi

# Password never reaches argv and never reaches a file: hidden read, env var only.
read -rsp "Apple ID password for $app_id: " ILOADER_PASSWORD; echo

set +e
ILOADER_PASSWORD="$ILOADER_PASSWORD" \
DRIVER_MODE="${DRIVER_MODE:-full}" \
EXPECTED_TEAM="${EXPECTED_TEAM:-}" \
TEAM_INDEX="${TEAM_INDEX:-}" \
TARGET_UDID="${TARGET_UDID:-}" \
PAIRING_OUT="${PAIRING_OUT:-$root/pairing_merged.plist}" \
  "$bin" "$app_id" "$ipa"
rc=$?
set -e
unset ILOADER_PASSWORD

case "$rc" in
  0) echo "SUCCESS";;
  20) echo "FAILED: remote pairing rejected - enable Developer Mode, reboot, unlock, retry.";;
  21) echo "FAILED: signing team != EXPECTED_TEAM. Existing LiveContainer data would be orphaned.";;
  22) echo "FAILED: TARGET_UDID is not the connected device.";;
  23) echo "FAILED: Apple ID sign-in (anisette / 2FA). See troubleshooting in README.";;
  30) echo "FAILED: three-certificate limit reached; nothing was revoked.";;
  *)  echo "FAILED: driver exit $rc";;
esac
exit "$rc"
