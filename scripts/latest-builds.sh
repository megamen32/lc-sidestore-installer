#!/usr/bin/env bash
# Prints the newest LiveContainer / SideStore build tags and the exact IPA URLs.
# LiveContainer ships a bundle with SideStore embedded, which is what this installer needs.
set -euo pipefail

api() {
  curl -fsSL --max-time 20 -H 'Accept: application/vnd.github+json' \
    "${1%/}" 2>/dev/null || return 1
}

row() { # repo, tag-argument ('' means /latest), label
  local repo="$1" tag="$2" label="$3" url
  if [[ -n "$tag" ]]; then
    url="https://api.github.com/repos/$repo/releases/tags/$tag"
  else
    url="https://api.github.com/repos/$repo/releases/latest"
  fi
  local json name date assets
  json="$(api "$url")" || { printf '%-28s %s\n' "$label" "FETCH FAILED"; return 0; }
  name="$(printf '%s' "$json" | grep -m1 '"tag_name"' | sed -E 's/.*"tag_name"[[:space:]]*:[[:space:]]*"([^"]*)".*/\1/')"
  date="$(printf '%s' "$json" | grep -m1 '"published_at"' | sed -E 's/.*"published_at"[[:space:]]*:[[:space:]]*"([^"]*)".*/\1/')"
  printf '%-28s tag=%-28s %s\n' "$label" "${name:-?}" "${date:-?}"
  assets="$(printf '%s' "$json" | grep -oE '"browser_download_url":[[:space:]]*"[^"]+\.ipa"' | sed -E 's/.*"(https[^"]+)".*/\1/' | sort -u)"
  while IFS= read -r a; do [[ -n "$a" ]] && printf '    %s\n' "$a"; done <<<"$assets"
}

echo "LiveContainer (bundle with SideStore embedded):"
row LiveContainer/LiveContainer nightly  "LC nightly"
row LiveContainer/LiveContainer ""       "LC stable"
echo
echo "SideStore standalone IPA (not used by this installer):"
row SideStore/SideStore nightly          "SideStore nightly"
row SideStore/SideStore ""               "SideStore stable"
echo
echo "iloader (GUI alternative to this driver):"
row nab138/iloader ""                   "iloader"

cat <<'EOF'

Which one to install:
  Use the LC *nightly* "LiveContainer+SideStore.ipa". Stable bundles have carried
  SideStore 0.6.4, which cannot sign in to Apple ID any more (HTTP 503 from Apple).
  Verify what is really inside the file you downloaded with scripts/inspect-ipa.sh.
EOF
