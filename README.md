# lc-sidestore-installer

Headless installer for **LiveContainer + embedded SideStore** on a physical iPhone, plus the
part people usually get wrong: it performs **remote pairing**, writes the pairing file into the
LiveContainer document directory in the one-protocol-per-file shape SideStore 0.7 demands, and
therefore lets the SideStore running inside LiveContainer talk to the phone and re-sign apps
**without a Mac and without weekly re-signing**.

It is a ~350-line Rust driver on top of
[`nab138/isideload`](https://github.com/nab138/isideload) (the engine of
[`nab138/iloader`](https://github.com/nab138/iloader)) and
[`jkcoxson/idevice`](https://github.com/jkcoxson/idevice-rs). If you prefer a GUI, use iloader —
this repo exists because iloader's GUI could not be driven from a terminal and its pairing-file
placement had to be reproduced by hand.

## What you end up with

`LiveContainer+SideStore.ipa` installed and signed with your free Apple ID. One IPA = **one of
the three app slots** a free account gets. Everything you install *inside* LiveContainer does not
consume another slot, and the embedded SideStore refreshes it over the network on its own timer.

## Requirements

- macOS or Linux host with the phone plugged in over USB; `usbmuxd` running (comes with
  libimobiledevice / Homebrew's `usbmuxd`, and with iTunes on Windows).
- iPhone on iOS 17.4 or newer (this project was verified on iOS 26.x, where plain lockdown
  pairing files are rejected and remote pairing is mandatory).
- **Developer Mode** enabled: Settings → Privacy & Security → Developer Mode → ON → reboot →
  unlock → "Trust".
- Your Apple ID **password** (not an app-specific password) and a **trusted Apple device** that
  can show the 2FA code. SMS 2FA is currently rejected with HTTP 403 by Apple's SRP endpoints.
- Rust 1.85+ (edition 2024).

## Quick start

```bash
git clone https://github.com/megamen32/lc-sidestore-installer.git
cd lc-sidestore-installer

# 1. which build should I download?  (prints the nightly IPA URL)
./scripts/latest-builds.sh

# 2. download the *nightly* bundle and check what is actually inside it
curl -L -o LiveContainer+SideStore.ipa \
  'https://github.com/LiveContainer/LiveContainer/releases/download/nightly/LiveContainer%2BSideStore.ipa'
./scripts/inspect-ipa.sh LiveContainer+SideStore.ipa

# 3. is the phone visible and is remote pairing possible?  (no password, no install)
./scripts/preflight.sh

# 4. sign + install + place the pairing file
./scripts/install.sh "you@icloud.com" LiveContainer+SideStore.ipa
```

Step 4 asks for the Apple ID password with hidden input, and later for the 6-digit code that
appears on your other Apple devices. If a trusted device only offered "Send Code" again, type
`d` to push the prompt to the devices, or `r` to resend.

Never pass the password in `argv` or an env var in a shell history file — the scripts read it
with `read -rsp` and hand it to the process environment only.

## Why the nightly bundle, and not the stable one

Stable LiveContainer 3.8.0 embeds SideStore **0.6.4**, and SideStore ≤ 0.6.4 cannot sign in to
Apple ID any more: Apple changed the auth endpoints and the old client gets HTTP 503. The
nightly bundle embeds SideStore 0.7.0-`<date>` which works. `scripts/inspect-ipa.sh` exists
precisely to check the embedded version before you burn one of your three slots:

```
LiveContainer app:      LiveContainer 3.8.11
SideStore embedded:     0.7.0-20261017.511+ee3cc16a
                        OK - this build can sign in to Apple ID.
```

## Scripts

| Script | What it does |
| --- | --- |
| `scripts/latest-builds.sh` | Queries GitHub for the newest LiveContainer/SideStore/iloader builds and prints the IPA URLs. |
| `scripts/inspect-ipa.sh <ipa>` | Shows the LiveContainer version and the version of the SideStore embedded in that IPA. |
| `scripts/preflight.sh` | Device + Developer Mode + remote pairing check. No password, no install. |
| `scripts/install.sh <apple-id> [ipa]` | Full run: pair, sign in, sign, install, place pairing files. |
| `scripts/fix-pairing-file.sh [merged.plist]` | Repairs an already-installed SideStore that is stuck on "pairing file format is ambiguous". |

## Environment variables

| Variable | Meaning |
| --- | --- |
| `TARGET_UDID` | Refuse to run unless this exact device is connected. Use it whenever more than one Apple device is plugged in. |
| `EXPECTED_TEAM` | Abort (exit 21) unless the signing team matches. **Set this when LiveContainer is already installed** — see below. |
| `TEAM_INDEX` | Pick the n-th developer team without a prompt. |
| `PAIRING_OUT` | Where the merged pairing file is written (default `./pairing_merged.plist`). |
| `DRIVER_MODE=preflight` | Stop after building the pairing file; no sign-in, no install. |
| `DRIVER_BIN` | Run a prebuilt binary from a non-standard location. |
| `DRIVER_LOG=debug` | Verbose tracing. |
| `ILOADER_PASSWORD` | Read by the driver; set it via the environment of a single command, not via a file. |

### The team-ID guard is not decoration

LiveContainer installs as `com.kdt.livecontainer.<TEAMID>`. Re-signing it with a **different**
free-account team creates a *second* LiveContainer container and orphans every app you had
inside the first one. Read the team out of the installed bundle id and pass it as
`EXPECTED_TEAM`; the driver aborts otherwise.

```bash
ideviceinstaller list --user | grep livecontainer      # com.kdt.livecontainer.XXXXXXX
```

### Certificate limit

A free account allows 2 development certificates. When the limit is hit, the driver prints the
certificates on the account and asks for the serial numbers to revoke; **blank input aborts with
exit 30** instead of silently destroying the signature that keeps your other apps alive.

## Exit codes

| Code | Meaning |
| --- | --- |
| 20 | Remote pairing failed — almost always Developer Mode off. |
| 21 | Signing team ≠ `EXPECTED_TEAM`; nothing was installed. |
| 22 | `TARGET_UDID` is not the connected device. |
| 23 | Apple ID sign-in failed (anisette / 2FA). |
| 30 | Max certificates; refused to revoke. |

## After the install: on the phone

1. Install **LocalDevVPN** from the App Store and leave it *connected*. It provides the network
   extension tunnel SideStore uses to reach the phone's own services. Without it you get
   "no ipsec/IKEv2 interface found" style failures.
2. Open LiveContainer → SideStore → Settings → **Pairing File**. The driver already wrote
   protocol-pure copies into
   `com.kdt.livecontainer…:/Documents/SideStore/Documents/`, so this should just be green.
   See [the ambiguous-format trap](#the-ambiguous-pairing-file-trap) if it is not.
3. SideStore → Account → sign in with the same Apple ID. If it fails, see
   [`-80045`](#-80045-the-anisette-wave) below before retrying the password.
4. **Resign SideStore → App Contains Extensions** → choose
   **Keep App Extensions (Use Main Profile)**. A free account is allowed 10 new App IDs per week;
   the bundle carries 4 extensions, and "Register App ID for Each Extension" burns 4 of those 10
   on every re-sign. "Use Main Profile" costs one, which is also what the driver did on the Mac
   (`ExtensionsBehavior::ReuseMain`).
5. SideStore → **Refresh All**, and confirm the apps inside LiveContainer get new certificates.
6. Only after step 5 works, delete AltStore (or any other signer) to free the slot. Apps it
   signed keep working: SideStore sees every user app on the device and refreshes anything signed
   with your own certificate.

## The ambiguous pairing file trap

SideStore 0.7 loads the pairing file through minimuxer with `preferred: nil`
(`SideStore/Core/DeviceApi/MinimuxerWrapper.swift` → `minimuxer.core.start(pairingFile:mountPath:)`),
and `PairingFileParser.validatePairingFile` throws as soon as a plist satisfies *both* key sets:

> The pairing file format is ambiguous: The pairing file contains credentials for both Remote
> Pairing and Lockdown protocols. Specify a preferred protocol to resolve ambiguity.

Every classic tool — iloader included — writes exactly such a merged file, so the newest
SideStore rejects the thing that used to be the recommended setup
([SideStore#1608](https://github.com/SideStore/SideStore/issues/1608)). The fix is one protocol
per file, which is what the driver writes:

| File in `/Documents/SideStore/Documents/` | Content |
| --- | --- |
| `ALTPairingFile.mobiledevicepairing` | remote-pairing only (`identifier`, `public_key`, `private_key`) |
| `PairingFile_RemoteRP.plist` | same, remote-pairing only — what SideStore uses on iOS 17.4+ |
| `PairingFile_Lockdown.plist` | lockdown only (the 10 classic keys) — legacy path |
| `pairing_merged.plist` (on the computer) | both halves, for Mac-side tools |

Already stuck on that screen? `scripts/fix-pairing-file.sh` pulls the merged file out of the
container, splits it and pushes the pure copies back. Force-quit LiveContainer first:
`house_arrest` refuses to open a container while the app is running.

## `-80045` (the anisette wave)

Since mid-October 2026 signing in fails with *"This action cannot be completed at this time.
(-80045)"* while the password is correct
([SideStore#1675](https://github.com/SideStore/SideStore/issues/1675),
[#1658](https://github.com/SideStore/SideStore/issues/1658)). Confirmed working sequence, done
inside SideStore:

1. **User Customizations → On-Device Anisette: OFF** (restart the app when asked).
2. **Anisette servers → SideStore (New)**, then **disable "Enable auto rotation"** at the bottom,
   otherwise the provider rotates back to a broken one.
3. **User Customizations → Anisette client configuration → Remote server sync → Fetch → Save as
   overrides**.
4. **User Customizations → Reset adi.pb**.
5. Sign in again; the first attempt can take noticeably longer than usual.

## Troubleshooting

| Symptom | Cause / fix |
| --- | --- |
| `No devices found over usbmuxd` | Phone locked, "Trust This Computer" not tapped, or `usbmuxd` not running. `brew services start usbmuxd`. |
| `remote pairing: … (Developer Mode is off)` | Settings → Privacy & Security → Developer Mode → ON, reboot, unlock, re-run `preflight`. |
| "The pairing file format is ambiguous" | Merged file; run `scripts/fix-pairing-file.sh` (see above). |
| HTTP 503 during sign-in | SideStore 0.6.4 embedded (see above) or a stale anisette state — re-install the nightly bundle / reset anisette. |
| `-80045` / `22411` on sign-in or refresh | The anisette wave — follow the five steps above. |
| SMS 2FA returns 403 | Use a trusted-device code (`d` at the prompt). |
| `usbmuxd rejected the Connect request: {'Number': 2}` on push | LiveContainer is running; force-quit it and retry. |
| Second LiveContainer appeared, apps missing | You signed with a different team; re-install with the original `EXPECTED_TEAM`. |
| Apps expired after 7 days | SideStore never refreshed: LocalDevVPN off, pairing file missing, or account not signed in inside SideStore. |

## Security notes

- The repo's `.gitignore` excludes pairing files, `*.mobiledevicepairing`, `ALTPairingFile*`,
  `anisette_state`, certificates, `.p12`, IPAs and logs. A pairing file is a device credential —
  treat it like a key.
- The driver contacts Apple and the remote anisette server `https://ani.iloader.app` (isideload's
  default). If that is unacceptable, use iloader's other providers; the serial number is set to
  `2` in `driver/src/main.rs`.
- Free-account sideloading is limited to 3 apps and 7-day certificates; automating around
  Apple's restrictions is your responsibility with respect to the Apple Developer Agreement.

## Build from source

```bash
cd driver && cargo build --release -j 4
```

Dependencies are pinned: `isideload` to git rev `326e56229d0727cb6e7b34c8cf03e37e2cd3e17d`
(the revision whose Grand Slam / anisette handling matches iloader 2.4.x) and `idevice` 0.1.61+.
Cargo.lock is committed so the build is reproducible.

```bash
cd driver && cargo test -j 4
```

The three tests cover the pairing-file split (remote-pairing-only has no lockdown keys,
lockdown-only has no remote-pairing keys, merged has both) — the invariant that keeps SideStore
from reporting an ambiguous format. They need no device.

## Credits

- [nab138/isideload](https://github.com/nab138/isideload) — sideloading, Grand Slam auth, certificates.
- [nab138/iloader](https://github.com/nab138/iloader) — the GUI whose pairing-file recipe this reproduces.
- [jkcoxson/idevice](https://github.com/jkcoxson/idevice) — lockdown, AFC, house_arrest, installation_proxy, remote pairing.
- [LiveContainer](https://github.com/LiveContainer/LiveContainer) and [SideStore](https://github.com/SideStore/SideStore).

MIT licensed. Not affiliated with Apple.
