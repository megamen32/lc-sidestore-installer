use std::{env, path::PathBuf};

use idevice::{
    IdeviceService, afc::opcode::AfcFopenMode, house_arrest::HouseArrestClient,
    installation_proxy::InstallationProxyClient, lockdown::LockdownClient,
    provider::IdeviceProvider,
    remote_pairing::{RemotePairingLockdownService, RpPairingFile},
    usbmuxd::{UsbmuxdAddr, UsbmuxdConnection},
};
use isideload::{
    anisette::remote_v3::RemoteV3AnisetteProvider,
    auth::apple_account::{AppleAccount, TwoFactorCallbackParams, TwoFactorCallbackResponse},
    dev::{certificates::DevelopmentCertificate, developer_session::DeveloperSession},
    sideload::{
        SideloaderBuilder, TeamSelection,
        builder::{ExtensionsBehavior, ExtensionsBehaviorSelection, MaxCertsBehavior},
    },
    util::callbacks::ExtensionsBehaviorCallbackBox,
};
use plist_macro::{plist, plist_to_xml_bytes};
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

fn read_line(prompt: &str) -> String {
    println!("{prompt}");
    let mut buf = String::new();
    let n = std::io::stdin().read_line(&mut buf).unwrap_or(0);
    if n == 0 {
        println!("[driver] stdin closed while waiting for input");
        return String::new();
    }
    let trimmed = buf.trim().to_string();
    println!("[driver] got input (len {})", trimmed.chars().count());
    trimmed
}

/// SideStore's minimuxer refuses a plist that satisfies both protocols:
/// "The pairing file format is ambiguous" (SideStore#1608). It calls
/// `start(pairingFile:mountPath:)` without a `preferred:` argument, so the file itself has
/// to carry exactly one protocol. Hence three variants instead of one merged blob.
struct PairingFiles {
    /// lockdown + remote pairing: what a Mac-side tool (pymobiledevice3, idevice, iloader) wants.
    merged: Vec<u8>,
    /// remote pairing only: what SideStore on iOS 17.4+ should load.
    rp_only: Vec<u8>,
    /// lockdown only: what SideStore loads on older iOS.
    lockdown_only: Vec<u8>,
}

/// Splits the two pairing records into the three files SideStore and Mac tooling expect.
/// Pure, so the "ambiguous format" guard can be tested without a phone.
fn make_pairing_files(
    lockdown_plist: plist::Value,
    rp_plist: plist::Value,
) -> Result<PairingFiles, String> {
    let lockdown_only = plist_to_xml_bytes(
        lockdown_plist
            .as_dictionary()
            .ok_or_else(|| "lockdown pair record is not a dictionary".to_string())?,
    );
    let rp_only = plist_to_xml_bytes(
        rp_plist
            .as_dictionary()
            .ok_or_else(|| "rppairing file is not a dictionary".to_string())?,
    );
    let merged = plist!(dict {
        :< lockdown_plist,
        :< rp_plist,
    });
    Ok(PairingFiles {
        merged: plist_to_xml_bytes(&merged),
        rp_only,
        lockdown_only,
    })
}

