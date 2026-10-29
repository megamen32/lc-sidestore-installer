#!/usr/bin/env bash
# Emergency fix for SideStore's "The pairing file format is ambiguous" screen
# (SideStore#1608). SideStore 0.7 loads a pairing file without telling minimuxer which
# protocol to prefer, so a merged lockdown+remote-pairing plist is rejected. This script
# splits the file into one-protocol-per-file and writes them back into the container.
#
#   scripts/fix-pairing-file.sh [path/to/merged.plist]
#
# Without an argument it pulls the merged copy SideStore already imported from the phone.
# Needs: python3 (stdlib plistlib) and pymobiledevice3.
set -euo pipefail

command -v pymobiledevice3 >/dev/null || { echo "pymobiledevice3 is required: pipx install pymobiledevice3" >&2; exit 2; }
command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 2; }

bundle="${BUNDLE:-}"
if [[ -z "$bundle" ]]; then
  # sed -n '1p' instead of head -1: head closes the pipe early and SIGPIPE + pipefail
  # would abort the script with no message at all.
  bundle="$(list_installed | grep -oE 'com\.kdt\.livecontainer\.[A-Z0-9]{10}' | sort -u | sed -n '1p')"
fi
[[ -n "$bundle" ]] || { echo "no com.kdt.livecontainer.<TEAM> app found on the device" >&2; exit 3; }
echo "container: $bundle"

work="$(mktemp -d)"; trap 'rm -rf "$work"' EXIT
src="${1:-}"
if [[ -z "$src" ]]; then
  echo "pulling the pairing file SideStore imported..."
  pymobiledevice3 apps pull "$bundle" \
    /Documents/SideStore/Documents/PairingFile_Lockdown.plist "$work/imported.plist" >/dev/null 2>&1 \
    || pymobiledevice3 apps pull "$bundle" \
       /Documents/SideStore/Documents/ALTPairingFile.mobiledevicepairing "$work/imported.plist" >/dev/null 2>&1 \
    || true
  src="$work/imported.plist"
fi
[[ -s "$src" ]] || { echo "could not read a pairing file (give the merged plist as an argument)" >&2; exit 4; }

python3 - "$src" "$work" <<'PY'
import plistlib, sys
LD = {"WiFiMACAddress","SystemBUID","RootPrivateKey","HostPrivateKey","HostID",
      "RootCertificate","UDID","EscrowBag","HostCertificate","DeviceCertificate"}
RP = {"private_key","public_key","identifier"}
d = plistlib.load(open(sys.argv[1],"rb"))
keys = set(d)
if not (keys & LD) or not (keys & RP):
    print("this file already carries a single protocol - nothing to split")
    sys.exit(0)
rp = {k:v for k,v in d.items() if k not in LD}
ld = {k:v for k,v in d.items() if k in LD}
open(sys.argv[2]+"/rp_only.plist","wb").write(plistlib.dumps(rp, fmt=plistlib.FMT_XML, sort_keys=False))
open(sys.argv[2]+"/lockdown_only.plist","wb").write(plistlib.dumps(ld, fmt=plistlib.FMT_XML, sort_keys=False))
print("split OK: rp_only", sorted(rp), "lockdown_only", len(ld), "keys")
PY

if [[ -f "$work/rp_only.plist" ]]; then
  D=/Documents/SideStore/Documents
  # house_arest refuses to open a container while the app is running, which is exactly the
  # state you are in when SideStore is showing the ambiguity error.
  if ! pymobiledevice3 apps push "$bundle" "$work/rp_only.plist" "$D/PairingFile_RemoteRP.plist" 2>"$work/err"; then
    echo "push failed: $(grep -m1 -oE 'usbmuxd rejected.*|ERROR .*' "$work/err" || true)" >&2
    echo "Force-quit LiveContainer first (app switcher, swipe it away), wait ~5s, then run this script again." >&2
    exit 5
  fi
  pymobiledevice3 apps push "$bundle" "$work/rp_only.plist"       "$D/ALTPairingFile.mobiledevicepairing"
  pymobiledevice3 apps push "$bundle" "$work/lockdown_only.plist" "$D/PairingFile_Lockdown.plist"
  echo "pushed. Now force-quit LiveContainer (swipe away) and reopen it."
  echo "SideStore > Settings > Pairing File should load without the ambiguity warning."
fi
