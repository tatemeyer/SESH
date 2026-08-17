//! The app registry: what SESH can launch, read from `apps.toml`.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// One launchable app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppSpec {
    /// Stable identifier used in URLs and event subjects.
    pub id: String,
    /// Display name shown on the TV.
    pub name: String,
    /// Program to execute. Must be on PATH inside the compositor session.
    pub command: String,
    /// Arguments passed to `command`.
    #[serde(default)]
    pub args: Vec<String>,
    /// Icon name the surface renders. Free-form; the surface decides.
    #[serde(default)]
    pub icon: String,
}

#[derive(Debug, Deserialize)]
struct AppsFile {
    #[serde(default)]
    app: Vec<AppSpec>,
}

/// Parse an app registry from TOML.
pub fn load_apps(toml_str: &str) -> Result<Vec<AppSpec>> {
    let parsed: AppsFile = toml::from_str(toml_str).context("apps registry is not valid TOML")?;

    let mut seen = std::collections::HashSet::new();
    for app in &parsed.app {
        if !seen.insert(app.id.as_str()) {
            anyhow::bail!("duplicate app id in registry: {}", app.id);
        }
    }

    Ok(parsed.app)
}

/// Read and parse an app registry from disk.
pub fn load_apps_file(path: &Path) -> Result<Vec<AppSpec>> {
    let toml_str = std::fs::read_to_string(path)
        .with_context(|| format!("reading app registry {}", path.display()))?;
    load_apps(&toml_str)
}

/// The address a phone on the LAN can reach this Pi at.
///
/// This cannot be taken from the request, which is the whole difficulty: the
/// TV fetches the join QR over `127.0.0.1`, so a QR built from the `Host`
/// header would encode loopback and be unusable by every phone that scanned it.
///
/// The trick is a *connected* UDP socket. Nothing is ever sent — connecting a
/// datagram socket only makes the kernel consult the routing table and pick
/// the source address it would use to reach that destination. On a Pi with
/// both Ethernet and Wi-Fi on one subnet, that is the interface the default
/// route prefers, which is the right answer.
pub fn detect_lan_ip() -> Option<std::net::IpAddr> {
    // A routable public address, purely as a direction to look in. No packet
    // is sent to it and it is never contacted.
    const ELSEWHERE: &str = "1.1.1.1:80";

    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect(ELSEWHERE).ok()?;
    let ip = socket.local_addr().ok()?.ip();

    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_full_app_entry() {
        let apps = load_apps(
            r#"
[[app]]
id = "kodi"
name = "Kodi"
command = "kodi"
args = ["--standalone"]
icon = "movie"
"#,
        )
        .unwrap();

        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].id, "kodi");
        assert_eq!(apps[0].name, "Kodi");
        assert_eq!(apps[0].command, "kodi");
        assert_eq!(apps[0].args, vec!["--standalone".to_string()]);
        assert_eq!(apps[0].icon, "movie");
    }

    #[test]
    fn args_and_icon_are_optional() {
        let apps = load_apps(
            r#"
[[app]]
id = "retroarch"
name = "RetroArch"
command = "retroarch"
"#,
        )
        .unwrap();

        assert!(apps[0].args.is_empty());
        assert_eq!(apps[0].icon, "");
    }

    #[test]
    fn parses_multiple_apps_in_order() {
        let apps = load_apps(
            r#"
[[app]]
id = "kodi"
name = "Kodi"
command = "kodi"

[[app]]
id = "moonlight"
name = "Moonlight"
command = "moonlight"
"#,
        )
        .unwrap();

        let ids: Vec<_> = apps.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["kodi", "moonlight"]);
    }

    #[test]
    fn an_empty_registry_is_valid() {
        assert!(load_apps("").unwrap().is_empty());
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let err = load_apps(
            r#"
[[app]]
id = "kodi"
name = "Kodi"
command = "kodi"

[[app]]
id = "kodi"
name = "Kodi Again"
command = "kodi"
"#,
        )
        .unwrap_err();

        assert!(
            err.to_string().contains("kodi"),
            "error should name the id: {err}"
        );
    }

    #[test]
    fn malformed_toml_is_rejected() {
        assert!(load_apps("this is not toml [[[").is_err());
    }

    // Deliberately tolerant: a machine with no route has nothing to find, and
    // that is a legitimate answer rather than a failure. What is asserted is
    // that when an address *is* found it is one a phone could actually use.
    #[test]
    fn any_detected_address_is_usable_by_a_phone() {
        if let Some(ip) = detect_lan_ip() {
            assert!(!ip.is_loopback(), "a QR pointing at loopback is useless");
            assert!(!ip.is_unspecified(), "0.0.0.0 is not an address to visit");
        }
    }

    #[test]
    fn the_shipped_registry_parses() {
        let toml = std::fs::read_to_string("../../deploy/apps.toml").unwrap();
        let apps = load_apps(&toml).unwrap();
        let ids: Vec<_> = apps.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["kodi", "retroarch", "moonlight"]);

        // The Debian and Flatpak packages install Moonlight as `moonlight-qt`.
        // A plain `moonlight` spawns nothing on the Pi.
        let moonlight = apps.iter().find(|a| a.id == "moonlight").unwrap();
        assert_eq!(moonlight.command, "moonlight-qt");
    }
}