/// Build the merged lockdown + remote-pairing plist that SideStore needs on iOS 17.4+.
async fn build_pairing_file(
    udid: &str,
    provider: &impl IdeviceProvider,
    usbmuxd: &mut UsbmuxdConnection,
) -> Result<PairingFiles, String> {
    let mut lockdown_pair = usbmuxd
        .get_pair_record(udid)
        .await
        .map_err(|e| format!("lockdown pair record: {e}"))?;
    lockdown_pair.udid = Some(udid.to_string());
    let mut lc = LockdownClient::connect(provider)
        .await
        .map_err(|e| format!("lockdown connect: {e}"))?;
    lc.start_session(&lockdown_pair)
        .await
        .map_err(|e| format!("lockdown session: {e}"))?;
    // iloader enables wifi debugging at this point; failure is not fatal
    lc.set_value(
        "EnableWifiDebugging",
        true.into(),
        Some("com.apple.mobile.wireless_lockdown"),
    )
    .await
    .ok();
    let lockdown_plist = plist::Value::from_reader_xml(std::io::Cursor::new(
        lockdown_pair
            .serialize()
            .map_err(|e| format!("serialize lockdown: {e}"))?,
    ))
    .map_err(|e| format!("parse lockdown plist: {e}"))?;

    let hostname = format!("lcsid-{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    println!("[pairing] remote pairing hostname: {hostname}");
    let rp: RpPairingFile = async {
        let service =
            RemotePairingLockdownService::connect(provider as &dyn IdeviceProvider).await?;
        let mut client = service.into_client(&hostname)?;
        let mut pf = RpPairingFile::generate(&hostname);
        client.connect(&mut pf, async || "000000".to_string()).await?;
        Ok::<_, idevice::IdeviceError>(pf)
    }
    .await
    .map_err(|e| {
        format!("remote pairing: {e} (on iOS 17.4+ this usually means Developer Mode is off)")
    })?;
    let rp_plist = plist::Value::from_reader_xml(std::io::Cursor::new(rp.to_bytes()))
        .map_err(|e| format!("parse rppairing plist: {e}"))?;
    make_pairing_files(lockdown_plist, rp_plist)
}

#[tokio::main]
async fn main() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");
    isideload::init().expect("Failed to initialize error reporting");
    let level = match env::var("DRIVER_LOG").as_deref() {
        Ok("debug") => Level::DEBUG,
        _ => Level::INFO,
    };
    tracing::subscriber::set_global_default(
        FmtSubscriber::builder().with_max_level(level).finish(),
    )
    .expect("failed to set subscriber");

    let args: Vec<String> = env::args().collect();
    let apple_id = args
        .get(1)
        .cloned()
        .expect("usage: driver <apple-id> <ipa-path>");
    let ipa_path = PathBuf::from(args.get(2).cloned().expect("missing ipa path"));
    let mode = env::var("DRIVER_MODE").unwrap_or_else(|_| "full".to_string());
    let expected_team = env::var("EXPECTED_TEAM").unwrap_or_default();
    let target_udid = env::var("TARGET_UDID").unwrap_or_default();
    let pairing_out = env::var("PAIRING_OUT")
        .unwrap_or_else(|_| "./pairing_merged.plist".to_string());

    println!("=== STEP A: device discovery over usbmuxd (mode: {mode}) ===");
    let mut usbmuxd = UsbmuxdConnection::default()
        .await
        .expect("Failed to connect to usbmuxd");
    let devs = usbmuxd.get_devices().await.expect("no devices");
    if devs.is_empty() {
        panic!("No devices found over usbmuxd");
    }
    let mut chosen = devs.first().unwrap();
    for d in devs.iter() {
        println!("device: udid={} conn={:?}", d.udid, d.connection_type);
    }
    if !target_udid.is_empty() {
        if let Some(d) = devs.iter().find(|d| d.udid == target_udid) {
            chosen = d;
        } else {
            println!("ABORT: target udid {target_udid} not connected");
            std::process::exit(22);
        }
    }
    println!("using device: {}", chosen.udid);
    let provider = chosen.to_provider(UsbmuxdAddr::from_env_var().unwrap(), "lcsidestore-driver");

    println!("=== STEP B: build merged lockdown + remote pairing file ===");
    let files = match build_pairing_file(&chosen.udid, &provider, &mut usbmuxd).await {
        Ok(b) => b,
        Err(e) => {
            println!("PAIRING FILE FAILED: {e}");
            println!("If Developer Mode is off: Settings > Privacy & Security > Developer Mode > ON, restart phone, unlock it, re-run.");
            std::process::exit(20);
        }
    };
    std::fs::write(&pairing_out, &files.merged).expect("write pairing file");
    println!(
        "=== STEP B OK: pairing file ({} bytes, merged) at {pairing_out} ===",
        files.merged.len()
    );

    if mode == "preflight" {
        println!("PREFLIGHT OK: device reachable and remote pairing works.");
        return;
    }

    println!("=== STEP C: Apple ID sign-in (anisette via default ani.iloader.app) ===");
    let password = env::var("ILOADER_PASSWORD")
        .expect("ILOADER_PASSWORD env var not set (pass it via env, never argv)");
    let account = AppleAccount::builder(&apple_id)
        .anisette_provider(
            RemoteV3AnisetteProvider::default()
                .unwrap()
                .set_serial_number("2".to_string()),
        )
        .login(&password, async move |params: TwoFactorCallbackParams| {
            if params.unknown {
                println!("STEP C2: previous 2FA method failed; enter 'd' for a trusted-device code");
                let c = read_line("> ");
                if c == "d" {
                    return Ok(TwoFactorCallbackResponse::SendToDevices);
                }
                if c == "r" {
                    return Ok(TwoFactorCallbackResponse::ResendCode);
                }
                return Ok(TwoFactorCallbackResponse::SubmitCode(c));
            }
            if params.sms {
                println!("STEP C2: SMS 2FA requested (SMS currently returns 403 for many accounts). Enter 'd' for a trusted-device code.");
                let c = read_line("> ");
                if c == "d" {
                    return Ok(TwoFactorCallbackResponse::SendToDevices);
                }
            } else {
                println!("STEP C2: 2FA CODE REQUIRED - a 6-digit code is appearing on your Apple devices.");
            }
            let code = read_line("Enter 6-digit code (or 'r' to resend):");
            if code == "r" {
                return Ok(TwoFactorCallbackResponse::ResendCode);
            }
            Ok(TwoFactorCallbackResponse::SubmitCode(code))
        })
        .await
        .map_err(|e| {
            println!("SIGN-IN FAILED: {e:?}");
            std::process::exit(23);
        })
        .unwrap();
    let mut account = account;
    println!("=== STEP C OK: signed in ===");

    println!("=== STEP D: developer session ===");
    let dev_session = DeveloperSession::from_account(&mut account)
        .await
        .expect("Failed to create developer session");

    println!("=== STEP E: sideloader ===");
    let team_prompt = |teams: &Vec<isideload::dev::teams::DeveloperTeam>| {
        println!("STEP E2: teams available:");
        for (i, t) in teams.iter().enumerate() {
            println!(
                "  {}: {} ({})",
                i + 1,
                t.name.as_deref().unwrap_or("<Unnamed>"),
                t.team_id
            );
        }
        if let Ok(idx) = env::var("TEAM_INDEX") {
            if let Ok(sel) = idx.trim().parse::<usize>() {
                if sel > 0 && sel <= teams.len() {
                    println!("auto-selected team index {sel} from TEAM_INDEX");
                    return Some(teams[sel - 1].team_id.clone());
                }
            }
        }
        let c = read_line("Select team number:");
        let sel: usize = c.trim().parse().ok()?;
        if sel == 0 || sel > teams.len() {
            return None;
        }
        Some(teams[sel - 1].team_id.clone())
    };

    let cert_prompt = async |certs: Vec<DevelopmentCertificate>| {
        println!("STEP E3: MAX CERTIFICATES REACHED. Certificates on this account:");
        for (i, c) in certs.iter().enumerate() {
            println!(
                "  {}: {} (machine: {}) serial={}",
                i + 1,
                c.name.as_deref().unwrap_or("<Unnamed>"),
                c.machine_name.as_deref().unwrap_or("<none>"),
                c.serial_number.as_deref().unwrap_or("?")
            );
        }
        let sel = read_line("Enter numbers to REVOKE (comma separated), or blank to abort:");
        if sel.trim().is_empty() {
            println!("refusing to revoke: aborting so existing signatures are not destroyed");
            std::process::exit(30);
        }
        let ids: Vec<String> = sel
            .split(',')
            .filter_map(|s| s.trim().parse::<usize>().ok())
            .filter(|n| *n > 0 && *n <= certs.len())
            .map(|n| certs[n - 1].serial_number.clone().unwrap_or_default())
            .collect();
        Ok(Some(ids))
    };

    let mut sideloader = SideloaderBuilder::<_, ExtensionsBehaviorCallbackBox>::new(
        dev_session,
        apple_id.clone(),
    )
    .team_selection(TeamSelection::PromptOnce(team_prompt))
    .max_certs_behavior(MaxCertsBehavior::Prompt(cert_prompt))
    .extensions_behavior(ExtensionsBehaviorSelection::Fixed(
        ExtensionsBehavior::ReuseMain,
    ))
    .machine_name(env!("CARGO_PKG_NAME").to_string())
    .build();

    let team = sideloader.get_team().await.expect("no team");
    println!(
        "=== team in use: {} ({}) ===",
        team.name.as_deref().unwrap_or("?"),
        team.team_id
    );
    if !expected_team.is_empty() && team.team_id != expected_team {
        println!(
            "ABORT: team {} != expected {expected_team}. A different team would create a second LiveContainer and orphan your existing apps inside the current one.",
            team.team_id
        );
        std::process::exit(21);
    }

    println!("=== STEP F: sign + install {} ===", ipa_path.display());
    let special = sideloader
        .install_app(
            &provider,
            ipa_path.clone(),
            true,
            None::<fn(f32) -> std::future::Ready<()>>,
            Some(&files.merged),
        )
        .await
        .expect("install_app failed");
    println!("install finished, special app = {special:?}");

    println!("=== STEP G: place pairing files into the container ===");
    let mut ip = InstallationProxyClient::connect(&provider)
        .await
        .expect("installation proxy connect");
    let apps = ip.get_apps(Some("User"), None).await.expect("get apps");
    let mut lc_bundle = None;
    let mut lc_version = String::new();
    for (bid, app) in apps.iter() {
        let Some(dict) = app.as_dictionary() else {
            continue;
        };
        let name = dict
            .get("CFBundleDisplayName")
            .and_then(|v| v.as_string())
            .map(|s| s.to_string())
            .unwrap_or_default();
        let ver = dict
            .get("CFBundleShortVersionString")
            .and_then(|v| v.as_string())
            .map(|s| s.to_string())
            .unwrap_or_default();
        if name == "LiveContainer" || bid.starts_with("com.kdt.livecontainer") {
            lc_bundle = Some(bid.clone());
            lc_version = ver;
            break;
        }
    }
    let lc_bundle = lc_bundle.expect("LiveContainer not found installed");
    println!("found {lc_bundle} version {lc_version}");

    let ha = HouseArrestClient::connect(&provider)
        .await
        .expect("house arrest connect");
    let mut afc = ha
        .vend_documents(lc_bundle.clone())
        .await
        .expect("vend documents");
    afc.mk_dir("/Documents/SideStore/Documents").await.ok();

    // ALTPairingFile.mobiledevicepairing is what SideStore imports on launch; it copies the
    // content to PairingFile_RemoteRP.plist / PairingFile_Lockdown.plist. Writing all three
    // keeps the import idempotent and each file unambiguous.
    let placements: [(&str, &Vec<u8>); 3] = [
        (
            "/Documents/SideStore/Documents/ALTPairingFile.mobiledevicepairing",
            &files.rp_only,
        ),
        (
            "/Documents/SideStore/Documents/PairingFile_RemoteRP.plist",
            &files.rp_only,
        ),
        (
            "/Documents/SideStore/Documents/PairingFile_Lockdown.plist",
            &files.lockdown_only,
        ),
    ];
    for (target, bytes) in placements {
        let mut f = afc
            .open(target.to_string(), AfcFopenMode::Wr)
            .await
            .unwrap_or_else(|e| panic!("open {target}: {e}"));
        f.write_entire(bytes)
            .await
            .unwrap_or_else(|e| panic!("write {target}: {e}"));
        f.close()
            .await
            .unwrap_or_else(|e| panic!("close {target}: {e}"));
        println!("placed {} bytes at {lc_bundle}:{target}", bytes.len());
    }
    println!(
        "DONE. {lc_bundle} {lc_version} installed with embedded SideStore; pairing files placed (remote-pairing-only where SideStore reads them, so it cannot report an ambiguous format)."
    );
    println!("On the phone: force-quit LiveContainer, reopen, SideStore > Settings > Pairing File should show Remote Pairing without warnings.");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict(xml: &str) -> plist::Value {
        plist::Value::from_reader(std::io::Cursor::new(xml.as_bytes())).expect("fixture plist")
    }

    fn keys(bytes: &[u8]) -> Vec<String> {
        let mut k: Vec<String> = plist::Value::from_reader_xml(std::io::Cursor::new(bytes.to_vec()))
            .expect("output is valid plist xml")
            .as_dictionary()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        k.sort();
        k
    }

    // Mirrors the required-key lists in SideStore/minimuxer Common/PairingFile.swift.
    const RP: [&str; 3] = ["identifier", "private_key", "public_key"];
    const LD: [&str; 10] = [
        "WiFiMACAddress",
        "SystemBUID",
        "RootPrivateKey",
        "HostPrivateKey",
        "HostID",
        "RootCertificate",
        "UDID",
        "EscrowBag",
        "HostCertificate",
        "DeviceCertificate",
    ];

    fn fixture() -> (plist::Value, plist::Value) {
        let lockdown = dict(
            r#"<dict><key>UDID</key><string>00008101-000000000000001E</string>
            <key>SystemBUID</key><string>BUID</string><key>WiFiMACAddress</key><string>00:11:22:33:44:55</string>
            <key>HostID</key><string>host-uuid</string><key>EscrowBag</key><data>AAAA</data>
            <key>RootPrivateKey</key><data>AAAA</data><key>HostPrivateKey</key><data>AAAA</data>
            <key>RootCertificate</key><data>AAAA</data><key>HostCertificate</key><data>AAAA</data>
            <key>DeviceCertificate</key><data>AAAA</data></dict>"#,
        );
        let rp = dict(
            r#"<dict><key>identifier</key><string>rp-uuid</string>
            <key>public_key</key><data>AAAA</data><key>private_key</key><data>AAAA</data>
            <key>alt_irk</key><data>AAAA</data></dict>"#,
        );
        (lockdown, rp)
    }

    #[test]
    fn rp_only_file_is_not_ambiguous() {
        let (l, r) = fixture();
        let files = make_pairing_files(l, r).unwrap();
        let k = keys(&files.rp_only);
        assert!(RP.iter().all(|key| k.contains(&key.to_string())), "rp keys: {k:?}");
        for bad in LD {
            assert!(!k.contains(&bad.to_string()), "lockdown key {bad} leaked into rp_only");
        }
    }

    #[test]
    fn lockdown_only_file_is_not_ambiguous() {
        let (l, r) = fixture();
        let files = make_pairing_files(l, r).unwrap();
        let k = keys(&files.lockdown_only);
        assert!(LD.iter().all(|key| k.contains(&key.to_string())), "lockdown keys: {k:?}");
        for bad in RP {
            assert!(!k.contains(&bad.to_string()), "remote-pairing key {bad} leaked into lockdown_only");
        }
    }

    #[test]
    fn merged_file_carries_both_halves_for_mac_tooling() {
        let (l, r) = fixture();
        let files = make_pairing_files(l, r).unwrap();
        let k = keys(&files.merged);
        assert!(
            RP.iter().chain(LD.iter()).all(|key| k.contains(&key.to_string())),
            "merged keys: {k:?}"
        );
    }
}
