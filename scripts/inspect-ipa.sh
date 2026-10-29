#!/usr/bin/env bash
# Reads the LiveContainer version and the version of the SideStore embedded inside an IPA.
# Usage: scripts/inspect-ipa.sh path/to/LiveContainer+SideStore.ipa
set -euo pipefail

ipa="${1:-}"
[[ -f "$ipa" ]] || { echo "usage: $0 <path.ipa>" >&2; exit 2; }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
unzip -qq -o "$ipa" 'Payload/*/Info.plist' 'Payload/*/SideStoreApp.framework/Info.plist' -d "$tmp" 2>/dev/null || true

main="$(find "$tmp/Payload" -maxdepth 2 -name Info.plist -print -quit 2>/dev/null || true)"
if [[ -n "$main" ]]; then
  printf 'LiveContainer app:      %s %s\n' \
    "$(plutil -extract CFBundleName raw -o - "$main" 2>/dev/null || echo '?')" \
    "$(plutil -extract CFBundleShortVersionString raw -o - "$main" 2>/dev/null || echo '?')"
  plutil -extract CFBundleIdentifier raw -o - "$main" 2>/dev/null | sed 's/^/bundle id:              /'
else
  echo "LiveContainer app:      Info.plist not found (is this an IPA?)"
  exit 1
fi

ss="$(find "$tmp/Payload" -name Info.plist -path '*SideStoreApp*' -print -quit 2>/dev/null || true)"
if [[ -n "$ss" ]]; then
  v="$(plutil -extract CFBundleShortVersionString raw -o - "$ss" 2>/dev/null || echo '?')"
  printf 'SideStore embedded:     %s\n' "$v"
  case "$v" in
    0.7*|0.8*|1.*) echo "                        OK - this build can sign in to Apple ID.";;
    *) echo "                        PROBLEM - older than 0.7.0 cannot sign in (Apple returns HTTP 503). Use the nightly bundle.";;
  esac
else
  echo "SideStore embedded:     none (LiveContainer-only IPA; SideStore must be installed separately)"
fi
